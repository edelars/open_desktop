//! Command-level tests: every command through `Tpm::execute` with raw command bytes.
//!
//! Test vectors marked "computed independently" were produced with Python `hashlib` (2026-11-07),
//! not with this crate. Response codes are compared with the numeric values of
//! TPM 2.0 Library Part 2 v184 (`codes.rs`).

use crate::codes::{alg, cap, cc, handle, pt, st};
use crate::dispatch::COMMANDS;
use crate::marshal::{
    Reader, TpmlDigest, TpmlDigestValues, TpmlPcrSelection, TpmsTimeInfo, TpmtTkHashcheck, limits,
};
use crate::{Rc, Tpm};

#[path = "tests_b.rs"]
mod tests_b;
#[path = "tests_c1.rs"]
mod tests_c1;
#[path = "tests_c2.rs"]
mod tests_c2;
#[path = "tests_c3.rs"]
mod tests_c3;
#[path = "tests_ek.rs"]
mod tests_ek;
#[path = "tests_obj.rs"]
mod tests_obj;
#[path = "tests_session_audit.rs"]
mod tests_session_audit;

const ENTROPY: [u8; 48] = [5; 48];

// Computed independently (Python hashlib).
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA384_ABC: &str = "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7";
const SHA384_EXTEND_ABC_ON_ZERO: &str = "93732e3733514a841c982cfa75ea76ab55fe011acb9cd980ef4523913c65be1b0998e04d77f8c174f81a82151619ca40";
const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// sha256(32 x 0x00 || sha256("abc"))
const EXTEND_ABC_ON_ZERO: &str = "589f9ffed4c477966bfb8d41f37895b08c69047df8f911d6f3b57fbe08faee8d";
/// sha256(32 x 0xFF || sha256("abc"))
const EXTEND_ABC_ON_FF: &str = "ded4cee9953bb84c83278424b1e8256ee3483023f4ae5730affa51aad0063efb";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn auth_area(passwords: &[&[u8]]) -> Vec<u8> {
    let mut area = Vec::new();
    for p in passwords {
        area.extend_from_slice(&handle::RS_PW.to_be_bytes());
        area.extend_from_slice(&0u16.to_be_bytes());
        area.push(1);
        area.extend_from_slice(&(p.len() as u16).to_be_bytes());
        area.extend_from_slice(p);
    }
    let mut v = (area.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(&area);
    v
}

/// Command without sessions.
fn plain(code: u32, handles: &[u32], params: &[u8]) -> Vec<u8> {
    frame(st::NO_SESSIONS, code, handles, &[], params)
}

/// Command with password sessions (one per entry of `passwords`).
fn with_pw(code: u32, handles: &[u32], passwords: &[&[u8]], params: &[u8]) -> Vec<u8> {
    frame(st::SESSIONS, code, handles, &auth_area(passwords), params)
}

fn frame(tag: u16, code: u32, handles: &[u32], auth: &[u8], params: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for h in handles {
        body.extend_from_slice(&h.to_be_bytes());
    }
    body.extend_from_slice(auth);
    body.extend_from_slice(params);
    let mut v = Vec::new();
    v.extend_from_slice(&tag.to_be_bytes());
    v.extend_from_slice(&((10 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(&code.to_be_bytes());
    v.extend_from_slice(&body);
    v
}

/// Moves the TPM clock (`Time`) forward by `secs` seconds; the next command sees it.
fn pass_time(t: &mut Tpm, secs: u64) {
    t.power_ms += secs * 1000;
}

fn tpm_new() -> Tpm {
    Tpm::new(&ENTROPY).unwrap()
}

fn started() -> Tpm {
    let mut t = tpm_new();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    t
}

fn run(t: &mut Tpm, cmd: Vec<u8>) -> Vec<u8> {
    let r = t.execute(0, &cmd, 0);
    // Every response is well-formed: known tag and responseSize equal to the buffer length.
    assert!(r.len() >= 10);
    let tag = u16::from_be_bytes([r[0], r[1]]);
    assert!(tag == st::NO_SESSIONS || tag == st::SESSIONS);
    assert_eq!(
        u32::from_be_bytes([r[2], r[3], r[4], r[5]]) as usize,
        r.len()
    );
    assert!(r.len() <= limits::MAX_RESPONSE_SIZE);
    r
}

fn rc(resp: &[u8]) -> u32 {
    u32::from_be_bytes([resp[6], resp[7], resp[8], resp[9]])
}

fn ok(resp: &[u8]) {
    assert_eq!(rc(resp), 0, "expected success, got {:#x}", rc(resp));
}

fn err(resp: &[u8], expect: Rc) {
    assert_eq!(
        rc(resp),
        expect.0,
        "expected {:#x}, got {:#x}",
        expect.0,
        rc(resp)
    );
    assert_eq!(resp.len(), 10, "an error response is exactly the header");
    assert_eq!(u16::from_be_bytes([resp[0], resp[1]]), st::NO_SESSIONS);
}

/// Parameters of a successful response (after the handle area `nh` handles).
fn params(resp: &[u8], nh: usize) -> &[u8] {
    ok(resp);
    let tag = u16::from_be_bytes([resp[0], resp[1]]);
    let mut at = 10 + 4 * nh;
    if tag == st::SESSIONS {
        let n = u32::from_be_bytes([resp[at], resp[at + 1], resp[at + 2], resp[at + 3]]) as usize;
        at += 4;
        return &resp[at..at + n];
    }
    &resp[at..]
}

fn u16b(v: u16) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

fn u32b(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

fn tpm2b(d: &[u8]) -> Vec<u8> {
    let mut v = u16b(d.len() as u16);
    v.extend_from_slice(d);
    v
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn sha256_ha(digest: &[u8]) -> Vec<u8> {
    cat(&[&u32b(1), &u16b(alg::SHA256), digest])
}

fn extend(t: &mut Tpm, pcr: u32, digest: &[u8]) -> Vec<u8> {
    run(
        t,
        with_pw(cc::PCR_EXTEND, &[pcr], &[b""], &sha256_ha(digest)),
    )
}

fn read_pcr(t: &mut Tpm, pcr: usize) -> Vec<u8> {
    read_pcr_bank(t, pcr, alg::SHA256)
}

fn read_pcr_bank(t: &mut Tpm, pcr: usize, hash: u16) -> Vec<u8> {
    let mut sel = [0u8; 3];
    sel[pcr / 8] |= 1 << (pcr % 8);
    let r = run(
        t,
        plain(
            cc::PCR_READ,
            &[],
            &cat(&[&u32b(1), &u16b(hash), &[3], &sel]),
        ),
    );
    let mut rd = Reader::new(params(&r, 0));
    let _counter = rd.u32().unwrap();
    TpmlPcrSelection::unmarshal(&mut rd).unwrap();
    let d = TpmlDigest::unmarshal(&mut rd).unwrap();
    rd.finish().unwrap();
    assert_eq!(d.digests.len(), 1);
    d.digests[0].clone()
}

fn pcr_counter(t: &mut Tpm) -> u32 {
    let r = run(t, plain(cc::PCR_READ, &[], &u32b(0)));
    Reader::new(params(&r, 0)).u32().unwrap()
}

fn clock(t: &mut Tpm, now: u64) -> TpmsTimeInfo {
    let r = t.execute(0, &plain(cc::READ_CLOCK, &[], &[]), now);
    TpmsTimeInfo::unmarshal(&mut Reader::new(params(&r, 0))).unwrap()
}

// ---------------------------------------------------------------- framing and profile

#[test]
fn malformed_headers_are_rejected_without_panic() {
    let mut t = started();
    err(&t.execute(0, &[], 0), Rc::COMMAND_SIZE);
    err(&t.execute(0, &[0x80, 0x01, 0, 0, 0], 0), Rc::COMMAND_SIZE);
    err(
        &t.execute(0, &plain(cc::GET_RANDOM, &[], &[0, 1])[..11], 0),
        Rc::COMMAND_SIZE,
    );
    let mut bad_tag = plain(cc::GET_RANDOM, &[], &[0, 1]);
    bad_tag[0] = 0x80;
    bad_tag[1] = 0x03;
    err(&t.execute(0, &bad_tag, 0), Rc::TAG);
    let mut bad_size = plain(cc::GET_RANDOM, &[], &[0, 1]);
    bad_size[5] += 1;
    err(&t.execute(0, &bad_size, 0), Rc::COMMAND_SIZE);
    err(
        &t.execute(0, &vec![0u8; limits::MAX_COMMAND_SIZE + 1], 0),
        Rc::COMMAND_SIZE,
    );
}

#[test]
fn locality_other_than_zero_is_refused() {
    let mut t = started();
    for l in 1..=4 {
        err(
            &t.execute(l, &plain(cc::GET_RANDOM, &[], &[0, 1]), 0),
            Rc::LOCALITY,
        );
    }
}

#[test]
fn commands_outside_the_profile_return_command_code_not_success() {
    // Every code of the Part 2 table (0x11F..=0x1A1) that is not in COMMANDS, before and after
    // Startup, answers TPM_RC_COMMAND_CODE; the implemented ones never do.
    let implemented: Vec<u32> = COMMANDS.iter().map(|c| c.code).collect();
    for startup in [false, true] {
        let mut t = if startup { started() } else { tpm_new() };
        for code in 0x11Fu32..=0x1A1 {
            let r = run(&mut t, plain(code, &[], &[]));
            if implemented.contains(&code) {
                assert_ne!(rc(&r), Rc::COMMAND_CODE.0, "{code:#x} must be dispatched");
            } else {
                err(&r, Rc::COMMAND_CODE);
            }
        }
        // Outside the table range too.
        err(&run(&mut t, plain(0x0000_0000, &[], &[])), Rc::COMMAND_CODE);
        err(&run(&mut t, plain(0xFFFF_FFFF, &[], &[])), Rc::COMMAND_CODE);
    }
}

#[test]
fn every_command_before_startup_is_initialize() {
    let mut t = tpm_new();
    for c in COMMANDS.iter().filter(|c| c.code != cc::STARTUP) {
        err(&run(&mut t, plain(c.code, &[], &[])), Rc::INITIALIZE);
    }
}

#[test]
fn fuzzed_commands_never_panic_and_always_answer_well_formed() {
    let seeds = [
        plain(cc::STARTUP, &[], &[0, 0]),
        with_pw(cc::PCR_EXTEND, &[3], &[b""], &sha256_ha(&[1; 32])),
        plain(
            cc::GET_CAPABILITY,
            &[],
            &cat(&[&u32b(cap::COMMANDS), &u32b(0), &u32b(100)]),
        ),
        plain(
            cc::HASH,
            &[],
            &cat(&[&tpm2b(b"abc"), &u16b(alg::SHA256), &u32b(handle::RH_NULL)]),
        ),
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b"x"), &u16b(alg::NULL)]),
        ),
        with_pw(
            cc::EVENT_SEQUENCE_COMPLETE,
            &[8, 0x8000_0000],
            &[b"", b""],
            &tpm2b(b"data"),
        ),
        plain(
            cc::PCR_READ,
            &[],
            &cat(&[&u32b(1), &u16b(alg::SHA256), &[3, 0xFF, 0xFF, 0xFF]]),
        ),
    ];
    let mut state = 0x1234_5678u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) as usize
    };
    let mut t = started();
    for seed in &seeds {
        for len in 0..=seed.len() {
            let _ = run(&mut t, seed[..len].to_vec());
        }
        for _ in 0..200 {
            let mut c = seed.clone();
            for _ in 0..1 + next() % 3 {
                let i = next() % c.len();
                c[i] = next() as u8;
            }
            // Keep commandSize consistent half of the time so the parameter parsers are reached.
            if next() % 2 == 0 {
                let n = c.len() as u32;
                c[2..6].copy_from_slice(&n.to_be_bytes());
            }
            let _ = run(&mut t, c);
        }
    }
}

// ---------------------------------------------------------------- Startup / Shutdown

#[test]
fn startup_clear_succeeds_once() {
    let mut t = tpm_new();
    let r = run(&mut t, plain(cc::STARTUP, &[], &[0, 0]));
    assert_eq!(r, [0x80, 0x01, 0, 0, 0, 10, 0, 0, 0, 0]);
    // A second Startup is TPM_RC_INITIALIZE.
    err(
        &run(&mut t, plain(cc::STARTUP, &[], &[0, 0])),
        Rc::INITIALIZE,
    );
}

#[test]
fn startup_errors() {
    let mut t = tpm_new();
    err(
        &run(&mut t, with_pw(cc::STARTUP, &[], &[b""], &[0, 0])),
        Rc::AUTH_CONTEXT,
    );
    // Unknown startupType: TPM_RC_VALUE with parameter 1 (0x1C4, Part 2 clause 6.6.2 example).
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 2])), Rc(0x1C4));
    // STATE without a preceding Shutdown(STATE).
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
    // Truncated and oversized parameter areas.
    err(
        &run(&mut t, plain(cc::STARTUP, &[], &[0])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0, 0])), Rc::SIZE);
    // After the failed attempts the TPM still starts.
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
}

#[test]
fn startup_flushes_sequences_and_resets_pcr() {
    let mut t = started();
    ok(&extend(&mut t, 4, &[1; 32]));
    let r = run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
        ),
    );
    ok(&r);
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert_eq!(read_pcr(&mut t, 4), vec![0u8; 32]);
    assert_eq!(read_pcr(&mut t, 17), vec![0xFFu8; 32]);
    let r = run(
        &mut t,
        with_pw(cc::SEQUENCE_UPDATE, &[0x8000_0000], &[b""], &tpm2b(b"x")),
    );
    err(&r, Rc::reference_handle(0));
}

#[test]
fn shutdown_state_then_startup_state_resumes_saved_pcr() {
    let mut t = started();
    let event = |pcr| with_pw(cc::PCR_EVENT, &[pcr], &[b""], &tpm2b(b"save both banks"));
    ok(&run(&mut t, event(0)));
    let counter = pcr_counter(&mut t);
    assert_eq!(counter, 2);
    ok(&run(&mut t, event(16)));
    let pcr0 = read_pcr(&mut t, 0);
    let pcr0_sha384 = read_pcr_bank(&mut t, 0, alg::SHA384);
    assert_ne!(read_pcr_bank(&mut t, 16, alg::SHA384), vec![0; 48]);
    assert_eq!(
        pcr_counter(&mut t),
        counter,
        "PCR 16 does not increment the counter"
    );
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])));
    // TPM_PT_PCR_SAVE PCR (0-15) are restored, the others take their initial value (PTP Table 14).
    assert_eq!(read_pcr(&mut t, 0), pcr0);
    assert_eq!(read_pcr(&mut t, 16), vec![0u8; 32]);
    assert_eq!(read_pcr(&mut t, 17), vec![0xFFu8; 32]);
    assert_eq!(read_pcr_bank(&mut t, 0, alg::SHA384), pcr0_sha384);
    assert_eq!(read_pcr_bank(&mut t, 16, alg::SHA384), vec![0; 48]);
    assert_eq!(read_pcr_bank(&mut t, 17, alg::SHA384), vec![0xff; 48]);
    assert_eq!(
        pcr_counter(&mut t),
        counter + 1,
        "Resume initializes counted PCR 17–20"
    );
    // Resume increments restartCount and leaves resetCount alone.
    let c = clock(&mut t, 0).clock_info;
    assert_eq!((c.reset_count, c.restart_count), (1, 1));
    // The saved state is consumed: Startup(STATE) cannot be repeated after another _TPM_Init.
    t.init();
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
}

#[test]
fn platform_hcrtm_preserves_startup_provenance_and_discards_an_interrupted_measurement() {
    let clear = plain(cc::STARTUP, &[], &[0, 0]);
    let resume = plain(cc::STARTUP, &[], &[0, 1]);
    let save = plain(cc::SHUTDOWN, &[], &[0, 1]);
    let mut t = Tpm::new(&[0x21; 48]).unwrap();
    t.platform_hash_start();
    t.platform_hash_data(b"ab");
    t.platform_hash_data(b"c");
    t.platform_hash_end(0);
    ok(&run(&mut t, clear.clone()));
    let measured = hex("15703cc929081671c587dad9b09606521a35aa6bf4741df448d22c4b307acc71");
    assert_eq!(read_pcr(&mut t, 0), measured);
    assert!(!t.platform_established());
    ok(&run(&mut t, save.clone()));
    t.init();
    err(&run(&mut t, resume.clone()), Rc::LOCALITY);
    // The repeated H-CRTM satisfies provenance. Resume restores PCR0 from Shutdown(STATE).
    t.platform_hash_start();
    t.platform_hash_data(b"other measurement");
    t.platform_hash_end(0);
    ok(&run(&mut t, resume));
    assert_eq!(read_pcr(&mut t, 0), measured);

    t.init();
    t.platform_hash_start();
    t.platform_hash_data(b"uncommitted");
    ok(&run(&mut t, clear));
    t.platform_hash_end(0);
    assert_eq!(read_pcr(&mut t, 0), vec![0; 32]);
    assert_eq!(read_pcr(&mut t, 17), vec![0xff; 32]);
    assert!(!t.platform_hash_active());
}

#[test]
fn platform_hcrtm_reserves_a_sequence_slot_and_persists_establishment() {
    let mut t = started();
    let sequence_handles: Vec<u32> = (0..3)
        .map(|_| seq_handle(&start_seq(&mut t, b"", alg::SHA256)))
        .collect();
    assert_eq!(t.sequences.loaded_handles(), sequence_handles);
    let counter = pcr_counter(&mut t);
    let restart_count = clock(&mut t, 0).clock_info.restart_count;
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.platform_hash_start();
    assert_eq!(t.sequences.loaded_handles(), sequence_handles[..2]);
    assert_eq!(t.sequences.free_slots(), 0);
    t.platform_hash_data(b"abc");
    t.platform_hash_end(0);
    let measured = hex("589f9ffed4c477966bfb8d41f37895b08c69047df8f911d6f3b57fbe08faee8d");
    assert_eq!(read_pcr(&mut t, 17), measured);
    for pcr in 18..=22 {
        assert_eq!(read_pcr(&mut t, pcr), vec![0; 32]);
        assert_eq!(read_pcr_bank(&mut t, pcr, alg::SHA384), vec![0; 48]);
    }
    assert_eq!(pcr_counter(&mut t), counter + 1);
    assert_eq!(clock(&mut t, 0).clock_info.restart_count, restart_count + 1);
    assert!(t.platform_established());
    let persisted = t.persistent_snapshot();
    let mut restored = Tpm::restore(&persisted, &[0x22; 48]).unwrap();
    assert!(restored.platform_established());
    restored.init();
    assert!(restored.platform_established());
    restored.platform_reset_establishment();
    assert!(!restored.platform_established());
    assert_eq!(t.sequences.free_slots(), 1);
    t.init();
    assert!(t.platform_established());
    err(
        &run(&mut t, plain(cc::STARTUP, &[], &[0, 1])),
        Rc::VALUE.in_param(1),
    );
}

#[test]
fn both_halves_of_the_null_seed_follow_reset_restart_and_resume() {
    let mut t = started();
    let initial = t.hv.null_seed.clone();
    assert_ne!(&initial[..32], &initial[32..]);
    for startup in [1, 0] {
        ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
        t.init();
        ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, startup])));
        assert_eq!(&t.hv.null_seed[..], &initial[..]);
    }
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert_ne!(&t.hv.null_seed[..32], &initial[..32]);
    assert_ne!(&t.hv.null_seed[32..], &initial[32..]);
}

#[test]
fn shutdown_state_then_startup_clear_is_restart() {
    let mut t = started();
    ok(&run(
        &mut t,
        with_pw(cc::PCR_EVENT, &[0], &[b""], &tpm2b(b"reset both banks")),
    ));
    let counter = pcr_counter(&mut t);
    assert_eq!(counter, 2);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert_eq!(read_pcr(&mut t, 0), vec![0u8; 32]);
    assert_eq!(read_pcr_bank(&mut t, 0, alg::SHA384), vec![0; 48]);
    assert_eq!(
        pcr_counter(&mut t),
        counter + 1,
        "Restart retains the saved base counter"
    );
    let c = clock(&mut t, 0).clock_info;
    assert_eq!((c.reset_count, c.restart_count), (1, 1));
}

#[test]
fn shutdown_clear_then_startup_clear_is_reset_and_state_is_refused() {
    let mut t = started();
    ok(&extend(&mut t, 0, &[1; 32]));
    assert_ne!(pcr_counter(&mut t), 0);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    t.init();
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert_eq!(pcr_counter(&mut t), 0);
    let c = clock(&mut t, 0).clock_info;
    assert_eq!((c.reset_count, c.restart_count), (2, 0));
}

#[test]
fn pcr_change_after_shutdown_state_voids_the_saved_state() {
    let mut t = started();
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    ok(&extend(&mut t, 1, &[1; 32]));
    t.init();
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
}

#[test]
fn shutdown_errors() {
    let mut t = started();
    err(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 2])), Rc(0x1C4));
    err(
        &run(&mut t, plain(cc::SHUTDOWN, &[], &[0])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0, 9])), Rc::SIZE);
    // The TPM keeps accepting commands after Shutdown (Part 3 clause 9.4.1).
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    ok(&run(&mut t, plain(cc::GET_RANDOM, &[], &[0, 4])));
}

// ---------------------------------------------------------------- SelfTest / GetTestResult

#[test]
fn self_test_and_get_test_result() {
    let mut t = started();
    ok(&run(&mut t, plain(cc::SELF_TEST, &[], &[1])));
    ok(&run(&mut t, plain(cc::SELF_TEST, &[], &[0])));
    let r = run(&mut t, plain(cc::GET_TEST_RESULT, &[], &[]));
    // outData empty (size 0), testResult TPM_RC_SUCCESS.
    assert_eq!(params(&r, 0), [0, 0, 0, 0, 0, 0]);
    // Errors: TPMI_YES_NO accepts only 0 and 1.
    err(
        &run(&mut t, plain(cc::SELF_TEST, &[], &[2])),
        Rc::VALUE.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::SELF_TEST, &[], &[])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::GET_TEST_RESULT, &[], &[0])),
        Rc::SIZE,
    );
}

#[test]
fn incremental_self_test_reports_completion_and_validates_the_algorithm_list() {
    let mut t = tpm_new();
    let command = |params: &[u8]| plain(cc::INCREMENTAL_SELF_TEST, &[], params);
    err(&run(&mut t, command(&u32b(0))), Rc::INITIALIZE);
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    let mut all = u32b(13).to_vec();
    for algorithm in [
        0x01, 0x05, 0x06, 0x08, 0x0a, 0x0b, 0x14, 0x15, 0x17, 0x18, 0x19, 0x23, 0x43,
    ] {
        all.extend_from_slice(&u16b(algorithm));
    }
    let mut duplicate = u32b(64).to_vec();
    for _ in 0..64 {
        duplicate.extend_from_slice(&u16b(alg::SHA256));
    }
    for valid in [u32b(0).to_vec(), all, duplicate] {
        let response = run(&mut t, command(&valid));
        ok(&response);
        assert_eq!(params(&response, 0), [0; 4]);
    }
    for (bad, error) in [
        (u32b(65).to_vec(), Rc::SIZE.in_param(1)),
        (cat(&[&u32b(1), &[0xff, 0xff]]), Rc::VALUE.in_param(1)),
        (
            cat(&[&u32b(2), &u16b(alg::SHA256)]),
            Rc::INSUFFICIENT.in_param(1),
        ),
        (cat(&[&u32b(0), &[0]]), Rc::SIZE),
    ] {
        err(&run(&mut t, command(&bad)), error);
    }
    ok(&run(&mut t, plain(cc::SELF_TEST, &[], &[1])));
    let random = plain(cc::GET_RANDOM, &[], &[0, 16]);
    assert_eq!(run(&mut t, random.clone()), run(&mut started(), random));
}

#[test]
fn failure_mode_answers_only_get_test_result_and_get_capability() {
    let mut t = started();
    t.force_failure_mode();
    err(
        &run(&mut t, plain(cc::INCREMENTAL_SELF_TEST, &[], &u32b(0))),
        Rc::FAILURE,
    );
    err(
        &run(&mut t, plain(cc::GET_RANDOM, &[], &[0, 4])),
        Rc::FAILURE,
    );
    err(
        &run(&mut t, plain(cc::PCR_READ, &[], &u32b(0))),
        Rc::FAILURE,
    );
    let r = run(&mut t, plain(cc::GET_TEST_RESULT, &[], &[]));
    assert_eq!(params(&r, 0), [0, 0, 0, 0, 0x01, 0x01]);
    ok(&run(
        &mut t,
        plain(
            cc::GET_CAPABILITY,
            &[],
            &cat(&[&u32b(cap::PCRS), &u32b(0), &u32b(1)]),
        ),
    ));
    // With a session tag even these two commands fail (Part 3 clause 10.4.1).
    err(
        &run(&mut t, with_pw(cc::GET_TEST_RESULT, &[], &[], &[])),
        Rc::FAILURE,
    );
    // The failure lasts until _TPM_Init.
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    ok(&run(&mut t, plain(cc::GET_RANDOM, &[], &[0, 4])));
}

// ---------------------------------------------------------------- GetCapability

fn get_cap(t: &mut Tpm, capability: u32, property: u32, count: u32) -> Vec<u8> {
    run(
        t,
        plain(
            cc::GET_CAPABILITY,
            &[],
            &cat(&[&u32b(capability), &u32b(property), &u32b(count)]),
        ),
    )
}

/// (moreData, capability, list payload after the count, count)
fn cap_list(resp: &[u8]) -> (bool, u32, Vec<u8>, u32) {
    let p = params(resp, 0);
    let more = p[0] == 1;
    let capability = u32::from_be_bytes([p[1], p[2], p[3], p[4]]);
    let count = u32::from_be_bytes([p[5], p[6], p[7], p[8]]);
    (more, capability, p[9..].to_vec(), count)
}

#[test]
fn get_capability_reports_the_locality_zero_pcr_properties_and_paginates() {
    let mut t = started();
    let capability = cap::PCR_PROPERTIES;
    let response = get_cap(&mut t, capability, 0, 2);
    ok(&response);
    assert_eq!(
        cap_list(&response),
        (
            true,
            capability,
            vec![0, 0, 0, 0, 3, 0xff, 0xff, 0, 0, 0, 0, 2, 3, 0, 0, 0x81],
            2
        )
    );
    let response = get_cap(&mut t, capability, 3, 10);
    ok(&response);
    assert_eq!(
        cap_list(&response),
        (
            false,
            capability,
            vec![0, 0, 0, 0x11, 3, 0, 0, 0xe1, 0, 0, 0, 0x12, 3, 0, 0, 0],
            2
        )
    );
    let response = get_cap(&mut t, capability, 0x13, 10);
    ok(&response);
    assert_eq!(cap_list(&response), (false, capability, Vec::new(), 0));
}

fn listed_commands(t: &mut Tpm) -> Vec<u32> {
    let (more, c, body, count) = cap_list(&get_cap(t, cap::COMMANDS, 0, 1000));
    assert!(!more && c == cap::COMMANDS);
    assert_eq!(body.len(), 4 * count as usize);
    body.chunks(4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) & 0xFFFF)
        .collect()
}

#[test]
fn capability_commands_lists_exactly_the_implemented_commands() {
    let mut t = started();
    // Expected list written out independently of the dispatch table (Part 2 Table 14 codes).
    let expected: Vec<u32> = vec![
        // Slice C1 of phase B: DictionaryAttackLockReset (0x139), DictionaryAttackParameters
        // (0x13A), ECDH_ZGen (0x154), RSA_Decrypt (0x159), RSA_Encrypt (0x174). Slice B: EvictControl
        // (0x120), CreatePrimary (0x131), Create (0x153), Load (0x157), Sign (0x15D), Unseal
        // (0x15E), LoadExternal (0x167), ReadPublic (0x173), VerifySignature (0x177). Slice A:
        // HierarchyControl, NV_UndefineSpace, Clear, ClearControl, HierarchyChangeAuth,
        // NV_DefineSpace, NV_Write, NV_WriteLock. The rest is phase A, slice A (NV_Read,
        // NV_ReadLock, FlushContext, NV_ReadPublic, StartAuthSession).
        0x11F, 0x120, 0x121, 0x122, 0x126, 0x127, 0x129, 0x12A, 0x131, 0x137, 0x138, 0x139, 0x13A,
        0x13C, 0x13D, 0x13E, 0x143, 0x144, 0x145, 0x146, 0x14E, 0x14F, 0x153, 0x154, 0x157, 0x159,
        0x15C, 0x15D, 0x15E, 0x165, 0x167, 0x169, 0x173, 0x174, 0x176, 0x177, 0x17A, 0x17B, 0x17C,
        0x17D, 0x17E, 0x181, 0x182, 0x185, 0x186,
        // Slice C2: Certify, PolicySecret, HMAC, Quote, ContextLoad/Save, PolicyAuthValue,
        // PolicyCommandCode, PolicyOR, PolicyPCR, PolicyRestart, PolicyGetDigest.
        0x148, 0x151, 0x155, 0x158, 0x161, 0x162, 0x16B, 0x16C, 0x171, 0x17F, 0x180, 0x189,
        // Slice C3: NV_Increment, NV_SetBits, NV_Extend, GetTime.
        0x134, 0x135, 0x136, 0x14C, // PTP 1.07 expansion.
        0x128, 0x147, 0x14A, 0x14D, 0x150, 0x15B, 0x163, 0x168, 0x16A, 0x16E, 0x16F, 0x170, 0x178,
        0x184, 0x18A, 0x18C, 0x190, 0x19C, 0x12E, 0x16D, 0x149, 0x18F, 0x192, 0x13B, 0x130, 0x172,
        0x160, 0x142, 0x12B, 0x156, 0x14B, 0x188, 0x1A5, 0x1A6, 0x1A4, 0x1AA, 0x1A3, 0x1A9,
    ];
    let mut expected = expected;
    expected.sort_unstable();
    assert_eq!(expected.len(), 99);
    assert_eq!(listed_commands(&mut t), expected);
    let table: Vec<u32> = COMMANDS.iter().map(|c| c.code).collect();
    assert_eq!(listed_commands(&mut t), table);
    // Every listed command is dispatched, every other code is TPM_RC_COMMAND_CODE (checked for
    // the whole Part 2 range in `commands_outside_the_profile_return_command_code_not_success`).
    for code in expected {
        assert_ne!(rc(&run(&mut t, plain(code, &[], &[]))), Rc::COMMAND_CODE.0);
    }
    // TPMA_CC attributes: EventSequenceComplete {NV F} with two handles, HashSequenceStart rHandle.
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x185, 1));
    let a = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    assert_eq!(a, 0x185 | 1 << 22 | 1 << 24 | 2 << 25);
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x186, 1));
    assert_eq!(
        u32::from_be_bytes([body[0], body[1], body[2], body[3]]),
        0x186 | 1 << 28
    );
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x18A, 1));
    assert_eq!(body, 0x18Au32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x18C, 1));
    assert_eq!(body, 0x0200_018Cu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x18F, 1));
    assert_eq!(body, 0x0200_018Fu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x184, 1));
    assert_eq!(body, 0x0600_0184u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x128, 1));
    assert_eq!(body, 0x0240_0128u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x168, 1));
    assert_eq!(body, 0x0200_0168u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x147, 1));
    assert_eq!(body, 0x0400_0147u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x149, 1));
    assert_eq!(body, 0x0600_0149u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x150, 1));
    assert_eq!(body, 0x0400_0150u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x16A, 1));
    assert_eq!(body, 0x0200_016Au32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x160, 1));
    assert_eq!(body, 0x0400_0160u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x16D, 1));
    assert_eq!(body, 0x0200_016Du32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x172, 1));
    assert_eq!(body, 0x0200_0172u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x170, 1));
    assert_eq!(body, 0x0200_0170u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x190, 1));
    assert_eq!(body, 0x0200_0190u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x192, 1));
    assert_eq!(body, 0x0600_0192u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x19C, 1));
    assert_eq!(body, 0x0200_019Cu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x12E, 1));
    assert_eq!(body, 0x0240_012Eu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x11F, 1));
    assert_eq!(body, 0x0440_011Fu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x13B, 1));
    assert_eq!(body, 0x0240_013Bu32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x130, 1));
    assert_eq!(body, 0x0200_0130u32.to_be_bytes());
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x142, 1));
    assert_eq!(body, 0x0040_0142u32.to_be_bytes());
    // TPM_PT_TOTAL_COMMANDS agrees with the list.
    let (_, _, props, count) =
        cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::TOTAL_COMMANDS, 1));
    assert_eq!(count, 1);
    assert_eq!(
        props,
        [&u32b(pt::TOTAL_COMMANDS)[..], &u32b(99)[..]].concat()
    );
}

#[test]
fn capability_commands_paging_and_more_data() {
    let mut t = started();
    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0, 5));
    assert!(more && count == 5 && body.len() == 20);
    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x17E, 100));
    let expected = [
        0x17E, 0x17F, 0x180, 0x181, 0x182, 0x184, 0x185, 0x186, 0x188, 0x189, 0x18A, 0x18C, 0x18F,
        0x190, 0x192, 0x19C, 0x1A3, 0x1A4, 0x1A5, 0x1A6, 0x1A9, 0x1AA,
    ];
    assert!(!more);
    assert_eq!(count as usize, expected.len());
    assert_eq!(body.len(), 4 * expected.len());
    for (entry, code) in body.chunks_exact(4).zip(expected) {
        assert_eq!(u16::from_be_bytes([entry[2], entry[3]]), code);
    }
    let (more, _, _, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x1AB, 100));
    assert!(!more && count == 0);
    // propertyCount 0: empty list, moreData YES because entries remain.
    let (more, _, _, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0, 0));
    assert!(more && count == 0);
}

#[test]
fn test_parms_accepts_supported_public_parameters_without_state_changes() {
    let mut t = started();
    let before = t.persistent_snapshot();
    for encoded in [
        "00010006008000430010080000000000", // RSA-2048 storage, AES-128-CFB, default exponent.
        "000100100014000b080000010001",     // RSA-2048 RSASSA-SHA256, explicit exponent.
        "000100100017000b080000000000",     // RSA-2048 OAEP-SHA256.
        "002300100018000b00030010",         // ECDSA-SHA256, NIST P-256, null KDF.
        "002300100019000b00030010",         // ECDH-SHA256, NIST P-256, null KDF.
        "00080005000b",                     // KEYEDHASH HMAC-SHA256.
        "00080010",                         // KEYEDHASH with no scheme.
    ] {
        let response = run(&mut t, plain(0x18A, &[], &hex(encoded)));
        assert_eq!(response, hex("80010000000a00000000"), "{encoded}");
    }
    assert_eq!(t.persistent_snapshot(), before);
    let random = plain(cc::GET_RANDOM, &[], &[0, 16]);
    assert_eq!(run(&mut t, random.clone()), run(&mut started(), random));
}

#[test]
fn test_parms_refuses_unsupported_encodings_and_reports_parameter_one() {
    let mut t = started();
    let before = t.persistent_snapshot();
    for (encoded, error) in [
        ("0025", Rc::TYPE), // SYMCIPHER is outside the implemented profile.
        ("00010006010000430010080000000000", Rc::SYMMETRIC), // AES-256.
        ("00010006008000420010080000000000", Rc::MODE), // AES-CBC.
        ("000100100016000b080000000000", Rc::VALUE), // RSAPSS.
        ("000100100014000c080000000000", Rc::HASH), // SHA-384.
        ("000100100014000b0c0000000000", Rc::VALUE), // RSA-3072.
        ("000100100014000b080000000003", Rc::RANGE), // Unsupported exponent.
        ("002300100018000b00040010", Rc::CURVE), // NIST P-384.
        ("002300100018000b00030021", Rc::KDF), // Non-null KDF.
        ("002300100014000b00030010", Rc::SCHEME), // RSASSA in ECC union.
        ("00080005", Rc::INSUFFICIENT), // Missing HMAC hash.
        ("", Rc::INSUFFICIENT), // Missing type selector.
    ] {
        err(
            &run(&mut t, plain(0x18A, &[], &hex(encoded))),
            error.in_param(1),
        );
    }
    let trailing = hex("0008001000");
    err(&run(&mut t, plain(0x18A, &[], &trailing)), Rc::SIZE);
    err(
        &run(&mut t, with_pw(0x18A, &[], &[b""], &hex("00080010"))),
        Rc::ATTRIBUTES.in_session(1),
    );
    assert_eq!(t.persistent_snapshot(), before);
}

#[test]
fn capability_algs_pcrs_and_handles() {
    let mut t = started();
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::ALGS, 0, 100));
    assert!(!more && c == cap::ALGS && count == 14);
    // RSA, HMAC, AES, KEYEDHASH, XOR, SHA256, RSASSA, RSAES, OAEP, ECDSA, ECDH, ECC, CFB with their
    // TPMA_ALGORITHM bits (Part 2, Table 11 types: RSA "A O", KEYEDHASH "H O", SHA256 "H", RSASSA
    // and ECDSA "A X", ECC "A O", RSAES "A E", OAEP "A E H", ECDH "A M"; AES "S O" and CFB "E" are
    // NOT VERIFIED).
    assert_eq!(
        body,
        [
            &[0x00, 0x01, 0, 0, 0, 9][..],
            // HMAC "H X" (slice C2).
            &[0x00, 0x05, 0, 0, 1, 4],
            &[0x00, 0x06, 0, 0, 0, 10],
            &[0x00, 0x08, 0, 0, 0, 12],
            // XOR "H S" (slice C3).
            &[0x00, 0x0A, 0, 0, 0, 6],
            &[0x00, 0x0B, 0, 0, 0, 4],
            &[0x00, 0x0C, 0, 0, 0, 4],
            &[0x00, 0x14, 0, 0, 1, 1],
            &[0x00, 0x15, 0, 0, 2, 1],
            &[0x00, 0x17, 0, 0, 2, 5],
            &[0x00, 0x18, 0, 0, 1, 1],
            &[0x00, 0x19, 0, 0, 4, 1],
            &[0x00, 0x23, 0, 0, 0, 9],
            &[0x00, 0x43, 0, 0, 2, 0],
        ]
        .concat()
    );
    // Paging starts at the given algorithm ID.
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x0C, 100)).3, 8);
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x24, 100)).3, 1);
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x44, 100)).3, 0);

    let r = get_cap(&mut t, cap::PCRS, 0, 100);
    let p = params(&r, 0);
    assert_eq!(p[0], 0);
    let sel = TpmlPcrSelection::unmarshal(&mut Reader::new(&p[5..])).unwrap();
    assert_eq!(sel.selections.len(), 2);
    assert_eq!(sel.selections[0].hash, alg::SHA256);
    assert_eq!(sel.selections[0].select, [0xFF; 3]);
    assert_eq!(sel.selections[1].hash, alg::SHA384);
    assert_eq!(sel.selections[1].select, [0xFF; 3]);
    err(&get_cap(&mut t, cap::PCRS, 1, 1), Rc::VALUE.in_param(2));

    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::HANDLES, 0, 1000));
    assert!(!more && count == 24);
    assert_eq!(&body[..4], u32b(0));
    assert_eq!(&body[92..96], u32b(23));
    // Paging inside the PCR range.
    let (more, _, _, count) = cap_list(&get_cap(&mut t, cap::HANDLES, 0, 10));
    assert!(more && count == 10);
    assert_eq!(cap_list(&get_cap(&mut t, cap::HANDLES, 20, 100)).3, 4);
    // Transient handles list the loaded sequences.
    assert_eq!(
        cap_list(&get_cap(&mut t, cap::HANDLES, 0x8000_0000, 10)).3,
        0
    );
    ok(&run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
        ),
    ));
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::HANDLES, 0x8000_0000, 10));
    assert_eq!((count, body), (1, u32b(0x8000_0000)));
    // Persistent, NV and session ranges are empty; an undefined handle type is TPM_RC_VALUE.
    for ht in [0x0100_0000u32, 0x0200_0000, 0x0300_0000, 0x8100_0000] {
        assert_eq!(cap_list(&get_cap(&mut t, cap::HANDLES, ht, 10)).3, 0);
    }
    err(
        &get_cap(&mut t, cap::HANDLES, 0x5500_0000, 10),
        Rc::VALUE.in_param(2),
    );
}

#[test]
fn capability_tpm_properties() {
    let mut t = started();
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::FIXED, 1000));
    assert!(!more && c == cap::TPM_PROPERTIES);
    let props: Vec<(u32, u32)> = body
        .chunks(8)
        .map(|b| {
            (
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            )
        })
        .collect();
    assert_eq!(props.len(), count as usize);
    let get = |tag: u32| props.iter().find(|p| p.0 == tag).map(|p| p.1);
    assert_eq!(get(pt::FAMILY_INDICATOR), Some(0x322E_3000));
    assert_eq!(get(pt::PCR_COUNT), Some(24));
    assert_eq!(get(pt::PCR_SELECT_MIN), Some(3));
    assert_eq!(get(pt::MAX_DIGEST), Some(32));
    assert_eq!(get(pt::PS_REVISION), Some(0x107));
    // ADR-0009: the buffer limits do not exceed the CRB buffer (0xF80 = 3968).
    assert!(get(pt::MAX_COMMAND_SIZE).unwrap() <= 3968);
    assert!(get(pt::MAX_RESPONSE_SIZE).unwrap() <= 3968);
    assert_eq!(get(pt::INPUT_BUFFER), Some(1024));
    // Sorted ascending and all inside the fixed group.
    assert!(props.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(props.iter().all(|p| p.0 / 0x100 == 1));
    // Starting in the middle of the group.
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::PCR_COUNT, 1));
    assert_eq!(body, [&u32b(pt::PCR_COUNT)[..], &u32b(24)[..]].concat());
    // Variable group: attributes, counts of indices and sessions, transient slots available.
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::VAR, 100));
    let var: Vec<(u32, u32)> = body
        .chunks(8)
        .map(|b| {
            (
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            )
        })
        .collect();
    assert_eq!(var.len(), count as usize);
    assert!(var.windows(2).all(|w| w[0].0 < w[1].0));
    let v = |tag: u32| var.iter().find(|p| p.0 == tag).map(|p| p.1);
    // Three sequence slots plus eight object slots (slice B).
    assert_eq!(v(pt::HR_TRANSIENT_AVAIL), Some(11));
    assert_eq!(v(pt::HR_PERSISTENT), Some(0));
    assert_eq!(v(pt::HR_PERSISTENT_AVAIL), Some(16));
    assert_eq!(v(pt::HR_NV_INDEX), Some(0));
    assert_eq!(v(pt::HR_LOADED), Some(0));
    assert_eq!(v(pt::HR_LOADED_AVAIL), Some(64));
    // TPMA_STARTUP_CLEAR: phEnable, shEnable, ehEnable, phEnableNV all SET; TPMA_PERMANENT:
    // only tpmGeneratedEPS.
    assert_eq!(v(pt::STARTUP_CLEAR), Some(0xF));
    assert_eq!(v(pt::PERMANENT), Some(1 << 10));
    // Other groups are empty.
    assert_eq!(cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, 0, 100)).3, 0);
    assert_eq!(
        cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, 0x300, 100)).3,
        0
    );
}

#[test]
fn capability_empty_lists_and_errors() {
    let mut t = started();
    for c in [cap::PP_COMMANDS, cap::AUDIT_COMMANDS] {
        let (more, cc_, _, count) = cap_list(&get_cap(&mut t, c, 0, 10));
        assert!(!more && cc_ == c && count == 0);
    }
    // Capabilities that are not implemented: TPM_RC_VALUE, parameter 1.
    for c in [0xA, 0xB, 0x100, 0xFFFF_FFFF] {
        err(&get_cap(&mut t, c, 0, 1), Rc(0x1C4));
    }
    err(
        &run(&mut t, plain(cc::GET_CAPABILITY, &[], &u32b(cap::ALGS))),
        Rc::INSUFFICIENT.in_param(2),
    );
    err(
        &run(
            &mut t,
            plain(
                cc::GET_CAPABILITY,
                &[],
                &cat(&[&u32b(0), &u32b(0), &u32b(1), &[0]]),
            ),
        ),
        Rc::SIZE,
    );
}

// ---------------------------------------------------------------- GetRandom / StirRandom

#[test]
fn get_random_returns_requested_octets_up_to_one_digest() {
    let mut t = started();
    let get = |t: &mut Tpm, n: u16| {
        let r = run(t, plain(cc::GET_RANDOM, &[], &u16b(n)));
        let p = params(&r, 0);
        let len = u16::from_be_bytes([p[0], p[1]]) as usize;
        assert_eq!(p.len(), 2 + len);
        p[2..].to_vec()
    };
    assert_eq!(get(&mut t, 0).len(), 0);
    assert_eq!(get(&mut t, 16).len(), 16);
    assert_eq!(get(&mut t, 32).len(), 32);
    // More than a TPM2B_DIGEST holds: no error, 32 octets (Part 3 clause 16.1).
    assert_eq!(get(&mut t, 4096).len(), 32);
    let a = get(&mut t, 32);
    let b = get(&mut t, 32);
    assert_ne!(a, b);
    assert_ne!(a, vec![0u8; 32]);
}

#[test]
fn get_random_errors() {
    let mut t = started();
    err(
        &run(&mut t, plain(cc::GET_RANDOM, &[], &[1])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::GET_RANDOM, &[], &[])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::GET_RANDOM, &[], &[0, 1, 2])),
        Rc::SIZE,
    );
    // A password session on a command without authorization handles is not accepted.
    err(
        &run(&mut t, with_pw(cc::GET_RANDOM, &[], &[b""], &[0, 1])),
        Rc::ATTRIBUTES.in_session(1),
    );
}

#[test]
fn get_random_is_a_function_of_the_owner_entropy() {
    let out = |e: u8| {
        let mut t = Tpm::new(&[e; 48]).unwrap();
        ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
        run(&mut t, plain(cc::GET_RANDOM, &[], &u16b(32)))
    };
    assert_eq!(out(1), out(1));
    assert_ne!(out(1), out(2));
}

#[test]
fn stir_random_changes_the_generator_and_checks_the_size() {
    let mut a = started();
    let mut b = started();
    let ra = run(
        &mut a,
        plain(cc::STIR_RANDOM, &[], &tpm2b(b"additional data")),
    );
    ok(&ra);
    assert_eq!(ra.len(), 10);
    let x = run(&mut a, plain(cc::GET_RANDOM, &[], &u16b(32)));
    let y = run(&mut b, plain(cc::GET_RANDOM, &[], &u16b(32)));
    assert_ne!(x, y);
    // Boundaries: 128 octets are accepted, 129 are TPM_RC_SIZE for parameter 1; empty is allowed.
    ok(&run(&mut a, plain(cc::STIR_RANDOM, &[], &tpm2b(&[7; 128]))));
    ok(&run(&mut a, plain(cc::STIR_RANDOM, &[], &tpm2b(&[]))));
    err(
        &run(&mut a, plain(cc::STIR_RANDOM, &[], &tpm2b(&[7; 129]))),
        Rc::SIZE.in_param(1),
    );
    err(
        &run(&mut a, plain(cc::STIR_RANDOM, &[], &[0, 5, 1])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(
            &mut a,
            plain(cc::STIR_RANDOM, &[], &cat(&[&tpm2b(b"x"), &[0]])),
        ),
        Rc::SIZE,
    );
}

// ---------------------------------------------------------------- PCR

fn pcr_read_cmd(hash: u16, size: u8, select: &[u8]) -> Vec<u8> {
    plain(
        cc::PCR_READ,
        &[],
        &cat(&[&u32b(1), &u16b(hash), &[size], select]),
    )
}

#[test]
fn pcr_allocate_activates_the_desired_banks_only_after_init_or_restore() {
    let mut t = started();
    let read_allocation = |t: &mut Tpm| cap_list(&get_cap(t, cap::PCRS, 0, 1)).2;
    let both = hex("000b03ffffff000c03ffffff");
    let sha384_only = hex("000b03000000000c03ffffff");
    assert_eq!(read_allocation(&mut t), both);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(1))));

    let allocate = |wire: &[u8]| with_pw(cc::PCR_ALLOCATE, &[handle::RH_PLATFORM], &[b""], wire);
    let response = run(&mut t, allocate(&hex("00000001000b03000000")));
    assert_eq!(params(&response, 0), hex("01000000180000048000000780"));
    assert_eq!(read_allocation(&mut t), both);
    err(
        &run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(1))),
        Rc::TYPE.in_param(1),
    );
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(0))));
    let snapshot = t.persistent_snapshot();
    assert_eq!(&snapshot[8..10], &u16b(crate::SNAPSHOT_VERSION));
    assert_eq!(snapshot[snapshot.len() - 34 - 96], 2);

    t.init();
    err(
        &run(&mut t, plain(cc::STARTUP, &[], &u16b(1))),
        Rc::VALUE.in_param(1),
    );
    ok(&run(&mut t, plain(cc::STARTUP, &[], &u16b(0))));
    assert_eq!(read_allocation(&mut t), sha384_only);
    let mut restored = Tpm::restore(&snapshot, &[0x55; 48]).unwrap();
    ok(&run(&mut restored, plain(cc::STARTUP, &[], &u16b(0))));
    assert_eq!(read_allocation(&mut restored), sha384_only);

    let selection = hex("00000002000b03010000000c03010000");
    assert_eq!(
        params(&run(&mut t, plain(cc::PCR_READ, &[], &selection)), 0),
        cat(&[
            &u32b(0),
            &hex("00000002000b03000000000c03010000"),
            &u32b(1),
            &tpm2b(&[0; 48]),
        ]),
    );
    // An extend affecting only a disabled bank neither increments the generation nor
    // invalidates the state saved by Shutdown(STATE).
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(1))));
    ok(&extend(&mut t, 0, &[1; 32]));
    assert_eq!(pcr_counter(&mut t), 0);
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &u16b(1))));
    assert_eq!(read_allocation(&mut t), sha384_only);
    let before = pcr_counter(&mut t);
    ok(&run(
        &mut t,
        with_pw(cc::PCR_EVENT, &[0], &[b""], &tpm2b(b"abc")),
    ));
    assert_eq!(pcr_counter(&mut t), before + 1);
    assert_eq!(
        read_pcr_bank(&mut t, 0, alg::SHA384),
        hex(SHA384_EXTEND_ABC_ON_ZERO)
    );
}

#[test]
fn pcr_allocate_requires_platform_and_merges_only_the_final_selection_per_bank() {
    let mut t = started();
    let allocate = |wire: &[u8]| with_pw(cc::PCR_ALLOCATE, &[handle::RH_PLATFORM], &[b""], wire);
    let orig = t.persistent_snapshot();
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_ALLOCATE, &[handle::RH_OWNER], &[b""], &u32b(0)),
        ),
        Rc::VALUE.in_handle(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::PCR_ALLOCATE, &[handle::RH_PLATFORM], &u32b(0)),
        ),
        Rc::AUTH_MISSING,
    );
    for (wire, error) in [
        ("00000002000b03000000000c03000000", Rc::PCR),
        ("00000001000c03010002", Rc::NO_RESULT),
        ("00000001000403ffffff", Rc::HASH.in_param(1)),
        ("00000003", Rc::SIZE.in_param(1)),
        ("00000001000b03", Rc::INSUFFICIENT.in_param(1)),
        ("0000000000", Rc::SIZE),
    ] {
        err(&run(&mut t, allocate(&hex(wire))), error);
    }
    assert_eq!(t.persistent_snapshot(), orig);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(1))));
    let orderly = t.persistent_snapshot();
    let duplicate = hex("00000002000b03010000000b03ffffff");
    let response = run(&mut t, allocate(&duplicate));
    assert_eq!(params(&response, 0), hex("01000000180000078000000780"));
    assert_eq!(t.persistent_snapshot(), orderly);
    err(
        &run(&mut t, plain(cc::SHUTDOWN, &[], &u16b(1))),
        Rc::TYPE.in_param(1),
    );

    // A later request preserves the pending choice for an unmentioned bank.
    ok(&run(&mut t, allocate(&hex("00000001000b03000000"))));
    ok(&run(&mut t, allocate(&hex("00000001000c03ffffff"))));
    err(
        &run(&mut t, allocate(&hex("00000001000c03000000"))),
        Rc::PCR,
    );
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &u16b(0))));
    assert_eq!(
        cap_list(&get_cap(&mut t, cap::PCRS, 0, 1)).2,
        hex("000b03000000000c03ffffff")
    );
}

#[test]
fn pcr_read_initial_values_follow_ptp_table_15() {
    let mut t = started();
    for i in 0..24 {
        let expect = if (17..=22).contains(&i) { 0xFF } else { 0 };
        assert_eq!(read_pcr(&mut t, i), vec![expect; 32], "PCR {i}");
    }
    // Selecting all 24: a response holds at most 8 digests (TPML_DIGEST), in ascending order;
    // pcrSelectionOut marks the returned ones and the caller continues with the rest.
    let r = run(&mut t, pcr_read_cmd(alg::SHA256, 3, &[0xFF, 0xFF, 0xFF]));
    let mut rd = Reader::new(params(&r, 0));
    assert_eq!(rd.u32().unwrap(), 0);
    let sel = TpmlPcrSelection::unmarshal(&mut rd).unwrap();
    assert_eq!(sel.selections[0].select, [0xFF, 0x00, 0x00]);
    assert_eq!(TpmlDigest::unmarshal(&mut rd).unwrap().digests.len(), 8);
    let r = run(&mut t, pcr_read_cmd(alg::SHA256, 3, &[0x00, 0x00, 0xFF]));
    let mut rd = Reader::new(params(&r, 0));
    rd.u32().unwrap();
    let sel = TpmlPcrSelection::unmarshal(&mut rd).unwrap();
    assert_eq!(sel.selections[0].select, [0x00, 0x00, 0xFF]);
    let d = TpmlDigest::unmarshal(&mut rd).unwrap();
    assert_eq!(d.digests.len(), 8);
    assert_eq!(
        d.digests[1],
        vec![0xFF; 32],
        "PCR 17 is the second value of 16..=23"
    );
    // Empty selection and empty list are valid and return no value.
    let r = run(&mut t, plain(cc::PCR_READ, &[], &u32b(0)));
    let mut rd = Reader::new(params(&r, 0));
    rd.u32().unwrap();
    assert!(
        TpmlPcrSelection::unmarshal(&mut rd)
            .unwrap()
            .selections
            .is_empty()
    );
    assert!(TpmlDigest::unmarshal(&mut rd).unwrap().digests.is_empty());
    let r = run(&mut t, pcr_read_cmd(alg::SHA256, 3, &[0, 0, 0]));
    let mut rd = Reader::new(params(&r, 0));
    rd.u32().unwrap();
    assert_eq!(
        TpmlPcrSelection::unmarshal(&mut rd).unwrap().selections[0].select,
        [0; 3]
    );
    assert!(TpmlDigest::unmarshal(&mut rd).unwrap().digests.is_empty());
}

#[test]
fn pcr_read_errors() {
    let mut t = started();
    // SHA-1 bank does not exist: TPM_RC_HASH, parameter 1.
    err(
        &run(&mut t, pcr_read_cmd(0x0004, 3, &[1, 0, 0])),
        Rc::HASH.in_param(1),
    );
    // sizeofSelect below PCR_SELECT_MIN, above PCR_SELECT_MAX: TPM_RC_VALUE.
    err(
        &run(&mut t, pcr_read_cmd(alg::SHA256, 2, &[1, 0])),
        Rc::VALUE.in_param(1),
    );
    err(
        &run(&mut t, pcr_read_cmd(alg::SHA256, 4, &[1, 0, 0, 0])),
        Rc::VALUE.in_param(1),
    );
    // More selections than banks: TPM_RC_SIZE.
    err(
        &run(&mut t, plain(cc::PCR_READ, &[], &u32b(3))),
        Rc::SIZE.in_param(1),
    );
    // Truncated.
    err(
        &run(&mut t, pcr_read_cmd(alg::SHA256, 3, &[1, 0])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::PCR_READ, &[], &[0, 0, 0])),
        Rc::INSUFFICIENT.in_param(1),
    );
}

#[test]
fn pcr_extend_matches_independent_vector() {
    let mut t = started();
    let digest = hex(SHA256_ABC);
    let r = extend(&mut t, 0, &digest);
    ok(&r);
    // With sessions the response carries parameterSize 0 and one password acknowledgement.
    assert_eq!(
        r,
        [
            0x80,
            0x02,
            0,
            0,
            0,
            10 + 4 + 5,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            0,
            0
        ]
    );
    // Computed independently: sha256(32 x 0x00 || sha256("abc")).
    assert_eq!(read_pcr(&mut t, 0), hex(EXTEND_ABC_ON_ZERO));
    assert_eq!(pcr_counter(&mut t), 1);
    // Extending again chains from the new value.
    ok(&extend(&mut t, 0, &digest));
    assert_eq!(
        read_pcr(&mut t, 0),
        hex("bdeb6c6dc63852834c89f67066194207ce7d3806ea40ca58dc079246ef58a926")
    );
    assert_eq!(pcr_counter(&mut t), 2);
    // PCR 16 and 23 can be extended at locality 0 without advancing pcrUpdateCounter.
    ok(&extend(&mut t, 16, &digest));
    ok(&extend(&mut t, 23, &digest));
    assert_eq!(read_pcr(&mut t, 16), hex(EXTEND_ABC_ON_ZERO));
    assert_eq!(pcr_counter(&mut t), 2);
}

#[test]
fn pcr_read_follows_bank_order_and_counts_each_extended_bank_once() {
    let mut t = started();
    let data256 = hex(SHA256_ABC);
    let data384 = hex(SHA384_ABC);
    let extend = |t: &mut Tpm, pcr, digests: &[u8]| {
        ok(&run(t, with_pw(cc::PCR_EXTEND, &[pcr], &[b""], digests)));
    };
    let mixed = cat(&[&u32b(2), &u16b(12), &data384, &u16b(11), &data256]);
    extend(&mut t, 0, &mixed);
    extend(&mut t, 16, &mixed); // The non-counting PCR attribute covers both banks.
    assert_eq!(pcr_counter(&mut t), 2);
    let selection = hex("00000002000c03010001000b03010001");
    let response = run(&mut t, plain(cc::PCR_READ, &[], &selection));
    assert_eq!(
        params(&response, 0),
        cat(&[
            &u32b(2),
            &selection,
            &u32b(4),
            &tpm2b(&hex(SHA384_EXTEND_ABC_ON_ZERO)),
            &tpm2b(&hex(SHA384_EXTEND_ABC_ON_ZERO)),
            &tpm2b(&hex(EXTEND_ABC_ON_ZERO)),
            &tpm2b(&hex(EXTEND_ABC_ON_ZERO)),
        ]),
    );
    let repeated = cat(&[&u32b(2), &u16b(12), &data384, &u16b(12), &data384]);
    extend(&mut t, 1, &repeated);
    assert_eq!(
        read_pcr_bank(&mut t, 1, alg::SHA384),
        hex(
            "0b815adb5c2824360b25f9c2ca667eee481dc15676327e8c56be97a3275d8f114d89b198e39f5f49e89657ea2a8adb6a"
        )
    );
    assert_eq!(pcr_counter(&mut t), 3);
    ok(&run(&mut t, with_pw(cc::PCR_RESET, &[16], &[b""], &[])));
    assert_eq!(read_pcr_bank(&mut t, 16, alg::SHA384), vec![0; 48]);
    assert_eq!(read_pcr(&mut t, 16), vec![0; 32]);
    assert_eq!(pcr_counter(&mut t), 3);
}

#[test]
fn pcr_extend_authorization_errors() {
    let mut t = started();
    let d = [1u8; 32];
    // No session: TPM_RC_AUTH_MISSING.
    err(
        &run(&mut t, plain(cc::PCR_EXTEND, &[3], &sha256_ha(&d))),
        Rc::AUTH_MISSING,
    );
    // Wrong password (PCR authValue is empty): TPM_RC_BAD_AUTH, session 1.
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[3], &[b"secret"], &sha256_ha(&d)),
        ),
        Rc::BAD_AUTH.in_session(1),
    );
    // Trailing zero octets of the password are ignored (Part 1 clause 17.4).
    ok(&run(
        &mut t,
        with_pw(cc::PCR_EXTEND, &[3], &[&[0, 0]], &sha256_ha(&d)),
    ));
    // The extra password session protects neither parameters nor audit.
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[3], &[b"", b""], &sha256_ha(&d)),
        ),
        Rc::ATTRIBUTES.in_session(2),
    );
    // A session tag requires at least one complete authorization entry.
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[], &sha256_ha(&d))),
        Rc::AUTHSIZE,
    );
    // An HMAC session handle that is not loaded: TPM_RC_REFERENCE_S0.
    let mut hmac_session = Vec::new();
    hmac_session.extend_from_slice(&0x0200_0000u32.to_be_bytes());
    hmac_session.extend_from_slice(&[0, 0, 1, 0, 0]);
    let mut area = (hmac_session.len() as u32).to_be_bytes().to_vec();
    area.extend_from_slice(&hmac_session);
    let c = frame(st::SESSIONS, cc::PCR_EXTEND, &[3], &area, &sha256_ha(&d));
    err(&run(&mut t, c), Rc::reference_session(0));
    // Nothing above changed PCR 3 except the one accepted extend.
    assert_eq!(pcr_counter(&mut t), 1);
}

#[test]
fn pcr_extend_handle_and_parameter_errors() {
    let mut t = started();
    let d = [1u8; 32];
    // PCR 24 does not exist; a non-PCR handle: TPM_RC_VALUE, handle 1.
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[24], &[b""], &sha256_ha(&d)),
        ),
        Rc::VALUE.in_handle(1),
    );
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[0x0100_0000], &[b""], &sha256_ha(&d)),
        ),
        Rc::VALUE.in_handle(1),
    );
    // SHA-1 digest: TPM_RC_HASH, parameter 1.
    let sha1 = cat(&[&u32b(1), &u16b(0x0004), &[0; 20]]);
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[b""], &sha1)),
        Rc::HASH.in_param(1),
    );
    // More digests than implemented banks: TPM_RC_SIZE.
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[b""], &u32b(3))),
        Rc::SIZE.in_param(1),
    );
    // Short digest and trailing octets.
    let short = cat(&[&u32b(1), &u16b(alg::SHA256), &[0; 31]]);
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[b""], &short)),
        Rc::INSUFFICIENT.in_param(1),
    );
    let long = cat(&[&sha256_ha(&d), &[0]]);
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[b""], &long)),
        Rc::SIZE,
    );
    // Missing handle area.
    err(
        &run(&mut t, plain(cc::PCR_EXTEND, &[], &[])),
        Rc::INSUFFICIENT,
    );
    assert_eq!(read_pcr(&mut t, 3), vec![0u8; 32]);
}

#[test]
fn pcr_extend_respects_locality_attributes() {
    let mut t = started();
    let d = hex(SHA256_ABC);
    // PCR 17-22 cannot be extended at locality 0 (PTP Table 14, column 9): TPM_RC_LOCALITY.
    for i in 17..=22 {
        err(&extend(&mut t, i, &d), Rc::LOCALITY);
        assert_eq!(read_pcr(&mut t, i as usize), vec![0xFF; 32]);
    }
    assert_eq!(pcr_counter(&mut t), 0);
    // Reference value computed independently for an all-ones PCR (used by the sequence test).
    assert_eq!(hex(EXTEND_ABC_ON_FF).len(), 32);
}

#[test]
fn pcr_extend_with_null_handle_has_no_effect() {
    let mut t = started();
    ok(&run(
        &mut t,
        with_pw(
            cc::PCR_EXTEND,
            &[handle::RH_NULL],
            &[b""],
            &sha256_ha(&[1; 32]),
        ),
    ));
    assert_eq!(pcr_counter(&mut t), 0);
    // An empty digest list is also valid.
    ok(&run(
        &mut t,
        with_pw(cc::PCR_EXTEND, &[0], &[b""], &u32b(0)),
    ));
    assert_eq!(read_pcr(&mut t, 0), vec![0u8; 32]);
}

#[test]
fn pcr_event_hashes_and_extends() {
    let mut t = started();
    let r = run(&mut t, with_pw(cc::PCR_EVENT, &[0], &[b""], &tpm2b(b"abc")));
    let list = TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0))).unwrap();
    assert_eq!(list.digests.len(), 2);
    assert_eq!(list.digests[0].hash, alg::SHA256);
    assert_eq!(list.digests[0].digest, hex(SHA256_ABC));
    assert_eq!(list.digests[1].hash, alg::SHA384);
    assert_eq!(list.digests[1].digest, hex(SHA384_ABC));
    assert_eq!(read_pcr(&mut t, 0), hex(EXTEND_ABC_ON_ZERO));
    assert_eq!(
        read_pcr_bank(&mut t, 0, alg::SHA384),
        hex(SHA384_EXTEND_ABC_ON_ZERO)
    );
    // Empty event data is hashed too (size 0 is valid); 1024 is the maximum.
    let r = run(&mut t, with_pw(cc::PCR_EVENT, &[1], &[b""], &tpm2b(&[])));
    let list = TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0))).unwrap();
    assert_eq!(list.digests[0].digest, hex(SHA256_EMPTY));
    ok(&run(
        &mut t,
        with_pw(cc::PCR_EVENT, &[1], &[b""], &tpm2b(&[0; 1024])),
    ));
    // TPM_RH_NULL: the digests are returned, no PCR changes.
    let before = pcr_counter(&mut t);
    let r = run(
        &mut t,
        with_pw(cc::PCR_EVENT, &[handle::RH_NULL], &[b""], &tpm2b(b"abc")),
    );
    assert_eq!(
        TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0)))
            .unwrap()
            .digests
            .len(),
        2
    );
    assert_eq!(pcr_counter(&mut t), before);
}

#[test]
fn pcr_event_errors() {
    let mut t = started();
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EVENT, &[0], &[b""], &tpm2b(&[0; 1025])),
        ),
        Rc::SIZE.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::PCR_EVENT, &[0], &tpm2b(b"x"))),
        Rc::AUTH_MISSING,
    );
    err(
        &run(&mut t, with_pw(cc::PCR_EVENT, &[0], &[b"x"], &tpm2b(b"x"))),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(&mut t, with_pw(cc::PCR_EVENT, &[22], &[b""], &tpm2b(b"x"))),
        Rc::LOCALITY,
    );
    err(
        &run(&mut t, with_pw(cc::PCR_EVENT, &[99], &[b""], &tpm2b(b"x"))),
        Rc::VALUE.in_handle(1),
    );
    err(
        &run(&mut t, with_pw(cc::PCR_EVENT, &[0], &[b""], &[0, 4, 1])),
        Rc::INSUFFICIENT.in_param(1),
    );
    assert_eq!(read_pcr(&mut t, 0), vec![0u8; 32]);
}

#[test]
fn pcr_reset_only_for_pcr_16_and_23_at_locality_0() {
    let mut t = started();
    let d = [9u8; 32];
    ok(&extend(&mut t, 16, &d));
    ok(&extend(&mut t, 23, &d));
    ok(&extend(&mut t, 0, &d));
    let counter = pcr_counter(&mut t);
    for i in [16u32, 23] {
        let r = run(&mut t, with_pw(cc::PCR_RESET, &[i], &[b""], &[]));
        ok(&r);
        assert_eq!(read_pcr(&mut t, i as usize), vec![0u8; 32]);
    }
    // PCR 16 and 23 have TPM_PT_PCR_NO_INCREMENT: the counter does not move.
    assert_eq!(pcr_counter(&mut t), counter);
    // Every other PCR: TPM_RC_LOCALITY (Part 3 clause 22.8).
    for i in (0u32..16).chain(17..24).filter(|i| *i != 23) {
        err(
            &run(&mut t, with_pw(cc::PCR_RESET, &[i], &[b""], &[])),
            Rc::LOCALITY,
        );
    }
    assert_ne!(read_pcr(&mut t, 0), vec![0u8; 32]);
    assert_eq!(read_pcr(&mut t, 17), vec![0xFF; 32]);
}

#[test]
fn pcr_reset_errors() {
    let mut t = started();
    err(
        &run(&mut t, plain(cc::PCR_RESET, &[16], &[])),
        Rc::AUTH_MISSING,
    );
    err(
        &run(&mut t, with_pw(cc::PCR_RESET, &[16], &[b"x"], &[])),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(&mut t, with_pw(cc::PCR_RESET, &[24], &[b""], &[])),
        Rc::VALUE.in_handle(1),
    );
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_RESET, &[handle::RH_NULL], &[b""], &[]),
        ),
        Rc::VALUE.in_handle(1),
    );
    err(
        &run(&mut t, with_pw(cc::PCR_RESET, &[16], &[b""], &[0])),
        Rc::SIZE,
    );
}

// ---------------------------------------------------------------- Hash

fn hash_cmd(data: &[u8], hash: u16, hierarchy: u32) -> Vec<u8> {
    plain(
        cc::HASH,
        &[],
        &cat(&[&tpm2b(data), &u16b(hash), &u32b(hierarchy)]),
    )
}

fn digest_and_ticket(resp: &[u8]) -> (Vec<u8>, TpmtTkHashcheck) {
    let mut rd = Reader::new(params(resp, 0));
    let d = rd.tpm2b(limits::MAX_PCR_DIGEST_SIZE).unwrap().to_vec();
    let t = TpmtTkHashcheck::unmarshal(&mut rd).unwrap();
    rd.finish().unwrap();
    (d, t)
}

#[test]
fn hash_returns_sha256_and_a_null_ticket() {
    let mut t = started();
    for h in [
        handle::RH_NULL,
        handle::RH_OWNER,
        handle::RH_PLATFORM,
        handle::RH_ENDORSEMENT,
    ] {
        let (d, ticket) = digest_and_ticket(&run(&mut t, hash_cmd(b"abc", alg::SHA256, h)));
        assert_eq!(d, hex(SHA256_ABC));
        assert_eq!(
            ticket,
            TpmtTkHashcheck {
                hierarchy: handle::RH_NULL,
                digest: vec![]
            }
        );
    }
    let (d, _) = digest_and_ticket(&run(&mut t, hash_cmd(b"", alg::SHA256, handle::RH_NULL)));
    assert_eq!(d, hex(SHA256_EMPTY));
    // Maximum input.
    ok(&run(
        &mut t,
        hash_cmd(&[0; 1024], alg::SHA256, handle::RH_NULL),
    ));
}

#[test]
fn hash_and_hash_sequence_accept_sha384() {
    let mut t = started();
    let direct = run(&mut t, hash_cmd(b"abc", alg::SHA384, handle::RH_NULL));
    assert_eq!(digest_and_ticket(&direct).0, hex(SHA384_ABC));
    let seq = seq_handle(&start_seq(&mut t, b"pw", alg::SHA384));
    ok(&seq_update(&mut t, seq, b"pw", b"a"));
    let streamed = seq_complete(&mut t, seq, b"pw", b"bc", handle::RH_NULL);
    assert_eq!(params(&streamed, 0), params(&direct, 0));
}

#[test]
fn hash_errors() {
    let mut t = started();
    err(
        &run(&mut t, hash_cmd(&[0; 1025], alg::SHA256, handle::RH_NULL)),
        Rc::SIZE.in_param(1),
    );
    // SHA-1 and NULL are not implemented/allowed: TPM_RC_HASH, parameter 2.
    err(
        &run(&mut t, hash_cmd(b"a", 0x0004, handle::RH_NULL)),
        Rc::HASH.in_param(2),
    );
    err(
        &run(&mut t, hash_cmd(b"a", alg::NULL, handle::RH_NULL)),
        Rc::HASH.in_param(2),
    );
    // Not a hierarchy: TPM_RC_VALUE, parameter 3.
    err(
        &run(&mut t, hash_cmd(b"a", alg::SHA256, 0x4000_000A)),
        Rc::VALUE.in_param(3),
    );
    err(
        &run(&mut t, hash_cmd(b"a", alg::SHA256, 0)),
        Rc::VALUE.in_param(3),
    );
    // Truncated and oversized.
    let mut c = hash_cmd(b"a", alg::SHA256, handle::RH_NULL);
    c.pop();
    let n = c.len() as u32;
    c[2..6].copy_from_slice(&n.to_be_bytes());
    err(&run(&mut t, c), Rc::INSUFFICIENT.in_param(3));
    let mut c = hash_cmd(b"a", alg::SHA256, handle::RH_NULL);
    c.push(0);
    let n = c.len() as u32;
    c[2..6].copy_from_slice(&n.to_be_bytes());
    err(&run(&mut t, c), Rc::SIZE);
}

// ---------------------------------------------------------------- sequences

fn start_seq(t: &mut Tpm, auth: &[u8], hash: u16) -> Vec<u8> {
    run(
        t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(auth), &u16b(hash)]),
        ),
    )
}

fn seq_handle(resp: &[u8]) -> u32 {
    ok(resp);
    assert_eq!(u16::from_be_bytes([resp[0], resp[1]]), st::NO_SESSIONS);
    assert_eq!(resp.len(), 14);
    u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]])
}

fn seq_update(t: &mut Tpm, h: u32, pw: &[u8], data: &[u8]) -> Vec<u8> {
    run(t, with_pw(cc::SEQUENCE_UPDATE, &[h], &[pw], &tpm2b(data)))
}

fn seq_complete(t: &mut Tpm, h: u32, pw: &[u8], data: &[u8], hierarchy: u32) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::SEQUENCE_COMPLETE,
            &[h],
            &[pw],
            &cat(&[&tpm2b(data), &u32b(hierarchy)]),
        ),
    )
}

#[test]
fn hash_sequence_matches_one_shot_hash() {
    let mut t = started();
    let h = seq_handle(&start_seq(&mut t, b"pw", alg::SHA256));
    assert_eq!(h >> 24, 0x80, "a transient object handle");
    let r = seq_update(&mut t, h, b"pw", b"ab");
    ok(&r);
    ok(&seq_update(&mut t, h, b"pw", b""));
    let r = seq_complete(&mut t, h, b"pw", b"c", handle::RH_NULL);
    let (d, ticket) = digest_and_ticket(&r);
    assert_eq!(d, hex(SHA256_ABC));
    assert_eq!(ticket.hierarchy, handle::RH_NULL);
    // The sequence is flushed by SequenceComplete (the {F} attribute).
    err(&seq_update(&mut t, h, b"pw", b"x"), Rc::reference_handle(0));
    err(
        &seq_complete(&mut t, h, b"pw", b"x", handle::RH_NULL),
        Rc::reference_handle(0),
    );
    // Long input: 5 updates of 1024 octets, the independent vector is checked below.
    let h = seq_handle(&start_seq(&mut t, b"", alg::SHA256));
    for _ in 0..5 {
        ok(&run(
            &mut t,
            with_pw(cc::SEQUENCE_UPDATE, &[h], &[b""], &tpm2b(&[0xA5; 1024])),
        ));
    }
    let (d, _) = digest_and_ticket(&seq_complete(&mut t, h, b"", &[], handle::RH_OWNER));
    let mut all = Vec::new();
    for _ in 0..5 {
        all.extend_from_slice(&[0xA5; 1024]);
    }
    let h2 = seq_handle(&start_seq(&mut t, b"", alg::SHA256));
    for range in [0..1000, 1000..2024, 2024..3000, 3000..4000, 4000..5000] {
        ok(&run(
            &mut t,
            with_pw(cc::SEQUENCE_UPDATE, &[h2], &[b""], &tpm2b(&all[range])),
        ));
    }
    let (d2, _) = digest_and_ticket(&seq_complete(
        &mut t,
        h2,
        b"",
        &all[5000..],
        handle::RH_NULL,
    ));
    // Computed independently (Python hashlib): sha256(5120 x 0xA5).
    assert_eq!(
        d2,
        hex("5d7c83bb5bdf240b1ac2de61b2fdba3ef3c91b2827a43134b558fe03c9519dac")
    );
    assert_eq!(d, d2, "chunking does not change the digest");
}

#[test]
fn sequence_authorization_and_handle_errors() {
    let mut t = started();
    let h = seq_handle(&start_seq(&mut t, b"pw", alg::SHA256));
    err(
        &run(&mut t, plain(cc::SEQUENCE_UPDATE, &[h], &tpm2b(b"x"))),
        Rc::AUTH_MISSING,
    );
    err(
        &seq_update(&mut t, h, b"bad", b"x"),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &seq_update(&mut t, h, b"", b"x"),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &seq_complete(&mut t, h, b"bad", b"", handle::RH_NULL),
        Rc::BAD_AUTH.in_session(1),
    );
    // A failed command leaves the sequence usable.
    ok(&seq_update(&mut t, h, b"pw", b"x"));
    // Handle types: PCR handle, session handle, persistent handle, unloaded transient handle.
    err(&seq_update(&mut t, 0, b"", b"x"), Rc::VALUE.in_handle(1));
    err(
        &seq_update(&mut t, 0x0200_0000, b"", b"x"),
        Rc::VALUE.in_handle(1),
    );
    err(
        &seq_update(&mut t, 0x8100_0000, b"", b"x"),
        Rc::HANDLE.in_handle(1),
    );
    err(
        &seq_update(&mut t, 0x8000_0001, b"", b"x"),
        Rc::reference_handle(0),
    );
    err(
        &seq_update(&mut t, 0x80FF_FFFF, b"", b"x"),
        Rc::reference_handle(0),
    );
    // Parameter errors.
    err(
        &seq_update(&mut t, h, b"pw", &[0; 1025]),
        Rc::SIZE.in_param(1),
    );
    err(
        &seq_complete(&mut t, h, b"pw", b"", 0x4000_000A),
        Rc::VALUE.in_param(2),
    );
    err(
        &seq_complete(&mut t, h, b"pw", &[0; 1025], handle::RH_NULL),
        Rc::SIZE.in_param(1),
    );
    // Still usable after all of the above.
    ok(&seq_complete(&mut t, h, b"pw", b"", handle::RH_NULL));
}

#[test]
fn hash_sequence_start_errors_and_slot_limit() {
    let mut t = started();
    err(
        &start_seq(&mut t, &[0; 33], alg::SHA256),
        Rc::SIZE.in_param(1),
    );
    err(&start_seq(&mut t, b"", 0x0004), Rc::HASH.in_param(2));
    err(&start_seq(&mut t, b"", 0x000D), Rc::HASH.in_param(2));
    err(
        &run(&mut t, plain(cc::HASH_SEQUENCE_START, &[], &tpm2b(b""))),
        Rc::INSUFFICIENT.in_param(2),
    );
    err(
        &run(
            &mut t,
            plain(
                cc::HASH_SEQUENCE_START,
                &[],
                &cat(&[&tpm2b(b""), &u16b(alg::NULL), &[0]]),
            ),
        ),
        Rc::SIZE,
    );
    let handles: Vec<u32> = (0..3)
        .map(|_| seq_handle(&start_seq(&mut t, b"", alg::SHA256)))
        .collect();
    assert_eq!(handles.len(), 3);
    assert!(handles.windows(2).all(|w| w[0] != w[1]));
    // A fourth sequence: TPM_RC_OBJECT_MEMORY (warning).
    err(&start_seq(&mut t, b"", alg::SHA256), Rc::OBJECT_MEMORY);
    // Completing one frees its slot.
    ok(&seq_complete(&mut t, handles[1], b"", b"", handle::RH_NULL));
    let again = seq_handle(&start_seq(&mut t, b"", alg::NULL));
    assert_eq!(again, handles[1]);
}

#[test]
fn sequence_kind_mismatch_is_mode() {
    let mut t = started();
    let hash = seq_handle(&start_seq(&mut t, b"", alg::SHA256));
    let event = seq_handle(&start_seq(&mut t, b"", alg::NULL));
    // SequenceComplete on an event sequence, EventSequenceComplete on a hash sequence.
    err(
        &seq_complete(&mut t, event, b"", b"", handle::RH_NULL),
        Rc::MODE.in_handle(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVENT_SEQUENCE_COMPLETE,
                &[8, hash],
                &[b"", b""],
                &tpm2b(b""),
            ),
        ),
        Rc::MODE.in_handle(2),
    );
    // Both sequences survive the failures; SequenceUpdate works on either kind.
    ok(&seq_update(&mut t, hash, b"", b"a"));
    ok(&seq_update(&mut t, event, b"", b"a"));
}

#[test]
fn event_sequence_extends_the_pcr_with_the_digest() {
    let mut t = started();
    let seq = seq_handle(&start_seq(&mut t, b"evt", alg::NULL));
    ok(&seq_update(&mut t, seq, b"evt", b"a"));
    ok(&seq_update(&mut t, seq, b"evt", b"b"));
    let r = run(
        &mut t,
        with_pw(
            cc::EVENT_SEQUENCE_COMPLETE,
            &[8, seq],
            &[b"", b"evt"],
            &tpm2b(b"c"),
        ),
    );
    let list = TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0))).unwrap();
    assert_eq!(list.digests.len(), 2);
    assert_eq!(list.digests[0].hash, alg::SHA256);
    // The digest of "abc" is extended into PCR 8 (computed independently: EXTEND_ABC_ON_ZERO).
    assert_eq!(list.digests[0].digest, hex(SHA256_ABC));
    assert_eq!(list.digests[1].hash, alg::SHA384);
    assert_eq!(list.digests[1].digest, hex(SHA384_ABC));
    assert_eq!(read_pcr(&mut t, 8), hex(EXTEND_ABC_ON_ZERO));
    assert_eq!(
        read_pcr_bank(&mut t, 8, alg::SHA384),
        hex(SHA384_EXTEND_ABC_ON_ZERO)
    );
    assert_eq!(pcr_counter(&mut t), 2);
    // Flushed.
    err(
        &seq_update(&mut t, seq, b"evt", b"x"),
        Rc::reference_handle(0),
    );
}

#[test]
fn event_sequence_complete_failures_keep_the_sequence() {
    let mut t = started();
    let seq = seq_handle(&start_seq(&mut t, b"", alg::NULL));
    // PCR 17 cannot be extended at locality 0: the command fails and the sequence is not flushed.
    let c = |pcr: u32, pw: [&[u8]; 2]| {
        with_pw(
            cc::EVENT_SEQUENCE_COMPLETE,
            &[pcr, seq],
            &pw,
            &tpm2b(b"abc"),
        )
    };
    err(&run(&mut t, c(17, [b"", b""])), Rc::LOCALITY);
    assert_eq!(read_pcr(&mut t, 17), vec![0xFF; 32]);
    // Authorization of either handle, handle errors, missing session.
    err(&run(&mut t, c(8, [b"x", b""])), Rc::BAD_AUTH.in_session(1));
    err(&run(&mut t, c(8, [b"", b"x"])), Rc::BAD_AUTH.in_session(2));
    err(&run(&mut t, c(99, [b"", b""])), Rc::VALUE.in_handle(1));
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVENT_SEQUENCE_COMPLETE,
                &[8, seq],
                &[b""],
                &tpm2b(b"abc"),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVENT_SEQUENCE_COMPLETE,
                &[8, 0x8000_0002],
                &[b"", b""],
                &tpm2b(b"abc"),
            ),
        ),
        Rc::reference_handle(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVENT_SEQUENCE_COMPLETE,
                &[8, seq],
                &[b"", b""],
                &tpm2b(&[0; 1025]),
            ),
        ),
        Rc::SIZE.in_param(1),
    );
    // TPM_RH_NULL as pcrHandle: digests are returned, nothing is extended, the sequence ends.
    let r = run(&mut t, c(handle::RH_NULL, [b"", b""]));
    assert_eq!(
        TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0)))
            .unwrap()
            .digests
            .len(),
        2
    );
    assert_eq!(pcr_counter(&mut t), 0);
    err(&seq_update(&mut t, seq, b"", b"x"), Rc::reference_handle(0));
}

// ---------------------------------------------------------------- ReadClock / state

#[test]
fn read_clock_reports_time_clock_and_counters() {
    let mut t = started();
    let c = clock(&mut t, 1234);
    assert_eq!(c.time, 1234);
    assert_eq!(c.clock_info.clock, 1234);
    assert_eq!(
        (c.clock_info.reset_count, c.clock_info.restart_count),
        (1, 0)
    );
    // The owner's time never runs backwards.
    assert_eq!(clock(&mut t, 1000).time, 1234);
    // Clock keeps the time of the earlier power cycle; Time restarts at _TPM_Init.
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    let c = clock(&mut t, 10);
    assert_eq!(c.time, 10);
    assert_eq!(c.clock_info.clock, 1244);
    assert_eq!(c.clock_info.reset_count, 2);
    // Errors.
    err(&run(&mut t, plain(cc::READ_CLOCK, &[], &[0])), Rc::SIZE);
}

#[test]
fn clock_safe_flag_follows_orderly_shutdown() {
    let mut t = started();
    assert!(clock(&mut t, 0).clock_info.safe, "a fresh TPM starts safe");
    // Power cycle without Shutdown: disorderly.
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert!(!clock(&mut t, 0).clock_info.safe);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert!(clock(&mut t, 0).clock_info.safe);
}

#[test]
fn snapshot_restore_keeps_counters_and_clock_but_not_pcr() {
    let mut t = started();
    ok(&extend(&mut t, 0, &[1; 32]));
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    let _ = t.execute(0, &plain(cc::GET_RANDOM, &[], &[0, 1]), 5000);
    let snapshot = t.persistent_snapshot();
    let mut r = Tpm::restore(&snapshot, &[3; 48]).unwrap();
    ok(&run(&mut r, plain(cc::STARTUP, &[], &[0, 0])));
    let c = clock(&mut r, 0);
    assert_eq!(c.clock_info.reset_count, 2);
    assert_eq!(c.clock_info.clock, 5000);
    assert!(c.clock_info.safe);
    assert_eq!(read_pcr(&mut r, 0), vec![0u8; 32], "PCR are volatile");
    // A restored TPM needs Startup first.
    let mut r2 = Tpm::restore(&snapshot, &[3; 48]).unwrap();
    err(
        &run(&mut r2, plain(cc::GET_RANDOM, &[], &[0, 1])),
        Rc::INITIALIZE,
    );
    // Fresh entropy: the restored generator does not repeat the original one.
    let mut again = Tpm::restore(&snapshot, &[4; 48]).unwrap();
    ok(&run(&mut again, plain(cc::STARTUP, &[], &[0, 0])));
    let a = run(&mut r, plain(cc::GET_RANDOM, &[], &u16b(32)));
    let b = run(&mut again, plain(cc::GET_RANDOM, &[], &u16b(32)));
    assert_ne!(a, b);
}

#[test]
fn secrets_do_not_appear_in_debug_output() {
    let t = started();
    let s = format!("{t:?}");
    assert!(s.starts_with("Tpm {"));
    assert!(!s.contains("seed") && !s.contains("drbg"));
}
