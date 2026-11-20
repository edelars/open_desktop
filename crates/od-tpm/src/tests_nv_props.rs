//! Slice 3 of S7-23: NV ORDERLY counters, `TPM_NT_PIN_FAIL` / `TPM_NT_PIN_PASS` and the
//! `TPM_PT` properties of PTP 1.07, Table 2, that were missing. Every command goes through
//! `Tpm::execute` with raw bytes.

use sha2::{Digest, Sha256};

use super::tests_b::{
    define, define_raw, nv_attributes, nv_public, nv_read, nv_read_data, nv_read_public, nv_write,
    pw_cmd, shutdown, start_session, startup,
};
use super::tests_obj::{create_primary, parse_primary, rsa_storage};
use super::*;
use crate::codes::se;
use crate::nv::attr;

const OWNER: u32 = handle::RH_OWNER;
const NV0: u32 = 0x0100_0000;
const OWNER_RW: u32 = attr::OWNERWRITE | attr::OWNERREAD;
const NT_COUNTER: u32 = 0x10;
const NT_BITS: u32 = 0x20;
const NT_EXTEND: u32 = 0x40;

// ------------------------------------------------------------------------ golden (SHA-256)

fn record(log: &mut Vec<u8>, response: Vec<u8>) -> Vec<u8> {
    log.extend_from_slice(&(response.len() as u32).to_be_bytes());
    log.extend_from_slice(&response);
    response
}

/// A fixed scenario of what was implemented before slice 3 (NV types, PCR, capability lists other
/// than `TPM_CAP_TPM_PROPERTIES`, `Shutdown`, snapshot, `Startup`): every response and both
/// snapshots, hashed. The value was taken on commit fbb4f38 before slice 3.
fn golden_transcript() -> String {
    let mut log = Vec::new();
    let mut t = started();
    let l = &mut log;
    record(l, define(&mut t, OWNER, NV0, OWNER_RW, 5));
    record(l, nv_write(&mut t, OWNER, NV0, b"", b"hello", 0));
    record(l, nv_read(&mut t, OWNER, NV0, b"", 5, 0));
    for (i, ty, size) in [
        (1, NT_COUNTER, 8),
        (2, NT_COUNTER, 8),
        (3, NT_BITS, 8),
        (4, NT_EXTEND, 32),
    ] {
        record(l, define(&mut t, OWNER, NV0 + i, OWNER_RW | ty, size));
    }
    for _ in 0..4 {
        for i in [1, 2] {
            record(
                l,
                pw_cmd(&mut t, cc::NV_INCREMENT, &[OWNER, NV0 + i], b"", &[]),
            );
        }
    }
    record(
        l,
        pw_cmd(
            &mut t,
            cc::NV_SET_BITS,
            &[OWNER, NV0 + 3],
            b"",
            &0x1234u64.to_be_bytes(),
        ),
    );
    record(
        l,
        pw_cmd(
            &mut t,
            cc::NV_EXTEND,
            &[OWNER, NV0 + 4],
            b"",
            &tpm2b(b"abc"),
        ),
    );
    for (i, size) in [(1, 8), (2, 8), (3, 8), (4, 32)] {
        record(l, nv_read(&mut t, OWNER, NV0 + i, b"", size, 0));
    }
    record(
        l,
        pw_cmd(&mut t, cc::NV_UNDEFINE_SPACE, &[OWNER, NV0 + 2], b"", &[]),
    );
    for i in [0, 1, 3, 4] {
        let (public, name) = nv_read_public(&mut t, NV0 + i);
        l.extend_from_slice(&public);
        l.extend_from_slice(&name);
    }
    // Refused definitions keep their codes.
    for a in [
        attr::WRITTEN | OWNER_RW,
        OWNER_RW | 0x30,
        OWNER_RW | attr::GLOBALLOCK,
        OWNER_RW | attr::CLEAR_STCLEAR,
    ] {
        record(l, define(&mut t, OWNER, NV0 + 9, a, 8));
    }
    record(l, extend(&mut t, 1, &hex(SHA256_ABC)));
    record(
        l,
        run(
            &mut t,
            plain(cc::PCR_READ, &[], &hex("00000001000b03020000")),
        ),
    );
    record(l, get_cap(&mut t, cap::HANDLES, 0x0100_0000, 16));
    record(l, get_cap(&mut t, cap::ALGS, 0, 64));
    record(l, get_cap(&mut t, cap::PCRS, 0, 8));
    shutdown(&mut t, 1);
    let snapshot = t.persistent_snapshot();
    l.extend_from_slice(&snapshot);
    let mut r = Tpm::restore(&snapshot, &[9; 48]).unwrap();
    startup(&mut r, 0);
    for i in [1, 3] {
        record(l, nv_read(&mut r, OWNER, NV0 + i, b"", 8, 0));
    }
    record(
        l,
        pw_cmd(&mut r, cc::NV_INCREMENT, &[OWNER, NV0 + 1], b"", &[]),
    );
    shutdown(&mut r, 0);
    l.extend_from_slice(&r.persistent_snapshot());
    Sha256::digest(&log)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn sha256_golden_unchanged() {
    assert_eq!(
        golden_transcript(),
        "7213c5cb8f83c555229cd431fb6b0212258dbce651d7a679bc04e845f3152770"
    );
}

// ------------------------------------------------------------------------------ helpers

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// One `TPM_CAP_TPM_PROPERTIES` value, `None` when the property is not listed.
fn property(t: &mut Tpm, tag: u32) -> Option<u32> {
    let (_, _, body, count) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, tag, 1));
    if count == 0 || be32(&body[..4]) != tag {
        return None;
    }
    Some(be32(&body[4..8]))
}

fn counter_at(t: &mut Tpm, index: u32) -> u64 {
    let data = nv_read_data(&nv_read(t, OWNER, index, b"", 8, 0));
    u64::from_be_bytes(data.try_into().unwrap())
}

fn increment(t: &mut Tpm, index: u32) {
    ok(&pw_cmd(t, cc::NV_INCREMENT, &[OWNER, index], b"", &[]));
}

const ORDERLY_COUNTER: u32 = OWNER_RW | NT_COUNTER | attr::ORDERLY;

// ------------------------------------------------------------------------- ORDERLY counters

#[test]
fn nv_orderly_counter_persists_across_shutdown_restart() {
    let mut t = started();
    ok(&define(&mut t, OWNER, NV0 + 1, ORDERLY_COUNTER, 8));
    ok(&define(&mut t, OWNER, NV0 + 2, OWNER_RW | NT_COUNTER, 8));
    assert_ne!(nv_attributes(&mut t, NV0 + 1) & attr::ORDERLY, 0);
    for _ in 0..5 {
        increment(&mut t, NV0 + 1);
        increment(&mut t, NV0 + 2);
    }
    // Counters share one floor (Part 3, clause 31.2): the plain counter started above the first.
    assert_eq!(
        (counter_at(&mut t, NV0 + 1), counter_at(&mut t, NV0 + 2)),
        (5, 6)
    );
    // A snapshot taken without Shutdown holds what NV holds: the first increment of an ORDERLY
    // counter reached NV, the later ones are in RAM; a plain counter is whole.
    let running = t.persistent_snapshot();
    // An orderly shutdown writes the ORDERLY counter; a Reset (Startup(CLEAR)) keeps it.
    shutdown(&mut t, 0);
    let snapshot = t.persistent_snapshot();
    let mut r = Tpm::restore(&snapshot, &[9; 48]).unwrap();
    assert_eq!(r.persistent_snapshot(), snapshot, "byte for byte");
    startup(&mut r, 0);
    assert_eq!(
        (counter_at(&mut r, NV0 + 1), counter_at(&mut r, NV0 + 2)),
        (5, 6)
    );
    increment(&mut r, NV0 + 1);
    assert_eq!(counter_at(&mut r, NV0 + 1), 6);
    // Shutdown(STATE), _TPM_Init, Startup(STATE): the value goes on.
    shutdown(&mut r, 1);
    r.init();
    startup(&mut r, 1);
    assert_eq!(counter_at(&mut r, NV0 + 1), 6);
    // Without Shutdown (the snapshot of a running TPM, and _TPM_Init) the counter comes back
    // from NV advanced by TPM_PT_ORDERLY_COUNT, so it never repeats a value: NV holds 6.
    increment(&mut r, NV0 + 1);
    let mut d = Tpm::restore(&r.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut d, 0);
    assert_eq!(counter_at(&mut d, NV0 + 1), 6 + 255);
    assert_eq!(counter_at(&mut d, NV0 + 2), 6, "a plain counter is whole");
    let mut e = Tpm::restore(&running, &[9; 48]).unwrap();
    startup(&mut e, 0);
    assert_eq!(counter_at(&mut e, NV0 + 1), 1 + 255);
    r.init();
    startup(&mut r, 0);
    assert_eq!(counter_at(&mut r, NV0 + 1), 6 + 255);
    // The next increment, and a counter defined now, start above everything the counter reported.
    increment(&mut r, NV0 + 1);
    assert_eq!(counter_at(&mut r, NV0 + 1), 6 + 255 + 1);
    ok(&define(&mut r, OWNER, NV0 + 3, OWNER_RW | NT_COUNTER, 8));
    increment(&mut r, NV0 + 3);
    assert!(counter_at(&mut r, NV0 + 3) > 6 + 255 + 1);
}

#[test]
fn orderly_counter_after_shutdown_increments_survive_restore() {
    let mut t = started();
    ok(&define(&mut t, OWNER, NV0 + 1, ORDERLY_COUNTER, 8));
    increment(&mut t, NV0 + 1);
    shutdown(&mut t, 0);
    // The TPM goes on after Shutdown; the snapshot is taken after the later increments.
    for _ in 0..10 {
        increment(&mut t, NV0 + 1);
    }
    assert_eq!(counter_at(&mut t, NV0 + 1), 11);
    let mut r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut r, 0);
    assert_eq!(counter_at(&mut r, NV0 + 1), 11);
}

#[test]
fn nv_orderly_counter_reaches_nv_every_orderly_count_increments() {
    let mut t = started();
    ok(&define(&mut t, OWNER, NV0 + 1, ORDERLY_COUNTER, 8));
    let n = u64::from(crate::nv::MAX_ORDERLY_COUNT);
    for _ in 0..(1 + n) {
        increment(&mut t, NV0 + 1);
    }
    // NV holds the first value (the move to WRITTEN) and then every n-th: the value 1 + n; a
    // disorderly Startup adds n to it.
    let mut r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut r, 0);
    assert_eq!(counter_at(&mut r, NV0 + 1), (1 + n) + n);
    // Only counters may be ORDERLY.
    for nt in [0, NT_BITS, NT_EXTEND, 0x80 | attr::NO_DA, 0x90] {
        err(
            &define(&mut t, OWNER, NV0 + 9, OWNER_RW | nt | attr::ORDERLY, 8),
            Rc::ATTRIBUTES.in_param(2),
        );
    }
}

#[test]
fn nv_orderly_count_and_capability() {
    let mut t = started();
    assert_eq!(property(&mut t, pt::ORDERLY_COUNT), Some(255));
    let max = property(&mut t, pt::NV_COUNTERS_MAX).unwrap();
    // PTP 1.07, clause 4.6.4: six counters, two of them ORDERLY.
    assert!(max >= 6);
    assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(0));
    assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(max));
    for i in 0..6 {
        let attributes = if i < 2 {
            ORDERLY_COUNTER
        } else {
            OWNER_RW | NT_COUNTER
        };
        ok(&define(&mut t, OWNER, NV0 + i, attributes, 8));
        assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(i + 1));
        assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(max - i - 1));
    }
    // The count does not depend on what the counters hold, and goes back on UndefineSpace.
    increment(&mut t, NV0);
    assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(6));
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, NV0 + 5],
        b"",
        &[],
    ));
    assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(5));
    assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(max - 5));
    // A bit field is no counter; it still takes an index slot.
    ok(&define(&mut t, OWNER, NV0 + 7, OWNER_RW | NT_BITS, 8));
    assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(5));
    assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(max - 6));
}

// ---------------------------------------------------------------------------- PIN indices

const NT_PIN_FAIL: u32 = 0x80;
const NT_PIN_PASS: u32 = 0x90;
const PIN: &[u8] = b"1234";

fn define_pin(t: &mut Tpm, index: u32, nt: u32, limit: u32) {
    let no_da = if nt == NT_PIN_FAIL { attr::NO_DA } else { 0 };
    // No AUTHWRITE: the PIN must not rewrite its own count and limit (the count is set through OWNER).
    let attributes = OWNER_RW | attr::AUTHREAD | nt | no_da;
    ok(&define_raw(
        t,
        OWNER,
        b"",
        PIN,
        &nv_public(index, alg::SHA256, attributes, &[], 8),
    ));
    // Unwritten: the PIN is not checked yet; the owner sets the count and the limit.
    err(&pin_read(t, index, PIN), Rc::NV_UNINITIALIZED);
    set_pin(t, index, 0, limit);
}

fn set_pin(t: &mut Tpm, index: u32, count: u32, limit: u32) {
    ok(&nv_write(
        t,
        OWNER,
        index,
        b"",
        &cat(&[&u32b(count), &u32b(limit)]),
        0,
    ));
}

fn pin_read(t: &mut Tpm, index: u32, pin: &[u8]) -> Vec<u8> {
    nv_read(t, index, index, pin, 8, 0)
}

fn pin_state(t: &mut Tpm, index: u32) -> (u32, u32) {
    let data = nv_read_data(&nv_read(t, OWNER, index, b"", 8, 0));
    (be32(&data[..4]), be32(&data[4..]))
}

#[test]
fn pin_fail_increments_and_locks_at_threshold() {
    let mut t = started();
    define_pin(&mut t, NV0 + 1, NT_PIN_FAIL, 3);
    // The type is in the public area; the data is 8 octets.
    assert_eq!(nv_attributes(&mut t, NV0 + 1) & attr::NT_MASK, NT_PIN_FAIL);
    ok(&pin_read(&mut t, NV0 + 1, PIN));
    assert_eq!(pin_state(&mut t, NV0 + 1), (0, 3));
    for n in 1..=3 {
        err(
            &pin_read(&mut t, NV0 + 1, b"bad"),
            Rc::BAD_AUTH.in_session(1),
        );
        assert_eq!(pin_state(&mut t, NV0 + 1), (n, 3));
    }
    // At the limit even the right PIN is refused, and nothing counts any more.
    err(&pin_read(&mut t, NV0 + 1, PIN), Rc::LOCKOUT);
    err(&pin_read(&mut t, NV0 + 1, b"bad"), Rc::LOCKOUT);
    assert_eq!(pin_state(&mut t, NV0 + 1), (3, 3));
    // The failures of a PIN do not count in the dictionary-attack lockout.
    assert_eq!(property(&mut t, pt::LOCKOUT_COUNTER), Some(0));
    // The lock survives a snapshot; the owner (other authorization) sets the count again.
    shutdown(&mut t, 0);
    let mut r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut r, 0);
    err(&pin_read(&mut r, NV0 + 1, PIN), Rc::LOCKOUT);
    set_pin(&mut r, NV0 + 1, 0, 3);
    ok(&pin_read(&mut r, NV0 + 1, PIN));
    // A write of the index is whole (8 octets at offset 0).
    err(
        &nv_write(&mut r, OWNER, NV0 + 1, b"", &[0, 0, 0, 0], 0),
        Rc::NV_RANGE,
    );
    err(
        &nv_write(&mut r, OWNER, NV0 + 1, b"", &[0; 4], 4),
        Rc::NV_RANGE,
    );
}

#[test]
fn pin_index_rejects_authwrite_and_writedefine() {
    let mut t = started();
    for (i, nt) in [NT_PIN_FAIL, NT_PIN_PASS].into_iter().enumerate() {
        let no_da = if nt == NT_PIN_FAIL { attr::NO_DA } else { 0 };
        let index = NV0 + 1 + i as u32;
        for bad in [attr::AUTHWRITE, attr::WRITEDEFINE] {
            let attributes = OWNER_RW | attr::AUTHREAD | nt | no_da | bad;
            err(
                &define_raw(
                    &mut t,
                    OWNER,
                    b"",
                    PIN,
                    &nv_public(index, alg::SHA256, attributes, &[], 8),
                ),
                Rc::ATTRIBUTES.in_param(2),
            );
        }
        // The same index without them is defined.
        define_pin(&mut t, index, nt, 3);
        // Whoever proves the PIN cannot rewrite the count and the limit with NV_Write.
        let before = pin_state(&mut t, index);
        let write = nv_write(&mut t, index, index, PIN, &[0; 8], 0);
        assert_ne!(be32(&write[6..10]), 0, "NV_Write with the PIN must fail");
        assert_eq!(pin_state(&mut t, index), before);
        let write = nv_write(
            &mut t,
            index,
            index,
            PIN,
            &cat(&[&u32b(0), &u32b(u32::MAX)]),
            0,
        );
        assert_ne!(be32(&write[6..10]), 0, "NV_Write with the PIN must fail");
        assert_eq!(pin_state(&mut t, index), before);
    }
}

#[test]
fn pin_pass_resets_fail_counter() {
    let mut t = started();
    define_pin(&mut t, NV0 + 1, NT_PIN_FAIL, 5);
    define_pin(&mut t, NV0 + 2, NT_PIN_PASS, 2);
    // A successful authorization clears the failures of a PIN_FAIL index.
    for _ in 0..2 {
        err(
            &pin_read(&mut t, NV0 + 1, b"bad"),
            Rc::BAD_AUTH.in_session(1),
        );
    }
    assert_eq!(pin_state(&mut t, NV0 + 1), (2, 5));
    ok(&pin_read(&mut t, NV0 + 1, PIN));
    assert_eq!(pin_state(&mut t, NV0 + 1), (0, 5));
    // A PIN_PASS index counts the successful authorizations (NV_Read with its own authValue), and a
    // failed one does not count.
    ok(&pin_read(&mut t, NV0 + 2, PIN));
    assert_eq!(pin_state(&mut t, NV0 + 2), (1, 2));
    err(
        &pin_read(&mut t, NV0 + 2, b"bad"),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(pin_state(&mut t, NV0 + 2), (1, 2));
    ok(&pw_cmd(
        &mut t,
        cc::NV_READ,
        &[NV0 + 2, NV0 + 2],
        PIN,
        &cat(&[&u16b(8), &u16b(0)]),
    ));
    assert_eq!(pin_state(&mut t, NV0 + 2), (2, 2));
    err(&pin_read(&mut t, NV0 + 2, PIN), Rc::LOCKOUT);
    assert_eq!(pin_state(&mut t, NV0 + 2), (2, 2));
    // Setting the count and the limit through OWNER proves no PIN (nothing is counted); the new
    // data is what the index holds.
    set_pin(&mut t, NV0 + 2, 0, 2);
    ok(&pin_read(&mut t, NV0 + 2, PIN));
    assert_eq!(pin_state(&mut t, NV0 + 2), (1, 2));
}

// ------------------------------------------------------------------------ TPM_PT (Table 2)

/// The first `count` properties of a group, from `first`.
fn properties_from(t: &mut Tpm, first: u32, count: u32) -> Vec<(u32, u32)> {
    let (_, _, body, n) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, first, count));
    assert_eq!(body.len(), 8 * n as usize);
    body.chunks(8)
        .map(|c| (be32(&c[..4]), be32(&c[4..])))
        .collect()
}

#[test]
fn capability_table2_properties_match_behaviour() {
    let mut t = started();
    let expected_fixed = [
        (pt::CONTEXT_GAP_MAX, 0),
        (pt::NV_COUNTERS_MAX, 128),
        (pt::CLOCK_UPDATE, 0),
        (pt::CONTEXT_HASH, u32::from(alg::SHA256)),
        (pt::CONTEXT_SYM, u32::from(alg::AES)),
        (pt::CONTEXT_SYM_SIZE, 128),
        (pt::ORDERLY_COUNT, 255),
        (pt::MAX_OBJECT_CONTEXT, 2066),
        (pt::MAX_SESSION_CONTEXT, 2066),
        (pt::SPLIT_MAX, 0),
        (pt::MODES, 0),
    ];
    for (tag, value) in expected_fixed {
        assert_eq!(property(&mut t, tag), Some(value), "property {tag:#x}");
    }
    let expected_var = [
        (pt::NV_COUNTERS, 0),
        (pt::NV_COUNTERS_AVAIL, 128),
        (pt::LOADED_CURVES, 2),
        (pt::NV_WRITE_RECOVERY, 0),
        (pt::AUDIT_COUNTER_0, 0),
        (pt::AUDIT_COUNTER_1, 0),
    ];
    for (tag, value) in expected_var {
        assert_eq!(property(&mut t, tag), Some(value), "property {tag:#x}");
    }
    // Both groups are listed in ascending order, once each, and contain the new tags.
    for group in [pt::FIXED, pt::VAR] {
        let all = properties_from(&mut t, group, 100);
        assert!(all.windows(2).all(|w| w[0].0 < w[1].0), "ascending");
        for (tag, _) in expected_fixed.iter().chain(&expected_var) {
            if tag / pt::GROUP == group / pt::GROUP {
                assert!(all.iter().any(|(listed, _)| listed == tag));
            }
        }
    }
    // The values follow the behaviour. Loaded curves: the list of `TPM_CAP_ECC_CURVES`.
    let (_, _, _, curves) = cap_list(&get_cap(&mut t, cap::ECC_CURVES, 0, 64));
    assert_eq!(property(&mut t, pt::LOADED_CURVES), Some(curves));
    // NV counters: defining a counter takes one of them, UndefineSpace gives it back.
    let before = property(&mut t, pt::NV_COUNTERS_AVAIL).unwrap();
    ok(&define(&mut t, OWNER, NV0 + 1, OWNER_RW | NT_COUNTER, 8));
    assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(before - 1));
    assert_eq!(property(&mut t, pt::NV_COUNTERS), Some(1));
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, NV0 + 1],
        b"",
        &[],
    ));
    assert_eq!(property(&mut t, pt::NV_COUNTERS_AVAIL), Some(before));
    // Contexts: AES-128-CFB, SHA-256 integrity; a saved context fits the reported maximum, and a
    // context of a session is a TPMS_CONTEXT of the same layout.
    let primary = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_storage()));
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[primary.handle], &[]));
    let object_context = params(&saved, 0).len() as u32;
    let session = start_session(&mut t, se::HMAC, handle::RH_NULL, b"", &[1; 16]);
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[session.handle], &[]));
    let session_context = params(&saved, 0).len() as u32;
    assert!(object_context <= property(&mut t, pt::MAX_OBJECT_CONTEXT).unwrap());
    assert!(session_context <= property(&mut t, pt::MAX_SESSION_CONTEXT).unwrap());
    // The context counter is not bounded by a gap (CONTEXT_GAP_MAX is 0, a recorded deviation):
    // many saves of one object all succeed.
    for _ in 0..40 {
        ok(&run(
            &mut t,
            plain(cc::CONTEXT_SAVE, &[primary.handle], &[]),
        ));
    }
    // No command audit: the audit counter stays 0 after commands.
    assert_eq!(property(&mut t, pt::AUDIT_COUNTER_0), Some(0));
    assert_eq!(property(&mut t, pt::AUDIT_COUNTER_1), Some(0));
}

#[test]
fn clock_update_property_present() {
    let mut t = started();
    let from = properties_from(&mut t, pt::CLOCK_UPDATE, 1);
    assert_eq!(from, vec![(pt::CLOCK_UPDATE, 0)]);
    // Present before and after a ClockSet, with the value the snapshot-based Clock implies (no
    // NV update interval of its own).
    ok(&pw_cmd(
        &mut t,
        cc::CLOCK_SET,
        &[OWNER],
        b"",
        &1_000u64.to_be_bytes(),
    ));
    assert_eq!(property(&mut t, pt::CLOCK_UPDATE), Some(0));
}

// ------------------------------------------------------------------------------- snapshot

#[test]
fn snapshot_orderly_and_pin_round_trip_and_v8_read() {
    // No new snapshot version: ORDERLY and the PIN types are bits of the existing attributes,
    // and the NV value of an ORDERLY counter is the data of the index.
    assert_eq!(crate::SNAPSHOT_VERSION, 8);
    let mut t = started();
    ok(&define(&mut t, OWNER, NV0 + 1, OWNER_RW | NT_COUNTER, 8));
    for _ in 0..3 {
        increment(&mut t, NV0 + 1);
    }
    shutdown(&mut t, 0);
    let v8 = t.persistent_snapshot();
    assert_eq!(u16::from_be_bytes([v8[8], v8[9]]), 8);
    // A v8 snapshot of before this slice reads: a plain counter, none ORDERLY, none PIN.
    let mut old = Tpm::restore(&v8, &[9; 48]).unwrap();
    startup(&mut old, 0);
    assert_eq!(nv_attributes(&mut old, NV0 + 1) & attr::ORDERLY, 0);
    assert_eq!(counter_at(&mut old, NV0 + 1), 3);
    assert_eq!(property(&mut old, pt::NV_COUNTERS), Some(1));
    // The same TPM now with an ORDERLY counter and both PIN types round-trips byte for byte.
    ok(&define(&mut old, OWNER, NV0 + 2, ORDERLY_COUNTER, 8));
    increment(&mut old, NV0 + 2);
    increment(&mut old, NV0 + 2);
    define_pin(&mut old, NV0 + 3, NT_PIN_FAIL, 4);
    define_pin(&mut old, NV0 + 4, NT_PIN_PASS, 6);
    err(
        &pin_read(&mut old, NV0 + 3, b"bad"),
        Rc::BAD_AUTH.in_session(1),
    );
    shutdown(&mut old, 0);
    let snapshot = old.persistent_snapshot();
    assert_eq!(u16::from_be_bytes([snapshot[8], snapshot[9]]), 8);
    let mut r = Tpm::restore(&snapshot, &[9; 48]).unwrap();
    assert_eq!(r.persistent_snapshot(), snapshot, "byte for byte");
    startup(&mut r, 0);
    assert_eq!(counter_at(&mut r, NV0 + 2), counter_at(&mut old, NV0 + 2));
    assert_ne!(nv_attributes(&mut r, NV0 + 2) & attr::ORDERLY, 0);
    assert_eq!(pin_state(&mut r, NV0 + 3), (1, 4));
    assert_eq!(pin_state(&mut r, NV0 + 4), (0, 6));
    assert_eq!(nv_attributes(&mut r, NV0 + 3) & attr::NT_MASK, NT_PIN_FAIL);
    assert_eq!(nv_attributes(&mut r, NV0 + 4) & attr::NT_MASK, NT_PIN_PASS);
}
