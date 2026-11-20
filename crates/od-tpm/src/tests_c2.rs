//! Command-level tests of slice C2 of phase B (S7-23): policy sessions and the policy commands,
//! `TPM2_ContextSave/ContextLoad`, `TPM2_HMAC` with keyed-hash objects, `TPM2_Quote` and
//! `TPM2_Certify`. Every command goes through `Tpm::execute` with raw bytes.
//!
//! Independent evidence (computed 2026-11-07 with Python `hashlib`/`hmac`, not with the crate):
//! the policy digests (`HpolicyAlg(old || TPM_CC || data)`), the RFC 4231 HMAC vectors, and the
//! attestation signatures: the attested structures are assembled here byte by byte from Part 2
//! v184, clause 10.12 and the signatures of the golden platform keys (RSASSA and deterministic
//! ECDSA) were verified by plain Python integer arithmetic (`pow` for RSASSA-PKCS1-v1_5, an
//! affine-coordinate P-256 verification) and pinned. The policy-session client is the HMAC
//! client of `tests_b.rs`, written from the formulas of Part 1.

use sha2::{Digest, Sha256};

use super::tests_b::{
    Hc, Hs, LOCKOUT, NV0, define, define_raw, hc, hierarchy_control, hmac_command, nv_public,
    nv_read, nv_read_data, nv_read_public, nv_write, pw_cmd, run_hmac, set_auth, shutdown,
    split_response, start_hmac, start_session, startup,
};
use super::tests_obj::*;
use super::*;
use crate::codes::{object_attr as oa, se, su};
use crate::marshal::{PublicParams, Scheme, TpmtPublic, Unique};
use crate::nv::attr as nv_attr;
use crate::policy::PolicyHashRestriction;

// ------------------------------------------------------------------------------- vectors

/// `TPM_ST_AUTH_SECRET`, `TPM_ST_ATTEST_CERTIFY`, `TPM_ST_ATTEST_QUOTE` (Part 2, Table 23),
/// written out here and not taken from `codes.rs`.
const ST_AUTH_SECRET: u16 = 0x8023;
const ST_ATTEST_CERTIFY: u16 = 0x8017;
const ST_ATTEST_QUOTE: u16 = 0x8018;

/// Python: `H(0^32 || 0000016B)`.
const V_AUTH_VALUE: &str = "8fcd2169ab92694e0c633f1ab772842b8241bbc20288981fc7ac1eddc1fddb0e";
/// Python: `H(0^32 || 0000016C || 0000015D)` (PolicyCommandCode(TPM2_Sign)).
const V_CC_SIGN: &str = "cc6918b226273b08f5bd406d7f10cf160f0a7d13dfd83b7770ccbcd1aa80d811";
/// Python: `H(0^32 || 0000016C || 00000148)` (PolicyCommandCode(TPM2_Certify)).
const V_CC_CERTIFY: &str = "048e9a3ace08583f79f344ff785bbea9f07ac7fa3325b3d49a21dd5194c65850";
/// Python: SHA-256 of PCR 0 (zeros) and PCR 4 (extended with `abc`), concatenated.
const V_PCR_DIGEST: &str = "b5ab2eaee749a8f5fe3e847815d70e8c15332cb6ab8a80491cfe7afc8dd7f8bc";
/// Python: `H(0^32 || 0000017F || 00000001 000B 03 110000 || V_PCR_DIGEST)`.
const V_POLICY_PCR: &str = "d0305547604cac3f74d18af0548e49d59036cbf6ceedb7501eb2cf338821edbc";
/// Python: `H(0^32 || 00000171 || V_POLICY_PCR || V_AUTH_VALUE)`.
const V_POLICY_OR: &str = "8677bb2b58f4501e339bc2da7548bae629508ad275d7ef6a52c0f03b20c7d213";
/// Python: `H(H(0^32 || 00000151 || 40000001) || "ref")` (PolicySecret of the owner).
const V_SECRET_OWNER: &str = "9586343a9dc675ef35af0f7d375bf6554fd161bee76216eb67f1ebcc901c9265";
/// Python: PolicyAuthValue then PolicyCommandCode(TPM2_Sign).
const V_AV_THEN_CC_SIGN: &str = "d9979a6b278c1d135ce124837caf9de446d714718eee9e3620b58c80a043a953";

pub(super) fn h256(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().to_vec()
}

// ------------------------------------------------------------------------------- helpers

fn policy_session(t: &mut Tpm, kind: u8) -> Hs {
    start_session(t, kind, NULL, b"", &[0x44; 16])
}

fn pol(t: &mut Tpm, code: u32, s: &Hs, p: &[u8]) -> Vec<u8> {
    run(t, plain(code, &[s.handle], p))
}

fn digest(t: &mut Tpm, s: &Hs) -> Vec<u8> {
    let r = pol(t, cc::POLICY_GET_DIGEST, s, &[]);
    let mut rd = Reader::new(params(&r, 0));
    let d = rd.tpm2b(64).unwrap().to_vec();
    rd.finish().unwrap();
    d
}

fn pol_cc(t: &mut Tpm, s: &Hs, code: u32) -> Vec<u8> {
    pol(t, cc::POLICY_COMMAND_CODE, s, &u32b(code))
}

fn set_primary_policy(
    t: &mut Tpm,
    hierarchy: u32,
    auth: &[u8],
    policy: &[u8],
    hash: u16,
) -> Vec<u8> {
    pw_cmd(
        t,
        cc::SET_PRIMARY_POLICY,
        &[hierarchy],
        auth,
        &cat(&[&tpm2b(policy), &u16b(hash)]),
    )
}

#[test]
fn primary_policy_uses_current_authorization_and_reports_explicit_zero() {
    let mut t = started();
    let mut zero = policy_session(&mut t, se::POLICY);
    let set = hc(
        cc::SET_PRIMARY_POLICY,
        &[OWNER],
        cat(&[&tpm2b(&[4; 32]), &u16b(alg::SHA256)]),
        (OWNER, b""),
    );
    err(
        &use_session(&mut t, &mut zero, &set, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &[0; 32],
        alg::SHA256,
    ));
    let response = get_cap(&mut t, cap::AUTH_POLICIES, 0x4000_0000, 2);
    let (more, selector, body, count) = cap_list(&response);
    assert!(more && selector == 9 && count == 2);
    assert_eq!(
        body,
        cat(&[
            &u32b(OWNER),
            &u16b(alg::SHA256),
            &[0; 32],
            &u32b(LOCKOUT),
            &u16b(alg::NULL),
        ])
    );
    let response = get_cap(&mut t, cap::AUTH_POLICIES, ENDORSEMENT, 6);
    let (more, _, body, count) = cap_list(&response);
    assert!(!more && count == 2);
    assert_eq!(
        body,
        cat(&[
            &u32b(ENDORSEMENT),
            &u16b(alg::NULL),
            &u32b(PLATFORM),
            &u16b(alg::NULL)
        ])
    );
    let response = get_cap(&mut t, cap::AUTH_POLICIES, OWNER, 0);
    assert_eq!(
        cap_list(&response),
        (true, cap::AUTH_POLICIES, Vec::new(), 0)
    );
    let response = get_cap(&mut t, cap::AUTH_POLICIES, PLATFORM + 1, 10);
    assert_eq!(
        cap_list(&response),
        (false, cap::AUTH_POLICIES, Vec::new(), 0)
    );
    err(
        &get_cap(&mut t, cap::AUTH_POLICIES, 0, 1),
        Rc::VALUE.in_param(2),
    );

    ok(&use_session(&mut t, &mut zero, &set, 0, true, false));
    assert_eq!(t.permanent_auth_policy(OWNER), Some(&[4; 32]));
    err(
        &use_session(&mut t, &mut zero, &set, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&set_auth(&mut t, OWNER, b"", b"new owner"));
    assert_eq!(t.permanent_auth_policy(OWNER), Some(&[4; 32]));
    err(
        &set_primary_policy(&mut t, OWNER, b"", b"", alg::NULL),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, b"new owner", &[1; 31], alg::SHA256),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, b"new owner", &[1; 32], alg::NULL),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, b"new owner", b"", 0x000c),
        Rc::HASH.in_param(2),
    );
    err(
        &set_primary_policy(&mut t, NULL, b"", b"", alg::NULL),
        Rc::VALUE.in_handle(1),
    );
    assert_eq!(t.permanent_auth_policy(OWNER), Some(&[4; 32]));
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"new owner",
        b"",
        alg::NULL,
    ));
    assert_eq!(t.permanent_auth_policy(OWNER), None);

    ok(&set_primary_policy(
        &mut t,
        ENDORSEMENT,
        b"",
        &[2; 32],
        alg::SHA256,
    ));
    ok(&super::tests_b::hierarchy_control(
        &mut t,
        PLATFORM,
        b"",
        ENDORSEMENT,
        0,
    ));
    err(
        &set_primary_policy(&mut t, ENDORSEMENT, b"", b"", alg::NULL),
        Rc::HIERARCHY.in_handle(1),
    );
    let response = get_cap(&mut t, cap::AUTH_POLICIES, ENDORSEMENT, 1);
    assert_eq!(
        cap_list(&response).2,
        cat(&[&u32b(ENDORSEMENT), &u16b(alg::SHA256), &[2; 32]])
    );
}

#[test]
fn hierarchy_policies_follow_reset_resume_clear_and_snapshot() {
    let mut t = started();
    let policies = [
        (OWNER, [1; 32]),
        (ENDORSEMENT, [2; 32]),
        (LOCKOUT, [3; 32]),
        (PLATFORM, [4; 32]),
    ];
    for (hierarchy, policy) in policies {
        ok(&set_primary_policy(
            &mut t,
            hierarchy,
            b"",
            &policy,
            alg::SHA256,
        ));
    }
    let restored = Tpm::restore(&t.persistent_snapshot(), &ENTROPY).unwrap();
    for (hierarchy, policy) in policies.into_iter().take(3) {
        assert_eq!(restored.permanent_auth_policy(hierarchy), Some(&policy));
    }
    assert_eq!(restored.permanent_auth_policy(PLATFORM), None);

    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::STATE);
    for (hierarchy, policy) in policies {
        assert_eq!(t.permanent_auth_policy(hierarchy), Some(&policy));
    }
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::CLEAR);
    assert_eq!(t.permanent_auth_policy(PLATFORM), None);
    for (hierarchy, policy) in policies.into_iter().take(3) {
        assert_eq!(t.permanent_auth_policy(hierarchy), Some(&policy));
    }
    ok(&set_primary_policy(
        &mut t,
        PLATFORM,
        b"",
        &[4; 32],
        alg::SHA256,
    ));
    ok(&pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]));
    for (hierarchy, _) in policies.into_iter().take(3) {
        assert_eq!(t.permanent_auth_policy(hierarchy), None);
    }
    assert_eq!(t.permanent_auth_policy(PLATFORM), Some(&[4; 32]));
    t.init();
    startup(&mut t, su::CLEAR);
    assert_eq!(t.permanent_auth_policy(PLATFORM), None);
}

#[test]
fn hierarchy_policy_restricts_create_primary_and_recovers_lockout_auth() {
    let mut t = started();
    let key = ecc_sign();
    let template_hash = h256(&[&key.to_bytes()]);
    let policy = h256(&[&[0; 32], &u32b(cc::POLICY_TEMPLATE), &template_hash]);
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &policy,
        alg::SHA256,
    ));
    let mut primary = policy_session(&mut t, se::POLICY);
    ok(&pol(
        &mut t,
        cc::POLICY_TEMPLATE,
        &primary,
        &tpm2b(&template_hash),
    ));
    let mut changed = key.clone();
    changed.attributes |= oa::NO_DA;
    let mut create = hc(
        cc::CREATE_PRIMARY,
        &[OWNER],
        creation_params(b"", b"", &changed, b"other", &NO_PCR),
        (OWNER, b""),
    );
    err(
        &use_session(&mut t, &mut primary, &create, 1, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    create.params = creation_params(b"", b"", &key, b"other", &NO_PCR);
    let result = use_session(&mut t, &mut primary, &create, 1, true, false);
    let _ = parse_primary(&result);
    ok(&flush(&mut t, primary.handle));

    let recovery = h256(&[
        &[0; 32],
        &u32b(cc::POLICY_COMMAND_CODE),
        &u32b(cc::DICTIONARY_ATTACK_LOCK_RESET),
    ]);
    ok(&set_auth(&mut t, LOCKOUT, b"", b"secret"));
    ok(&set_primary_policy(
        &mut t,
        LOCKOUT,
        b"secret",
        &recovery,
        alg::SHA256,
    ));
    err(
        &pw_cmd(
            &mut t,
            cc::DICTIONARY_ATTACK_LOCK_RESET,
            &[LOCKOUT],
            b"wrong",
            &[],
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &pw_cmd(
            &mut t,
            cc::DICTIONARY_ATTACK_LOCK_RESET,
            &[LOCKOUT],
            b"secret",
            &[],
        ),
        Rc::LOCKOUT,
    );
    let mut session = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &session, cc::DICTIONARY_ATTACK_LOCK_RESET));
    let reset = hc(
        cc::DICTIONARY_ATTACK_LOCK_RESET,
        &[LOCKOUT],
        Vec::new(),
        (LOCKOUT, b""),
    );
    ok(&use_session(&mut t, &mut session, &reset, 0, true, false));
    ok(&pw_cmd(
        &mut t,
        cc::DICTIONARY_ATTACK_LOCK_RESET,
        &[LOCKOUT],
        b"secret",
        &[],
    ));
}

fn pol_pcr(t: &mut Tpm, s: &Hs, d: &[u8], sel: &[u8]) -> Vec<u8> {
    pol(t, cc::POLICY_PCR, s, &cat(&[&tpm2b(d), sel]))
}

fn or_params(list: &[&[u8]]) -> Vec<u8> {
    let mut p = u32b(list.len() as u32);
    for d in list {
        p.extend_from_slice(&tpm2b(d));
    }
    p
}

fn pol_or(t: &mut Tpm, s: &Hs, list: &[&[u8]]) -> Vec<u8> {
    pol(t, cc::POLICY_OR, s, &or_params(list))
}

/// Selection of PCR 0 and PCR 4 in the SHA-256 bank.
pub(super) fn sel04() -> Vec<u8> {
    hex("00000001000b03110000")
}

fn secret_params(nonce: &[u8], cp: &[u8], pref: &[u8], expiration: u32) -> Vec<u8> {
    cat(&[&tpm2b(nonce), &tpm2b(cp), &tpm2b(pref), &u32b(expiration)])
}

/// `PolicySecret` of `auth_handle` authorized by a password.
fn secret_pw(t: &mut Tpm, auth_handle: u32, pw: &[u8], s: &Hs, p: &[u8]) -> Vec<u8> {
    run(
        t,
        with_pw(cc::POLICY_SECRET, &[auth_handle, s.handle], &[pw], p),
    )
}

#[test]
fn low_range_eks_require_endorsement_policy_to_use_as_storage_parents() {
    use crate::ek::{LowRangeEk, POLICY_A};

    let mut t = started();
    let empty_secret = secret_params(b"", b"", b"", 0);
    let mut endorsement_policy = policy_session(&mut t, se::POLICY);
    let mut owner_policy = policy_session(&mut t, se::POLICY);
    ok(&secret_pw(&mut t, OWNER, b"", &owner_policy, &empty_secret));
    let mut hmac = start_hmac(&mut t, NULL, b"", &[0x45; 16]);

    for profile in [LowRangeEk::Rsa2048, LowRangeEk::EccP256] {
        let response = create_primary(&mut t, ENDORSEMENT, b"", &profile.template());
        ok(&response);
        let ek = parse_primary(&response);
        let child = sealed();
        let child_data = b"test";
        let policy_fail = Rc::POLICY_FAIL.in_session(1);
        err(
            &create_child(&mut t, ek.handle, b"", b"", child_data, &child),
            policy_fail,
        );
        let inputs = creation_params(b"", child_data, &child, b"", &NO_PCR);
        let command = hc(cc::CREATE, &[ek.handle], inputs, (ek.handle, &ek.name));
        err(
            &use_session(&mut t, &mut hmac, &command, 0, false, false),
            policy_fail,
        );
        err(
            &use_session(&mut t, &mut owner_policy, &command, 0, true, false),
            policy_fail,
        );

        ok(&secret_pw(
            &mut t,
            ENDORSEMENT,
            b"",
            &endorsement_policy,
            &empty_secret,
        ));
        let first = Sha256::digest(cat(&[&[0; 32], &hex("000001514000000b")]));
        let expected = Sha256::digest(first);
        assert_eq!(POLICY_A[..], expected[..]);
        assert_eq!(digest(&mut t, &endorsement_policy), POLICY_A);
        ok(&use_session(
            &mut t,
            &mut endorsement_policy,
            &command,
            0,
            true,
            false,
        ));
        ok(&flush(&mut t, ek.handle));
    }
}

/// Runs a command authorized by the policy or HMAC session `s`; the client nonce rolls on success.
fn use_session(t: &mut Tpm, s: &mut Hs, c: &Hc, nh: usize, empty: bool, corrupt: bool) -> Vec<u8> {
    let resp = run(t, hmac_command(s, c, empty, corrupt));
    if rc(&resp) == 0 {
        let (_, acks) = split_response(&resp, nh);
        s.nonce_tpm = acks[0].0.clone();
    }
    resp
}

/// Creates and loads a child; `(handle, name, public)`.
pub(super) fn make_child(
    t: &mut Tpm,
    parent: u32,
    public: &TpmtPublic,
    auth: &[u8],
    data: &[u8],
) -> (u32, Vec<u8>, Vec<u8>) {
    let c = parse_child(&create_child(t, parent, b"parent-pw", auth, data, public));
    let (h, name) = parse_loaded(&load(t, parent, b"parent-pw", &c.private, &c.public));
    (h, name, c.public)
}

fn policy_key(auth_policy: &[u8]) -> TpmtPublic {
    let mut p = ecc_sign();
    p.attributes &= !oa::USER_WITH_AUTH;
    p.auth_policy = auth_policy.to_vec();
    p
}

fn sign_command(key: u32, name: &[u8], key_auth: &[u8]) -> Hc {
    let digest = [7u8; 32];
    let mut c = hc(
        cc::SIGN,
        &[key],
        sign_params(&digest, &ecc_sig_scheme()),
        (key, key_auth),
    );
    c.names = vec![name.to_vec()];
    c
}

fn handles_listed(t: &mut Tpm, property: u32) -> Vec<u32> {
    super::tests_b::handles_of(t, property)
}

// ------------------------------------------------------------------- session start and flush

#[test]
fn policy_and_trial_sessions_start_with_a_zero_digest_and_type_3_handles() {
    let mut t = started();
    let p = policy_session(&mut t, se::POLICY);
    let tr = policy_session(&mut t, se::TRIAL);
    let h = start_hmac(&mut t, NULL, b"", &[0x55; 16]);
    assert_eq!(p.handle >> 24, 3);
    assert_eq!(tr.handle >> 24, 3);
    assert_eq!(h.handle >> 24, 2);
    assert_ne!(p.handle, tr.handle);
    assert_eq!(digest(&mut t, &p), vec![0u8; 32]);
    assert_eq!(digest(&mut t, &tr), vec![0u8; 32]);
    // TPM_CAP_HANDLES with the loaded-session type lists both families under their own types.
    let loaded = handles_listed(&mut t, 0x0200_0000);
    assert_eq!(loaded.len(), 3);
    assert!(loaded.contains(&p.handle) && loaded.contains(&h.handle));
    // An HMAC handle is not a policy session (TPM_RC_VALUE, handle 1); an unloaded one is
    // TPM_RC_REFERENCE_H0; a non-session handle is TPM_RC_VALUE.
    for (code, params_) in [
        (cc::POLICY_GET_DIGEST, vec![]),
        (cc::POLICY_RESTART, vec![]),
    ] {
        err(
            &run(&mut t, plain(code, &[h.handle], &params_)),
            Rc::VALUE.in_handle(1),
        );
        err(
            &run(&mut t, plain(code, &[0x0300_0020], &params_)),
            Rc::reference_handle(0),
        );
        err(
            &run(&mut t, plain(code, &[OWNER], &params_)),
            Rc::VALUE.in_handle(1),
        );
    }
    // FlushContext of a policy session; a second flush finds nothing.
    ok(&flush(&mut t, p.handle));
    err(&flush(&mut t, p.handle), Rc::HANDLE.in_param(1));
    err(
        &run(&mut t, plain(cc::POLICY_GET_DIGEST, &[p.handle], &[])),
        Rc::reference_handle(0),
    );
    // Startup flushes every session.
    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    assert!(handles_listed(&mut t, 0x0200_0000).is_empty());
}

// ---------------------------------------------------------------------- policy commands

#[test]
fn policy_digests_match_the_independent_extend_vectors() {
    let mut t = started();
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    // PolicyAuthValue.
    let s = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &s, &[]));
    assert_eq!(digest(&mut t, &s), hex(V_AUTH_VALUE));
    let trial = policy_session(&mut t, se::TRIAL);
    err(&pol(&mut t, cc::POLICY_PASSWORD, &trial, &[0]), Rc::SIZE);
    assert_eq!(digest(&mut t, &trial), [0; 32]);
    ok(&pol(&mut t, cc::POLICY_PASSWORD, &trial, &[]));
    assert_eq!(digest(&mut t, &trial), hex(V_AUTH_VALUE));
    ok(&flush(&mut t, trial.handle));
    // PolicyCommandCode(Sign), also on a trial session.
    let s = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    assert_eq!(digest(&mut t, &s), hex(V_CC_SIGN));
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_cc(&mut t, &tr, cc::SIGN));
    assert_eq!(digest(&mut t, &tr), hex(V_CC_SIGN));
    // PolicyPCR with the right digest, with an empty digest (the TPM's own) and on a trial session
    // with a caller-supplied digest (no PCR is checked there).
    let right = hex(V_PCR_DIGEST);
    let s = policy_session(&mut t, se::POLICY);
    ok(&pol_pcr(&mut t, &s, &right, &sel04()));
    assert_eq!(digest(&mut t, &s), hex(V_POLICY_PCR));
    let s2 = policy_session(&mut t, se::POLICY);
    ok(&pol_pcr(&mut t, &s2, &[], &sel04()));
    assert_eq!(digest(&mut t, &s2), hex(V_POLICY_PCR));
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_pcr(&mut t, &tr, &[9u8; 32], &sel04()));
    assert_eq!(
        digest(&mut t, &tr),
        h256(&[&[0u8; 32], &u32b(cc::POLICY_PCR), &sel04(), &[9u8; 32]])
    );
    // PolicyOR: the digest of `s` is on the list; the result is `H(0 || CC || list)`.
    ok(&pol_or(
        &mut t,
        &s,
        &[&hex(V_POLICY_PCR), &hex(V_AUTH_VALUE)],
    ));
    assert_eq!(digest(&mut t, &s), hex(V_POLICY_OR));
    // PolicySecret(owner, policyRef "ref"), the authorization being a password.
    let s = policy_session(&mut t, se::POLICY);
    let r = secret_pw(&mut t, OWNER, b"", &s, &secret_params(&[], &[], b"ref", 0));
    let mut rd = Reader::new(params(&r, 0));
    assert!(rd.tpm2b(64).unwrap().is_empty(), "empty timeout");
    assert_eq!(rd.u16().unwrap(), ST_AUTH_SECRET);
    assert_eq!(rd.u32().unwrap(), NULL);
    assert!(rd.tpm2b(64).unwrap().is_empty(), "NULL ticket");
    rd.finish().unwrap();
    assert_eq!(digest(&mut t, &s), hex(V_SECRET_OWNER));
    // The chain used by the object tests below.
    let s = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &s, &[]));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    assert_eq!(digest(&mut t, &s), hex(V_AV_THEN_CC_SIGN));
}

#[test]
fn policy_pcr_refuses_a_wrong_digest_and_a_changed_pcr() {
    let mut t = started();
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    let s = policy_session(&mut t, se::POLICY);
    // A wrong digest: TPM_RC_VALUE (parameter 1), the policy digest is unchanged.
    err(
        &pol_pcr(&mut t, &s, &[1u8; 32], &sel04()),
        Rc::VALUE.in_param(1),
    );
    assert_eq!(digest(&mut t, &s), vec![0u8; 32]);
    // A digest of another size cannot match either; above one digest it is a size error.
    err(
        &pol_pcr(&mut t, &s, &[1u8; 5], &sel04()),
        Rc::VALUE.in_param(1),
    );
    err(
        &pol_pcr(&mut t, &s, &[1u8; 33], &sel04()),
        Rc::SIZE.in_param(1),
    );
    // Malformed selections: another bank, a wrong sizeofSelect, trailing octets, too few octets.
    err(
        &pol_pcr(&mut t, &s, &[], &hex("00000001000400110000")[..]),
        Rc::HASH.in_param(2),
    );
    err(
        &pol_pcr(&mut t, &s, &[], &hex("00000001000b0211")[..]),
        Rc::VALUE.in_param(2),
    );
    err(&pol_pcr(&mut t, &s, &[], &cat(&[&sel04(), &[0]])), Rc::SIZE);
    err(
        &pol_pcr(&mut t, &s, &[], &sel04()[..6]),
        Rc::INSUFFICIENT.in_param(2),
    );
    // The right digest works; then a PCR changes: a later PolicyPCR on the same session reports
    // TPM_RC_PCR_CHANGED (Part 3, clause 23.7.1) and PolicyRestart clears the memory.
    ok(&pol_pcr(&mut t, &s, &hex(V_PCR_DIGEST), &sel04()));
    ok(&extend(&mut t, 5, &hex(SHA256_ABC)));
    err(&pol_pcr(&mut t, &s, &[], &sel04()), Rc::PCR_CHANGED);
    ok(&pol(&mut t, cc::POLICY_RESTART, &s, &[]));
    assert_eq!(digest(&mut t, &s), vec![0u8; 32]);
    ok(&pol_pcr(&mut t, &s, &[], &sel04()));
    // An empty selection hashes the empty string.
    let e = policy_session(&mut t, se::POLICY);
    ok(&pol_pcr(&mut t, &e, &hex(SHA256_EMPTY), &hex("00000000")));
    assert_eq!(
        digest(&mut t, &e),
        h256(&[
            &[0u8; 32],
            &u32b(cc::POLICY_PCR),
            &hex("00000000"),
            &hex(SHA256_EMPTY)
        ])
    );
}

#[test]
fn policy_pcr_trial_keeps_banks_that_are_inactive_locally() {
    let mut t = started();
    ok(&run(
        &mut t,
        with_pw(
            cc::PCR_ALLOCATE,
            &[PLATFORM],
            &[b""],
            &hex("00000001000b03000000"),
        ),
    ));
    t.init();
    startup(&mut t, 0);
    let requested = hex("00000002000b03010000000c03010000");
    let filtered = hex("00000002000b03000000000c03010000");
    let cc = u32b(cc::POLICY_PCR);
    let current = h256(&[&[0; 48]]);
    let actual = policy_session(&mut t, 1);
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        &actual,
        &cat(&[&tpm2b(&current), &requested]),
    ));
    assert_eq!(
        digest(&mut t, &actual),
        h256(&[&[0; 32], &cc, &filtered, &current])
    );

    for (input, expected) in [(&[0x45; 32][..], &[0x45; 32][..]), (&[][..], &current[..])] {
        let trial = policy_session(&mut t, 3);
        ok(&pol(
            &mut t,
            cc::POLICY_PCR,
            &trial,
            &cat(&[&tpm2b(input), &requested]),
        ));
        assert_eq!(
            digest(&mut t, &trial),
            h256(&[&[0; 32], &cc, &requested, expected])
        );
    }
}

#[test]
fn policy_or_needs_two_digests_and_a_matching_one() {
    let mut t = started();
    let s = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    let d = hex(V_CC_SIGN);
    // The current digest is not on the list: TPM_RC_VALUE, nothing changes.
    err(
        &pol_or(&mut t, &s, &[&[1u8; 32], &[2u8; 32]]),
        Rc::VALUE.in_param(1),
    );
    assert_eq!(digest(&mut t, &s), d);
    // Fewer than two entries, more than eight, trailing octets.
    err(&pol_or(&mut t, &s, &[&d]), Rc::SIZE.in_param(1));
    err(&pol_or(&mut t, &s, &[]), Rc::SIZE.in_param(1));
    let nine: Vec<&[u8]> = vec![&d; 9];
    err(&pol_or(&mut t, &s, &nine), Rc::SIZE.in_param(1));
    let mut long = or_params(&[&d, &d]);
    long.push(0);
    err(&pol(&mut t, cc::POLICY_OR, &s, &long), Rc::SIZE);
    // An entry of another size is not an error but cannot match.
    err(
        &pol_or(&mut t, &s, &[&[1u8; 5], &[2u8; 7]]),
        Rc::VALUE.in_param(1),
    );
    // It matches at any position; the result is the same for the same list.
    ok(&pol_or(&mut t, &s, &[&[1u8; 32], &d, &[3u8; 32]]));
    let after = digest(&mut t, &s);
    assert_eq!(
        after,
        h256(&[&[0u8; 32], &u32b(cc::POLICY_OR), &[1u8; 32], &d, &[3u8; 32]])
    );
    // A trial session assumes the digest matches.
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_or(&mut t, &tr, &[&[1u8; 32], &d, &[3u8; 32]]));
    assert_eq!(digest(&mut t, &tr), after);
}

#[test]
fn policy_command_code_checks_the_code() {
    let mut t = started();
    let s = policy_session(&mut t, se::POLICY);
    // A command that this TPM does not implement: TPM_RC_POLICY_CC (parameter 1).
    err(
        &pol_cc(&mut t, &s, cc::SEQUENCE_UPDATE + 0x100),
        Rc::POLICY_CC.in_param(1),
    );
    assert_eq!(digest(&mut t, &s), vec![0u8; 32]);
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    // The same code again is fine, a different one is TPM_RC_VALUE.
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    err(&pol_cc(&mut t, &s, cc::UNSEAL), Rc::VALUE.in_param(1));
    // Short and long parameters.
    err(
        &pol(&mut t, cc::POLICY_COMMAND_CODE, &s, &[0, 0]),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(&pol(&mut t, cc::POLICY_COMMAND_CODE, &s, &[0; 5]), Rc::SIZE);
    // PolicyRestart forgets the command code.
    ok(&pol(&mut t, cc::POLICY_RESTART, &s, &[]));
    ok(&pol_cc(&mut t, &s, cc::UNSEAL));
    // A trial session records the digest of any code, implemented or not.
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_cc(&mut t, &tr, 0x0000_0137 + 0x100));
}

#[test]
fn policy_cp_hash_pins_a_command_across_context_save_and_load() {
    let mut t = started();
    let policy = policy_session(&mut t, se::POLICY);
    let cp_hash = [0x42; 32];
    err(
        &pol(&mut t, cc::POLICY_CP_HASH, &policy, &tpm2b(&cp_hash[..31])),
        Rc::SIZE.in_param(1),
    );
    let encoded = tpm2b(&cp_hash);
    let first = h256(&[&[0; 32], &[0, 0, 1, 0x6e], &cp_hash]);
    ok(&pol(&mut t, cc::POLICY_CP_HASH, &policy, &encoded));
    assert_eq!(digest(&mut t, &policy), first);
    err(
        &pol(&mut t, cc::POLICY_CP_HASH, &policy, &tpm2b(&[0x43; 32])),
        Rc::CPHASH,
    );
    assert_eq!(digest(&mut t, &policy), first);
    ok(&pol(&mut t, cc::POLICY_CP_HASH, &policy, &encoded));
    let second = h256(&[&first, &[0, 0, 1, 0x6e], &cp_hash]);
    assert_eq!(digest(&mut t, &policy), second);

    let saved = params(&save(&mut t, policy.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
    let slot = t.sessions.slot_of(policy.handle).unwrap();
    assert_eq!(
        t.policy_use_check(
            slot,
            (cc::SIGN, NULL),
            |rule| rule == PolicyHashRestriction::CpHash(cp_hash),
            &second,
            false,
            1
        ),
        Ok((false, false))
    );
    assert_eq!(
        t.policy_use_check(
            slot,
            (cc::SIGN, NULL),
            |rule| rule == PolicyHashRestriction::CpHash([0x43; 32]),
            &second,
            false,
            1
        ),
        Err(Rc::POLICY_FAIL.in_session(1))
    );
    ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
    ok(&pol(
        &mut t,
        cc::POLICY_CP_HASH,
        &policy,
        &tpm2b(&[0x43; 32]),
    ));

    let trial = policy_session(&mut t, se::TRIAL);
    ok(&pol(&mut t, cc::POLICY_CP_HASH, &trial, &encoded));
    assert_eq!(digest(&mut t, &trial), first);
}

#[test]
fn policy_name_hash_has_one_shared_restriction_and_a_fixed_digest() {
    let mut t = started();
    let real = policy_session(&mut t, se::POLICY);
    let name_hash = [1; 32];
    let encoded = tpm2b(&name_hash);
    let expected = hex("6201f0688e9638e4730cb773be7e8ab89e9af7c2851d3aa563c1254d892028b5");
    err(
        &pol(
            &mut t,
            cc::POLICY_NAME_HASH,
            &real,
            &tpm2b(&name_hash[..31]),
        ),
        Rc::SIZE.in_param(1),
    );
    ok(&pol(&mut t, cc::POLICY_NAME_HASH, &real, &encoded));
    assert_eq!(digest(&mut t, &real), expected);
    for code in [cc::POLICY_NAME_HASH, cc::POLICY_CP_HASH] {
        err(&pol(&mut t, code, &real, &encoded), Rc::CPHASH);
    }
    err(
        &pol(
            &mut t,
            cc::POLICY_NAME_HASH,
            &real,
            &tpm2b(&name_hash[..31]),
        ),
        Rc::CPHASH,
    );
    err(
        &pol(&mut t, cc::POLICY_NAME_HASH, &real, &tpm2b(&[0; 33])),
        Rc::SIZE.in_param(1),
    );
    err(
        &secret_pw(
            &mut t,
            OWNER,
            b"",
            &real,
            &secret_params(b"", &name_hash, b"", 0),
        ),
        Rc::CPHASH,
    );
    assert_eq!(digest(&mut t, &real), expected);
    ok(&secret_pw(
        &mut t,
        OWNER,
        b"",
        &real,
        &secret_params(b"", b"", b"", 0),
    ));
    err(
        &pol(&mut t, cc::POLICY_CP_HASH, &real, &encoded),
        Rc::CPHASH,
    );
    ok(&pol(&mut t, cc::POLICY_RESTART, &real, &[]));
    ok(&pol(&mut t, cc::POLICY_CP_HASH, &real, &encoded));
    err(
        &pol(&mut t, cc::POLICY_NAME_HASH, &real, &encoded),
        Rc::CPHASH,
    );

    let trial = policy_session(&mut t, se::TRIAL);
    ok(&pol(&mut t, cc::POLICY_NAME_HASH, &trial, &encoded));
    assert_eq!(digest(&mut t, &trial), expected);
    ok(&pol(&mut t, cc::POLICY_NAME_HASH, &trial, &encoded));
    assert_eq!(
        digest(&mut t, &trial),
        h256(&[&expected, &[0, 0, 1, 0x70], &name_hash])
    );
    let key_name = cat(&[&u16b(alg::SHA256), &[1; 32]]);
    let ticket = cat(&[&u16b(st::VERIFIED), &u32b(NULL), &tpm2b(b"")]);
    for (reference, vector) in [
        (
            b"".as_slice(),
            "90c719491286d75a4a8bbc92afece1ec35d71f597e06fcd6aeed14fd0ac004af",
        ),
        (
            b"test-ref".as_slice(),
            "33c94480a43f05904e915ecb9d5ea4bef1ec6ce12f5146f45dc5854ffcf23177",
        ),
    ] {
        let parameters = cat(&[&tpm2b(b""), &tpm2b(reference), &tpm2b(&key_name), &ticket]);
        ok(&pol(&mut t, cc::POLICY_AUTHORIZE, &trial, &parameters));
        assert_eq!(digest(&mut t, &trial), hex(vector));
    }
}

#[test]
fn policy_authorize_preserves_the_name_hash_across_context_save_and_authorization() {
    let (mut t, parent) = with_ecc_parent();
    let parent_name = unmarshal_name(&mut t, parent);
    let alias = 0x8100_0001;
    ok(&evict(&mut t, OWNER, b"", parent, alias));
    let authority = parse_primary(&create_primary(&mut t, OWNER, b"authority", &ecc_sign()));
    let policy_ref = b"approved name list";
    let authorize_args = |approved: &[u8], reference: &[u8], name: &[u8], ticket: &[u8]| {
        cat(&[&tpm2b(approved), &tpm2b(reference), &tpm2b(name), ticket])
    };
    let null_ticket = cat(&[&u16b(st::VERIFIED), &u32b(NULL), &tpm2b(b"")]);
    let trial = policy_session(&mut t, se::TRIAL);
    ok(&pol(
        &mut t,
        cc::POLICY_AUTHORIZE,
        &trial,
        &authorize_args(b"", policy_ref, &authority.name, &null_ticket),
    ));
    ok(&pol_cc(&mut t, &trial, cc::OBJECT_CHANGE_AUTH));
    let mut child = sealed();
    child.auth_policy = digest(&mut t, &trial);
    child.attributes |= oa::ADMIN_WITH_POLICY;
    ok(&flush(&mut t, trial.handle));
    let (key, key_name, public) = make_child(&mut t, parent, &child, b"old", b"sealed");
    let mut real = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_LOCALITY, &real, &[1]));
    ok(&pol(
        &mut t,
        cc::POLICY_NAME_HASH,
        &real,
        &tpm2b(&h256(&[&key_name, &parent_name])),
    ));
    let approved = digest(&mut t, &real);
    let signed = h256(&[&approved, policy_ref]);
    let signature = params(
        &sign(
            &mut t,
            authority.handle,
            b"authority",
            &signed,
            &ecc_sig_scheme(),
        ),
        0,
    )
    .to_vec();
    let ticket = params(&verify(&mut t, authority.handle, &signed, &signature), 0).to_vec();
    ok(&flush(&mut t, authority.handle));
    let other = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_storage()));
    for (parameters, expected) in [
        (
            authorize_args(b"", policy_ref, &authority.name, &ticket),
            Rc::VALUE.in_param(1),
        ),
        (
            authorize_args(&approved, b"altered", &authority.name, &ticket),
            Rc::POLICY,
        ),
        (
            authorize_args(&approved, policy_ref, &authority.name, &null_ticket),
            Rc::POLICY,
        ),
        (
            authorize_args(&approved, policy_ref, &[0, 0x10], &ticket),
            Rc::HASH.in_param(3),
        ),
        (
            authorize_args(&approved, policy_ref, &authority.name[..33], &ticket),
            Rc::SIZE.in_param(3),
        ),
    ] {
        err(
            &pol(&mut t, cc::POLICY_AUTHORIZE, &real, &parameters),
            expected,
        );
        assert_eq!(digest(&mut t, &real), approved);
    }
    ok(&pol(
        &mut t,
        cc::POLICY_AUTHORIZE,
        &real,
        &authorize_args(&approved, policy_ref, &authority.name, &ticket),
    ));
    ok(&pol_cc(&mut t, &real, cc::OBJECT_CHANGE_AUTH));
    assert_eq!(digest(&mut t, &real), child.auth_policy);
    let saved = params(&save(&mut t, real.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), real.handle);
    let slot = t.sessions.slot_of(real.handle).unwrap();
    assert_eq!(t.sessions.get(slot).unwrap().policy.command_locality, 1);

    let mut command = hc(
        cc::OBJECT_CHANGE_AUTH,
        &[key, other.handle],
        tpm2b(b"new"),
        (key, b"old"),
    );
    command.names = vec![key_name, other.name];
    err(
        &use_session(&mut t, &mut real, &command, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &real), child.auth_policy);
    command.handles[1] = alias;
    command.names[1] = parent_name;
    let result = use_session(&mut t, &mut real, &command, 0, true, false);
    let private = Reader::new(params(&result, 0)).tpm2b(1024).unwrap();
    ok(&flush(&mut t, key));
    let (key, _) = parse_loaded(&load(&mut t, alias, b"parent-pw", private, &public));
    assert_eq!(
        params(&pw_cmd(&mut t, cc::UNSEAL, &[key], b"new", &[]), 0),
        tpm2b(b"sealed")
    );
}

#[test]
fn policy_template_and_parameters_have_exclusive_hash_slots_and_fixed_digests() {
    let mut t = started();
    let digest_bytes = [1u8; 32];
    let encoded = tpm2b(&digest_bytes);
    for (code, vector) in [
        (
            cc::POLICY_TEMPLATE,
            "4fd0086d512ac28f5490da713b39cdcbd5264e7cada04cc66f4d6a5515df8d17",
        ),
        (
            cc::POLICY_PARAMETERS,
            "e6d12291c7ee907e57565318a98e46d1844dff9e60fcbf26629b05e88da88d31",
        ),
    ] {
        let real = policy_session(&mut t, se::POLICY);
        err(
            &pol(&mut t, code, &real, &tpm2b(&digest_bytes[..31])),
            Rc::SIZE.in_param(1),
        );
        ok(&pol(&mut t, code, &real, &encoded));
        assert_eq!(digest(&mut t, &real), hex(vector));
        for conflicting in [
            cc::POLICY_TEMPLATE,
            cc::POLICY_PARAMETERS,
            cc::POLICY_CP_HASH,
            cc::POLICY_NAME_HASH,
        ] {
            err(&pol(&mut t, conflicting, &real, &encoded), Rc::CPHASH);
        }
        if code == cc::POLICY_TEMPLATE {
            err(
                &pol(&mut t, code, &real, &tpm2b(&[2; 32])),
                Rc::VALUE.in_param(1),
            );
        } else {
            err(
                &pol(&mut t, code, &real, &tpm2b(&digest_bytes[..31])),
                Rc::CPHASH,
            );
        }
        assert_eq!(digest(&mut t, &real), hex(vector));
        let trial = policy_session(&mut t, se::TRIAL);
        ok(&pol(&mut t, code, &trial, &encoded));
        assert_eq!(digest(&mut t, &trial), hex(vector));
        ok(&pol(&mut t, code, &trial, &encoded));
        assert_eq!(
            digest(&mut t, &trial),
            h256(&[&hex(vector), &u32b(code), &digest_bytes])
        );
        ok(&flush(&mut t, real.handle));
        ok(&flush(&mut t, trial.handle));
    }
}

#[test]
fn policy_template_restricts_the_creation_template_and_survives_context_save() {
    let mut t = started();
    let child = sealed();
    let hash = h256(&[&child.to_bytes()]);
    let mut template = ecc_storage();
    template.auth_policy = h256(&[&[0; 32], &u32b(cc::POLICY_TEMPLATE), &hash]);
    let parent = parse_primary(&create_primary(&mut t, OWNER, b"parent", &template));
    let mut policy = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_TEMPLATE, &policy, &tpm2b(&hash)));
    let saved = params(&save(&mut t, policy.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);

    let mut changed = child.clone();
    changed.attributes |= oa::NO_DA;
    let mut command = hc(
        cc::CREATE,
        &[parent.handle],
        creation_params(b"new", b"contents", &changed, b"outside", &NO_PCR),
        (parent.handle, b"parent"),
    );
    command.names = vec![parent.name];
    err(
        &use_session(&mut t, &mut policy, &command, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    command.params = vec![0, 0, 0, 2, 0]; // inPublic is truncated.
    err(
        &use_session(&mut t, &mut policy, &command, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    command.code = cc::UNSEAL;
    command.params.clear();
    err(
        &use_session(&mut t, &mut policy, &command, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &policy), template.auth_policy);

    command.code = cc::CREATE;
    command.params = creation_params(b"new", b"contents", &child, b"outside", &NO_PCR);
    let created = parse_child(&use_session(&mut t, &mut policy, &command, 0, true, false));
    let (h, _) = parse_loaded(&load(
        &mut t,
        parent.handle,
        b"parent",
        &created.private,
        &created.public,
    ));
    assert_eq!(
        params(&pw_cmd(&mut t, cc::UNSEAL, &[h], b"new", &[]), 0),
        tpm2b(b"contents")
    );
}

#[test]
fn policy_parameters_restricts_code_and_wire_parameters_without_binding_names() {
    let mut t = started();
    let child = sealed();
    let input = creation_params(b"", b"contents", &child, b"outside", &NO_PCR);
    let hash = h256(&[&u32b(cc::CREATE), &input]);
    let mut template = ecc_storage();
    template.auth_policy = h256(&[&[0; 32], &u32b(cc::POLICY_PARAMETERS), &hash]);
    let owner = parse_primary(&create_primary(&mut t, OWNER, b"", &template));
    let endorsement = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &template));
    assert_ne!(owner.name, endorsement.name);
    for parent in [owner, endorsement] {
        let mut policy = policy_session(&mut t, se::POLICY);
        ok(&pol(&mut t, cc::POLICY_PARAMETERS, &policy, &tpm2b(&hash)));
        let saved = params(&save(&mut t, policy.handle), 0).to_vec();
        assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
        let mut command = hc(
            cc::CREATE,
            &[parent.handle],
            input.clone(),
            (parent.handle, b""),
        );
        command.names = vec![parent.name];
        command.code = cc::HMAC;
        err(
            &use_session(&mut t, &mut policy, &command, 0, true, false),
            Rc::POLICY_FAIL.in_session(1),
        );
        command.code = cc::CREATE;
        command.params = creation_params(b"", b"contents", &child, b"altered", &NO_PCR);
        err(
            &use_session(&mut t, &mut policy, &command, 0, true, false),
            Rc::POLICY_FAIL.in_session(1),
        );
        assert_eq!(digest(&mut t, &policy), template.auth_policy);

        command.params = input.clone();
        ok(&use_session(&mut t, &mut policy, &command, 0, true, false));
        ok(&flush(&mut t, policy.handle));
    }
}

#[test]
fn policy_locality_intersects_masks_and_gates_real_authorization_after_restore() {
    let (mut t, parent) = with_ecc_parent();
    let first = h256(&[&[0; 32], &[0, 0, 1, 0x6f, 3]]);
    for (mask, expected_error) in [(1, None), (2, Some(Rc::LOCALITY))] {
        let expected = h256(&[&first, &[0, 0, 1, 0x6f, mask]]);
        let (key, name, _) = make_child(&mut t, parent, &policy_key(&expected), b"kpw", b"");
        let mut policy = policy_session(&mut t, se::POLICY);
        err(
            &pol(&mut t, cc::POLICY_LOCALITY, &policy, &[0]),
            Rc::RANGE.in_param(1),
        );
        ok(&pol(&mut t, cc::POLICY_LOCALITY, &policy, &[3]));
        ok(&pol(&mut t, cc::POLICY_LOCALITY, &policy, &[mask]));
        for rejected in [mask ^ 3, 36] {
            err(
                &pol(&mut t, cc::POLICY_LOCALITY, &policy, &[rejected]),
                Rc::RANGE.in_param(1),
            );
        }
        assert_eq!(digest(&mut t, &policy), expected);
        let saved = params(&save(&mut t, policy.handle), 0).to_vec();
        assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
        let sign = sign_command(key, &name, b"kpw");
        let response = use_session(&mut t, &mut policy, &sign, 0, true, false);
        if let Some(error) = expected_error {
            err(&response, error);
            assert_eq!(digest(&mut t, &policy), expected);
        } else {
            ok(&response);
            ok(&verify(&mut t, key, &[7; 32], params(&response, 0)));
        }
        ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
        ok(&pol(&mut t, cc::POLICY_LOCALITY, &policy, &[36]));
        ok(&pol(&mut t, cc::POLICY_LOCALITY, &policy, &[36]));
        for rejected in [37, 3] {
            err(
                &pol(&mut t, cc::POLICY_LOCALITY, &policy, &[rejected]),
                Rc::RANGE.in_param(1),
            );
        }
        ok(&flush(&mut t, policy.handle));
        ok(&flush(&mut t, key));
    }
    let trial = policy_session(&mut t, se::TRIAL);
    ok(&pol(&mut t, cc::POLICY_LOCALITY, &trial, &[3]));
    assert_eq!(digest(&mut t, &trial), first);
}

fn counter_timer(
    t: &mut Tpm,
    session: &Hs,
    operand: &[u8],
    offset: u16,
    operation: u16,
) -> Vec<u8> {
    pol(
        t,
        cc::POLICY_COUNTER_TIMER,
        session,
        &cat(&[&tpm2b(operand), &u16b(offset), &u16b(operation)]),
    )
}

#[test]
fn counter_timer_compares_canonical_time_and_trial_skips_runtime_checks() {
    let mut t = started();
    let policy = policy_session(&mut t, se::POLICY);
    ok(&counter_timer(&mut t, &policy, &[1], 24, 0));
    // Independent SHA256(SHA256(01 || 0018 || 0000)) policy input vector.
    assert_eq!(
        digest(&mut t, &policy),
        hex("310a0eb2a2c3ebd96c39d954d2865a80c7925ab8996c5d73d0bb723756ec42bf")
    );
    t.advance_power_time(0x8135);
    ok(&counter_timer(&mut t, &policy, &[0x81, 0x35, 0], 6, 0));
    for (operation, operand) in [
        (0, 0x81),
        (1, 0x80),
        (2, 0x80),
        (3, 0x80),
        (4, 0x00),
        (5, 0x82),
        (6, 0x81),
        (7, 0x81),
        (8, 0x82),
        (9, 0x82),
        (10, 0x81),
        (11, 0x7e),
    ] {
        ok(&counter_timer(&mut t, &policy, &[operand], 6, operation));
    }
    ok(&counter_timer(&mut t, &policy, &[], 25, 0));
    let before = digest(&mut t, &policy);
    err(&counter_timer(&mut t, &policy, &[0x82], 6, 2), Rc::POLICY);
    err(
        &counter_timer(&mut t, &policy, &[1, 0], 24, 0),
        Rc::RANGE.in_param(1),
    );
    err(
        &counter_timer(&mut t, &policy, &[], 26, 0),
        Rc::VALUE.in_param(2),
    );
    assert_eq!(digest(&mut t, &policy), before);

    let trial = policy_session(&mut t, se::TRIAL);
    ok(&counter_timer(&mut t, &trial, &[0; 32], 65535, 2));
    err(
        &counter_timer(&mut t, &trial, &[0; 33], 65535, 2),
        Rc::SIZE.in_param(1),
    );
    err(
        &counter_timer(&mut t, &trial, &[], 65535, 12),
        Rc::VALUE.in_param(3),
    );
}

#[test]
fn counter_timer_deadline_uses_nonce_time_and_keeps_the_earliest_limit() {
    let mut t = started();
    t.advance_power_time(2000);
    let mut at_limit = policy_session(&mut t, se::POLICY);
    let mut after_limit = policy_session(&mut t, se::POLICY);
    for policy in [&at_limit, &after_limit] {
        for seconds in [8u64, 5, 9] {
            ok(&counter_timer(&mut t, policy, &seconds.to_be_bytes(), 0, 5));
        }
    }
    let authorized = digest(&mut t, &at_limit);
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &authorized,
        alg::SHA256,
    ));
    let saved = params(&save(&mut t, after_limit.handle), 0).to_vec();
    let clock = hc(
        cc::CLOCK_SET,
        &[OWNER],
        1_000_000_000u64.to_be_bytes().to_vec(),
        (OWNER, b""),
    );
    t.advance_power_time(7000);
    ok(&use_session(&mut t, &mut at_limit, &clock, 0, true, false));
    assert_eq!(digest(&mut t, &at_limit), [0; 32]);
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), after_limit.handle);
    t.advance_power_time(7001);
    err(
        &use_session(&mut t, &mut after_limit, &clock, 0, true, false),
        Rc::EXPIRED.in_session(1),
    );
    let slot = t.sessions.slot_of(after_limit.handle).unwrap();
    assert_eq!(
        t.policy_use_check(slot, (cc::CERTIFY, NULL), |_| true, &authorized, true, 1),
        Err(Rc::POLICY_FAIL.in_session(1))
    );
    err(
        &counter_timer(&mut t, &after_limit, &5u64.to_be_bytes(), 0, 5),
        Rc::EXPIRED,
    );
    assert_eq!(digest(&mut t, &after_limit), authorized);
    ok(&pol(&mut t, cc::POLICY_RESTART, &after_limit, &[]));
    err(
        &counter_timer(&mut t, &after_limit, &5u64.to_be_bytes(), 0, 5),
        Rc::EXPIRED,
    );
    ok(&counter_timer(
        &mut t,
        &after_limit,
        &6u64.to_be_bytes(),
        0,
        5,
    ));
    // The successful authorization rolled the first session's nonce at 7000 ms.
    ok(&counter_timer(&mut t, &at_limit, &1u64.to_be_bytes(), 0, 5));
}

#[test]
fn counter_timer_context_cannot_revive_a_deadline_after_restart_or_resume() {
    for startup_type in [su::CLEAR, su::STATE] {
        let mut t = started();
        t.advance_power_time(1000);
        let mut policy = policy_session(&mut t, se::POLICY);
        ok(&counter_timer(&mut t, &policy, &60u64.to_be_bytes(), 0, 5));
        let authorized = digest(&mut t, &policy);
        ok(&set_primary_policy(
            &mut t,
            OWNER,
            b"",
            &authorized,
            alg::SHA256,
        ));
        let saved = params(&save(&mut t, policy.handle), 0).to_vec();
        shutdown(&mut t, su::STATE);
        t.init();
        startup(&mut t, startup_type);
        assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
        let clock = hc(
            cc::CLOCK_SET,
            &[OWNER],
            2_000u64.to_be_bytes().to_vec(),
            (OWNER, b""),
        );
        err(
            &use_session(&mut t, &mut policy, &clock, 0, true, false),
            Rc::EXPIRED.in_session(1),
        );
        assert_eq!(digest(&mut t, &policy), authorized);
        ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
        err(
            &counter_timer(&mut t, &policy, &60u64.to_be_bytes(), 0, 5),
            Rc::EXPIRED,
        );
        ok(&counter_timer(&mut t, &policy, &[1], 24, 0));
    }
}

fn policy_nv_args(operand: &[u8], offset: u16, operation: u16) -> Vec<u8> {
    cat(&[&tpm2b(operand), &u16b(offset), &u16b(operation)])
}

#[test]
fn policy_nv_requires_read_authorization_for_trial_and_checks_data_only_for_real_sessions() {
    let mut t = started();
    let index = NV0;
    let attrs = nv_attr::AUTHREAD | nv_attr::AUTHWRITE | nv_attr::READ_STCLEAR;
    let public = nv_public(index, alg::SHA256, attrs, b"", 3);
    ok(&define_raw(&mut t, OWNER, b"", b"nv-pw", &public));
    let policy = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    let p = policy_nv_args(b"b", 1, 0);
    let real_handles = [index, index, policy.handle];
    let trial_handles = [index, index, trial.handle];
    err(
        &pw_cmd(&mut t, cc::POLICY_NV, &real_handles, b"bad", &p),
        Rc::NV_UNINITIALIZED,
    );
    assert_eq!(t.persistent.da.failed_tries, 0);
    err(
        &pw_cmd(&mut t, cc::POLICY_NV, &trial_handles, b"bad", &p),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(t.persistent.da.failed_tries, 1);
    let (_, unwritten_name) = nv_read_public(&mut t, index);
    let outside = policy_nv_args(b"b", 400, 0);
    ok(&pw_cmd(
        &mut t,
        cc::POLICY_NV,
        &trial_handles,
        b"nv-pw",
        &outside,
    ));
    let arguments = h256(&[b"b", &u16b(400), &u16b(0)]);
    let trial_digest = h256(&[&[0; 32], &u32b(0x149), &arguments, &unwritten_name]);
    assert_eq!(digest(&mut t, &trial), trial_digest);

    ok(&nv_write(&mut t, index, index, b"nv-pw", b"abc", 0));
    let (_, name) = nv_read_public(&mut t, index);
    assert_ne!(name, unwritten_name);
    err(
        &pw_cmd(
            &mut t,
            cc::POLICY_NV,
            &[OWNER, index, policy.handle],
            b"",
            &p,
        ),
        Rc::AUTH_TYPE,
    );
    err(
        &pw_cmd(&mut t, cc::POLICY_NV, &real_handles, b"nv-pw", &outside),
        Rc::NV_RANGE,
    );
    err(
        &pw_cmd(
            &mut t,
            cc::POLICY_NV,
            &real_handles,
            b"nv-pw",
            &policy_nv_args(b"c", 1, 0),
        ),
        Rc::POLICY,
    );
    assert_eq!(digest(&mut t, &policy), [0; 32]);

    let mut authorizer = start_hmac(&mut t, NULL, b"", &[0x67; 16]);
    let mut compare = hc(cc::POLICY_NV, &real_handles, p, (index, b"nv-pw"));
    compare.names = vec![name.clone(), name.clone(), u32b(policy.handle).to_vec()];
    ok(&run_hmac(&mut t, &mut authorizer, &compare));
    let arguments = h256(&[b"b", &u16b(1), &u16b(0)]);
    let expected = h256(&[&[0; 32], &u32b(0x149), &arguments, &name]);
    assert_eq!(digest(&mut t, &policy), expected);
    ok(&pw_cmd(
        &mut t,
        cc::NV_READ_LOCK,
        &[index, index],
        b"nv-pw",
        &[],
    ));
    err(
        &pw_cmd(
            &mut t,
            cc::POLICY_NV,
            &real_handles,
            b"nv-pw",
            &compare.params,
        ),
        Rc::NV_LOCKED,
    );
    ok(&pw_cmd(
        &mut t,
        cc::POLICY_NV,
        &trial_handles,
        b"nv-pw",
        &outside,
    ));
    let (_, locked_name) = nv_read_public(&mut t, index);
    let arguments = h256(&[b"b", &u16b(400), &u16b(0)]);
    assert_eq!(
        digest(&mut t, &trial),
        h256(&[&trial_digest, &u32b(0x149), &arguments, &locked_name])
    );
    assert_eq!(digest(&mut t, &policy), expected);
}

#[test]
fn policy_nv_checks_typed_values_and_does_not_recheck_nv_at_authorization() {
    let mut t = started();
    let mut policy = policy_session(&mut t, se::POLICY);
    let attrs = nv_attr::OWNERREAD | nv_attr::OWNERWRITE;
    let counter = NV0;
    let bits = NV0 + 1;
    let extend = NV0 + 2;
    for (index, ty, size, code, parameters) in [
        (counter, 1, 8, cc::NV_INCREMENT, Vec::new()),
        (bits, 2, 8, cc::NV_SET_BITS, 0x81u64.to_be_bytes().to_vec()),
        (extend, 4, 32, cc::NV_EXTEND, tpm2b(b"hello")),
    ] {
        ok(&define(&mut t, OWNER, index, attrs | (ty << 4), size));
        ok(&pw_cmd(&mut t, code, &[OWNER, index], b"", &parameters));
    }
    let mut expected = vec![0; 32];
    let extended = h256(&[&[0; 32], b"hello"]);
    for (index, operand, offset, op) in [
        (counter, 2u64.to_be_bytes().to_vec(), 0, 5),
        (bits, vec![0x80], 7, 10),
        (extend, extended[16..].to_vec(), 16, 0),
    ] {
        let handles = [OWNER, index, policy.handle];
        ok(&pw_cmd(
            &mut t,
            cc::POLICY_NV,
            &handles,
            b"",
            &policy_nv_args(&operand, offset, op),
        ));
        let (_, name) = nv_read_public(&mut t, index);
        let arguments = h256(&[&operand, &u16b(offset), &u16b(op)]);
        expected = h256(&[&expected, &u32b(0x149), &arguments, &name]);
        assert_eq!(digest(&mut t, &policy), expected);
    }
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &expected,
        alg::SHA256,
    ));
    let saved = params(&save(&mut t, policy.handle), 0).to_vec();
    // The counter is now 2, so the original unsigned less-than comparison would fail.
    ok(&pw_cmd(
        &mut t,
        cc::NV_INCREMENT,
        &[OWNER, counter],
        b"",
        &[],
    ));
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, extend],
        b"",
        &[],
    ));
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
    let clock = hc(
        cc::CLOCK_SET,
        &[OWNER],
        23u64.to_be_bytes().to_vec(),
        (OWNER, b""),
    );
    ok(&use_session(&mut t, &mut policy, &clock, 0, true, false));
    assert_eq!(digest(&mut t, &policy), [0; 32]);
}

#[test]
fn nv_policy_access_selects_the_session_mode_and_survives_snapshot_restore() {
    let mut t = started();
    let attrs = nv_attr::POLICYREAD | nv_attr::POLICYWRITE;
    let secret = b"index-password";
    for (index, flags, policy) in [
        (NV0, attrs, &[0; 32][..]),
        (NV0 + 1, attrs, &[][..]),
        (
            NV0 + 2,
            attrs | nv_attr::AUTHREAD | nv_attr::AUTHWRITE,
            &[0; 32][..],
        ),
    ] {
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            secret,
            &nv_public(index, alg::SHA256, flags, policy, 3),
        ));
    }
    let mut real = policy_session(&mut t, se::POLICY);
    let write = |index| {
        hc(
            cc::NV_WRITE,
            &[index, index],
            cat(&[&tpm2b(b"abc"), &u16b(0)]),
            (index, b""),
        )
    };
    let read = |index| {
        hc(
            cc::NV_READ,
            &[index, index],
            cat(&[&u16b(3), &u16b(0)]),
            (index, secret),
        )
    };
    err(
        &nv_write(&mut t, NV0, NV0, secret, b"abc", 0),
        Rc::AUTH_UNAVAILABLE,
    );
    err(
        &use_session(&mut t, &mut real, &write(NV0 + 1), 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&use_session(&mut t, &mut real, &write(NV0), 0, true, false));
    err(
        &nv_read(&mut t, NV0, NV0, secret, 3, 0),
        Rc::AUTH_UNAVAILABLE,
    );
    assert_eq!(t.persistent.da.failed_tries, 0);
    assert_eq!(
        nv_read_data(&use_session(&mut t, &mut real, &read(NV0), 0, true, false)),
        b"abc"
    );

    let both = NV0 + 2;
    ok(&nv_write(&mut t, both, both, secret, b"abc", 0));
    let mut hmac = start_hmac(&mut t, NULL, b"", &[0x51; 16]);
    let mut both_read = read(both);
    let (_, name_both) = nv_read_public(&mut t, both);
    both_read.names = vec![name_both.clone(), name_both];
    assert_eq!(
        nv_read_data(&run_hmac(&mut t, &mut hmac, &both_read)),
        b"abc"
    );
    assert_eq!(
        nv_read_data(&use_session(&mut t, &mut real, &both_read, 0, true, false)),
        b"abc"
    );

    let (_, name) = nv_read_public(&mut t, NV0);
    shutdown(&mut t, su::CLEAR);
    let mut restored = Tpm::restore(&t.persistent_snapshot(), &[7; 48]).unwrap();
    startup(&mut restored, su::CLEAR);
    assert_eq!(nv_read_public(&mut restored, NV0).1, name);
    let mut real = policy_session(&mut restored, se::POLICY);
    ok(&use_session(
        &mut restored,
        &mut real,
        &write(NV0),
        0,
        true,
        false,
    ));
    assert_eq!(
        nv_read_data(&use_session(
            &mut restored,
            &mut real,
            &read(NV0),
            0,
            true,
            false
        )),
        b"abc"
    );
}

#[test]
fn policy_nv_written_gates_writes_for_the_actual_authorized_handle() {
    let mut t = started();
    let unwritten = hex("3c326323670e28ad37bd57f63b4cc34d26ab205ef22f275c58d47fab2485466e");
    let written = hex("f7887d158ae8d38be0ac5319f37a9e07618bf54885453c7a54ddb0c6a6193beb");
    let attrs = nv_attr::POLICYREAD | nv_attr::POLICYWRITE | nv_attr::OWNERWRITE;
    for (index, digest) in [(NV0, &unwritten), (NV0 + 1, &written)] {
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(index, alg::SHA256, attrs, digest, 2),
        ));
    }
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &unwritten,
        alg::SHA256,
    ));
    let mut policy = policy_session(&mut t, se::POLICY);
    let write = |index, auth| {
        hc(
            cc::NV_WRITE,
            &[auth, index],
            cat(&[&tpm2b(b"ab"), &u16b(0)]),
            (auth, b""),
        )
    };
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[0]));
    assert_eq!(digest(&mut t, &policy), unwritten);
    err(
        &use_session(&mut t, &mut policy, &write(NV0, OWNER), 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    let saved = params(&save(&mut t, policy.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), policy.handle);
    ok(&use_session(
        &mut t,
        &mut policy,
        &write(NV0, NV0),
        0,
        true,
        false,
    ));
    assert_eq!(digest(&mut t, &policy), [0; 32]);
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[0]));
    err(
        &use_session(&mut t, &mut policy, &write(NV0, NV0), 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &policy), unwritten);

    ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[1]));
    assert_eq!(digest(&mut t, &policy), written);
    err(
        &use_session(
            &mut t,
            &mut policy,
            &write(NV0 + 1, NV0 + 1),
            0,
            true,
            false,
        ),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&nv_write(&mut t, OWNER, NV0 + 1, b"", b"cd", 0));
    ok(&use_session(
        &mut t,
        &mut policy,
        &write(NV0 + 1, NV0 + 1),
        0,
        true,
        false,
    ));
}

#[test]
fn policy_nv_written_repeats_the_same_assertion_and_rejects_real_conflicts() {
    let mut t = started();
    let policy = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    for session in [&policy, &trial] {
        err(
            &pol(&mut t, cc::POLICY_NV_WRITTEN, session, &[]),
            Rc::INSUFFICIENT.in_param(1),
        );
        err(
            &pol(&mut t, cc::POLICY_NV_WRITTEN, session, &[2]),
            Rc::VALUE.in_param(1),
        );
        err(
            &pol(&mut t, cc::POLICY_NV_WRITTEN, session, &[1, 0]),
            Rc::SIZE,
        );
        ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, session, &[0]));
    }
    let first = hex("3c326323670e28ad37bd57f63b4cc34d26ab205ef22f275c58d47fab2485466e");
    err(
        &pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[1]),
        Rc::VALUE.in_param(1),
    );
    assert_eq!(digest(&mut t, &policy), first);
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[0]));
    assert_eq!(digest(&mut t, &policy), h256(&[&first, &u32b(0x18f), &[0]]));
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &trial, &[1]));
    assert_eq!(digest(&mut t, &trial), h256(&[&first, &u32b(0x18f), &[1]]));
}

#[test]
fn policy_authorize_nv_replaces_the_digest_but_preserves_the_nv_written_constraint() {
    let mut t = started();
    let approval = NV0;
    let subject = NV0 + 1;
    let approval_attrs = nv_attr::POLICYREAD | nv_attr::OWNERWRITE;
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"",
        &nv_public(approval, alg::SHA256, approval_attrs, &[0; 32], 37),
    ));
    let mut authorizer = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    let mut policy = policy_session(&mut t, se::POLICY);
    let authorize = |session: &Hs| {
        hc(
            cc::POLICY_AUTHORIZE_NV,
            &[approval, approval, session.handle],
            Vec::new(),
            (approval, b""),
        )
    };
    let (_, unwritten_name) = nv_read_public(&mut t, approval);
    err(
        &use_session(&mut t, &mut authorizer, &authorize(&policy), 0, true, false),
        Rc::NV_UNINITIALIZED,
    );
    ok(&use_session(
        &mut t,
        &mut authorizer,
        &authorize(&trial),
        0,
        true,
        false,
    ));
    assert_eq!(
        digest(&mut t, &trial),
        h256(&[&[0; 32], &u32b(0x192), &unwritten_name])
    );

    ok(&nv_write(
        &mut t,
        OWNER,
        approval,
        b"",
        &cat(&[&u16b(alg::SHA256), &[0; 32], b"tag"]),
        0,
    ));
    let (_, name) = nv_read_public(&mut t, approval);
    ok(&use_session(
        &mut t,
        &mut authorizer,
        &authorize(&trial),
        0,
        true,
        false,
    ));
    let stable = h256(&[&[0; 32], &u32b(0x192), &name]);
    assert_eq!(digest(&mut t, &trial), stable);
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"",
        &nv_public(
            subject,
            alg::SHA256,
            nv_attr::OWNERREAD | nv_attr::POLICYWRITE,
            &stable,
            2,
        ),
    ));

    let write = hc(
        cc::NV_WRITE,
        &[subject, subject],
        cat(&[&tpm2b(b"ab"), &u16b(0)]),
        (subject, b""),
    );
    let approved = h256(&[&[0; 32], &u32b(0x18f), &[0]]);
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[0]));
    err(
        &use_session(&mut t, &mut authorizer, &authorize(&policy), 0, true, false),
        Rc::VALUE.in_handle(2),
    );
    assert_eq!(digest(&mut t, &policy), approved);
    let record = cat(&[&u16b(alg::SHA256), &approved, b"tag"]);
    ok(&nv_write(&mut t, OWNER, approval, b"", &record, 0));
    ok(&use_session(
        &mut t,
        &mut authorizer,
        &authorize(&policy),
        0,
        true,
        false,
    ));
    assert_eq!(digest(&mut t, &policy), stable);
    let context = params(&save(&mut t, policy.handle), 0).to_vec();

    ok(&nv_write(&mut t, OWNER, approval, b"", &[7; 32], 2));
    assert_eq!(loaded(&load_ctx(&mut t, &context)), policy.handle);
    ok(&use_session(&mut t, &mut policy, &write, 0, true, false));
    ok(&pol(&mut t, cc::POLICY_NV_WRITTEN, &policy, &[0]));
    err(
        &use_session(&mut t, &mut authorizer, &authorize(&policy), 0, true, false),
        Rc::VALUE.in_handle(2),
    );
    ok(&nv_write(&mut t, OWNER, approval, b"", &record, 0));
    ok(&use_session(
        &mut t,
        &mut authorizer,
        &authorize(&policy),
        0,
        true,
        false,
    ));
    err(
        &use_session(&mut t, &mut policy, &write, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &policy), stable);
}

#[test]
fn policy_authorize_nv_validates_a_raw_tagged_digest_only_for_real_sessions() {
    let mut t = started();
    let policy = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    let attrs = nv_attr::AUTHREAD | nv_attr::OWNERWRITE | nv_attr::READ_STCLEAR;
    let mut written = cat(&[&u16b(alg::SHA256), &[0; 32]]);
    for (number, size, error) in [
        (0, 1, Rc::INSUFFICIENT.in_handle(2)),
        (1, 33, Rc::INSUFFICIENT.in_handle(2)),
        (2, 34, Rc::HASH.in_handle(2)),
    ] {
        let index = NV0 + number;
        let auth = b"read-secret";
        let handles = [index, index, policy.handle];
        let trial_handles = [index, index, trial.handle];
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            auth,
            &nv_public(index, alg::SHA256, attrs, &[], size),
        ));
        err(
            &pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles, b"wrong", &[]),
            Rc::NV_UNINITIALIZED,
        );
        if number == 2 {
            written[1] = alg::NULL as u8;
        }
        ok(&nv_write(
            &mut t,
            OWNER,
            index,
            b"",
            &written[..usize::from(size)],
            0,
        ));
        err(
            &pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles, auth, &[]),
            error,
        );
        err(
            &pw_cmd(
                &mut t,
                cc::POLICY_AUTHORIZE_NV,
                &trial_handles,
                b"wrong",
                &[],
            ),
            Rc::AUTH_FAIL.in_session(1),
        );
        ok(&pw_cmd(
            &mut t,
            cc::NV_READ_LOCK,
            &[index, index],
            auth,
            &[],
        ));
        err(
            &pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles, auth, &[]),
            Rc::NV_LOCKED,
        );
        ok(&pw_cmd(
            &mut t,
            cc::POLICY_AUTHORIZE_NV,
            &trial_handles,
            auth,
            &[],
        ));
        let name = nv_read_public(&mut t, index).1;
        assert_eq!(
            digest(&mut t, &trial),
            h256(&[&[0; 32], &u32b(0x192), &name])
        );
        assert_eq!(digest(&mut t, &policy), [0; 32]);
    }
}

#[test]
fn nv_change_auth_checks_admin_policy_and_updates_the_proof_without_changing_the_name() {
    let mut t = started();
    let old = b"old secret";
    let new = b"new secret";
    let attrs = nv_attr::AUTHREAD | nv_attr::AUTHWRITE;
    let command = h256(&[&[0; 32], &u32b(0x16c), &u32b(0x13b)]);
    let approval = h256(&[&command, &u32b(0x16b)]);
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        old,
        &nv_public(NV0, alg::SHA256, attrs, &approval, 3),
    ));
    ok(&nv_write(&mut t, NV0, NV0, old, b"abc", 0));
    let (public, name) = nv_read_public(&mut t, NV0);
    let parameters = tpm2b(&cat(&[new, &[0; 2]]));
    let change = |parameters: Vec<u8>| {
        let mut c = hc(cc::NV_CHANGE_AUTH, &[NV0], parameters, (NV0, old));
        c.names = vec![name.clone()];
        c.entity_after = new.to_vec();
        c
    };
    err(
        &pw_cmd(&mut t, cc::NV_CHANGE_AUTH, &[NV0], old, &parameters),
        Rc::AUTH_TYPE,
    );
    let hmac = start_hmac(&mut t, NULL, b"", &[0x38; 16]);
    err(
        &run(
            &mut t,
            hmac_command(&hmac, &change(parameters.clone()), false, false),
        ),
        Rc::AUTH_TYPE,
    );
    let mut policy = policy_session(&mut t, se::POLICY);
    err(
        &use_session(
            &mut t,
            &mut policy,
            &change(parameters.clone()),
            0,
            true,
            false,
        ),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&pol_cc(&mut t, &policy, cc::NV_WRITE));
    err(
        &use_session(
            &mut t,
            &mut policy,
            &change(parameters.clone()),
            0,
            true,
            false,
        ),
        Rc::POLICY_CC.in_session(1),
    );
    ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
    ok(&pol_cc(&mut t, &policy, cc::NV_CHANGE_AUTH));
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &policy, &[]));
    assert_eq!(digest(&mut t, &policy), approval);
    err(
        &run_hmac(&mut t, &mut policy, &change(tpm2b(&[0; 33]))),
        Rc::SIZE.in_param(1),
    );
    err(
        &run_hmac(&mut t, &mut policy, &change(cat(&[&parameters, &[0]]))),
        Rc::SIZE,
    );
    assert_eq!(digest(&mut t, &policy), approval);
    assert_eq!(nv_read_data(&nv_read(&mut t, NV0, NV0, old, 3, 0)), b"abc");
    ok(&run_hmac(&mut t, &mut policy, &change(parameters)));
    assert_eq!(digest(&mut t, &policy), [0; 32]);
    assert_eq!(nv_read_public(&mut t, NV0), (public, name));
    err(
        &nv_read(&mut t, NV0, NV0, old, 3, 0),
        Rc::AUTH_FAIL.in_session(1),
    );

    shutdown(&mut t, su::CLEAR);
    let mut restored = Tpm::restore(&t.persistent_snapshot(), &[4; 48]).unwrap();
    startup(&mut restored, su::CLEAR);
    assert_eq!(
        nv_read_data(&nv_read(&mut restored, NV0, NV0, new, 3, 0)),
        b"abc"
    );
}

#[test]
fn nv_change_auth_can_clear_the_secret_on_an_unwritten_locked_index() {
    let mut t = started();
    let approval = h256(&[&[0; 32], &u32b(0x16c), &u32b(0x13b)]);
    let auth = b"counter secret";
    let attrs = nv_attr::AUTHREAD
        | nv_attr::AUTHWRITE
        | nv_attr::READ_STCLEAR
        | nv_attr::WRITE_STCLEAR
        | (1 << 4);
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        auth,
        &nv_public(NV0, alg::SHA256, attrs, &approval, 8),
    ));
    ok(&pw_cmd(&mut t, cc::NV_READ_LOCK, &[NV0, NV0], auth, &[]));
    ok(&pw_cmd(&mut t, cc::NV_WRITE_LOCK, &[NV0, NV0], auth, &[]));
    let public = nv_read_public(&mut t, NV0);
    let mut policy = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &policy, cc::NV_CHANGE_AUTH));
    let change = hc(cc::NV_CHANGE_AUTH, &[NV0], tpm2b(&[0; 32]), (NV0, b""));
    ok(&use_session(&mut t, &mut policy, &change, 0, true, false));
    assert_eq!(nv_read_public(&mut t, NV0), public);
    assert_eq!(t.nv_auth(NV0).unwrap().as_slice(), b"");
}

#[test]
fn nv_undefine_space_special_requires_both_roles_and_preserves_counter_state() {
    let mut t = started();
    let digest = h256(&[&[0; 32], &u32b(0x16c), &u32b(0x11f)]);
    let flags = nv_attr::PPREAD
        | nv_attr::PPWRITE
        | nv_attr::PLATFORMCREATE
        | nv_attr::READ_STCLEAR
        | nv_attr::WRITE_STCLEAR
        | (1 << 4);
    let attrs = flags | nv_attr::POLICY_DELETE;
    let public = nv_public(NV0, alg::SHA256, attrs, &digest, 8);
    err(
        &define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &nv_public(
                NV0,
                alg::SHA256,
                nv_attr::OWNERREAD | nv_attr::OWNERWRITE | nv_attr::POLICY_DELETE,
                &digest,
                8,
            ),
        ),
        Rc::ATTRIBUTES.in_param(2),
    );
    ok(&define_raw(&mut t, PLATFORM, b"", b"", &public));
    ok(&define_raw(
        &mut t,
        PLATFORM,
        b"",
        b"",
        &nv_public(NV0 + 1, alg::SHA256, flags, &digest, 8),
    ));
    ok(&pw_cmd(
        &mut t,
        cc::NV_INCREMENT,
        &[PLATFORM, NV0],
        b"",
        &[],
    ));
    assert_eq!(t.persistent.nv.counter_max(), 1);
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, NV0], b"", &[]),
        Rc::NV_AUTHORIZATION,
    );
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[PLATFORM, NV0], b"", &[]),
        Rc::ATTRIBUTES,
    );
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]));
    shutdown(&mut t, su::CLEAR);
    let mut t = Tpm::restore(&t.persistent_snapshot(), &[6; 48]).unwrap();
    startup(&mut t, su::CLEAR);
    ok(&pw_cmd(
        &mut t,
        cc::NV_READ_LOCK,
        &[PLATFORM, NV0],
        b"",
        &[],
    ));
    ok(&pw_cmd(
        &mut t,
        cc::NV_WRITE_LOCK,
        &[PLATFORM, NV0],
        b"",
        &[],
    ));
    let policy = policy_session(&mut t, se::POLICY);
    let remove = |index, platform, password: &[u8]| {
        let platform_auth = auth_area(&[password]);
        let first = cat(&[&u32b(policy.handle), &tpm2b(&[0x71; 16]), &[1], &tpm2b(&[])]);
        let area = cat(&[&first, &platform_auth[4..]]);
        frame(
            st::SESSIONS,
            cc::NV_UNDEFINE_SPACE_SPECIAL,
            &[index, platform],
            &cat(&[&u32b(area.len() as u32), &area]),
            &[],
        )
    };
    err(
        &run(
            &mut t,
            with_pw(
                cc::NV_UNDEFINE_SPACE_SPECIAL,
                &[NV0, PLATFORM],
                &[b"", b""],
                &[],
            ),
        ),
        Rc::AUTH_TYPE,
    );
    err(
        &run(&mut t, remove(NV0, OWNER, b"")),
        Rc::VALUE.in_handle(2),
    );
    err(
        &run(&mut t, remove(NV0, PLATFORM, b"")),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&pol_cc(&mut t, &policy, cc::NV_UNDEFINE_SPACE_SPECIAL));
    err(
        &run(&mut t, remove(NV0, PLATFORM, b"wrong")),
        Rc::BAD_AUTH.in_session(2),
    );
    err(&run(&mut t, remove(NV0 + 1, PLATFORM, b"")), Rc::ATTRIBUTES);
    assert_eq!(self::digest(&mut t, &policy), digest);
    ok(&run(&mut t, remove(NV0, PLATFORM, b"")));
    assert_eq!(self::digest(&mut t, &policy), [0; 32]);
    err(
        &run(&mut t, plain(cc::NV_READ_PUBLIC, &[NV0], &[])),
        Rc::HANDLE.in_handle(1),
    );
    ok(&pw_cmd(
        &mut t,
        cc::NV_INCREMENT,
        &[PLATFORM, NV0 + 1],
        b"",
        &[],
    ));
    assert_eq!(
        nv_read_data(&nv_read(&mut t, PLATFORM, NV0 + 1, b"", 8, 0)),
        2u64.to_be_bytes()
    );
}

#[test]
fn policy_restart_resets_everything_but_the_nonce() {
    let mut t = started();
    let mut s = policy_session(&mut t, se::POLICY);
    let nonce = s.nonce_tpm.clone();
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &s, &[]));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    ok(&pol_pcr(&mut t, &s, &[], &sel04()));
    ok(&pol(&mut t, cc::POLICY_RESTART, &s, &[]));
    assert_eq!(digest(&mut t, &s), vec![0u8; 32]);
    // The session is reusable with another command code and without the authValue need.
    ok(&pol_cc(&mut t, &s, cc::UNSEAL));
    assert_eq!(
        digest(&mut t, &s),
        h256(&[
            &[0u8; 32],
            &u32b(cc::POLICY_COMMAND_CODE),
            &u32b(cc::UNSEAL)
        ])
    );
    // No parameters allowed; the nonce (kept by the TPM) is what the client still knows.
    err(&pol(&mut t, cc::POLICY_RESTART, &s, &[0]), Rc::SIZE);
    assert_eq!(s.nonce_tpm, nonce);
    s.nonce_tpm.clear();
}

#[test]
fn policy_secret_keeps_the_earliest_absolute_or_nonce_relative_expiration() {
    let mut t = started();
    t.advance_power_time(2000);
    let mut at_limit = policy_session(&mut t, se::POLICY);
    let mut after_limit = policy_session(&mut t, se::POLICY);
    let empty_ticket = hex("00008023400000070000");
    for session in [&at_limit, &after_limit] {
        for (nonce, expiration) in [
            (&session.nonce_tpm[..], 5),
            (&[][..], 6),
            (&session.nonce_tpm[..], 8),
        ] {
            let response = secret_pw(
                &mut t,
                OWNER,
                b"",
                session,
                &secret_params(nonce, &[], b"ref", expiration),
            );
            ok(&response);
            assert_eq!(params(&response, 0), empty_ticket);
        }
    }
    let authorized = digest(&mut t, &at_limit);
    assert_eq!(digest(&mut t, &after_limit), authorized);
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &authorized,
        alg::SHA256,
    ));
    let saved = params(&save(&mut t, after_limit.handle), 0).to_vec();
    t.advance_power_time(6000);
    let clock = hc(
        cc::CLOCK_SET,
        &[OWNER],
        99_999u64.to_be_bytes().to_vec(),
        (OWNER, b""),
    );
    ok(&use_session(&mut t, &mut at_limit, &clock, 0, true, false));
    t.advance_power_time(6001);
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), after_limit.handle);
    err(
        &use_session(&mut t, &mut after_limit, &clock, 0, true, false),
        Rc::EXPIRED.in_session(1),
    );
    assert_eq!(digest(&mut t, &after_limit), authorized);
}

#[test]
fn policy_secret_validation_order_and_trial_do_not_install_restrictions_on_failure() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"pw"));
    t.advance_power_time(1500);
    let real = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_NAME_HASH, &real, &tpm2b(&[5; 32])));
    let before = digest(&mut t, &real);
    let expired = secret_params(&[], b"short", b"ref", 1);
    err(
        &secret_pw(&mut t, OWNER, b"pw", &real, &expired),
        Rc::EXPIRED,
    );
    let conflict = secret_params(&[], b"short", b"ref", 10);
    err(
        &secret_pw(&mut t, OWNER, b"pw", &real, &conflict),
        Rc::CPHASH,
    );
    assert_eq!(digest(&mut t, &real), before);

    let trial = policy_session(&mut t, se::TRIAL);
    let unusual = secret_params(b"bad nonce", b"short", b"ref", u32::MAX);
    err(
        &secret_pw(&mut t, OWNER, b"wrong", &trial, &unusual),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&secret_pw(&mut t, OWNER, b"pw", &trial, &unusual));
    assert_eq!(digest(&mut t, &trial), hex(V_SECRET_OWNER));
}

#[test]
fn policy_ticket_replays_a_secret_assertion_and_keeps_its_command_binding() {
    let mut t = started();
    t.advance_power_time(2000);
    let source = policy_session(&mut t, se::POLICY);
    let replay = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    let clock = hc(
        cc::CLOCK_SET,
        &[OWNER],
        100_000u64.to_be_bytes().to_vec(),
        (OWNER, b""),
    );
    let cp_hash = h256(&[&u32b(0x128), &u32b(OWNER), &clock.params]);
    let issued = secret_pw(
        &mut t,
        OWNER,
        b"",
        &source,
        &secret_params(&source.nonce_tpm, &cp_hash, b"", (-2i32) as u32),
    );
    ok(&issued);
    let output = params(&issued, 0);
    let timeout = &output[..10];
    let ticket = &output[10..];
    assert_eq!(timeout, tpm2b(&4000u64.to_be_bytes()));
    assert_eq!(&ticket[..8], &hex("8023400000010020"));
    let request = |timeout: &[u8], cp: &[u8], reference: &[u8], ticket: &[u8]| {
        cat(&[
            timeout,
            &tpm2b(cp),
            &tpm2b(reference),
            &tpm2b(&u32b(OWNER)),
            ticket,
        ])
    };
    let valid = request(timeout, &cp_hash, b"", ticket);
    let repartition = request(timeout, b"", &cp_hash, ticket);
    // Trial uses the structurally valid fields without verifying their HMAC.
    ok(&pol(&mut t, cc::POLICY_TICKET, &trial, &repartition));
    let trial_first = h256(&[&[0; 32], &u32b(0x151), &u32b(OWNER)]);
    assert_eq!(digest(&mut t, &trial), h256(&[&trial_first, &cp_hash]));
    err(
        &pol(&mut t, cc::POLICY_TICKET, &replay, &repartition),
        Rc::TICKET.in_param(5),
    );
    let new_timeout = tpm2b(&(4000u64 | (1 << 63)).to_be_bytes());
    let altered = request(&new_timeout, &cp_hash, b"", ticket);
    err(
        &pol(&mut t, cc::POLICY_TICKET, &replay, &altered),
        Rc::TICKET.in_param(5),
    );
    assert_eq!(digest(&mut t, &replay), [0; 32]);
    ok(&pol(&mut t, cc::POLICY_TICKET, &replay, &valid));
    let approved = digest(&mut t, &source);
    assert_eq!(digest(&mut t, &replay), approved);
    ok(&set_primary_policy(
        &mut t,
        OWNER,
        b"",
        &approved,
        alg::SHA256,
    ));

    let saved = params(&save(&mut t, replay.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), replay.handle);
    let mut replay = replay;
    let different_clock = hc(
        cc::CLOCK_SET,
        &[OWNER],
        100_001u64.to_be_bytes().to_vec(),
        (OWNER, b""),
    );
    err(
        &use_session(&mut t, &mut replay, &different_clock, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    t.advance_power_time(4000);
    ok(&use_session(&mut t, &mut replay, &clock, 0, true, false));
    t.advance_power_time(4001);
    err(
        &pol(&mut t, cc::POLICY_TICKET, &replay, &valid),
        Rc::EXPIRED,
    );
    assert_eq!(digest(&mut t, &replay), [0; 32]);
}

#[test]
fn policy_ticket_depends_on_the_issuing_epoch_and_persistent_hierarchy_proof() {
    let mut t = started();
    let source = policy_session(&mut t, se::POLICY);
    let pending = policy_session(&mut t, se::POLICY);
    let mut requests = Vec::new();
    for hierarchy in [LOCKOUT, PLATFORM] {
        let issued = secret_pw(
            &mut t,
            hierarchy,
            b"",
            &source,
            &secret_params(&[], b"", b"", i32::MIN as u32),
        );
        ok(&issued);
        let output = params(&issued, 0);
        let abs_ms = (u64::from(i32::MIN.unsigned_abs()) * 1000) | (1 << 63);
        assert_eq!(&output[..10], &tpm2b(&abs_ms.to_be_bytes()));
        let ticket = &output[10..];
        let proof_hierarchy = if hierarchy == LOCKOUT {
            OWNER
        } else {
            PLATFORM
        };
        assert_eq!(&ticket[..6], &cat(&[&u16b(0x8023), &u32b(proof_hierarchy)]));
        requests.push(cat(&[
            &output[..10],
            &[0; 4],
            &tpm2b(&u32b(hierarchy)),
            ticket,
        ]));
    }
    assert_eq!(t.persistent.total_reset_count, 1);
    ok(&pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]));
    assert_eq!(t.persistent.total_reset_count, 1);
    assert_eq!(t.persistent.reset_count, 0);
    err(
        &pol(&mut t, cc::POLICY_TICKET, &pending, &requests[0]),
        Rc::TICKET.in_param(5),
    );
    ok(&pol(&mut t, cc::POLICY_TICKET, &pending, &requests[1]));
    let saved = params(&save(&mut t, pending.handle), 0).to_vec();
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::STATE);
    assert_eq!(t.persistent.total_reset_count, 1);
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), pending.handle);
    err(
        &pol(&mut t, cc::POLICY_TICKET, &pending, &requests[1]),
        Rc::EXPIRED,
    );
    let after = policy_session(&mut t, se::POLICY);
    err(
        &pol(&mut t, cc::POLICY_TICKET, &after, &requests[1]),
        Rc::TICKET.in_param(5),
    );
    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    assert_eq!(t.persistent.total_reset_count, 2);
}

#[test]
fn policy_signed_accepts_public_rsa_and_ecc_keys_and_replays_tickets_after_flush() {
    let mut t = started();
    t.advance_power_time(2000);
    for (template, scheme) in [
        (ecc_sign(), ecc_sig_scheme()),
        (rsa_sign(), rsa_sig_scheme()),
    ] {
        let signing_key =
            parse_primary(&create_primary(&mut t, OWNER, b"signing secret", &template));
        let public = unmarshal_public(&signing_key.public);
        let (auth_object, name) = parse_loaded(&load_external(&mut t, &public, NULL));
        assert_eq!(name, signing_key.name);
        let issuer = policy_session(&mut t, se::POLICY);
        let replay = policy_session(&mut t, se::POLICY);
        let reference = b"policy ref";
        let expiration = (-4i32).to_be_bytes();
        let command_hash = [0x83; 32];
        let signed = h256(&[&issuer.nonce_tpm, &expiration, &command_hash, reference]);
        let signing = sign(
            &mut t,
            signing_key.handle,
            b"signing secret",
            &signed,
            &scheme,
        );
        ok(&signing);
        let signature = params(&signing, 0);
        let request = |policy_ref: &[u8]| {
            cat(&[
                &secret_params(
                    &issuer.nonce_tpm,
                    &command_hash,
                    policy_ref,
                    u32::from_be_bytes(expiration),
                ),
                signature,
            ])
        };
        let handles = [auth_object, issuer.handle];
        err(
            &run(
                &mut t,
                plain(cc::POLICY_SIGNED, &handles, &request(b"altered ref")),
            ),
            Rc::POLICY_FAIL,
        );
        assert_eq!(digest(&mut t, &issuer), [0; 32]);
        assert_eq!(t.persistent.da.failed_tries, 0);
        let response = run(
            &mut t,
            plain(cc::POLICY_SIGNED, &handles, &request(reference)),
        );
        ok(&response);
        let ticket = params(&response, 0);
        assert_eq!(&ticket[..10], &tpm2b(&6000u64.to_be_bytes()));
        assert_eq!(&ticket[10..18], &hex("8025400000070020"));
        let first = h256(&[&[0; 32], &u32b(0x160), &name]);
        let expected = h256(&[&first, reference]);
        assert_eq!(digest(&mut t, &issuer), expected);

        ok(&flush(&mut t, auth_object));
        ok(&flush(&mut t, signing_key.handle));
        err(
            &run(
                &mut t,
                plain(cc::POLICY_SIGNED, &handles, &request(reference)),
            ),
            Rc::reference_handle(0),
        );
        let replay_parameters = cat(&[
            &ticket[..10],
            &tpm2b(&command_hash),
            &tpm2b(reference),
            &tpm2b(&name),
            &ticket[10..],
        ]);
        ok(&pol(&mut t, cc::POLICY_TICKET, &replay, &replay_parameters));
        assert_eq!(digest(&mut t, &replay), expected);
    }
}

#[test]
fn policy_signed_checks_real_parameters_and_signature_usage_but_allows_a_null_trial_signature() {
    let mut t = started();
    let signer = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    let real = policy_session(&mut t, se::POLICY);
    let trial = policy_session(&mut t, se::TRIAL);
    let null = u16b(alg::NULL);
    let null_params = cat(&[&secret_params(b"", b"", b"ref", 0), &null]);
    err(
        &run(
            &mut t,
            plain(
                cc::POLICY_SIGNED,
                &[signer.handle, real.handle],
                &null_params,
            ),
        ),
        Rc::POLICY_FAIL,
    );
    let wrong_nonce = cat(&[
        &secret_params(b"wrong nonce", b"short", b"ref", u32::MAX),
        &null,
    ]);
    err(
        &run(
            &mut t,
            plain(
                cc::POLICY_SIGNED,
                &[signer.handle, real.handle],
                &wrong_nonce,
            ),
        ),
        Rc::VALUE.in_param(1),
    );
    let answer = run(
        &mut t,
        plain(
            cc::POLICY_SIGNED,
            &[signer.handle, trial.handle],
            &wrong_nonce,
        ),
    );
    ok(&answer);
    assert_eq!(params(&answer, 0), hex("00008025400000070000"));
    let first = h256(&[&[0; 32], &u32b(0x160), &signer.name]);
    assert_eq!(digest(&mut t, &trial), h256(&[&first, b"ref"]));
    assert_eq!(digest(&mut t, &real), [0; 32]);

    let signed = h256(&[&[0; 4], b"ref"]);
    let signed_response = sign(&mut t, signer.handle, b"", &signed, &ecc_sig_scheme());
    let signature = params(&signed_response, 0);
    let params = cat(&[&secret_params(b"", b"", b"ref", 0), signature]);
    let mut nonsigning = unmarshal_public(&signer.public);
    nonsigning.attributes &= !oa::SIGN;
    let (object, _) = parse_loaded(&load_external(&mut t, &nonsigning, NULL));
    err(
        &run(
            &mut t,
            plain(cc::POLICY_SIGNED, &[object, real.handle], &params),
        ),
        Rc::POLICY_FAIL,
    );
    ok(&run(
        &mut t,
        plain(cc::POLICY_SIGNED, &[signer.handle, real.handle], &params),
    ));
    assert_eq!(digest(&mut t, &real), digest(&mut t, &trial));
}

#[test]
fn policy_secret_checks_authorization_and_parameters() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-pw"));
    let s = policy_session(&mut t, se::POLICY);
    let good = secret_params(&[], &[], b"ref", 0);
    // The authorization of the entity: a wrong password (the owner is not DA-protected).
    err(
        &secret_pw(&mut t, OWNER, b"bad", &s, &good),
        Rc::BAD_AUTH.in_session(1),
    );
    assert_eq!(digest(&mut t, &s), vec![0u8; 32]);
    // Without any session.
    err(
        &run(&mut t, plain(cc::POLICY_SECRET, &[OWNER, s.handle], &good)),
        Rc::AUTH_MISSING,
    );
    // nonceTPM: the session's own nonce is accepted, any other is TPM_RC_VALUE (parameter 1).
    err(
        &secret_pw(
            &mut t,
            OWNER,
            b"owner-pw",
            &s,
            &secret_params(&[9u8; 16], &[], b"ref", 0),
        ),
        Rc::VALUE.in_param(1),
    );
    let own = s.nonce_tpm.clone();
    ok(&secret_pw(
        &mut t,
        OWNER,
        b"owner-pw",
        &s,
        &secret_params(&own, &[], b"ref", 0),
    ));
    // cpHashA must be one digest (TPM_RC_SIZE, parameter 2); policyRef above one digest.
    err(
        &secret_pw(
            &mut t,
            OWNER,
            b"owner-pw",
            &s,
            &secret_params(&[], &[1u8; 7], b"ref", 0),
        ),
        Rc::SIZE.in_param(2),
    );
    err(
        &secret_pw(
            &mut t,
            OWNER,
            b"owner-pw",
            &s,
            &secret_params(&[], &[], &[1u8; 33], 0),
        ),
        Rc::SIZE.in_param(3),
    );
    // A cpHashA sticks to the session: another one is TPM_RC_CPHASH.
    ok(&secret_pw(
        &mut t,
        OWNER,
        b"owner-pw",
        &s,
        &secret_params(&[], &[5u8; 32], b"", 0),
    ));
    err(
        &secret_pw(
            &mut t,
            OWNER,
            b"owner-pw",
            &s,
            &secret_params(&[], &[6u8; 32], b"", 0),
        ),
        Rc::CPHASH,
    );
    // Handles: an HMAC session as policySession, an unloaded one.
    let h = start_hmac(&mut t, NULL, b"", &[0x55; 16]);
    err(
        &run(
            &mut t,
            with_pw(cc::POLICY_SECRET, &[OWNER, h.handle], &[b"owner-pw"], &good),
        ),
        Rc::VALUE.in_handle(2),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::POLICY_SECRET,
                &[OWNER, 0x0300_0030],
                &[b"owner-pw"],
                &good,
            ),
        ),
        Rc::reference_handle(1),
    );
    // A disabled hierarchy cannot authorize.
    ok(&pw_hier_off(&mut t));
    err(
        &secret_pw(&mut t, OWNER, b"owner-pw", &s, &good),
        Rc::HIERARCHY.in_handle(1),
    );
    // A trial session still checks the authorization.
    let tr = policy_session(&mut t, se::TRIAL);
    err(
        &secret_pw(&mut t, ENDORSEMENT, b"wrong", &tr, &good),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&secret_pw(&mut t, ENDORSEMENT, b"", &tr, &good));
}

fn pw_hier_off(t: &mut Tpm) -> Vec<u8> {
    // Platform CLEARs `shEnable` (the owner hierarchy).
    run(
        t,
        with_pw(
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            &[b""],
            &cat(&[&u32b(OWNER), &[0]]),
        ),
    )
}

#[test]
fn policy_secret_with_an_hmac_session_and_with_an_hmac_bound_session() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-pw"));
    let ps = policy_session(&mut t, se::POLICY);
    let mut hs = start_hmac(&mut t, NULL, b"", &[0x55; 16]);
    let c = hc(
        cc::POLICY_SECRET,
        &[OWNER, ps.handle],
        secret_params(&[], &[], b"ref", 0),
        (OWNER, b"owner-pw"),
    );
    ok(&super::tests_b::run_hmac(&mut t, &mut hs, &c));
    let own = digest(&mut t, &ps);
    assert_eq!(
        own,
        h256(&[
            &h256(&[&[0u8; 32], &u32b(cc::POLICY_SECRET), &u32b(OWNER)]),
            b"ref"
        ])
    );
    // A corrupted HMAC and an empty HMAC are authorization failures.
    err(
        &use_session(&mut t, &mut hs, &c, 0, false, true),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &use_session(&mut t, &mut hs, &c, 0, true, false),
        Rc::BAD_AUTH.in_session(1),
    );
    assert_eq!(digest(&mut t, &ps), own, "failures change nothing");
}

// ------------------------------------------------------------ authorization by a policy session

#[test]
fn an_object_with_an_auth_policy_is_authorized_by_the_policy_session() {
    let (mut t, parent) = with_ecc_parent();
    let (key, name, public) = make_child(&mut t, parent, &policy_key(&hex(V_CC_SIGN)), b"kpw", b"");
    assert_eq!(unmarshal_public(&public).auth_policy, hex(V_CC_SIGN));
    // A password or HMAC cannot use an object whose userWithAuth is CLEAR.
    err(
        &sign(&mut t, key, b"kpw", &[7u8; 32], &ecc_sig_scheme()),
        Rc::POLICY_FAIL.in_session(1),
    );
    // The policy session of the right policy authorizes (nothing to prove: unbound session, no
    // authValue in the policy, so an empty HMAC).
    let mut s = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    assert_eq!(digest(&mut t, &s), hex(V_CC_SIGN));
    let c = sign_command(key, &name, b"kpw");
    let mut bad_params = sign_command(key, &name, b"kpw");
    bad_params.params.push(0);
    err(
        &use_session(&mut t, &mut s, &bad_params, 0, true, false),
        Rc::SIZE,
    );
    assert_eq!(digest(&mut t, &s), hex(V_CC_SIGN));
    let r1 = use_session(&mut t, &mut s, &c, 0, true, false);
    ok(&r1);
    let sig = params(&r1, 0).to_vec();
    ok(&verify(&mut t, key, &[7u8; 32], &sig));
    assert_eq!(digest(&mut t, &s), [0; 32]);
    err(
        &use_session(&mut t, &mut s, &c, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    ok(&use_session(&mut t, &mut s, &c, 0, true, false));
    assert_eq!(digest(&mut t, &s), [0; 32]);
    // A session of another policy: TPM_RC_POLICY_FAIL; the same command code but a zero digest.
    let mut z = policy_session(&mut t, se::POLICY);
    err(
        &use_session(&mut t, &mut z, &c, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    // A session whose command code is another one: TPM_RC_POLICY_CC (checked before the digest).
    let mut u = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &u, cc::UNSEAL));
    err(
        &use_session(&mut t, &mut u, &c, 0, true, false),
        Rc::POLICY_CC.in_session(1),
    );
    // A trial session never authorizes.
    let mut tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_cc(&mut t, &tr, cc::SIGN));
    err(
        &use_session(&mut t, &mut tr, &c, 0, true, false),
        Rc::ATTRIBUTES.in_session(1),
    );
    // The policy does not authorize an object without an authPolicy: its digest cannot equal an
    // empty value.
    let (plain_key, plain_name, _) = make_child(&mut t, parent, &ecc_sign(), b"ppw", b"");
    let c2 = sign_command(plain_key, &plain_name, b"ppw");
    err(
        &use_session(&mut t, &mut s, &c2, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    // continueSession CLEAR ends the session after the use.
    let mut e = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &e, cc::SIGN));
    let mut ce = sign_command(key, &name, b"kpw");
    ce.attrs = 0;
    ok(&use_session(&mut t, &mut e, &ce, 0, true, false));
    err(
        &use_session(&mut t, &mut e, &ce, 0, true, false),
        Rc::reference_session(0),
    );
}

#[test]
fn a_policy_with_authvalue_needs_the_hmac_and_a_policy_failure_is_not_a_da_failure() {
    let (mut t, parent) = with_ecc_parent();
    let (key, name, _) = make_child(
        &mut t,
        parent,
        &policy_key(&hex(V_AV_THEN_CC_SIGN)),
        b"kpw",
        b"",
    );
    let tries = |t: &mut Tpm| {
        let r = run(
            t,
            plain(
                cc::GET_CAPABILITY,
                &[],
                &cat(&[
                    &u32b(cap::TPM_PROPERTIES),
                    &u32b(pt::LOCKOUT_COUNTER),
                    &u32b(1),
                ]),
            ),
        );
        let p = params(&r, 0);
        u32::from_be_bytes([
            p[p.len() - 4],
            p[p.len() - 3],
            p[p.len() - 2],
            p[p.len() - 1],
        ])
    };
    let mut s = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &s, &[]));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    let c = sign_command(key, &name, b"kpw");
    // The right HMAC (key = sessionKey || authValue) authorizes.
    ok(&run_hmac(&mut t, &mut s, &c));
    assert_eq!(digest(&mut t, &s), [0; 32]);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &s, &[]));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    // A wrong or an empty HMAC is an authorization failure counted by DA (the object is
    // DA-protected): TPM_RC_AUTH_FAIL.
    assert_eq!(tries(&mut t), 0);
    err(
        &use_session(&mut t, &mut s, &c, 0, false, true),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &use_session(&mut t, &mut s, &c, 0, true, false),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(tries(&mut t), 2);
    // A policy failure is not: the digest of a session without PolicyAuthValue differs.
    let mut z = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &z, cc::SIGN));
    err(
        &use_session(&mut t, &mut z, &c, 0, false, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(tries(&mut t), 2, "a policy failure does not count");
    // The reserved TPM_RS_PW cannot satisfy the policy of this key.
    err(
        &sign(&mut t, key, b"kpw", &[7u8; 32], &ecc_sig_scheme()),
        Rc::POLICY_FAIL.in_session(1),
    );
}

#[test]
fn policy_password_switches_authorization_and_survives_context_save() {
    let (mut t, parent) = with_ecc_parent();
    let digest_twice = h256(&[&hex(V_AUTH_VALUE), &u32b(cc::POLICY_AUTH_VALUE)]);
    let (key, name, _) = make_child(&mut t, parent, &policy_key(&digest_twice), b"kpw", b"");
    let command = sign_command(key, &name, b"kpw");
    let password_command = |session: &Hs, password: &[u8]| {
        let entry = cat(&[
            &u32b(session.handle),
            &tpm2b(&command.nonce_caller),
            &[command.attrs],
            &tpm2b(password),
        ]);
        frame(
            st::SESSIONS,
            command.code,
            &command.handles,
            &cat(&[&u32b(entry.len() as u32), &entry]),
            &command.params,
        )
    };

    ok(&set_auth(&mut t, OWNER, b"", b"binding"));
    let mut session = start_session(&mut t, se::POLICY, OWNER, b"binding", &[0x53; 16]);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &session, &[]));
    ok(&pol(&mut t, cc::POLICY_PASSWORD, &session, &[]));
    assert_eq!(digest(&mut t, &session), digest_twice);
    let context = save(&mut t, session.handle);
    session.handle = loaded(&load_ctx(&mut t, params(&context, 0)));

    err(
        &run(&mut t, password_command(&session, b"bad")),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &session), digest_twice);
    let response = run(&mut t, password_command(&session, b"kpw\0"));
    let (parameters, acks) = split_response(&response, 0);
    let [(nonce, attributes, hmac)] = acks.as_slice() else {
        panic!("expected a single policy-session acknowledgement");
    };
    assert_eq!(*attributes, command.attrs);
    assert_ne!(nonce, &session.nonce_tpm);
    let rp_hash = h256(&[&[0; 4], &u32b(cc::SIGN), &parameters]);
    let expected = crate::crypto::hmac_sha256(
        &session.key,
        &[&rp_hash, nonce, &command.nonce_caller, &[*attributes]],
    );
    assert_eq!(hmac.as_slice(), expected);
    ok(&verify(&mut t, key, &[7; 32], &parameters));
    assert_eq!(digest(&mut t, &session), [0; 32]);
    session.nonce_tpm = nonce.clone();

    ok(&pol(&mut t, cc::POLICY_PASSWORD, &session, &[]));
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &session, &[]));
    assert_eq!(digest(&mut t, &session), digest_twice);
    err(
        &run(&mut t, password_command(&session, b"kpw")),
        Rc::AUTH_FAIL.in_session(1),
    );
    ok(&run_hmac(&mut t, &mut session, &command));
}

#[test]
fn policy_password_with_empty_auth_has_an_empty_response_hmac() {
    let (mut t, parent) = with_ecc_parent();
    let (key, name, _) = make_child(&mut t, parent, &policy_key(&hex(V_AUTH_VALUE)), b"", b"");
    let session = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_PASSWORD, &session, &[]));
    let response = run(
        &mut t,
        hmac_command(&session, &sign_command(key, &name, b""), true, false),
    );
    let (parameters, acks) = split_response(&response, 0);
    assert_eq!(acks.len(), 1);
    assert!(!acks[0].0.is_empty());
    assert!(acks[0].2.is_empty());
    ok(&verify(&mut t, key, &[7; 32], &parameters));
}

#[test]
fn a_pcr_policy_stops_working_when_a_pcr_changes() {
    let (mut t, parent) = with_ecc_parent();
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    // authPolicy = PolicyPCR (PCR 0 and 4) then PolicyCommandCode(Sign), computed by a trial
    // session of this TPM and checked with the independent PolicyPCR vector.
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_pcr(&mut t, &tr, &hex(V_PCR_DIGEST), &sel04()));
    assert_eq!(digest(&mut t, &tr), hex(V_POLICY_PCR));
    ok(&pol_cc(&mut t, &tr, cc::SIGN));
    let auth_policy = digest(&mut t, &tr);
    assert_eq!(
        auth_policy,
        h256(&[
            &hex(V_POLICY_PCR),
            &u32b(cc::POLICY_COMMAND_CODE),
            &u32b(cc::SIGN)
        ])
    );
    let (key, name, _) = make_child(&mut t, parent, &policy_key(&auth_policy), b"kpw", b"");
    let c = sign_command(key, &name, b"kpw");
    let mut s = policy_session(&mut t, se::POLICY);
    ok(&pol_pcr(&mut t, &s, &[], &sel04()));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    ok(&use_session(&mut t, &mut s, &c, 0, true, false));
    ok(&pol_pcr(&mut t, &s, &[], &sel04()));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    // A PCR changes (even one that the policy does not name): the session fails until restarted.
    ok(&extend(&mut t, 7, &hex(SHA256_ABC)));
    err(
        &use_session(&mut t, &mut s, &c, 0, true, false),
        Rc::PCR_CHANGED,
    );
    ok(&pol(&mut t, cc::POLICY_RESTART, &s, &[]));
    ok(&pol_pcr(&mut t, &s, &[], &sel04()));
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    ok(&use_session(&mut t, &mut s, &c, 0, true, false));
    // A change of a PCR that the policy names breaks the digest check of the PolicyPCR command.
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    ok(&pol(&mut t, cc::POLICY_RESTART, &s, &[]));
    err(
        &pol_pcr(&mut t, &s, &hex(V_PCR_DIGEST), &sel04()),
        Rc::VALUE.in_param(1),
    );
}

#[test]
fn policy_or_lets_two_policies_authorize_one_object() {
    let (mut t, parent) = with_ecc_parent();
    // authPolicy = OR(PolicyCommandCode(Sign), PolicyCommandCode(Unseal)).
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol_or(&mut t, &tr, &[&hex(V_CC_SIGN), &[8u8; 32]]));
    let or_digest = digest(&mut t, &tr);
    let (key, name, _) = make_child(&mut t, parent, &policy_key(&or_digest), b"kpw", b"");
    let c = sign_command(key, &name, b"kpw");
    let mut s = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &s, cc::SIGN));
    // The command code stays Sign after the OR (PolicyOR keeps the deferred assertions).
    ok(&pol_or(&mut t, &s, &[&hex(V_CC_SIGN), &[8u8; 32]]));
    assert_eq!(digest(&mut t, &s), or_digest);
    ok(&use_session(&mut t, &mut s, &c, 0, true, false));
}

#[test]
fn policy_secret_can_be_authorized_by_a_policy_session() {
    let (mut t, parent) = with_ecc_parent();
    // An entity whose policy is "PolicyAuthValue, PolicyCommandCode(PolicySecret)".
    let tr = policy_session(&mut t, se::TRIAL);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &tr, &[]));
    ok(&pol_cc(&mut t, &tr, cc::POLICY_SECRET));
    let with_av = digest(&mut t, &tr);
    let tr2 = policy_session(&mut t, se::TRIAL);
    ok(&pol_cc(&mut t, &tr2, cc::POLICY_SECRET));
    let without_av = digest(&mut t, &tr2);
    let (ineligible, ineligible_name, _) =
        make_child(&mut t, parent, &policy_key(&with_av), b"k1", b"");
    let mut object = ecc_sign();
    object.auth_policy = with_av;
    let (k1, n1, _) = make_child(&mut t, parent, &object, b"k1", b"");
    object.auth_policy = without_av;
    let (k2, _, _) = make_child(&mut t, parent, &object, b"k2", b"");
    let target = policy_session(&mut t, se::POLICY);
    // A policy session without PolicyAuthValue/PolicyPassword cannot authorize PolicySecret
    // (TPM_RC_MODE, Part 3, clause 23.4.1).
    let mut a2 = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &a2, cc::POLICY_SECRET));
    let mut c2 = hc(
        cc::POLICY_SECRET,
        &[k2, target.handle],
        secret_params(&[], &[], b"r", 0),
        (k2, b"k2"),
    );
    c2.names[0] = unmarshal_name(&mut t, k2);
    err(
        &use_session(&mut t, &mut a2, &c2, 0, true, false),
        Rc::MODE.in_session(1),
    );
    // With PolicyAuthValue the HMAC proves the authValue and the target digest takes the Name of
    // the object.
    let mut a1 = policy_session(&mut t, se::POLICY);
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &a1, &[]));
    ok(&pol_cc(&mut t, &a1, cc::POLICY_SECRET));
    let authorized_digest = digest(&mut t, &a1);
    let mut rejected = hc(
        cc::POLICY_SECRET,
        &[ineligible, target.handle],
        secret_params(&[], &[], b"r", 0),
        (ineligible, b"k1"),
    );
    rejected.names[0] = ineligible_name;
    err(
        &use_session(&mut t, &mut a1, &rejected, 0, false, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert_eq!(digest(&mut t, &a1), authorized_digest);
    assert_eq!(digest(&mut t, &target), [0; 32]);
    let mut c1 = hc(
        cc::POLICY_SECRET,
        &[k1, target.handle],
        secret_params(&[], &[], b"r", 0),
        (k1, b"k1"),
    );
    c1.names[0] = n1.clone();
    ok(&run_hmac(&mut t, &mut a1, &c1));
    assert_eq!(digest(&mut t, &a1), [0; 32]);
    assert_eq!(
        digest(&mut t, &target),
        h256(&[&h256(&[&[0u8; 32], &u32b(cc::POLICY_SECRET), &n1]), b"r"])
    );
}

#[test]
fn policy_secret_requires_index_read_authorization() {
    use super::tests_b::{define_raw, nv_public};
    use crate::nv::attr;

    let mut t = started();
    let denied = 0x0100_0001;
    let allowed = 0x0100_0002;
    for (index, read) in [(denied, attr::OWNERREAD), (allowed, attr::AUTHREAD)] {
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            b"nv-secret",
            &nv_public(index, alg::SHA256, attr::OWNERWRITE | read, &[], 8),
        ));
    }
    let target = policy_session(&mut t, se::POLICY);
    let parameters = secret_params(&[], &[], b"ref", 0);
    err(
        &secret_pw(&mut t, denied, b"nv-secret", &target, &parameters),
        Rc::AUTH_UNAVAILABLE,
    );
    assert_eq!(digest(&mut t, &target), [0; 32]);
    ok(&secret_pw(
        &mut t,
        allowed,
        b"nv-secret",
        &target,
        &parameters,
    ));
    assert_ne!(digest(&mut t, &target), [0; 32]);
}

#[test]
fn nv_policy_secret_requires_authread_and_proof_even_on_an_unwritten_index() {
    let mut t = started();
    let target = policy_session(&mut t, se::POLICY);
    let mut authorizer = policy_session(&mut t, se::POLICY);
    let secret = b"nv-secret";
    let policy_only = NV0;
    let auth_read = NV0 + 1;
    let no_proof = NV0 + 2;
    let with_auth = hex(V_AUTH_VALUE);
    for (index, read, policy) in [
        (policy_only, nv_attr::POLICYREAD, with_auth.as_slice()),
        (auth_read, nv_attr::AUTHREAD, with_auth.as_slice()),
        (no_proof, nv_attr::AUTHREAD, &[0; 32]),
    ] {
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            secret,
            &nv_public(index, alg::SHA256, nv_attr::OWNERWRITE | read, policy, 1),
        ));
    }
    let parameters = secret_params(&[], &[], b"index reference", 0);
    let mut command = hc(
        cc::POLICY_SECRET,
        &[no_proof, target.handle],
        parameters,
        (no_proof, secret),
    );
    err(
        &use_session(&mut t, &mut authorizer, &command, 0, true, false),
        Rc::MODE.in_session(1),
    );
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, &authorizer, &[]));
    command.handles[0] = policy_only;
    err(
        &use_session(&mut t, &mut authorizer, &command, 0, false, false),
        Rc::AUTH_UNAVAILABLE,
    );
    command.handles[0] = auth_read;
    command.entity.0 = auth_read;
    let (_, name) = nv_read_public(&mut t, auth_read);
    command.names = vec![name.clone(), u32b(target.handle).to_vec()];
    err(
        &use_session(&mut t, &mut authorizer, &command, 0, false, true),
        Rc::AUTH_FAIL.in_session(1),
    );
    ok(&use_session(
        &mut t,
        &mut authorizer,
        &command,
        0,
        false,
        false,
    ));
    let update = h256(&[&[0; 32], &u32b(0x151), &name]);
    assert_eq!(
        digest(&mut t, &target),
        h256(&[&update, b"index reference"])
    );

    ok(&hierarchy_control(&mut t, PLATFORM, b"", OWNER, 0));
    err(
        &secret_pw(&mut t, auth_read, secret, &target, &command.params),
        Rc::HANDLE.in_handle(1),
    );
}

fn unmarshal_name(t: &mut Tpm, h: u32) -> Vec<u8> {
    let r = read_public(t, h);
    let mut rd = Reader::new(params(&r, 0));
    let _public = rd.tpm2b(1024).unwrap();
    rd.tpm2b(64).unwrap().to_vec()
}

#[test]
fn lockout_bind_protects_policy_ahead_of_validation_and_preserves_both_gates() {
    let (mut t, parent) = with_ecc_parent();
    let lockout_auth = b"recovery key";
    ok(&set_auth(&mut t, LOCKOUT, b"", lockout_auth));
    let loc_digest = h256(&[&[0; 32], &u32b(cc::POLICY_LOCALITY), &[1]]);
    let mut template = sealed();
    template.attributes |= oa::NO_DA;
    template.auth_policy = h256(&[
        &loc_digest,
        &u32b(cc::POLICY_COMMAND_CODE),
        &u32b(cc::UNSEAL),
    ]);
    let (key, key_name, _) = make_child(&mut t, parent, &template, b"", b"contents");
    let mut unseal = hc(cc::UNSEAL, &[key], Vec::new(), (key, b""));
    unseal.names = vec![key_name];

    let mut p = start_session(&mut t, se::POLICY, LOCKOUT, lockout_auth, &[0x55; 16]);
    err(
        &use_session(&mut t, &mut p, &unseal, 0, true, false),
        Rc::POLICY_FAIL.in_session(1),
    );
    assert!(t.persistent.da.lockout_auth_enabled);
    ok(&pol(&mut t, cc::POLICY_RESTART, &p, &[]));
    ok(&pol(&mut t, cc::POLICY_LOCALITY, &p, &[1]));
    ok(&pol_cc(&mut t, &p, cc::UNSEAL));
    let saved = params(&save(&mut t, p.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t, &saved)), p.handle);

    ok(&pw_cmd(
        &mut t,
        cc::DICTIONARY_ATTACK_PARAMETERS,
        &[LOCKOUT],
        lockout_auth,
        &cat(&[&u32b(1), &u32b(600), &u32b(86_400)]),
    ));
    err(
        &create_child(&mut t, parent, b"wrong", b"", b"", &sealed()),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert!(t.da_in_lockout() && t.persistent.da.lockout_auth_enabled);
    let mut hmac = start_hmac(&mut t, LOCKOUT, lockout_auth, &[0x56; 16]);
    let mut create = hc(
        cc::CREATE,
        &[parent],
        creation_params(b"", b"", &sealed(), b"", &NO_PCR),
        (parent, b"parent-pw"),
    );
    create.names = vec![unmarshal_name(&mut t, parent)];
    err(
        &use_session(&mut t, &mut hmac, &create, 0, false, false),
        Rc::LOCKOUT,
    );
    assert!(t.persistent.da.lockout_auth_enabled);
    // The target noDA object can still use the independent, enabled Lockout secret.
    assert_eq!(
        params(&use_session(&mut t, &mut p, &unseal, 0, false, false), 0),
        tpm2b(b"contents")
    );
    ok(&pol(&mut t, cc::POLICY_LOCALITY, &p, &[1]));
    ok(&pol_cc(&mut t, &p, cc::UNSEAL));
    err(
        &use_session(&mut t, &mut p, &unseal, 0, true, false),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert!(!t.persistent.da.lockout_auth_enabled);
    assert_eq!(t.persistent.da.failed_tries, 1);
    ok(&pol(&mut t, cc::POLICY_RESTART, &p, &[]));
    err(
        &use_session(&mut t, &mut p, &unseal, 0, false, false),
        Rc::LOCKOUT,
    );
}

#[test]
fn da_lockout_precedes_role_and_flagged_policy_checks() {
    let (mut t, parent) = with_ecc_parent();
    let mut template = ecc_sign();
    template.attributes |= oa::ADMIN_WITH_POLICY;
    template.auth_policy = h256(&[
        &[0; 32],
        &u32b(cc::POLICY_COMMAND_CODE),
        &u32b(cc::OBJECT_CHANGE_AUTH),
    ]);
    let (key, name, public) = make_child(&mut t, parent, &template, b"old", b"");
    let parent_name = unmarshal_name(&mut t, parent);
    let mut change = hc(
        cc::OBJECT_CHANGE_AUTH,
        &[key, parent],
        tpm2b(b"new"),
        (key, b"old"),
    );
    change.names = vec![name, parent_name];
    let change_pw = |t: &mut Tpm, target| {
        pw_cmd(
            t,
            cc::OBJECT_CHANGE_AUTH,
            &[target, parent],
            b"old",
            &change.params,
        )
    };
    err(&change_pw(&mut t, key), Rc::AUTH_TYPE);
    ok(&pw_cmd(
        &mut t,
        cc::DICTIONARY_ATTACK_PARAMETERS,
        &[LOCKOUT],
        b"",
        &cat(&[&u32b(1), &u32b(600), &u32b(86_400)]),
    ));
    err(
        &sign(&mut t, key, b"wrong", &[0; 32], &ecc_sig_scheme()),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert!(t.da_in_lockout());
    let persistent = t.persistent.da;
    let timer = t.da_v;

    err(&change_pw(&mut t, key), Rc::LOCKOUT);
    let mut hmac = start_hmac(&mut t, NULL, b"", &[0x55; 16]);
    err(
        &use_session(&mut t, &mut hmac, &change, 0, false, false),
        Rc::LOCKOUT,
    );
    ok(&flush(&mut t, hmac.handle));
    for (kind, assertion, expected) in [
        (se::POLICY, Some(cc::POLICY_AUTH_VALUE), Rc::LOCKOUT),
        (se::POLICY, Some(cc::POLICY_PASSWORD), Rc::LOCKOUT),
        (se::POLICY, None, Rc::POLICY_FAIL.in_session(1)),
        (
            se::TRIAL,
            Some(cc::POLICY_PASSWORD),
            Rc::ATTRIBUTES.in_session(1),
        ),
    ] {
        let mut session = policy_session(&mut t, kind);
        if let Some(code) = assertion {
            ok(&pol(&mut t, code, &session, &[]));
        }
        let before = digest(&mut t, &session);
        err(
            &use_session(&mut t, &mut session, &change, 0, true, false),
            expected,
        );
        assert_eq!(digest(&mut t, &session), before);
        let slot = t.sessions.slot_of(session.handle).unwrap();
        assert_eq!(t.sessions.get(slot).unwrap().nonce_tpm, session.nonce_tpm);
        ok(&flush(&mut t, session.handle));
    }
    let mut pure = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &pure, cc::OBJECT_CHANGE_AUTH));
    ok(&use_session(&mut t, &mut pure, &change, 0, true, false));

    let mut external = unmarshal_public(&public);
    external.attributes = oa::SIGN;
    let (external, _) = parse_loaded(&load_external(&mut t, &external, NULL));
    err(&change_pw(&mut t, external), Rc::AUTH_UNAVAILABLE);
    assert_eq!(t.persistent.da, persistent);
    assert_eq!(t.da_v.heal_epoch_ms, timer.heal_epoch_ms);
    assert_eq!(t.da_v.lockout_fail_ms, timer.lockout_fail_ms);
}

#[test]
fn certify_is_an_admin_role_command() {
    let (mut t, parent) = with_ecc_parent();
    let signer = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign()));
    // The certified object has adminWithPolicy SET and a policy that names TPM2_Certify.
    let mut obj = ecc_sign();
    obj.attributes |= oa::ADMIN_WITH_POLICY;
    obj.auth_policy = hex(V_CC_CERTIFY);
    let (key, name, _) = make_child(&mut t, parent, &obj, b"opw", b"");
    let qualifying = [1u8; 4];
    let params_ = cat(&[&tpm2b(&qualifying), &ecc_sig_scheme()]);
    let cert_pw = |t: &mut Tpm| {
        run(
            t,
            with_pw(cc::CERTIFY, &[key, signer.handle], &[b"opw", b""], &params_),
        )
    };
    // A password cannot do ADMIN-role work on an adminWithPolicy object: TPM_RC_AUTH_TYPE.
    err(&cert_pw(&mut t), Rc::AUTH_TYPE);
    // A policy session without PolicyCommandCode(Certify) cannot either: TPM_RC_POLICY_FAIL
    // (Part 3, clause 5.6, step 6), even if its digest matches.
    let mut p = policy_session(&mut t, se::POLICY);
    let mut c = hc(
        cc::CERTIFY,
        &[key, signer.handle],
        params_.clone(),
        (key, b"opw"),
    );
    c.names = vec![name.clone(), signer.name.clone()];
    // (the digest of this session is zero: policy failure as well)
    err(
        &run(&mut t, certify_two_sessions(&p, &c, b"")),
        Rc::POLICY_FAIL.in_session(1),
    );
    // With the command code the policy session authorizes `objectHandle`, a password the key.
    ok(&pol_cc(&mut t, &p, cc::CERTIFY));
    assert_eq!(digest(&mut t, &p), hex(V_CC_CERTIFY));
    err(
        &run(&mut t, certify_two_sessions(&p, &c, b"wrong")),
        Rc::AUTH_FAIL.in_session(2),
    );
    assert_eq!(digest(&mut t, &p), hex(V_CC_CERTIFY));
    let resp = run(&mut t, certify_two_sessions(&p, &c, b""));
    ok(&resp);
    let (_, acks) = split_response(&resp, 0);
    p.nonce_tpm = acks[0].0.clone();
    // The attested structure names the certified object (Name and Qualified Name).
    let (attest, _sig) = parse_attestation(&resp);
    assert_eq!(&attest[..4], &hex("ff544347")[..]);
    assert_eq!(&attest[4..6], &u16b(ST_ATTEST_CERTIFY)[..]);
    assert!(contains(&attest, &cat(&[&u16b(name.len() as u16), &name])));
    // The signing key needs its own authorization (session 2): a wrong password is refused
    // (the endorsement primary has an empty authValue; the object below has "opw").
    let (plain_key, _, plain_pub) = make_child(&mut t, parent, &ecc_sign(), b"ppw", b"");
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY,
                &[plain_key, plain_key],
                &[b"ppw", b"bad"],
                &params_,
            ),
        ),
        Rc::AUTH_FAIL.in_session(2),
    );
    // A key that has no private part cannot be certified (TPM_RC_AUTH_UNAVAILABLE).
    let (ext, _) = parse_loaded(&load_external(&mut t, &unmarshal_public(&plain_pub), NULL));
    err(
        &run(
            &mut t,
            with_pw(cc::CERTIFY, &[ext, signer.handle], &[b"", b""], &params_),
        ),
        Rc::AUTH_UNAVAILABLE,
    );
    // adminWithPolicy CLEAR: any session type works for objectHandle, the authValue is checked.
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY,
                &[plain_key, signer.handle],
                &[b"bad", b""],
                &params_,
            ),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    ok(&run(
        &mut t,
        with_pw(
            cc::CERTIFY,
            &[plain_key, signer.handle],
            &[b"ppw", b""],
            &params_,
        ),
    ));
}

/// A `TPM2_Certify` command with the policy session `p` (empty HMAC) for `objectHandle` and a
/// password session for `signHandle`.
fn certify_two_sessions(p: &Hs, c: &Hc, signer_password: &[u8]) -> Vec<u8> {
    let mut area = Vec::new();
    area.extend_from_slice(&p.handle.to_be_bytes());
    area.extend_from_slice(&tpm2b(&[0x33; 16]));
    area.push(0);
    area.extend_from_slice(&tpm2b(&[]));
    area.extend_from_slice(&handle::RS_PW.to_be_bytes());
    area.extend_from_slice(&tpm2b(&[]));
    area.push(1);
    area.extend_from_slice(&tpm2b(signer_password));
    let mut auth = (area.len() as u32).to_be_bytes().to_vec();
    auth.extend_from_slice(&area);
    frame(st::SESSIONS, c.code, &c.handles, &auth, &c.params)
}

pub(super) fn parse_attestation(resp: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut rd = Reader::new(params(resp, 0));
    let attest = rd.tpm2b(1024).unwrap().to_vec();
    let rest = rd.bytes(rd.remaining()).unwrap().to_vec();
    (attest, rest)
}

// ------------------------------------------------------------------------------------- HMAC

fn hmac_key(scheme: Scheme, origin: bool) -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes: oa::FIXED_TPM
            | oa::FIXED_PARENT
            | oa::USER_WITH_AUTH
            | oa::SIGN
            | if origin { oa::SENSITIVE_DATA_ORIGIN } else { 0 },
        auth_policy: vec![],
        params: PublicParams::KeyedHash { scheme },
        unique: Unique::Digest(vec![]),
    }
}

const HMAC_SHA256: Scheme = Scheme {
    alg: alg::HMAC,
    hash: alg::SHA256,
};

fn hmac_cmd(t: &mut Tpm, key: u32, pw: &[u8], buffer: &[u8], hash: u16) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::HMAC,
            &[key],
            &[pw],
            &cat(&[&tpm2b(buffer), &u16b(hash)]),
        ),
    )
}

fn hmac_out(resp: &[u8]) -> Vec<u8> {
    let mut rd = Reader::new(params(resp, 0));
    let d = rd.tpm2b(64).unwrap().to_vec();
    rd.finish().unwrap();
    d
}

fn hmac_start(t: &mut Tpm, key: u32, pw: &[u8], auth: &[u8], hash: u16) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::HMAC_START,
            &[key],
            &[pw],
            &cat(&[&tpm2b(auth), &u16b(hash)]),
        ),
    )
}

#[test]
fn hmac_start_streams_across_chunks_and_releases_on_success() {
    let (mut t, parent) = with_ecc_parent();
    let secret: Vec<u8> = (1..=128).collect();
    let (key, _, _) = make_child(
        &mut t,
        parent,
        &hmac_key(HMAC_SHA256, false),
        b"key",
        &secret,
    );
    let r = hmac_start(&mut t, key, b"key", b"sequence", alg::NULL);
    let sequence = u32::from_be_bytes(r[10..14].try_into().unwrap());
    let (parameters, acks) = split_response(&r, 1);
    assert!(parameters.is_empty());
    assert_eq!(acks, vec![(Vec::new(), 1, Vec::new())]);
    ok(&flush(&mut t, key));
    err(
        &seq_update(&mut t, sequence, b"key", b"bad"),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&seq_update(&mut t, sequence, b"sequence", &[0xa5; 1024]));
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVENT_SEQUENCE_COMPLETE,
                &[8, sequence],
                &[b"", b"sequence"],
                &tpm2b(b"bad"),
            ),
        ),
        Rc::MODE.in_handle(2),
    );
    ok(&seq_update(&mut t, sequence, b"sequence", &[0x5a; 1024]));
    let complete = seq_complete(&mut t, sequence, b"sequence", b"tail", OWNER);
    let (digest, ticket) = digest_and_ticket(&complete);
    // Python hmac with key 01..80 and message A5×1024 || 5A×1024 || "tail".
    assert_eq!(
        digest,
        hex("e0544081cb513ab12e4a18bddb78ff8e66af37d9fff0d1e6ac3c13975c018bde")
    );
    assert_eq!((ticket.hierarchy, ticket.digest), (NULL, Vec::new()));
    err(
        &seq_update(&mut t, sequence, b"sequence", b""),
        Rc::reference_handle(0),
    );
}

#[test]
fn hmac_start_uses_the_shared_key_rules_and_sequence_capacity() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(
        &mut t,
        parent,
        &hmac_key(Scheme::NULL, false),
        b"key",
        b"secret",
    );
    err(
        &hmac_start(&mut t, key, b"key", b"", alg::NULL),
        Rc::VALUE.in_param(2),
    );
    err(
        &hmac_start(&mut t, key, b"bad", b"", alg::SHA256),
        Rc::AUTH_FAIL.in_session(1),
    );
    for slot in 0..3 {
        let r = hmac_start(&mut t, key, b"key", b"", alg::SHA256);
        ok(&r);
        assert_eq!(&r[10..14], &(0x8000_0000u32 + slot).to_be_bytes());
    }
    err(&start_seq(&mut t, b"", alg::SHA256), Rc::OBJECT_MEMORY);
    err(
        &hmac_start(&mut t, key, b"key", b"", alg::SHA256),
        Rc::OBJECT_MEMORY,
    );
    ok(&flush(&mut t, 0x8000_0001));
    assert_eq!(
        seq_handle(&start_seq(&mut t, b"", alg::SHA256)),
        0x8000_0001
    );
}

#[test]
fn hmac_matches_the_rfc_4231_vectors_for_keyed_hash_objects() {
    let (mut t, parent) = with_ecc_parent();
    // Test cases 1-4 of RFC 4231 (HMAC-SHA-256), recomputed with Python `hmac` as well.
    let cases: [(Vec<u8>, Vec<u8>, &str); 4] = [
        (
            vec![0x0b; 20],
            b"Hi There".to_vec(),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        ),
        (
            b"Jefe".to_vec(),
            b"what do ya want for nothing?".to_vec(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        ),
        (
            vec![0xaa; 20],
            vec![0xdd; 50],
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
        ),
        (
            (1u8..=25).collect(),
            vec![0xcd; 50],
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
        ),
    ];
    for scheme in [HMAC_SHA256, Scheme::NULL] {
        for (key, data, expect) in &cases {
            let (h, _, _) = make_child(&mut t, parent, &hmac_key(scheme, false), b"hk", key);
            // With the key's scheme `hashAlg` may be NULL or SHA-256; with a NULL scheme it must
            // name the hash.
            let r = hmac_cmd(&mut t, h, b"hk", data, alg::SHA256);
            assert_eq!(hmac_out(&r), hex(expect));
            if scheme.alg == alg::HMAC {
                let r = hmac_cmd(&mut t, h, b"hk", data, alg::NULL);
                assert_eq!(hmac_out(&r), hex(expect));
            }
            ok(&flush(&mut t, h));
        }
    }
    // The empty buffer and the largest buffer.
    let (h, _, _) = make_child(&mut t, parent, &hmac_key(HMAC_SHA256, false), b"hk", b"k");
    assert_eq!(
        hmac_out(&hmac_cmd(&mut t, h, b"hk", b"", alg::NULL)).len(),
        32
    );
    let big = vec![1u8; 1024];
    assert_eq!(
        hmac_out(&hmac_cmd(&mut t, h, b"hk", &big, alg::NULL)).len(),
        32
    );
    err(
        &hmac_cmd(&mut t, h, b"hk", &vec![1u8; 1025], alg::NULL),
        Rc::SIZE.in_param(1),
    );
}

#[test]
fn hmac_key_generated_by_the_tpm_and_error_cases() {
    let (mut t, parent) = with_ecc_parent();
    // The TPM makes the key (data empty, sensitiveDataOrigin SET); the HMAC is stable and the key
    // does not leave the TPM.
    let c = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"hk",
        b"",
        &hmac_key(HMAC_SHA256, true),
    ));
    let (h, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &c.private, &c.public));
    let a = hmac_out(&hmac_cmd(&mut t, h, b"hk", b"msg", alg::NULL));
    assert_eq!(a, hmac_out(&hmac_cmd(&mut t, h, b"hk", b"msg", alg::NULL)));
    assert_ne!(a, hmac_out(&hmac_cmd(&mut t, h, b"hk", b"msg2", alg::NULL)));
    let key = secret_of(&t, h);
    assert_eq!(key.len(), 32);
    for blob in [&c.private, &c.public, &a] {
        assert!(!contains(blob, &key));
    }
    // Authorization: a wrong password (DA-protected object), no session, an unloaded handle.
    err(
        &hmac_cmd(&mut t, h, b"bad", b"x", alg::NULL),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::HMAC, &[h], &cat(&[&tpm2b(b"x"), &u16b(alg::NULL)])),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &hmac_cmd(&mut t, 0x8000_0020, b"", b"x", alg::NULL),
        Rc::reference_handle(0),
    );
    // hashAlg: SHA-1 and unknown algorithms are TPM_RC_HASH; a NULL-scheme key needs a hash.
    err(
        &hmac_cmd(&mut t, h, b"hk", b"x", 0x0004),
        Rc::HASH.in_param(2),
    );
    err(
        &hmac_cmd(&mut t, h, b"hk", b"x", 0xFFFF),
        Rc::HASH.in_param(2),
    );
    let (nk, _, _) = make_child(&mut t, parent, &hmac_key(Scheme::NULL, true), b"nk", b"");
    err(
        &hmac_cmd(&mut t, nk, b"nk", b"x", alg::NULL),
        Rc::VALUE.in_param(2),
    );
    // Trailing octets and a missing hashAlg.
    let mut long = cat(&[&tpm2b(b"x"), &u16b(alg::NULL)]);
    long.push(0);
    err(
        &run(&mut t, with_pw(cc::HMAC, &[h], &[b"hk"], &long)),
        Rc::SIZE,
    );
    err(
        &run(&mut t, with_pw(cc::HMAC, &[h], &[b"hk"], &tpm2b(b"x"))),
        Rc::INSUFFICIENT.in_param(2),
    );
    // Another key type: TPM_RC_TYPE; without `sign`: TPM_RC_KEY (checked first).
    let (ek, _, _) = make_child(&mut t, parent, &ecc_sign(), b"ek", b"");
    err(
        &hmac_cmd(&mut t, ek, b"ek", b"x", alg::SHA256),
        Rc::TYPE.in_handle(1),
    );
    let (dk, _, _) = make_child(&mut t, parent, &sealed(), b"dk", b"data");
    err(
        &hmac_cmd(&mut t, dk, b"dk", b"x", alg::SHA256),
        Rc::KEY.in_handle(1),
    );
    err(
        &hmac_cmd(&mut t, parent, b"parent-pw", b"x", alg::SHA256),
        Rc::KEY.in_handle(1),
    );
    // A restricted keyed hash cannot be made by a command but is refused all the same
    // (Part 3, clause 15.5.1: TPM_RC_ATTRIBUTES; `TPM2_Sign` is the way with a ticket).
    {
        use crate::hash::AuthValue;
        use crate::object::{Object, Secret, Sensitive};
        use zeroize::Zeroizing;
        let mut p = hmac_key(HMAC_SHA256, false);
        p.attributes |= oa::RESTRICTED;
        p.unique = Unique::Digest(vec![1; 32]);
        let s = Sensitive {
            auth: AuthValue::empty(),
            seed: Zeroizing::new(vec![]),
            secret: Secret::Data(Zeroizing::new(vec![1, 2, 3])),
        };
        let rh = t
            .objects
            .insert(Object::new(p, Some(s), OWNER, &u32b(OWNER)))
            .unwrap();
        err(
            &hmac_cmd(&mut t, rh, b"", b"x", alg::SHA256),
            Rc::ATTRIBUTES.in_handle(1),
        );
    }
    // Other uses of an HMAC key: Unseal (TPM_RC_ATTRIBUTES), Sign (TPM_RC_KEY).
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[h], &[b"hk"], &[])),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &sign(&mut t, h, b"hk", &[7u8; 32], &ecc_sig_scheme()),
        Rc::KEY.in_handle(1),
    );
}

#[test]
fn hmac_key_templates_are_checked() {
    let (mut t, parent) = with_ecc_parent();
    let go = |t: &mut Tpm, p: &TpmtPublic, data: &[u8]| {
        create_child(t, parent, b"parent-pw", b"", data, p)
    };
    // The key bytes and sensitiveDataOrigin exclude each other.
    err(
        &go(&mut t, &hmac_key(HMAC_SHA256, true), b"key"),
        Rc::ATTRIBUTES.in_param(2),
    );
    err(
        &go(&mut t, &hmac_key(HMAC_SHA256, false), b""),
        Rc::ATTRIBUTES.in_param(2),
    );
    // sign + decrypt (XOR or a general keyed hash) is not implemented.
    let mut both = hmac_key(HMAC_SHA256, false);
    both.attributes |= oa::DECRYPT;
    err(&go(&mut t, &both, b"key"), Rc::ATTRIBUTES.in_param(2));
    // A scheme on a sealed object; a scheme other than HMAC/SHA-256.
    let mut sealed_hmac = sealed();
    if let PublicParams::KeyedHash { scheme } = &mut sealed_hmac.params {
        *scheme = HMAC_SHA256;
    }
    err(&go(&mut t, &sealed_hmac, b"d"), Rc::SCHEME.in_param(2));
    let mut raw = hmac_key(HMAC_SHA256, false).to_bytes();
    // `TPMT_PUBLIC`: type, nameAlg, attributes, authPolicy(2), scheme (0005 000B): the hash of
    // the scheme becomes SHA-1.
    let at = raw.len() - 2 - 4;
    raw[at..at + 4].copy_from_slice(&[0x00, 0x05, 0x00, 0x04]);
    let params_ = cat(&[
        &tpm2b(&cat(&[&tpm2b(b""), &tpm2b(b"key")])),
        &tpm2b(&raw),
        &tpm2b(b""),
        &NO_PCR,
    ]);
    err(
        &run(
            &mut t,
            with_pw(cc::CREATE, &[parent], &[b"parent-pw"], &params_),
        ),
        Rc::HASH.in_param(2),
    );
    // The public area with the HMAC scheme round-trips through Load and ReadPublic.
    let (h, _, public) = make_child(&mut t, parent, &hmac_key(HMAC_SHA256, false), b"hk", b"key");
    let p = unmarshal_public(&public);
    assert_eq!(
        p.params,
        PublicParams::KeyedHash {
            scheme: HMAC_SHA256
        }
    );
    let r = read_public(&mut t, h);
    assert_eq!(Reader::new(params(&r, 0)).tpm2b(1024).unwrap(), &public[..]);
    // A primary HMAC key is a function of the seed, the template and the key bytes.
    let a = parse_primary(&create_primary_raw(
        &mut t,
        OWNER,
        b"",
        &creation_params(b"", b"key", &hmac_key(HMAC_SHA256, false), b"", &NO_PCR),
    ));
    let b = parse_primary(&create_primary_raw(
        &mut t,
        OWNER,
        b"",
        &creation_params(b"", b"key", &hmac_key(HMAC_SHA256, false), b"", &NO_PCR),
    ));
    assert_eq!(a.name, b.name);
    assert_eq!(
        hmac_out(&hmac_cmd(&mut t, a.handle, b"", b"m", alg::NULL)),
        hmac_out(&hmac_cmd(&mut t, b.handle, b"", b"m", alg::NULL))
    );
}

// ------------------------------------------------------------------------------- attestation

/// `TPMS_ATTEST` assembled byte by byte from Part 2 v184, Table 151.
pub(super) fn attest_bytes(
    ty: u16,
    qn: &[u8],
    extra: &[u8],
    clock: (u64, u32, u32),
    firmware: u64,
    attested: &[u8],
) -> Vec<u8> {
    cat(&[
        &hex("ff544347"),
        &u16b(ty),
        &tpm2b(qn),
        &tpm2b(extra),
        &clock.0.to_be_bytes(),
        &clock.1.to_be_bytes(),
        &clock.2.to_be_bytes(),
        &[1],
        &firmware.to_be_bytes(),
        attested,
    ])
}

pub(super) const FIRMWARE: u64 = 0x0001_0000_0000_0000;

pub(super) fn qualified(parent: u32, name: &[u8]) -> Vec<u8> {
    cat(&[&u16b(alg::SHA256), &h256(&[&u32b(parent), name])])
}

pub(super) fn quote(
    t: &mut Tpm,
    key: u32,
    pw: &[u8],
    qualifying: &[u8],
    scheme: &[u8],
    sel: &[u8],
) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::QUOTE,
            &[key],
            &[pw],
            &cat(&[&tpm2b(qualifying), scheme, sel]),
        ),
    )
}

pub(super) fn certify(
    t: &mut Tpm,
    obj: u32,
    key: u32,
    pws: [&[u8]; 2],
    qualifying: &[u8],
    scheme: &[u8],
) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::CERTIFY,
            &[obj, key],
            &pws,
            &cat(&[&tpm2b(qualifying), scheme]),
        ),
    )
}

fn certify_creation_params(
    nonce: &[u8],
    hash: &[u8],
    scheme: &[u8],
    ticket: &(u16, u32, Vec<u8>),
) -> Vec<u8> {
    cat(&[
        &tpm2b(nonce),
        &tpm2b(hash),
        scheme,
        &u16b(ticket.0),
        &u32b(ticket.1),
        &tpm2b(&ticket.2),
    ])
}

#[test]
fn certify_creation_attests_the_creation_hash_and_child_name() {
    let mut t = started();
    let signer = parse_primary(&create_primary(&mut t, PLATFORM, b"signer", &ecc_sign()));
    let owner = parse_primary(&create_primary(&mut t, OWNER, b"parent", &ecc_storage()));
    let child = parse_child(&create_child(
        &mut t,
        owner.handle,
        b"parent",
        b"child-auth",
        b"",
        &ecc_sign(),
    ));
    let (handle, name) = parse_loaded(&load(
        &mut t,
        owner.handle,
        b"parent",
        &child.private,
        &child.public,
    ));
    let args = certify_creation_params(b"fresh", &child.hash, &u16b(alg::NULL), &child.ticket);
    let r = run(
        &mut t,
        with_pw(
            cc::CERTIFY_CREATION,
            &[signer.handle, handle],
            &[b"signer"],
            &args,
        ),
    );
    let (attestation, signature) = parse_attestation(&r);
    assert_eq!(
        attestation,
        attest_bytes(
            0x801a,
            &qualified(PLATFORM, &signer.name),
            b"fresh",
            (0, 1, 0),
            FIRMWARE,
            &cat(&[&tpm2b(&name), &tpm2b(&child.hash)]),
        ),
    );
    ok(&verify(
        &mut t,
        signer.handle,
        &h256(&[&attestation]),
        &signature,
    ));

    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY_CREATION,
                &[signer.handle, owner.handle],
                &[b"signer"],
                &args,
            ),
        ),
        Rc::TICKET.in_param(4),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY_CREATION,
                &[signer.handle, handle],
                &[b"child-auth"],
                &args,
            ),
        ),
        Rc::BAD_AUTH.in_session(1),
    );
    let wrong_scheme = cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256)]);
    let args = certify_creation_params(b"fresh", &child.hash, &wrong_scheme, &child.ticket);
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY_CREATION,
                &[signer.handle, handle],
                &[b"signer"],
                &args,
            ),
        ),
        Rc::SCHEME.in_param(3),
    );
}

#[test]
fn certify_creation_rejects_empty_forged_and_mismatched_null_hierarchy_tickets() {
    let mut t = started();
    let object = parse_primary(&create_primary(&mut t, NULL, b"object-auth", &ecc_sign()));
    let handles = [NULL, object.handle];
    let send = |t: &mut Tpm, hash: &[u8], ticket: &(u16, u32, Vec<u8>)| {
        let args = certify_creation_params(b"fresh", hash, &u16b(alg::NULL), ticket);
        run(t, with_pw(cc::CERTIFY_CREATION, &handles, &[b""], &args))
    };
    let success = send(&mut t, &object.hash, &object.ticket);
    let (attested, signature) = parse_attestation(&success);
    assert_eq!(attested[4..6], [0x80, 0x1a]);
    assert_eq!(signature, [0, 0x10]);
    assert!(attested.ends_with(&cat(&[&tpm2b(&object.name), &tpm2b(&object.hash)])));
    let mut forged = object.ticket.clone();
    forged.2[0] ^= 1;
    err(&send(&mut t, &object.hash, &forged), Rc::TICKET.in_param(4));
    err(
        &send(&mut t, &object.hash, &(0x8021, NULL, Vec::new())),
        Rc::TICKET.in_param(4),
    );
    let mut other_hash = object.hash.clone();
    other_hash[0] ^= 1;
    err(
        &send(&mut t, &other_hash, &object.ticket),
        Rc::TICKET.in_param(4),
    );
    let mut malformed = object.ticket.clone();
    malformed.0 = 0x8022;
    err(&send(&mut t, &object.hash, &malformed), Rc::TAG.in_param(4));

    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    let old_public = unmarshal_public(&object.public);
    let (loaded, same_name) = parse_loaded(&load_external(&mut t, &old_public, NULL));
    assert_eq!(same_name, object.name);
    let args = certify_creation_params(b"fresh", &object.hash, &u16b(alg::NULL), &object.ticket);
    err(
        &run(
            &mut t,
            with_pw(cc::CERTIFY_CREATION, &[NULL, loaded], &[b""], &args),
        ),
        Rc::TICKET.in_param(4),
    );
}

fn quote_attested() -> Vec<u8> {
    cat(&[&sel04(), &tpm2b(&hex(V_PCR_DIGEST))])
}

pub(super) fn ecdsa_parts(sig: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut rd = Reader::new(sig);
    assert_eq!(rd.u16().unwrap(), alg::ECDSA);
    assert_eq!(rd.u16().unwrap(), alg::SHA256);
    let r = rd.tpm2b(32).unwrap().to_vec();
    let s = rd.tpm2b(32).unwrap().to_vec();
    rd.finish().unwrap();
    (r, s)
}

#[test]
fn quote_signs_the_attest_structure_and_the_signature_verifies() {
    let mut t = started();
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    // A platform key: its attestations are not obfuscated (Part 3, clause 18.1).
    let k = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let qn = qualified(PLATFORM, &k.name);
    let r = quote(
        &mut t,
        k.handle,
        b"kpw",
        b"nonce",
        &u16b(alg::NULL),
        &sel04(),
    );
    let (attest, sig) = parse_attestation(&r);
    let expect = attest_bytes(
        ST_ATTEST_QUOTE,
        &qn,
        b"nonce",
        (0, 1, 0),
        FIRMWARE,
        &quote_attested(),
    );
    assert_eq!(attest, expect);
    // The TPM verifies its own signature over SHA-256(attest) (the VerifySignature path) ...
    let digest = h256(&[&attest]);
    ok(&verify(&mut t, k.handle, &digest, &sig));
    // ... and a changed attest does not verify.
    let mut bad = attest.clone();
    bad[10] ^= 1;
    err(
        &verify(&mut t, k.handle, &h256(&[&bad]), &sig),
        Rc::SIGNATURE,
    );
    // External check: the pinned public key, attest and signature (RFC 6979 makes ECDSA
    // deterministic) were verified on 2026-11-07 with plain Python P-256 arithmetic, an
    // affine-coordinate ECDSA check on SHA-256(attest); the same code rejects a changed attest.
    let Unique::Ecc { x, y } = unmarshal_public(&k.public).unique else {
        panic!()
    };
    let (r_, s_) = ecdsa_parts(&sig);
    assert_eq!(hexs(&x), QUOTE_X);
    assert_eq!(hexs(&y), QUOTE_Y);
    assert_eq!(hexs(&r_), QUOTE_R);
    assert_eq!(hexs(&s_), QUOTE_S);
    assert_eq!(hexs(&attest), QUOTE_ATTEST);
    // A different qualifyingData, selection or PCR value changes what is signed.
    let r2 = quote(
        &mut t,
        k.handle,
        b"kpw",
        b"other",
        &u16b(alg::NULL),
        &sel04(),
    );
    assert_ne!(parse_attestation(&r2).0, attest);
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    assert_ne!(
        parse_attestation(&quote(
            &mut t,
            k.handle,
            b"kpw",
            b"nonce",
            &u16b(alg::NULL),
            &sel04()
        ))
        .0,
        attest
    );
    // inScheme may repeat the key's scheme; the empty selection quotes SHA-256 of nothing.
    let r3 = quote(
        &mut t,
        k.handle,
        b"kpw",
        b"",
        &ecc_sig_scheme(),
        &hex("00000000"),
    );
    let (a3, _) = parse_attestation(&r3);
    assert!(a3.ends_with(&cat(&[&hex("00000000"), &tpm2b(&hex(SHA256_EMPTY))])));
}

pub(super) fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const QUOTE_X: &str = "ef7f114d6b36e22e25103e3ea3f22a7598ed7bf9baca96df4214ca22589601b4";
const QUOTE_Y: &str = "cb94108076eeef988e6d73698714edef104caf5576928bcf409c688ade13cf5c";
const QUOTE_R: &str = "98378dddb7c3cc2274927e49eb753cdcd4125309b2a5776138c62971bda2e8e6";
const QUOTE_S: &str = "04aceeb16951370efbbac748dc8dac2691b30850e4e755a74a7d56547dc52d45";
const QUOTE_ATTEST: &str = "ff54434780180022000b6619f848a29382a87230dd749a509f02a2b808317654843277c8a533b5c021bf00056e6f6e63650000000000000000000000010000000001000100000000000000000001000b031100000020b5ab2eaee749a8f5fe3e847815d70e8c15332cb6ab8a80491cfe7afc8dd7f8bc";

#[test]
fn attestations_of_storage_hierarchy_keys_are_obfuscated() {
    let mut t = started();
    let k = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &ecc_sign()));
    let qn = qualified(OWNER, &k.name);
    // obfuscation = KDFa(SHA-256, shProof, "OBFUSCATE", QN, Empty Buffer, 128), written out here.
    let sh_proof = t.proof(OWNER).unwrap();
    let data = cat(&[&u32b(1), b"OBFUSCATE", &[0], &qn, &u32b(128)]);
    let obf = crate::crypto::hmac_sha256(&sh_proof, &[&data]);
    let be64 = |b: &[u8]| u64::from_be_bytes(b.try_into().unwrap());
    let be32 = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
    let r = quote(&mut t, k.handle, b"kpw", b"n", &u16b(alg::NULL), &sel04());
    let (attest, _) = parse_attestation(&r);
    let expect = attest_bytes(
        ST_ATTEST_QUOTE,
        &qn,
        b"n",
        (0, 1 + be32(&obf[8..12]), be32(&obf[12..16])),
        FIRMWARE.wrapping_add(be64(&obf[0..8])),
        &cat(&[&sel04(), &tpm2b(&h256(&[&[0u8; 32], &[0u8; 32]]))]),
    );
    assert_eq!(attest, expect);
    // An endorsement key is not obfuscated.
    let e = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign()));
    let (ea, _) = parse_attestation(&quote(
        &mut t,
        e.handle,
        b"",
        b"n",
        &u16b(alg::NULL),
        &hex("00000000"),
    ));
    assert_eq!(
        ea,
        attest_bytes(
            ST_ATTEST_QUOTE,
            &qualified(ENDORSEMENT, &e.name),
            b"n",
            (0, 1, 0),
            FIRMWARE,
            &cat(&[&hex("00000000"), &tpm2b(&hex(SHA256_EMPTY))]),
        )
    );
}

#[test]
fn quote_errors() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let q = |t: &mut Tpm, k: u32, pw: &[u8], scheme: &[u8], sel: &[u8]| {
        quote(t, k, pw, b"n", scheme, sel)
    };
    // Authorization and handles.
    err(
        &q(&mut t, key, b"bad", &u16b(alg::NULL), &sel04()),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(
                cc::QUOTE,
                &[key],
                &cat(&[&tpm2b(b"n"), &u16b(alg::NULL), &sel04()]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &q(&mut t, 0x8000_0020, b"", &u16b(alg::NULL), &sel04()),
        Rc::reference_handle(0),
    );
    // A key that does not sign: the storage parent (TPM_RC_KEY), an HMAC key (TPM_RC_KEY).
    err(
        &q(&mut t, parent, b"parent-pw", &u16b(alg::NULL), &sel04()),
        Rc::KEY.in_handle(1),
    );
    let (hk, _, _) = make_child(&mut t, parent, &hmac_key(HMAC_SHA256, true), b"hk", b"");
    err(
        &q(&mut t, hk, b"hk", &u16b(alg::NULL), &sel04()),
        Rc::KEY.in_handle(1),
    );
    // x509sign keys do not sign TPM-generated structures.
    let mut x = ecc_sign();
    x.attributes |= oa::X509_SIGN;
    let (xk, _, _) = make_child(&mut t, parent, &x, b"xk", b"");
    err(
        &q(&mut t, xk, b"xk", &u16b(alg::NULL), &sel04()),
        Rc::ATTRIBUTES.in_handle(1),
    );
    // Schemes: an RSA scheme for an ECC key, an unknown scheme, and a key without a default scheme
    // (the generic `inScheme` is required, TPM_RC_SCHEME for NULL).
    err(
        &q(&mut t, key, b"kpw", &rsa_sig_scheme(), &sel04()),
        Rc::SCHEME.in_param(2),
    );
    err(
        &q(&mut t, key, b"kpw", &u16b(0x0015), &sel04()),
        Rc::SCHEME.in_param(2),
    );
    err(
        &q(
            &mut t,
            key,
            b"kpw",
            &cat(&[&u16b(alg::ECDSA), &u16b(0x0004)]),
            &sel04(),
        ),
        Rc::HASH.in_param(2),
    );
    let mut nul = ecc_sign();
    if let PublicParams::Ecc { scheme, .. } = &mut nul.params {
        *scheme = Scheme::NULL;
    }
    // (a signing key with a NULL scheme is not creatable as ECC: use a sign+decrypt key)
    nul.attributes |= oa::DECRYPT;
    let (nk, _, _) = make_child(&mut t, parent, &nul, b"nk", b"");
    err(
        &q(&mut t, nk, b"nk", &u16b(alg::NULL), &sel04()),
        Rc::SCHEME.in_param(2),
    );
    ok(&q(&mut t, nk, b"nk", &ecc_sig_scheme(), &sel04()));
    // Parameters: qualifyingData above TPM2B_DATA, trailing octets, a malformed selection.
    err(
        &quote(&mut t, key, b"kpw", &[1u8; 35], &u16b(alg::NULL), &sel04()),
        Rc::SIZE.in_param(1),
    );
    err(
        &q(&mut t, key, b"kpw", &u16b(alg::NULL), &hex("00000003")),
        Rc::SIZE.in_param(3),
    );
    let mut long = cat(&[&tpm2b(b"n"), &u16b(alg::NULL), &sel04()]);
    long.push(0);
    err(
        &run(&mut t, with_pw(cc::QUOTE, &[key], &[b"kpw"], &long)),
        Rc::SIZE,
    );
    // A restricted signing key signs attestations (and only those).
    let mut rs = ecc_sign();
    rs.attributes |= oa::RESTRICTED;
    let (rk, _, _) = make_child(&mut t, parent, &rs, b"rk", b"");
    ok(&q(&mut t, rk, b"rk", &u16b(alg::NULL), &sel04()));
    err(
        &sign(&mut t, rk, b"rk", &[7u8; 32], &ecc_sig_scheme()),
        Rc::TICKET.in_param(3),
    );
}

#[test]
fn certify_rsa_attestation_verifies_externally() {
    let mut t = started();
    let obj = parse_primary(&create_primary(&mut t, PLATFORM, b"opw", &ecc_sign()));
    let rsa_key = parse_primary(&create_primary(&mut t, PLATFORM, b"rpw", &rsa_sign()));
    let r = certify(
        &mut t,
        obj.handle,
        rsa_key.handle,
        [b"opw", b"rpw"],
        b"qd",
        &u16b(alg::NULL),
    );
    let (attest, sig) = parse_attestation(&r);
    let qn_obj = qualified(PLATFORM, &obj.name);
    let qn_signer = qualified(PLATFORM, &rsa_key.name);
    let attested = cat(&[&tpm2b(&obj.name), &tpm2b(&qn_obj)]);
    assert_eq!(
        attest,
        attest_bytes(
            ST_ATTEST_CERTIFY,
            &qn_signer,
            b"qd",
            (0, 1, 0),
            FIRMWARE,
            &attested
        )
    );
    // The signature: RSASSA-PKCS1-v1_5 over SHA-256(attest). The pinned modulus, attest and
    // signature were verified on 2026-11-07 with plain Python (pow(sig, 65537, n) compared with
    // the EMSA-PKCS1-v1_5 encoding of SHA-256(attest)).
    let mut rd = Reader::new(&sig);
    assert_eq!(rd.u16().unwrap(), alg::RSASSA);
    assert_eq!(rd.u16().unwrap(), alg::SHA256);
    let sigbytes = rd.tpm2b(256).unwrap().to_vec();
    rd.finish().unwrap();
    ok(&verify(&mut t, rsa_key.handle, &h256(&[&attest]), &sig));
    let Unique::Rsa(n) = unmarshal_public(&rsa_key.public).unique else {
        panic!()
    };
    assert_eq!(hexs(&n), CERTIFY_N);
    assert_eq!(hexs(&sigbytes), CERTIFY_SIG);
    assert_eq!(hexs(&attest), CERTIFY_ATTEST);
    // The same bytes with the RSA scheme named by the caller and with a wrong one.
    ok(&certify(
        &mut t,
        obj.handle,
        rsa_key.handle,
        [b"opw", b"rpw"],
        b"",
        &rsa_sig_scheme(),
    ));
    err(
        &certify(
            &mut t,
            obj.handle,
            rsa_key.handle,
            [b"opw", b"rpw"],
            b"",
            &ecc_sig_scheme(),
        ),
        Rc::SCHEME.in_param(2),
    );
    // Parameter errors and a missing second session.
    err(
        &certify(
            &mut t,
            obj.handle,
            rsa_key.handle,
            [b"opw", b"rpw"],
            &[1u8; 35],
            &u16b(alg::NULL),
        ),
        Rc::SIZE.in_param(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY,
                &[obj.handle, rsa_key.handle],
                &[b"opw"],
                &cat(&[&tpm2b(b""), &u16b(alg::NULL)]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &certify(
            &mut t,
            0x8000_0020,
            rsa_key.handle,
            [b"", b"rpw"],
            b"",
            &u16b(alg::NULL),
        ),
        Rc::reference_handle(0),
    );
    err(
        &certify(
            &mut t,
            obj.handle,
            0x8000_0020,
            [b"opw", b""],
            b"",
            &u16b(alg::NULL),
        ),
        Rc::reference_handle(1),
    );
    // The certified object may be any key; the signer must sign (the storage key does not).
    let (mut t2, parent) = with_ecc_parent();
    let (k, _, _) = make_child(&mut t2, parent, &ecc_sign(), b"k", b"");
    err(
        &certify(
            &mut t2,
            k,
            parent,
            [b"k", b"parent-pw"],
            b"",
            &u16b(alg::NULL),
        ),
        Rc::KEY.in_handle(2),
    );
}

const CERTIFY_N: &str = "b038f472bb201850d62c5ada54cbfa8f3f30fdde80e8cacf791336c85f82dbf353822e7d2f8232b6d989862532724f40b96d0fab017b575b0f525da782b034831d4567c0099badf4681136571827db44f30adc9eef869d35fb7933ad938612643ce9054dd4c1a969c374b9f014ab8d35dad2087a3e8d4469c7836b3b2909f2570481a85fff8313a9140d0c5e3ef10986cd6b7c8e138086d4221d903c0a56e22d68c96a08da5426ff6634a86b309d83692aec101fe2a08c2aa9de7e8af4cd9ea2c257a7dff598e90554f01d79d042ab1165870ab97d3ef175b6bd1dc2e8eff3b522cc1c01e52ca7aa6f6a32a56b3b01173e96c5f7422c223e345d64823c62bf7b";
const CERTIFY_SIG: &str = "3ec0ad1f0ec99ca280b30a5fde302d873246844e9e5b2c6a1ce5e7a3ad10e6276bddfb81892957c65e26dc23c723ebec57649d101e6e74e7d52dd87eb2051b8ea79932542d9d9f5316707071a0dcf70629bc8be30ff7518e9f98d5d56d9cbeb5e9df18703c2f8dde53064e16a793483716ee851883bdff6dd1aac5a61298979f97e8b93c05d68de4a07301606b110e07cc6bc2b474339dc1f8a8faf0f630555aa8220b9edd5c00c5385af66605a7b3e96887e703fd0c15e5392e9dae2d2669eb5bd3abe49404b0fccb3924a5b327eb1ab1f475f5d046ecf88a9f350944423b49ba7762f5ce3ad278d20f9ef2054479e3713cb674eea8cc8715ab657f75b1c891";
const CERTIFY_ATTEST: &str = "ff54434780170022000b02a4d0ae21957188d74b31d30296fef3248833ff888f696f30a6f404dc7b1e2700027164000000000000000000000001000000000100010000000000000022000b0fc39ce8add7b118a91cbb6c011aba1a808e967a442584fc66f3392ab0dc79a50022000b6619f848a29382a87230dd749a509f02a2b808317654843277c8a533b5c021bf";

// -------------------------------------------------------------------------------- contexts

fn save(t: &mut Tpm, h: u32) -> Vec<u8> {
    run(t, plain(cc::CONTEXT_SAVE, &[h], &[]))
}

fn load_ctx(t: &mut Tpm, ctx: &[u8]) -> Vec<u8> {
    run(t, plain(cc::CONTEXT_LOAD, &[], ctx))
}

fn loaded(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]])
}

/// `TPMS_CONTEXT` fields of a `TPM2_ContextSave` response: `(sequence, savedHandle, hierarchy)`.
fn ctx_fields(ctx: &[u8]) -> (u64, u32, u32) {
    (
        u64::from_be_bytes(ctx[..8].try_into().unwrap()),
        u32::from_be_bytes(ctx[8..12].try_into().unwrap()),
        u32::from_be_bytes(ctx[12..16].try_into().unwrap()),
    )
}

#[test]
fn an_object_context_round_trips_and_the_key_signs_the_same() {
    let (mut t, parent) = with_ecc_parent();
    let (key, name, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let digest = [9u8; 32];
    let before = sign(&mut t, key, b"kpw", &digest, &ecc_sig_scheme());
    ok(&before);
    let ctx = params(&save(&mut t, key), 0).to_vec();
    assert_eq!(ctx_fields(&ctx), (1, 0x8000_0000, OWNER));
    // The saved object stays loaded; flush it, then load the context.
    ok(&flush(&mut t, key));
    err(
        &sign(&mut t, key, b"kpw", &digest, &ecc_sig_scheme()),
        Rc::reference_handle(0),
    );
    let h2 = loaded(&load_ctx(&mut t, &ctx));
    assert_eq!(unmarshal_name(&mut t, h2), name);
    // ECDSA is deterministic (RFC 6979): the same key signs the same bytes.
    let after = sign(&mut t, h2, b"kpw", &digest, &ecc_sig_scheme());
    assert_eq!(params(&after, 0), params(&before, 0));
    // A context loads again, into another slot; both copies work.
    let h3 = loaded(&load_ctx(&mut t, &ctx));
    assert_ne!(h2, h3);
    ok(&sign(&mut t, h3, b"kpw", &digest, &ecc_sig_scheme()));
    // The sequence counts the saves.
    let ctx2 = params(&save(&mut t, h2), 0).to_vec();
    assert_eq!(ctx_fields(&ctx2).0, 2);
    // Neither the private scalar nor the authValue is in the clear in the context.
    assert!(!contains(&ctx, &secret_of(&t, h2)));
    assert!(!contains(&ctx, b"kpw"));
    // A storage parent keeps its seed: a child created under the reloaded parent loads under the
    // original parent as well.
    let pctx = params(&save(&mut t, parent), 0).to_vec();
    let p2 = loaded(&load_ctx(&mut t, &pctx));
    let c = parse_child(&create_child(
        &mut t,
        p2,
        b"parent-pw",
        b"x",
        b"",
        &ecc_sign(),
    ));
    ok(&load(&mut t, parent, b"parent-pw", &c.private, &c.public));
    // A public-only (external) object context holds no private part, and still verifies.
    let (kk, _, public) = make_child(&mut t, parent, &ecc_sign(), b"e", b"");
    let sig = params(&sign(&mut t, kk, b"e", &digest, &ecc_sig_scheme()), 0).to_vec();
    let (ext, _) = parse_loaded(&load_external(&mut t, &unmarshal_public(&public), NULL));
    let ectx = params(&save(&mut t, ext), 0).to_vec();
    assert_eq!(ctx_fields(&ectx).2, NULL);
    let e2 = loaded(&load_ctx(&mut t, &ectx));
    ok(&verify(&mut t, e2, &digest, &sig));
    err(&verify(&mut t, e2, &[1u8; 32], &sig), Rc::SIGNATURE);
}

#[test]
fn damaged_and_foreign_contexts_are_refused() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let ctx = params(&save(&mut t, key), 0).to_vec();
    ok(&flush(&mut t, key));
    // Every flipped bit of the header and of the blob is an error, never a loaded object.
    for at in 0..ctx.len() {
        for bit in [0x01u8, 0x80] {
            let mut bad = ctx.clone();
            bad[at] ^= bit;
            let r = load_ctx(&mut t, &bad);
            assert_ne!(rc(&r), 0, "bit {bit:#x} of octet {at} was accepted");
        }
    }
    assert!(
        t.objects.loaded_handles().len() == 1,
        "only the parent is loaded"
    );
    // The codes: a bigger sequence is TPM_RC_VALUE, a smaller one and any other field change the
    // integrity value; a damaged blob is TPM_RC_INTEGRITY; a cut blob a size error.
    let mut b = ctx.clone();
    b[0] = 0x80;
    err(&load_ctx(&mut t, &b), Rc::VALUE.in_param(1));
    let mut b = ctx.clone();
    b[7] = 0;
    err(&load_ctx(&mut t, &b), Rc::INTEGRITY.in_param(1));
    let mut b = ctx.clone();
    b[11] = 2;
    err(&load_ctx(&mut t, &b), Rc::INTEGRITY.in_param(1));
    let mut b = ctx.clone();
    b[12..16].copy_from_slice(&PLATFORM.to_be_bytes());
    err(&load_ctx(&mut t, &b), Rc::INTEGRITY.in_param(1));
    let mut b = ctx.clone();
    let last = b.len() - 1;
    b[last] ^= 1;
    err(&load_ctx(&mut t, &b), Rc::INTEGRITY.in_param(1));
    err(
        &load_ctx(&mut t, &ctx[..ctx.len() - 1]),
        Rc::INSUFFICIENT.in_param(1),
    );
    let mut long = ctx.clone();
    long.push(0);
    err(&load_ctx(&mut t, &long), Rc::SIZE);
    err(&load_ctx(&mut t, &ctx[..10]), Rc::INSUFFICIENT.in_param(1));
    // A blob that is only an integrity value.
    let mut tiny = ctx[..16].to_vec();
    tiny.extend_from_slice(&tpm2b(&tpm2b(&[0u8; 32])));
    err(&load_ctx(&mut t, &tiny), Rc::SIZE.in_param(1));
    // A saved handle that names nothing loadable, and a hierarchy that is none.
    for h in [0x8000_0001u32, 0x8100_0000, 0x0100_0000, 0x4000_0001, 0] {
        let mut b = ctx.clone();
        b[8..12].copy_from_slice(&h.to_be_bytes());
        err(&load_ctx(&mut t, &b), Rc::VALUE.in_param(1));
    }
    let mut b = ctx.clone();
    b[12..16].copy_from_slice(&handle::RH_LOCKOUT.to_be_bytes());
    err(&load_ctx(&mut t, &b), Rc::VALUE.in_param(1));
    // The context is still good.
    ok(&load_ctx(&mut t, &ctx));
}

#[test]
fn object_context_encryption_changes_after_reset_and_keeps_saved_restart_contexts() {
    use crate::object::sensitive_to_bytes;

    let mut t = started();
    let create = |t: &mut Tpm| parse_primary(&create_primary(t, OWNER, b"", &ecc_sign())).handle;
    let sensitive = |t: &Tpm, handle| {
        let o = t.objects.get(handle).unwrap();
        sensitive_to_bytes(o.sensitive.as_ref().unwrap(), o.public.type_alg()).unwrap()
    };
    let ciphertext = |ctx: &[u8]| {
        let mut context = Reader::new(&ctx[16..]);
        let mut blob = Reader::new(context.tpm2b(4096).unwrap());
        assert_eq!(blob.tpm2b(32).unwrap().len(), 32);
        blob.bytes(blob.remaining()).unwrap().to_vec()
    };
    let object = create(&mut t);
    let original_sensitive = sensitive(&t, object);
    let original_public = params(&read_public(&mut t, object), 0).to_vec();
    let original = params(&save(&mut t, object), 0).to_vec();
    for startup_type in [su::CLEAR, su::STATE] {
        shutdown(&mut t, su::STATE);
        t.init();
        startup(&mut t, startup_type);
        let loaded = loaded(&load_ctx(&mut t, &original));
        assert_eq!(params(&read_public(&mut t, loaded), 0), original_public);
    }

    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    let recreated = create(&mut t);
    let new = params(&save(&mut t, recreated), 0).to_vec();
    assert_eq!(ctx_fields(&new), ctx_fields(&original));
    assert_eq!(params(&read_public(&mut t, recreated), 0), original_public);
    assert!(
        sensitive(&t, recreated) == original_sensitive,
        "the same sensitive body"
    );
    assert_ne!(ciphertext(&new), ciphertext(&original));
    err(&load_ctx(&mut t, &original), Rc::INTEGRITY.in_param(1));
    ok(&load_ctx(&mut t, &new));
}

#[test]
fn a_context_dies_with_its_hierarchy_seed_a_reset_and_a_disabled_hierarchy() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let ctx = params(&save(&mut t, key), 0).to_vec();
    // A disabled hierarchy: TPM_RC_HIERARCHY (parameter 1); enabling it again helps.
    ok(&pw_hier_off(&mut t));
    err(&load_ctx(&mut t, &ctx), Rc::HIERARCHY.in_param(1));
    ok(&run(
        &mut t,
        with_pw(
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            &[b""],
            &cat(&[&u32b(OWNER), &[1]]),
        ),
    ));
    ok(&load_ctx(&mut t, &ctx));
    // Restart (Shutdown(STATE), Startup(CLEAR)) and Resume keep it.
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::CLEAR);
    ok(&load_ctx(&mut t, &ctx));
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::STATE);
    ok(&load_ctx(&mut t, &ctx));
    // A change of the storage seed (TPM2_Clear) invalidates it.
    ok(&run(&mut t, with_pw(cc::CLEAR, &[PLATFORM], &[b""], &[])));
    err(&load_ctx(&mut t, &ctx), Rc::INTEGRITY.in_param(1));
    // So does a TPM Reset, for objects of every hierarchy including Null.
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let owner_ctx = params(&save(&mut t, key), 0).to_vec();
    let nul = parse_primary(&create_primary(&mut t, NULL, b"", &ecc_sign()));
    let null_ctx = params(&save(&mut t, nul.handle), 0).to_vec();
    assert_eq!(ctx_fields(&null_ctx).2, NULL);
    ok(&load_ctx(&mut t, &null_ctx));
    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    // The counters start over: an old context is beyond them (TPM_RC_VALUE) until new saves
    // catch up, and then its integrity value does not match (a new resetValue and nullProof).
    err(&load_ctx(&mut t, &owner_ctx), Rc::VALUE.in_param(1));
    let fresh = parse_primary(&create_primary(&mut t, NULL, b"", &ecc_sign()));
    ok(&save(&mut t, fresh.handle));
    ok(&save(&mut t, fresh.handle));
    err(&load_ctx(&mut t, &owner_ctx), Rc::INTEGRITY.in_param(1));
    err(&load_ctx(&mut t, &null_ctx), Rc::INTEGRITY.in_param(1));
    // An object with stClear does not survive a Restart.
    let mut t = started();
    let mut st = ecc_sign();
    st.attributes |= oa::ST_CLEAR;
    let sk = parse_primary(&create_primary(&mut t, OWNER, b"", &st));
    let plain_k = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    let sctx = params(&save(&mut t, sk.handle), 0).to_vec();
    let pctx = params(&save(&mut t, plain_k.handle), 0).to_vec();
    assert_eq!(ctx_fields(&sctx).1, 0x8000_0002);
    ok(&load_ctx(&mut t, &sctx));
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::CLEAR);
    err(&load_ctx(&mut t, &sctx), Rc::INTEGRITY.in_param(1));
    ok(&load_ctx(&mut t, &pctx));
    // It survives a Resume (`Startup(STATE)` does not change clearCount), twice in a row.
    let mut t = started();
    let sk = parse_primary(&create_primary(&mut t, OWNER, b"", &st));
    let sctx = params(&save(&mut t, sk.handle), 0).to_vec();
    for _ in 0..2 {
        shutdown(&mut t, su::STATE);
        t.init();
        startup(&mut t, su::STATE);
        ok(&load_ctx(&mut t, &sctx));
    }
    // ... and a Restart after those Resumes still invalidates it.
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::CLEAR);
    err(&load_ctx(&mut t, &sctx), Rc::INTEGRITY.in_param(1));
}

#[test]
fn saved_pcr_assertions_are_stale_after_counted_startup_resets() {
    for startup_type in [su::CLEAR, su::STATE] {
        let mut t = started();
        ok(&extend(&mut t, 0, &[1u8; 32]));
        let policy = policy_session(&mut t, se::POLICY);
        ok(&pol_pcr(&mut t, &policy, &[], &sel04()));
        let previous_digest = digest(&mut t, &policy);
        let context = params(&save(&mut t, policy.handle), 0).to_vec();
        shutdown(&mut t, su::STATE);
        t.init();
        startup(&mut t, startup_type);
        assert_eq!(loaded(&load_ctx(&mut t, &context)), policy.handle);
        assert_eq!(digest(&mut t, &policy), previous_digest);

        err(&pol_pcr(&mut t, &policy, &[], &sel04()), Rc::PCR_CHANGED);
        let slot = t.sessions.slot_of(policy.handle).unwrap();
        assert_eq!(
            t.policy_use_check(
                slot,
                (cc::SIGN, NULL),
                |rule| rule == PolicyHashRestriction::CpHash([0; 32]),
                &previous_digest,
                false,
                1
            ),
            Err(Rc::PCR_CHANGED)
        );
        ok(&pol(&mut t, cc::POLICY_RESTART, &policy, &[]));
        ok(&pol_pcr(&mut t, &policy, &[], &sel04()));
    }
}

#[test]
fn session_contexts_save_once_and_load_once() {
    let mut t = started();
    let mut p = policy_session(&mut t, se::POLICY);
    ok(&pol_cc(&mut t, &p, cc::SIGN));
    let d = digest(&mut t, &p);
    let before = handles_listed(&mut t, 0x0200_0000);
    assert_eq!(before, vec![p.handle]);
    let ctx = params(&save(&mut t, p.handle), 0).to_vec();
    assert_eq!(ctx_fields(&ctx), (1, p.handle, NULL));
    // Saved: no longer loaded, listed as saved (type 0x03, reported with the HMAC type), counted
    // as active, and its slot stays reserved for it.
    err(
        &run(&mut t, plain(cc::POLICY_GET_DIGEST, &[p.handle], &[])),
        Rc::reference_handle(0),
    );
    assert!(handles_listed(&mut t, 0x0200_0000).is_empty());
    assert_eq!(
        handles_listed(&mut t, 0x0300_0000),
        vec![0x0200_0000 | (p.handle & 0xFF)]
    );
    let other = policy_session(&mut t, se::POLICY);
    assert_ne!(
        other.handle & 0xFF,
        p.handle & 0xFF,
        "the saved slot is reserved"
    );
    let prop = |t: &mut Tpm, tag: u32| {
        let r = run(
            t,
            plain(
                cc::GET_CAPABILITY,
                &[],
                &cat(&[&u32b(cap::TPM_PROPERTIES), &u32b(tag), &u32b(1)]),
            ),
        );
        let b = params(&r, 0);
        u32::from_be_bytes(b[b.len() - 4..].try_into().unwrap())
    };
    assert_eq!(prop(&mut t, pt::HR_ACTIVE), 2);
    assert_eq!(prop(&mut t, pt::HR_LOADED), 1);
    // The load returns the same handle and the same state; the saved record is used up.
    assert_eq!(loaded(&load_ctx(&mut t, &ctx)), p.handle);
    assert_eq!(digest(&mut t, &p), d);
    err(&load_ctx(&mut t, &ctx), Rc::HANDLE.in_param(1));
    assert_eq!(prop(&mut t, pt::HR_ACTIVE), 2);
    // The reloaded session still authorizes (its nonce travelled with it).
    let (mut t2, parent) = with_ecc_parent();
    let mut s = policy_session(&mut t2, se::POLICY);
    let (key, name, _) = make_child(&mut t2, parent, &policy_key(&hex(V_CC_SIGN)), b"kpw", b"");
    ok(&pol_cc(&mut t2, &s, cc::SIGN));
    let c = sign_command(key, &name, b"kpw");
    ok(&use_session(&mut t2, &mut s, &c, 0, true, false));
    let sctx = params(&save(&mut t2, s.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t2, &sctx)), s.handle);
    assert_eq!(digest(&mut t2, &s), [0; 32]);
    ok(&pol_cc(&mut t2, &s, cc::SIGN));
    ok(&use_session(&mut t2, &mut s, &c, 0, true, false));
    // A newer save supersedes an older context of the same session (replay protection).
    let old = params(&save(&mut t2, s.handle), 0).to_vec();
    assert_eq!(loaded(&load_ctx(&mut t2, &old)), s.handle);
    let newer = params(&save(&mut t2, s.handle), 0).to_vec();
    err(&load_ctx(&mut t2, &old), Rc::HANDLE.in_param(1));
    assert_eq!(loaded(&load_ctx(&mut t2, &newer)), s.handle);
    // FlushContext forgets a saved session; the context is useless afterwards.
    let again = params(&save(&mut t2, s.handle), 0).to_vec();
    ok(&flush(&mut t2, s.handle));
    err(&load_ctx(&mut t2, &again), Rc::HANDLE.in_param(1));
    err(&flush(&mut t2, s.handle), Rc::HANDLE.in_param(1));
    // An HMAC session bound to the owner keeps its key and binding.
    ok(&set_auth(&mut t2, OWNER, b"", b"owner-pw"));
    let mut h = start_hmac(&mut t2, OWNER, b"owner-pw", &[0x66; 16]);
    let hctx = params(&save(&mut t2, h.handle), 0).to_vec();
    assert_eq!(h.handle >> 24, 2);
    assert_eq!(loaded(&load_ctx(&mut t2, &hctx)), h.handle);
    let c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"owner-pw"),
        (OWNER, b"owner-pw"),
    );
    ok(&super::tests_b::run_hmac(&mut t2, &mut h, &c));
    // The context of a session cannot be claimed for another handle or hierarchy.
    let sctx2 = params(&save(&mut t2, h.handle), 0).to_vec();
    let mut b = sctx2.clone();
    b[11] ^= 1;
    err(&load_ctx(&mut t2, &b), Rc::HANDLE.in_param(1));
    let mut b = sctx2.clone();
    b[12..16].copy_from_slice(&OWNER.to_be_bytes());
    err(&load_ctx(&mut t2, &b), Rc::VALUE.in_param(1));
    // Saved sessions survive Restart and Resume, and a Reset ends them.
    shutdown(&mut t2, su::STATE);
    t2.init();
    startup(&mut t2, su::STATE);
    assert_eq!(loaded(&load_ctx(&mut t2, &sctx2)), h.handle);
    let again = params(&save(&mut t2, h.handle), 0).to_vec();
    shutdown(&mut t2, su::STATE);
    t2.init();
    startup(&mut t2, su::CLEAR);
    assert_eq!(handles_listed(&mut t2, 0x0300_0000).len(), 1);
    assert_eq!(loaded(&load_ctx(&mut t2, &again)), h.handle);
    let last = params(&save(&mut t2, h.handle), 0).to_vec();
    shutdown(&mut t2, su::CLEAR);
    t2.init();
    startup(&mut t2, su::CLEAR);
    assert!(handles_listed(&mut t2, 0x0300_0000).is_empty());
    err(&load_ctx(&mut t2, &last), Rc::HANDLE.in_param(1));
    p.nonce_tpm.clear();
}

#[test]
fn context_commands_check_their_handles_and_take_no_session() {
    let (mut t, parent) = with_ecc_parent();
    // What cannot be saved: persistent objects, NV indices, permanent handles, PCR, unloaded
    // objects and sessions, sequence objects.
    let seq = {
        let r = run(
            &mut t,
            plain(
                cc::HASH_SEQUENCE_START,
                &[],
                &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
            ),
        );
        ok(&r);
        u32::from_be_bytes(r[10..14].try_into().unwrap())
    };
    for h in [0x8100_0000u32, NV0_FOR_CTX, OWNER, 0, 0x0000_0001, seq] {
        err(&save(&mut t, h), Rc::VALUE.in_handle(1));
    }
    err(&save(&mut t, 0x8000_0020), Rc::reference_handle(0));
    err(&save(&mut t, 0x0200_0010), Rc::HANDLE.in_handle(1));
    err(&save(&mut t, 0x0300_0010), Rc::HANDLE.in_handle(1));
    // Parameters, and sessions (TPM_RC_AUTH_CONTEXT, like FlushContext).
    err(
        &run(&mut t, plain(cc::CONTEXT_SAVE, &[parent], &[0])),
        Rc::SIZE,
    );
    err(
        &run(&mut t, with_pw(cc::CONTEXT_SAVE, &[parent], &[b""], &[])),
        Rc::AUTH_CONTEXT,
    );
    err(
        &run(&mut t, with_pw(cc::CONTEXT_LOAD, &[], &[b""], &[0; 20])),
        Rc::AUTH_CONTEXT,
    );
    err(&load_ctx(&mut t, &[]), Rc::INSUFFICIENT.in_param(1));
    // No free object slot: TPM_RC_OBJECT_MEMORY (a warning), and nothing changes.
    let ctx = params(&save(&mut t, parent), 0).to_vec();
    while t.objects.free_slots() > 0 {
        let c = parse_child(&create_child(
            &mut t,
            parent,
            b"parent-pw",
            b"",
            b"",
            &ecc_sign(),
        ));
        ok(&load(&mut t, parent, b"parent-pw", &c.private, &c.public));
    }
    err(&load_ctx(&mut t, &ctx), Rc::OBJECT_MEMORY);
    ok(&flush(&mut t, FIRST_OBJECT + 1));
    ok(&load_ctx(&mut t, &ctx));
}

const NV0_FOR_CTX: u32 = 0x0100_0000;

// ------------------------------------------------------------------------ robustness

#[test]
fn malformed_slice_c2_commands_never_panic() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let ps = policy_session(&mut t, se::POLICY);
    let codes = [
        cc::CERTIFY,
        cc::POLICY_SECRET,
        cc::HMAC,
        cc::QUOTE,
        cc::CONTEXT_LOAD,
        cc::CONTEXT_SAVE,
        cc::POLICY_AUTH_VALUE,
        cc::POLICY_PASSWORD,
        cc::POLICY_COMMAND_CODE,
        cc::POLICY_CP_HASH,
        cc::POLICY_LOCALITY,
        cc::POLICY_OR,
        cc::POLICY_PCR,
        cc::POLICY_RESTART,
        cc::POLICY_GET_DIGEST,
    ];
    let mut x = 0x1234_5678u32;
    let mut next = || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 16) as u8
    };
    for code in codes {
        for round in 0..300 {
            let n = usize::from(next()) % 40;
            let body: Vec<u8> = (0..n).map(|_| next()).collect();
            let handles: Vec<u32> = match round % 4 {
                0 => vec![key, key],
                1 => vec![ps.handle, ps.handle],
                2 => vec![OWNER, ps.handle],
                _ => vec![parent, key],
            };
            let tag = if round % 2 == 0 {
                st::NO_SESSIONS
            } else {
                st::SESSIONS
            };
            let cmd = frame(
                tag,
                code,
                &handles,
                &if tag == st::SESSIONS {
                    auth_area(&[b"kpw"])
                } else {
                    vec![]
                },
                &body,
            );
            let r = run(&mut t, cmd);
            assert!(rc(&r) != 0 || r.len() >= 10);
        }
    }
}

#[test]
fn a_persistent_hmac_key_survives_the_snapshot_and_a_persistent_policy_key_still_authorizes() {
    let (mut t, parent) = with_ecc_parent();
    let (hk, _, _) = make_child(
        &mut t,
        parent,
        &hmac_key(HMAC_SHA256, false),
        b"hk",
        b"snapshot-key",
    );
    let before = hmac_out(&hmac_cmd(&mut t, hk, b"hk", b"data", alg::NULL));
    ok(&evict(&mut t, OWNER, b"", hk, 0x8100_0001));
    let (pk, pname, _) = make_child(&mut t, parent, &policy_key(&hex(V_CC_SIGN)), b"kpw", b"");
    ok(&evict(&mut t, OWNER, b"", pk, 0x8100_0002));
    let snapshot = t.persistent_snapshot();
    let mut r = Tpm::restore(&snapshot, &[9u8; 48]).unwrap();
    startup(&mut r, su::CLEAR);
    assert_eq!(
        hmac_out(&hmac_cmd(&mut r, 0x8100_0001, b"hk", b"data", alg::NULL)),
        before
    );
    // The public area keeps the HMAC scheme and the authPolicy; the policy session authorizes the
    // restored key.
    let mut s = policy_session(&mut r, se::POLICY);
    ok(&pol_cc(&mut r, &s, cc::SIGN));
    let c = sign_command(0x8100_0002, &pname, b"kpw");
    ok(&use_session(&mut r, &mut s, &c, 0, true, false));
    // Sessions and saved contexts are volatile: the snapshot holds none.
    assert_eq!(r.sessions.loaded(), 1);
    assert!(handles_listed(&mut r, 0x0300_0000).is_empty());
}
