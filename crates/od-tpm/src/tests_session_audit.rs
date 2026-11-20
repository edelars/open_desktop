//! Raw TPM commands and an independent digest oracle for session audit.

use super::tests_b::{
    Hc, Hs, LOCKOUT, check_hmac_ack, hmac_auth_entry, set_auth, split_response, start_hmac,
    start_session,
};
use super::tests_c2::{FIRMWARE, attest_bytes, h256, parse_attestation, qualified};
use super::tests_obj::{create_primary, ecc_sign, parse_primary, rsa_sign, verify};
use super::*;
use crate::codes::{se, session_attr};

const AUDIT: u8 = session_attr::AUDIT | session_attr::CONTINUE_SESSION;
const EXCLUSIVE: u8 = session_attr::AUDIT_EXCLUSIVE;
const RESET: u8 = session_attr::AUDIT_RESET;
const ENDORSEMENT: u32 = handle::RH_ENDORSEMENT;
const NULL: u32 = handle::RH_NULL;
const OWNER: u32 = handle::RH_OWNER;
const PLATFORM: u32 = handle::RH_PLATFORM;

fn command(code: u32, handles: &[u32], params: &[u8], attrs: u8) -> Hc {
    Hc {
        code,
        handles: handles.to_vec(),
        names: handles.iter().map(|h| h.to_be_bytes().to_vec()).collect(),
        params: params.to_vec(),
        entity: (NULL, Vec::new()),
        attrs,
        nonce_caller: vec![0x33; 16],
        entity_after: Vec::new(),
    }
}

fn packet(c: &Hc, passwords: &[&[u8]], audit_entries: &[(&Hs, bool)]) -> Vec<u8> {
    let mut entries = auth_area(passwords).split_off(4);
    for (s, corrupt) in audit_entries {
        entries.extend_from_slice(&hmac_auth_entry(s, c, &[], false, *corrupt));
    }
    let auth = cat(&[&(entries.len() as u32).to_be_bytes(), &entries]);
    frame(st::SESSIONS, c.code, &c.handles, &auth, &c.params)
}

fn audited(
    tpm: &mut Tpm,
    mut c: Hc,
    passwords: &[&[u8]],
    session: &mut Hs,
    expected_attrs: u8,
) -> Vec<u8> {
    let response = run(tpm, packet(&c, passwords, &[(session, false)]));
    let (params, acks) = split_response(&response, 0);
    assert_eq!(acks.len(), passwords.len() + 1);
    c.attrs = expected_attrs;
    check_hmac_ack(session, &c, &params, &acks[passwords.len()]);
    params
}

fn parms(attrs: u8) -> Hc {
    command(cc::TEST_PARMS, &[], &hex("002300100018000b00030010"), attrs)
}

fn retrieve(session: u32, attrs: u8) -> Hc {
    command(
        cc::GET_SESSION_AUDIT_DIGEST,
        &[ENDORSEMENT, NULL, session],
        &hex("00000010"),
        attrs,
    )
}

fn first_digest() -> Vec<u8> {
    // SHA256(zero digest || SHA256(0000018A || ECC parameters) || SHA256(00000000 || 0000018A)),
    // computed with Python hashlib outside the TPM implementation.
    hex("37a3281b59e7ff115c040c2de077a09fbe913ecae71ae1defec47a4d1b8ecfb3")
}

fn check_attestation(params: &[u8], exclusive: bool, digest: &[u8]) {
    let mut result = Reader::new(params);
    let attest = result.tpm2b(1024).unwrap();
    assert_eq!(result.u16().unwrap(), 0x0010);
    result.finish().unwrap();
    let mut r = Reader::new(attest);
    assert_eq!(r.u32().unwrap(), 0xff54_4347);
    assert_eq!(r.u16().unwrap(), 0x8016);
    assert_eq!(r.tpm2b(68).unwrap(), NULL.to_be_bytes());
    assert!(r.tpm2b(64).unwrap().is_empty());
    r.bytes(17 + 8).unwrap(); // TPMS_CLOCK_INFO and firmwareVersion
    assert_eq!(r.u8().unwrap(), u8::from(exclusive));
    assert_eq!(r.tpm2b(64).unwrap(), digest);
    r.finish().unwrap();
}

#[test]
fn session_audit_context_roundtrip_and_self_attestation_commit_after_response() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"bound"));
    let mut s = start_hmac(&mut t, OWNER, b"bound", &[0x22; 16]);
    err(
        &run(
            &mut t,
            packet(&parms(AUDIT | EXCLUSIVE), &[], &[(&s, false)]),
        ),
        Rc::EXCLUSIVE,
    );
    assert!(audited(&mut t, parms(AUDIT), &[], &mut s, AUDIT | EXCLUSIVE).is_empty());
    let slot = t.sessions.slot_of(s.handle).unwrap();
    assert!(t.sessions.get(slot).unwrap().bound.is_none());
    s.bound = None;

    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[s.handle], &[]));
    ok(&saved);
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &saved[10..]));
    ok(&loaded);
    assert_eq!(loaded[10..], s.handle.to_be_bytes());

    let passwords: &[&[u8]] = &[b"", b""];
    let mut wrong_name = retrieve(s.handle, AUDIT | EXCLUSIVE);
    wrong_name.names.pop();
    err(
        &run(&mut t, packet(&wrong_name, passwords, &[(&s, false)])),
        Rc::BAD_AUTH.in_session(3),
    );
    let c = retrieve(s.handle, AUDIT | EXCLUSIVE);
    let response = audited(&mut t, c, passwords, &mut s, AUDIT | EXCLUSIVE);
    check_attestation(&response, true, &first_digest());

    let c = retrieve(s.handle, AUDIT | EXCLUSIVE);
    let cp = h256(&[&c.code.to_be_bytes(), &c.names.concat(), &c.params]);
    let rp = h256(&[&0u32.to_be_bytes(), &c.code.to_be_bytes(), &response]);
    let dig = h256(&[&first_digest(), &cp, &rp]);
    let second = audited(&mut t, c, passwords, &mut s, AUDIT | EXCLUSIVE);
    check_attestation(&second, true, &dig);
}

#[test]
fn lockout_bind_remains_protected_after_audit_conversion_and_context_save() {
    let mut t = started();
    ok(&set_auth(&mut t, LOCKOUT, b"", b"secret"));
    let mut auditor = start_hmac(&mut t, LOCKOUT, b"secret", &[0x45; 16]);
    audited(&mut t, parms(AUDIT), &[], &mut auditor, AUDIT | EXCLUSIVE);
    let slot = t.sessions.slot_of(auditor.handle).unwrap();
    assert!(t.sessions.get(slot).unwrap().bound.is_none());
    let response = run(&mut t, plain(cc::CONTEXT_SAVE, &[auditor.handle], &[]));
    let context = params(&response, 0).to_vec();
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &context));
    ok(&loaded);
    assert_eq!(&loaded[10..14], &auditor.handle.to_be_bytes());

    // The issued session retains the key derived before a hierarchy auth change.
    ok(&set_auth(&mut t, LOCKOUT, b"secret", b"rotated"));
    audited(&mut t, parms(AUDIT), &[], &mut auditor, AUDIT);
    let tries = t.persistent.da.failed_tries;
    err(
        &run(&mut t, packet(&parms(AUDIT), &[], &[(&auditor, true)])),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert!(!t.persistent.da.lockout_auth_enabled);
    assert_eq!(t.persistent.da.failed_tries, tries);
    err(
        &run(&mut t, packet(&parms(AUDIT), &[], &[(&auditor, false)])),
        Rc::LOCKOUT,
    );
    let mut exempt = start_hmac(&mut t, OWNER, b"", &[0x46; 16]);
    audited(&mut t, parms(AUDIT), &[], &mut exempt, AUDIT | EXCLUSIVE);
}

#[test]
fn session_audit_errors_leave_nonce_digest_and_exclusivity_unchanged() {
    let mut t = started();
    let mut s = start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let other = start_hmac(&mut t, NULL, b"", &[0x45; 16]);
    let policy = start_session(&mut t, se::POLICY, NULL, b"", &[0x46; 16]);
    let before = retrieve(s.handle, 0);
    err(
        &run(
            &mut t,
            with_pw(before.code, &before.handles, &[b"", b""], &before.params),
        ),
        Rc::TYPE.in_handle(3),
    );
    err(
        &run(
            &mut t,
            packet(&parms(AUDIT), &[], &[(&s, false), (&other, false)]),
        ),
        Rc::ATTRIBUTES.in_session(2),
    );
    err(
        &run(&mut t, packet(&parms(AUDIT), &[], &[(&policy, false)])),
        Rc::ATTRIBUTES.in_session(1),
    );
    audited(&mut t, parms(AUDIT), &[], &mut s, AUDIT | EXCLUSIVE);
    err(
        &run(&mut t, packet(&parms(AUDIT | RESET), &[], &[(&s, true)])),
        Rc::BAD_AUTH.in_session(1),
    );
    let broken = command(cc::READ_CLOCK, &[], &[0x01], AUDIT | EXCLUSIVE | RESET);
    assert_ne!(rc(&run(&mut t, packet(&broken, &[], &[(&s, false)]))), 0);
    let c = retrieve(s.handle, AUDIT | EXCLUSIVE);
    let r = audited(&mut t, c, &[b"", b""], &mut s, AUDIT | EXCLUSIVE);
    check_attestation(&r, true, &first_digest());

    ok(&run(&mut t, plain(cc::READ_CLOCK, &[], &[])));
    err(
        &run(
            &mut t,
            packet(&parms(AUDIT | EXCLUSIVE), &[], &[(&s, false)]),
        ),
        Rc::EXCLUSIVE,
    );
    audited(&mut t, parms(AUDIT | RESET), &[], &mut s, AUDIT | EXCLUSIVE);
    let c = retrieve(s.handle, AUDIT | EXCLUSIVE);
    let result = audited(&mut t, c, &[b"", b""], &mut s, AUDIT | EXCLUSIVE);
    check_attestation(&result, true, &first_digest());

    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[s.handle], &[]));
    ok(&saved);
    ok(&run(
        &mut t,
        plain(cc::FLUSH_CONTEXT, &[], &s.handle.to_be_bytes()),
    ));
    let replacement = start_hmac(&mut t, NULL, b"", &[0x47; 16]);
    assert_eq!(replacement.handle, s.handle);
    err(
        &run(
            &mut t,
            packet(&parms(AUDIT | EXCLUSIVE), &[], &[(&replacement, false)]),
        ),
        Rc::EXCLUSIVE,
    );
}

#[test]
fn session_audit_attestation_signs_with_ecc_and_rsa() {
    let mut t = started();
    let ecc = parse_primary(&create_primary(&mut t, PLATFORM, b"ek", &ecc_sign()));
    let rsa = parse_primary(&create_primary(&mut t, PLATFORM, b"rk", &rsa_sign()));
    let mut session = start_hmac(&mut t, NULL, b"", &[0x55; 16]);
    for (key, pw) in [(&ecc, b"ek"), (&rsa, b"rk")] {
        audited(
            &mut t,
            parms(AUDIT | RESET),
            &[],
            &mut session,
            AUDIT | EXCLUSIVE,
        );
        let params = cat(&[&tpm2b(b"audit nonce"), &0x0010u16.to_be_bytes()]);
        let handles = [ENDORSEMENT, key.handle, session.handle];
        let command_code = cc::GET_SESSION_AUDIT_DIGEST;
        err(
            &run(
                &mut t,
                with_pw(command_code, &handles, &[b"", b"bad"], &params),
            ),
            Rc::BAD_AUTH.in_session(2),
        );
        let reply = run(&mut t, with_pw(command_code, &handles, &[b"", pw], &params));
        let (attestation, signature) = parse_attestation(&reply);
        let union = cat(&[&[1], &tpm2b(&first_digest())]);
        assert_eq!(
            attestation,
            attest_bytes(
                0x8016,
                &qualified(PLATFORM, &key.name),
                b"audit nonce",
                (0, 1, 0),
                FIRMWARE,
                &union
            )
        );
        ok(&verify(
            &mut t,
            key.handle,
            &h256(&[&attestation]),
            &signature,
        ));
    }
}
