//! S2-32 (x87 remainder, part 1): FSIN, FCOS, FSINCOS, FPTAN, F2XM1, FYL2X, FYL2XP1, FPATAN, FSCALE and
//! FXTRACT (bare `D9 F0..FF`) in the bounded local x87 profile.
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 2A/2B, the
//! instruction page of every mnemonic above ("Operation", "FPU Flags Affected"); Vol. 1 section 8.3.10
//! (transcendental instructions, the argument range |x| < 2^63 of the trigonometric forms) and section
//! 8.5.1 (the condition codes C0-C3). The physical oracle is a Core i5-3210M (Ivy Bridge, Ubuntu x86-64,
//! user space): `data/x87_oracle.c` runs each case between FNINIT/FLDENV and FNSTENV/FSTP, and
//! `data/x87_oracle_i5_3210m.txt` holds its 1,800 answers, one line per case:
//! `op fcw sw0 x y ST0 ST1 fsw ftw` (binary80 in memory order; the operand list comes from
//! `data/x87_oracle_gen.py`).
//!
//! What the oracle shows and the model reproduces (the modelled helpers are in `od_isa::x87_transcend`):
//! * the precision control is ignored, the rounding control is honoured (all twelve masked control words);
//! * TOP, C0, C2 (C2 cleared by the reduction forms, set for |x| >= 2^63 without any other change, kept by
//!   the others) and C3 are reproduced on every row, and so are the tag word and PE;
//! * results are the correctly rounded values (about 2^-250 relative error before rounding); the
//!   microcode is not correctly rounded, so a fraction of the rows differs by one unit in the last place
//!   (and C1, the round-up indicator, follows the microcode's internal approximation on a few rows).
//!   The pinned counters below are this test's measurement of that divergence, not a requirement of
//!   the architecture.

use od_isa::execute::{StepOutcome, step};
use od_isa::long_decode::{DecodeError, Operation};
use od_isa::state::{X87State, X87Tag};
use od_isa::{AccessContext, AccessKind, GuestMemory, LongModeState, MemoryError};
use std::collections::BTreeMap;

const ORACLE: &str = include_str!("data/x87_oracle_i5_3210m.txt");
const RIP: u64 = 0x1000;

struct Code(Vec<u8>);

impl GuestMemory for Code {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(RIP)
            .and_then(|offset| self.0.get(offset as usize).copied())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }
    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        panic!("register-only instruction read {address:#x}")
    }
    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        panic!("register-only instruction wrote {address:#x}")
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn hex(text: &str) -> [u8; 10] {
    let mut out = [0u8; 10];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap();
    }
    out
}

fn is_zero(raw: [u8; 10]) -> bool {
    raw[..8] == [0; 8] && raw[8] == 0 && raw[9] & 0x7f == 0
}

fn opcode_byte(op: &str) -> u8 {
    match op {
        "f2xm1" => 0xf0,
        "fyl2x" => 0xf1,
        "fptan" => 0xf2,
        "fpatan" => 0xf3,
        "fxtract" => 0xf4,
        "fyl2xp1" => 0xf9,
        "fsincos" => 0xfb,
        "fscale" => 0xfd,
        "fsin" => 0xfe,
        "fcos" => 0xff,
        _ => panic!("{op}"),
    }
}

fn binary(op: &str) -> bool {
    matches!(op, "fyl2x" | "fyl2xp1" | "fpatan" | "fscale")
}

/// The state the oracle harness reaches after FNINIT, FLDENV(fcw, sw0, all empty) and the FLDT pushes.
fn loaded_state(op: &str, fcw: u16, sw0: u16, x: [u8; 10], y: [u8; 10]) -> LongModeState {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = fcw;
    x87.tag_word = 0xffff;
    let mut top = ((sw0 >> 11) & 7) as u8;
    let pushes: &[[u8; 10]] = if binary(op) { &[y, x] } else { &[x] };
    for raw in pushes {
        top = top.wrapping_sub(1) & 7;
        x87.registers[usize::from(top)] = *raw;
        x87.set_tag(
            top,
            if is_zero(*raw) {
                X87Tag::Zero
            } else {
                X87Tag::Valid
            },
        );
    }
    // FLD clears C1; the exception flags of sw0 are all clear.
    x87.status_word = (sw0 & !0x3800 & !0x0200) | (u16::from(top) << 11);
    LongModeState {
        rip: RIP,
        x87: Some(x87),
        ..LongModeState::default()
    }
}

fn execute(state: &mut LongModeState, op: &str) -> Result<StepOutcome, MemoryError> {
    let mut memory = Code(vec![0xd9, opcode_byte(op)]);
    step(state, &mut memory, AccessContext::default())
}

fn mantissa(raw: [u8; 10]) -> u64 {
    u64::from_le_bytes(raw[..8].try_into().unwrap())
}

fn sign_exponent(raw: [u8; 10]) -> u16 {
    u16::from_le_bytes([raw[8], raw[9]])
}

/// Signed distance in units in the last place of the smaller binade; None when further than one binade.
fn ulp_distance(model: [u8; 10], hardware: [u8; 10]) -> Option<i64> {
    if model == hardware {
        return Some(0);
    }
    let (a, b) = (sign_exponent(model), sign_exponent(hardware));
    if a == b {
        return Some(mantissa(model).wrapping_sub(mantissa(hardware)) as i64);
    }
    if (a ^ b) & 0x8000 != 0 || (i32::from(a & 0x7fff) - i32::from(b & 0x7fff)).abs() != 1 {
        return None;
    }
    // Adjacent binades: 2^63 (smaller exponent's all-ones neighbour) is one ulp of the lower binade.
    let (low, high, model_high) = if a & 0x7fff > b & 0x7fff {
        (hardware, model, true)
    } else {
        (model, hardware, false)
    };
    let lower_gap = u64::MAX - mantissa(low); // distance of the lower value to 2^64-1
    let upper_gap = (mantissa(high) - (1 << 63)) * 2; // in half-ulps of the lower binade, rounded up below
    let distance = i64::try_from(lower_gap + 1 + upper_gap / 2).ok()?;
    Some(if model_high { distance } else { -distance })
}

#[derive(Default, Debug)]
struct Counters {
    rows: u32,
    out_of_range: u32,
    refused: u32,
    exact: u32,
    one_ulp: u32,
    c1_differs: u32,
}

#[test]
fn oracle_rows_match_the_interpreter() {
    let mut counters: BTreeMap<String, Counters> = BTreeMap::new();
    for line in ORACLE.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let op = f[0];
        let fcw = u16::from_str_radix(f[1], 16).unwrap();
        let sw0 = u16::from_str_radix(f[2], 16).unwrap();
        let (x, y) = (hex(f[3]), hex(f[4]));
        let hw_st0 = hex(f[5]);
        let hw_st1 = f[6];
        let hw_sw = u16::from_str_radix(f[7], 16).unwrap();
        let hw_tw = u16::from_str_radix(f[8], 16).unwrap();

        let mut state = loaded_state(op, fcw, sw0, x, y);
        let state_before = state.clone();
        let before = state.x87.clone().unwrap();
        let outcome = execute(&mut state, op).unwrap();
        let entry = counters.entry(op.to_string()).or_default();
        entry.rows += 1;
        let context = line.to_string();
        if !matches!(outcome, StepOutcome::Advanced) {
            // Only FYL2XP1 operands outside -(1 - sqrt(2)/2) < x < sqrt(2) - 1 (undefined in the SDM) are
            // refused here; the refusal changes nothing.
            assert_eq!(op, "fyl2xp1", "{context}");
            assert_eq!(
                outcome,
                StepOutcome::Unsupported { opcode: 0xd9 },
                "{context}"
            );
            assert_eq!(state, state_before, "{context}");
            entry.refused += 1;
            continue;
        }
        let after = state.x87.as_ref().unwrap();
        assert_eq!(state.rip, RIP + 2, "{context}");
        assert_eq!(after.instruction_pointer, RIP, "{context}");
        // Condition codes and TOP.
        let c_mask = 0x3800 | 0x4500 | 0x0400;
        assert_eq!(
            after.status_word & (c_mask & !0x0200),
            hw_sw & (c_mask & !0x0200),
            "{context}"
        );
        assert_eq!(after.status_word & 0x20, hw_sw & 0x20, "PE: {context}");
        assert_eq!(after.status_word & 0x80df & !0x20, 0, "{context}");
        // C1 (the round-up indicator) follows the microcode's internal approximation.
        if after.status_word & 0x0200 != hw_sw & 0x0200 {
            entry.c1_differs += 1;
        }
        let top = after.top();
        let st0 = after.registers[usize::from(top)];
        if hw_sw & 0x0400 != 0 && matches!(op, "fsin" | "fcos" | "fsincos" | "fptan") {
            // |x| >= 2^63: operand unchanged, no push.
            entry.out_of_range += 1;
            assert_eq!(st0, x, "{context}");
            assert_eq!(after.registers, before.registers, "{context}");
            assert_eq!(after.tag_word, before.tag_word, "{context}");
            assert_eq!(st0, hw_st0, "{context}");
            continue;
        }
        assert_eq!(after.tag_word, hw_tw, "tags: {context}");
        let mut compared = vec![(st0, hw_st0)];
        if hw_st1 != "-" {
            compared.push((after.registers[usize::from((top + 1) & 7)], hex(hw_st1)));
        }
        let mut worst = 0;
        for (index, (model, hardware)) in compared.into_iter().enumerate() {
            let distance = ulp_distance(model, hardware).unwrap_or_else(|| {
                panic!("result {index} far from the oracle: {context} model={model:02x?}")
            });
            worst = worst.max(distance.abs());
        }
        match worst {
            0 => entry.exact += 1,
            1 => entry.one_ulp += 1,
            _ => panic!("more than one ulp: {context}"),
        }
        // Registers that the instruction does not own are untouched.
        for index in 0..8usize {
            let owned = (0..2).any(|k| usize::from((top + k) & 7) == index)
                || index == usize::from(before.top());
            if !owned {
                assert_eq!(after.registers[index], before.registers[index], "{context}");
            }
        }
    }
    // rows, |x| >= 2^63 rows, refused rows, exact, one ulp off, C1 differs.
    let expected: [(&str, [u32; 6]); 10] = [
        ("f2xm1", [146, 0, 0, 138, 8, 3]),
        ("fcos", [260, 24, 0, 235, 1, 1]),
        ("fpatan", [184, 0, 0, 178, 6, 1]),
        ("fptan", [260, 24, 0, 231, 5, 8]),
        ("fscale", [86, 0, 0, 86, 0, 0]),
        ("fsin", [260, 24, 0, 234, 2, 0]),
        ("fsincos", [260, 24, 0, 232, 4, 0]),
        ("fxtract", [60, 0, 0, 60, 0, 0]),
        ("fyl2x", [152, 0, 0, 145, 7, 5]),
        ("fyl2xp1", [132, 0, 2, 115, 15, 4]),
    ];
    let measured: Vec<(&str, [u32; 6])> = counters
        .iter()
        .map(|(op, c)| {
            (
                op.as_str(),
                [
                    c.rows,
                    c.out_of_range,
                    c.refused,
                    c.exact,
                    c.one_ulp,
                    c.c1_differs,
                ],
            )
        })
        .collect();
    assert_eq!(measured, expected);
}

// ---- literal behaviour ----

fn b80(neg: bool, exponent: i32, mantissa: u64) -> [u8; 10] {
    let mut raw = [0u8; 10];
    raw[..8].copy_from_slice(&mantissa.to_le_bytes());
    let se = (u16::from(neg) << 15) | (exponent + 16383) as u16;
    raw[8..].copy_from_slice(&se.to_le_bytes());
    raw
}

const ONE: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
const ZERO: [u8; 10] = [0; 10];
const NEG_ZERO: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80];

/// FCW 037F, TOP = 3 before the operands are pushed (so ST(0) = x sits in physical register 2, or in 1 with
/// ST(1) = y in 2 for the binary forms); all other registers Empty.
fn fresh(op: &str, x: [u8; 10], y: [u8; 10]) -> LongModeState {
    loaded_state(op, 0x037f, 0x1800, x, y)
}

fn run(op: &str, x: [u8; 10], y: [u8; 10]) -> LongModeState {
    let mut state = fresh(op, x, y);
    assert_eq!(execute(&mut state, op), Ok(StepOutcome::Advanced));
    state
}

fn assert_refused(op: &str, state: LongModeState, why: &str) {
    let before = state.clone();
    let mut state = state;
    assert_eq!(
        execute(&mut state, op),
        Ok(StepOutcome::Unsupported { opcode: 0xd9 }),
        "{op}: {why}"
    );
    assert_eq!(state, before, "{op}: {why}");
}

const ALL: [&str; 10] = [
    "f2xm1", "fyl2x", "fptan", "fpatan", "fxtract", "fyl2xp1", "fsincos", "fscale", "fsin", "fcos",
];

#[test]
fn zero_and_exact_literals() {
    // FSIN(+-0) = +-0 (Zero tag), FCOS(+-0) = 1, no exception and C1 clear.
    for (x, sin) in [(ZERO, ZERO), (NEG_ZERO, NEG_ZERO)] {
        let state = run("fsin", x, ZERO);
        let x87 = state.x87.as_ref().unwrap();
        assert_eq!((x87.registers[2], x87.tag(2)), (sin, Some(X87Tag::Zero)));
        assert_eq!(x87.status_word, 0x1000);
        let state = run("fcos", x, ZERO);
        let x87 = state.x87.as_ref().unwrap();
        assert_eq!((x87.registers[2], x87.tag(2)), (ONE, Some(X87Tag::Valid)));
        assert_eq!(x87.status_word, 0x1000);
        // FSINCOS and FPTAN push: ST(1) = sin / tan = +-0, ST(0) = cos / 1.0.
        for op in ["fsincos", "fptan"] {
            let state = run(op, x, ZERO);
            let x87 = state.x87.as_ref().unwrap();
            assert_eq!(x87.top(), 1, "{op}");
            assert_eq!(
                (x87.registers[1], x87.tag(1)),
                (ONE, Some(X87Tag::Valid)),
                "{op}"
            );
            assert_eq!(
                (x87.registers[2], x87.tag(2)),
                (sin, Some(X87Tag::Zero)),
                "{op}"
            );
            assert_eq!(x87.status_word, 0x0800, "{op}");
        }
    }
    // FXTRACT(12.0): ST(1) = 3.0 (exponent), ST(0) = 1.5 (significand), one push.
    let state = run("fxtract", b80(false, 3, 0xc000_0000_0000_0000), ZERO);
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(x87.top(), 1);
    assert_eq!(x87.registers[1], b80(false, 0, 0xc000_0000_0000_0000));
    assert_eq!(x87.registers[2], b80(false, 1, 0xc000_0000_0000_0000));
    assert_eq!(
        (x87.tag(1), x87.tag(2)),
        (Some(X87Tag::Valid), Some(X87Tag::Valid))
    );
    // FXTRACT of a value with exponent 0 gives ST(1) = +0 tagged Zero; a negative exponent a negative ST(1).
    let state = run("fxtract", b80(true, 0, 0xa000_0000_0000_0000), ZERO);
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!((x87.registers[2], x87.tag(2)), (ZERO, Some(X87Tag::Zero)));
    assert_eq!(x87.registers[1], b80(true, 0, 0xa000_0000_0000_0000));
    let state = run("fxtract", b80(false, -5, 0x8000_0000_0000_0000), ZERO);
    assert_eq!(
        state.x87.as_ref().unwrap().registers[2],
        b80(true, 2, 0xa000_0000_0000_0000)
    );
    // FSCALE: 1.5 * 2^trunc(3.9) = 12, 1.5 * 2^trunc(-3.9) = 0.1875; ST(1) is untouched, no pop.
    let three_nine = b80(false, 1, 0xf333_3333_3333_3333);
    let state = run("fscale", b80(false, 0, 0xc000_0000_0000_0000), three_nine);
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(x87.registers[1], b80(false, 3, 0xc000_0000_0000_0000));
    assert_eq!(x87.registers[2], three_nine);
    assert_eq!((x87.top(), x87.status_word & 0xc7ff), (1, 0));
    let minus = b80(true, 1, 0xf333_3333_3333_3333);
    let state = run("fscale", b80(false, 0, 0xc000_0000_0000_0000), minus);
    assert_eq!(
        state.x87.as_ref().unwrap().registers[1],
        b80(false, -3, 0xc000_0000_0000_0000)
    );
    // FYL2X(1, y) = +-0 with the sign of y and no exception; FYL2XP1(+-0, y) = +-0.
    let state = run("fyl2x", ONE, b80(true, 1, 0xa000_0000_0000_0000));
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(
        (x87.registers[2], x87.tag(2), x87.tag(1)),
        (NEG_ZERO, Some(X87Tag::Zero), Some(X87Tag::Empty))
    );
    assert_eq!(x87.status_word & 0xc7ff, 0x0000);
    assert_eq!(x87.top(), 2);
    // F2XM1(+-1) = 1 and -1/2, with PE (the microcode reports it) and C1 clear.
    let state = run("f2xm1", b80(true, 0, 0x8000_0000_0000_0000), ZERO);
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(x87.registers[2], b80(true, -1, 0x8000_0000_0000_0000));
    assert_eq!(x87.status_word & 0x0fff, 0x0020);
}

#[test]
fn trigonometric_range_edge_and_reduction() {
    // |x| = 2^63 is outside the reduction range: C2 set, C1 clear, operand unchanged, no push.
    let big = b80(false, 63, 0x8000_0000_0000_0000);
    for op in ["fsin", "fcos", "fsincos", "fptan"] {
        let before = fresh(op, big, ZERO);
        let state = run(op, big, ZERO);
        let (after, was) = (state.x87.as_ref().unwrap(), before.x87.as_ref().unwrap());
        assert_eq!(after.registers, was.registers, "{op}");
        assert_eq!(after.tag_word, was.tag_word, "{op}");
        assert_eq!(after.status_word, was.status_word | 0x0400, "{op}");
    }
    // 2^62 - the largest exponent that is reduced - completes with C2 clear.
    let state = run("fsin", b80(false, 62, 0x8000_0000_0000_0000), ZERO);
    assert_eq!(state.x87.as_ref().unwrap().status_word & 0x0400, 0);
    // The hardware reduces by the 66-bit pi/2, so sin(pi rounded to 64 bits) = -(pi_64 - pi_66), about
    // -2^-64 (the true value, -(pi_64 - pi), is about -2^-62.8); the golden oracle rows pin the exact bytes.
    let pi64 = b80(false, 1, 0xc90f_daa2_2168_c235);
    let state = run("fsin", pi64, ZERO);
    let result = state.x87.as_ref().unwrap().registers[2];
    assert_eq!(result[9] & 0x80, 0x80, "negative: {result:02x?}");
    let exponent = i32::from(sign_exponent(result) & 0x7fff) - 16383;
    assert!((-66..=-62).contains(&exponent), "{exponent}");
}

#[test]
fn bounded_profile_refuses_everything_else_without_a_trace() {
    let normal = b80(false, 0, 0xc000_0000_0000_0000);
    let half = b80(false, -1, 0xc000_0000_0000_0000);
    for op in ALL {
        let x = if matches!(op, "fyl2x") { normal } else { half };
        let y = normal;
        // No x87 state at all.
        let mut state = fresh(op, x, y);
        state.x87 = None;
        assert_refused(op, state, "no x87 state");
        // A represented selector.
        for selector in 0..2 {
            let mut state = fresh(op, x, y);
            let x87 = state.x87.as_mut().unwrap();
            if selector == 0 {
                x87.instruction_selector = Some(0);
            } else {
                x87.data_selector = Some(0);
            }
            assert_refused(op, state, "selector");
        }
        // Unmasked exceptions, reserved precision control, any sticky exception except PE.
        for control in [
            0x037e, 0x037d, 0x037b, 0x0377, 0x036f, 0x035f, 0x017f, 0x0000, 0x137f,
        ] {
            let mut state = fresh(op, x, y);
            state.x87.as_mut().unwrap().control_word = control;
            assert_refused(op, state, "control word");
        }
        for sticky in [
            0x0001, 0x0002, 0x0004, 0x0008, 0x0010, 0x0040, 0x0080, 0x8000,
        ] {
            let mut state = fresh(op, x, y);
            state.x87.as_mut().unwrap().status_word |= sticky;
            assert_refused(op, state, "sticky exception");
        }
        // Operand classes: Empty, denormal, pseudo-denormal, unnormal, infinity, NaN.
        let bad: [[u8; 10]; 6] = [
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x40, 0xff, 0x3f],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
        ];
        for slot in 0..(if binary(op) { 2 } else { 1 }) {
            for raw in bad {
                let mut state = fresh(op, x, y);
                let x87 = state.x87.as_mut().unwrap();
                let physical = (x87.top() + slot) & 7;
                x87.registers[usize::from(physical)] = raw;
                assert_refused(op, state, "special operand");
            }
            let mut state = fresh(op, x, y);
            let x87 = state.x87.as_mut().unwrap();
            let physical = (x87.top() + slot) & 7;
            x87.set_tag(physical, X87Tag::Empty);
            assert_refused(op, state, "empty operand");
            let mut state = fresh(op, x, y);
            let x87 = state.x87.as_mut().unwrap();
            let physical = (x87.top() + slot) & 7;
            x87.set_tag(physical, X87Tag::Special);
            assert_refused(op, state, "special tag");
        }
        // The pushing forms need an Empty slot below TOP.
        if matches!(op, "fsincos" | "fptan" | "fxtract") {
            let mut state = fresh(op, x, y);
            let x87 = state.x87.as_mut().unwrap();
            let below = (x87.top() + 7) & 7;
            x87.set_tag(below, X87Tag::Valid);
            assert_refused(op, state, "stack overflow");
        }
    }
    // Domain and range limits.
    let refused: [(&str, [u8; 10], [u8; 10], &str); 14] = [
        (
            "f2xm1",
            b80(false, 1, 0x8000_0000_0000_0000),
            ZERO,
            "|x| > 1",
        ),
        (
            "f2xm1",
            b80(true, 0, 0x8000_0000_0000_0001),
            ZERO,
            "|x| just above 1",
        ),
        (
            "fyl2x",
            b80(true, 0, 0x8000_0000_0000_0000),
            normal,
            "negative x",
        ),
        ("fyl2x", ZERO, normal, "zero x"),
        ("fyl2x", normal, ZERO, "zero y"),
        (
            "fyl2x",
            b80(false, 16000, 0x8000_0000_0000_0000),
            b80(false, 16380, 0x8000_0000_0000_0000),
            "overflow",
        ),
        (
            "fyl2xp1",
            b80(false, -1, 0xc000_0000_0000_0000),
            normal,
            "x above sqrt(2) - 1",
        ),
        (
            "fyl2xp1",
            b80(true, -1, 0xa000_0000_0000_0000),
            normal,
            "x below -(1 - sqrt(2)/2)",
        ),
        ("fpatan", ZERO, normal, "zero x"),
        ("fpatan", normal, ZERO, "zero y"),
        (
            "fscale",
            b80(false, 16383, 0x8000_0000_0000_0000),
            b80(false, 0, 0x8000_0000_0000_0000),
            "overflow",
        ),
        (
            "fscale",
            b80(false, -16382, 0x8000_0000_0000_0000),
            b80(true, 0, 0x8000_0000_0000_0000),
            "denormal result",
        ),
        (
            "fscale",
            normal,
            b80(false, 30, 0x8000_0000_0000_0000),
            "huge scale",
        ),
        ("fxtract", ZERO, ZERO, "zero (ZE and -infinity)"),
    ];
    for (op, x, y, why) in refused {
        assert_refused(op, fresh(op, x, y), why);
    }
}

#[test]
fn decoder_maps_the_ten_opcodes_and_rejects_prefixes() {
    use od_isa::long_decode;
    let decode =
        |code: &[u8]| long_decode::decode(&mut Code(code.to_vec()), RIP, AccessContext::default());
    let expected = [
        (0xf0u8, Operation::F2xm1),
        (0xf1, Operation::Fyl2x),
        (0xf2, Operation::Fptan),
        (0xf3, Operation::Fpatan),
        (0xf4, Operation::Fxtract),
        (0xf9, Operation::Fyl2xp1),
        (0xfb, Operation::Fsincos),
        (0xfd, Operation::Fscale),
        (0xfe, Operation::Fsin),
        (0xff, Operation::Fcos),
    ];
    for (second, operation) in expected {
        let instruction = decode(&[0xd9, second]).unwrap();
        assert_eq!((instruction.operation, instruction.length), (operation, 2));
        for prefix in [0x66u8, 0xf2, 0xf3, 0x2e, 0x67, 0x48, 0x64, 0xf0] {
            assert!(
                matches!(
                    decode(&[prefix, 0xd9, second]),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{prefix:02x} D9 {second:02x}"
            );
        }
        assert!(decode(&[0xd9]).is_err());
    }
    // The neighbours of the group keep their decoding.
    assert_eq!(decode(&[0xd9, 0xf8]).unwrap().operation, Operation::Fprem);
    assert_eq!(decode(&[0xd9, 0xf5]).unwrap().operation, Operation::Fprem1);
    assert_eq!(decode(&[0xd9, 0xfa]).unwrap().operation, Operation::Fsqrt);
    assert_eq!(decode(&[0xd9, 0xfc]).unwrap().operation, Operation::Frndint);
    assert!(matches!(
        decode(&[0xd9, 0xf6]).unwrap().operation,
        Operation::Fdecstp
    ));
    assert!(matches!(
        decode(&[0xd9, 0xf7]).unwrap().operation,
        Operation::Fincstp
    ));
}
