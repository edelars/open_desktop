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
#[path = "tests_obj.rs"]
mod tests_obj;

const ENTROPY: [u8; 48] = [5; 48];

// Computed independently (Python hashlib).
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
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
    let mut sel = [0u8; 3];
    sel[pcr / 8] |= 1 << (pcr % 8);
    let r = run(
        t,
        plain(
            cc::PCR_READ,
            &[],
            &cat(&[&u32b(1), &u16b(alg::SHA256), &[3], &sel]),
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
fn pcr_allocate_is_refused_as_unsupported_command() {
    // TPM2_PCR_Allocate (0x12B) is not supported in phase A (docs/specs/VTPM.md); it must not
    // pretend to succeed and must not be listed by GetCapability(COMMANDS).
    let mut t = started();
    let r = run(
        &mut t,
        with_pw(0x12B, &[handle::RH_PLATFORM], &[b""], &u32b(0)),
    );
    err(&r, Rc::COMMAND_CODE);
    assert!(!listed_commands(&mut t).contains(&0x12B));
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
    ok(&extend(&mut t, 0, &[1; 32]));
    ok(&extend(&mut t, 16, &[2; 32]));
    let pcr0 = read_pcr(&mut t, 0);
    let counter = pcr_counter(&mut t);
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])));
    // TPM_PT_PCR_SAVE PCR (0-15) are restored, the others take their initial value (PTP Table 14).
    assert_eq!(read_pcr(&mut t, 0), pcr0);
    assert_eq!(read_pcr(&mut t, 16), vec![0u8; 32]);
    assert_eq!(read_pcr(&mut t, 17), vec![0xFFu8; 32]);
    assert_eq!(pcr_counter(&mut t), counter);
    // Resume increments restartCount only.
    let c = clock(&mut t, 0).clock_info;
    assert_eq!((c.reset_count, c.restart_count), (1, 1));
    // The saved state is consumed: Startup(STATE) cannot be repeated after another _TPM_Init.
    t.init();
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
}

#[test]
fn shutdown_state_then_startup_clear_is_restart() {
    let mut t = started();
    ok(&extend(&mut t, 0, &[1; 32]));
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    assert_eq!(read_pcr(&mut t, 0), vec![0u8; 32]);
    let c = clock(&mut t, 0).clock_info;
    assert_eq!((c.reset_count, c.restart_count), (1, 1));
}

#[test]
fn shutdown_clear_then_startup_clear_is_reset_and_state_is_refused() {
    let mut t = started();
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    t.init();
    err(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])), Rc(0x1C4));
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
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
fn failure_mode_answers_only_get_test_result_and_get_capability() {
    let mut t = started();
    t.force_failure_mode();
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
        0x120, 0x121, 0x122, 0x126, 0x127, 0x129, 0x12A, 0x131, 0x137, 0x138, 0x139, 0x13A, 0x13C,
        0x13D, 0x13E, 0x143, 0x144, 0x145, 0x146, 0x14E, 0x14F, 0x153, 0x154, 0x157, 0x159, 0x15C,
        0x15D, 0x15E, 0x165, 0x167, 0x169, 0x173, 0x174, 0x176, 0x177, 0x17A, 0x17B, 0x17C, 0x17D,
        0x17E, 0x181, 0x182, 0x185, 0x186,
        // Slice C2: Certify, PolicySecret, HMAC, Quote, ContextLoad/Save, PolicyAuthValue,
        // PolicyCommandCode, PolicyOR, PolicyPCR, PolicyRestart, PolicyGetDigest.
        0x148, 0x151, 0x155, 0x158, 0x161, 0x162, 0x16B, 0x16C, 0x171, 0x17F, 0x180, 0x189,
    ];
    let mut expected = expected;
    expected.sort_unstable();
    assert_eq!(expected.len(), 56);
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
    // TPM_PT_TOTAL_COMMANDS agrees with the list.
    let (_, _, props, count) =
        cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::TOTAL_COMMANDS, 1));
    assert_eq!(count, 1);
    assert_eq!(
        props,
        [&u32b(pt::TOTAL_COMMANDS)[..], &u32b(56)[..]].concat()
    );
}

#[test]
fn capability_commands_paging_and_more_data() {
    let mut t = started();
    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0, 5));
    assert!(more && count == 5 && body.len() == 20);
    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x17E, 100));
    assert!(
        !more && count == 8,
        "0x17E, 0x17F, 0x180, 0x181, 0x182, 0x185, 0x186, 0x189"
    );
    assert_eq!(body.len(), 32);
    let (more, _, _, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x18A, 100));
    assert!(!more && count == 0);
    // propertyCount 0: empty list, moreData YES because entries remain.
    let (more, _, _, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0, 0));
    assert!(more && count == 0);
}

#[test]
fn capability_algs_pcrs_and_handles() {
    let mut t = started();
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::ALGS, 0, 100));
    assert!(!more && c == cap::ALGS && count == 12);
    // RSA, HMAC, AES, KEYEDHASH, SHA256, RSASSA, RSAES, OAEP, ECDSA, ECDH, ECC, CFB with their
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
            &[0x00, 0x0B, 0, 0, 0, 4],
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
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x0C, 100)).3, 7);
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x24, 100)).3, 1);
    assert_eq!(cap_list(&get_cap(&mut t, cap::ALGS, 0x44, 100)).3, 0);

    let r = get_cap(&mut t, cap::PCRS, 0, 100);
    let p = params(&r, 0);
    assert_eq!(p[0], 0);
    let sel = TpmlPcrSelection::unmarshal(&mut Reader::new(&p[5..])).unwrap();
    assert_eq!(sel.selections.len(), 1);
    assert_eq!(sel.selections[0].hash, alg::SHA256);
    assert_eq!(sel.selections[0].select, [0xFF; 3]);
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
    for c in [0x7, 0x9, 0xA, 0xB, 0x100, 0xFFFF_FFFF] {
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
        Rc::AUTHSIZE,
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
    let two = cat(&[
        &u32b(2),
        &u16b(alg::SHA256),
        &[3, 1, 0, 0],
        &u16b(alg::SHA256),
        &[3, 1, 0, 0],
    ]);
    err(
        &run(&mut t, plain(cc::PCR_READ, &[], &two)),
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
    // Two sessions for one authorization handle: TPM_RC_AUTHSIZE.
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[3], &[b"", b""], &sha256_ha(&d)),
        ),
        Rc::AUTHSIZE,
    );
    // Session tag with an empty authorization area counts as missing.
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[], &sha256_ha(&d))),
        Rc::AUTH_MISSING,
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
    // Two digests (HASH_COUNT is 1): TPM_RC_SIZE.
    let two = cat(&[&u32b(2), &u16b(alg::SHA256), &d, &u16b(alg::SHA256), &d]);
    err(
        &run(&mut t, with_pw(cc::PCR_EXTEND, &[3], &[b""], &two)),
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
    assert_eq!(list.digests.len(), 1);
    assert_eq!(list.digests[0].hash, alg::SHA256);
    assert_eq!(list.digests[0].digest, hex(SHA256_ABC));
    assert_eq!(read_pcr(&mut t, 0), hex(EXTEND_ABC_ON_ZERO));
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
        1
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
    let d = rd.tpm2b(32).unwrap().to_vec();
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
    err(&start_seq(&mut t, b"", 0x000C), Rc::HASH.in_param(2));
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
    assert_eq!(list.digests.len(), 1);
    assert_eq!(list.digests[0].hash, alg::SHA256);
    // The digest of "abc" is extended into PCR 8 (computed independently: EXTEND_ABC_ON_ZERO).
    assert_eq!(list.digests[0].digest, hex(SHA256_ABC));
    assert_eq!(read_pcr(&mut t, 8), hex(EXTEND_ABC_ON_ZERO));
    assert_eq!(pcr_counter(&mut t), 1);
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
        1
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
