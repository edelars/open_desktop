//! PERF-24b, group G4 (ADR-0017, section 1): the moves with memory in `execute::try_fast_step` —
//! `Mov`/`MovImmediate` with a memory operand, `Extend` (MOVZX/MOVSX/MOVSXD, both forms) and
//! `Lea`. Each case runs the general path (one instruction through `run` of the CPL0 profile set)
//! and the fast path (called directly on a copy of the state), both over a recording
//! [`CheckMemory`] around a fresh copy of the same memory, and compares the CPU state, `rflags_defined`,
//! the outcome (retired, the same fault, the same memory error), the memory left behind and the
//! journal of the data calls of the PERF-22 check (method, address, context, width, value, error,
//! in order). Page faults, backend errors, non-canonical addresses and overflowing ranges must end
//! the instruction exactly as the general path does; inadmissible conditions are refused without a
//! single memory call.

use std::collections::{BTreeMap, BTreeSet};

use crate::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, StepOutcome,
    run_with_cpl0_profile_set, try_fast_step,
};
use crate::fast_path_check::{CheckMemory, MemoryLog, MemoryMethod};
use crate::fast_step_g2_tests::{assert_refused, context};
use crate::long_decode::{self, Instruction, Operand, Operation};
use crate::memory::AccessKind;
use crate::{
    AccessContext, ArchitecturalFault, GuestMemory, INTEGER_FLAGS, LongModeState, MemoryError,
    PrivilegeLevel, StiShadow,
};

const CODE: u64 = 0x1000;
/// Reads and writes of any byte in this window are a page fault (vector 14).
const PAGE_FAULT: u64 = 0x0000_0040_0000_0000;
/// Reads and writes of any byte in this window are a backend error (not a guest fault).
const UNMAPPED: u64 = 0x0000_0050_0000_0000;
const WINDOW: u64 = 0x1000;
const CF: u64 = 1;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const AF: u64 = 1 << 4;
const TF: u64 = 1 << 8;
const RF: u64 = 1 << 16;

/// Code at `code_base` for the fetch; data is a sparse RAM over every other address (an unwritten
/// byte has a value derived from its address), except the page-fault and unmapped windows.
#[derive(Clone)]
struct Ram {
    code_base: u64,
    code: Vec<u8>,
    written: BTreeMap<u64, u8>,
}

impl Ram {
    fn new(code_base: u64, code: &[u8]) -> Self {
        Self {
            code_base,
            code: code.to_vec(),
            written: BTreeMap::new(),
        }
    }

    fn byte(&self, address: u64) -> u8 {
        self.written.get(&address).copied().unwrap_or_else(|| {
            let mixed = address.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            (mixed >> 56) as u8 ^ address as u8
        })
    }

    /// The first byte of `[address, address + len)` in `window`, if any.
    fn hit(window: u64, address: u64, len: usize) -> Option<u64> {
        (0..len as u64)
            .map(|offset| address.wrapping_add(offset))
            .find(|byte| (window..window + WINDOW).contains(byte))
    }

    fn check(address: u64, len: usize, access: AccessKind) -> Result<(), MemoryError> {
        if let Some(byte) = Self::hit(PAGE_FAULT, address, len) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 2 } else { 0 }),
                linear_address: Some(byte),
            }));
        }
        if Self::hit(UNMAPPED, address, len).is_some() {
            return Err(MemoryError::Unmapped { address, access });
        }
        Ok(())
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.code_base)
            .and_then(|offset| self.code.get(offset as usize))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        Self::check(address, destination.len(), AccessKind::Read)?;
        for (offset, byte) in destination.iter_mut().enumerate() {
            *byte = self.byte(address.wrapping_add(offset as u64));
        }
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        Self::check(address, source.len(), AccessKind::Write)?;
        for (offset, byte) in source.iter().enumerate() {
            self.written
                .insert(address.wrapping_add(offset as u64), *byte);
        }
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// How one instruction ended.
#[derive(Debug, PartialEq)]
enum End {
    Retired,
    Fault(ArchitecturalFault),
    Error(MemoryError),
}

/// Everything one path left: state, definedness, end, memory, the data calls.
struct Run {
    state: LongModeState,
    defined: u64,
    end: End,
    ram: Ram,
    log: MemoryLog,
}

fn start_state(gpr: [u64; 16], rip: u64, rflags: u64) -> LongModeState {
    LongModeState {
        rip,
        rflags,
        gpr,
        cr0: Some(0x8001_0033),
        cr4: Some(0x20),
        activity: Some(crate::CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

fn general(bytes: &[u8], start: &LongModeState, defined: u64) -> Run {
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = start.clone();
    let mut ram = Ram::new(start.rip, bytes);
    let mut recording = CheckMemory::record(&mut ram, MemoryLog::default());
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut recording,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(PrivilegeLevel::Ring0),
        },
        defined,
        &plain,
        None,
        || false,
    )
    .unwrap();
    let log = recording.into_log();
    let end = match result.exit {
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted) => {
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            End::Retired
        }
        Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) => End::Fault(fault),
        Cpl0ProfileSetExit::Run(RunExit::MemoryError(error)) => End::Error(error),
        other => panic!("{bytes:02x?}: {other:?}"),
    };
    Run {
        state,
        defined: result.rflags_defined,
        end,
        ram,
        log,
    }
}

fn decode_at(base: u64, bytes: &[u8]) -> Instruction {
    let mut ram = Ram::new(base, bytes);
    let instruction = long_decode::decode(&mut ram, base, context(PrivilegeLevel::Ring0))
        .unwrap_or_else(|error| panic!("{bytes:02x?}: {error:?}"));
    assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
    instruction
}

/// The decode of a generated encoding; `None` for a form the decoder does not support (the general
/// path stops there as `Unsupported` and the fast path is never asked).
fn try_decode_at(base: u64, bytes: &[u8]) -> Option<Instruction> {
    let mut ram = Ram::new(base, bytes);
    match long_decode::decode(&mut ram, base, context(PrivilegeLevel::Ring0)) {
        Ok(instruction) => {
            assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
            Some(instruction)
        }
        Err(long_decode::DecodeError::Unsupported { .. }) => None,
        Err(error) => panic!("{bytes:02x?}: {error:?}"),
    }
}

/// The fast path on a copy of `start`; it must take the instruction.
fn fast(bytes: &[u8], start: &LongModeState, defined: u64) -> Run {
    let instruction = decode_at(start.rip, bytes);
    let mut state = start.clone();
    let mut defined = defined;
    let mut ram = Ram::new(start.rip, bytes);
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
    let end = match outcome {
        Some(Ok(StepOutcome::Advanced)) => End::Retired,
        Some(Ok(StepOutcome::Fault(fault))) => End::Fault(fault),
        Some(Err(error)) => End::Error(error),
        other => panic!("{bytes:02x?}: not taken or another outcome: {other:?}"),
    };
    Run {
        state,
        defined,
        end,
        ram,
        log,
    }
}

/// The data calls of two journals are the same calls in the same order (fetches are not compared:
/// the decode is common to both paths).
fn assert_same_calls(label: &str, general: &MemoryLog, fast: &MemoryLog) {
    assert_eq!(
        general.calls().len(),
        fast.calls().len(),
        "{label}: general {:?} | fast {:?}",
        general.calls(),
        fast.calls()
    );
    for (index, (a, b)) in general.calls().iter().zip(fast.calls()).enumerate() {
        assert!(
            a.method == b.method
                && a.address == b.address
                && a.context == b.context
                && a.size == b.size
                && a.values == b.values
                && a.error == b.error
                && general.data_of(a) == fast.data_of(b),
            "{label}: call {index}: general {} | fast {}",
            general.describe(index),
            fast.describe(index)
        );
    }
}

/// Runs both paths and compares everything; returns the general run.
fn compare(label: &str, bytes: &[u8], start: &LongModeState, defined: u64) -> Run {
    let expected = general(bytes, start, defined);
    let got = fast(bytes, start, defined);
    assert_eq!(got.end, expected.end, "{label}: {bytes:02x?}");
    assert_eq!(got.state, expected.state, "{label}: {bytes:02x?}");
    assert_eq!(got.defined, expected.defined, "{label}: {bytes:02x?}");
    assert_eq!(
        got.ram.written, expected.ram.written,
        "{label}: {bytes:02x?}"
    );
    assert_same_calls(label, &expected.log, &got.log);
    if expected.end != End::Retired {
        // A fault or an error changes nothing in the CPU state (RIP and definedness included).
        assert_eq!(expected.state, *start, "{label}: {bytes:02x?}");
        assert_eq!(expected.defined, defined, "{label}: {bytes:02x?}");
        assert!(expected.ram.written.is_empty(), "{label}: {bytes:02x?}");
    }
    expected
}

/// One r/m memory operand: ModRM `mod` and `rm` low bits, REX.X/REX.B, the SIB and displacement
/// bytes, and whether a 0x67 prefix selects 32-bit addressing.
#[derive(Clone)]
struct Address {
    label: String,
    mode: u8,
    rm: u8,
    rex_x: bool,
    rex_b: bool,
    tail: Vec<u8>,
    addr32: bool,
}

fn address(label: String, mode: u8, rm: u8, rex_x: bool, rex_b: bool, tail: Vec<u8>) -> Address {
    Address {
        label,
        mode,
        rm,
        rex_x,
        rex_b,
        tail,
        addr32: false,
    }
}

/// Every ModRM memory shape: base registers (all sixteen where the encoding allows), SIB with and
/// without base and index and every scale, disp8 and disp32, RIP-relative, and 32-bit addressing.
fn address_forms() -> Vec<Address> {
    let mut out = Vec::new();
    // [base], mod 00 (rm 100 is the SIB escape, rm 101 is RIP-relative).
    for base in 0u8..16 {
        if base & 7 != 4 && base & 7 != 5 {
            out.push(address(
                format!("[r{base}]"),
                0,
                base & 7,
                false,
                base >= 8,
                vec![],
            ));
        }
    }
    // [rip + disp32].
    for disp in [0i32, 0x40, -0x40, 0x7fff_0000] {
        out.push(address(
            format!("[rip{disp:+#x}]"),
            0,
            5,
            false,
            false,
            disp.to_le_bytes().to_vec(),
        ));
    }
    // SIB, mod 00: base 101 means no base and a disp32; index 100 without REX.X means no index.
    for base in [0u8, 3, 4, 5, 12, 13] {
        for index in [4u8, 1, 9, 12, 5] {
            for scale in 0u8..4 {
                let sib = (scale << 6) | ((index & 7) << 3) | (base & 7);
                let mut tail = vec![sib];
                if base & 7 == 5 {
                    tail.extend(0x2000i32.to_le_bytes());
                }
                out.push(address(
                    format!("sib mod0 base {base} index {index} scale {scale}"),
                    0,
                    4,
                    index >= 8,
                    base >= 8,
                    tail,
                ));
            }
        }
    }
    // [base + disp8], every base (rm 100: SIB with base RSP/R12 and no index).
    for rm in 0u8..16 {
        for disp in [0i8, 8, 0x7f, -0x80] {
            let mut tail = Vec::new();
            if rm & 7 == 4 {
                tail.push(0x24);
            }
            tail.push(disp as u8);
            out.push(address(
                format!("[r{rm}{disp:+}] disp8"),
                1,
                rm & 7,
                false,
                rm >= 8,
                tail,
            ));
        }
    }
    // [base + disp32].
    for rm in [0u8, 3, 4, 5, 6, 12, 13, 15] {
        for disp in [0x1000i32, -0x1000, 0x7fff_ffff, i32::MIN] {
            let mut tail = Vec::new();
            if rm & 7 == 4 {
                tail.push(0x24);
            }
            tail.extend(disp.to_le_bytes());
            out.push(address(
                format!("[r{rm}{disp:+#x}] disp32"),
                2,
                rm & 7,
                false,
                rm >= 8,
                tail,
            ));
        }
    }
    // SIB with a displacement.
    for (mode, base, index, scale) in [
        (1u8, 0u8, 1u8, 3u8),
        (1, 5, 6, 2),
        (1, 13, 14, 1),
        (1, 4, 9, 0),
        (2, 7, 2, 3),
        (2, 12, 15, 2),
        (2, 5, 4, 0),
    ] {
        let mut tail = vec![(scale << 6) | ((index & 7) << 3) | (base & 7)];
        if mode == 1 {
            tail.push(0xf0);
        } else {
            tail.extend((-0x3456i32).to_le_bytes());
        }
        out.push(address(
            format!("sib mod{mode} base {base} index {index} scale {scale}"),
            mode,
            4,
            index >= 8,
            base >= 8,
            tail,
        ));
    }
    // 32-bit addressing (0x67): the effective address is truncated to 32 bits.
    let mut addr32 = vec![
        address("[eax]".into(), 0, 0, false, false, vec![]),
        address("[r9d]".into(), 0, 1, false, true, vec![]),
        address("[ebx+0x7f]".into(), 1, 3, false, false, vec![0x7f]),
        address(
            "[eip+0x40]".into(),
            0,
            5,
            false,
            false,
            0x40i32.to_le_bytes().to_vec(),
        ),
        address("[esp]".into(), 0, 4, false, false, vec![0x24]),
        address(
            "[eax+ecx*4-0x10]".into(),
            1,
            4,
            false,
            false,
            vec![0x88, 0xf0],
        ),
        address("[disp32]".into(), 0, 4, false, false, {
            let mut tail = vec![0x25];
            tail.extend(0x0040_0000i32.to_le_bytes());
            tail
        }),
    ];
    for form in &mut addr32 {
        form.label = format!("addr32 {}", form.label);
        form.addr32 = true;
    }
    out.extend(addr32);
    out
}

/// An instruction `legacy` prefixes, REX (W, R from `reg`, X/B from the form; forced when asked),
/// the opcode, ModRM, the SIB and displacement, the immediate.
fn encode(
    legacy: &[u8],
    wide: bool,
    force_rex: bool,
    opcode: &[u8],
    reg: u8,
    form: &Address,
    immediate: &[u8],
) -> Vec<u8> {
    let mut bytes = legacy.to_vec();
    if form.addr32 {
        bytes.push(0x67);
    }
    let rex = (u8::from(wide) << 3)
        | (u8::from(reg >= 8) << 2)
        | (u8::from(form.rex_x) << 1)
        | u8::from(form.rex_b);
    if rex != 0 || force_rex {
        bytes.push(0x40 | rex);
    }
    bytes.extend(opcode);
    bytes.push((form.mode << 6) | ((reg & 7) << 3) | form.rm);
    bytes.extend(&form.tail);
    bytes.extend(immediate);
    bytes
}

/// (label, bytes) of every G4 operation over `form`: MOV load and store of each width (with
/// high-byte and extended registers), MOV m, imm of each width, MOVZX/MOVSX of a byte and a word to
/// each destination width, MOVSXD, LEA of each width.
fn g4_encodings(form: &Address) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let f = &form.label;
    // (label, legacy prefixes, REX.W)
    let widths: [(&str, &[u8], bool); 3] = [
        ("16", &[0x66], false),
        ("32", &[], false),
        ("64", &[], true),
    ];
    for reg in [0u8, 3, 4, 7, 9, 15] {
        // Byte registers 4..=7 are AH..BH without REX and SPL..DIL with one.
        for force_rex in [false, true] {
            if force_rex && !(4..8).contains(&reg) {
                continue;
            }
            let tag = format!("reg {reg} rex {force_rex} {f}");
            out.push((
                format!("mov m8, r8 {tag}"),
                encode(&[], false, force_rex, &[0x88], reg, form, &[]),
            ));
            out.push((
                format!("mov r8, m8 {tag}"),
                encode(&[], false, force_rex, &[0x8a], reg, form, &[]),
            ));
        }
        for (width, legacy, wide) in widths {
            let tag = format!("reg {reg} {f}");
            out.push((
                format!("mov m{width}, r {tag}"),
                encode(legacy, wide, false, &[0x89], reg, form, &[]),
            ));
            out.push((
                format!("mov r{width}, m {tag}"),
                encode(legacy, wide, false, &[0x8b], reg, form, &[]),
            ));
            for (op, label) in [
                (&[0x0f, 0xb6][..], "movzx m8"),
                (&[0x0f, 0xb7][..], "movzx m16"),
                (&[0x0f, 0xbe][..], "movsx m8"),
                (&[0x0f, 0xbf][..], "movsx m16"),
            ] {
                out.push((
                    format!("{label} -> r{width} {tag}"),
                    encode(legacy, wide, false, op, reg, form, &[]),
                ));
            }
            out.push((
                format!("lea r{width} {tag}"),
                encode(legacy, wide, false, &[0x8d], reg, form, &[]),
            ));
        }
        out.push((
            format!("movsxd r64, m32 reg {reg} {f}"),
            encode(&[], true, false, &[0x63], reg, form, &[]),
        ));
    }
    // MOV m, imm (C6 /0, C7 /0): the immediate follows the displacement.
    out.push((
        format!("mov m8, imm {f}"),
        encode(&[], false, false, &[0xc6], 0, form, &[0x9c]),
    ));
    out.push((
        format!("mov m16, imm {f}"),
        encode(&[0x66], false, false, &[0xc7], 0, form, &[0x34, 0x92]),
    ));
    out.push((
        format!("mov m32, imm {f}"),
        encode(
            &[],
            false,
            false,
            &[0xc7],
            0,
            form,
            &[0x78, 0x56, 0x34, 0x92],
        ),
    ));
    out.push((
        format!("mov m64, simm32 {f}"),
        encode(
            &[],
            true,
            false,
            &[0xc7],
            0,
            form,
            &[0x78, 0x56, 0x34, 0x92],
        ),
    ));
    out
}

/// MOV between the accumulator and an absolute 64-bit address (A0..A3), each width.
fn moffs_encodings(address: u64) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (width, legacy, wide) in [
        ("8", &[][..], false),
        ("16", &[0x66][..], false),
        ("32", &[][..], false),
        ("64", &[][..], true),
    ] {
        for opcode in [0xa0u8, 0xa2] {
            let opcode = opcode + u8::from(width != "8");
            let mut bytes = legacy.to_vec();
            if wide {
                bytes.push(0x48);
            }
            bytes.push(opcode);
            bytes.extend(address.to_le_bytes());
            out.push((
                format!("mov moffs {opcode:#x} w{width} {address:#x}"),
                bytes,
            ));
        }
    }
    out
}

/// Register sets: low RAM (unaligned), the canonical high half, values near the top of the lower
/// half (an access may leave the canonical range), and mixed values that are mostly non-canonical.
fn register_sets() -> Vec<(&'static str, [u64; 16])> {
    let mut low = [0u64; 16];
    let mut high = [0u64; 16];
    let mut edge = [0u64; 16];
    let mut mixed = [0u64; 16];
    for index in 0..16 {
        let i = index as u64;
        low[index] = 0x0010_0000 + i * 0x1_0000 + i;
        high[index] = 0xffff_8000_0010_0000 + i * 0x1000 + 3 * i;
        edge[index] = 0x0000_7fff_ffff_fff0 + (i & 7);
        mixed[index] = 0x9e37_79b9_7f4a_7c15u64.wrapping_mul(i + 1) ^ (i << 40);
    }
    // Small indexes in the low set keep scaled sums inside low RAM; RSP is a plausible stack.
    low[4] = 0x0080_0ff8;
    vec![
        ("low", low),
        ("high", high),
        ("edge", edge),
        ("mixed", mixed),
    ]
}

/// The decoded operation is one of G4 (checks the generator).
fn g4_kind(instruction: &Instruction) -> &'static str {
    match instruction.operation {
        Operation::Mov {
            dst: Operand::Memory(_),
            ..
        } => "mov store",
        Operation::Mov {
            src: Operand::Memory(_),
            ..
        } => "mov load",
        Operation::MovImmediate {
            dst: Operand::Memory(_),
            ..
        } => "mov imm store",
        Operation::Extend {
            src: Operand::Memory(_),
            ..
        } => "extend memory",
        Operation::Extend {
            src: Operand::Register(_),
            ..
        } => "extend register",
        Operation::Lea { .. } => "lea",
        other => panic!("{other:?} is not a G4 memory form"),
    }
}

#[test]
fn every_g4_form_equals_the_general_path_with_the_same_memory_calls() {
    let forms = address_forms();
    let sets = register_sets();
    let mut kinds = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    let mut ends = BTreeMap::<String, u64>::new();
    let mut calls = 0u64;
    let mut cases = 0u64;
    let mut encodings: Vec<(String, Vec<u8>)> = forms.iter().flat_map(g4_encodings).collect();
    for address in [
        0x0000_0000_0020_0000u64,
        0xffff_8000_0000_1000,
        0x0000_8000_0000_0000,
    ] {
        encodings.extend(moffs_encodings(address));
    }
    let mut skipped = BTreeSet::new();
    for (label, bytes) in &encodings {
        let Some(instruction) = try_decode_at(CODE, bytes) else {
            skipped.insert(label.split(" reg ").next().unwrap_or(label).to_owned());
            continue;
        };
        kinds.insert(g4_kind(&instruction));
        sizes.insert(instruction.operand_size.bytes());
        for (set, gpr) in &sets {
            let start = start_state(*gpr, CODE, 2 | CF | ZF);
            for defined in [INTEGER_FLAGS, INTEGER_FLAGS & !(AF | OF)] {
                let run = compare(&format!("{label} regs {set}"), bytes, &start, defined);
                let end = match &run.end {
                    End::Retired => "retired".to_owned(),
                    End::Fault(fault) => format!("fault {}", fault.vector),
                    End::Error(error) => format!("error {error:?}").chars().take(20).collect(),
                };
                *ends.entry(end).or_default() += 1;
                calls += run.log.calls().len() as u64;
                // LEA and the register extension touch no memory; a load or store one call at most.
                let expected_max = match g4_kind(&instruction) {
                    "lea" | "extend register" => 0,
                    _ => 1,
                };
                assert!(run.log.calls().len() <= expected_max, "{label}");
                cases += 1;
            }
        }
    }
    // The decoder has no word-to-word MOVZX/MOVSX: those encodings are the only ones refused.
    assert_eq!(
        skipped,
        BTreeSet::from(["movsx m16 -> r16".to_owned(), "movzx m16 -> r16".to_owned()])
    );
    assert_eq!(
        kinds,
        BTreeSet::from([
            "extend memory",
            "lea",
            "mov imm store",
            "mov load",
            "mov store"
        ])
    );
    assert_eq!(sizes, BTreeSet::from([1, 2, 4, 8]));
    assert!(ends["retired"] > 10_000, "{ends:?}");
    assert!(
        ends.get("fault 13").copied().unwrap_or(0) > 1_000,
        "{ends:?}"
    );
    assert!(ends.get("fault 12").copied().unwrap_or(0) > 100, "{ends:?}");
    assert!(calls > 10_000, "{calls}");
    assert!(cases > 30_000, "{cases}");
}

/// The register forms of MOVZX/MOVSX/MOVSXD (`Extend` with a register source) of every width.
#[test]
fn every_register_extension_equals_the_general_path() {
    let sets = register_sets();
    let mut cases = 0;
    let mut skipped = BTreeSet::new();
    for (width, legacy, wide) in [
        ("16", &[0x66][..], false),
        ("32", &[][..], false),
        ("64", &[][..], true),
    ] {
        for (op, name) in [
            (&[0x0f, 0xb6][..], "movzx r8"),
            (&[0x0f, 0xb7][..], "movzx r16"),
            (&[0x0f, 0xbe][..], "movsx r8"),
            (&[0x0f, 0xbf][..], "movsx r16"),
            (&[0x63][..], "movsxd r32"),
        ] {
            if op == [0x63] && !wide {
                continue;
            }
            for (dst, src) in [(0u8, 1u8), (3, 4), (7, 6), (9, 12), (15, 5)] {
                for force_rex in [false, true] {
                    let mut bytes = legacy.to_vec();
                    let rex =
                        (u8::from(wide) << 3) | (u8::from(dst >= 8) << 2) | u8::from(src >= 8);
                    if rex != 0 || force_rex {
                        bytes.push(0x40 | rex);
                    }
                    bytes.extend(op);
                    bytes.push(0xc0 | ((dst & 7) << 3) | (src & 7));
                    let label = format!("{name} -> r{width} {dst} {src} rex {force_rex}");
                    let Some(instruction) = try_decode_at(CODE, &bytes) else {
                        skipped.insert(format!("{name} -> r{width}"));
                        continue;
                    };
                    assert_eq!(g4_kind(&instruction), "extend register", "{label}");
                    for (set, gpr) in &sets {
                        let start = start_state(*gpr, CODE, 2 | SF);
                        let run = compare(&format!("{label} {set}"), &bytes, &start, INTEGER_FLAGS);
                        assert_eq!(run.end, End::Retired, "{label}");
                        assert!(run.log.calls().is_empty(), "{label}");
                        cases += 1;
                    }
                }
            }
        }
    }
    // As for the memory forms: no word-to-word MOVZX/MOVSX in the decoder.
    assert_eq!(
        skipped,
        BTreeSet::from(["movsx r16 -> r16".to_owned(), "movzx r16 -> r16".to_owned()])
    );
    assert!(cases > 300, "{cases}");
}

/// A sample of loads, stores and extensions over [rsi], [rsp] and [rbp+disp], with RSI, RSP and
/// RBP placed by each case.
const FAULT_SAMPLE: &[(&str, &[u8])] = &[
    ("mov rax, [rsi]", &[0x48, 0x8b, 0x06]),
    ("mov [rsi], rax", &[0x48, 0x89, 0x06]),
    ("mov al, [rsi]", &[0x8a, 0x06]),
    ("mov [rsi], ax", &[0x66, 0x89, 0x06]),
    (
        "mov dword [rsi], imm",
        &[0xc7, 0x06, 0x01, 0x02, 0x03, 0x04],
    ),
    ("mov byte [rsi], imm", &[0xc6, 0x06, 0x05]),
    ("movzx eax, byte [rsi]", &[0x0f, 0xb6, 0x06]),
    ("movsx rax, word [rsi]", &[0x48, 0x0f, 0xbf, 0x06]),
    ("movsxd rax, [rsi]", &[0x48, 0x63, 0x06]),
    ("mov rax, [rsi+rcx*8+8]", &[0x48, 0x8b, 0x44, 0xce, 0x08]),
    ("mov rax, [rsp]", &[0x48, 0x8b, 0x04, 0x24]),
    ("mov [rsp+8], rax", &[0x48, 0x89, 0x44, 0x24, 0x08]),
    ("mov eax, [rbp-8]", &[0x8b, 0x45, 0xf8]),
    ("mov [rbp-8], ecx", &[0x89, 0x4d, 0xf8]),
];

fn fault_state(rsi_rsp_rbp: u64) -> LongModeState {
    let mut gpr = [0u64; 16];
    gpr[0] = 0x1122_3344_5566_7788;
    gpr[1] = 0;
    gpr[4] = rsi_rsp_rbp;
    gpr[5] = rsi_rsp_rbp.wrapping_add(8);
    gpr[6] = rsi_rsp_rbp;
    start_state(gpr, CODE, 2 | CF)
}

#[test]
fn memory_faults_and_errors_end_the_instruction_as_the_general_path_does() {
    let mut seen = BTreeMap::<&str, u64>::new();
    for (label, bytes) in FAULT_SAMPLE {
        decode_at(CODE, bytes);
        let stack_based = matches!(
            label.split(['[', '+', '-', ']']).nth(1),
            Some("rsp" | "rbp")
        );
        let cases: [(&str, u64); 8] = [
            // Page fault in the first byte; a qword straddling into the window from below.
            ("page fault", PAGE_FAULT + 0x10),
            ("page fault straddle", PAGE_FAULT - 4),
            // A backend error, not a guest fault: returned as an error.
            ("unmapped", UNMAPPED + 0x20),
            // Non-canonical start, and a canonical start whose last byte is not canonical.
            ("non-canonical", 0x0000_8000_0000_0000),
            ("non-canonical high", 0xfff0_0000_0000_0000),
            ("canonical edge", 0x0000_7fff_ffff_fffe),
            // A range past the end of the address space.
            ("overflow", 0xffff_ffff_ffff_fffe),
            ("ram", 0x0000_0000_0030_0000),
        ];
        for (case, base) in cases {
            let start = fault_state(base);
            for defined in [INTEGER_FLAGS, INTEGER_FLAGS & !(SF | OF)] {
                let run = compare(&format!("{label} {case}"), bytes, &start, defined);
                let kind = match &run.end {
                    End::Retired => "retired",
                    End::Fault(fault) if fault.vector == 14 => {
                        assert_eq!(run.log.calls().len(), 1, "{label} {case}");
                        assert!(run.log.calls()[0].error.is_some(), "{label} {case}");
                        "#PF"
                    }
                    End::Fault(fault) if fault.vector == 13 => {
                        assert!(!stack_based, "{label} {case}");
                        assert_eq!(fault.error_code, Some(0), "{label} {case}");
                        assert!(
                            run.log.calls().is_empty(),
                            "{label} {case}: {:?}",
                            run.log.calls()
                        );
                        "#GP"
                    }
                    End::Fault(fault) if fault.vector == 12 => {
                        assert!(stack_based, "{label} {case}");
                        assert!(run.log.calls().is_empty(), "{label} {case}");
                        "#SS"
                    }
                    End::Fault(fault) => panic!("{label} {case}: {fault:?}"),
                    End::Error(MemoryError::Unmapped { .. }) => {
                        assert_eq!(run.log.calls().len(), 1, "{label} {case}");
                        "unmapped"
                    }
                    End::Error(MemoryError::AddressOverflow { .. }) => {
                        assert!(run.log.calls().is_empty(), "{label} {case}");
                        "overflow"
                    }
                    End::Error(error) => panic!("{label} {case}: {error:?}"),
                };
                *seen.entry(kind).or_default() += 1;
            }
        }
    }
    for kind in ["retired", "#PF", "#GP", "#SS", "unmapped", "overflow"] {
        assert!(seen.get(kind).copied().unwrap_or(0) > 0, "{kind}: {seen:?}");
    }
}

#[test]
fn lea_never_faults_and_never_touches_memory() {
    // Effective addresses that a load would fault on: LEA writes them as they are.
    let samples: &[(&str, &[u8])] = &[
        ("lea rax, [rsi]", &[0x48, 0x8d, 0x06]),
        ("lea eax, [rsi+rcx*8+8]", &[0x8d, 0x44, 0xce, 0x08]),
        ("lea ax, [rsp+8]", &[0x66, 0x8d, 0x44, 0x24, 0x08]),
        (
            "lea rdx, [rip+0x7fff0000]",
            &[0x48, 0x8d, 0x15, 0x00, 0x00, 0xff, 0x7f],
        ),
        ("lea r9d, [eax+ecx*4]", &[0x67, 0x44, 0x8d, 0x0c, 0x88]),
    ];
    for (label, bytes) in samples {
        for base in [
            0x0000_8000_0000_0000u64,
            PAGE_FAULT,
            UNMAPPED,
            0xffff_ffff_ffff_fffe,
        ] {
            let mut start = fault_state(base);
            start.gpr[1] = 0x7fff_ffff_ffff_ffff;
            let run = compare(&format!("{label} {base:#x}"), bytes, &start, INTEGER_FLAGS);
            assert_eq!(run.end, End::Retired, "{label}");
            assert!(run.log.calls().is_empty(), "{label}");
        }
    }
}

#[test]
fn rip_relative_accesses_near_the_canonical_edge_are_the_same_fault() {
    // The code lies at the top of the lower half: RIP-relative targets leave the canonical range.
    let base = 0x0000_7fff_ffff_ff00u64;
    let samples: &[(&str, &[u8])] = &[
        (
            "mov rax, [rip+0x200]",
            &[0x48, 0x8b, 0x05, 0x00, 0x02, 0x00, 0x00],
        ),
        (
            "mov [rip+0x200], eax",
            &[0x89, 0x05, 0x00, 0x02, 0x00, 0x00],
        ),
        (
            "movzx eax, byte [rip+0xf0]",
            &[0x0f, 0xb6, 0x05, 0xf0, 0x00, 0x00, 0x00],
        ),
        (
            "mov qword [rip+0xf0], imm",
            &[0x48, 0xc7, 0x05, 0xf0, 0x00, 0x00, 0x00, 1, 0, 0, 0],
        ),
        (
            "mov rax, [rip-0x100]",
            &[0x48, 0x8b, 0x05, 0x00, 0xff, 0xff, 0xff],
        ),
    ];
    let mut faults = 0;
    for (label, bytes) in samples {
        let start = start_state([0x40; 16], base, 2);
        let run = compare(label, bytes, &start, INTEGER_FLAGS);
        if let End::Fault(fault) = &run.end {
            assert_eq!(fault.vector, 13, "{label}");
            assert!(run.log.calls().is_empty(), "{label}");
            faults += 1;
        } else {
            assert_eq!(run.end, End::Retired, "{label}");
            assert_eq!(run.log.calls().len(), 1, "{label}");
        }
    }
    // Five samples: the byte at 0x7fff_ffff_fff7 is still canonical, so that MOVZX retires with one
    // read, and [rip-0x100] lies below the code; the other three leave the canonical range.
    // Everything but the MOVZX at the edge and [rip-0x100].
    assert_eq!(faults, 3);
}

#[test]
fn the_cs_ds_es_ss_segment_prefixes_are_taken_and_equal_the_general_path() {
    for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
        for (label, bytes) in FAULT_SAMPLE {
            let mut prefixed = vec![segment];
            prefixed.extend(*bytes);
            for base in [
                0x0000_0000_0030_0000u64,
                PAGE_FAULT + 8,
                0x0000_8000_0000_0000,
            ] {
                compare(
                    &format!("{segment:#x} {label}"),
                    &prefixed,
                    &fault_state(base),
                    INTEGER_FLAGS,
                );
            }
        }
    }
}

/// Memory forms of G4 for the refusal checks.
const REFUSAL_SAMPLE: &[&[u8]] = &[
    &[0x48, 0x8b, 0x06],                   // mov rax, [rsi]
    &[0x48, 0x89, 0x06],                   // mov [rsi], rax
    &[0xc6, 0x06, 0x05],                   // mov byte [rsi], 5
    &[0x0f, 0xb6, 0x06],                   // movzx eax, byte [rsi]
    &[0x48, 0x63, 0x06],                   // movsxd rax, [rsi]
    &[0x0f, 0xbe, 0xc1],                   // movsx eax, cl
    &[0x48, 0x8d, 0x44, 0xce, 0x08],       // lea rax, [rsi+rcx*8+8]
    &[0x8b, 0x05, 0x00, 0x00, 0x00, 0x00], // mov eax, [rip]
];

#[test]
fn inadmissible_conditions_are_refused_without_a_memory_call() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let base = || {
        let mut state = crate::fast_step_g2_tests::state(5, 2 | CF);
        state.gpr[6] = 0x2000;
        state.gpr[1] = 1;
        state
    };
    for &bytes in REFUSAL_SAMPLE {
        // Control: the plain case is taken, so each refusal below comes from its condition.
        g4_kind(&decode_at(CODE, bytes));
        fast(bytes, &base(), INTEGER_FLAGS);
        for privilege in [
            PrivilegeLevel::Ring1,
            PrivilegeLevel::Ring2,
            PrivilegeLevel::Ring3,
        ] {
            assert_refused("cpl", bytes, base(), context(privilege), |_, _| {});
        }
        assert_refused("rf", bytes, base(), ring0, |s, _| s.rflags |= RF);
        assert_refused("tf", bytes, base(), ring0, |s, _| s.rflags |= TF);
        assert_refused("shadow", bytes, base(), ring0, |s, _| {
            s.sti_shadow = Some(StiShadow::Blocking);
        });
        assert_refused("no shadow model", bytes, base(), ring0, |s, _| {
            s.sti_shadow = None;
        });
        for group in [0xf0u8, 0xf2, 0xf3] {
            assert_refused("group1", bytes, base(), ring0, |_, i| {
                i.prefixes.group1 = Some(group);
            });
        }
        for segment in [0x64u8, 0x65] {
            assert_refused("fs/gs", bytes, base(), ring0, |_, i| {
                i.prefixes.segment = Some(segment);
            });
        }
    }
}

/// The step takes the G4 forms through the fast path only when the set was built with
/// `with_fast_path`, and the run (retired count, exit, state, memory, the order of the data calls) is the
/// same either way.
#[test]
fn the_step_takes_g4_forms_with_the_flag_and_the_run_is_unchanged() {
    use crate::fast_path_check;
    // 0x00 mov [rsi+8], rax        48 89 46 08
    // 0x04 mov rcx, [rsi+8]        48 8b 4e 08
    // 0x08 movzx edx, byte [rsi+9] 0f b6 56 09
    // 0x0c movsxd rdi, [rsi+0x10]  48 63 7e 10
    // 0x10 lea r8, [rsi+rdx*4+1]   4c 8d 44 96 01
    // 0x15 mov byte [rsp-1], 0x7f  c6 44 24 ff 7f
    // 0x1a mov al, [rsp-1]         8a 44 24 ff
    // 0x1e mov rbx, [rip+0x100]    48 8b 1d 00 01 00 00
    // 0x25 mov [rsi], r8           4c 89 06
    // 0x28 nop                     90
    let program = [
        0x48, 0x89, 0x46, 0x08, 0x48, 0x8b, 0x4e, 0x08, 0x0f, 0xb6, 0x56, 0x09, 0x48, 0x63, 0x7e,
        0x10, 0x4c, 0x8d, 0x44, 0x96, 0x01, 0xc6, 0x44, 0x24, 0xff, 0x7f, 0x8a, 0x44, 0x24, 0xff,
        0x48, 0x8b, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x4c, 0x89, 0x06, 0x90,
    ];
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let run_set = |set: &Cpl0ProfileSet<'_>, seed: u64| {
        let mut gpr = [0u64; 16];
        for (index, register) in gpr.iter_mut().enumerate() {
            *register = 0x1234_5678_9abc_def0 ^ (seed << 8) ^ index as u64;
        }
        gpr[4] = 0x0080_0000 + seed * 0x40;
        gpr[6] = 0x0030_0000 + seed * 0x13;
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
        assert_eq!(off.instructions_executed, 10);
        assert_eq!(on, off, "seed {seed}");
        assert_eq!(on_state, off_state, "seed {seed}");
        assert_eq!(on_ram.written, off_ram.written, "seed {seed}");
        assert_same_calls(&format!("seed {seed}"), &off_log, &on_log);
        // Eight memory calls: every instruction but the LEA and the NOP.
        assert_eq!(off_log.calls().len(), 8);
        let methods: Vec<MemoryMethod> = off_log.calls().iter().map(|call| call.method).collect();
        assert_eq!(
            methods,
            [
                MemoryMethod::WriteInteger,
                MemoryMethod::ReadInteger,
                MemoryMethod::ReadInteger,
                MemoryMethod::ReadInteger,
                MemoryMethod::WriteInteger,
                MemoryMethod::ReadInteger,
                MemoryMethod::ReadInteger,
                MemoryMethod::WriteInteger,
            ]
        );
        assert_eq!(taken_off, 0);
        if fast_path_check::FAST_STEPS_COUNTED {
            // Everything but the final NOP.
            assert_eq!(taken_on, 9, "seed {seed}");
        }
    }
}
