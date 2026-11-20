//! Command-level tests of slice C3 of phase B (S7-23): NV counters, bit fields and extend
//! indices (`TPM2_NV_Increment`, `TPM2_NV_SetBits`, `TPM2_NV_Extend`), `TPM2_GetTime`,
//! `signHandle` = `TPM_RH_NULL` and session-based parameter encryption (AES-128-CFB and XOR).
//! Every command goes through `Tpm::execute` with raw bytes.
//!
//! Independent evidence (computed 2026-11-07 outside the crate): the `NV_Extend` digests with
//! Python `hashlib` (`SHA-256(old || data)`), the AES-128-CFB and XOR vectors of `paramenc.rs`
//! with Python `hmac` and `openssl enc -aes-128-cfb`, the `GetTime` signature with plain Python
//! P-256 arithmetic (an affine-coordinate ECDSA verification of the pinned key, attest and
//! signature). The parameter-encryption client below is written from Part 1 v184, clause 19.

use sha2::{Digest, Sha256};

use super::tests_b::{
    Hc, Hs, check_hmac_ack, define, define_raw, hc, hmac_auth_entry, hmac_command, kdfa_256,
    name_of, nv_attributes, nv_public, nv_read, nv_read_data, nv_read_public, nv_write, pw_cmd,
    run_hmac, set_auth, shutdown, split_response, start_cmd, start_params, start_session, startup,
    trimmed,
};
use super::tests_c2::{
    FIRMWARE, attest_bytes, ecdsa_parts, h256, hexs, make_child, parse_attestation, qualified,
    quote,
};
use super::tests_obj::*;
use super::*;
use crate::StateError;
use crate::codes::{object_attr as oa, se, session_attr};
use crate::marshal::{SymDef, Unique};
use crate::nv::attr;

const NV0: u32 = 0x0100_0000;
const LOCKOUT: u32 = handle::RH_LOCKOUT;
const OWNER_RW: u32 = attr::OWNERWRITE | attr::OWNERREAD;
const NT_COUNTER: u32 = 0x10;
const NT_BITS: u32 = 0x20;
const NT_EXTEND: u32 = 0x40;
const ST_ATTEST_TIME: u16 = 0x8019;

/// Python: `SHA-256(0^32 || "abc")`, then `SHA-256(that || "def")`, then `SHA-256(that)`.
const EXT_1: &str = "365aa7d8f7f9402c4b9434502b4cc89ddb09fe50d7cd95b493b834c62d5a5370";
const EXT_2: &str = "532f0131db1635cb105c9753e41e7908599aa4dd144658c92e50b60113ad993a";
const EXT_3: &str = "7f79fa5ca1edde2fe3027948a841617e05e8ecde764b7ac267286bba8f02b06b";

// ------------------------------------------------------------------------------- helpers

fn nv_cmd(t: &mut Tpm, code: u32, auth: u32, index: u32, pw: &[u8], params: &[u8]) -> Vec<u8> {
    pw_cmd(t, code, &[auth, index], pw, params)
}

fn increment(t: &mut Tpm, index: u32) -> Vec<u8> {
    nv_cmd(t, cc::NV_INCREMENT, OWNER, index, b"", &[])
}

fn extend_nv(t: &mut Tpm, index: u32, data: &[u8]) -> Vec<u8> {
    nv_cmd(t, cc::NV_EXTEND, OWNER, index, b"", &tpm2b(data))
}

fn set_bits(t: &mut Tpm, index: u32, bits: u64) -> Vec<u8> {
    nv_cmd(t, cc::NV_SET_BITS, OWNER, index, b"", &bits.to_be_bytes())
}

fn read_all(t: &mut Tpm, index: u32, size: u16) -> Vec<u8> {
    nv_read_data(&nv_read(t, OWNER, index, b"", size, 0))
}

fn counter_value(t: &mut Tpm, index: u32) -> u64 {
    u64::from_be_bytes(read_all(t, index, 8).try_into().unwrap())
}

fn def(t: &mut Tpm, index: u32, attributes: u32, size: u16) -> Vec<u8> {
    define(t, OWNER, index, attributes, size)
}

fn nv_certify(
    t: &mut Tpm,
    handles: [u32; 3],
    passwords: &[&[u8]],
    scheme: &[u8],
    size: u16,
    offset: u16,
) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::NV_CERTIFY,
            &handles,
            passwords,
            &cat(&[&tpm2b(b"nonce"), scheme, &u16b(size), &u16b(offset)]),
        ),
    )
}

#[test]
fn nv_certify_signs_a_range_and_the_digest_of_a_large_index() {
    let mut t = started();
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let mut data = vec![0x12; 512];
    data.extend_from_slice(&[0x34; 188]);
    let index = NV0 + 1;
    ok(&def(&mut t, index, OWNER_RW, 700));
    let handles = [key.handle, OWNER, index];
    let passwords = &[b"kpw".as_slice(), b"".as_slice()];
    let scheme = u16b(alg::NULL);
    err(
        &nv_certify(&mut t, handles, passwords, &scheme, 0, 0),
        Rc::NV_UNINITIALIZED,
    );
    ok(&nv_write(&mut t, OWNER, index, b"", &data[..512], 0));
    ok(&nv_write(&mut t, OWNER, index, b"", &data[512..], 512));
    let (_, name) = nv_read_public(&mut t, index);
    let (_, clock) = clock_now(&mut t);
    for (size, offset, tag, payload) in [
        (16, 508, 0x8014, cat(&[&u16b(508), &tpm2b(&data[508..524])])),
        (0, 700, 0x8014, cat(&[&u16b(700), &tpm2b(b"")])),
        (0, 0, 0x801C, tpm2b(&h256(&[&data]))),
    ] {
        let response = nv_certify(&mut t, handles, passwords, &scheme, size, offset);
        let (attest, signature) = parse_attestation(&response);
        assert_eq!(
            attest,
            attest_bytes(
                tag,
                &qualified(PLATFORM, &key.name),
                b"nonce",
                clock,
                FIRMWARE,
                &cat(&[&tpm2b(&name), &payload]),
            ),
        );
        ok(&verify(&mut t, key.handle, &h256(&[&attest]), &signature));
    }
    assert_eq!(nv_read_public(&mut t, index).1, name);
}

#[test]
fn nv_certify_checks_nv_access_and_requires_a_hash_for_an_unsigned_digest() {
    let mut t = started();
    let index = NV0 + 1;
    let public = nv_public(
        index,
        alg::SHA256,
        attr::AUTHREAD | attr::AUTHWRITE | attr::READ_STCLEAR,
        b"",
        520,
    );
    ok(&define_raw(&mut t, OWNER, b"", b"nv-pw", &public));
    let handles = [NULL, index, index];
    let passwords = &[b"".as_slice(), b"nv-pw".as_slice()];
    let null = u16b(alg::NULL);
    let ecdsa = ecc_sig_scheme();
    ok(&nv_write(&mut t, index, index, b"nv-pw", b"abc", 0));
    for (handles, passwords, scheme, size, offset, error) in [
        (
            [NULL, OWNER, index],
            [b"".as_slice(); 2],
            &ecdsa,
            0,
            0,
            Rc::AUTH_TYPE,
        ),
        (
            [NULL, NULL, index],
            [b"".as_slice(); 2],
            &ecdsa,
            0,
            0,
            Rc::VALUE.in_handle(2),
        ),
        (
            [NULL, OWNER, index + 1],
            [b"".as_slice(); 2],
            &ecdsa,
            0,
            0,
            Rc::HANDLE.in_handle(3),
        ),
        (
            handles,
            [b"".as_slice(), b"bad".as_slice()],
            &ecdsa,
            0,
            0,
            Rc::AUTH_FAIL.in_session(2),
        ),
        (handles, *passwords, &null, 0, 0, Rc::SCHEME.in_param(2)),
        (
            handles,
            *passwords,
            &hex("00180010"),
            0,
            0,
            Rc::SCHEME.in_param(2),
        ),
        (handles, *passwords, &ecdsa, 513, 0, Rc::VALUE.in_param(3)),
        (handles, *passwords, &ecdsa, 2, 519, Rc::NV_RANGE),
    ] {
        err(
            &nv_certify(&mut t, handles, &passwords, scheme, size, offset),
            error,
        );
    }
    let (_, name) = nv_read_public(&mut t, index);
    let response = nv_certify(&mut t, handles, passwords, &ecdsa, 0, 0);
    let (attest, signature) = parse_attestation(&response);
    let expected_digest = h256(&[b"abc", &[0; 517]]);
    assert_eq!(&attest[..6], &hex("ff544347801c"));
    assert!(attest.ends_with(&cat(&[&tpm2b(&name), &tpm2b(&expected_digest)])));
    assert_eq!(signature, null);
    ok(&nv_cmd(
        &mut t,
        cc::NV_READ_LOCK,
        index,
        index,
        b"nv-pw",
        &[],
    ));
    err(
        &nv_certify(&mut t, handles, passwords, &ecdsa, 0, 0),
        Rc::NV_LOCKED,
    );
}

fn nt_of(attributes: u32) -> u32 {
    (attributes >> 4) & 0xF
}

// ------------------------------------------------------------------------------ NV counters

#[test]
fn a_counter_starts_unwritten_counts_up_and_refuses_other_writes() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(&mut t, idx, OWNER_RW | NT_COUNTER, 8));
    let a = nv_attributes(&mut t, idx);
    assert_eq!(nt_of(a), 1);
    assert_eq!(a & attr::WRITTEN, 0);
    let (_, name_before) = nv_read_public(&mut t, idx);
    // An unwritten counter cannot be read (Part 3, clause 31.3).
    err(
        &nv_read(&mut t, OWNER, idx, b"", 8, 0),
        Rc::NV_UNINITIALIZED,
    );
    // The first increment starts from the largest value any counter had (none: 0) plus one.
    for expect in 1..=3u64 {
        ok(&increment(&mut t, idx));
        assert_eq!(counter_value(&mut t, idx), expect);
    }
    assert_ne!(nv_attributes(&mut t, idx) & attr::WRITTEN, 0);
    assert_ne!(
        nv_read_public(&mut t, idx).1,
        name_before,
        "Name follows WRITTEN"
    );
    // A partial read of the counter.
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 2, 6)),
        [0, 3]
    );
    // The counter changes only by its own command.
    err(
        &nv_write(&mut t, OWNER, idx, b"", &[0; 8], 0),
        Rc::ATTRIBUTES,
    );
    err(&extend_nv(&mut t, idx, b"x"), Rc::ATTRIBUTES);
    err(&set_bits(&mut t, idx, 1), Rc::ATTRIBUTES);
    assert_eq!(counter_value(&mut t, idx), 3);
    // Increment of the other types.
    ok(&def(&mut t, NV0 + 2, OWNER_RW, 8));
    err(&increment(&mut t, NV0 + 2), Rc::ATTRIBUTES);
    ok(&def(&mut t, NV0 + 3, OWNER_RW | NT_BITS, 8));
    err(&increment(&mut t, NV0 + 3), Rc::ATTRIBUTES);
    ok(&def(&mut t, NV0 + 4, OWNER_RW | NT_EXTEND, 32));
    err(&increment(&mut t, NV0 + 4), Rc::ATTRIBUTES);
}

#[test]
fn a_redefined_counter_never_goes_back() {
    let mut t = started();
    let (a, b) = (NV0 + 1, NV0 + 2);
    ok(&def(&mut t, a, OWNER_RW | NT_COUNTER, 8));
    for _ in 0..5 {
        ok(&increment(&mut t, a));
    }
    // A second counter defined while the first lives starts above the largest value seen.
    ok(&def(&mut t, b, OWNER_RW | NT_COUNTER, 8));
    ok(&increment(&mut t, b));
    assert_eq!(counter_value(&mut t, b), 6);
    // Delete and define the first one again: it does not restart at 1.
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, a],
        b"",
        &[],
    ));
    ok(&def(&mut t, a, OWNER_RW | NT_COUNTER, 8));
    ok(&increment(&mut t, a));
    assert_eq!(counter_value(&mut t, a), 7);
    // ... and not after `TPM2_Clear` either.
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]));
    ok(&def(&mut t, a, OWNER_RW | NT_COUNTER, 8));
    ok(&increment(&mut t, a));
    assert_eq!(counter_value(&mut t, a), 8);
}

#[test]
fn nv_increment_checks_handles_authorization_locks_and_parameters() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(
        &mut t,
        idx,
        OWNER_RW | NT_COUNTER | attr::WRITEDEFINE,
        8,
    ));
    // Handles: undefined index (handle 2), no NV index at handle 2, authHandle types (handle 1).
    err(&increment(&mut t, NV0 + 9), Rc::HANDLE.in_handle(2));
    err(&increment(&mut t, 0x0200_0001), Rc::VALUE.in_handle(2));
    for bad in [NULL, ENDORSEMENT, LOCKOUT, 0u32, 0x8000_0000] {
        err(
            &nv_cmd(&mut t, cc::NV_INCREMENT, bad, idx, b"", &[]),
            Rc::VALUE.in_handle(1),
        );
    }
    // Authorization: wrong password, no session, a role the attributes do not allow.
    err(
        &nv_cmd(&mut t, cc::NV_INCREMENT, OWNER, idx, b"bad", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(&mut t, plain(cc::NV_INCREMENT, &[OWNER, idx], &[])),
        Rc::AUTH_MISSING,
    );
    err(
        &nv_cmd(&mut t, cc::NV_INCREMENT, PLATFORM, idx, b"", &[]),
        Rc::AUTH_TYPE,
    );
    err(
        &nv_cmd(&mut t, cc::NV_INCREMENT, idx, idx, b"", &[]),
        Rc::AUTH_UNAVAILABLE,
    );
    // A trailing octet.
    err(
        &nv_cmd(&mut t, cc::NV_INCREMENT, OWNER, idx, b"", &[0]),
        Rc::SIZE,
    );
    // Nothing above changed the counter.
    ok(&increment(&mut t, idx));
    assert_eq!(counter_value(&mut t, idx), 1);
    // Write lock: authorized first, then TPM_RC_NV_LOCKED; the counter stays.
    ok(&nv_cmd(&mut t, cc::NV_WRITE_LOCK, OWNER, idx, b"", &[]));
    err(&increment(&mut t, idx), Rc::NV_LOCKED);
    err(
        &nv_cmd(&mut t, cc::NV_INCREMENT, OWNER, idx, b"bad", &[]),
        Rc::BAD_AUTH.in_session(1),
    );
    assert_eq!(counter_value(&mut t, idx), 1);
}

#[test]
fn nv_increment_with_an_hmac_session_and_the_index_as_its_own_authorization() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"idxpw",
        &nv_public(
            idx,
            alg::SHA256,
            attr::AUTHWRITE | attr::AUTHREAD | NT_COUNTER,
            &[],
            8,
        ),
    ));
    let (_, name) = nv_read_public(&mut t, idx);
    let mut s = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let mut c = hc(cc::NV_INCREMENT, &[idx, idx], Vec::new(), (idx, b"idxpw"));
    c.names = vec![name.clone(), name];
    // The Name in the cpHash changes with WRITTEN: the response HMAC and the next command use
    // the new Name, so the client recomputes it each time.
    ok(&run_hmac(&mut t, &mut s, &c));
    let (_, name2) = nv_read_public(&mut t, idx);
    let mut c2 = hc(cc::NV_INCREMENT, &[idx, idx], Vec::new(), (idx, b"idxpw"));
    c2.names = vec![name2.clone(), name2];
    ok(&run_hmac(&mut t, &mut s, &c2));
    assert_eq!(
        u64::from_be_bytes(
            nv_read_data(&nv_read(&mut t, idx, idx, b"idxpw", 8, 0))
                .try_into()
                .unwrap()
        ),
        2
    );
    // A command HMAC computed over the stale Name is refused.
    let mut stale = hc(cc::NV_INCREMENT, &[idx, idx], Vec::new(), (idx, b"idxpw"));
    stale.names = vec![name_of(idx), name_of(idx)];
    err(
        &run(&mut t, hmac_command(&s, &stale, false, false)),
        Rc::AUTH_FAIL.in_session(1),
    );
}

// ------------------------------------------------------------------------------- NV extend

#[test]
fn nv_extend_follows_sha256_of_old_value_and_data() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(&mut t, idx, OWNER_RW | NT_EXTEND, 32));
    assert_eq!(nt_of(nv_attributes(&mut t, idx)), 4);
    err(
        &nv_read(&mut t, OWNER, idx, b"", 32, 0),
        Rc::NV_UNINITIALIZED,
    );
    // The old value of an unwritten index is the zero digest.
    ok(&extend_nv(&mut t, idx, b"abc"));
    assert_eq!(read_all(&mut t, idx, 32), hex(EXT_1));
    assert_ne!(nv_attributes(&mut t, idx) & attr::WRITTEN, 0);
    ok(&extend_nv(&mut t, idx, b"def"));
    assert_eq!(read_all(&mut t, idx, 32), hex(EXT_2));
    // An empty buffer is a valid extend; so is the largest buffer.
    ok(&extend_nv(&mut t, idx, b""));
    assert_eq!(read_all(&mut t, idx, 32), hex(EXT_3));
    ok(&extend_nv(&mut t, idx, &[7u8; NV_BUFFER_MAX_FOR_TESTS]));
    err(
        &extend_nv(&mut t, idx, &[7u8; NV_BUFFER_MAX_FOR_TESTS + 1]),
        Rc::SIZE.in_param(1),
    );
    // Reads of part of the digest.
    let whole = read_all(&mut t, idx, 32);
    assert_eq!(
        nv_read_data(&nv_read(&mut t, OWNER, idx, b"", 16, 8)),
        whole[8..24]
    );
}

const NV_BUFFER_MAX_FOR_TESTS: usize = 512;

#[test]
fn nv_extend_errors() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(
        &mut t,
        idx,
        OWNER_RW | NT_EXTEND | attr::WRITEDEFINE,
        32,
    ));
    // Not an extend index; write of an extend index.
    ok(&def(&mut t, NV0 + 2, OWNER_RW, 32));
    err(&extend_nv(&mut t, NV0 + 2, b"x"), Rc::ATTRIBUTES);
    ok(&def(&mut t, NV0 + 3, OWNER_RW | NT_COUNTER, 8));
    err(&extend_nv(&mut t, NV0 + 3, b"x"), Rc::ATTRIBUTES);
    err(
        &nv_write(&mut t, OWNER, idx, b"", &[0; 32], 0),
        Rc::ATTRIBUTES,
    );
    // Handles and authorization.
    err(&extend_nv(&mut t, NV0 + 9, b"x"), Rc::HANDLE.in_handle(2));
    err(
        &nv_cmd(&mut t, cc::NV_EXTEND, NULL, idx, b"", &tpm2b(b"x")),
        Rc::VALUE.in_handle(1),
    );
    err(
        &nv_cmd(&mut t, cc::NV_EXTEND, OWNER, idx, b"bad", &tpm2b(b"x")),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &nv_cmd(&mut t, cc::NV_EXTEND, PLATFORM, idx, b"", &tpm2b(b"x")),
        Rc::AUTH_TYPE,
    );
    // Parameters: missing, trailing.
    err(
        &nv_cmd(&mut t, cc::NV_EXTEND, OWNER, idx, b"", &[]),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &nv_cmd(
            &mut t,
            cc::NV_EXTEND,
            OWNER,
            idx,
            b"",
            &cat(&[&tpm2b(b"x"), &[0]]),
        ),
        Rc::SIZE,
    );
    // Locked: TPM_RC_NV_LOCKED after the authorization.
    ok(&extend_nv(&mut t, idx, b"abc"));
    ok(&nv_cmd(&mut t, cc::NV_WRITE_LOCK, OWNER, idx, b"", &[]));
    err(&extend_nv(&mut t, idx, b"def"), Rc::NV_LOCKED);
    err(
        &nv_cmd(&mut t, cc::NV_EXTEND, OWNER, idx, b"bad", &tpm2b(b"x")),
        Rc::BAD_AUTH.in_session(1),
    );
    assert_eq!(read_all(&mut t, idx, 32), hex(EXT_1));
}

// ------------------------------------------------------------------------------ NV bit field

#[test]
fn nv_set_bits_ors_into_the_field() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(&mut t, idx, OWNER_RW | NT_BITS, 8));
    assert_eq!(nt_of(nv_attributes(&mut t, idx)), 2);
    err(
        &nv_read(&mut t, OWNER, idx, b"", 8, 0),
        Rc::NV_UNINITIALIZED,
    );
    // An unwritten field counts as zero; WRITTEN is set even when no bit is set.
    ok(&set_bits(&mut t, idx, 0));
    assert_ne!(nv_attributes(&mut t, idx) & attr::WRITTEN, 0);
    assert_eq!(read_all(&mut t, idx, 8), [0u8; 8]);
    ok(&set_bits(&mut t, idx, 0x0000_0000_0000_00F0));
    ok(&set_bits(&mut t, idx, 0x0000_0000_0000_000F));
    assert_eq!(read_all(&mut t, idx, 8), hex("00000000000000ff"));
    // Bits are never cleared; all 64 bits are usable.
    ok(&set_bits(&mut t, idx, 0x8000_0000_0000_0001));
    ok(&set_bits(&mut t, idx, 0));
    assert_eq!(read_all(&mut t, idx, 8), hex("80000000000000ff"));
    // The field changes only by SetBits.
    err(
        &nv_write(&mut t, OWNER, idx, b"", &[0; 8], 0),
        Rc::ATTRIBUTES,
    );
    err(&extend_nv(&mut t, idx, b"x"), Rc::ATTRIBUTES);
    err(&increment(&mut t, idx), Rc::ATTRIBUTES);
    assert_eq!(read_all(&mut t, idx, 8), hex("80000000000000ff"));
}

#[test]
fn nv_set_bits_errors() {
    let mut t = started();
    let idx = NV0 + 1;
    ok(&def(&mut t, idx, OWNER_RW | NT_BITS | attr::WRITEDEFINE, 8));
    ok(&def(&mut t, NV0 + 2, OWNER_RW, 8));
    err(&set_bits(&mut t, NV0 + 2, 1), Rc::ATTRIBUTES);
    ok(&def(&mut t, NV0 + 3, OWNER_RW | NT_COUNTER, 8));
    err(&set_bits(&mut t, NV0 + 3, 1), Rc::ATTRIBUTES);
    err(&set_bits(&mut t, NV0 + 9, 1), Rc::HANDLE.in_handle(2));
    err(
        &nv_cmd(&mut t, cc::NV_SET_BITS, ENDORSEMENT, idx, b"", &[0; 8]),
        Rc::VALUE.in_handle(1),
    );
    err(
        &nv_cmd(&mut t, cc::NV_SET_BITS, OWNER, idx, b"bad", &[0; 8]),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &nv_cmd(&mut t, cc::NV_SET_BITS, PLATFORM, idx, b"", &[0; 8]),
        Rc::AUTH_TYPE,
    );
    // Parameters: a short field, a trailing octet.
    err(
        &nv_cmd(&mut t, cc::NV_SET_BITS, OWNER, idx, b"", &[0; 7]),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &nv_cmd(&mut t, cc::NV_SET_BITS, OWNER, idx, b"", &[0; 9]),
        Rc::SIZE,
    );
    ok(&set_bits(&mut t, idx, 6));
    ok(&nv_cmd(&mut t, cc::NV_WRITE_LOCK, OWNER, idx, b"", &[]));
    err(&set_bits(&mut t, idx, 1), Rc::NV_LOCKED);
    assert_eq!(read_all(&mut t, idx, 8), hex("0000000000000006"));
}

// ---------------------------------------------------------------------- definition by type

#[test]
fn nv_define_space_checks_the_size_of_the_type() {
    let mut t = started();
    for (nt, good, bad) in [
        (NT_COUNTER, 8u16, [0u16, 7, 9, 32]),
        (NT_BITS, 8, [0, 7, 9, 32]),
        (NT_EXTEND, 32, [0, 8, 31, 33]),
    ] {
        for size in bad {
            err(
                &def(&mut t, NV0 + 1, OWNER_RW | nt, size),
                Rc::SIZE.in_param(2),
            );
        }
        ok(&def(&mut t, NV0 + 1, OWNER_RW | nt, good));
        ok(&pw_cmd(
            &mut t,
            cc::NV_UNDEFINE_SPACE,
            &[OWNER, NV0 + 1],
            b"",
            &[],
        ));
    }
    // The public area reports the type and the size.
    ok(&def(&mut t, NV0 + 1, OWNER_RW | NT_COUNTER, 8));
    let (public, _) = nv_read_public(&mut t, NV0 + 1);
    assert_eq!(&public[10..12], &[0, 0], "authPolicy is empty");
    assert_eq!(&public[12..14], &[0, 8], "dataSize");
    // PIN indices, reserved types and CLEAR_STCLEAR stay TPM_RC_ATTRIBUTES.
    for a in [
        OWNER_RW | 0x80,
        OWNER_RW | 0x90,
        OWNER_RW | 0x30,
        OWNER_RW | 0x50,
        OWNER_RW | 0x70,
        OWNER_RW | NT_COUNTER | attr::CLEAR_STCLEAR,
    ] {
        err(&def(&mut t, NV0 + 2, a, 8), Rc::ATTRIBUTES.in_param(2));
    }
    // The platform defines a counter that the owner cannot delete; `TPM2_Clear` keeps it.
    let attrs = attr::PPWRITE | attr::PPREAD | attr::PLATFORMCREATE | NT_COUNTER;
    ok(&define(&mut t, PLATFORM, NV0 + 5, attrs, 8));
    ok(&nv_cmd(
        &mut t,
        cc::NV_INCREMENT,
        PLATFORM,
        NV0 + 5,
        b"",
        &[],
    ));
    err(
        &pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, NV0 + 5], b"", &[]),
        Rc::NV_AUTHORIZATION,
    );
    ok(&pw_cmd(&mut t, cc::CLEAR, &[LOCKOUT], b"", &[]));
    assert_eq!(
        u64::from_be_bytes(
            nv_read_data(&nv_read(&mut t, PLATFORM, NV0 + 5, b"", 8, 0))
                .try_into()
                .unwrap()
        ),
        1
    );
}

// -------------------------------------------------------------------------------- snapshot

/// A TPM with one index of each type, written, and a deleted counter (so the maximum is above
/// every live counter), shut down.
fn typed_nv_tpm() -> Tpm {
    let mut t = started();
    ok(&def(&mut t, NV0 + 1, OWNER_RW | NT_COUNTER, 8));
    for _ in 0..3 {
        ok(&increment(&mut t, NV0 + 1));
    }
    ok(&def(&mut t, NV0 + 2, OWNER_RW | NT_BITS, 8));
    ok(&set_bits(&mut t, NV0 + 2, 0x1234_0000_0000_0042));
    ok(&def(&mut t, NV0 + 3, OWNER_RW | NT_EXTEND, 32));
    ok(&extend_nv(&mut t, NV0 + 3, b"abc"));
    ok(&def(&mut t, NV0 + 4, OWNER_RW, 5));
    ok(&nv_write(&mut t, OWNER, NV0 + 4, b"", b"hello", 0));
    // An unwritten counter, and a deleted counter that had counted to 9.
    ok(&def(&mut t, NV0 + 5, OWNER_RW | NT_COUNTER, 8));
    ok(&def(&mut t, NV0 + 6, OWNER_RW | NT_COUNTER, 8));
    for _ in 0..6 {
        ok(&increment(&mut t, NV0 + 6));
    }
    assert_eq!(counter_value(&mut t, NV0 + 6), 9);
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, NV0 + 6],
        b"",
        &[],
    ));
    shutdown(&mut t, 0);
    t
}

fn refix(snapshot: &mut [u8]) {
    let n = snapshot.len() - 16 - 32;
    snapshot[12..16].copy_from_slice(&(n as u32).to_be_bytes());
    let digest = Sha256::digest(&snapshot[16..16 + n]);
    snapshot[16 + n..].copy_from_slice(&digest);
}

fn offset_of(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("pattern in the snapshot")
}

#[test]
fn counter_bit_field_and_extend_indices_survive_the_snapshot() {
    let t = typed_nv_tpm();
    let snap = t.persistent_snapshot();
    assert_eq!(
        u16::from_be_bytes([snap[8], snap[9]]),
        crate::SNAPSHOT_VERSION
    );
    let mut r = Tpm::restore(&snap, &[9; 48]).unwrap();
    assert_eq!(r.persistent_snapshot(), snap, "byte for byte");
    startup(&mut r, 0);
    assert_eq!(counter_value(&mut r, NV0 + 1), 3);
    assert_eq!(read_all(&mut r, NV0 + 2, 8), hex("1234000000000042"));
    assert_eq!(read_all(&mut r, NV0 + 3, 32), hex(EXT_1));
    assert_eq!(read_all(&mut r, NV0 + 4, 5), b"hello");
    for (idx, nt) in [(NV0 + 1, 1), (NV0 + 2, 2), (NV0 + 3, 4), (NV0 + 4, 0)] {
        assert_eq!(nt_of(nv_attributes(&mut r, idx)), nt);
    }
    // The restored counter goes on; the unwritten one starts above the largest value ever seen
    // (the deleted counter reached 9), also after a second round trip.
    ok(&increment(&mut r, NV0 + 1));
    assert_eq!(counter_value(&mut r, NV0 + 1), 4);
    err(
        &nv_read(&mut r, OWNER, NV0 + 5, b"", 8, 0),
        Rc::NV_UNINITIALIZED,
    );
    shutdown(&mut r, 0);
    let mut r2 = Tpm::restore(&r.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut r2, 0);
    ok(&increment(&mut r2, NV0 + 5));
    assert_eq!(counter_value(&mut r2, NV0 + 5), 10);
    // The extend index continues the chain and the bit field keeps accumulating.
    ok(&extend_nv(&mut r2, NV0 + 3, b"def"));
    assert_eq!(read_all(&mut r2, NV0 + 3, 32), hex(EXT_2));
    ok(&set_bits(&mut r2, NV0 + 2, 1));
    assert_eq!(read_all(&mut r2, NV0 + 2, 8), hex("1234000000000043"));
}

#[test]
fn a_damaged_nv_snapshot_is_refused_never_reset() {
    let snap = typed_nv_tpm().persistent_snapshot();
    let entropy = [9u8; 48];
    let refuse = |b: &mut Vec<u8>, expect: StateError, what: &str| {
        refix(b);
        assert_eq!(Tpm::restore(b, &entropy).unwrap_err(), expect, "{what}");
    };
    // Plain damage: truncation, a flipped bit under the digest, a foreign version.
    assert_eq!(
        Tpm::restore(&snap[..snap.len() - 1], &entropy).unwrap_err(),
        StateError::BadLength
    );
    let mut b = snap.clone();
    b[40] ^= 1;
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::BadDigest
    );
    let mut b = snap.clone();
    b[9] = 2;
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::UnsupportedVersion(2)
    );
    // The attributes of the counter (handle, nameAlg, then attributes): change its type.
    let attrs_at = offset_of(&snap, &u32b(NV0 + 1)) + 6;
    let attrs = u32::from_be_bytes(snap[attrs_at..attrs_at + 4].try_into().unwrap());
    assert_eq!(nt_of(attrs), 1);
    let set_attrs = |snap: &[u8], v: u32| {
        let mut b = snap.to_vec();
        b[attrs_at..attrs_at + 4].copy_from_slice(&v.to_be_bytes());
        b
    };
    // Extend type with 8 octets of data: wrong size.
    refuse(
        &mut set_attrs(&snap, (attrs & !0xF0) | NT_EXTEND),
        StateError::BadField,
        "size of the type",
    );
    // Reserved type, PIN type, unsupported attribute, reserved bit.
    for (v, what) in [
        ((attrs & !0xF0) | 0x30, "reserved type"),
        ((attrs & !0xF0) | 0x80, "PIN type"),
        (attrs | attr::ORDERLY, "unsupported attribute"),
        (attrs | 0x100, "reserved bit"),
        (attrs & !(attr::OWNERREAD), "no read access"),
    ] {
        refuse(&mut set_attrs(&snap, v), StateError::BadField, what);
    }
    // WRITTEN clear but the data not zero.
    refuse(
        &mut set_attrs(&snap, attrs & !attr::WRITTEN),
        StateError::BadField,
        "data without WRITTEN",
    );
    // The counter maximum precedes the object count, policies, total reset count, bank mask
    // and establishment bit, followed by the three seed extensions.
    let max_at = snap.len() - 32 - 96 - 1 - 1 - 8 - 6 - 1 - 8;
    assert_eq!(&snap[max_at..max_at + 8], &9u64.to_be_bytes());
    let mut b = snap.clone();
    b[max_at..max_at + 8].copy_from_slice(&2u64.to_be_bytes());
    refuse(&mut b, StateError::BadField, "counter above the maximum");
    // The maximum may be larger; a truncated payload is a length error, not an empty TPM.
    let mut b = snap.clone();
    b[max_at..max_at + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    refix(&mut b);
    assert!(Tpm::restore(&b, &entropy).is_ok());
    let mut b = snap.clone();
    b.drain(max_at..b.len() - 32);
    refuse(&mut b, StateError::BadLength, "maximum missing");
}

// ------------------------------------------------------------------------------- GetTime

fn get_time(t: &mut Tpm, sign: u32, key_pw: &[u8], qualifying: &[u8], scheme: &[u8]) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::GET_TIME,
            &[ENDORSEMENT, sign],
            &[b"", key_pw],
            &cat(&[&tpm2b(qualifying), scheme]),
        ),
    )
}

/// `TPMS_TIME_INFO` and the `(clock, resetCount, restartCount)` of the TPM now (`TPM2_ReadClock`).
fn clock_now(t: &mut Tpm) -> (Vec<u8>, (u64, u32, u32)) {
    let r = run(t, plain(cc::READ_CLOCK, &[], &[]));
    let p = params(&r, 0).to_vec();
    assert_eq!(p.len(), 25);
    assert_eq!(p[24], 1, "safe");
    let clock = u64::from_be_bytes(p[8..16].try_into().unwrap());
    let reset = u32::from_be_bytes(p[16..20].try_into().unwrap());
    let restart = u32::from_be_bytes(p[20..24].try_into().unwrap());
    (p, (clock, reset, restart))
}

/// `TPMS_TIME_ATTEST_INFO`: the time info and the firmware version.
fn time_attested(time_info: &[u8]) -> Vec<u8> {
    cat(&[time_info, &FIRMWARE.to_be_bytes()])
}

#[test]
fn clock_set_is_monotonic_authorized_and_survives_a_power_cycle() {
    const MAX_SET: u64 = 0xffff_0000_0000_0000;
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner"));
    ok(&set_auth(&mut t, PLATFORM, b"", b"platform"));
    pass_time(&mut t, 5);
    let initial = clock_now(&mut t).0;
    let set = |t: &mut Tpm, auth, password: &[u8], milliseconds: u64| {
        run(
            t,
            with_pw(
                cc::CLOCK_SET,
                &[auth],
                &[password],
                &milliseconds.to_be_bytes(),
            ),
        )
    };
    for invalid in [4_999, MAX_SET + 1] {
        err(
            &set(&mut t, OWNER, b"owner", invalid),
            Rc::VALUE.in_param(1),
        );
    }
    err(
        &set(&mut t, OWNER, b"bad", 50_000),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &set(&mut t, ENDORSEMENT, b"", 50_000),
        Rc::VALUE.in_handle(1),
    );
    assert_eq!(clock_now(&mut t).0, initial);

    ok(&set(&mut t, OWNER, b"owner", 5_000));
    assert_eq!(clock_now(&mut t).0, initial);
    ok(&set(&mut t, OWNER, b"owner", 50_000));
    let (info, clock) = clock_now(&mut t);
    assert_eq!(clock.0, 50_000);
    assert_eq!(&info[..8], &5_000u64.to_be_bytes());
    assert_eq!(&info[16..], &initial[16..], "counters and safe");
    pass_time(&mut t, 2);
    assert_eq!(clock_now(&mut t).1.0, 52_000);
    ok(&set(&mut t, PLATFORM, b"platform", MAX_SET));
    assert_eq!(clock_now(&mut t).1.0, MAX_SET);
    pass_time(&mut t, 1);
    assert_eq!(clock_now(&mut t).1.0, MAX_SET + 1_000);

    let snapshot = t.persistent_snapshot();
    t.init();
    let mut restored = Tpm::restore(&snapshot, &[0x41; 48]).unwrap();
    for after in [&mut t, &mut restored] {
        startup(after, crate::codes::su::CLEAR);
        let response = run(after, plain(cc::READ_CLOCK, &[], &[]));
        let info = params(&response, 0);
        assert_eq!(&info[8..16], &(MAX_SET + 1_000).to_be_bytes());
        assert_eq!(&info[..8], &[0; 8], "new power-on cycle");
        assert_eq!(info[24], 0, "the previous power cycle did not shut down");
    }
}

#[test]
fn clock_rate_adjust_calibrates_both_clocks_and_snapshots_the_elapsed_time() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner"));
    ok(&set_auth(&mut t, PLATFORM, b"", b"platform"));
    t.advance_power_time(1000);
    let original = clock_now(&mut t).0;
    for (authorization, password, input, error) in [
        (ENDORSEMENT, b"".as_slice(), vec![3], Rc::VALUE.in_handle(1)),
        (
            OWNER,
            b"wrong".as_slice(),
            vec![3],
            Rc::BAD_AUTH.in_session(1),
        ),
        (OWNER, b"owner".as_slice(), vec![4], Rc::VALUE.in_param(1)),
        (
            OWNER,
            b"owner".as_slice(),
            Vec::new(),
            Rc::INSUFFICIENT.in_param(1),
        ),
        (OWNER, b"owner".as_slice(), vec![0, 0], Rc::SIZE),
    ] {
        err(
            &pw_cmd(
                &mut t,
                cc::CLOCK_RATE_ADJUST,
                &[authorization],
                password,
                &input,
            ),
            error,
        );
    }
    ok(&pw_cmd(
        &mut t,
        cc::CLOCK_RATE_ADJUST,
        &[PLATFORM],
        b"platform",
        &[3],
    ));
    assert_eq!(clock_now(&mut t).0, original);
    t.advance_power_time(101_000);
    let (info, clock) = clock_now(&mut t);
    assert_eq!(&info[..8], &102_000u64.to_be_bytes());
    assert_eq!(clock.0, 102_000);

    ok(&pw_cmd(
        &mut t,
        cc::CLOCK_SET,
        &[OWNER],
        b"owner",
        &800_000u64.to_be_bytes(),
    ));
    assert_eq!(clock_now(&mut t).1.0, 800_000);
    t.advance_power_time(201_000);
    let (info, clock) = clock_now(&mut t);
    assert_eq!(&info[..8], &203_000u64.to_be_bytes());
    assert_eq!(clock.0, 901_000);
    ok(&pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"platform", &[]));
    assert_eq!(
        &clock_now(&mut t).0[..16],
        &cat(&[&203_000u64.to_be_bytes(), &[0; 8]])
    );

    t.advance_power_time(301_000);
    let snapshot = t.persistent_snapshot();
    t.init();
    let mut restored = Tpm::restore(&snapshot, &[8; 48]).unwrap();
    for t in [&mut t, &mut restored] {
        startup(t, crate::codes::su::CLEAR);
        t.advance_power_time(1_000);
        let read = run(t, plain(cc::READ_CLOCK, &[], &[]));
        assert_eq!(
            &params(&read, 0)[..16],
            &cat(&[&1_000u64.to_be_bytes(), &102_000u64.to_be_bytes()])
        );
    }
}

#[test]
fn clock_rate_adjust_accumulates_fractional_time_and_rejects_out_of_range_calibration() {
    let adjust = |t: &mut Tpm, step: i8| {
        pw_cmd(t, cc::CLOCK_RATE_ADJUST, &[OWNER], b"", &step.to_be_bytes())
    };
    let mut long = started();
    let mut split = started();
    ok(&adjust(&mut long, 1));
    ok(&adjust(&mut split, 1));
    long.advance_power_time(1_000_000);
    for elapsed in (1_000..=1_000_000).step_by(1_000) {
        split.advance_power_time(elapsed);
    }
    assert_eq!(clock_now(&mut long).0, clock_now(&mut split).0);
    assert_eq!(clock_now(&mut long).1.0, 1_000_001);
    split.advance_power_time(999_950);
    assert_eq!(clock_now(&mut split).1.0, 1_000_001);

    let mut switched = started();
    ok(&adjust(&mut switched, 1));
    switched.advance_power_time(999_999);
    ok(&adjust(&mut switched, 1));
    switched.advance_power_time(1_000_000);
    assert_eq!(clock_now(&mut switched).1.0, 1_000_001);

    for (sign, adjusted_elapsed) in [(1, 115_000), (-1, 85_000)] {
        let mut t = started();
        ok(&adjust(&mut t, 2 * sign));
        ok(&adjust(&mut t, -2 * sign));
        for _ in 0..15 {
            ok(&adjust(&mut t, 3 * sign));
        }
        err(&adjust(&mut t, sign), Rc::VALUE.in_param(1));
        ok(&adjust(&mut t, 0));
        t.advance_power_time(100_000);
        assert_eq!(clock_now(&mut t).1.0, adjusted_elapsed);
        ok(&adjust(&mut t, -3 * sign));
        t.advance_power_time(200_000);
        assert_eq!(
            clock_now(&mut t).1.0,
            adjusted_elapsed + (100_000i32 + 14_000 * i32::from(sign)) as u64
        );
    }
}

const TIME_X: &str = "ef7f114d6b36e22e25103e3ea3f22a7598ed7bf9baca96df4214ca22589601b4";
const TIME_Y: &str = "cb94108076eeef988e6d73698714edef104caf5576928bcf409c688ade13cf5c";
const TIME_R: &str = "da696a2c6d384de20bc676e7601f5f644c86f1a95c4f2086899826fe59056e8a";
const TIME_S: &str = "2eafec5516683e7089e95a315ee5a3c1c56ca0c16cc0ccc3b9217aac922404ae";
const TIME_ATTEST: &str = "ff54434780190022000b6619f848a29382a87230dd749a509f02a2b808317654843277c8a533b5c021bf00056e6f6e636500000000000013880000000100000000010001000000000000000000000000138800000000000013880000000100000000010001000000000000";

#[test]
fn get_time_attests_time_and_clock_and_the_signature_verifies() {
    let mut t = started();
    pass_time(&mut t, 5);
    // A platform key: nothing is obfuscated (clause 18.7.1, the note on the two copies).
    let k = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let (info, clock) = clock_now(&mut t);
    let r = get_time(&mut t, k.handle, b"kpw", b"nonce", &u16b(alg::NULL));
    let (attest, sig) = parse_attestation(&r);
    let expect = attest_bytes(
        ST_ATTEST_TIME,
        &qualified(PLATFORM, &k.name),
        b"nonce",
        clock,
        FIRMWARE,
        &time_attested(&info),
    );
    assert_eq!(attest, expect);
    assert_eq!(clock.0, 5000);
    assert_eq!(
        u64::from_be_bytes(info[..8].try_into().unwrap()),
        5000,
        "Time"
    );
    // The TPM verifies its own signature; a changed attest does not verify.
    ok(&verify(&mut t, k.handle, &h256(&[&attest]), &sig));
    let mut bad = attest.clone();
    bad[12] ^= 1;
    err(
        &verify(&mut t, k.handle, &h256(&[&bad]), &sig),
        Rc::SIGNATURE,
    );
    // External check (Python, 2026-11-07): the pinned key, attest and signature.
    let Unique::Ecc { x, y } = unmarshal_public(&k.public).unique else {
        panic!()
    };
    let (r_, s_) = ecdsa_parts(&sig);
    assert_eq!(hexs(&x), TIME_X);
    assert_eq!(hexs(&y), TIME_Y);
    assert_eq!(hexs(&r_), TIME_R);
    assert_eq!(hexs(&s_), TIME_S);
    assert_eq!(hexs(&attest), TIME_ATTEST);
    // Time moves on: a later GetTime attests later values.
    pass_time(&mut t, 3);
    let (a2, _) = parse_attestation(&get_time(
        &mut t,
        k.handle,
        b"kpw",
        b"nonce",
        &u16b(alg::NULL),
    ));
    assert_ne!(a2, attest);
    assert_eq!(
        u64::from_be_bytes(a2[a2.len() - 33..a2.len() - 25].try_into().unwrap()),
        8000
    );
    // inScheme may repeat the key's scheme.
    ok(&get_time(&mut t, k.handle, b"kpw", b"", &ecc_sig_scheme()));
}

#[test]
fn get_time_obfuscates_only_the_outer_copies_for_a_storage_hierarchy_key() {
    let mut t = started();
    pass_time(&mut t, 2);
    let k = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &ecc_sign()));
    let qn = qualified(OWNER, &k.name);
    let (info, clock) = clock_now(&mut t);
    let (attest, _) = parse_attestation(&get_time(&mut t, k.handle, b"kpw", b"", &u16b(alg::NULL)));
    let sh_proof = t.proof(OWNER).unwrap();
    let obf = crate::crypto::hmac_sha256(
        &sh_proof,
        &[&cat(&[&u32b(1), b"OBFUSCATE", &[0], &qn, &u32b(128)])],
    );
    let be64 = |b: &[u8]| u64::from_be_bytes(b.try_into().unwrap());
    let be32 = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
    let expect = attest_bytes(
        ST_ATTEST_TIME,
        &qn,
        b"",
        (
            clock.0,
            clock.1.wrapping_add(be32(&obf[8..12])),
            clock.2.wrapping_add(be32(&obf[12..16])),
        ),
        FIRMWARE.wrapping_add(be64(&obf[0..8])),
        // The copies inside `attested.time` are in clear.
        &time_attested(&info),
    );
    assert_eq!(attest, expect);
    // An endorsement key is not obfuscated.
    let e = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign()));
    let (ea, _) = parse_attestation(&get_time(&mut t, e.handle, b"", b"", &u16b(alg::NULL)));
    assert_eq!(
        ea,
        attest_bytes(
            ST_ATTEST_TIME,
            &qualified(ENDORSEMENT, &e.name),
            b"",
            clock,
            FIRMWARE,
            &time_attested(&info)
        )
    );
}

#[test]
fn get_time_with_an_rsa_key_and_the_signature_checks_out() {
    let mut t = started();
    let k = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &rsa_sign()));
    let (attest, sig) =
        parse_attestation(&get_time(&mut t, k.handle, b"kpw", b"q", &u16b(alg::NULL)));
    let mut rd = Reader::new(&sig);
    assert_eq!(rd.u16().unwrap(), alg::RSASSA);
    assert_eq!(rd.u16().unwrap(), alg::SHA256);
    assert_eq!(rd.tpm2b(256).unwrap().len(), 256);
    ok(&verify(&mut t, k.handle, &h256(&[&attest]), &sig));
    assert_eq!(&attest[4..6], &u16b(ST_ATTEST_TIME)[..]);
}

#[test]
fn get_time_errors() {
    let (mut t, parent) = with_ecc_parent();
    let (key, _, _) = make_child(&mut t, parent, &ecc_sign(), b"kpw", b"");
    let gt = |t: &mut Tpm, sign: u32, pw: &[u8], scheme: &[u8]| get_time(t, sign, pw, b"n", scheme);
    ok(&gt(&mut t, key, b"kpw", &u16b(alg::NULL)));
    // privacyAdminHandle must be the endorsement hierarchy.
    for bad in [OWNER, PLATFORM, NULL, LOCKOUT, 0x8000_0003] {
        err(
            &run(
                &mut t,
                with_pw(
                    cc::GET_TIME,
                    &[bad, key],
                    &[b"", b"kpw"],
                    &cat(&[&tpm2b(b"n"), &u16b(alg::NULL)]),
                ),
            ),
            Rc::VALUE.in_handle(1),
        );
    }
    // signHandle: not loaded, not an object.
    err(
        &gt(&mut t, 0x8000_0020, b"", &u16b(alg::NULL)),
        Rc::reference_handle(1),
    );
    err(
        &gt(&mut t, OWNER, b"", &u16b(alg::NULL)),
        Rc::VALUE.in_handle(2),
    );
    // Authorization: key password (session 2), endorsement password (session 1), missing session.
    err(
        &gt(&mut t, key, b"bad", &u16b(alg::NULL)),
        Rc::AUTH_FAIL.in_session(2),
    );
    ok(&set_auth(&mut t, ENDORSEMENT, b"", b"endpw"));
    err(
        &gt(&mut t, key, b"kpw", &u16b(alg::NULL)),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&run(
        &mut t,
        with_pw(
            cc::GET_TIME,
            &[ENDORSEMENT, key],
            &[b"endpw", b"kpw"],
            &cat(&[&tpm2b(b"n"), &u16b(alg::NULL)]),
        ),
    ));
    err(
        &run(
            &mut t,
            with_pw(
                cc::GET_TIME,
                &[ENDORSEMENT, key],
                &[b"endpw"],
                &cat(&[&tpm2b(b"n"), &u16b(alg::NULL)]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    ok(&set_auth(&mut t, ENDORSEMENT, b"endpw", b""));
    // A key that does not sign, an x509sign key.
    err(
        &gt(&mut t, parent, b"parent-pw", &u16b(alg::NULL)),
        Rc::KEY.in_handle(2),
    );
    let mut x = ecc_sign();
    x.attributes |= oa::X509_SIGN;
    let (xk, _, _) = make_child(&mut t, parent, &x, b"xk", b"");
    err(
        &gt(&mut t, xk, b"xk", &u16b(alg::NULL)),
        Rc::ATTRIBUTES.in_handle(2),
    );
    // Schemes.
    err(
        &gt(&mut t, key, b"kpw", &rsa_sig_scheme()),
        Rc::SCHEME.in_param(2),
    );
    err(
        &gt(&mut t, key, b"kpw", &u16b(0x0015)),
        Rc::SCHEME.in_param(2),
    );
    err(
        &gt(
            &mut t,
            key,
            b"kpw",
            &cat(&[&u16b(alg::ECDSA), &u16b(0x0004)]),
        ),
        Rc::HASH.in_param(2),
    );
    // Parameters: qualifyingData too long, trailing octet.
    err(
        &get_time(&mut t, key, b"kpw", &[1u8; 35], &u16b(alg::NULL)),
        Rc::SIZE.in_param(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::GET_TIME,
                &[ENDORSEMENT, key],
                &[b"", b"kpw"],
                &cat(&[&tpm2b(b"n"), &u16b(alg::NULL), &[0]]),
            ),
        ),
        Rc::SIZE,
    );
    // A disabled endorsement hierarchy cannot authorize.
    ok(&super::tests_b::hierarchy_control(
        &mut t,
        ENDORSEMENT,
        b"",
        ENDORSEMENT,
        0,
    ));
    err(
        &gt(&mut t, key, b"kpw", &u16b(alg::NULL)),
        Rc::HIERARCHY.in_handle(1),
    );
}

#[test]
fn get_time_with_an_hmac_session_for_the_endorsement_hierarchy() {
    let mut t = started();
    let k = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let s = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let p = cat(&[&tpm2b(b"n"), &u16b(alg::NULL)]);
    let mut c = hc(
        cc::GET_TIME,
        &[ENDORSEMENT, k.handle],
        p.clone(),
        (ENDORSEMENT, b""),
    );
    c.names = vec![name_of(ENDORSEMENT), k.name.clone()];
    // Session 1 is the HMAC session (empty endorsement authValue, key empty: an empty HMAC is
    // valid); session 2 is the password of the key.
    let mut area = Vec::new();
    area.extend_from_slice(&s.handle.to_be_bytes());
    area.extend_from_slice(&tpm2b(&c.nonce_caller));
    area.push(0);
    area.extend_from_slice(&tpm2b(&[]));
    area.extend_from_slice(&handle::RS_PW.to_be_bytes());
    area.extend_from_slice(&tpm2b(&[]));
    area.push(1);
    area.extend_from_slice(&tpm2b(b"kpw"));
    let mut auth = (area.len() as u32).to_be_bytes().to_vec();
    auth.extend_from_slice(&area);
    let resp = run(
        &mut t,
        frame(
            st::SESSIONS,
            cc::GET_TIME,
            &[ENDORSEMENT, k.handle],
            &auth,
            &p,
        ),
    );
    let (_, acks) = split_response(&resp, 0);
    assert_eq!(acks.len(), 2);
    assert_eq!(
        acks[0].2,
        Vec::<u8>::new(),
        "empty HMAC answered with an empty one"
    );
}

// --------------------------------------------------------------------- signHandle = TPM_RH_NULL

/// The attestation of a command with `signHandle` `TPM_RH_NULL`: `qualifiedSigner` and the
/// obfuscation use the Qualified Name `TPM_RH_NULL` (4 octets). Built from the same formula as the
/// code; NOT VERIFIED that this is what a real TPM writes (clause 18.1 gives the QN and the KDFa
/// formula but not the content of `qualifiedSigner` for a NULL signer).
fn null_signer_attest(
    t: &Tpm,
    ty: u16,
    extra: &[u8],
    clock: (u64, u32, u32),
    attested: &[u8],
) -> Vec<u8> {
    let sh_proof = t.proof(OWNER).unwrap();
    let obf = crate::crypto::hmac_sha256(
        &sh_proof,
        &[&cat(&[
            &u32b(1),
            b"OBFUSCATE",
            &[0],
            &u32b(NULL),
            &u32b(128),
        ])],
    );
    let be64 = |b: &[u8]| u64::from_be_bytes(b.try_into().unwrap());
    let be32 = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
    attest_bytes(
        ty,
        &u32b(NULL),
        extra,
        (
            clock.0,
            clock.1.wrapping_add(be32(&obf[8..12])),
            clock.2.wrapping_add(be32(&obf[12..16])),
        ),
        FIRMWARE.wrapping_add(be64(&obf[0..8])),
        attested,
    )
}

#[test]
fn a_null_sign_handle_returns_the_attestation_with_the_null_signature() {
    let mut t = started();
    ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
    pass_time(&mut t, 4);
    let (info, clock) = clock_now(&mut t);
    let null_sig = u16b(alg::NULL);
    // GetTime.
    let r = get_time(&mut t, NULL, b"", b"qd", &u16b(alg::NULL));
    let (attest, sig) = parse_attestation(&r);
    assert_eq!(sig, null_sig, "the NULL signature is sigAlg alone");
    assert_eq!(
        attest,
        null_signer_attest(&t, ST_ATTEST_TIME, b"qd", clock, &time_attested(&info))
    );
    // The scheme is required to be valid but has no effect: NULL, RSASSA and ECDSA all give the
    // same NULL signature; an invalid scheme is refused.
    for scheme in [u16b(alg::NULL), rsa_sig_scheme(), ecc_sig_scheme()] {
        let (a, s) = parse_attestation(&get_time(&mut t, NULL, b"", b"qd", &scheme));
        assert_eq!((a, s), (attest.clone(), null_sig.clone()));
    }
    err(
        &get_time(&mut t, NULL, b"", b"qd", &u16b(0x0015)),
        Rc::SCHEME.in_param(2),
    );
    // A password for TPM_RH_NULL must be empty.
    err(
        &get_time(&mut t, NULL, b"x", b"qd", &u16b(alg::NULL)),
        Rc::BAD_AUTH.in_session(2),
    );
    // Quote.
    let sel = hex("00000001000b03110000");
    let r = quote(&mut t, NULL, b"", b"qq", &u16b(alg::NULL), &sel);
    let (attest, sig) = parse_attestation(&r);
    assert_eq!(sig, null_sig);
    let pcr_digest = h256(&[&[0u8; 32], &hex(&hexs(&read_pcr_value(&mut t, 4)))]);
    assert_eq!(
        attest,
        null_signer_attest(&t, 0x8018, b"qq", clock, &cat(&[&sel, &tpm2b(&pcr_digest)]))
    );
    // Certify: the certified object still needs its (ADMIN) authorization.
    let obj = parse_primary(&create_primary(&mut t, PLATFORM, b"opw", &ecc_sign()));
    let r = run(
        &mut t,
        with_pw(
            cc::CERTIFY,
            &[obj.handle, NULL],
            &[b"opw", b""],
            &cat(&[&tpm2b(b"qc"), &u16b(alg::NULL)]),
        ),
    );
    let (attest, sig) = parse_attestation(&r);
    assert_eq!(sig, null_sig);
    assert_eq!(
        attest,
        null_signer_attest(
            &t,
            0x8017,
            b"qc",
            clock,
            &cat(&[&tpm2b(&obj.name), &tpm2b(&qualified(PLATFORM, &obj.name))])
        )
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::CERTIFY,
                &[obj.handle, NULL],
                &[b"bad", b""],
                &cat(&[&tpm2b(b"qc"), &u16b(alg::NULL)]),
            ),
        ),
        // The platform hierarchy is not under dictionary-attack protection: BAD_AUTH.
        Rc::BAD_AUTH.in_session(1),
    );
    // Quote and Certify with a missing session, and an oversized qualifyingData.
    err(
        &run(
            &mut t,
            plain(
                cc::QUOTE,
                &[NULL],
                &cat(&[&tpm2b(b"q"), &u16b(alg::NULL), &sel]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &quote(&mut t, NULL, b"", &[1u8; 35], &u16b(alg::NULL), &sel),
        Rc::SIZE.in_param(1),
    );
}

fn read_pcr_value(t: &mut Tpm, pcr: usize) -> Vec<u8> {
    super::read_pcr(t, pcr)
}

#[test]
fn a_null_sign_handle_with_an_hmac_session() {
    let mut t = started();
    let mut s = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let sel = hex("00000000");
    let mut c = hc(
        cc::QUOTE,
        &[NULL],
        cat(&[&tpm2b(b"q"), &u16b(alg::NULL), &sel]),
        (NULL, b""),
    );
    c.names = vec![name_of(NULL)];
    // The authValue of TPM_RH_NULL is empty: the session key alone (empty here) keys the HMAC.
    let resp = run_hmac(&mut t, &mut s, &c);
    let (attest, sig) = parse_attestation(&resp);
    assert_eq!(sig, u16b(alg::NULL));
    assert_eq!(&attest[..4], &hex("ff544347")[..]);
    // A command HMAC that does not fit is refused (the NULL handle is not under DA protection).
    err(
        &run(&mut t, hmac_command(&s, &c, false, true)),
        Rc::BAD_AUTH.in_session(1),
    );
}

// -------------------------------------------------------------------- parameter encryption

const AES_SYM: [u8; 6] = [0x00, 0x06, 0x00, 128, 0x00, 0x43];
const XOR_SYM: [u8; 4] = [0x00, 0x0A, 0x00, 0x0B];

/// KDFa for any size (`bits` in the input; the Part 1 counter-mode construction).
fn kdfa_bits(key: &[u8], label: &[u8], u: &[u8], v: &[u8], bits: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 1u32;
    while out.len() < (bits as usize).div_ceil(8) {
        let data = cat(&[&u32b(i), label, &[0], u, v, &u32b(bits)]);
        out.extend_from_slice(&crate::crypto::hmac_sha256(key, &[&data]));
        i += 1;
    }
    out.truncate((bits as usize).div_ceil(8));
    out
}

/// The client side of clause 19: `sessionValue = sessionKey || authValue`.
fn crypt(
    sym: &[u8],
    s: &Hs,
    entity_auth: &[u8],
    newer: &[u8],
    older: &[u8],
    data: &[u8],
    encrypt: bool,
) -> Vec<u8> {
    let mut value = s.key.clone();
    value.extend_from_slice(&trimmed(entity_auth));
    if sym == AES_SYM {
        let kv = kdfa_bits(&value, b"CFB", newer, older, 256);
        let iv: [u8; 16] = kv[16..32].try_into().unwrap();
        crate::object::aes128_cfb(&kv[..16], &iv, data, encrypt).to_vec()
    } else {
        let mask = kdfa_bits(&value, b"XOR", newer, older, (data.len() * 8) as u32);
        data.iter().zip(mask).map(|(d, m)| d ^ m).collect()
    }
}

/// Starts a session with the given `symmetric` (the start helper of `tests_b` uses NULL).
fn start_sym(t: &mut Tpm, bind: u32, bind_auth: &[u8], sym: &[u8]) -> Hs {
    start_sym_kind(t, bind, bind_auth, sym, se::HMAC)
}

fn start_sym_kind(t: &mut Tpm, bind: u32, bind_auth: &[u8], sym: &[u8], kind: u8) -> Hs {
    let nonce_caller = [0x44u8; 16];
    let resp = run(
        t,
        start_cmd(
            NULL,
            bind,
            &start_params(&nonce_caller, &[], kind, sym, alg::SHA256),
        ),
    );
    ok(&resp);
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(&resp[14..]);
    let nonce_tpm = r.tpm2b(64).unwrap().to_vec();
    let (key, bound) = if bind == NULL {
        (Vec::new(), None)
    } else {
        let auth = trimmed(bind_auth);
        (
            kdfa_256(&auth, b"ATH", &nonce_tpm, &nonce_caller),
            Some((bind, auth)),
        )
    };
    Hs {
        handle,
        key,
        nonce_tpm,
        bound,
    }
}

const OWNER_PW: &[u8] = b"ownerpw";
const PLAIN: &[u8] = b"a secret NV value";

/// An owner-authorized index of 32 octets; the owner has a password.
fn nv_with_owner_pw() -> (Tpm, u32) {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let idx = NV0 + 1;
    ok(&define_raw(
        &mut t,
        OWNER,
        OWNER_PW,
        b"",
        &nv_public(idx, alg::SHA256, OWNER_RW, &[], 32),
    ));
    (t, idx)
}

/// `TPM2_NV_Write` as an HMAC command; the Name of the index is read now (it changes with
/// `WRITTEN`, so the cpHash needs the current one).
fn nv_write_command(t: &mut Tpm, idx: u32, params: Vec<u8>, attrs: u8) -> Hc {
    let mut c = hc(cc::NV_WRITE, &[OWNER, idx], params, (OWNER, OWNER_PW));
    c.names = vec![name_of(OWNER), nv_read_public(t, idx).1];
    c.attrs = attrs;
    c
}

fn stored(t: &mut Tpm, idx: u32, size: u16) -> Vec<u8> {
    nv_read_data(&nv_read(t, OWNER, idx, OWNER_PW, size, 0))
}

#[test]
fn a_decrypted_command_parameter_reaches_the_command_in_clear() {
    for sym in [&AES_SYM[..], &XOR_SYM[..]] {
        for bind in [NULL, OWNER] {
            let (mut t, idx) = nv_with_owner_pw();
            let mut s = start_sym(&mut t, bind, OWNER_PW, sym);
            // The client encrypts `data` with nonceCaller of the command and nonceTPM of the
            // session; the size field stays in clear; `offset` is a later parameter.
            let c0 = nv_write_command(&mut t, idx, Vec::new(), session_attr::CONTINUE_SESSION);
            let ct = crypt(
                sym,
                &s,
                OWNER_PW,
                &c0.nonce_caller,
                &s.nonce_tpm,
                PLAIN,
                true,
            );
            assert_ne!(ct, PLAIN);
            let c = nv_write_command(
                &mut t,
                idx,
                cat(&[&tpm2b(&ct), &u16b(0)]),
                session_attr::CONTINUE_SESSION | session_attr::DECRYPT,
            );
            ok(&run_hmac(&mut t, &mut s, &c));
            assert_eq!(
                stored(&mut t, idx, PLAIN.len() as u16),
                PLAIN,
                "{sym:?} bind {bind:#x}"
            );
            // The same bytes with a password session (no decrypt) store the ciphertext as is.
            ok(&nv_write(&mut t, OWNER, idx, OWNER_PW, &ct, 0));
            assert_eq!(stored(&mut t, idx, ct.len() as u16), ct);
        }
    }
}

#[test]
fn a_command_parameter_sent_in_clear_with_the_decrypt_flag_is_scrambled() {
    // Nothing in the protocol lets the TPM see that the caller forgot to encrypt: the stored
    // value is the "decryption" of the plaintext, not the plaintext.
    let (mut t, idx) = nv_with_owner_pw();
    let mut s = start_sym(&mut t, NULL, b"", &AES_SYM);
    let c = nv_write_command(
        &mut t,
        idx,
        cat(&[&tpm2b(PLAIN), &u16b(0)]),
        session_attr::CONTINUE_SESSION | session_attr::DECRYPT,
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    let got = stored(&mut t, idx, PLAIN.len() as u16);
    assert_ne!(got, PLAIN);
    // A command sent without the flag and with the plaintext stores it.
    let c = nv_write_command(
        &mut t,
        idx,
        cat(&[&tpm2b(PLAIN), &u16b(0)]),
        session_attr::CONTINUE_SESSION,
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    assert_eq!(stored(&mut t, idx, PLAIN.len() as u16), PLAIN);
}

#[test]
fn the_cp_hash_covers_the_encrypted_parameter() {
    // The command HMAC is computed over the parameter as it travels (encrypted): a client that
    // hashes the plaintext but sends the ciphertext is refused, and nothing is stored.
    let (mut t, idx) = nv_with_owner_pw();
    let s = start_sym(&mut t, NULL, b"", &AES_SYM);
    let c0 = nv_write_command(&mut t, idx, Vec::new(), 1);
    let ct = crypt(
        &AES_SYM,
        &s,
        OWNER_PW,
        &c0.nonce_caller,
        &s.nonce_tpm,
        PLAIN,
        true,
    );
    let attrs = session_attr::CONTINUE_SESSION | session_attr::DECRYPT;
    let honest = nv_write_command(&mut t, idx, cat(&[&tpm2b(&ct), &u16b(0)]), attrs);
    let mut cmd = hmac_command(&s, &honest, false, false);
    // Replace the ciphertext on the wire by one bit flipped: the HMAC no longer fits.
    let at = cmd.len() - 2 - ct.len();
    cmd[at] ^= 1;
    err(&run(&mut t, cmd), Rc::BAD_AUTH.in_session(1));
    err(
        &nv_read(&mut t, OWNER, idx, OWNER_PW, 1, 0),
        Rc::NV_UNINITIALIZED,
    );
}

#[test]
fn an_encrypted_response_parameter_decrypts_with_the_new_tpm_nonce() {
    for sym in [&AES_SYM[..], &XOR_SYM[..]] {
        let (mut t, idx) = nv_with_owner_pw();
        ok(&nv_write(&mut t, OWNER, idx, OWNER_PW, PLAIN, 0));
        let name = nv_read_public(&mut t, idx).1;
        let mut s = start_sym(&mut t, NULL, b"", sym);
        let mut c = hc(
            cc::NV_READ,
            &[OWNER, idx],
            cat(&[&u16b(PLAIN.len() as u16), &u16b(0)]),
            (OWNER, OWNER_PW),
        );
        c.names = vec![name_of(OWNER), name.clone()];
        c.attrs = session_attr::CONTINUE_SESSION | session_attr::ENCRYPT;
        // `run_hmac` checks the response HMAC over the encrypted parameter (rpHash) and rolls the
        // session nonce; the response key stream uses the new nonceTPM (nonceNewer) and the
        // nonceCaller of the command (nonceOlder).
        let resp = run_hmac(&mut t, &mut s, &c);
        let ct = nv_read_data(&resp);
        assert_eq!(ct.len(), PLAIN.len());
        assert_ne!(ct, PLAIN);
        let plain = crypt(sym, &s, OWNER_PW, &s.nonce_tpm, &c.nonce_caller, &ct, false);
        assert_eq!(plain, PLAIN, "{sym:?}");
    }
}

#[test]
fn policy_use_protects_the_response_and_resets_its_assertions() {
    const PASSWORD: &[u8] = b"sealed-password";
    let (mut t, parent) = with_ecc_parent();
    let mut session = start_sym_kind(&mut t, NULL, b"", &AES_SYM, se::POLICY);
    let policy_handle = [session.handle];
    ok(&run(
        &mut t,
        plain(cc::POLICY_AUTH_VALUE, &policy_handle, &[]),
    ));
    ok(&run(
        &mut t,
        plain(cc::POLICY_COMMAND_CODE, &policy_handle, &u32b(cc::UNSEAL)),
    ));
    let digest = run(&mut t, plain(cc::POLICY_GET_DIGEST, &policy_handle, &[]));
    let mut template = sealed();
    template.auth_policy = Reader::new(params(&digest, 0)).tpm2b(32).unwrap().to_vec();
    let (object, name, _) = make_child(&mut t, parent, &template, PASSWORD, PLAIN);
    let mut command = hc(cc::UNSEAL, &[object], Vec::new(), (object, PASSWORD));
    command.names = vec![name];
    command.attrs = session_attr::CONTINUE_SESSION | session_attr::ENCRYPT;

    // Check the response HMAC over ciphertext, then decrypt with the returned nonce.
    let response = run_hmac(&mut t, &mut session, &command);
    let ciphertext = Reader::new(params(&response, 0)).tpm2b(64).unwrap();
    assert_ne!(ciphertext, PLAIN);
    assert_eq!(
        crypt(
            &AES_SYM,
            &session,
            PASSWORD,
            &session.nonce_tpm,
            &command.nonce_caller,
            ciphertext,
            false
        ),
        PLAIN
    );
    let digest = run(&mut t, plain(cc::POLICY_GET_DIGEST, &policy_handle, &[]));
    assert_eq!(params(&digest, 0), tpm2b(&[0; 32]));
}

#[test]
fn encryption_attributes_are_checked_against_the_session_and_the_command() {
    let (mut t, idx) = nv_with_owner_pw();
    // Written first: a read of an unwritten index fails before the sessions are looked at.
    ok(&nv_write(&mut t, OWNER, idx, OWNER_PW, b"seed", 0));
    let name = nv_read_public(&mut t, idx).1;
    let s_null = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x44; 16]);
    let s_aes = start_sym(&mut t, NULL, b"", &AES_SYM);
    let data = cat(&[&tpm2b(PLAIN), &u16b(0)]);
    // A session without a symmetric algorithm cannot encrypt or decrypt.
    for bits in [session_attr::DECRYPT, session_attr::ENCRYPT] {
        let c = nv_write_command(
            &mut t,
            idx,
            data.clone(),
            session_attr::CONTINUE_SESSION | bits,
        );
        err(
            &run(&mut t, hmac_command(&s_null, &c, false, false)),
            Rc::SYMMETRIC.in_session(1),
        );
    }
    // NV_Write returns no parameter: `encrypt` does not apply.
    let c = nv_write_command(
        &mut t,
        idx,
        data.clone(),
        session_attr::CONTINUE_SESSION | session_attr::ENCRYPT,
    );
    err(
        &run(&mut t, hmac_command(&s_aes, &c, false, false)),
        Rc::ATTRIBUTES.in_session(1),
    );
    // NV_Read starts with a size, not a sized buffer: `decrypt` does not apply.
    let mut r = hc(
        cc::NV_READ,
        &[OWNER, idx],
        cat(&[&u16b(4), &u16b(0)]),
        (OWNER, OWNER_PW),
    );
    r.names = vec![name_of(OWNER), name.clone()];
    r.attrs = session_attr::CONTINUE_SESSION | session_attr::DECRYPT;
    err(
        &run(&mut t, hmac_command(&s_aes, &r, false, false)),
        Rc::ATTRIBUTES.in_session(1),
    );
    // A password session carries no encryption attribute at all.
    let mut area = Vec::new();
    area.extend_from_slice(&handle::RS_PW.to_be_bytes());
    area.extend_from_slice(&tpm2b(&[]));
    area.push(1 | session_attr::DECRYPT);
    area.extend_from_slice(&tpm2b(OWNER_PW));
    let mut auth = (area.len() as u32).to_be_bytes().to_vec();
    auth.extend_from_slice(&area);
    err(
        &run(
            &mut t,
            frame(st::SESSIONS, cc::NV_WRITE, &[OWNER, idx], &auth, &data),
        ),
        Rc::ATTRIBUTES.in_session(1),
    );
    // Nothing was written by any of these.
    assert_eq!(stored(&mut t, idx, 4), b"seed");
}

#[test]
fn only_one_session_of_a_command_may_decrypt_and_only_one_encrypt() {
    // `TPM2_Certify` has two authorization sessions; the formats are checked before the HMACs.
    let mut t = started();
    let s1 = start_sym(&mut t, NULL, b"", &AES_SYM);
    let s2 = start_sym(&mut t, NULL, b"", &XOR_SYM);
    let obj = parse_primary(&create_primary(&mut t, PLATFORM, b"opw", &ecc_sign()));
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let params = cat(&[&tpm2b(b"q"), &u16b(alg::NULL)]);
    let two = |a1: u8, a2: u8| {
        let mut area = Vec::new();
        for (s, a) in [(&s1, a1), (&s2, a2)] {
            area.extend_from_slice(&s.handle.to_be_bytes());
            area.extend_from_slice(&tpm2b(&[0x33; 16]));
            area.push(a);
            area.extend_from_slice(&tpm2b(&[0; 32]));
        }
        let mut auth = (area.len() as u32).to_be_bytes().to_vec();
        auth.extend_from_slice(&area);
        frame(
            st::SESSIONS,
            cc::CERTIFY,
            &[obj.handle, key.handle],
            &auth,
            &params,
        )
    };
    for bit in [session_attr::DECRYPT, session_attr::ENCRYPT] {
        err(&run(&mut t, two(bit, bit)), Rc::ATTRIBUTES.in_session(2));
    }
    // One decrypt and one encrypt in different sessions pass the format checks (the HMAC of the
    // test frame is garbage: authorization fails afterwards, with the first session's code).
    let r = run(&mut t, two(session_attr::DECRYPT, session_attr::ENCRYPT));
    assert_ne!(rc(&r), Rc::ATTRIBUTES.in_session(2).0);
}

#[test]
fn first_certify_hmac_binds_other_parameter_encryption_sessions() {
    use session_attr::{CONTINUE_SESSION, DECRYPT, ENCRYPT};

    #[derive(Clone, Copy)]
    enum FirstAuth {
        Hmac,
        Password,
        PolicyHmac,
        PolicyEmpty,
    }
    const OBJECT_PASSWORD: &[u8] = b"objectpw";
    const SIGNER_PASSWORD: &[u8] = b"signerpw";
    const QUALIFYING: &[u8] = b"bound-certify";

    for (first_flags, second_flags, first_auth) in [
        (0, DECRYPT, FirstAuth::Hmac),
        (0, ENCRYPT, FirstAuth::Hmac),
        (0, DECRYPT | ENCRYPT, FirstAuth::Hmac),
        (DECRYPT, ENCRYPT, FirstAuth::Hmac),
        (ENCRYPT, DECRYPT, FirstAuth::Hmac),
        (DECRYPT | ENCRYPT, 0, FirstAuth::Hmac),
        (0, ENCRYPT, FirstAuth::Password),
        (0, ENCRYPT, FirstAuth::PolicyHmac),
        (0, ENCRYPT, FirstAuth::PolicyEmpty),
    ] {
        let mut t = started();
        let with_policy = matches!(first_auth, FirstAuth::PolicyHmac | FirstAuth::PolicyEmpty);
        let first_kind = if with_policy { se::POLICY } else { se::HMAC };
        let mut first = start_sym_kind(&mut t, NULL, b"", &AES_SYM, first_kind);
        let mut second = start_sym(&mut t, NULL, b"", &XOR_SYM);
        let mut object_template = ecc_sign();
        if with_policy {
            if matches!(first_auth, FirstAuth::PolicyHmac) {
                ok(&run(
                    &mut t,
                    plain(cc::POLICY_AUTH_VALUE, &[first.handle], &[]),
                ));
            }
            ok(&run(
                &mut t,
                plain(cc::POLICY_COMMAND_CODE, &[first.handle], &u32b(cc::CERTIFY)),
            ));
            let response = run(&mut t, plain(cc::POLICY_GET_DIGEST, &[first.handle], &[]));
            object_template.auth_policy = Reader::new(params(&response, 0))
                .tpm2b(32)
                .unwrap()
                .to_vec();
        }
        let object = parse_primary(&create_primary(
            &mut t,
            PLATFORM,
            OBJECT_PASSWORD,
            &object_template,
        ));
        let key = parse_primary(&create_primary(
            &mut t,
            PLATFORM,
            SIGNER_PASSWORD,
            &ecc_sign(),
        ));
        let handles = [object.handle, key.handle];
        let mut first_command = hc(
            cc::CERTIFY,
            &handles,
            Vec::new(),
            (object.handle, OBJECT_PASSWORD),
        );
        let mut second_command = hc(
            cc::CERTIFY,
            &handles,
            Vec::new(),
            (key.handle, SIGNER_PASSWORD),
        );
        first_command.attrs = CONTINUE_SESSION | first_flags;
        second_command.attrs = CONTINUE_SESSION | second_flags;
        second_command.nonce_caller.fill(0x25);
        let names = vec![object.name, key.name];
        let qualifying = if first_flags & DECRYPT != 0 {
            crypt(
                &AES_SYM,
                &first,
                OBJECT_PASSWORD,
                &first_command.nonce_caller,
                &first.nonce_tpm,
                QUALIFYING,
                true,
            )
        } else if second_flags & DECRYPT != 0 {
            crypt(
                &XOR_SYM,
                &second,
                SIGNER_PASSWORD,
                &second_command.nonce_caller,
                &second.nonce_tpm,
                QUALIFYING,
                true,
            )
        } else {
            QUALIFYING.to_vec()
        };
        let wire = cat(&[&tpm2b(&qualifying), &u16b(alg::NULL)]);
        first_command.names = names.clone();
        second_command.names = names;
        first_command.params = wire.clone();
        second_command.params = wire.clone();

        let frame_with_binding = |binding: &[&[u8]]| {
            let mut area = match first_auth {
                FirstAuth::Password => cat(&[
                    &u32b(handle::RS_PW),
                    &tpm2b(&[]),
                    &[0],
                    &tpm2b(OBJECT_PASSWORD),
                ]),
                _ => hmac_auth_entry(
                    &first,
                    &first_command,
                    binding,
                    matches!(first_auth, FirstAuth::PolicyEmpty),
                    false,
                ),
            };
            area.extend_from_slice(&hmac_auth_entry(
                &second,
                &second_command,
                &[],
                false,
                false,
            ));
            let area_size = u32b(area.len() as u32);
            frame(
                st::SESSIONS,
                cc::CERTIFY,
                &handles,
                &cat(&[&area_size, &area]),
                &wire,
            )
        };
        let mut binding = Vec::new();
        if second_flags != 0 && matches!(first_auth, FirstAuth::Hmac | FirstAuth::PolicyHmac) {
            err(
                &run(&mut t, frame_with_binding(&[])),
                Rc::BAD_AUTH.in_session(1),
            );
            if second_flags == DECRYPT | ENCRYPT {
                let duplicated = [second.nonce_tpm.as_slice(); 2];
                err(
                    &run(&mut t, frame_with_binding(&duplicated)),
                    Rc::BAD_AUTH.in_session(1),
                );
            }
            binding.push(second.nonce_tpm.as_slice());
        }
        let reply = run(&mut t, frame_with_binding(&binding));
        let (wire_params, ack) = split_response(&reply, 0);
        assert_eq!(ack.len(), 2);
        if matches!(first_auth, FirstAuth::Hmac | FirstAuth::PolicyHmac) {
            check_hmac_ack(&mut first, &first_command, &wire_params, &ack[0]);
        } else {
            assert!(ack[0].2.is_empty());
        }
        check_hmac_ack(&mut second, &second_command, &wire_params, &ack[1]);
        let mut reader = Reader::new(&wire_params);
        let parameter = reader.tpm2b(1024).unwrap();
        let attestation = if first_flags & ENCRYPT != 0 {
            crypt(
                &AES_SYM,
                &first,
                OBJECT_PASSWORD,
                &first.nonce_tpm,
                &first_command.nonce_caller,
                parameter,
                false,
            )
        } else if second_flags & ENCRYPT != 0 {
            crypt(
                &XOR_SYM,
                &second,
                SIGNER_PASSWORD,
                &second.nonce_tpm,
                &second_command.nonce_caller,
                parameter,
                false,
            )
        } else {
            parameter.to_vec()
        };
        assert!(
            attestation
                .windows(QUALIFYING.len())
                .any(|window| window == QUALIFYING)
        );
        let signature = reader.bytes(reader.remaining()).unwrap();
        ok(&verify(
            &mut t,
            key.handle,
            &h256(&[&attestation]),
            signature,
        ));
    }
}

#[test]
fn additional_session_encrypts_only_the_shared_ecdh_point() {
    for sym in [&AES_SYM[..], &XOR_SYM[..]] {
        let mut t = started();
        ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
        let mut session = start_sym(&mut t, OWNER, OWNER_PW, sym);
        let mut public = ecc(oa::SIGN, SymDef::Null, ECDSA);
        // With the generator as the input public key, zPoint == pubPoint.
        public.unique = Unique::Ecc {
            x: hex("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296"),
            y: hex("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5"),
        };
        let (key, name) = parse_loaded(&load_external(&mut t, &public, NULL));
        let before = t.persistent_snapshot();

        let mut command = hc(cc::ECDH_KEY_GEN, &[key], Vec::new(), (NULL, b""));
        command.attrs |= session_attr::ENCRYPT;
        err(
            &run(&mut t, hmac_command(&session, &command, false, false)),
            Rc::BAD_AUTH.in_session(1), // A handle number is not the Name of the object.
        );
        command.names = vec![name];
        err(
            &run(&mut t, hmac_command(&session, &command, false, true)),
            Rc::BAD_AUTH.in_session(1),
        );

        command.attrs |= session_attr::DECRYPT;
        err(
            &run(&mut t, hmac_command(&session, &command, false, false)),
            Rc::ATTRIBUTES.in_session(1),
        );
        command.attrs = session_attr::ENCRYPT; // A failing command did not roll the nonce.
        let response = run_hmac(&mut t, &mut session, &command);
        let mut reader = Reader::new(params(&response, 0));
        let shared = reader.tpm2b(68).unwrap();
        let public = reader.tpm2b(68).unwrap();
        reader.finish().unwrap();
        assert_eq!(public.len(), 68);
        assert_ne!(shared, public);
        assert_eq!(
            crypt(
                sym,
                &session,
                b"",
                &session.nonce_tpm,
                &command.nonce_caller,
                shared,
                false
            ),
            public
        );
        err(
            &run(&mut t, hmac_command(&session, &command, false, false)),
            Rc::reference_session(0),
        );
        assert_eq!(t.persistent_snapshot(), before); // Extra HMACs do not touch DA.
    }
}

#[test]
fn additional_policy_session_keeps_its_assertions_and_trial_cannot_encrypt() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let mut policy = start_sym_kind(&mut t, OWNER, OWNER_PW, &AES_SYM, se::POLICY);
    let digest = plain(cc::POLICY_GET_DIGEST, &[policy.handle], &[]);
    ok(&run(
        &mut t,
        plain(
            cc::POLICY_COMMAND_CODE,
            &[policy.handle],
            &u32b(cc::CERTIFY),
        ),
    ));
    let before = run(&mut t, digest.clone());
    let mut command = hc(cc::GET_RANDOM, &[], u16b(8).to_vec(), (NULL, b""));
    command.attrs |= session_attr::ENCRYPT;
    ok(&run_hmac(&mut t, &mut policy, &command));
    assert_eq!(run(&mut t, digest), before);

    let trial = start_sym_kind(&mut t, OWNER, OWNER_PW, &AES_SYM, se::TRIAL);
    err(
        &run(&mut t, hmac_command(&trial, &command, false, false)),
        Rc::ATTRIBUTES.in_session(1),
    );
    command.attrs = session_attr::CONTINUE_SESSION;
    err(
        &run(&mut t, hmac_command(&policy, &command, false, false)),
        Rc::ATTRIBUTES.in_session(1),
    );
}

#[test]
fn third_session_encrypts_a_response_after_two_authorizations() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let object = parse_primary(&create_primary(&mut t, PLATFORM, b"object", &ecc_sign()));
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"key", &ecc_sign()));
    let mut first = start_sym(&mut t, NULL, b"", &XOR_SYM);
    let mut extra = start_sym(&mut t, OWNER, OWNER_PW, &AES_SYM);
    let mut command = hc(
        cc::CERTIFY,
        &[object.handle, key.handle],
        cat(&[&tpm2b(b"qualified"), &u16b(alg::NULL)]),
        (object.handle, b"object"),
    );
    command.names = vec![object.name, key.name];
    let mut encrypt = hc(
        command.code,
        &command.handles,
        command.params.clone(),
        (NULL, b""),
    );
    encrypt.names = command.names.clone();
    encrypt.attrs |= session_attr::ENCRYPT;
    encrypt.nonce_caller.fill(0x55);
    let first_auth = hmac_auth_entry(&first, &command, &[&extra.nonce_tpm], false, false);
    let password = auth_area(&[b"key"]);
    let extra_auth = hmac_auth_entry(&extra, &encrypt, &[], false, false);
    let request = |with_extra: bool| {
        let mut area = cat(&[&first_auth, &password[4..]]);
        if with_extra {
            area.extend_from_slice(&extra_auth);
        }
        let prefix = u32b(area.len() as u32);
        frame(
            st::SESSIONS,
            command.code,
            &command.handles,
            &cat(&[&prefix, &area]),
            &command.params,
        )
    };
    // The first HMAC commits to the extra session nonce.
    err(&run(&mut t, request(false)), Rc::BAD_AUTH.in_session(1));
    let response = run(&mut t, request(true));
    let (parameters, acks) = split_response(&response, 0);
    assert_eq!(acks.len(), 3);
    check_hmac_ack(&mut first, &command, &parameters, &acks[0]);
    assert!(acks[1].0.is_empty() && acks[1].2.is_empty());
    check_hmac_ack(&mut extra, &encrypt, &parameters, &acks[2]);
    let ciphertext = Reader::new(&parameters).tpm2b(1024).unwrap();
    let attest = crypt(
        &AES_SYM,
        &extra,
        b"",
        &extra.nonce_tpm,
        &encrypt.nonce_caller,
        ciphertext,
        false,
    );
    assert_eq!(&attest[..4], &0xff54_4347u32.to_be_bytes());
}

#[test]
fn additional_session_decrypts_the_command_and_encrypts_the_response() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let mut session = start_sym(&mut t, OWNER, OWNER_PW, &AES_SYM);
    let mut command = hc(cc::HASH, &[], Vec::new(), (NULL, b""));
    command.attrs |= session_attr::DECRYPT | session_attr::ENCRYPT;
    let input = crypt(
        &AES_SYM,
        &session,
        b"",
        &command.nonce_caller,
        &session.nonce_tpm,
        PLAIN,
        true,
    );
    command.params = cat(&[&tpm2b(&input), &u16b(alg::SHA256), &u32b(NULL)]);
    let response = run_hmac(&mut t, &mut session, &command);
    let digest = Reader::new(params(&response, 0)).tpm2b(32).unwrap();
    assert_eq!(
        crypt(
            &AES_SYM,
            &session,
            b"",
            &session.nonce_tpm,
            &command.nonce_caller,
            digest,
            false
        ),
        h256(&[PLAIN])
    );
}

#[test]
fn encrypted_set_primary_policy_stores_the_plaintext_digest() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let mut session = start_sym(&mut t, OWNER, OWNER_PW, &AES_SYM);
    let mut command = hc(
        cc::SET_PRIMARY_POLICY,
        &[OWNER],
        Vec::new(),
        (OWNER, OWNER_PW),
    );
    command.attrs |= session_attr::DECRYPT;
    let digest = [0x53; 32];
    let ciphertext = crypt(
        &AES_SYM,
        &session,
        OWNER_PW,
        &command.nonce_caller,
        &session.nonce_tpm,
        &digest,
        true,
    );
    assert_ne!(ciphertext, digest);
    command.params = cat(&[&tpm2b(&ciphertext), &u16b(alg::SHA256)]);
    ok(&run_hmac(&mut t, &mut session, &command));
    assert_eq!(t.permanent_auth_policy(OWNER), Some(&digest));
}

#[test]
fn additional_decrypt_session_protects_policy_cp_hash() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let mut protection = start_sym(&mut t, OWNER, OWNER_PW, &AES_SYM);
    let policy = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
    let mut command = hc(
        cc::POLICY_CP_HASH,
        &[policy.handle],
        Vec::new(),
        (NULL, b""),
    );
    command.attrs |= session_attr::DECRYPT;
    let hash = [0x37; 32];
    let ciphertext = crypt(
        &AES_SYM,
        &protection,
        b"",
        &command.nonce_caller,
        &protection.nonce_tpm,
        &hash,
        true,
    );
    assert_ne!(ciphertext, hash);
    command.params = tpm2b(&ciphertext);
    ok(&run_hmac(&mut t, &mut protection, &command));
    let r = run(&mut t, plain(cc::POLICY_GET_DIGEST, &[policy.handle], &[]));
    assert_eq!(
        Reader::new(params(&r, 0)).tpm2b(32).unwrap(),
        h256(&[&[0; 32], &[0, 0, 1, 0x6e], &hash])
    );
}

#[test]
fn additional_session_protects_start_auth_session_nonces() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWNER_PW));
    let mut protection = start_sym(&mut t, OWNER, OWNER_PW, &AES_SYM);
    let mut command = hc(
        cc::START_AUTH_SESSION,
        &[NULL, NULL],
        Vec::new(),
        (NULL, b""),
    );
    command.attrs |= session_attr::DECRYPT | session_attr::ENCRYPT;
    let nonce_caller = [0x63; 16];
    let nonce_ciphertext = crypt(
        &AES_SYM,
        &protection,
        b"",
        &command.nonce_caller,
        &protection.nonce_tpm,
        &nonce_caller,
        true,
    );
    command.params = start_params(&nonce_ciphertext, b"", se::HMAC, &XOR_SYM, alg::SHA256);
    let response = run(&mut t, hmac_command(&protection, &command, false, false));
    let handle = u32::from_be_bytes(response[10..14].try_into().unwrap());
    assert_ne!(handle, protection.handle);
    let (parameters, acks) = split_response(&response, 1);
    assert_eq!(acks.len(), 1);
    check_hmac_ack(&mut protection, &command, &parameters, &acks[0]);
    let ciphertext = Reader::new(&parameters).tpm2b(16).unwrap();
    let new_nonce = crypt(
        &AES_SYM,
        &protection,
        b"",
        &protection.nonce_tpm,
        &command.nonce_caller,
        ciphertext,
        false,
    );
    let slot = t.sessions.slot_of(handle).unwrap();
    assert_eq!(t.sessions.get(slot).unwrap().nonce_tpm, new_nonce);
}

#[test]
fn get_test_result_allows_encrypt_and_rejects_decrypt() {
    let mut t = started();
    let mut session = start_sym(&mut t, NULL, b"", &AES_SYM);
    let mut command = hc(cc::GET_TEST_RESULT, &[], Vec::new(), (NULL, b""));
    command.attrs |= session_attr::DECRYPT;
    err(
        &run(&mut t, hmac_command(&session, &command, false, false)),
        Rc::ATTRIBUTES.in_session(1),
    );
    command.attrs = session_attr::CONTINUE_SESSION | session_attr::ENCRYPT;
    let response = run_hmac(&mut t, &mut session, &command);
    // An empty TPM2B outData still has a clear size. The following result stays clear too.
    assert_eq!(params(&response, 0), [0; 6]);
}

#[test]
fn a_saved_and_loaded_session_keeps_its_symmetric_algorithm() {
    let (mut t, idx) = nv_with_owner_pw();
    let mut s = start_sym(&mut t, NULL, b"", &AES_SYM);
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[s.handle], &[]));
    ok(&saved);
    let ctx = params(&saved, 0).to_vec();
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &ctx));
    ok(&loaded);
    assert_eq!(
        u32::from_be_bytes([loaded[10], loaded[11], loaded[12], loaded[13]]),
        s.handle
    );
    let c0 = nv_write_command(&mut t, idx, Vec::new(), 1);
    let ct = crypt(
        &AES_SYM,
        &s,
        OWNER_PW,
        &c0.nonce_caller,
        &s.nonce_tpm,
        PLAIN,
        true,
    );
    let c = nv_write_command(
        &mut t,
        idx,
        cat(&[&tpm2b(&ct), &u16b(0)]),
        session_attr::CONTINUE_SESSION | session_attr::DECRYPT,
    );
    ok(&run_hmac(&mut t, &mut s, &c));
    assert_eq!(stored(&mut t, idx, PLAIN.len() as u16), PLAIN);
}

// ------------------------------------------------------------------------------- misc

#[test]
fn capability_reports_the_new_commands_with_their_attributes() {
    let mut t = started();
    let attr_of = |t: &mut Tpm, code: u32| {
        let (_, _, body, count) = cap_list(&get_cap(t, cap::COMMANDS, code, 1));
        assert_eq!(count, 1);
        u32::from_be_bytes([body[0], body[1], body[2], body[3]])
    };
    for code in [0x134u32, 0x135, 0x136] {
        // {NV}, two handles, no response handle.
        assert_eq!(attr_of(&mut t, code), code | 1 << 22 | 2 << 25);
    }
    assert_eq!(attr_of(&mut t, 0x14C), 0x14C | 2 << 25);
}

#[test]
fn malformed_slice_c3_commands_never_panic() {
    let (mut t, idx) = nv_with_owner_pw();
    ok(&define_raw(
        &mut t,
        OWNER,
        OWNER_PW,
        b"",
        &nv_public(NV0 + 2, alg::SHA256, OWNER_RW | NT_COUNTER, &[], 8),
    ));
    let s = start_sym(&mut t, NULL, b"", &AES_SYM);
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"kpw", &ecc_sign()));
    let mut cmds: Vec<Vec<u8>> = Vec::new();
    let bodies = [
        (cc::NV_INCREMENT, vec![OWNER, NV0 + 2], Vec::new()),
        (cc::NV_SET_BITS, vec![OWNER, idx], vec![0u8; 8]),
        (cc::NV_EXTEND, vec![OWNER, idx], tpm2b(b"abc")),
        (
            cc::GET_TIME,
            vec![ENDORSEMENT, key.handle],
            cat(&[&tpm2b(b"n"), &u16b(alg::NULL)]),
        ),
        (
            cc::GET_TIME,
            vec![ENDORSEMENT, NULL],
            cat(&[&tpm2b(b"n"), &ecc_sig_scheme()]),
        ),
    ];
    for (code, handles, params) in &bodies {
        for cut in 0..=params.len() {
            cmds.push(with_pw(*code, handles, &[OWNER_PW, b"kpw"], &params[..cut]));
        }
        let mut long = params.clone();
        long.extend_from_slice(&[0xFF; 5]);
        cmds.push(with_pw(*code, handles, &[OWNER_PW, b"kpw"], &long));
        // Every prefix of the whole command, and with the HMAC session of an AES session.
        let whole = with_pw(*code, handles, &[OWNER_PW, b"kpw"], params);
        for cut in 0..whole.len() {
            cmds.push(whole[..cut].to_vec());
        }
    }
    let _ = s;
    for c in cmds {
        let r = t.execute(0, &c, 0);
        assert!(r.len() >= 10);
    }
}

#[test]
fn increment_overflow_is_nv_range_and_changes_nothing() {
    // The counter is at the maximum: restore a snapshot that holds u64::MAX as a counter and as
    // the maximum, then try to count on.
    let mut t = started();
    ok(&def(&mut t, NV0 + 1, OWNER_RW | NT_COUNTER, 8));
    ok(&increment(&mut t, NV0 + 1));
    shutdown(&mut t, 0);
    let mut snap = t.persistent_snapshot();
    // The data of the counter is the 8 octets after its `lv16` length; the maximum is just before
    // the object count.
    let at = offset_of(&snap, &u32b(NV0 + 1));
    let data_at = snap[at..]
        .windows(10)
        .position(|w| w == [0, 8, 0, 0, 0, 0, 0, 0, 0, 1])
        .map(|p| at + p + 2)
        .expect("counter data");
    snap[data_at..data_at + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    // Empty policies, reset count, bank mask, establishment and seed extensions follow the table.
    let max_at = snap.len() - 32 - 96 - 1 - 1 - 8 - 6 - 1 - 8;
    snap[max_at..max_at + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    refix(&mut snap);
    let mut r = Tpm::restore(&snap, &[9; 48]).unwrap();
    startup(&mut r, 0);
    assert_eq!(counter_value(&mut r, NV0 + 1), u64::MAX);
    let (_, name_before) = nv_read_public(&mut r, NV0 + 1);
    err(&increment(&mut r, NV0 + 1), Rc::NV_RANGE);
    assert_eq!(counter_value(&mut r, NV0 + 1), u64::MAX);
    assert_eq!(nv_read_public(&mut r, NV0 + 1).1, name_before);
    // A new counter cannot start above the maximum either.
    ok(&def(&mut r, NV0 + 2, OWNER_RW | NT_COUNTER, 8));
    err(&increment(&mut r, NV0 + 2), Rc::NV_RANGE);
}
