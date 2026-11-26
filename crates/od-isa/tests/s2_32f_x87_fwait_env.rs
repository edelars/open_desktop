//! S2-32f: the FWAIT spellings of FSTENV and FSAVE (`9B D9 30`, `9B DD 30`, image at [RAX], no prefix) in the bounded
//! local x87 profile and at CPL0 of the set with the FP member.
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200): Vol. 2A "FSTENV/FNSTENV" (p. 3-390) and
//! "FSAVE/FNSAVE" (p. 3-375): the FSTENV and FSAVE mnemonics are the non-waiting form preceded by the WAIT/FWAIT byte `9B`;
//! Vol. 2D "WAIT/FWAIT" (#NM only for CR0.MP and CR0.TS; checks for a pending unmasked x87 exception, delivered as #MF with
//! CR0.NE = 1); Vol. 1 section 8.7 (x87 exception synchronization). The decoder consumes exactly `9B D9 30` and `9B DD 30` as one
//! instruction of three bytes whose operation is the one of the non-waiting form; `9B` before any other byte is the bare FWAIT of
//! S4-38L9d (owned by the CPL0 set, refused in the bounded profile), so `9B D9 20` (FWAIT, FLDENV), a prefix between FWAIT and the
//! escape byte, or a truncated fetch are not these forms. A prefix before `9B` is refused by the decoder.
//!
//! In the bounded profile the admission of the environment forms (all-masked control word, ES and B clear) leaves no pending
//! unmasked exception, so the FWAIT part is a no-op; a state that would raise #MF (незамаскированное FCW, ES, B) is refused
//! (`Unsupported`, opcode `9B`), carried over, not modeled. In the CPL0 set the FWAIT part keeps the checks of the FWAIT of
//! `step_cpl0_fp` (CR0.MP and CR0.TS #NM, pending exception #MF with CR0.NE set) before the gates of the x87 forms.
//!
//! Saved RIP of a fault (Intel SDM, FSTENV/FSAVE: the assembler issues two instructions, and "the save EIP points to the
//! instruction that caused the exception"): an error of the FWAIT part (#NM for CR0.MP with CR0.TS, #MF) leaves RIP at `9B`;
//! an error after the FWAIT part retired (the #NM of CR0.EM, or of CR0.TS with CR0.MP clear; a fault of the store) leaves
//! RIP at the FNSTENV/FNSAVE byte, `9B` + 1. A model error of the store (unmapped memory, address overflow; not an x86
//! exception) is returned as `Err` with the state untouched, RIP at `9B`, like the non-waiting forms. The decoder fetches no byte after `9B` across the page of `9B` (the three
//! bytes of the spelling lie on one 4 KiB page, else the instruction is the bare FWAIT of its own byte).
//!
//! Physical oracle: Core i5-3210M (`data/x87_fwait_oracle.c`, images of `data/x87_env_oracle_gen.py`; the assembler emits
//! `9B D9 30` for `fstenv` and `9B DD 30` for `fsave`): 324 rows, answers in `data/x87_fwait_oracle_i5_3210m.txt`; the file is
//! byte-identical to the answers of the non-waiting forms of S2-32c (`data/x87_env_oracle_i5_3210m.txt`).

mod mmx_common;

use mmx_common::*;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig,
    StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::long_decode::{self, DecodeError, Operation, Prefixes};
use od_isa::state::X87State;
use od_isa::{
    AccessContext, GuestMemory, LongModeState, MemoryError, PrivilegeLevel, Register,
    cpuid::CpuidProfile,
};
use std::cell::RefCell;

const ORACLE_FWAIT: &str = include_str!("data/x87_fwait_oracle_i5_3210m.txt");
const ORACLE_NON_WAITING: &str = include_str!("data/x87_env_oracle_i5_3210m.txt");

const FSTENV: [u8; 3] = [0x9b, 0xd9, 0x30];
const FSAVE: [u8; 3] = [0x9b, 0xdd, 0x30];
const FNSTENV: [u8; 2] = [0xd9, 0x30];
const FNSAVE: [u8; 2] = [0xdd, 0x30];
const FRSTOR: [u8; 2] = [0xdd, 0x20];
const FLDENV: [u8; 2] = [0xd9, 0x20];
const ENV: u64 = DATA;
const SAVE: u64 = DATA + 0x400;
const ENV2: u64 = DATA + 0x800;
const IN: u64 = DATA + 0x1000;
const PREFIXES: [u8; 9] = [0x66, 0xf2, 0xf3, 0x2e, 0x67, 0x48, 0x40, 0x64, 0x65];

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap())
        .collect()
}

fn decode(code: &[u8]) -> Result<long_decode::Instruction, DecodeError> {
    let mut ram = Ram::new(code);
    long_decode::decode(&mut ram, CODE, AccessContext::default())
}

/// Bare step of `code` at CODE on `x87` (None: absent) with RAX = `rax`; returns the result, the state and the memory.
fn bare(
    code: &[u8],
    x87: Option<&X87State>,
    rax: u64,
) -> (Result<StepOutcome, MemoryError>, LongModeState, Ram) {
    let mut state = cpu();
    state.x87 = x87.cloned();
    state.gpr[Register::Rax as usize] = rax;
    let mut ram = Ram::new(code);
    ram.put(DATA, &[0xa5; 0x200]);
    let result = step(&mut state, &mut ram, PrivilegeLevel::Ring0);
    (result, state, ram)
}

fn with_selectors(mut x87: X87State, selectors: (Option<u16>, Option<u16>)) -> X87State {
    x87.instruction_selector = selectors.0;
    x87.data_selector = selectors.1;
    x87
}

/// Admitted states: the S2-39 seed and the S2-32 fixture, selectors absent and represented, several control and status words.
fn admitted_states() -> Vec<(String, X87State)> {
    let mut states = vec![];
    for selectors in [
        (None, None),
        (Some(0x2b), Some(0x53)),
        (Some(0), Some(0x1234)),
        (Some(0x33), None),
    ] {
        for (label, base) in [("seed", x87_seed()), ("fixture", X87State::fixture_seed())] {
            states.push((
                format!("{label} {selectors:?}"),
                with_selectors(base, selectors),
            ));
        }
    }
    let mut top3 = x87_seed();
    top3.control_word = 0x0f7f;
    top3.status_word = 0x1800 | 0x0041;
    top3.tag_word = 0xffff;
    states.push(("top3 empty".to_string(), top3));
    states
}

#[test]
fn fstenv_and_fsave_spellings_leave_the_state_of_the_non_waiting_forms() {
    for (label, x87) in admitted_states() {
        for (waiting, plain, length) in [
            (&FSTENV[..], &FNSTENV[..], 28),
            (&FSAVE[..], &FNSAVE[..], 108),
        ] {
            let (reference, plain_state, plain_ram) = bare(plain, Some(&x87), DATA);
            let (result, state, ram) = bare(waiting, Some(&x87), DATA);
            assert_eq!(reference.unwrap(), StepOutcome::Advanced, "{label}");
            assert_eq!(result.unwrap(), StepOutcome::Advanced, "{label}");
            assert_eq!(
                state.rip,
                CODE + 3,
                "{label}: one instruction of three bytes"
            );
            assert_eq!(plain_state.rip, CODE + 2);
            // Everything but RIP is the state of the non-waiting form.
            let mut state_without_rip = state.clone();
            state_without_rip.rip = plain_state.rip;
            assert_eq!(state_without_rip, plain_state, "{label}");
            assert_eq!(ram.get(DATA, 0x200), plain_ram.get(DATA, 0x200), "{label}");
            assert_eq!(
                ram.writes,
                [(DATA, length)],
                "{label}: one write of the image"
            );
            assert!(ram.reads.is_empty(), "{label}: no read");
            assert_eq!(plain_ram.writes, ram.writes, "{label}");
        }
    }
}

#[test]
fn the_waiting_spellings_reproduce_the_324_core_i5_3210m_rows() {
    // The physical answers of the FWAIT spellings are the answers of the non-waiting forms.
    assert_eq!(ORACLE_FWAIT, ORACLE_NON_WAITING);
    let mut rows = 0;
    for line in ORACLE_FWAIT.lines() {
        let fields: Vec<Vec<u8>> = line.split_whitespace().map(hex).collect();
        let [image, env, save, env2] = &fields[..] else {
            panic!("{line}")
        };
        assert_eq!(
            (image.len(), env.len(), save.len(), env2.len()),
            (108, 28, 108, 28)
        );
        rows += 1;
        for selectors in [(None, None), (Some(0x5a5a), Some(0xa5a5))] {
            let mut state = cpu();
            state.x87 = Some(with_selectors(X87State::fixture_seed(), selectors));
            let mut ram = Ram::new(&[]);
            ram.put(IN, image);
            // FNINIT is the initial state of the model after FRSTOR: the image overwrites the environment.
            let mut run = |code: &[u8], address: u64| {
                state.rip = CODE;
                state.gpr[Register::Rax as usize] = address;
                ram.put(CODE, code);
                step(&mut state, &mut ram, PrivilegeLevel::Ring0).unwrap()
            };
            assert_eq!(run(&FRSTOR, IN), StepOutcome::Advanced, "{line}");
            assert_eq!(run(&FSTENV, ENV), StepOutcome::Advanced, "{line}");
            assert_eq!(run(&FSAVE, SAVE), StepOutcome::Advanced, "{line}");
            assert_eq!(run(&FSTENV, ENV2), StepOutcome::Advanced, "{line}");
            assert_eq!(state.rip, CODE + 3);
            assert_eq!(ram.get(ENV, 28), &env[..], "env {line}");
            assert_eq!(ram.get(SAVE, 108), &save[..], "save {line}");
            assert_eq!(ram.get(ENV2, 28), &env2[..], "env2 {line}");
            assert_eq!(
                ram.writes,
                [(ENV, 28), (SAVE, 108), (ENV2, 28)],
                "one write per image: {line}"
            );
        }
    }
    assert_eq!(rows, 324);
}

#[test]
fn a_state_that_would_raise_mf_at_the_fwait_is_refused_with_no_trace() {
    let mut unmasked = x87_seed();
    unmasked.control_word = 0x037e; // IM clear
    unmasked.status_word = 0x0081; // ES and IE: a pending unmasked exception
    let mut es_only = x87_seed();
    es_only.status_word |= 0x0080;
    let mut busy = x87_seed();
    busy.status_word |= 0x8000;
    let mut unmasked_control = x87_seed();
    unmasked_control.control_word = 0x037e;
    for (label, x87) in [
        ("pending unmasked", Some(unmasked)),
        ("ES", Some(es_only)),
        ("B", Some(busy)),
        ("unmasked control word", Some(unmasked_control)),
        ("no x87 state", None),
    ] {
        for (waiting, plain) in [(&FSTENV[..], &FNSTENV[..]), (&FSAVE[..], &FNSAVE[..])] {
            let mut before = cpu();
            before.x87 = x87.clone();
            let (result, state, ram) = bare(waiting, x87.as_ref(), DATA);
            assert_eq!(
                result.unwrap(),
                StepOutcome::Unsupported { opcode: 0x9b },
                "{label}"
            );
            assert_eq!(state, before, "{label}: no state change, RIP included");
            assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
            assert_eq!(
                ram.get(DATA, 0x200),
                &[0xa5; 0x200][..],
                "{label}: memory untouched"
            );
            // The non-waiting form is refused by the same admission (it has no FWAIT to carry the exception).
            let (plain_result, _, _) = bare(plain, x87.as_ref(), DATA);
            assert!(
                matches!(plain_result, Ok(StepOutcome::Unsupported { opcode }) if opcode == plain[0]),
                "{label}: {plain_result:?}"
            );
        }
    }
}

#[test]
fn memory_faults_of_the_waiting_spellings_are_those_of_the_non_waiting_forms() {
    let x87 = x87_seed();
    for (label, rax, fault_page) in [
        ("non-canonical", 0x0000_8000_0000_0000_u64, None),
        ("unmapped", 0x1_0000, None),
        ("page fault", DATA, Some(DATA)),
        ("wraps past 2^64 (FSAVE 108 bytes)", u64::MAX - 100, None),
        ("last byte non-canonical", 0x0000_7fff_ffff_fff0, None),
    ] {
        for (waiting, plain) in [(&FSTENV[..], &FNSTENV[..]), (&FSAVE[..], &FNSAVE[..])] {
            let run = |code: &[u8]| {
                let mut state = cpu();
                state.x87 = Some(x87.clone());
                state.gpr[Register::Rax as usize] = rax;
                let mut ram = Ram::new(code);
                ram.fault_page = fault_page;
                let before = (state.clone(), ram.bytes.clone());
                let result = step(&mut state, &mut ram, PrivilegeLevel::Ring0);
                (format!("{result:?}"), state, before, ram.bytes)
            };
            let (waiting_result, waiting_state, waiting_before, waiting_bytes) = run(waiting);
            let (plain_result, plain_state, ..) = run(plain);
            assert_eq!(waiting_result, plain_result, "{label} {waiting:02x?}");
            let mut expected = waiting_before.0.clone();
            if waiting_result.starts_with("Ok(Fault") {
                // The FWAIT part retired: the saved RIP of the fault is the FNSTENV/FNSAVE byte, 9B + 1 (Intel SDM,
                // FSTENV/FSAVE: "the save EIP points to the instruction that caused the exception"); nothing else moved.
                expected.rip = CODE + 1;
                assert_eq!(
                    waiting_state, expected,
                    "{label}: only RIP moved, to 9B + 1"
                );
            } else {
                // A model error is not an x86 exception: Err, state untouched, RIP at 9B (as FNSTENV/FLDENV).
                assert!(
                    waiting_result.starts_with("Err("),
                    "{label}: {waiting_result}"
                );
                assert_eq!(
                    waiting_state, expected,
                    "{label}: model error, RIP stays 9B"
                );
            }
            assert_eq!(plain_state.rip, CODE, "{label}");
            assert_eq!(waiting_bytes, waiting_before.1, "{label}: memory unchanged");
        }
    }
}

#[test]
fn a_model_error_of_the_waiting_spellings_leaves_rip_at_the_9b_byte() {
    let x87 = x87_seed();
    for (label, rax, waiting, expect_overflow) in [
        ("unmapped FSTENV", 0x1_0000_u64, &FSTENV[..], false),
        ("unmapped FSAVE", 0x1_0000, &FSAVE[..], false),
        ("wraps past 2^64 FSAVE", u64::MAX - 100, &FSAVE[..], true),
    ] {
        let mut state = cpu();
        state.x87 = Some(x87.clone());
        state.gpr[Register::Rax as usize] = rax;
        let before = state.clone();
        let mut ram = Ram::new(waiting);
        let bytes_before = ram.bytes.clone();
        let result = step(&mut state, &mut ram, PrivilegeLevel::Ring0);
        match result {
            Err(MemoryError::AddressOverflow { .. }) => assert!(expect_overflow, "{label}"),
            Err(MemoryError::Unmapped { .. }) => assert!(!expect_overflow, "{label}"),
            other => panic!("{label}: {other:?}"),
        }
        assert_eq!(state, before, "{label}: state untouched, RIP is 9B");
        assert_eq!(state.rip, CODE, "{label}");
        assert_eq!(ram.bytes, bytes_before, "{label}: memory untouched");
    }
}

#[test]
fn an_architectural_general_protection_of_the_waiting_spellings_has_rip_9b_plus_1() {
    let x87 = x87_seed();
    for (label, rax, waiting) in [
        (
            "non-canonical FSTENV",
            0x0000_8000_0000_0000_u64,
            &FSTENV[..],
        ),
        ("non-canonical FSAVE", 0x0000_8000_0000_0000, &FSAVE[..]),
        (
            "last byte non-canonical FSAVE",
            0x0000_7fff_ffff_ffa0,
            &FSAVE[..],
        ),
    ] {
        let mut state = cpu();
        state.x87 = Some(x87.clone());
        state.gpr[Register::Rax as usize] = rax;
        let mut before = state.clone();
        let mut ram = Ram::new(waiting);
        let result = step(&mut state, &mut ram, PrivilegeLevel::Ring0);
        assert!(
            matches!(result, Ok(StepOutcome::Fault(_))),
            "{label}: {result:?}"
        );
        before.rip = CODE + 1;
        assert_eq!(state, before, "{label}: only RIP moved, to 9B + 1");
        assert!(ram.writes.is_empty(), "{label}");
    }
}

#[test]
fn decoder_consumes_exactly_the_two_spellings() {
    for (code, operation, length) in [
        (&FSTENV[..], Operation::FnstenvRax, 3),
        (&FSAVE[..], Operation::FnsaveRax, 3),
        (&FNSTENV[..], Operation::FnstenvRax, 2),
        (&FNSAVE[..], Operation::FnsaveRax, 2),
        (&FLDENV[..], Operation::FldenvRax, 2),
        (&FRSTOR[..], Operation::FrstorRax, 2),
    ] {
        let instruction = decode(code).unwrap();
        assert_eq!(
            (instruction.operation, instruction.length),
            (operation, length),
            "{code:02x?}"
        );
        assert_eq!(instruction.prefixes, Prefixes::default());
    }
    assert_eq!(decode(&FSTENV).unwrap().opcode.byte, 0x9b);
    // 9B before anything else (and the neighbouring modrm bytes) stays the bare FWAIT of its own byte.
    for code in [
        &[0x9b][..],
        &[0x9b, 0xd9, 0x20],
        &[0x9b, 0xdd, 0x20],
        &[0x9b, 0xd9, 0x31],
        &[0x9b, 0xd9, 0x38],
        &[0x9b, 0xdd, 0x38],
        &[0x9b, 0xd9, 0xe4],
        &[0x9b, 0xdb, 0xe3],
        &[0x9b, 0xdf, 0x30],
        &[0x9b, 0x90, 0x30],
        &[0x9b, 0x9b, 0xd9, 0x30],
    ] {
        let instruction = decode(code).unwrap();
        assert_eq!(
            (instruction.operation, instruction.length),
            (Operation::Fwait, 1),
            "{code:02x?}"
        );
    }
    // A prefix between FWAIT and the escape byte: FWAIT alone, the rest is a prefixed (refused) x87 form.
    for prefix in PREFIXES {
        for escape in [0xd9, 0xdd] {
            let code = [0x9b, prefix, escape, 0x30];
            let instruction = decode(&code).unwrap();
            assert_eq!(
                (instruction.operation, instruction.length),
                (Operation::Fwait, 1),
                "{code:02x?}"
            );
            // S7-32 W4: `67 DD 30` and a REX without W before `DD 30` are the general FNSAVE at
            // [EAX] or [RAX]/[R8] (only the FP member of the CPL0 set executes it).
            let rest = decode(&code[1..]);
            if escape == 0xdd && (prefix == 0x67 || (0x40..=0x47).contains(&prefix)) {
                assert!(
                    matches!(
                        rest,
                        Ok(long_decode::Instruction {
                            operation: Operation::FnsaveMemory { .. },
                            ..
                        })
                    ),
                    "{code:02x?}: {rest:?}"
                );
                continue;
            }
            assert!(
                matches!(rest, Err(DecodeError::Unsupported { .. })),
                "{code:02x?}"
            );
        }
    }
    // A prefix before 9B is refused (LOCK excepted: the bare FWAIT of the CPL0 set reports #UD for it).
    for prefix in PREFIXES {
        for waiting in [FSTENV, FSAVE] {
            let code = [prefix, waiting[0], waiting[1], waiting[2]];
            assert!(
                matches!(decode(&code), Err(DecodeError::Unsupported { .. })),
                "{code:02x?}"
            );
        }
    }
    let locked = decode(&[0xf0, 0x9b, 0xd9, 0x30]).unwrap();
    assert_eq!((locked.operation, locked.length), (Operation::Fwait, 2));
    assert_eq!(locked.prefixes.group1, Some(0xf0));
}

#[test]
fn prefixed_and_split_spellings_are_refused_with_no_trace() {
    let x87 = x87_seed();
    let mut cases: Vec<Vec<u8>> = vec![
        vec![0x9b, 0xd9, 0x20],
        vec![0x9b, 0xdd, 0x20],
        vec![0x9b, 0xd9, 0x31],
    ];
    for prefix in PREFIXES {
        cases.push(vec![prefix, 0x9b, 0xd9, 0x30]);
        cases.push(vec![prefix, 0x9b, 0xdd, 0x30]);
        cases.push(vec![0x9b, prefix, 0xd9, 0x30]);
        cases.push(vec![0x9b, prefix, 0xdd, 0x30]);
    }
    for code in cases {
        let mut before = cpu();
        before.x87 = Some(x87.clone());
        let (result, state, ram) = bare(&code, Some(&x87), DATA);
        assert_eq!(
            result.unwrap(),
            StepOutcome::Unsupported { opcode: 0x9b },
            "{code:02x?}"
        );
        assert_eq!(state, before, "{code:02x?}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn a_truncated_or_faulting_lookahead_leaves_a_bare_fwait() {
    // FWAIT is the last byte, or the last two, of the mapped memory: the decode of FWAIT does not fail
    // on the bytes after it, and the bare FWAIT is refused by the bounded profile as before.
    for (tail, label) in [
        (&[0x9b][..], "9B"),
        (&[0x9b, 0xd9][..], "9B D9"),
        (&[0x9b, 0xdd][..], "9B DD"),
    ] {
        let start = RAM - tail.len();
        let mut ram = Ram::new(&[]);
        ram.put(start as u64, tail);
        let instruction =
            long_decode::decode(&mut ram, start as u64, AccessContext::default()).unwrap();
        assert_eq!(
            (instruction.operation, instruction.length),
            (Operation::Fwait, 1),
            "{label}"
        );
        let mut state = cpu();
        state.rip = start as u64;
        let before = state.clone();
        assert_eq!(
            step(&mut state, &mut ram, PrivilegeLevel::Ring0).unwrap(),
            StepOutcome::Unsupported { opcode: 0x9b },
            "{label}"
        );
        assert_eq!(state, before, "{label}");
    }
    // 9B D9 30 with the third byte unmapped is FWAIT alone as well.
    let start = RAM - 2;
    let mut ram = Ram::new(&[]);
    ram.put(start as u64, &[0x9b, 0xd9]);
    assert_eq!(
        long_decode::decode(&mut ram, start as u64, AccessContext::default())
            .unwrap()
            .length,
        1
    );
}

/// A RAM that records the address of every instruction fetch.
struct FetchLog {
    ram: Ram,
    fetches: Vec<u64>,
}

impl GuestMemory for FetchLog {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetches.push(address);
        self.ram.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        out: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.ram.read(address, context, out)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        data: &[u8],
    ) -> Result<(), MemoryError> {
        self.ram.write(address, context, data)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.ram.mfence(context)
    }
}

#[test]
fn the_lookahead_after_9b_never_leaves_the_page_of_the_9b_byte() {
    // A page boundary at 0x5000. The three bytes of the spelling must all lie on the page of 9B, otherwise no byte after
    // 9B is fetched (the next page may not be walked or recorded for a bare FWAIT) and the instruction is the bare FWAIT.
    for (waiting, plain_operation) in [
        (&FSTENV, Operation::FnstenvRax),
        (&FSAVE, Operation::FnsaveRax),
    ] {
        for (start, fetched, operation, length) in [
            (0x4ffd_u64, 3, plain_operation, 3), // 9B D9 30 end on the last byte of the page
            (0x4ffe, 1, Operation::Fwait, 1), // 30 would be on the next page: 9B alone, nothing after it fetched
            (0x4fff, 1, Operation::Fwait, 1), // 9B is the last byte: nothing after it fetched
            (0x5000, 3, plain_operation, 3), // a spelling that starts on the next page is whole again
        ] {
            let mut log = FetchLog {
                ram: Ram::new(&[]),
                fetches: vec![],
            };
            log.ram.put(start, waiting);
            let instruction =
                long_decode::decode(&mut log, start, AccessContext::default()).unwrap();
            assert_eq!(
                (instruction.operation, instruction.length),
                (operation, length),
                "{start:#x} {waiting:02x?}"
            );
            assert_eq!(
                log.fetches,
                (start..start + fetched).collect::<Vec<_>>(),
                "{start:#x} {waiting:02x?}: fetches"
            );
        }
    }
}

#[test]
fn fldenv_frstor_and_the_non_waiting_forms_do_not_change() {
    let x87 = x87_seed();
    // The non-waiting forms keep their two bytes, their opcode byte and their one memory access.
    // The image of FNSAVE of the seed is what FLDENV and FRSTOR load.
    let (_, _, saved) = bare(&FNSAVE, Some(&x87), DATA);
    let image = saved.get(DATA, 108).to_vec();
    for (code, length, writes, reads) in [
        (&FNSTENV[..], 28, true, false),
        (&FNSAVE[..], 108, true, false),
        (&FLDENV[..], 28, false, true),
        (&FRSTOR[..], 108, false, true),
    ] {
        let mut state = cpu();
        state.x87 = Some(x87.clone());
        let mut ram = Ram::new(code);
        ram.put(DATA, &image);
        let result = step(&mut state, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(result.unwrap(), StepOutcome::Advanced, "{code:02x?}");
        assert_eq!(state.rip, CODE + 2, "{code:02x?}");
        assert_eq!(ram.writes.len(), usize::from(writes), "{code:02x?}");
        assert_eq!(
            ram.reads,
            if reads { vec![(DATA, length)] } else { vec![] },
            "{code:02x?}"
        );
    }
    // FLDENV has no spelling of its own with FWAIT: `9B D9 20` is FWAIT then FLDENV, FWAIT being refused here.
    let (result, state, ram) = bare(&[0x9b, 0xd9, 0x20], Some(&x87), DATA);
    assert_eq!(result.unwrap(), StepOutcome::Unsupported { opcode: 0x9b });
    assert_eq!(state.rip, CODE);
    assert!(ram.reads.is_empty());
}

// ---- CPL0 of the set -------------------------------------------------------------------------------------------

const CR4_FP: u64 = CR4_PAE | CR4_OSFXSR | (1 << 10);

fn seed() -> X87State {
    let mut x87 = x87_seed();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    x87
}

/// One instruction in the set; returns the exit, the state, the cell before and after, the memory and the accesses.
fn in_set(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    x87: &X87State,
) -> (
    String,
    LongModeState,
    GuestFpState,
    GuestFpState,
    Vec<u8>,
    usize,
) {
    // The memory returned is the data area [DATA, DATA + 0x200).
    let mut state = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        cr0: Some(cr0),
        cr4: Some(cr4),
        activity: Some(od_isa::CpuActivity::Running),
        sti_shadow: Some(od_isa::StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr = cpu().gpr;
    state.gpr[Register::Rax as usize] = DATA;
    let mut ram = Ram::new(code);
    ram.put(DATA, &[0xa5; 0x200]);
    let cell = RefCell::new(GuestFpState {
        x87: x87.clone(),
        sse: cpu().sse.unwrap(),
    });
    let before = cell.borrow().clone();
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(PrivilegeLevel::Ring0),
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .expect("the case is admitted by the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    let after = cell.borrow().clone();
    (
        format!("{exit:?}"),
        state,
        before,
        after,
        ram.get(DATA, 0x200).to_vec(),
        ram.reads.len() + ram.writes.len(),
    )
}

#[test]
fn in_the_set_the_waiting_spellings_store_the_image_of_the_non_waiting_forms() {
    for (waiting, plain) in [(&FSTENV[..], &FNSTENV[..]), (&FSAVE[..], &FNSAVE[..])] {
        let (exit, state, before, after, bytes, accesses) =
            in_set(waiting, CR0_BASE, CR4_FP, &seed());
        let (plain_exit, plain_state, _, plain_after, plain_bytes, _) =
            in_set(plain, CR0_BASE, CR4_FP, &seed());
        assert_eq!(exit, "InstructionBudgetExhausted");
        assert_eq!(exit, plain_exit);
        assert_eq!((state.rip, plain_state.rip), (CODE + 3, CODE + 2));
        assert_eq!(after, plain_after);
        assert_eq!(bytes, plain_bytes);
        // FSTENV stores in one write; FSAVE, like FNSAVE in the set since S7-32 W4, in 14 aligned pieces
        // (13 qwords and a dword at the 8-byte aligned DATA).
        let stores = if waiting == &FSAVE[..] { 14 } else { 1 };
        assert_eq!(accesses, stores, "the stores of the image");
        assert_eq!(before.sse, after.sse, "the SSE half is untouched");
    }
}

#[test]
fn in_the_set_the_fwait_checks_come_first() {
    let mut pending = seed();
    pending.control_word = 0x037e;
    pending.status_word = 0x0081;
    for (waiting, name) in [(&FSTENV[..], "fstenv"), (&FSAVE[..], "fsave")] {
        // The bare FWAIT of the set is the reference for the checks of its part: #NM for MP and TS, #MF for a
        // pending unmasked exception (the policy of the set admits only CR0.NE = 1, so the NE-clear refusal is not reachable here).
        for (label, cr0, x87) in [
            ("MP+TS", CR0_BASE | 1 << 3, seed()),
            ("pending, NE set", CR0_BASE, pending.clone()),
            ("MP+TS and pending", CR0_BASE | 1 << 3, pending.clone()),
            // CR0.EM is no condition of FWAIT: with a pending unmasked exception the FWAIT part raises #MF (16)
            // before the #NM of the FNSTENV/FNSAVE part; a check order with EM first would raise #NM (7).
            ("EM and pending", CR0_BASE | 1 << 2, pending.clone()),
        ] {
            let (fwait_exit, fwait_state, ..) = in_set(&[0x9b], cr0, CR4_FP, &x87);
            let (exit, state, before, after, bytes, accesses) = in_set(waiting, cr0, CR4_FP, &x87);
            assert_ne!(exit, "InstructionBudgetExhausted", "{name} {label}");
            assert_eq!(exit, fwait_exit, "{name} {label}");
            assert_eq!(state.rip, fwait_state.rip, "{name} {label}: RIP stays");
            assert_eq!(state.rip, CODE, "{name} {label}");
            assert_eq!(before, after, "{name} {label}: the cell is unchanged");
            assert_eq!(accesses, 0, "{name} {label}: no memory access");
            assert_eq!(bytes, vec![0xa5; 0x200], "{name} {label}");
        }
        // The non-waiting form does not wait, no #MF: FNSTENV is refused by the bounded profile for that state;
        // FNSAVE (S7-32 W4, `step_cpl0_fnsave`) stores it and initializes the FPU.
        let plain: &[u8] = if name == "fstenv" { &FNSTENV } else { &FNSAVE };
        let (exit, _, _, after, bytes, _) = in_set(plain, CR0_BASE, CR4_FP, &pending);
        if name == "fstenv" {
            assert!(exit.contains("Unsupported"), "{name}: {exit}");
        } else {
            assert_eq!(exit, "InstructionBudgetExhausted", "{name}");
            assert_eq!(bytes[0..2], 0x037eu16.to_le_bytes(), "{name}");
            assert_eq!(bytes[4..6], 0x0081u16.to_le_bytes(), "{name}");
            assert_eq!(after.x87.control_word, 0x037f, "{name}");
        }
    }
    // The vectors themselves.
    let (exit, ..) = in_set(&FSTENV, CR0_BASE, CR4_FP, &pending);
    assert!(exit.contains("vector: 16"), "{exit}");
    let (exit, ..) = in_set(&FSTENV, CR0_BASE | 1 << 3, CR4_FP, &seed());
    assert!(exit.contains("vector: 7"), "{exit}");
    for waiting in [&FSTENV[..], &FSAVE[..]] {
        let (exit, state, ..) = in_set(waiting, CR0_BASE | 1 << 2, CR4_FP, &pending);
        assert!(exit.contains("vector: 16"), "EM and pending: {exit}");
        assert_eq!(state.rip, CODE, "the #MF is that of the FWAIT part");
    }
}

#[test]
fn in_the_set_em_and_ts_without_a_pending_exception_are_nm_and_change_nothing() {
    // The saved RIP depends on the part that raises #NM: CR0.MP with CR0.TS is the check of the FWAIT part (RIP = 9B);
    // CR0.EM alone is a condition of the FNSTENV/FNSAVE part only, after FWAIT retired (RIP = 9B + 1).
    for (cr0, rip) in [
        (CR0_BASE | 1 << 2, CODE + 1),
        (CR0_BASE | 1 << 3, CODE),
        (CR0_BASE | 1 << 2 | 1 << 3, CODE),
    ] {
        for waiting in [&FSTENV[..], &FSAVE[..]] {
            let (exit, state, before, after, _, accesses) = in_set(waiting, cr0, CR4_FP, &seed());
            assert!(exit.contains("vector: 7"), "{exit}");
            assert_eq!((state.rip, accesses), (rip, 0), "cr0 {cr0:#x}");
            assert_eq!(before, after);
        }
    }
}
