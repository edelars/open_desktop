//! Command-level tests of slice A of phase B (S7-23): hierarchies, HMAC sessions, `FlushContext`,
//! NV indices. Every command goes through `Tpm::execute` with raw bytes, like `tests.rs`.
//!
//! The session HMACs are computed here by a small client written from the formulas of Part 1
//! v184 (clauses 16.7, 16.8, 17.6.5, 17.6.8-17.6.10) and not by the production helpers; one test
//! additionally pins a whole exchange against values computed independently with Python
//! `hmac`/`hashlib` (2026-11-07). Response codes follow Part 2 v184, Table 20, with the index
//! rules of clause 6.6.2; codes that the open text does not give are listed as NOT VERIFIED in
//! `docs/specs/VTPM.md` and in the source of the command.

use sha2::{Digest, Sha256};

use super::*;
use crate::codes::{se, session_attr};
use crate::nv::{MAX_NV_INDEX_SIZE, NV_BUFFER_MAX, NV_POOL_SIZE, attr};

const OWNER: u32 = handle::RH_OWNER;
const PLATFORM: u32 = handle::RH_PLATFORM;
const ENDORSEMENT: u32 = handle::RH_ENDORSEMENT;
pub(super) const LOCKOUT: u32 = handle::RH_LOCKOUT;
const NULL: u32 = handle::RH_NULL;
pub(super) const NV0: u32 = 0x0100_0000;

pub(super) fn pw_cmd(t: &mut Tpm, code: u32, handles: &[u32], pw: &[u8], params: &[u8]) -> Vec<u8> {
    run(t, with_pw(code, handles, &[pw], params))
}

pub(super) fn set_auth(t: &mut Tpm, who: u32, old: &[u8], new: &[u8]) -> Vec<u8> {
    pw_cmd(t, cc::HIERARCHY_CHANGE_AUTH, &[who], old, &tpm2b(new))
}

pub(super) fn hierarchy_control(
    t: &mut Tpm,
    auth: u32,
    pw: &[u8],
    enable: u32,
    state: u8,
) -> Vec<u8> {
    pw_cmd(
        t,
        cc::HIERARCHY_CONTROL,
        &[auth],
        pw,
        &cat(&[&u32b(enable), &[state]]),
    )
}

fn startup_clear_attributes(t: &mut Tpm) -> u32 {
    let (_, _, body, _) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, pt::STARTUP_CLEAR, 1));
    assert_eq!(&body[..4], &u32b(pt::STARTUP_CLEAR)[..]);
    u32::from_be_bytes([body[4], body[5], body[6], body[7]])
}

pub(super) fn permanent_attributes(t: &mut Tpm) -> u32 {
    let (_, _, body, _) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, pt::PERMANENT, 1));
    assert_eq!(&body[..4], &u32b(pt::PERMANENT)[..]);
    u32::from_be_bytes([body[4], body[5], body[6], body[7]])
}

pub(super) fn startup(t: &mut Tpm, kind: u16) {
    ok(&run(t, plain(cc::STARTUP, &[], &u16b(kind))));
}

pub(super) fn shutdown(t: &mut Tpm, kind: u16) {
    ok(&run(t, plain(cc::SHUTDOWN, &[], &u16b(kind))));
}

// ---------------------------------------------------------------- HMAC session client

/// Client side of an HMAC session (Part 1 v184, clauses 17.6.5-17.6.10).
pub(super) struct Hs {
    pub(super) handle: u32,
    /// `sessionKey`: empty for an unbound, unsalted session.
    pub(super) key: Vec<u8>,
    pub(super) nonce_tpm: Vec<u8>,
    /// `(handle, authValue)` the session was bound to.
    pub(super) bound: Option<(u32, Vec<u8>)>,
}

pub(super) fn trimmed(v: &[u8]) -> Vec<u8> {
    let end = v.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    v[..end].to_vec()
}

/// KDFa written out here: SP 800-108 counter mode (Part 1, clause 9.4.10.2) for one block.
pub(super) fn kdfa_256(key: &[u8], label: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(label);
    data.push(0);
    data.extend_from_slice(u);
    data.extend_from_slice(v);
    data.extend_from_slice(&256u32.to_be_bytes());
    crate::crypto::hmac_sha256(key, &[&data]).to_vec()
}

pub(super) fn start_params(
    nonce_caller: &[u8],
    salt: &[u8],
    kind: u8,
    sym: &[u8],
    hash: u16,
) -> Vec<u8> {
    cat(&[
        &tpm2b(nonce_caller),
        &tpm2b(salt),
        &[kind],
        sym,
        &u16b(hash),
    ])
}

pub(super) fn start_cmd(key: u32, bind: u32, params: &[u8]) -> Vec<u8> {
    plain(cc::START_AUTH_SESSION, &[key, bind], params)
}

pub(super) fn start_hmac(t: &mut Tpm, bind: u32, bind_auth: &[u8], nonce_caller: &[u8]) -> Hs {
    start_session(t, se::HMAC, bind, bind_auth, nonce_caller)
}

/// Starts a session of type `kind` (`TPM_SE_*`); a policy session is never bound (the client
/// keeps `bound` empty and uses the session key only as the key derivation of `bind`).
pub(super) fn start_session(
    t: &mut Tpm,
    kind: u8,
    bind: u32,
    bind_auth: &[u8],
    nonce_caller: &[u8],
) -> Hs {
    let resp = run(
        t,
        start_cmd(
            NULL,
            bind,
            &start_params(nonce_caller, &[], kind, &u16b(alg::NULL), alg::SHA256),
        ),
    );
    ok(&resp);
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(&resp[14..]);
    let nonce_tpm = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    assert_eq!(nonce_tpm.len(), nonce_caller.len());
    let (key, bound) = if bind == NULL {
        (Vec::new(), None)
    } else {
        let auth = trimmed(bind_auth);
        (
            kdfa_256(&auth, b"ATH", &nonce_tpm, nonce_caller),
            (kind == se::HMAC).then_some((bind, auth)),
        )
    };
    Hs {
        handle,
        key,
        nonce_tpm,
        bound,
    }
}

pub(super) fn name_of(h: u32) -> Vec<u8> {
    h.to_be_bytes().to_vec()
}

/// Options of one HMAC-authorized command.
pub(super) struct Hc {
    pub(super) code: u32,
    pub(super) handles: Vec<u32>,
    pub(super) names: Vec<Vec<u8>>,
    pub(super) params: Vec<u8>,
    /// Handle and authValue of the entity that the session authorizes.
    pub(super) entity: (u32, Vec<u8>),
    pub(super) attrs: u8,
    pub(super) nonce_caller: Vec<u8>,
    /// authValue of the entity after the command (for the response HMAC).
    pub(super) entity_after: Vec<u8>,
}

pub(super) fn hmac_auth_entry(
    s: &Hs,
    c: &Hc,
    parameter_nonces: &[&[u8]],
    empty_hmac: bool,
    corrupt: bool,
) -> Vec<u8> {
    let mut cp = Sha256::new();
    cp.update(c.code.to_be_bytes());
    for n in &c.names {
        cp.update(n);
    }
    cp.update(&c.params);
    let cp = cp.finalize();
    let auth = trimmed(&c.entity.1);
    let bound_here = s
        .bound
        .as_ref()
        .is_some_and(|b| b.0 == c.entity.0 && b.1 == auth);
    let mut key = s.key.clone();
    if !bound_here {
        key.extend_from_slice(&auth);
    }
    let mut data = cp.to_vec();
    data.extend_from_slice(&c.nonce_caller);
    data.extend_from_slice(&s.nonce_tpm);
    for nonce in parameter_nonces {
        data.extend_from_slice(nonce);
    }
    data.push(c.attrs);
    let mut mac = crate::crypto::hmac_sha256(&key, &[&data]).to_vec();
    if corrupt {
        mac[0] ^= 1;
    }
    if empty_hmac {
        mac.clear();
    }
    let mut area = Vec::new();
    area.extend_from_slice(&s.handle.to_be_bytes());
    area.extend_from_slice(&tpm2b(&c.nonce_caller));
    area.push(c.attrs);
    area.extend_from_slice(&tpm2b(&mac));
    area
}

pub(super) fn hmac_command(s: &Hs, c: &Hc, empty_hmac: bool, corrupt: bool) -> Vec<u8> {
    let area = hmac_auth_entry(s, c, &[], empty_hmac, corrupt);
    let mut auth_area = (area.len() as u32).to_be_bytes().to_vec();
    auth_area.extend_from_slice(&area);
    frame(st::SESSIONS, c.code, &c.handles, &auth_area, &c.params)
}

/// Splits a successful response with sessions: (parameters, [(nonce, attrs, hmac)]).
/// `(nonceTPM, sessionAttributes, hmac)` of one acknowledgement.
type Ack = (Vec<u8>, u8, Vec<u8>);

pub(super) fn split_response(resp: &[u8], nh: usize) -> (Vec<u8>, Vec<Ack>) {
    ok(resp);
    assert_eq!(u16::from_be_bytes([resp[0], resp[1]]), st::SESSIONS);
    let at = 10 + 4 * nh;
    let n = u32::from_be_bytes([resp[at], resp[at + 1], resp[at + 2], resp[at + 3]]) as usize;
    let params = resp[at + 4..at + 4 + n].to_vec();
    let mut r = Reader::new(&resp[at + 4 + n..]);
    let mut acks = Vec::new();
    while r.remaining() > 0 {
        let nonce = r.tpm2b(64).unwrap().to_vec();
        let attrs = r.u8().unwrap();
        let hmac = r.tpm2b(64).unwrap().to_vec();
        acks.push((nonce, attrs, hmac));
    }
    (params, acks)
}

/// Runs an HMAC-authorized command, checks the response HMAC and rolls the client nonce.
pub(super) fn run_hmac(t: &mut Tpm, s: &mut Hs, c: &Hc) -> Vec<u8> {
    let resp = run(t, hmac_command(s, c, false, false));
    if rc(&resp) != 0 {
        return resp;
    }
    let (params, acks) = split_response(&resp, 0);
    assert_eq!(acks.len(), 1);
    check_hmac_ack(s, c, &params, &acks[0]);
    resp
}

pub(super) fn check_hmac_ack(s: &mut Hs, c: &Hc, params: &[u8], ack: &Ack) {
    let (nonce, attrs, hmac) = ack;
    assert_eq!(nonce.len(), s.nonce_tpm.len());
    assert_ne!(nonce, &s.nonce_tpm, "the TPM rolls its nonce");
    assert_eq!(*attrs, c.attrs);
    let mut rp = Sha256::new();
    rp.update(0u32.to_be_bytes());
    rp.update(c.code.to_be_bytes());
    rp.update(params);
    let mut data = rp.finalize().to_vec();
    data.extend_from_slice(nonce);
    data.extend_from_slice(&c.nonce_caller);
    data.push(*attrs);
    let auth = trimmed(&c.entity.1);
    let bound_here = s
        .bound
        .as_ref()
        .is_some_and(|b| b.0 == c.entity.0 && b.1 == auth);
    let mut key = s.key.clone();
    if !bound_here {
        key.extend_from_slice(&trimmed(&c.entity_after));
    }
    assert_eq!(
        hmac,
        &crate::crypto::hmac_sha256(&key, &[&data]).to_vec(),
        "response HMAC"
    );
    s.nonce_tpm = nonce.clone();
}

pub(super) fn hc(code: u32, handles: &[u32], params: Vec<u8>, entity: (u32, &[u8])) -> Hc {
    Hc {
        code,
        handles: handles.to_vec(),
        names: handles.iter().map(|h| name_of(*h)).collect(),
        params,
        entity: (entity.0, entity.1.to_vec()),
        attrs: session_attr::CONTINUE_SESSION,
        nonce_caller: vec![0x33; 16],
        entity_after: entity.1.to_vec(),
    }
}

// ---------------------------------------------------------------- NV helpers

pub(super) fn nv_public(
    index: u32,
    name_alg: u16,
    attributes: u32,
    policy: &[u8],
    size: u16,
) -> Vec<u8> {
    cat(&[
        &u32b(index),
        &u16b(name_alg),
        &u32b(attributes),
        &tpm2b(policy),
        &u16b(size),
    ])
}

pub(super) fn define_raw(t: &mut Tpm, who: u32, pw: &[u8], auth: &[u8], public: &[u8]) -> Vec<u8> {
    pw_cmd(
        t,
        cc::NV_DEFINE_SPACE,
        &[who],
        pw,
        &cat(&[&tpm2b(auth), &tpm2b(public)]),
    )
}

pub(super) fn define(t: &mut Tpm, who: u32, index: u32, attributes: u32, size: u16) -> Vec<u8> {
    define_raw(
        t,
        who,
        b"",
        b"",
        &nv_public(index, alg::SHA256, attributes, &[], size),
    )
}

/// Attribute sets used by most tests.
const OWNER_RW: u32 = attr::OWNERWRITE | attr::OWNERREAD;
const PLATFORM_CREATED_RW: u32 = attr::PPWRITE | attr::PPREAD | attr::PLATFORMCREATE;

pub(super) fn nv_write(
    t: &mut Tpm,
    auth: u32,
    index: u32,
    pw: &[u8],
    data: &[u8],
    offset: u16,
) -> Vec<u8> {
    pw_cmd(
        t,
        cc::NV_WRITE,
        &[auth, index],
        pw,
        &cat(&[&tpm2b(data), &u16b(offset)]),
    )
}

pub(super) fn nv_read(
    t: &mut Tpm,
    auth: u32,
    index: u32,
    pw: &[u8],
    size: u16,
    offset: u16,
) -> Vec<u8> {
    pw_cmd(
        t,
        cc::NV_READ,
        &[auth, index],
        pw,
        &cat(&[&u16b(size), &u16b(offset)]),
    )
}

pub(super) fn nv_read_data(resp: &[u8]) -> Vec<u8> {
    let p = params(resp, 0);
    let mut r = Reader::new(p);
    let d = r.tpm2b(NV_BUFFER_MAX).unwrap().to_vec();
    r.finish().unwrap();
    d
}

/// `(public area, Name)` of an index from `TPM2_NV_ReadPublic`.
pub(super) fn nv_read_public(t: &mut Tpm, index: u32) -> (Vec<u8>, Vec<u8>) {
    let r = run(t, plain(cc::NV_READ_PUBLIC, &[index], &[]));
    let mut rd = Reader::new(params(&r, 0));
    let public = rd.tpm2b(1024).unwrap().to_vec();
    let name = rd.tpm2b(64).unwrap().to_vec();
    rd.finish().unwrap();
    (public, name)
}

pub(super) fn nv_attributes(t: &mut Tpm, index: u32) -> u32 {
    let (public, _) = nv_read_public(t, index);
    u32::from_be_bytes([public[6], public[7], public[8], public[9]])
}

pub(super) fn handles_of(t: &mut Tpm, property: u32) -> Vec<u32> {
    let (more, _, body, _) = cap_list(&get_cap(t, cap::HANDLES, property, 256));
    assert!(!more);
    body.chunks(4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

// ================================================================ hierarchies

#[test]
fn changing_a_hierarchy_auth_makes_the_old_value_useless() {
    for who in [OWNER, ENDORSEMENT, PLATFORM, LOCKOUT] {
        let mut t = started();
        ok(&set_auth(&mut t, who, b"", b"first"));
        // A wrong value of lockoutAuth is DA protected: TPM_RC_AUTH_FAIL, and lockoutAuth is
        // disabled until lockoutRecovery has passed (`da.rs`); the others are TPM_RC_BAD_AUTH.
        let wrong = if who == LOCKOUT {
            Rc::AUTH_FAIL
        } else {
            Rc::BAD_AUTH
        };
        // The empty password no longer authorizes; the new one does.
        err(&set_auth(&mut t, who, b"", b"second"), wrong.in_session(1));
        pass_time(&mut t, 86_400);
        err(
            &set_auth(&mut t, who, b"firsT", b"second"),
            wrong.in_session(1),
        );
        pass_time(&mut t, 86_400);
        ok(&set_auth(&mut t, who, b"first", b"second"));
        err(
            &set_auth(&mut t, who, b"first", b"third"),
            wrong.in_session(1),
        );
        pass_time(&mut t, 86_400);
        // Changing back to the empty value.
        ok(&set_auth(&mut t, who, b"second", b""));
        ok(&set_auth(&mut t, who, b"", b""));
    }
}

#[test]
fn hierarchy_auth_values_are_independent_and_drop_trailing_zeros() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    // The other hierarchies are untouched.
    ok(&set_auth(&mut t, ENDORSEMENT, b"", b"x"));
    ok(&set_auth(&mut t, PLATFORM, b"", b"y"));
    ok(&set_auth(&mut t, LOCKOUT, b"", b"z"));
    // A value with trailing zeros is stored without them (Part 1, clause 17.6.5).
    ok(&set_auth(&mut t, OWNER, b"own", b"abc\0\0"));
    ok(&set_auth(&mut t, OWNER, b"abc", b"abc"));
    ok(&set_auth(&mut t, OWNER, b"abc\0\0\0", b"abc"));
    // The largest value is one digest, the next is TPM_RC_SIZE for parameter 1.
    ok(&set_auth(&mut t, OWNER, b"abc", &[0x7A; 32]));
    err(
        &set_auth(&mut t, OWNER, &[0x7A; 32], &[1; 33]),
        Rc::SIZE.in_param(1),
    );
    ok(&set_auth(&mut t, OWNER, &[0x7A; 32], b""));
}

#[test]
fn hierarchy_change_auth_errors() {
    let mut t = started();
    // Handle types of TPMI_RH_HIERARCHY_AUTH minus the null hierarchy.
    for h in [NULL, 0x4000_0002, handle::RS_PW, 0x0000_0001, 0x8000_0000] {
        err(&set_auth(&mut t, h, b"", b"a"), Rc::VALUE.in_handle(1));
    }
    // No session / two sessions / a session that is not loaded.
    err(
        &run(
            &mut t,
            plain(cc::HIERARCHY_CHANGE_AUTH, &[OWNER], &tpm2b(b"a")),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::HIERARCHY_CHANGE_AUTH,
                &[OWNER],
                &[b"", b""],
                &tpm2b(b"a"),
            ),
        ),
        Rc::ATTRIBUTES.in_session(2),
    );
    let area = {
        let mut a = Vec::new();
        a.extend_from_slice(&0x0200_0003u32.to_be_bytes());
        a.extend_from_slice(&tpm2b(&[]));
        a.push(1);
        a.extend_from_slice(&tpm2b(&[]));
        let mut v = (a.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(&a);
        v
    };
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::HIERARCHY_CHANGE_AUTH,
                &[OWNER],
                &area,
                &tpm2b(b"a"),
            ),
        ),
        Rc::reference_session(0),
    );
    // Parameter errors: trailing octet, missing size.
    err(
        &pw_cmd(
            &mut t,
            cc::HIERARCHY_CHANGE_AUTH,
            &[OWNER],
            b"",
            &cat(&[&tpm2b(b"a"), &[0]]),
        ),
        Rc::SIZE,
    );
    err(
        &pw_cmd(&mut t, cc::HIERARCHY_CHANGE_AUTH, &[OWNER], b"", &[0]),
        Rc::INSUFFICIENT.in_param(1),
    );
    // A failed command changed nothing.
    ok(&set_auth(&mut t, OWNER, b"", b"a"));
}

#[test]
fn hierarchy_control_changes_the_enables_with_the_right_authorization() {
    let mut t = started();
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
    // Owner clears shEnable; with it cleared the owner authorization is refused (clause 11.2).
    ok(&hierarchy_control(&mut t, OWNER, b"", OWNER, 0));
    assert_eq!(startup_clear_attributes(&mut t), 0xD);
    err(
        &set_auth(&mut t, OWNER, b"", b"a"),
        Rc::HIERARCHY.in_handle(1),
    );
    err(
        &hierarchy_control(&mut t, OWNER, b"", OWNER, 0),
        Rc::HIERARCHY.in_handle(1),
    );
    // Only the platform can SET it again.
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 1));
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
    ok(&set_auth(&mut t, OWNER, b"", b"a"));
    // Endorsement: same rules for ehEnable.
    ok(&hierarchy_control(&mut t, ENDORSEMENT, b"", ENDORSEMENT, 0));
    assert_eq!(startup_clear_attributes(&mut t), 0xB);
    ok(&hierarchy_control(&mut t, PLATFORM, b"", ENDORSEMENT, 1));
    // Platform may CLEAR and SET phEnableNV, CLEAR (not SET) phEnable.
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"",
        handle::RH_PLATFORM_NV,
        0,
    ));
    assert_eq!(startup_clear_attributes(&mut t), 0x7);
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"",
        handle::RH_PLATFORM_NV,
        1,
    ));
    ok(&hierarchy_control(&mut t, PLATFORM, b"", PLATFORM, 0));
    assert_eq!(startup_clear_attributes(&mut t), 0xE);
    // phEnable CLEAR: platformAuth cannot authorize until _TPM_Init + Startup.
    err(
        &hierarchy_control(&mut t, PLATFORM, b"", PLATFORM, 1),
        Rc::HIERARCHY.in_handle(1),
    );
    t.init();
    startup(&mut t, 0);
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
}

#[test]
fn hierarchy_control_errors() {
    let mut t = started();
    // Roles: owner/endorsement cannot SET, nor change another hierarchy's enable.
    err(
        &hierarchy_control(&mut t, OWNER, b"", OWNER, 1),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, OWNER, b"", ENDORSEMENT, 0),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, OWNER, b"", PLATFORM, 0),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, OWNER, b"", handle::RH_PLATFORM_NV, 0),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, ENDORSEMENT, b"", ENDORSEMENT, 1),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, ENDORSEMENT, b"", OWNER, 0),
        Rc::AUTH_TYPE,
    );
    err(
        &hierarchy_control(&mut t, PLATFORM, b"", PLATFORM, 1),
        Rc::AUTH_TYPE,
    );
    // Unknown enable and state values.
    err(
        &hierarchy_control(&mut t, PLATFORM, b"", 0x4000_0002, 0),
        Rc::VALUE.in_param(1),
    );
    err(
        &hierarchy_control(&mut t, PLATFORM, b"", NULL, 0),
        Rc::VALUE.in_param(1),
    );
    err(
        &hierarchy_control(&mut t, PLATFORM, b"", OWNER, 2),
        Rc::VALUE.in_param(2),
    );
    // Authorization handles outside TPMI_RH_BASE_HIERARCHY.
    for h in [LOCKOUT, NULL, 0x0000_0000] {
        err(
            &hierarchy_control(&mut t, h, b"", OWNER, 0),
            Rc::VALUE.in_handle(1),
        );
    }
    // Wrong password, trailing octets.
    err(
        &hierarchy_control(&mut t, PLATFORM, b"x", OWNER, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &pw_cmd(
            &mut t,
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            b"",
            &cat(&[&u32b(OWNER), &[0, 0]]),
        ),
        Rc::SIZE,
    );
    // Nothing changed.
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
}

#[test]
fn startup_applies_the_reset_restart_and_resume_rules_to_the_hierarchies() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    ok(&set_auth(&mut t, PLATFORM, b"", b"plat"));
    ok(&hierarchy_control(&mut t, PLATFORM, b"plat", OWNER, 0));
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"plat",
        handle::RH_PLATFORM_NV,
        0,
    ));
    assert_eq!(startup_clear_attributes(&mut t), 0x5);

    // Resume: everything preserved (phEnable SET anyway).
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 1);
    assert_eq!(startup_clear_attributes(&mut t), 0x5);
    ok(&set_auth(&mut t, PLATFORM, b"plat", b"plat"));

    // Restart: platformAuth Empty, enables SET, ownerAuth persistent.
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 0);
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
    err(
        &set_auth(&mut t, PLATFORM, b"plat", b"x"),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&set_auth(&mut t, PLATFORM, b"", b"plat"));
    ok(&set_auth(&mut t, OWNER, b"own", b"own"));

    // Reset after Shutdown(CLEAR): same visible result.
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"plat",
        ENDORSEMENT,
        0,
    ));
    shutdown(&mut t, 0);
    t.init();
    startup(&mut t, 0);
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
    ok(&set_auth(&mut t, PLATFORM, b"", b""));
    ok(&set_auth(&mut t, OWNER, b"own", b"own"));
    // A disorderly power cycle (no Shutdown) is a Reset too.
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 0));
    t.init();
    startup(&mut t, 0);
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
}

#[test]
fn null_seed_is_new_after_a_reset_and_kept_by_restart_and_resume() {
    let mut t = started();
    let seed0 = t.hv.null_seed.to_vec();
    assert_ne!(
        seed0,
        vec![0u8; 32],
        "a seed is generated at the first Reset"
    );
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 1);
    assert_eq!(t.hv.null_seed.to_vec(), seed0, "Resume keeps the Null seed");
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 0);
    assert_eq!(
        t.hv.null_seed.to_vec(),
        seed0,
        "Restart keeps the Null seed"
    );
    shutdown(&mut t, 0);
    t.init();
    startup(&mut t, 0);
    assert_ne!(
        t.hv.null_seed.to_vec(),
        seed0,
        "Reset draws a new Null seed"
    );
    // The persistent seeds are not touched by startup.
    let s = t.persistent_snapshot();
    assert_ne!(&s[16..48], &s[48..80]);
}

#[test]
fn clear_resets_the_owner_and_changes_only_the_storage_seed() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    ok(&set_auth(&mut t, ENDORSEMENT, b"", b"end"));
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lock"));
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 0));
    ok(&hierarchy_control(&mut t, PLATFORM, b"", ENDORSEMENT, 0));
    // An owner index and a platform index.
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 1));
    ok(&define_raw(
        &mut t,
        OWNER,
        b"own",
        b"",
        &nv_public(NV0 + 1, alg::SHA256, OWNER_RW, &[], 4),
    ));
    ok(&define(&mut t, PLATFORM, NV0 + 2, PLATFORM_CREATED_RW, 4));
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 0));
    let before = t.persistent_snapshot();
    assert_eq!(permanent_attributes(&mut t), 0b111 | 1 << 10);
    // Advance the clock and the counters.
    t.execute(0, &plain(cc::READ_CLOCK, &[], &[]), 5000);
    shutdown(&mut t, 0);
    t.init();
    startup(&mut t, 0);
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 0));
    // Three seconds into this power cycle (the Clock restarts at zero at the Clear below).
    t.execute(0, &plain(cc::READ_CLOCK, &[], &[]), 3000);

    // Wrong platform password; then Clear with platform authorization.
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"x", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    let r = pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]);
    ok(&r);
    assert!(params(&r, 0).is_empty());
    // Enables SET; owner, endorsement and lockout values Empty; flags reset.
    assert_eq!(startup_clear_attributes(&mut t), 0xF);
    assert_eq!(permanent_attributes(&mut t), 1 << 10);
    ok(&set_auth(&mut t, OWNER, b"", b"n"));
    ok(&set_auth(&mut t, ENDORSEMENT, b"", b"n"));
    ok(&set_auth(&mut t, LOCKOUT, b"", b"n"));
    // The owner index is gone, the platform index stays.
    assert_eq!(handles_of(&mut t, NV0), vec![NV0 + 2]);
    // Only the storage seed changed (platform and endorsement seeds, offsets 16 and 48 / 80).
    let after = t.persistent_snapshot();
    assert_eq!(&before[16..48], &after[16..48], "platform seed");
    assert_ne!(&before[48..80], &after[48..80], "storage seed");
    assert_eq!(&before[80..112], &after[80..112], "endorsement seed");
    // Clock zero, counters zero, safe YES, pcrUpdateCounter + 1.
    let info = clock(&mut t, 3500);
    assert_eq!(
        (info.clock_info.reset_count, info.clock_info.restart_count),
        (0, 0)
    );
    assert!(info.clock_info.safe);
    assert_eq!(
        info.clock_info.clock, 500,
        "Clock restarted at zero at the Clear"
    );
    assert_eq!(info.time, 3500, "Time is not changed by Clear");
    // `pcrUpdateCounter` was 0 after Startup(CLEAR); TPM2_Clear adds one (Part 3, clause 24.6).
    assert_eq!(pcr_counter(&mut t), 1);
}

#[test]
fn clear_errors_and_clear_control() {
    let mut t = started();
    // Handles of TPMI_RH_CLEAR only.
    for h in [OWNER, ENDORSEMENT, NULL, 0] {
        err(
            &pw_cmd(&mut t, cc::CLEAR, &[h], b"", &[]),
            Rc::VALUE.in_handle(1),
        );
        err(
            &pw_cmd(&mut t, cc::CLEAR_CONTROL, &[h], b"", &[0]),
            Rc::VALUE.in_handle(1),
        );
    }
    err(&pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[1]), Rc::SIZE);
    // Lockout may SET disableClear but not CLEAR it.
    ok(&pw_cmd(&mut t, cc::CLEAR_CONTROL, &[LOCKOUT], b"", &[1]));
    assert_eq!(permanent_attributes(&mut t) & (1 << 8), 1 << 8);
    err(
        &pw_cmd(&mut t, cc::CLEAR_CONTROL, &[LOCKOUT], b"", &[0]),
        Rc::AUTH_FAIL.in_handle(1),
    );
    // While disabled, Clear is TPM_RC_DISABLED for both authorizations (after the authorization).
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]),
        Rc::DISABLED,
    );
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]),
        Rc::DISABLED,
    );
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"bad", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    // Platform clears the flag; value errors for the parameter.
    err(
        &pw_cmd(&mut t, cc::CLEAR_CONTROL, &[PLATFORM], b"", &[2]),
        Rc::VALUE.in_param(1),
    );
    ok(&pw_cmd(&mut t, cc::CLEAR_CONTROL, &[PLATFORM], b"", &[0]));
    assert_eq!(permanent_attributes(&mut t) & (1 << 8), 0);
    // Clear with lockout authorization.
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lk"));
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]),
        Rc::AUTH_FAIL.in_session(1),
    );
    // lockoutAuth is disabled after the failure (`da.rs`) until lockoutRecovery has passed.
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"lk", &[]),
        Rc::LOCKOUT,
    );
    pass_time(&mut t, 86_400);
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"lk", &[]));
    // Clear emptied lockoutAuth.
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]));
    // Disabled platform hierarchy: platformAuth is refused for Clear.
    ok(&hierarchy_control(&mut t, PLATFORM, b"", PLATFORM, 0));
    err(
        &pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]),
        Rc::HIERARCHY.in_handle(1),
    );
}

#[test]
fn clear_response_hmac_uses_the_new_lockout_auth() {
    let mut t = started();
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lk"));
    let mut s = start_hmac(&mut t, NULL, b"", &[0x11; 16]);
    // Clear authorized with the old lockoutAuth; the response HMAC key is the new (empty) value.
    let mut c = hc(cc::CLEAR, &[LOCKOUT], Vec::new(), (LOCKOUT, b"lk"));
    c.entity_after = Vec::new();
    ok(&run_hmac(&mut t, &mut s, &c));
    // The session survives (continueSession SET): HierarchyChangeAuth with the new value, whose
    // response HMAC key is the changed value.
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[LOCKOUT],
        tpm2b(b"newlock"),
        (LOCKOUT, b""),
    );
    c.entity_after = b"newlock".to_vec();
    ok(&run_hmac(&mut t, &mut s, &c));
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"newlock", &[]));
}

// ================================================================ sessions

#[test]
fn start_auth_session_gives_handles_and_nonces_and_flush_context_frees_them() {
    let mut t = started();
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
    let a = start_hmac(&mut t, NULL, b"", &[1; 16]);
    let b = start_hmac(&mut t, NULL, b"", &[2; 32]);
    assert_eq!((a.handle, b.handle), (0x0200_0000, 0x0200_0001));
    assert_eq!((a.nonce_tpm.len(), b.nonce_tpm.len()), (16, 32));
    assert_ne!(a.nonce_tpm, b.nonce_tpm[..16].to_vec());
    assert_eq!(handles_of(&mut t, 0x0200_0000), vec![a.handle, b.handle]);
    let (_, _, v, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::HR_LOADED, 1));
    assert_eq!(&v[4..], &u32b(2)[..]);
    // Flush the first; the slot is reused by the next session.
    let r = run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(a.handle)));
    ok(&r);
    assert_eq!(r.len(), 10);
    assert_eq!(handles_of(&mut t, 0x0200_0000), vec![b.handle]);
    let c = start_hmac(&mut t, NULL, b"", &[3; 16]);
    assert_eq!(c.handle, 0x0200_0000);
    // A session of the same slot has a new nonce even with the same caller nonce.
    let d = start_hmac(&mut t, NULL, b"", &[3; 16]);
    assert_ne!(c.nonce_tpm, d.nonce_tpm);
}

#[test]
fn start_auth_session_refuses_what_slice_a_does_not_do() {
    let mut t = started();
    let sym_null = u16b(alg::NULL);
    let ok_params = |nonce: &[u8], salt: &[u8], kind: u8, sym: &[u8], hash: u16| {
        start_params(nonce, salt, kind, sym, hash)
    };
    // Nonce: 15 octets is too short, 33 exceeds a digest (both TPM_RC_SIZE, parameter 1).
    for n in [&[0u8; 15][..], &[0u8; 33][..], &[][..]] {
        err(
            &run(
                &mut t,
                start_cmd(
                    NULL,
                    NULL,
                    &ok_params(n, &[], se::HMAC, &sym_null, alg::SHA256),
                ),
            ),
            Rc::SIZE.in_param(1),
        );
    }
    // encryptedSalt must be empty without a tpmKey.
    err(
        &run(
            &mut t,
            start_cmd(
                NULL,
                NULL,
                &ok_params(&[1; 16], &[1], se::HMAC, &sym_null, alg::SHA256),
            ),
        ),
        Rc::VALUE.in_param(2),
    );
    // Session types: 2 and 0xFF are no session types (policy and trial exist since slice C2).
    for kind in [2, 0x04, 0xFF] {
        err(
            &run(
                &mut t,
                start_cmd(
                    NULL,
                    NULL,
                    &ok_params(&[1; 16], &[], kind, &sym_null, alg::SHA256),
                ),
            ),
            Rc::VALUE.in_param(3),
        );
    }
    // Parameter encryption (slice C3): AES-128-CFB and XOR(SHA-256) are accepted; every other
    // `TPMT_SYM_DEF` is refused with parameter number 4.
    let aes = cat(&[&u16b(0x0006), &u16b(128), &u16b(0x0043)]);
    let xor = cat(&[&u16b(0x000A), &u16b(alg::SHA256)]);
    for sym in [&aes[..], &xor[..]] {
        let r = run(
            &mut t,
            start_cmd(
                NULL,
                NULL,
                &ok_params(&[1; 16], &[], se::HMAC, sym, alg::SHA256),
            ),
        );
        ok(&r);
        ok(&super::tests_obj::flush(
            &mut t,
            u32::from_be_bytes([r[10], r[11], r[12], r[13]]),
        ));
    }
    for (sym, expect) in [
        (
            cat(&[&u16b(0x0006), &u16b(256), &u16b(0x0043)]),
            Rc::KEY_SIZE,
        ),
        (cat(&[&u16b(0x0006), &u16b(128), &u16b(0x0044)]), Rc::MODE),
        (cat(&[&u16b(0x000A), &u16b(0x0004)]), Rc::HASH),
        (u16b(0x0013), Rc::SYMMETRIC),
    ] {
        err(
            &run(
                &mut t,
                start_cmd(
                    NULL,
                    NULL,
                    &ok_params(&[1; 16], &[], se::HMAC, &sym, alg::SHA256),
                ),
            ),
            expect.in_param(4),
        );
    }
    // Hashes: SHA-1, NULL.
    for hash in [0x0004, alg::NULL, 0xFFFF] {
        err(
            &run(
                &mut t,
                start_cmd(
                    NULL,
                    NULL,
                    &ok_params(&[1; 16], &[], se::HMAC, &sym_null, hash),
                ),
            ),
            Rc::HASH.in_param(5),
        );
    }
    // tpmKey: transient (not loaded), persistent, anything else.
    let good = ok_params(&[1; 16], &[], se::HMAC, &sym_null, alg::SHA256);
    err(
        &run(&mut t, start_cmd(0x8000_0000, NULL, &good)),
        Rc::reference_handle(0),
    );
    err(
        &run(&mut t, start_cmd(0x8100_0001, NULL, &good)),
        Rc::HANDLE.in_handle(1),
    );
    for k in [OWNER, 0, NV0, 0x0200_0000] {
        err(
            &run(&mut t, start_cmd(k, NULL, &good)),
            Rc::VALUE.in_handle(1),
        );
    }
    // bind: transient, persistent, PCR, NV index, the null password handle.
    err(
        &run(&mut t, start_cmd(NULL, 0x8000_0002, &good)),
        Rc::reference_handle(1),
    );
    err(
        &run(&mut t, start_cmd(NULL, 0x8100_0001, &good)),
        Rc::HANDLE.in_handle(2),
    );
    for b in [0, 7, NV0, handle::RS_PW, 0x4000_0002] {
        err(
            &run(&mut t, start_cmd(NULL, b, &good)),
            Rc::VALUE.in_handle(2),
        );
    }
    // Trailing octet; a session in the command (encrypt-only sessions are not supported).
    let mut long = good.clone();
    long.push(0);
    err(&run(&mut t, start_cmd(NULL, NULL, &long)), Rc::SIZE);
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::START_AUTH_SESSION,
                &[NULL, NULL],
                &auth_area(&[b""]),
                &good,
            ),
        ),
        Rc::ATTRIBUTES.in_session(1),
    );
    // No session was created by any of the failures.
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
}

#[test]
fn session_slots_run_out_at_64_and_startup_flushes_all() {
    let mut t = started();
    let mut last = 0;
    for i in 0..64 {
        let s = start_hmac(&mut t, NULL, b"", &[i as u8; 16]);
        last = s.handle;
    }
    assert_eq!(last, 0x0200_003F);
    let params = start_params(&[1; 16], &[], se::HMAC, &u16b(alg::NULL), alg::SHA256);
    err(
        &run(&mut t, start_cmd(NULL, NULL, &params)),
        Rc::SESSION_HANDLES,
    );
    let (_, _, v, _) = cap_list(&get_cap(
        &mut t,
        cap::TPM_PROPERTIES,
        pt::HR_ACTIVE_AVAIL,
        1,
    ));
    assert_eq!(&v[4..], &u32b(0)[..]);
    // Flush one: a new session fits again.
    ok(&run(
        &mut t,
        plain(cc::FLUSH_CONTEXT, &[], &u32b(0x0200_0010)),
    ));
    assert_eq!(start_hmac(&mut t, NULL, b"", &[9; 16]).handle, 0x0200_0010);
    // Any Startup ends every session; a command with the old handle names no loaded session.
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 1);
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
    let area = {
        let mut a = Vec::new();
        a.extend_from_slice(&0x0200_0010u32.to_be_bytes());
        a.extend_from_slice(&tpm2b(&[0; 16]));
        a.push(1);
        a.extend_from_slice(&tpm2b(&[0; 32]));
        let mut v = (a.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(&a);
        v
    };
    err(
        &run(
            &mut t,
            frame(st::SESSIONS, cc::CLEAR, &[PLATFORM], &area, &[]),
        ),
        Rc::reference_session(0),
    );
}

#[test]
fn flush_context_flushes_sessions_and_sequence_objects_and_refuses_the_rest() {
    let mut t = started();
    let s = start_hmac(&mut t, NULL, b"", &[1; 16]);
    // A session handle with another type octet names the same slot (Part 3, clause 28.4).
    ok(&run(
        &mut t,
        plain(cc::FLUSH_CONTEXT, &[], &u32b(0x0300_0000)),
    ));
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
    err(
        &run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(s.handle))),
        Rc::HANDLE.in_param(1),
    );
    // A sequence object.
    let seq = seq_handle(&start_seq(&mut t, b"", alg::SHA256));
    ok(&run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(seq))));
    err(&seq_update(&mut t, seq, b"", b"x"), Rc::reference_handle(0));
    err(
        &run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(seq))),
        Rc::HANDLE.in_param(1),
    );
    // Types that cannot be flushed: persistent object, PCR, NV index, permanent handle, and a
    // session slot beyond the table.
    for h in [0x8100_0000u32, 0, NV0, OWNER, 0x0200_0040, 0x0300_0040] {
        err(
            &run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(h))),
            Rc::VALUE.in_param(1),
        );
    }
    err(
        &run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(0x8000_0005))),
        Rc::HANDLE.in_param(1),
    );
    // Framing: no sessions of any type, exact size.
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::FLUSH_CONTEXT,
                &[],
                &auth_area(&[b""]),
                &u32b(0x0200_0000),
            ),
        ),
        Rc::AUTH_CONTEXT,
    );
    err(
        &run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &[])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::FLUSH_CONTEXT, &[], &cat(&[&u32b(0x0200_0000), &[0]])),
        ),
        Rc::SIZE,
    );
}

/// The whole exchange pinned against values computed independently with Python `hmac`/`hashlib`
/// (SP 800-90A HMAC_DRBG as in `random.rs`, KDFa and Equations 17, 18, 20-22 of Part 1 v184):
/// owner password, an unbound session that changes ownerAuth (response HMAC under the NEW value),
/// then a session bound to the owner that changes it again with the session key alone.
#[test]
fn hmac_session_exchange_matches_independent_python_values() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"ownerpw"));

    // Unbound, unsalted session.
    let resp = run(
        &mut t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&[7; 16], &[], se::HMAC, &u16b(alg::NULL), alg::SHA256),
        ),
    );
    assert_eq!(
        u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]),
        0x0200_0000
    );
    assert_eq!(
        &resp[14..],
        &tpm2b(&hex("a6780a72c7f12919629982405d094398"))[..]
    );
    let s = Hs {
        handle: 0x0200_0000,
        key: Vec::new(),
        nonce_tpm: hex("a6780a72c7f12919629982405d094398"),
        bound: None,
    };
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"second"),
        (OWNER, b"ownerpw"),
    );
    c.nonce_caller = vec![9; 16];
    c.entity_after = b"second".to_vec();
    let cmd = hmac_command(&s, &c, false, false);
    // The HMAC inside the command is the independent value.
    let tag = hex("d06a7a1617be4e6ee38c9497284f72cc251529818df82aff1dd01ff2c102fb26");
    assert!(cmd.windows(32).any(|w| w == &tag[..]));
    let resp = run(&mut t, cmd);
    let (params, acks) = split_response(&resp, 0);
    assert!(params.is_empty());
    assert_eq!(acks[0].0, hex("3f64e53032fbb062370276a9b143893b"));
    assert_eq!(acks[0].1, 1);
    assert_eq!(
        acks[0].2,
        hex("da7f61439dc667a5ac5148a4943460d72528db2cc1a87f5dc59fdc8f8dfed1c3")
    );

    // Session bound to the owner (authValue "second").
    let resp = run(
        &mut t,
        start_cmd(
            NULL,
            OWNER,
            &start_params(&[0x0b; 16], &[], se::HMAC, &u16b(alg::NULL), alg::SHA256),
        ),
    );
    assert_eq!(
        &resp[14..],
        &tpm2b(&hex("40dfabbb2ce0a1e4be5ea5e06355c692"))[..]
    );
    let key = hex("3902877e4f28822bf73899081c62b8b1eb03bdc7776e8cb60760b18897abe5f4");
    let s = Hs {
        handle: 0x0200_0001,
        key: key.clone(),
        nonce_tpm: hex("40dfabbb2ce0a1e4be5ea5e06355c692"),
        bound: Some((OWNER, b"second".to_vec())),
    };
    // The client model derives the same session key as the independent computation.
    assert_eq!(kdfa_256(b"second", b"ATH", &s.nonce_tpm, &[0x0b; 16]), key);
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"third"),
        (OWNER, b"second"),
    );
    c.nonce_caller = vec![0x0d; 16];
    c.entity_after = b"third".to_vec();
    let cmd = hmac_command(&s, &c, false, false);
    let tag = hex("990ad4ac5e075386c8401b03314d93f1eedc1ac58bf5dd88e819efb01b116675");
    assert!(cmd.windows(32).any(|w| w == &tag[..]));
    let resp = run(&mut t, cmd);
    let (_, acks) = split_response(&resp, 0);
    assert_eq!(acks[0].0, hex("7b3c1bf8fd90bc408f7cd7822ce5562d"));
    // Bound to the entity: the response HMAC key is the session key alone, also after the change.
    assert_eq!(
        acks[0].2,
        hex("fd4358d5230aec5cb0178adbfa16462970315de2a7d636e4ffe70946a01ed8cc")
    );
    ok(&set_auth(&mut t, OWNER, b"third", b""));
}

#[test]
fn hmac_session_authorization_rules() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    let mut s = start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let change = |new: &[u8], old: &[u8]| {
        hc(
            cc::HIERARCHY_CHANGE_AUTH,
            &[OWNER],
            tpm2b(new),
            (OWNER, old),
        )
    };

    // A wrong HMAC (one bit) and a wrong authValue are TPM_RC_BAD_AUTH for session 1, and the
    // failed command leaves the session and the hierarchy untouched.
    let c = change(b"x", b"own");
    err(
        &run(&mut t, hmac_command(&s, &c, false, true)),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(
            &mut t,
            hmac_command(&s, &change(b"x", b"wrong"), false, false),
        ),
        Rc::BAD_AUTH.in_session(1),
    );
    // A non-empty key with an empty HMAC is no authorization.
    err(
        &run(&mut t, hmac_command(&s, &c, true, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    // The same session, unchanged nonce, with the right HMAC succeeds; the response verifies.
    let mut c = change(b"own2", b"own");
    c.entity_after = b"own2".to_vec();
    let replay = hmac_command(&s, &c, false, false);
    ok(&run_hmac(&mut t, &mut s, &c));
    // Replaying the same bytes fails: the TPM rolled its nonce.
    err(&run(&mut t, replay), Rc::BAD_AUTH.in_session(1));
    // The HMAC covers the parameters (cpHash): flipping a parameter bit after the HMAC was
    // computed is an authorization failure.
    let mut cmd = hmac_command(&s, &change(b"own3", b"own2"), false, false);
    let last = cmd.len() - 1;
    cmd[last] ^= 1;
    err(&run(&mut t, cmd), Rc::BAD_AUTH.in_session(1));
    let mut c = change(b"own3", b"own2");
    c.entity_after = b"own3".to_vec();
    ok(&run_hmac(&mut t, &mut s, &c));

    // Nonce and attribute rules. nonceCaller of any size up to the session nonce is fine.
    for len in [0usize, 1, 16] {
        let mut c = change(b"own3", b"own3");
        c.nonce_caller = vec![0x55; len];
        ok(&run_hmac(&mut t, &mut s, &c));
    }
    let mut c = change(b"own3", b"own3");
    c.nonce_caller = vec![0x55; 17];
    err(
        &run(&mut t, hmac_command(&s, &c, false, false)),
        Rc::NONCE.in_session(1),
    );
    for (bits, expect) in [
        (session_attr::DECRYPT, Rc::SYMMETRIC.in_session(1)),
        (session_attr::ENCRYPT, Rc::SYMMETRIC.in_session(1)),
        (session_attr::AUDIT, Rc::ATTRIBUTES.in_session(1)),
        (session_attr::AUDIT_EXCLUSIVE, Rc::ATTRIBUTES.in_session(1)),
        (session_attr::AUDIT_RESET, Rc::ATTRIBUTES.in_session(1)),
        (0x08, Rc::ATTRIBUTES.in_session(1)),
        (0x10, Rc::ATTRIBUTES.in_session(1)),
    ] {
        let mut c = change(b"own3", b"own3");
        c.attrs = session_attr::CONTINUE_SESSION | bits;
        err(&run(&mut t, hmac_command(&s, &c, false, false)), expect);
    }
}

#[test]
fn continue_session_clear_ends_the_session_only_after_success() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    let mut s = start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    // A failed command with continueSession CLEAR keeps the session.
    let mut bad = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"z"),
        (OWNER, b"nope"),
    );
    bad.attrs = 0;
    err(
        &run(&mut t, hmac_command(&s, &bad, false, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    assert_eq!(handles_of(&mut t, 0x0200_0000), vec![s.handle]);
    // A successful one ends it, and the response shows continueSession CLEAR.
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"own"),
        (OWNER, b"own"),
    );
    c.attrs = 0;
    let resp = run_hmac(&mut t, &mut s, &c);
    let (_, acks) = split_response(&resp, 0);
    assert_eq!(acks[0].1, 0);
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
    // The session is gone: the next use names no loaded session.
    let c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"own"),
        (OWNER, b"own"),
    );
    err(
        &run(&mut t, hmac_command(&s, &c, false, false)),
        Rc::reference_session(0),
    );
}

#[test]
fn empty_key_session_accepts_an_empty_hmac_and_answers_with_one() {
    let mut t = started();
    // PCR authValue is empty and the session is unbound: the key is empty (clause 17.6.15).
    let mut s = start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let digest = Sha256::digest(b"abc");
    let params = cat(&[&u32b(1), &u16b(alg::SHA256), &digest]);
    let c = hc(cc::PCR_EXTEND, &[1], params, (1, b""));
    let resp = run(&mut t, hmac_command(&s, &c, true, false));
    let (_, acks) = split_response(&resp, 0);
    assert!(
        acks[0].2.is_empty(),
        "an empty HMAC is answered with an empty HMAC"
    );
    assert_eq!(acks[0].0.len(), 16);
    s.nonce_tpm = acks[0].0.clone();
    assert_eq!(read_pcr(&mut t, 1), hex(EXTEND_ABC_ON_ZERO));
    // A full HMAC is accepted as well and answered with a full HMAC.
    let c = hc(
        cc::PCR_EXTEND,
        &[1],
        cat(&[&u32b(1), &u16b(alg::SHA256), &digest]),
        (1, b""),
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    // A wrong HMAC is refused even with the empty key.
    let c = hc(
        cc::PCR_EXTEND,
        &[1],
        cat(&[&u32b(1), &u16b(alg::SHA256), &digest]),
        (1, b""),
    );
    err(
        &run(&mut t, hmac_command(&s, &c, false, true)),
        Rc::BAD_AUTH.in_session(1),
    );
}

#[test]
fn a_session_bound_to_an_entity_uses_its_authvalue_only_for_other_entities() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    ok(&set_auth(&mut t, PLATFORM, b"", b"plat"));
    let mut s = start_hmac(&mut t, OWNER, b"own", &[0x66; 16]);
    // For the owner (the bound entity) the key is the session key alone.
    let c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"own"),
        (OWNER, b"own"),
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    // A key built as for an unbound entity is refused.
    let unbound = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: None,
    };
    err(
        &run(&mut t, hmac_command(&unbound, &c, false, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    // For another entity the key is sessionKey || authValue.
    let c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[PLATFORM],
        tpm2b(b"plat"),
        (PLATFORM, b"plat"),
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    // After the owner's authValue changed, the session is no longer bound to the owner: the key
    // gets the new value appended (Part 1, clause 17.6.10: the TPM records the bound authValue).
    ok(&set_auth(&mut t, OWNER, b"own", b"own-new"));
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"own-new"),
        (OWNER, b"own-new"),
    );
    // A client that still treats the session as bound uses the session key alone; the TPM, which
    // recorded the old authValue, no longer does and expects sessionKey || "own-new".
    s.bound = Some((OWNER, b"own-new".to_vec()));
    err(
        &run(&mut t, hmac_command(&s, &c, false, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    s.bound = None;
    c.entity_after = b"own-new".to_vec();
    ok(&run_hmac(&mut t, &mut s, &c));
}

#[test]
fn hmac_sessions_authorize_phase_a_commands_with_the_right_names() {
    let mut t = started();
    let mut s = start_hmac(&mut t, NULL, b"", &[0x77; 16]);
    // A hash sequence object: Name is the Empty Buffer (Part 1, Table 6); authValue "sq".
    let seq = seq_handle(&start_seq(&mut t, b"sq", alg::SHA256));
    let mut c = hc(cc::SEQUENCE_UPDATE, &[seq], tpm2b(b"abc"), (seq, b"sq"));
    c.names = vec![Vec::new()];
    ok(&run_hmac(&mut t, &mut s, &c));
    // The Name of a PCR is its handle; with the handle used as Name the command is refused.
    let mut c = hc(cc::SEQUENCE_UPDATE, &[seq], tpm2b(b"abc"), (seq, b"sq"));
    c.names = vec![name_of(seq)];
    err(
        &run(&mut t, hmac_command(&s, &c, false, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    // SequenceComplete flushes the sequence; the response HMAC key then has no authValue.
    let mut c = hc(
        cc::SEQUENCE_COMPLETE,
        &[seq],
        cat(&[&tpm2b(b""), &u32b(NULL)]),
        (seq, b"sq"),
    );
    c.names = vec![Vec::new()];
    c.entity_after = Vec::new();
    let resp = run_hmac(&mut t, &mut s, &c);
    let (p, _) = split_response(&resp, 0);
    let mut r = Reader::new(&p);
    assert_eq!(r.tpm2b(32).unwrap(), &Sha256::digest(b"abc")[..]);
    assert!(handles_of(&mut t, 0x8000_0000).is_empty());
}

#[test]
fn two_sessions_in_one_command_and_duplicate_session_handles() {
    let mut t = started();
    let s = start_hmac(&mut t, NULL, b"", &[0x77; 16]);
    // EventSequenceComplete needs two sessions; naming the same HMAC session twice is refused
    // (a session handle may occur only once in a command, Part 1, clause 16.6.3).
    let ev = seq_handle(&start_seq(&mut t, b"", alg::NULL));
    let mut area = Vec::new();
    for _ in 0..2 {
        area.extend_from_slice(&s.handle.to_be_bytes());
        area.extend_from_slice(&tpm2b(&[]));
        area.push(1);
        area.extend_from_slice(&tpm2b(&[]));
    }
    let mut auth = (area.len() as u32).to_be_bytes().to_vec();
    auth.extend_from_slice(&area);
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::EVENT_SEQUENCE_COMPLETE,
                &[1, ev],
                &auth,
                &tpm2b(b""),
            ),
        ),
        Rc::VALUE.in_session(2),
    );
}

// ================================================================ NV

#[test]
fn nv_define_write_read_and_read_public_round_trip() {
    let mut t = started();
    let idx = NV0 + 0x10;
    ok(&define(&mut t, OWNER, idx, OWNER_RW, 16));
    // ReadPublic: the public area as defined (WRITTEN CLEAR) and its Name.
    let (public, name) = nv_read_public(&mut t, idx);
    assert_eq!(public, nv_public(idx, alg::SHA256, OWNER_RW, &[], 16));
    let mut expect_name = u16b(alg::SHA256);
    expect_name.extend_from_slice(&Sha256::digest(&public));
    assert_eq!(name, expect_name);
    // Never written: reads fail with TPM_RC_NV_UNINITIALIZED even for size zero.
    err(
        &nv_read(&mut t, OWNER, idx, b"", 4, 0),
        Rc::NV_UNINITIALIZED,
    );
    err(
        &nv_read(&mut t, OWNER, idx, b"", 0, 0),
        Rc::NV_UNINITIALIZED,
    );
    // Write in the middle; the rest stays zero; WRITTEN is SET and the Name changes.
    let r = nv_write(&mut t, OWNER, idx, b"", &[1, 2, 3, 4, 5, 6, 7, 8], 4);
    ok(&r);
    assert!(params(&r, 0).is_empty());
    let (public2, name2) = nv_read_public(&mut t, idx);
    assert_eq!(
        public2,
        nv_public(idx, alg::SHA256, OWNER_RW | attr::WRITTEN, &[], 16)
    );
    assert_ne!(name, name2);
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 16, 0)),
        [0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0]
    );
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 4, 6)),
        [3, 4, 5, 6]
    );
    assert_eq!(nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 0, 16)), []);
    // Overwrite.
    ok(&nv_write(&mut t, OWNER, idx, b"", &[9; 16], 0));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 16, 0)),
        [9; 16]
    );
    // Listing and counts.
    assert_eq!(handles_of(&mut t, NV0), vec![idx]);
    let (_, _, v, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::HR_NV_INDEX, 1));
    assert_eq!(&v[4..], &u32b(1)[..]);
    // The data survive _TPM_Init and Startup (NV).
    t.init();
    startup(&mut t, 0);
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 16, 0)),
        [9; 16]
    );
}

#[test]
fn nv_define_space_errors() {
    let mut t = started();
    let idx = NV0 + 1;
    let other = NV0 + 2;
    let pubd = |attrs: u32| nv_public(other, alg::SHA256, attrs, &[], 8);
    let def = |t: &mut Tpm, attrs: u32| define_raw(t, OWNER, b"", b"", &pubd(attrs));
    ok(&define(&mut t, OWNER, idx, OWNER_RW, 8));
    // Already defined.
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(idx, alg::SHA256, OWNER_RW, &[], 8),
        ),
        Rc::NV_DEFINED,
    );
    // Handle, algorithm and reserved bits (parameter 2).
    for bad in [0x0200_0001u32, 0x0000_0001, 0x4000_0001, 0x8100_0001] {
        err(
            &define_raw(
                &mut t,
                OWNER,
                b"",
                b"",
                &nv_public(bad, alg::SHA256, OWNER_RW, &[], 8),
            ),
            Rc::VALUE.in_param(2),
        );
    }
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(other, 0x0004, OWNER_RW, &[], 8),
        ),
        Rc::HASH.in_param(2),
    );
    for bit in [0x100u32, 0x200, 1 << 20, 1 << 24] {
        err(
            &define_raw(
                &mut t,
                OWNER,
                b"",
                b"",
                &nv_public(other, alg::SHA256, OWNER_RW | bit, &[], 8),
            ),
            Rc::RESERVED_BITS.in_param(2),
        );
    }
    // Attribute rules (clause 31.3.1) and the features slice A lacks.
    let attributes_error = [
        attr::WRITTEN | OWNER_RW,
        attr::READLOCKED | OWNER_RW,
        attr::WRITELOCKED | OWNER_RW,
        attr::OWNERWRITE,                // no read access
        attr::OWNERREAD,                 // no write access
        0,                               // neither
        OWNER_RW | attr::PLATFORMCREATE, // owner-defined with PLATFORMCREATE SET
        OWNER_RW | 0x80,                 // TPM_NT_PIN_FAIL
        OWNER_RW | 0x90,                 // TPM_NT_PIN_PASS
        OWNER_RW | 0x30,                 // reserved TPM_NT
        OWNER_RW | attr::POLICYREAD,
        OWNER_RW | attr::POLICYWRITE,
        OWNER_RW | attr::POLICY_DELETE,
        OWNER_RW | attr::GLOBALLOCK,
        OWNER_RW | attr::ORDERLY,
        OWNER_RW | attr::CLEAR_STCLEAR,
    ];
    for a in attributes_error {
        err(&def(&mut t, a), Rc::ATTRIBUTES.in_param(2));
    }
    // The platform must set PLATFORMCREATE.
    err(
        &define_raw(
            &mut t,
            PLATFORM,
            b"",
            b"",
            &nv_public(other, alg::SHA256, OWNER_RW, &[], 8),
        ),
        Rc::ATTRIBUTES.in_param(2),
    );
    // Sizes (parameter 1: auth, parameter 2: public area).
    err(
        &define_raw(&mut t, OWNER, b"", &[1; 33], &pubd(OWNER_RW)),
        Rc::SIZE.in_param(1),
    );
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(
                other,
                alg::SHA256,
                OWNER_RW,
                &[],
                (MAX_NV_INDEX_SIZE + 1) as u16,
            ),
        ),
        Rc::SIZE.in_param(2),
    );
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(
                other,
                alg::SHA256,
                OWNER_RW | attr::WRITEALL,
                &[],
                (NV_BUFFER_MAX + 1) as u16,
            ),
        ),
        Rc::SIZE.in_param(2),
    );
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(other, alg::SHA256, OWNER_RW, &[1; 5], 8),
        ),
        Rc::SIZE.in_param(2),
    );
    // `size` of TPM2B_NV_PUBLIC that does not match the structure.
    let mut wrong = tpm2b(&pubd(OWNER_RW));
    wrong[1] += 1;
    wrong.push(0);
    err(
        &pw_cmd(
            &mut t,
            cc::NV_DEFINE_SPACE,
            &[OWNER],
            b"",
            &cat(&[&tpm2b(b""), &wrong]),
        ),
        Rc::SIZE.in_param(2),
    );
    // Authorization.
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"bad",
            b"",
            &nv_public(other, alg::SHA256, OWNER_RW, &[], 8),
        ),
        Rc::BAD_AUTH.in_session(1),
    );
    for h in [NULL, ENDORSEMENT, LOCKOUT, 0] {
        err(
            &define_raw(
                &mut t,
                h,
                b"",
                b"",
                &nv_public(other, alg::SHA256, OWNER_RW, &[], 8),
            ),
            Rc::VALUE.in_handle(1),
        );
    }
    err(
        &run(
            &mut t,
            plain(
                cc::NV_DEFINE_SPACE,
                &[OWNER],
                &cat(&[&tpm2b(b""), &tpm2b(&pubd(OWNER_RW))]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    // None of the failures defined anything.
    assert_eq!(handles_of(&mut t, NV0), vec![idx]);
}

#[test]
fn nv_pool_and_index_count_limits() {
    let mut t = started();
    // Two indices fill the pool exactly; one more octet is TPM_RC_NV_SPACE.
    ok(&define(&mut t, OWNER, NV0 + 1, OWNER_RW, 8192));
    ok(&define(
        &mut t,
        OWNER,
        NV0 + 2,
        OWNER_RW,
        (NV_POOL_SIZE - 8192) as u16,
    ));
    err(&define(&mut t, OWNER, NV0 + 3, OWNER_RW, 1), Rc::NV_SPACE);
    // Deleting one frees its space.
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, NV0 + 1],
        b"",
        &[],
    ));
    ok(&define(&mut t, OWNER, NV0 + 3, OWNER_RW, 8192));
    // The count limit: 128 indices.
    let mut t = started();
    for i in 0..128u32 {
        ok(&define(&mut t, OWNER, NV0 + i, OWNER_RW, 1));
    }
    err(&define(&mut t, OWNER, NV0 + 128, OWNER_RW, 1), Rc::NV_SPACE);
    // Zero-size indices are fine but still count (here: no room because of the count).
    assert_eq!(handles_of(&mut t, NV0).len(), 128);
}

#[test]
fn nv_read_write_errors() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&define(&mut t, OWNER, idx, OWNER_RW, 8));
    // Range: offset + size beyond the data area.
    err(&nv_write(&mut t, OWNER, idx, b"", &[1; 9], 0), Rc::NV_RANGE);
    err(&nv_write(&mut t, OWNER, idx, b"", &[1; 4], 5), Rc::NV_RANGE);
    err(&nv_write(&mut t, OWNER, idx, b"", &[1], 9), Rc::NV_RANGE);
    ok(&nv_write(&mut t, OWNER, idx, b"", &[1; 4], 4));
    err(&nv_read(&mut t, OWNER, idx, b"", 9, 0), Rc::NV_RANGE);
    err(&nv_read(&mut t, OWNER, idx, b"", 4, 5), Rc::NV_RANGE);
    err(&nv_read(&mut t, OWNER, idx, b"", 1, 9), Rc::NV_RANGE);
    // Buffer limits: write data above NV_BUFFER_MAX is a size error, a read size too.
    err(
        &nv_write(&mut t, OWNER, idx, b"", &[0; NV_BUFFER_MAX + 1], 0),
        Rc::SIZE.in_param(1),
    );
    err(
        &nv_read(&mut t, OWNER, idx, b"", (NV_BUFFER_MAX + 1) as u16, 0),
        Rc::VALUE.in_param(1),
    );
    // Parameters: trailing octets, missing offset.
    err(
        &pw_cmd(
            &mut t,
            cc::NV_WRITE,
            &[OWNER, idx],
            b"",
            &cat(&[&tpm2b(&[1]), &u16b(0), &[0]]),
        ),
        Rc::SIZE,
    );
    err(
        &pw_cmd(&mut t, cc::NV_READ, &[OWNER, idx], b"", &u16b(1)),
        Rc::INSUFFICIENT.in_param(2),
    );
    // Handles: not defined, outside the NV range, authHandle types.
    err(
        &nv_write(&mut t, OWNER, NV0 + 9, b"", &[1], 0),
        Rc::HANDLE.in_handle(2),
    );
    err(
        &nv_read(&mut t, OWNER, NV0 + 9, b"", 1, 0),
        Rc::HANDLE.in_handle(2),
    );
    for bad in [0x0200_0001u32, 0, 0x4000_0001, 0x8100_0001] {
        err(
            &nv_write(&mut t, OWNER, bad, b"", &[1], 0),
            Rc::VALUE.in_handle(2),
        );
    }
    for bad in [NULL, ENDORSEMENT, LOCKOUT, 0u32, 0x8000_0000] {
        err(
            &nv_write(&mut t, bad, idx, b"", &[1], 0),
            Rc::VALUE.in_handle(1),
        );
        err(
            &nv_read(&mut t, bad, idx, b"", 1, 0),
            Rc::VALUE.in_handle(1),
        );
    }
    // Wrong password; no session.
    err(
        &nv_write(&mut t, OWNER, idx, b"bad", &[1], 0),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &nv_read(&mut t, OWNER, idx, b"bad", 1, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::NV_READ, &[OWNER, idx], &cat(&[&u16b(1), &u16b(0)])),
        ),
        Rc::AUTH_MISSING,
    );
    // The platform has no PPWRITE/PPREAD here: TPM_RC_AUTH_TYPE (checked before the password).
    err(
        &nv_write(&mut t, PLATFORM, idx, b"", &[1], 0),
        Rc::AUTH_TYPE,
    );
    err(&nv_read(&mut t, PLATFORM, idx, b"", 1, 0), Rc::AUTH_TYPE);
    // The index as its own authorization needs AUTHWRITE/AUTHREAD.
    err(&nv_write(&mut t, idx, idx, b"", &[1], 0), Rc::AUTH_TYPE);
    // Another index as authorization handle: TPM_RC_NV_AUTHORIZATION.
    ok(&define(
        &mut t,
        OWNER,
        NV0 + 2,
        attr::AUTHWRITE | attr::AUTHREAD,
        4,
    ));
    err(
        &nv_write(&mut t, NV0 + 2, idx, b"", &[1], 0),
        Rc::NV_AUTHORIZATION,
    );
    err(
        &nv_read(&mut t, NV0 + 2, idx, b"", 1, 0),
        Rc::NV_AUTHORIZATION,
    );
    // An undefined index as authorization handle is TPM_RC_HANDLE for handle 1.
    err(
        &nv_write(&mut t, NV0 + 7, idx, b"", &[1], 0),
        Rc::HANDLE.in_handle(1),
    );
}

#[test]
fn nv_writeall_requires_a_full_write() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&define(&mut t, OWNER, idx, OWNER_RW | attr::WRITEALL, 8));
    err(&nv_write(&mut t, OWNER, idx, b"", &[1; 4], 0), Rc::NV_RANGE);
    err(&nv_write(&mut t, OWNER, idx, b"", &[1; 9], 0), Rc::NV_RANGE);
    ok(&nv_write(&mut t, OWNER, idx, b"", &[1; 8], 0));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 8, 0)),
        [1; 8]
    );
}

#[test]
fn nv_index_authvalue_authorizes_when_the_attributes_allow() {
    let mut t = started();
    let idx = NV0 + 5;
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"idxpw\0",
        &nv_public(idx, alg::SHA256, attr::AUTHWRITE | attr::AUTHREAD, &[], 4),
    ));
    err(
        &nv_write(&mut t, idx, idx, b"", &[1], 0),
        Rc::AUTH_FAIL.in_session(1),
    );
    ok(&nv_write(&mut t, idx, idx, b"idxpw", &[7, 8, 9, 10], 0));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, idx, idx, b"idxpw", 4, 0)),
        [7, 8, 9, 10]
    );
    // The owner has no OWNERREAD/OWNERWRITE on this index.
    err(&nv_read(&mut t, OWNER, idx, b"", 1, 0), Rc::AUTH_TYPE);
}

#[test]
fn nv_locks_follow_the_attributes_and_startup_clear() {
    let mut t = started();
    let (a, b, c) = (NV0 + 1, NV0 + 2, NV0 + 3);
    ok(&define(&mut t, OWNER, a, OWNER_RW | attr::WRITEDEFINE, 4));
    ok(&define(&mut t, OWNER, b, OWNER_RW | attr::WRITE_STCLEAR, 4));
    ok(&define(&mut t, OWNER, c, OWNER_RW | attr::READ_STCLEAR, 4));
    let lock = |t: &mut Tpm, code: u32, i: u32| pw_cmd(t, code, &[OWNER, i], b"", &[]);
    // Without the matching attribute the lock commands are TPM_RC_ATTRIBUTES.
    err(&lock(&mut t, cc::NV_WRITE_LOCK, c), Rc::ATTRIBUTES);
    err(&lock(&mut t, cc::NV_READ_LOCK, a), Rc::ATTRIBUTES);
    // Wrong password is an authorization error before anything else.
    err(
        &pw_cmd(&mut t, cc::NV_WRITE_LOCK, &[OWNER, a], b"bad", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    for i in [a, b, c] {
        ok(&nv_write(&mut t, OWNER, i, b"", &[1, 2, 3, 4], 0));
    }
    // Write lock: writes are TPM_RC_NV_LOCKED, but the authorization comes first.
    ok(&lock(&mut t, cc::NV_WRITE_LOCK, a));
    ok(&lock(&mut t, cc::NV_WRITE_LOCK, a));
    ok(&lock(&mut t, cc::NV_WRITE_LOCK, b));
    assert_ne!(nv_attributes(&mut t, a) & attr::WRITELOCKED, 0);
    err(&nv_write(&mut t, OWNER, a, b"", &[9], 0), Rc::NV_LOCKED);
    err(
        &nv_write(&mut t, OWNER, a, b"bad", &[9], 0),
        Rc::BAD_AUTH.in_session(1),
    );
    // Reads are not affected by a write lock.
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, a, b"", 4, 0)),
        [1, 2, 3, 4]
    );
    // Read lock: reads are TPM_RC_NV_LOCKED, writes are not.
    ok(&lock(&mut t, cc::NV_READ_LOCK, c));
    ok(&lock(&mut t, cc::NV_READ_LOCK, c));
    err(&nv_read(&mut t, OWNER, c, b"", 4, 0), Rc::NV_LOCKED);
    err(
        &nv_read(&mut t, OWNER, c, b"bad", 4, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&nv_write(&mut t, OWNER, c, b"", &[5, 5, 5, 5], 0));
    // Startup(CLEAR): the read lock and the WRITE_STCLEAR lock go, the WRITEDEFINE lock of a
    // written index stays (Part 2, Table 233; Part 3, clause 9.3.1).
    t.init();
    startup(&mut t, 0);
    err(&nv_write(&mut t, OWNER, a, b"", &[9], 0), Rc::NV_LOCKED);
    ok(&nv_write(&mut t, OWNER, b, b"", &[9], 0));
    assert_eq!(nv_read_data(&nv_read(&mut t, OWNER, c, b"", 4, 0)), [5; 4]);
    // An unwritten WRITEDEFINE index that was locked is unlocked by the next Startup(CLEAR).
    let d = NV0 + 4;
    ok(&define(&mut t, OWNER, d, OWNER_RW | attr::WRITEDEFINE, 4));
    ok(&lock(&mut t, cc::NV_WRITE_LOCK, d));
    err(&nv_write(&mut t, OWNER, d, b"", &[1], 0), Rc::NV_LOCKED);
    t.init();
    startup(&mut t, 0);
    ok(&nv_write(&mut t, OWNER, d, b"", &[1], 0));
    // Resume keeps the locks.
    ok(&lock(&mut t, cc::NV_WRITE_LOCK, b));
    shutdown(&mut t, 1);
    t.init();
    startup(&mut t, 1);
    err(&nv_write(&mut t, OWNER, b, b"", &[1], 0), Rc::NV_LOCKED);
}

#[test]
fn nv_undefine_space_rules() {
    let mut t = started();
    let (own, plat) = (NV0 + 1, NV0 + 2);
    ok(&define(&mut t, OWNER, own, OWNER_RW, 8));
    ok(&define(
        &mut t,
        PLATFORM,
        plat,
        PLATFORM_CREATED_RW | attr::OWNERREAD,
        8,
    ));
    // The owner cannot delete a platform-created index; the platform can delete either.
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, plat], b"", &[]),
        Rc::NV_AUTHORIZATION,
    );
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, own], b"bad", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[PLATFORM, own],
        b"",
        &[],
    ));
    assert_eq!(handles_of(&mut t, NV0), vec![plat]);
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[PLATFORM, plat],
        b"",
        &[],
    ));
    assert!(handles_of(&mut t, NV0).is_empty());
    // Not defined; out of range; authorization handle types; no session; trailing octets.
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, own], b"", &[]),
        Rc::HANDLE.in_handle(2),
    );
    err(
        &pw_cmd(
            &mut t,
            cc::NV_UNDEFINE_SPACE,
            &[OWNER, 0x0200_0001],
            b"",
            &[],
        ),
        Rc::VALUE.in_handle(2),
    );
    for h in [NULL, ENDORSEMENT, LOCKOUT, 0u32] {
        err(
            &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[h, own], b"", &[]),
            Rc::VALUE.in_handle(1),
        );
    }
    ok(&define(&mut t, OWNER, own, OWNER_RW, 8));
    err(
        &run(&mut t, plain(cc::NV_UNDEFINE_SPACE, &[OWNER, own], &[])),
        Rc::AUTH_MISSING,
    );
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, own], b"", &[0]),
        Rc::SIZE,
    );
    // A redefinition after deletion starts unwritten with fresh data.
    ok(&nv_write(&mut t, OWNER, own, b"", &[1; 8], 0));
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, own],
        b"",
        &[],
    ));
    ok(&define(&mut t, OWNER, own, OWNER_RW, 8));
    err(
        &nv_read(&mut t, OWNER, own, b"", 8, 0),
        Rc::NV_UNINITIALIZED,
    );
    ok(&nv_write(&mut t, OWNER, own, b"", &[], 0));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, own, b"", 8, 0)),
        [0; 8]
    );
}

#[test]
fn nv_follows_the_hierarchy_enables() {
    let mut t = started();
    let (own, plat) = (NV0 + 1, NV0 + 2);
    ok(&define(
        &mut t,
        OWNER,
        own,
        OWNER_RW | attr::PPREAD | attr::PPWRITE,
        4,
    ));
    ok(&define(
        &mut t,
        PLATFORM,
        plat,
        PLATFORM_CREATED_RW | OWNER_RW,
        4,
    ));
    ok(&nv_write(&mut t, OWNER, own, b"", &[1; 4], 0));
    ok(&nv_write(&mut t, PLATFORM, plat, b"", &[2; 4], 0));

    // shEnable CLEAR: the owner authorization is refused, owner-created indices are not
    // accessible even for the platform (Part 2, Table 43), platform-created ones still are.
    ok(&hierarchy_control(&mut t, OWNER, b"", OWNER, 0));
    err(
        &nv_read(&mut t, OWNER, plat, b"", 4, 0),
        Rc::HIERARCHY.in_handle(1),
    );
    err(
        &nv_read(&mut t, PLATFORM, own, b"", 4, 0),
        Rc::HANDLE.in_handle(2),
    );
    err(
        &run(&mut t, plain(cc::NV_READ_PUBLIC, &[own], &[])),
        Rc::HANDLE.in_handle(1),
    );
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[PLATFORM, own], b"", &[]),
        Rc::HANDLE.in_handle(2),
    );
    assert_eq!(
        nv_read_data(&nv_read(&mut t, PLATFORM, plat, b"", 4, 0)),
        [2; 4]
    );
    err(
        &define(&mut t, OWNER, NV0 + 9, OWNER_RW, 1),
        Rc::HIERARCHY.in_handle(1),
    );
    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 1));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, own, b"", 4, 0)),
        [1; 4]
    );

    // phEnableNV CLEAR: platform-created indices are not accessible, the platform cannot define
    // indices (TPM_RC_HIERARCHY), owner indices work.
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"",
        handle::RH_PLATFORM_NV,
        0,
    ));
    err(
        &nv_read(&mut t, OWNER, plat, b"", 4, 0),
        Rc::HANDLE.in_handle(2),
    );
    err(
        &define(&mut t, PLATFORM, NV0 + 9, PLATFORM_CREATED_RW, 1),
        Rc::HIERARCHY.in_handle(1),
    );
    assert_eq!(
        nv_read_data(&nv_read(&mut t, PLATFORM, own, b"", 4, 0)),
        [1; 4]
    );
    ok(&hierarchy_control(
        &mut t,
        PLATFORM,
        b"",
        handle::RH_PLATFORM_NV,
        1,
    ));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, plat, b"", 4, 0)),
        [2; 4]
    );

    // phEnable CLEAR: platformAuth is refused, the indices stay accessible through the owner.
    ok(&hierarchy_control(&mut t, PLATFORM, b"", PLATFORM, 0));
    err(
        &nv_read(&mut t, PLATFORM, own, b"", 4, 0),
        Rc::HIERARCHY.in_handle(1),
    );
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, own, b"", 4, 0)),
        [1; 4]
    );
}

#[test]
fn nv_commands_work_under_hmac_sessions_with_the_nv_name_in_the_cphash() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    let idx = NV0 + 3;
    let mut s = start_hmac(&mut t, NULL, b"", &[0x21; 16]);
    // DefineSpace: the only handle is the owner, whose Name is its handle.
    let pubd = nv_public(
        idx,
        alg::SHA256,
        OWNER_RW | attr::AUTHREAD | attr::AUTHWRITE,
        &[],
        6,
    );
    let params_define = cat(&[&tpm2b(b"idxauth"), &tpm2b(&pubd)]);
    let c = hc(
        cc::NV_DEFINE_SPACE,
        &[OWNER],
        params_define,
        (OWNER, b"own"),
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    // Write authorized by the owner: names are [owner handle, Name of the index].
    let (_, name) = nv_read_public(&mut t, idx);
    let mut c = hc(
        cc::NV_WRITE,
        &[OWNER, idx],
        cat(&[&tpm2b(&[1, 2, 3]), &u16b(0)]),
        (OWNER, b"own"),
    );
    c.names = vec![name_of(OWNER), name.clone()];
    ok(&run_hmac(&mut t, &mut s, &c));
    // The Name changed (WRITTEN): a cpHash built with the old Name is refused.
    let (_, name_after) = nv_read_public(&mut t, idx);
    assert_ne!(name, name_after);
    let mut c = hc(
        cc::NV_READ,
        &[OWNER, idx],
        cat(&[&u16b(3), &u16b(0)]),
        (OWNER, b"own"),
    );
    c.names = vec![name_of(OWNER), name];
    err(
        &run(&mut t, hmac_command(&s, &c, false, false)),
        Rc::BAD_AUTH.in_session(1),
    );
    // With the current Name, and with the index as authorization handle (authValue "idxauth").
    c.names = vec![name_of(OWNER), name_after.clone()];
    let resp = run_hmac(&mut t, &mut s, &c);
    let (p, _) = split_response(&resp, 0);
    assert_eq!(p, tpm2b(&[1, 2, 3]));
    let mut c = hc(
        cc::NV_READ,
        &[idx, idx],
        cat(&[&u16b(3), &u16b(0)]),
        (idx, b"idxauth"),
    );
    c.names = vec![name_after.clone(), name_after];
    let resp = run_hmac(&mut t, &mut s, &c);
    assert_eq!(split_response(&resp, 0).0, tpm2b(&[1, 2, 3]));
}

// ================================================================ capability and persistence

#[test]
fn capability_handles_list_the_new_entities() {
    let mut t = started();
    assert_eq!(
        handles_of(&mut t, 0x4000_0000),
        vec![OWNER, NULL, LOCKOUT, ENDORSEMENT, PLATFORM]
    );
    // Starting inside the range skips the lower handles.
    assert_eq!(
        handles_of(&mut t, LOCKOUT),
        vec![LOCKOUT, ENDORSEMENT, PLATFORM]
    );
    assert!(
        handles_of(&mut t, 0x0300_0000).is_empty(),
        "no saved sessions"
    );
    assert!(handles_of(&mut t, 0x8100_0000).is_empty());
    let a = start_hmac(&mut t, NULL, b"", &[1; 16]);
    let b = start_hmac(&mut t, NULL, b"", &[1; 16]);
    assert_eq!(handles_of(&mut t, 0x0200_0000), vec![a.handle, b.handle]);
    assert_eq!(handles_of(&mut t, 0x0200_0001), vec![b.handle]);
    ok(&define(&mut t, OWNER, NV0 + 3, OWNER_RW, 1));
    ok(&define(&mut t, OWNER, NV0 + 1, OWNER_RW, 1));
    assert_eq!(handles_of(&mut t, NV0), vec![NV0 + 1, NV0 + 3]);
    assert_eq!(handles_of(&mut t, NV0 + 2), vec![NV0 + 3]);
    // Limits as properties.
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::NV_INDEX_MAX, 1));
    assert_eq!(&body[4..], &u32b(MAX_NV_INDEX_SIZE as u32)[..]);
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::NV_BUFFER_MAX, 1));
    assert_eq!(&body[4..], &u32b(NV_BUFFER_MAX as u32)[..]);
    let (_, _, body, _) = cap_list(&get_cap(
        &mut t,
        cap::TPM_PROPERTIES,
        pt::ACTIVE_SESSIONS_MAX,
        1,
    ));
    assert_eq!(&body[4..], &u32b(64)[..]);
}

#[test]
fn capability_commands_report_the_extensive_and_nv_attributes() {
    let mut t = started();
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x121, 1000));
    // From 0x121 on: 69 commands less EvictControl (0x120).
    assert_eq!(count, 68);
    let attrs: Vec<u32> = body
        .chunks(4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let of = |code: u32| *attrs.iter().find(|a| *a & 0xFFFF == code).unwrap();
    // {NV E} with one handle.
    assert_eq!(of(0x121), 0x121 | 1 << 22 | 1 << 23 | 1 << 25);
    assert_eq!(of(0x126), 0x126 | 1 << 22 | 1 << 23 | 1 << 25);
    // {NV} with one handle.
    assert_eq!(of(0x129), 0x129 | 1 << 22 | 1 << 25);
    // StartAuthSession: two handles and a handle in the response.
    assert_eq!(of(0x176), 0x176 | 2 << 25 | 1 << 28);
    assert_eq!(of(0x14A), 0x14A | 2 << 25);
    assert_eq!(of(0x14D), 0x14D | 3 << 25);
    assert_eq!(of(0x15B), 0x15B | 1 << 25 | 1 << 28);
    // NV_Read: two handles, no NV write; NV_Write: {NV}.
    assert_eq!(of(0x14E), 0x14E | 2 << 25);
    assert_eq!(of(0x137), 0x137 | 1 << 22 | 2 << 25);
    // FlushContext and NV_ReadPublic.
    assert_eq!(of(0x165), 0x165);
    assert_eq!(of(0x169), 0x169 | 1 << 25);
}

#[test]
fn every_authorized_command_refuses_a_session_that_is_not_loaded() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&define(
        &mut t,
        OWNER,
        idx,
        OWNER_RW | attr::WRITE_STCLEAR | attr::READ_STCLEAR,
        4,
    ));
    ok(&nv_write(&mut t, OWNER, idx, b"", &[1; 4], 0));
    let area = {
        let mut a = Vec::new();
        a.extend_from_slice(&0x0200_0007u32.to_be_bytes());
        a.extend_from_slice(&tpm2b(&[]));
        a.push(1);
        a.extend_from_slice(&tpm2b(&[]));
        let mut v = (a.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(&a);
        v
    };
    let nv_pub = nv_public(NV0 + 8, alg::SHA256, OWNER_RW, &[], 1);
    let cases: Vec<(u32, Vec<u32>, Vec<u8>)> = vec![
        (
            cc::HIERARCHY_CONTROL,
            vec![OWNER],
            cat(&[&u32b(OWNER), &[0]]),
        ),
        (cc::HIERARCHY_CHANGE_AUTH, vec![OWNER], tpm2b(b"")),
        (cc::CLEAR, vec![PLATFORM], Vec::new()),
        (cc::CLEAR_CONTROL, vec![PLATFORM], vec![0]),
        (
            cc::NV_DEFINE_SPACE,
            vec![OWNER],
            cat(&[&tpm2b(b""), &tpm2b(&nv_pub)]),
        ),
        (cc::NV_UNDEFINE_SPACE, vec![OWNER, idx], Vec::new()),
        (
            cc::NV_WRITE,
            vec![OWNER, idx],
            cat(&[&tpm2b(&[1]), &u16b(0)]),
        ),
        (cc::NV_READ, vec![OWNER, idx], cat(&[&u16b(1), &u16b(0)])),
        (cc::NV_WRITE_LOCK, vec![OWNER, idx], Vec::new()),
        (cc::NV_READ_LOCK, vec![OWNER, idx], Vec::new()),
        (cc::PCR_EXTEND, vec![1], sha256_ha(&[0; 32])),
    ];
    for (code, handles, params) in cases {
        err(
            &run(&mut t, frame(st::SESSIONS, code, &handles, &area, &params)),
            Rc::reference_session(0),
        );
    }
    // Nothing happened: the index is intact.
    assert_eq!(handles_of(&mut t, NV0), vec![idx]);
}

#[test]
fn secrets_of_the_new_state_do_not_appear_in_debug_output() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"topsecret"));
    let _s = start_hmac(&mut t, OWNER, b"topsecret", &[1; 16]);
    ok(&define_raw(
        &mut t,
        OWNER,
        b"topsecret",
        b"",
        &nv_public(NV0 + 1, alg::SHA256, OWNER_RW, &[], 4),
    ));
    ok(&nv_write(
        &mut t,
        OWNER,
        NV0 + 1,
        b"topsecret",
        &[0x5A; 4],
        0,
    ));
    let text = format!("{t:?}");
    assert!(!text.contains("topsecret") && !text.contains("seed") && !text.contains("auth"));
}

#[test]
fn fuzzed_phase_b_commands_never_panic_and_always_answer_well_formed() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&define(
        &mut t,
        OWNER,
        idx,
        OWNER_RW | attr::AUTHREAD | attr::AUTHWRITE,
        16,
    ));
    ok(&nv_write(&mut t, OWNER, idx, b"", &[1; 16], 0));
    let s = start_hmac(&mut t, OWNER, b"", &[0x42; 16]);
    let hmac_cmd = hmac_command(
        &s,
        &hc(
            cc::HIERARCHY_CHANGE_AUTH,
            &[OWNER],
            tpm2b(b""),
            (OWNER, b""),
        ),
        false,
        false,
    );
    let seeds = [
        hmac_cmd,
        start_cmd(
            NULL,
            OWNER,
            &start_params(&[1; 16], &[], se::HMAC, &u16b(alg::NULL), alg::SHA256),
        ),
        plain(cc::FLUSH_CONTEXT, &[], &u32b(0x0200_0000)),
        with_pw(
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            &[b""],
            &cat(&[&u32b(OWNER), &[1]]),
        ),
        with_pw(
            cc::HIERARCHY_CHANGE_AUTH,
            &[ENDORSEMENT],
            &[b""],
            &tpm2b(b"e"),
        ),
        with_pw(cc::CLEAR_CONTROL, &[LOCKOUT], &[b""], &[1]),
        with_pw(
            cc::NV_DEFINE_SPACE,
            &[OWNER],
            &[b""],
            &cat(&[
                &tpm2b(b"a"),
                &tpm2b(&nv_public(NV0 + 2, alg::SHA256, OWNER_RW, &[], 8)),
            ]),
        ),
        with_pw(
            cc::NV_WRITE,
            &[OWNER, idx],
            &[b""],
            &cat(&[&tpm2b(b"xy"), &u16b(2)]),
        ),
        with_pw(
            cc::NV_READ,
            &[idx, idx],
            &[b""],
            &cat(&[&u16b(4), &u16b(0)]),
        ),
        with_pw(cc::NV_UNDEFINE_SPACE, &[OWNER, NV0 + 2], &[b""], &[]),
        plain(cc::NV_READ_PUBLIC, &[idx], &[]),
        with_pw(cc::NV_WRITE_LOCK, &[OWNER, idx], &[b""], &[]),
        with_pw(cc::NV_READ_LOCK, &[OWNER, idx], &[b""], &[]),
    ];
    let mut state = 0x9E37_79B9u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) as usize
    };
    for seed in &seeds {
        for len in 0..=seed.len() {
            let _ = run(&mut t, seed[..len].to_vec());
        }
        for _ in 0..300 {
            let mut c = seed.clone();
            for _ in 0..1 + next() % 3 {
                let i = next() % c.len();
                c[i] = next() as u8;
            }
            if next() % 2 == 0 {
                let n = c.len() as u32;
                c[2..6].copy_from_slice(&n.to_be_bytes());
            }
            let _ = run(&mut t, c);
        }
    }
}
