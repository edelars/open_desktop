//! PERF-33 (E4), group G6: the two-operand IMUL r, r/m (`0F AF`, register and memory source),
//! NOP (`90` with its accepted prefixes) and the ModRM NOP (`0F 1F /0` in every operand form,
//! `0F 18 /0..7` with a memory operand) in `execute::try_fast_step`. As for G5
//! (fast_step_g5_tests.rs), each case runs the general path (one instruction through `run` of the
//! CPL0 profile set) and the fast path on copies of the same state and memory and compares the CPU
//! state, `rflags_defined`, the outcome, the memory and the journal of the data calls. A NOP makes
//! no data call; an IMUL with a memory source makes exactly the read of the general path, and its
//! fault commits nothing. Inadmissible conditions are refused without a memory call.

use std::collections::{BTreeMap, BTreeSet};

use crate::execute::try_fast_step;
use crate::fast_step_g2_tests::{assert_refused, context};
use crate::fast_step_g4_tests::{
    CODE, End, address_forms, encode, register_sets, start_state, try_decode_at,
};
use crate::fast_step_g5_tests::{compare, end_name};
use crate::long_decode::{Operand, Operation};
use crate::{INTEGER_FLAGS, PrivilegeLevel, StiShadow};

const CF: u64 = 1;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const TF: u64 = 1 << 8;
const OF: u64 = 1 << 11;
const RF: u64 = 1 << 16;

const REGS: [u8; 6] = [0, 3, 4, 7, 9, 15];
/// (label, legacy prefixes, REX.W) of the 16-, 32- and 64-bit forms.
const WIDTHS: [(&str, &[u8], bool); 3] = [
    ("16", &[0x66], false),
    ("32", &[], false),
    ("64", &[], true),
];
/// Two flag states with two definedness masks: the result must not depend on the old flags, and
/// the definedness must follow `flag_effects` from either mask.
const FLAG_STATES: [(u64, u64); 2] = [
    (2 | CF | ZF, INTEGER_FLAGS),
    (2 | SF | OF | AF, INTEGER_FLAGS & !(AF | OF | SF)),
];

/// `IMUL r, r` (`0F AF /r`, mod 11) with the legacy prefixes and REX.W of a width.
fn imul_register(legacy: &[u8], wide: bool, reg: u8, rm: u8) -> Vec<u8> {
    let mut bytes = legacy.to_vec();
    let rex = (u8::from(wide) << 3) | (u8::from(reg >= 8) << 2) | u8::from(rm >= 8);
    if rex != 0 {
        bytes.push(0x40 | rex);
    }
    bytes.extend([0x0f, 0xaf, 0xc0 | ((reg & 7) << 3) | (rm & 7)]);
    bytes
}

#[test]
fn every_g6_imul_form_equals_the_general_path_with_the_same_memory_calls() {
    let sets = register_sets();
    let mut sizes = BTreeSet::new();
    let mut ends = BTreeMap::<String, u64>::new();
    let (mut overflow, mut no_overflow, mut memory_cases, mut cases) = (0u64, 0u64, 0u64, 0u64);
    // Register sources: every width, a spread of destination and source registers (the same
    // register too), every register set, both flag states.
    for (width, legacy, wide) in WIDTHS {
        for reg in REGS {
            for rm in REGS {
                let bytes = imul_register(legacy, wide, reg, rm);
                let instruction = try_decode_at(CODE, &bytes).expect("IMUL r, r decodes");
                assert!(
                    matches!(
                        instruction.operation,
                        Operation::Imul {
                            src: Operand::Register(_),
                            ..
                        }
                    ),
                    "{bytes:02x?}: {instruction:?}"
                );
                sizes.insert(instruction.operand_size.bytes());
                for (set, gpr) in &sets {
                    for (rflags, defined) in FLAG_STATES {
                        let start = start_state(*gpr, CODE, rflags);
                        let label = format!("imul r{width} {reg}, {rm} regs {set}");
                        let run = compare(&label, &bytes, &start, defined, false);
                        assert_eq!(run.end, End::Retired, "{label}");
                        assert!(run.log.calls().is_empty(), "{label}");
                        if run.state.rflags & (CF | OF) == CF | OF {
                            overflow += 1;
                        } else {
                            assert_eq!(run.state.rflags & (CF | OF), 0, "{label}");
                            no_overflow += 1;
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    // Memory sources: every address form, a rotating register and width, every register set (so
    // canonical, non-canonical, page-fault and unmapped addresses all occur).
    for (index, form) in address_forms().iter().enumerate() {
        for (number, (width, legacy, wide)) in WIDTHS.into_iter().enumerate() {
            if (index + number) % 2 != 0 {
                continue;
            }
            let reg = REGS[(index + number) % REGS.len()];
            let bytes = encode(legacy, wide, false, &[0x0f, 0xaf], reg, form, &[]);
            let label = format!("imul r{width} {reg}, {}", form.label);
            let Some(instruction) = try_decode_at(CODE, &bytes) else {
                panic!("{label}: {bytes:02x?} does not decode");
            };
            assert!(
                matches!(
                    instruction.operation,
                    Operation::Imul {
                        src: Operand::Memory(_),
                        ..
                    }
                ),
                "{label}: {instruction:?}"
            );
            for (set, gpr) in &sets {
                let (rflags, defined) = FLAG_STATES[index % 2];
                let start = start_state(*gpr, CODE, rflags);
                let run = compare(
                    &format!("{label} regs {set}"),
                    &bytes,
                    &start,
                    defined,
                    false,
                );
                assert!(run.log.calls().len() <= 1, "{label}");
                if run.end == End::Retired {
                    assert_eq!(run.log.calls().len(), 1, "{label}");
                }
                *ends.entry(end_name(&run)).or_default() += 1;
                memory_cases += 1;
                cases += 1;
            }
        }
    }
    assert_eq!(sizes, BTreeSet::from([2, 4, 8]));
    assert!(
        overflow > 100 && no_overflow > 100,
        "{overflow} {no_overflow}"
    );
    assert!(ends["retired"] > 200, "{ends:?}");
    assert!(ends.get("fault 13").copied().unwrap_or(0) > 20, "{ends:?}");
    assert!(memory_cases > 500, "{memory_cases}");
    assert!(cases > 1_500, "{cases}");
}

/// (label, bytes) of the NOP forms: `90` bare and with the prefixes the decoder keeps as NOP, and
/// the ModRM NOP over every address form (with the 66 and CS prefixes on some), its register form,
/// and the `0F 18` hints with a memory operand.
fn nop_encodings() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = [
        ("nop", &[0x90][..]),
        ("o16 nop", &[0x66, 0x90]),
        ("rex nop", &[0x40, 0x90]),
        ("rex.w nop", &[0x48, 0x90]),
        ("cs nop", &[0x2e, 0x90]),
        ("nop eax", &[0x0f, 0x1f, 0xc0]),
        ("nop r9", &[0x41, 0x0f, 0x1f, 0xc1]),
        (
            "o16 o16 nop",
            &[0x66, 0x66, 0x0f, 0x1f, 0x84, 0x00, 0, 0, 0, 0],
        ),
    ]
    .into_iter()
    .map(|(label, bytes)| (label.to_owned(), bytes.to_vec()))
    .collect();
    for (index, form) in address_forms().iter().enumerate() {
        let legacy: &[u8] = match index % 3 {
            0 => &[],
            1 => &[0x66],
            _ => &[0x2e],
        };
        out.push((
            format!("nop {legacy:02x?} {}", form.label),
            encode(legacy, index % 4 == 0, false, &[0x0f, 0x1f], 0, form, &[]),
        ));
        if index % 5 == 0 {
            let hint = (index / 5) as u8 % 8;
            out.push((
                format!("0f 18 /{hint} {}", form.label),
                encode(&[], false, false, &[0x0f, 0x18], hint, form, &[]),
            ));
        }
    }
    out
}

#[test]
fn every_g6_nop_form_equals_the_general_path_without_a_memory_call() {
    let sets = register_sets();
    let (mut nop, mut nop_modrm) = (0u64, 0u64);
    for (label, bytes) in nop_encodings() {
        let instruction =
            try_decode_at(CODE, &bytes).unwrap_or_else(|| panic!("{label}: {bytes:02x?}"));
        match instruction.operation {
            Operation::Nop => nop += 1,
            Operation::NopModrm => nop_modrm += 1,
            other => panic!("{label}: {bytes:02x?}: {other:?}"),
        }
        for (set, gpr) in &sets {
            for (rflags, defined) in FLAG_STATES {
                let start = start_state(*gpr, CODE, rflags);
                let label = format!("{label} regs {set}");
                let run = compare(&label, &bytes, &start, defined, false);
                assert_eq!(run.end, End::Retired, "{label}");
                assert!(run.log.calls().is_empty(), "{label}");
                assert!(run.ram.written.is_empty(), "{label}");
                let mut expected = start.clone();
                expected.rip = CODE + bytes.len() as u64;
                assert_eq!(run.state, expected, "{label}");
                assert_eq!(run.defined, defined, "{label}");
            }
        }
    }
    assert_eq!(nop, 5);
    assert!(nop_modrm > 100, "{nop_modrm}");
}

/// A sample of every G6 form.
const REFUSAL_SAMPLE: &[&[u8]] = &[
    &[0x48, 0x0f, 0xaf, 0xc1],       // imul rax, rcx
    &[0x0f, 0xaf, 0x06],             // imul eax, [rsi]
    &[0x90],                         // nop
    &[0x66, 0x90],                   // o16 nop
    &[0x0f, 0x1f, 0x44, 0x00, 0x00], // nop dword [rax+rax]
    &[0x0f, 0x18, 0x0e],             // prefetcht0 [rsi]
];

fn refusal_state() -> crate::LongModeState {
    let mut state = crate::fast_step_g2_tests::state(5, 2 | CF);
    state.gpr[6] = 0x2000;
    state
}

#[test]
fn inadmissible_g6_conditions_are_refused_without_a_memory_call() {
    let ring0 = context(PrivilegeLevel::Ring0);
    for &bytes in REFUSAL_SAMPLE {
        // Control: the plain case is taken (`compare` panics on "not handled").
        compare("control", bytes, &refusal_state(), INTEGER_FLAGS, false);
        for privilege in [
            PrivilegeLevel::Ring1,
            PrivilegeLevel::Ring2,
            PrivilegeLevel::Ring3,
        ] {
            assert_refused("cpl", bytes, refusal_state(), context(privilege), |_, _| {});
        }
        assert_refused("rf", bytes, refusal_state(), ring0, |s, _| s.rflags |= RF);
        assert_refused("tf", bytes, refusal_state(), ring0, |s, _| s.rflags |= TF);
        assert_refused("shadow", bytes, refusal_state(), ring0, |s, _| {
            s.sti_shadow = Some(StiShadow::Blocking);
        });
        assert_refused("no shadow model", bytes, refusal_state(), ring0, |s, _| {
            s.sti_shadow = None;
        });
        for group in [0xf0u8, 0xf2, 0xf3] {
            assert_refused("group1", bytes, refusal_state(), ring0, |_, i| {
                i.prefixes.group1 = Some(group);
            });
        }
        for segment in [0x64u8, 0x65] {
            assert_refused("fs/gs", bytes, refusal_state(), ring0, |_, i| {
                i.prefixes.segment = Some(segment);
            });
        }
    }
    // Outside G6: PAUSE (`F3 90`, a REP-prefixed encoding), the decoded FS/GS ModRM NOP, the
    // three-operand IMUL.
    for (label, bytes) in [
        ("pause", &[0xf3, 0x90][..]),
        ("fs nop", &[0x64, 0x0f, 0x1f, 0x00]),
        ("gs nop", &[0x65, 0x0f, 0x1f, 0x00]),
        ("imul imm8", &[0x6b, 0xc1, 0x05]),
        ("imul imm32", &[0x69, 0xc1, 1, 2, 3, 4]),
    ] {
        assert_refused(label, bytes, refusal_state(), ring0, |_, _| {});
    }
    // `executable` admits the ModRM NOP only under a control-register policy: without one the
    // fast path leaves it to the general path, unchanged.
    let bytes = [0x0f, 0x1f, 0x44, 0x00, 0x00];
    let instruction = try_decode_at(CODE, &bytes).expect("nop dword [rax+rax]");
    let mut state = refusal_state();
    let before = state.clone();
    let mut defined = INTEGER_FLAGS;
    let mut ram = crate::fast_step_g4_tests::Ram::new(CODE, &bytes);
    let outcome = try_fast_step(
        &mut state,
        &mut ram,
        ring0,
        &mut defined,
        &instruction,
        false,
    );
    assert!(outcome.is_none(), "{outcome:?}");
    assert_eq!(state, before);
    assert_eq!(defined, INTEGER_FLAGS);
}
