//! PERF-25, group G5 (ADR-0017, section 1): the ALU forms with a memory operand (`Arithmetic` and
//! `Logic`: memory destination with a register or immediate source, register destination with a
//! memory source) and the stack forms (PUSH/POP of a register, CALL rel32, RET and RET imm16) in
//! `execute::try_fast_step`. As for G4 (fast_step_g4_tests.rs), each case runs the general path (one
//! instruction through `run` of the CPL0 profile set) and the fast path (called directly on a copy of
//! the state), both over a recording [`CheckMemory`] around a fresh copy of the same memory, and
//! compares the CPU state, `rflags_defined`, the outcome (`Advanced` for the ALU forms and PUSH/POP,
//! `Branched` for CALL and RET, the same fault, the same memory error), the memory left behind and the
//! journal of the data calls of the PERF-22 check (method, address, context, width, value, error,
//! in order). A fault after a read (a read-modify-write or a push into a read-only page, a RET to a
//! non-canonical address) must have the same journal and commit nothing; inadmissible conditions
//! are refused without a single memory call.

use std::collections::{BTreeMap, BTreeSet};

use crate::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, StepOutcome,
    run_with_cpl0_profile_set, try_fast_step,
};
use crate::fast_path_check::{CheckMemory, MemoryLog, MemoryMethod};
use crate::fast_step_g2_tests::{assert_refused, context};
use crate::fast_step_g4_tests::{
    Address, CODE, End, PAGE_FAULT, READ_ONLY, Ram, Run, UNMAPPED, WINDOW, address_forms,
    assert_same_calls, decode_at, encode, general_on, register_sets, start_state, try_decode_at,
};
use crate::long_decode::{Instruction, OpcodeMap, Operand, Operation};
use crate::{INTEGER_FLAGS, LongModeState, MemoryError, PrivilegeLevel, StiShadow};

const CF: u64 = 1;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const TF: u64 = 1 << 8;
const OF: u64 = 1 << 11;
const RF: u64 = 1 << 16;

/// How the fast path ended a case it took.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Taken {
    Advanced,
    Branched,
    /// A fault or a memory error.
    Ended,
}

/// The fast path on a copy of `start` over `ram`; it must take the instruction.
fn fast_on(mut ram: Ram, bytes: &[u8], start: &LongModeState, defined: u64) -> (Run, Taken) {
    let instruction = decode_at(start.rip, bytes);
    let mut state = start.clone();
    let mut defined = defined;
    let mut recording = CheckMemory::record(&mut ram, MemoryLog::default());
    let outcome = try_fast_step(
        &mut state,
        &mut recording,
        context(PrivilegeLevel::Ring0),
        &mut defined,
        &instruction,
        true,
    );
    let log = recording.into_log();
    let (end, taken) = match outcome {
        Some(Ok(StepOutcome::Advanced)) => (End::Retired, Taken::Advanced),
        Some(Ok(StepOutcome::Branched)) => (End::Retired, Taken::Branched),
        Some(Ok(StepOutcome::Fault(fault))) => (End::Fault(fault), Taken::Ended),
        Some(Err(error)) => (End::Error(error), Taken::Ended),
        other => panic!("{bytes:02x?}: not taken or another outcome: {other:?}"),
    };
    (
        Run {
            state,
            defined,
            end,
            ram,
            log,
        },
        taken,
    )
}

/// Runs both paths over copies of `ram` and compares everything; returns the general run. A
/// retired case must be `Branched` when `branch` (CALL, RET) and `Advanced` otherwise; a fault or
/// an error changes nothing (state, definedness, memory).
fn compare_on(
    label: &str,
    bytes: &[u8],
    start: &LongModeState,
    defined: u64,
    ram: Ram,
    branch: bool,
) -> Run {
    let initial = ram.written.clone();
    let expected = general_on(ram.clone(), bytes, start, defined);
    let (got, taken) = fast_on(ram, bytes, start, defined);
    assert_eq!(got.end, expected.end, "{label}: {bytes:02x?}");
    assert_eq!(got.state, expected.state, "{label}: {bytes:02x?}");
    assert_eq!(got.defined, expected.defined, "{label}: {bytes:02x?}");
    assert_eq!(
        got.ram.written, expected.ram.written,
        "{label}: {bytes:02x?}"
    );
    assert_same_calls(label, &expected.log, &got.log);
    if expected.end == End::Retired {
        let want = if branch {
            Taken::Branched
        } else {
            Taken::Advanced
        };
        assert_eq!(taken, want, "{label}: {bytes:02x?}");
    } else {
        assert_eq!(expected.state, *start, "{label}: {bytes:02x?}");
        assert_eq!(expected.defined, defined, "{label}: {bytes:02x?}");
        assert_eq!(expected.ram.written, initial, "{label}: {bytes:02x?}");
    }
    expected
}

pub(super) fn compare(
    label: &str,
    bytes: &[u8],
    start: &LongModeState,
    defined: u64,
    branch: bool,
) -> Run {
    compare_on(
        label,
        bytes,
        start,
        defined,
        Ram::new(start.rip, bytes),
        branch,
    )
}

/// A short name of how a run ended, for the coverage counts.
pub(super) fn end_name(run: &Run) -> String {
    match &run.end {
        End::Retired => "retired".to_owned(),
        End::Fault(fault) => format!("fault {}", fault.vector),
        End::Error(MemoryError::Unmapped { .. }) => "unmapped".to_owned(),
        End::Error(MemoryError::AddressOverflow { .. }) => "overflow".to_owned(),
        End::Error(error) => panic!("{error:?}"),
    }
}

const REGS: [u8; 6] = [0, 3, 4, 7, 9, 15];
/// (label, legacy prefixes, REX.W) of the 16-, 32- and 64-bit forms.
const WIDTHS: [(&str, &[u8], bool); 3] = [
    ("16", &[0x66], false),
    ("32", &[], false),
    ("64", &[], true),
];

/// (label, bytes) of every G5 ALU encoding over `form`: the eight classic operations (ADD, OR, ADC,
/// SBB, AND, SUB, XOR, CMP) as r/m, r and r, r/m of each width (high-byte and REX byte registers),
/// 80/81/83 with an immediate; TEST r/m, r and TEST r/m, imm; INC, DEC, NOT, NEG of each width.
fn alu_encodings(form: &Address) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let f = &form.label;
    for (n, name) in ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"]
        .into_iter()
        .enumerate()
    {
        let base = n as u8 * 8;
        let reg = REGS[n % REGS.len()];
        let rex_choices: &[bool] = if (4..8).contains(&reg) {
            &[false, true]
        } else {
            &[false]
        };
        for &force_rex in rex_choices {
            let tag = format!("reg {reg} rex {force_rex} {f}");
            out.push((
                format!("{name} m8, r8 {tag}"),
                encode(&[], false, force_rex, &[base], reg, form, &[]),
            ));
            out.push((
                format!("{name} r8, m8 {tag}"),
                encode(&[], false, force_rex, &[base + 2], reg, form, &[]),
            ));
        }
        out.push((
            format!("{name} m8, imm8 {f}"),
            encode(&[], false, false, &[0x80], n as u8, form, &[0x9c]),
        ));
        for (width, legacy, wide) in WIDTHS {
            let reg = REGS[(n + 3) % REGS.len()];
            out.push((
                format!("{name} m{width}, r reg {reg} {f}"),
                encode(legacy, wide, false, &[base + 1], reg, form, &[]),
            ));
            out.push((
                format!("{name} r{width}, m reg {reg} {f}"),
                encode(legacy, wide, false, &[base + 3], reg, form, &[]),
            ));
            let immediate: &[u8] = if width == "16" {
                &[0x34, 0x92]
            } else {
                &[0x78, 0x56, 0x34, 0x92]
            };
            out.push((
                format!("{name} m{width}, imm {f}"),
                encode(legacy, wide, false, &[0x81], n as u8, form, immediate),
            ));
            out.push((
                format!("{name} m{width}, simm8 {f}"),
                encode(legacy, wide, false, &[0x83], n as u8, form, &[0xf0]),
            ));
        }
    }
    out.push((
        format!("test m8, r8 bh {f}"),
        encode(&[], false, false, &[0x84], 7, form, &[]),
    ));
    out.push((
        format!("test m8, r8 sil {f}"),
        encode(&[], false, true, &[0x84], 6, form, &[]),
    ));
    out.push((
        format!("test m8, imm8 {f}"),
        encode(&[], false, false, &[0xf6], 0, form, &[0x81]),
    ));
    for (width, legacy, wide) in WIDTHS {
        out.push((
            format!("test m{width}, r {f}"),
            encode(legacy, wide, false, &[0x85], 9, form, &[]),
        ));
        let immediate: &[u8] = if width == "16" {
            &[0x01, 0x80]
        } else {
            &[0x01, 0x00, 0x00, 0x80]
        };
        out.push((
            format!("test m{width}, imm {f}"),
            encode(legacy, wide, false, &[0xf7], 0, form, immediate),
        ));
    }
    for (byte_op, op, extension, name) in [
        (0xfeu8, 0xffu8, 0u8, "inc"),
        (0xfe, 0xff, 1, "dec"),
        (0xf6, 0xf7, 2, "not"),
        (0xf6, 0xf7, 3, "neg"),
    ] {
        out.push((
            format!("{name} m8 {f}"),
            encode(&[], false, false, &[byte_op], extension, form, &[]),
        ));
        for (width, legacy, wide) in WIDTHS {
            out.push((
                format!("{name} m{width} {f}"),
                encode(legacy, wide, false, &[op], extension, form, &[]),
            ));
        }
    }
    out
}

/// The decoded operation is a G5 ALU form with memory (checks the generator); its kind, direction
/// and whether it writes its destination.
fn alu_kind(instruction: &Instruction) -> (String, bool) {
    match instruction.operation {
        Operation::Arithmetic {
            kind,
            dst: Operand::Memory(_),
            src: Operand::Register(_) | Operand::Immediate(_),
        } => (
            format!("{kind:?} m"),
            kind != crate::long_decode::ArithmeticKind::Cmp,
        ),
        Operation::Arithmetic {
            kind,
            dst: Operand::Register(_),
            src: Operand::Memory(_),
        } => (format!("{kind:?} r, m"), false),
        Operation::Logic {
            kind,
            dst: Operand::Memory(_),
            src: Operand::Register(_) | Operand::Immediate(_),
        } => (
            format!("{kind:?} m"),
            kind != crate::long_decode::LogicKind::Test,
        ),
        Operation::Logic {
            kind,
            dst: Operand::Register(_),
            src: Operand::Memory(_),
        } => (format!("{kind:?} r, m"), false),
        other => panic!("{other:?} is not a G5 ALU memory form"),
    }
}

#[test]
fn every_g5_alu_memory_form_equals_the_general_path_with_the_same_memory_calls() {
    let forms = address_forms();
    let sets = register_sets();
    let mut kinds = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    let mut ends = BTreeMap::<String, u64>::new();
    let mut skipped = BTreeSet::new();
    let mut calls = 0u64;
    let mut cases = 0u64;
    for (index, form) in forms.iter().enumerate() {
        let encodings = alu_encodings(form);
        // Every operation over a few forms, and a rotating seventh of them over every form: each
        // encoding shape meets every kind of address.
        let full = index % 29 == 0;
        for (number, (label, bytes)) in encodings.iter().enumerate() {
            if !full && (number + index) % 7 != 0 {
                continue;
            }
            let Some(instruction) = try_decode_at(CODE, bytes) else {
                skipped.insert(label.split(" reg ").next().unwrap_or(label).to_owned());
                continue;
            };
            let (kind, writes_memory) = alu_kind(&instruction);
            kinds.insert(kind);
            sizes.insert(instruction.operand_size.bytes());
            for (set, gpr) in &sets {
                for (rflags, defined) in [
                    (2 | CF | ZF, INTEGER_FLAGS),
                    (2 | SF | OF | AF, INTEGER_FLAGS & !(AF | OF)),
                ] {
                    let start = start_state(*gpr, CODE, rflags);
                    let run = compare(
                        &format!("{label} regs {set}"),
                        bytes,
                        &start,
                        defined,
                        false,
                    );
                    *ends.entry(end_name(&run)).or_default() += 1;
                    // A read, and a write for an operation that stores into memory.
                    let max = 1 + usize::from(writes_memory);
                    assert!(run.log.calls().len() <= max, "{label}");
                    if run.end == End::Retired {
                        assert_eq!(run.log.calls().len(), max, "{label}");
                    }
                    calls += run.log.calls().len() as u64;
                    cases += 1;
                }
            }
        }
    }
    assert!(skipped.is_empty(), "{skipped:?}");
    let expected_kinds: BTreeSet<String> = [
        "Add m", "Add r, m", "Adc m", "Adc r, m", "Sub m", "Sub r, m", "Sbb m", "Sbb r, m",
        "Cmp m", "Cmp r, m", "Inc m", "Dec m", "Neg m", "And m", "And r, m", "Or m", "Or r, m",
        "Xor m", "Xor r, m", "Test m", "Not m",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(kinds, expected_kinds);
    assert_eq!(sizes, BTreeSet::from([1, 2, 4, 8]));
    assert!(ends["retired"] > 5_000, "{ends:?}");
    assert!(ends.get("fault 13").copied().unwrap_or(0) > 500, "{ends:?}");
    assert!(ends.get("fault 12").copied().unwrap_or(0) > 50, "{ends:?}");
    assert!(calls > 10_000, "{calls}");
    assert!(cases > 20_000, "{cases}");
}

/// ALU forms over [rsi], [rsp], [rbp-8] and [rsi+rcx*8+8], with RSI, RSP and RBP placed by each case.
const ALU_FAULT_SAMPLE: &[(&str, &[u8])] = &[
    ("add [rsi], rax", &[0x48, 0x01, 0x06]),
    ("add rax, [rsi]", &[0x48, 0x03, 0x06]),
    ("sub dword [rsi], 5", &[0x83, 0x2e, 0x05]),
    ("cmp [rsi], al", &[0x38, 0x06]),
    ("cmp rax, [rsi]", &[0x48, 0x3b, 0x06]),
    ("inc qword [rsi]", &[0x48, 0xff, 0x06]),
    ("neg word [rsi]", &[0x66, 0xf7, 0x1e]),
    ("not byte [rsi]", &[0xf6, 0x16]),
    ("and [rsi], ecx", &[0x21, 0x0e]),
    ("test byte [rsi], 1", &[0xf6, 0x06, 0x01]),
    ("or rax, [rsi+rcx*8+8]", &[0x48, 0x0b, 0x44, 0xce, 0x08]),
    ("xor [rsp], rax", &[0x48, 0x31, 0x04, 0x24]),
    ("test [rsp+8], eax", &[0x85, 0x44, 0x24, 0x08]),
    ("adc [rbp-8], ecx", &[0x11, 0x4d, 0xf8]),
    ("sbb rax, [rbp-8]", &[0x48, 0x1b, 0x45, 0xf8]),
];

fn fault_state(base: u64) -> LongModeState {
    let mut gpr = [0u64; 16];
    gpr[0] = 0x1122_3344_5566_7788;
    gpr[1] = 0;
    gpr[4] = base;
    gpr[5] = base.wrapping_add(8);
    gpr[6] = base;
    start_state(gpr, CODE, 2 | CF)
}

/// The bases of the fault cases: RAM, page faults (first byte, straddling), a read-only page (the
/// read succeeds, the write faults), a backend error, non-canonical starts and last bytes, a range
/// past the end of the address space.
const FAULT_BASES: [(&str, u64); 10] = [
    ("ram", 0x0000_0000_0030_0000),
    ("page fault", PAGE_FAULT + 0x10),
    ("page fault straddle", PAGE_FAULT - 4),
    ("read-only", READ_ONLY + 0x10),
    ("read-only straddle", READ_ONLY - 4),
    ("unmapped", UNMAPPED + 0x20),
    ("non-canonical", 0x0000_8000_0000_0000),
    ("non-canonical high", 0xfff0_0000_0000_0000),
    ("canonical edge", 0x0000_7fff_ffff_fffe),
    ("overflow", 0xffff_ffff_ffff_fffe),
];

#[test]
fn alu_memory_faults_and_errors_end_the_instruction_as_the_general_path_does() {
    let mut seen = BTreeMap::<&str, u64>::new();
    for (label, bytes) in ALU_FAULT_SAMPLE {
        let (_, writes) = alu_kind(&decode_at(CODE, bytes));
        let stack_based = label.contains("[rsp") || label.contains("[rbp");
        for (case, base) in FAULT_BASES {
            let start = fault_state(base);
            for defined in [INTEGER_FLAGS, INTEGER_FLAGS & !(SF | OF)] {
                let run = compare(&format!("{label} {case}"), bytes, &start, defined, false);
                let calls = run.log.calls();
                let kind = match &run.end {
                    End::Retired => "retired",
                    End::Fault(fault) if fault.vector == 14 && calls.len() == 2 => {
                        // The read succeeded, the write faulted: the journal has both, in order.
                        assert!(writes, "{label} {case}");
                        assert_eq!(calls[0].method, MemoryMethod::ReadInteger, "{label} {case}");
                        assert!(calls[0].error.is_none(), "{label} {case}");
                        assert_eq!(
                            calls[1].method,
                            MemoryMethod::WriteInteger,
                            "{label} {case}"
                        );
                        assert!(calls[1].error.is_some(), "{label} {case}");
                        assert_eq!(calls[0].address, calls[1].address, "{label} {case}");
                        "#PF after the read"
                    }
                    End::Fault(fault) if fault.vector == 14 => {
                        assert_eq!(calls.len(), 1, "{label} {case}");
                        assert!(calls[0].error.is_some(), "{label} {case}");
                        "#PF"
                    }
                    End::Fault(fault) if fault.vector == 13 => {
                        assert!(!stack_based, "{label} {case}");
                        assert!(calls.is_empty(), "{label} {case}");
                        "#GP"
                    }
                    End::Fault(fault) if fault.vector == 12 => {
                        assert!(stack_based, "{label} {case}");
                        assert!(calls.is_empty(), "{label} {case}");
                        "#SS"
                    }
                    End::Fault(fault) => panic!("{label} {case}: {fault:?}"),
                    End::Error(MemoryError::Unmapped { .. }) => {
                        assert_eq!(calls.len(), 1, "{label} {case}");
                        "unmapped"
                    }
                    End::Error(MemoryError::AddressOverflow { .. }) => {
                        assert!(calls.is_empty(), "{label} {case}");
                        "overflow"
                    }
                    End::Error(error) => panic!("{label} {case}: {error:?}"),
                };
                if case.starts_with("read-only") && !writes {
                    // CMP, TEST and the register destinations only read: a read-only page retires.
                    assert_eq!(kind, "retired", "{label} {case}");
                }
                *seen.entry(kind).or_default() += 1;
            }
        }
    }
    for kind in [
        "retired",
        "#PF",
        "#PF after the read",
        "#GP",
        "#SS",
        "unmapped",
        "overflow",
    ] {
        assert!(seen.get(kind).copied().unwrap_or(0) > 0, "{kind}: {seen:?}");
    }
}

#[test]
fn rip_relative_alu_accesses_near_the_canonical_edge_are_the_same_fault() {
    let base = 0x0000_7fff_ffff_ff00u64;
    let samples: &[(&str, &[u8])] = &[
        (
            "add [rip+0x200], rax",
            &[0x48, 0x01, 0x05, 0x00, 0x02, 0x00, 0x00],
        ),
        (
            "cmp eax, [rip+0x200]",
            &[0x3b, 0x05, 0x00, 0x02, 0x00, 0x00],
        ),
        (
            "or byte [rip+0xf0], 1",
            &[0x80, 0x0d, 0xf0, 0x00, 0x00, 0x00, 0x01],
        ),
        (
            "sub qword [rip-0x100], 3",
            &[0x48, 0x83, 0x2d, 0x00, 0xff, 0xff, 0xff, 0x03],
        ),
    ];
    let mut faults = 0;
    for (label, bytes) in samples {
        let start = start_state([0x40; 16], base, 2);
        let run = compare(label, bytes, &start, INTEGER_FLAGS, false);
        if let End::Fault(fault) = &run.end {
            assert_eq!(fault.vector, 13, "{label}");
            assert!(run.log.calls().is_empty(), "{label}");
            faults += 1;
        } else {
            assert_eq!(run.end, End::Retired, "{label}");
        }
    }
    // The two [rip+0x200] forms leave the canonical range; the byte at 0x7fff_ffff_fff7 and
    // [rip-0x100] stay inside it.
    assert_eq!(faults, 2);
}

#[test]
fn the_cs_ds_es_ss_segment_prefixes_are_taken_for_alu_memory_forms() {
    for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
        for (label, bytes) in ALU_FAULT_SAMPLE {
            let mut prefixed = vec![segment];
            prefixed.extend(*bytes);
            for base in [
                0x0000_0000_0030_0000u64,
                READ_ONLY + 8,
                0x0000_8000_0000_0000,
            ] {
                compare(
                    &format!("{segment:#x} {label}"),
                    &prefixed,
                    &fault_state(base),
                    INTEGER_FLAGS,
                    false,
                );
            }
        }
    }
}

/// (label, bytes) of PUSH and POP of every register: plain, with an empty REX (0x40), REX.B,
/// REX.W, 66 (the word form) and 66 with REX.W.
fn stack_encodings() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for reg in 0u8..16 {
        for push in [true, false] {
            let opcode = if push { 0x50 } else { 0x58 } + (reg & 7);
            let b = u8::from(reg >= 8);
            let name = format!("{} r{reg}", if push { "push" } else { "pop" });
            let mut variants: Vec<(&str, Vec<u8>)> = Vec::new();
            if b == 0 {
                variants.push(("plain", vec![opcode]));
                variants.push(("rex 40", vec![0x40, opcode]));
                variants.push(("66", vec![0x66, opcode]));
            } else {
                variants.push(("rex.b", vec![0x41, opcode]));
                variants.push(("66 rex.b", vec![0x66, 0x41, opcode]));
            }
            variants.push(("rex.w", vec![0x48 | b, opcode]));
            variants.push(("66 rex.w", vec![0x66, 0x48 | b, opcode]));
            for (variant, bytes) in variants {
                out.push((format!("{name} {variant}"), bytes));
            }
        }
    }
    out
}

/// Stack pointers of the stack cases: aligned and unaligned RAM, zero (a push wraps to the top of
/// the canonical high half), 4 (a qword push overflows the address space), the canonical edge from
/// both sides, non-canonical, the top of the address space (a qword pop overflows), page faults
/// (first byte, straddling), a read-only page (a push faults at its write, a pop succeeds), a
/// backend error, the canonical high half.
const STACK_POINTERS: [(&str, u64); 14] = [
    ("ram", 0x0080_0ff8),
    ("ram unaligned", 0x0080_0ffb),
    ("zero", 0),
    ("four", 4),
    ("edge+4", 0x0000_8000_0000_0004),
    ("edge-4", 0x0000_7fff_ffff_fffc),
    ("non-canonical", 0x0000_8000_0000_0000),
    ("top-4", 0xffff_ffff_ffff_fffc),
    ("page fault", PAGE_FAULT + 0x10),
    ("page fault straddle", PAGE_FAULT + WINDOW + 4),
    ("read-only", READ_ONLY + 0x10),
    ("unmapped", UNMAPPED + 0x20),
    ("high", 0xffff_8000_0000_0108),
    ("high unaligned", 0xffff_8000_0000_0103),
];

fn stack_state(rsp: u64, rip: u64) -> LongModeState {
    let mut gpr = [0u64; 16];
    for (index, register) in gpr.iter_mut().enumerate() {
        *register = 0x0101_0101_0101_0101u64.wrapping_mul(index as u64 + 1) ^ 0x8000_0000_0000_f00d;
    }
    gpr[4] = rsp;
    gpr[5] = rsp ^ 0x10;
    start_state(gpr, rip, 2 | ZF)
}

/// Classifies a stack case and checks its journal: a #SS or an overflowing range before any call,
/// one call for a page fault, a backend error or a retired PUSH/POP.
fn stack_end(label: &str, run: &Run, retired_calls: usize) -> &'static str {
    let calls = run.log.calls();
    match &run.end {
        End::Retired => {
            assert_eq!(calls.len(), retired_calls, "{label}");
            "retired"
        }
        End::Fault(fault) if fault.vector == 12 => {
            assert_eq!(fault.error_code, Some(0), "{label}");
            assert!(calls.is_empty(), "{label}");
            "#SS"
        }
        End::Fault(fault) if fault.vector == 13 => {
            assert_eq!(fault.error_code, Some(0), "{label}");
            "#GP"
        }
        End::Fault(fault) if fault.vector == 14 => {
            assert_eq!(calls.len(), 1, "{label}");
            assert!(calls[0].error.is_some(), "{label}");
            "#PF"
        }
        End::Fault(fault) => panic!("{label}: {fault:?}"),
        End::Error(MemoryError::Unmapped { .. }) => {
            assert_eq!(calls.len(), 1, "{label}");
            "unmapped"
        }
        End::Error(MemoryError::AddressOverflow { .. }) => {
            assert!(calls.is_empty(), "{label}");
            "overflow"
        }
        End::Error(error) => panic!("{label}: {error:?}"),
    }
}

#[test]
fn every_push_and_pop_of_a_register_equals_the_general_path() {
    let mut seen = BTreeMap::<String, u64>::new();
    let mut sizes = BTreeSet::new();
    for (label, bytes) in stack_encodings() {
        let instruction = decode_at(CODE, &bytes);
        let Operation::StackRegister { push, .. } = instruction.operation else {
            panic!("{label}: {:?}", instruction.operation);
        };
        sizes.insert(instruction.operand_size.bytes());
        for (case, rsp) in STACK_POINTERS {
            for defined in [INTEGER_FLAGS, INTEGER_FLAGS & !(AF | OF)] {
                let label = format!("{label} rsp {case}");
                let run = compare(&label, &bytes, &stack_state(rsp, CODE), defined, false);
                let end = stack_end(&label, &run, 1);
                assert_ne!(end, "#GP", "{label}");
                if run.end == End::Retired {
                    let method = if push {
                        MemoryMethod::WriteInteger
                    } else {
                        MemoryMethod::ReadInteger
                    };
                    assert_eq!(run.log.calls()[0].method, method, "{label}");
                }
                if case == "read-only" && !push {
                    assert_eq!(end, "retired", "{label}");
                }
                *seen
                    .entry(format!("{} {end}", if push { "push" } else { "pop" }))
                    .or_default() += 1;
            }
        }
    }
    assert_eq!(sizes, BTreeSet::from([2, 8]));
    for kind in [
        "push retired",
        "push #SS",
        "push #PF",
        "push unmapped",
        "push overflow",
        "pop retired",
        "pop #SS",
        "pop #PF",
        "pop unmapped",
        "pop overflow",
    ] {
        assert!(seen.get(kind).copied().unwrap_or(0) > 0, "{kind}: {seen:?}");
    }
}

#[test]
fn push_and_pop_of_rsp_and_rbp_use_the_stack_pointer_as_the_general_path() {
    // PUSH RSP stores the value before the decrement; POP RSP ends with the popped value (not the
    // incremented pointer); the word forms change only the low word.
    let samples: &[(&str, &[u8])] = &[
        ("push rsp", &[0x54]),
        ("pop rsp", &[0x5c]),
        ("push sp", &[0x66, 0x54]),
        ("pop sp", &[0x66, 0x5c]),
        ("push rbp", &[0x55]),
        ("pop rbp", &[0x5d]),
        ("push bp", &[0x66, 0x55]),
        ("pop bp", &[0x66, 0x5d]),
    ];
    for (label, bytes) in samples {
        for rsp in [0x0080_0ff8u64, 0x0080_0ffbu64, 0xffff_8000_0000_0108] {
            let mut ram = Ram::new(CODE, bytes);
            for (offset, byte) in 0x1122_3344_5566_7788u64.to_le_bytes().iter().enumerate() {
                ram.written.insert(rsp + offset as u64, *byte);
            }
            let start = stack_state(rsp, CODE);
            let run = compare_on(label, bytes, &start, INTEGER_FLAGS, ram, false);
            assert_eq!(run.end, End::Retired, "{label}");
            match *label {
                "push rsp" => assert_eq!(run.state.gpr[4], rsp - 8),
                "pop rsp" => assert_eq!(run.state.gpr[4], 0x1122_3344_5566_7788),
                "pop sp" => assert_eq!(run.state.gpr[4], ((rsp + 2) & !0xffff) | 0x7788),
                "pop rbp" => assert_eq!(run.state.gpr[5], 0x1122_3344_5566_7788),
                _ => {}
            }
        }
    }
}

/// CALL rel32 at three code addresses: low RAM (targets in both halves), the top of the lower half
/// (a target past it is #GP before any call) and the bottom of the higher half (a target below it
/// is #GP); every stack pointer of the stack cases.
#[test]
fn call_rel32_equals_the_general_path_with_its_push_and_faults() {
    let mut seen = BTreeMap::<&str, u64>::new();
    let codes: [(u64, &[i32]); 3] = [
        (CODE, &[0, 5, -5, 0x7fff_ffff, i32::MIN, -0x1005]),
        (0x0000_7fff_ffff_ff00, &[0x200, 0, -0x100, 0xf0]),
        (0xffff_8000_0000_0010, &[-0x100, 0, 0x1000]),
    ];
    for (code, displacements) in codes {
        for &displacement in displacements {
            let mut bytes = vec![0xe8];
            bytes.extend(displacement.to_le_bytes());
            let target = (code + 5).wrapping_add_signed(i64::from(displacement));
            let target_canonical = crate::long_decode::canonical(target);
            for (case, rsp) in STACK_POINTERS {
                let label = format!("call {displacement:#x} at {code:#x} rsp {case}");
                let run = compare(&label, &bytes, &stack_state(rsp, code), INTEGER_FLAGS, true);
                let end = stack_end(&label, &run, 1);
                if !target_canonical {
                    // The target is checked before the push: #GP with no memory call.
                    assert_eq!(end, "#GP", "{label}");
                    assert!(run.log.calls().is_empty(), "{label}");
                } else if end == "retired" {
                    assert_eq!(run.state.rip, target, "{label}");
                    assert_eq!(run.state.gpr[4], rsp.wrapping_sub(8), "{label}");
                    let call = &run.log.calls()[0];
                    assert_eq!(call.method, MemoryMethod::WriteInteger, "{label}");
                    assert_eq!(call.address, rsp.wrapping_sub(8), "{label}");
                }
                *seen.entry(end).or_default() += 1;
            }
        }
    }
    for kind in ["retired", "#GP", "#SS", "#PF", "unmapped", "overflow"] {
        assert!(seen.get(kind).copied().unwrap_or(0) > 0, "{kind}: {seen:?}");
    }
}

/// RET and RET imm16 with return addresses placed on the stack: canonical in both halves and
/// non-canonical (#GP after the read, nothing committed); every stack pointer of the stack cases.
#[test]
fn ret_and_ret_imm16_equal_the_general_path_with_their_pop_and_faults() {
    let mut seen = BTreeMap::<&str, u64>::new();
    let forms: [(&str, Vec<u8>); 5] = [
        ("ret", vec![0xc3]),
        ("ret 0", vec![0xc2, 0x00, 0x00]),
        ("ret 8", vec![0xc2, 0x08, 0x00]),
        ("ret 0xfff0", vec![0xc2, 0xf0, 0xff]),
        ("ret 0xffff", vec![0xc2, 0xff, 0xff]),
    ];
    let targets = [
        0x0000_0000_0000_2000u64,
        0xffff_8000_0000_1234,
        0x0000_7fff_ffff_ffff,
        0x0000_8000_0000_0000,
        0x1234_5678_9abc_def0,
    ];
    for (name, bytes) in &forms {
        let instruction = decode_at(CODE, bytes);
        let Operation::Return { stack_adjust } = instruction.operation else {
            panic!("{name}: {:?}", instruction.operation);
        };
        for target in targets {
            for (case, rsp) in STACK_POINTERS {
                let mut ram = Ram::new(CODE, bytes);
                for (offset, byte) in target.to_le_bytes().iter().enumerate() {
                    ram.written.insert(rsp.wrapping_add(offset as u64), *byte);
                }
                let label = format!("{name} to {target:#x} rsp {case}");
                let start = stack_state(rsp, CODE);
                let run = compare_on(&label, bytes, &start, INTEGER_FLAGS, ram, true);
                let end = stack_end(&label, &run, 1);
                match end {
                    "retired" => {
                        assert_eq!(run.state.rip, target, "{label}");
                        assert_eq!(
                            run.state.gpr[4],
                            rsp.wrapping_add(8 + u64::from(stack_adjust)),
                            "{label}"
                        );
                    }
                    "#GP" => {
                        // The slot was read, then the target refused: one successful read.
                        assert!(!crate::long_decode::canonical(target), "{label}");
                        assert_eq!(run.log.calls().len(), 1, "{label}");
                        assert!(run.log.calls()[0].error.is_none(), "{label}");
                    }
                    _ => {}
                }
                *seen.entry(end).or_default() += 1;
            }
        }
    }
    for kind in ["retired", "#GP", "#SS", "#PF", "unmapped", "overflow"] {
        assert!(seen.get(kind).copied().unwrap_or(0) > 0, "{kind}: {seen:?}");
    }
}

/// G5 forms for the refusal checks.
const REFUSAL_SAMPLE: &[&[u8]] = &[
    &[0x48, 0x01, 0x06],             // add [rsi], rax
    &[0x48, 0x03, 0x06],             // add rax, [rsi]
    &[0x48, 0xff, 0x06],             // inc qword [rsi]
    &[0x85, 0x06],                   // test [rsi], eax
    &[0x80, 0x0e, 0x01],             // or byte [rsi], 1
    &[0x50],                         // push rax
    &[0x41, 0x59],                   // pop r9
    &[0x66, 0x55],                   // push bp
    &[0xe8, 0x00, 0x00, 0x00, 0x00], // call +0
    &[0xc3],                         // ret
    &[0xc2, 0x08, 0x00],             // ret 8
];

fn refusal_state() -> LongModeState {
    let mut state = crate::fast_step_g2_tests::state(5, 2 | CF);
    state.gpr[6] = 0x2000;
    state.gpr[4] = 0x0080_0ff8;
    state.gpr[1] = 1;
    state
}

#[test]
fn inadmissible_conditions_are_refused_without_a_memory_call() {
    let ring0 = context(PrivilegeLevel::Ring0);
    for &bytes in REFUSAL_SAMPLE {
        // Control: the plain case is taken, so each refusal below comes from its condition.
        // (`fast_on` panics on "not handled"; how the case ends does not matter here.)
        fast_on(
            Ram::new(CODE, bytes),
            bytes,
            &refusal_state(),
            INTEGER_FLAGS,
        );
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
}

/// CALL and RET are taken only in the form of `executable` with default prefixes (ADR-0017, 2.2):
/// the decoded `2E E8` and `66 C3` and every other prefix, map or opcode byte go to the
/// general path, refused without a memory call.
#[test]
fn call_and_ret_with_prefixes_or_another_opcode_are_refused() {
    let ring0 = context(PrivilegeLevel::Ring0);
    assert_refused(
        "2e e8",
        &[0x2e, 0xe8, 0x00, 0x00, 0x00, 0x00],
        refusal_state(),
        ring0,
        |_, _| {},
    );
    assert_refused("66 c3", &[0x66, 0xc3], refusal_state(), ring0, |_, _| {});
    for bytes in [
        &[0xe8, 0x00, 0x00, 0x00, 0x00][..],
        &[0xc3],
        &[0xc2, 0x08, 0x00],
    ] {
        assert_refused("rex", bytes, refusal_state(), ring0, |_, i| {
            i.prefixes.rex = Some(0x48);
        });
        assert_refused("66", bytes, refusal_state(), ring0, |_, i| {
            i.prefixes.operand_override = true;
        });
        assert_refused("67", bytes, refusal_state(), ring0, |_, i| {
            i.prefixes.address_override = true;
        });
        for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
            assert_refused("segment", bytes, refusal_state(), ring0, |_, i| {
                i.prefixes.segment = Some(segment);
            });
        }
        assert_refused("map", bytes, refusal_state(), ring0, |_, i| {
            i.opcode.map = OpcodeMap::Escape0f;
        });
        assert_refused("opcode byte", bytes, refusal_state(), ring0, |_, i| {
            i.opcode.byte = 0xe9;
        });
    }
}

/// The step takes the G5 forms through the fast path only when the set was built with
/// `with_fast_path`, and the run (retired count, exit, state, memory, the order of the data calls) is
/// the same either way.
#[test]
fn the_step_takes_g5_forms_with_the_flag_and_the_run_is_unchanged() {
    use crate::fast_path_check;
    // 0x00 push rax                 50
    // 0x01 push rbx                 53
    // 0x02 add [rsp], rax           48 01 04 24
    // 0x06 pop rcx                  59
    // 0x07 cmp rcx, [rsp]           48 3b 0c 24
    // 0x0b call 0x12                e8 02 00 00 00
    // 0x10 jmp 0x21                 eb 0f            (after the return)
    // 0x12 add qword [rsp+8], 7     48 83 44 24 08 07
    // 0x18 push r9w                 66 41 51
    // 0x1b pop r9w                  66 41 59
    // 0x1e ret 0                    c2 00 00
    // 0x21 pop rdx                  5a
    // 0x22 pause                    f3 90 (outside the fast path; a NOP is G6 since PERF-33, E4)
    let program = [
        0x50, 0x53, 0x48, 0x01, 0x04, 0x24, 0x59, 0x48, 0x3b, 0x0c, 0x24, 0xe8, 0x02, 0x00, 0x00,
        0x00, 0xeb, 0x0f, 0x48, 0x83, 0x44, 0x24, 0x08, 0x07, 0x66, 0x41, 0x51, 0x66, 0x41, 0x59,
        0xc2, 0x00, 0x00, 0x5a, 0xf3, 0x90,
    ];
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let run_set = |set: &Cpl0ProfileSet<'_>, seed: u64| {
        let mut gpr = [0u64; 16];
        for (index, register) in gpr.iter_mut().enumerate() {
            *register = 0x1234_5678_9abc_def0 ^ (seed << 8) ^ index as u64;
        }
        gpr[4] = 0x0080_0000 + seed * 0x43;
        let mut state = start_state(gpr, CODE, 0x202);
        let mut ram = Ram::new(CODE, &program);
        let mut recording = CheckMemory::record(&mut ram, MemoryLog::default());
        fast_path_check::reset_fast_steps_taken();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut recording,
            RunConfig {
                instruction_budget: 64,
                stop_rip: Some(CODE + program.len() as u64),
                context: context(PrivilegeLevel::Ring0),
            },
            INTEGER_FLAGS,
            set,
            None,
            || false,
        )
        .unwrap();
        let log = recording.into_log();
        let taken = fast_path_check::fast_steps_taken();
        (result, state, ram, log, taken)
    };
    for seed in 0..6 {
        let (off, off_state, off_ram, off_log, taken_off) = run_set(&plain, seed);
        let (on, on_state, on_ram, on_log, taken_on) = run_set(&plain.with_fast_path(), seed);
        assert_eq!(off.exit, Cpl0ProfileSetExit::Run(RunExit::StopAddress));
        assert_eq!(off.instructions_executed, 13);
        assert_eq!(on, off, "seed {seed}");
        assert_eq!(on_state, off_state, "seed {seed}");
        assert_eq!(on_ram.written, off_ram.written, "seed {seed}");
        assert_same_calls(&format!("seed {seed}"), &off_log, &on_log);
        // The stack is balanced again: RSP back where it started, RDX holds the pushed RAX plus 7.
        assert_eq!(off_state.gpr[4], 0x0080_0000 + seed * 0x43);
        let initial_rax = 0x1234_5678_9abc_def0 ^ (seed << 8);
        assert_eq!(off_state.gpr[2], initial_rax.wrapping_add(7), "seed {seed}");
        use MemoryMethod::{ReadInteger as R, WriteInteger as W};
        let methods: Vec<MemoryMethod> = off_log.calls().iter().map(|call| call.method).collect();
        assert_eq!(methods, [W, W, R, W, R, R, W, R, W, W, R, R, R]);
        assert_eq!(taken_off, 0);
        if fast_path_check::FAST_STEPS_COUNTED {
            // Everything but the final PAUSE.
            assert_eq!(taken_on, 12, "seed {seed}");
        }
    }
}
