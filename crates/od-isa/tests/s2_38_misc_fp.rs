//! S2-38: the remaining forms of the general SSE floating-point core (S2-37): SQRTPS/SQRTPD/SQRTSS/SQRTSD
//! (0F 51), MINPS/MINPD/MINSS/MINSD (0F 5D), MAXPS/MAXPD/MAXSS/MAXSD (0F 5F), COMISS/COMISD (0F 2F),
//! UCOMISS/UCOMISD (0F 2E), CMPPS/CMPPD/CMPSS/CMPSD (0F C2 /r ib), CVTDQ2PS (0F 5B), CVTPS2DQ (66 0F 5B) and
//! CVTTPS2DQ (F3 0F 5B), with any XMM or memory source.
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A CMPPD pp. 3-168..3-174, CMPPS pp. 3-175..3-184 (predicate table 1-2/1-5, imm8 bits 2:0 for the legacy
//! encoding, bits 7:3 reserved), CMPSD pp. 3-185..3-188, CMPSS pp. 3-189..3-193, COMISD pp. 3-199..3-200, COMISS
//! pp. 3-201..3-202 (ZF, PF, CF; OF, SF, AF cleared; EFLAGS not updated by an unmasked exception; invalid for QNaN
//! and SNaN), CVTDQ2PS pp. 3-211..3-213, CVTPS2DQ pp. 3-225..3-227 and CVTTPS2DQ pp. 3-248..3-249 (invalid,
//! precision; integer indefinite 80000000H); Vol. 2B MAXPD pp. 4-5..4-7, MAXPS pp. 4-8..4-10, MAXSD pp. 4-11..4-12,
//! MAXSS pp. 4-13..4-15, MINPD pp. 4-16..4-18, MINPS pp. 4-19..4-21, MINSD pp. 4-22..4-23, MINSS pp. 4-24..4-26
//! (the second source operand is the result for equal zeros and for any NaN; a QNaN source is invalid too),
//! SQRTPD pp. 4-661..4-663, SQRTPS pp. 4-664..4-666, SQRTSD pp. 4-667..4-668, SQRTSS pp. 4-669..4-670, UCOMISD
//! pp. 4-733..4-734, UCOMISS pp. 4-735..4-736; exception classes Vol. 2A Table 2-19 (Type 2: packed forms need a
//! 16-byte aligned memory operand) and Table 2-20 (Type 3: scalar forms); Vol. 1 section 4.8.3.5 (SSE NaN rules),
//! 4.9.1/4.9.2, 10.2.3 and 11.5.2/11.5.3 (MXCSR, SIMD floating-point exceptions).
//!
//! Where the SDM text does not fix an observable detail, the expected value is the one the physical x86 oracle
//! gave (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; the program loads MXCSR with LDMXCSR and runs the
//! instruction directly). The table GOLDEN below is hardware results (selected from 2 x 282275 cases in
//! work/stage8/s2_38_oracle/: gen_cases.py, oracle.c, pick_golden.js), not model output; the hardware-confirmed
//! points that the SDM leaves open: CVTPS2DQ/CVTTPS2DQ never set the denormal-operand flag (DAZ still turns a
//! denormal source into zero), MINPS/MAXPS and the like report a denormal operand only when neither operand is a
//! NaN, SQRT of a negative nonzero value is the default NaN with IE only, and the hardware ignores the reserved
//! CMPxx predicate bits 7:3 (the model refuses them instead).
//!
//! Not asserted here: the CPUID gate (S2-57), the alignment-check exception of the scalar forms (an unaligned
//! scalar operand at CPL3 with RFLAGS.AC = 1 is Unsupported), FS/GS overrides and the prefix sequences the
//! hardware tolerates (Unsupported), the JIT (refused, see od-jit), CPL3 (refused, see cpl3_profile_set).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{
    self, DecodeError, Operation, Sse3Source, SseFpArithKind, SseFpMiscKind, SseFpShape,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, StiShadow,
};

const CODE: u64 = 0x2000;
const DATA: u64 = 0x8000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
/// The arithmetic flags COMISS/UCOMISS define.
const ARITH: u64 = 0x8d5;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const ZF: u64 = 1 << 6;
/// All six exceptions masked, round to nearest, no DAZ, no FTZ, no flag set.
const MXCSR: u32 = 0x1f80;
const IE: u32 = 1;
const DE: u32 = 2;
const PE: u32 = 0x20;
const DAZ: u32 = 0x40;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;

const PS: SseFpShape = SseFpShape::Packed32;
const PD: SseFpShape = SseFpShape::Packed64;
const SS: SseFpShape = SseFpShape::Scalar32;
const SD: SseFpShape = SseFpShape::Scalar64;

struct Ram {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below.
    fault_page: Option<u64>,
}

impl Ram {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![SENTINEL; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            reads: vec![],
            writes: vec![],
            fault_page: None,
        }
    }

    fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
    }

    fn check(&self, address: u64, length: usize, access: AccessKind) -> Result<usize, MemoryError> {
        if let Some(page) = self.fault_page
            && address < page + 0x1000
            && address + length as u64 > page
        {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 2 } else { 0 }),
                linear_address: Some(address.max(page)),
            }));
        }
        let start = usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM));
        start.ok_or(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        self.reads.push((address, out.len()));
        let start = self.check(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        self.writes.push((address, data.len()));
        let start = self.check(address, data.len(), AccessKind::Write)?;
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
    })
}

/// Distinct GPRs, distinct XMM registers, a flag word with many bits set and an x87 state: any unwanted write
/// is visible in a whole-state comparison. The state carries no CR0 or CR4 (the bare run).
fn cpu() -> LongModeState {
    let mut cpu = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        x87: Some(X87State::fixture_seed()),
        sse: Some(SseState {
            xmm: core::array::from_fn(xmm_pattern),
            mxcsr: MXCSR,
        }),
        ..LongModeState::default()
    };
    for (index, slot) in cpu.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    cpu
}

/// The state that the FP-init CR0/CR4 profile pair admits plus the given control registers.
fn fp_init_cpu(cr0: u64, cr4: u64) -> LongModeState {
    let mut cpu = cpu();
    cpu.activity = Some(CpuActivity::Running);
    cpu.sti_shadow = Some(StiShadow::Clear);
    cpu.cr0 = Some(cr0);
    cpu.cr4 = Some(cr4);
    cpu
}

fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        privilege,
        ..AccessContext::default()
    }
}

fn step(cpu: &mut LongModeState, ram: &mut Ram) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(PrivilegeLevel::Ring3))
}

fn run_fp_init(mut cpu: LongModeState, ram: &mut Ram) -> (execute::RunResult, LongModeState) {
    let result = execute::run_with_cr4_profile(
        &mut cpu,
        ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(PrivilegeLevel::Ring3),
        },
        Cr4Profile::FpInitLong64V1,
        || false,
    );
    (result, cpu)
}

fn set_xmm(cpu: &mut LongModeState, index: usize, value: u128) {
    cpu.sse.as_mut().unwrap().xmm[index] = value.to_le_bytes();
}

fn xmm(cpu: &LongModeState, index: usize) -> u128 {
    u128::from_le_bytes(cpu.sse.as_ref().unwrap().xmm[index])
}

fn mxcsr(cpu: &LongModeState) -> u32 {
    cpu.sse.as_ref().unwrap().mxcsr
}

fn fault_of(vector: u8, error_code: Option<u32>) -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}

fn xm() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 19,
        error_code: None,
        linear_address: None,
    })
}

/// A vector of four binary32 (or two binary64) lanes, lane 0 lowest.
fn s4(lanes: [u32; 4]) -> u128 {
    lanes
        .iter()
        .enumerate()
        .fold(0, |acc, (i, lane)| acc | u128::from(*lane) << (32 * i))
}

fn d2(lanes: [u64; 2]) -> u128 {
    u128::from(lanes[0]) | u128::from(lanes[1]) << 64
}

/// What the hardware did: the destination register, MXCSR and the six arithmetic flags afterwards, or a
/// SIMD floating-point exception (#XM) with the MXCSR the fault leaves behind (nothing else changes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Out {
    Ok { dst: u128, mxcsr: u32, flags: u64 },
    Fault { mxcsr: u32 },
}

const fn ok(dst: u128, mxcsr: u32, flags: u64) -> Out {
    Out::Ok { dst, mxcsr, flags }
}

const fn fault(mxcsr: u32) -> Out {
    Out::Fault { mxcsr }
}

/// One hardware vector: name, instruction bytes (register form, hex), (destination register, value),
/// (source register, value), MXCSR and RFLAGS before, and the outcome.
struct G {
    name: &'static str,
    code: &'static str,
    destination: (u8, u128),
    source: (u8, u128),
    mxcsr: u32,
    flags: u64,
    out: Out,
}

const fn g(
    name: &'static str,
    code: &'static str,
    destination: (u8, u128),
    source: (u8, u128),
    mxcsr: u32,
    flags: u64,
    out: Out,
) -> G {
    G {
        name,
        code,
        destination,
        source,
        mxcsr,
        flags,
        out,
    }
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// The state change one instruction must make: only the destination XMM, MXCSR, the six arithmetic flags and RIP
/// (a fault changes MXCSR only).
fn expected_state(before: &LongModeState, dst: usize, out: Out, length: usize) -> LongModeState {
    let mut expected = before.clone();
    match out {
        Out::Ok {
            dst: value,
            mxcsr,
            flags,
        } => {
            set_xmm(&mut expected, dst, value);
            expected.sse.as_mut().unwrap().mxcsr = mxcsr;
            expected.rflags = expected.rflags & !ARITH | flags;
            expected.rip += length as u64;
        }
        Out::Fault { mxcsr } => expected.sse.as_mut().unwrap().mxcsr = mxcsr,
    }
    expected
}

fn run_golden(vector: &G) {
    let code = from_hex(vector.code);
    let (d, s) = (
        usize::from(vector.destination.0),
        usize::from(vector.source.0),
    );
    let build = |cr0: Option<u64>| {
        let mut cpu = match cr0 {
            Some(cr0) => fp_init_cpu(cr0, CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT),
            None => cpu(),
        };
        cpu.rflags = vector.flags | 2;
        cpu.sse.as_mut().unwrap().mxcsr = vector.mxcsr;
        set_xmm(&mut cpu, d, vector.destination.1);
        set_xmm(&mut cpu, s, vector.source.1);
        cpu
    };
    // The FP-init profile pair with CR4.OSXMMEXCPT = 1 also runs the faulting vectors (#XM).
    let before = build(Some(CR0_BASE));
    let mut ram = Ram::new(&code);
    let (result, after) = run_fp_init(before.clone(), &mut ram);
    assert!(
        ram.reads.is_empty() && ram.writes.is_empty(),
        "{}",
        vector.name
    );
    let expected = expected_state(&before, d, vector.out, code.len());
    match vector.out {
        Out::Ok { .. } => assert_eq!(
            result.exit,
            RunExit::InstructionBudgetExhausted,
            "{}",
            vector.name
        ),
        Out::Fault { .. } => assert_eq!(result.exit, xm(), "{}", vector.name),
    }
    assert_eq!(after, expected, "{}", vector.name);
    // The bare run (no control registers) gives the same state for every completed instruction.
    if let Out::Ok { .. } = vector.out {
        let before = build(None);
        let mut bare = before.clone();
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut bare, &mut ram),
            Ok(StepOutcome::Advanced),
            "{} (bare)",
            vector.name
        );
        assert_eq!(
            bare,
            expected_state(&before, d, vector.out, code.len()),
            "{} (bare)",
            vector.name
        );
    }
}

#[test]
fn hardware_vectors_register_forms() {
    assert!(GOLDEN.len() > 500);
    for vector in GOLDEN {
        run_golden(vector);
    }
}

// ---- encodings ----

#[derive(Clone, Copy, Debug)]
enum Op {
    Sqrt,
    Min,
    Max,
    Comi,
    Ucomi,
    Cmp(u8),
    Cvtdq2ps,
    Cvtps2dq,
    Cvttps2dq,
}

fn prefix(op: Op, shape: SseFpShape) -> &'static [u8] {
    match op {
        Op::Comi | Op::Ucomi => match shape {
            SD => &[0x66],
            _ => &[],
        },
        Op::Cvtdq2ps => &[],
        Op::Cvtps2dq => &[0x66],
        Op::Cvttps2dq => &[0xf3],
        _ => match shape {
            PS => &[],
            PD => &[0x66],
            SS => &[0xf3],
            SD => &[0xf2],
        },
    }
}

fn opcode(op: Op) -> u8 {
    match op {
        Op::Sqrt => 0x51,
        Op::Min => 0x5d,
        Op::Max => 0x5f,
        Op::Comi => 0x2f,
        Op::Ucomi => 0x2e,
        Op::Cmp(_) => 0xc2,
        Op::Cvtdq2ps | Op::Cvtps2dq | Op::Cvttps2dq => 0x5b,
    }
}

/// [prefix] [REX] 0F op modrm [imm8] with a register source. REX.W is ignored by the instruction; with it the
/// narrow profile decoder refuses the encoding and the general decoder handles it.
fn reg_code(op: Op, shape: SseFpShape, dst: u8, src: u8, rex_w: bool) -> Vec<u8> {
    let mut code = prefix(op, shape).to_vec();
    let rex = 0x40 | u8::from(rex_w) << 3 | u8::from(dst >= 8) << 2 | u8::from(src >= 8);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(op), 0xc0 | (dst & 7) << 3 | (src & 7)]);
    if let Op::Cmp(predicate) = op {
        code.push(predicate);
    }
    code
}

/// The same instruction with the memory source given by the ModRM, SIB and displacement bytes.
fn mem_code(op: Op, shape: SseFpShape, dst: u8, address: &[u8], rex_b: bool) -> Vec<u8> {
    let mut code = prefix(op, shape).to_vec();
    let rex = 0x40 | u8::from(dst >= 8) << 2 | u8::from(rex_b);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(op)]);
    code.push(address[0] | (dst & 7) << 3);
    code.extend(&address[1..]);
    if let Op::Cmp(predicate) = op {
        code.push(predicate);
    }
    code
}

/// The ten operation and shape combinations (the exact forms of every instruction).
fn all_forms() -> Vec<(Op, SseFpShape)> {
    let mut forms = vec![];
    for shape in [PS, PD, SS, SD] {
        forms.extend([
            (Op::Sqrt, shape),
            (Op::Min, shape),
            (Op::Max, shape),
            (Op::Cmp(2), shape),
        ]);
    }
    for shape in [SS, SD] {
        forms.extend([(Op::Comi, shape), (Op::Ucomi, shape)]);
    }
    forms.extend([(Op::Cvtdq2ps, PS), (Op::Cvtps2dq, PS), (Op::Cvttps2dq, PS)]);
    forms
}

struct Obs {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    length: usize,
}

impl Obs {
    fn dst(&self, index: usize) -> u128 {
        xmm(&self.after, index)
    }
    fn mxcsr(&self) -> u32 {
        mxcsr(&self.after)
    }
    fn advanced(&self) -> bool {
        self.exit == RunExit::InstructionBudgetExhausted
    }
}

const CR4_ALL: u64 = CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT;

/// One instruction under the FP-init profile pair: xmm1 = a, xmm2 = b, MXCSR as given, RFLAGS = FLAGS | extra.
fn run_code(code: &[u8], (a, b): (u128, u128), mx: u32, flags: u64) -> Obs {
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_ALL);
    cpu.sse.as_mut().unwrap().mxcsr = mx;
    cpu.rflags = FLAGS | flags;
    set_xmm(&mut cpu, 1, a);
    set_xmm(&mut cpu, 2, b);
    let before = cpu.clone();
    let mut ram = Ram::new(code);
    let (result, after) = run_fp_init(cpu, &mut ram);
    assert!(ram.reads.is_empty() && ram.writes.is_empty());
    Obs {
        exit: result.exit,
        before,
        after,
        length: code.len(),
    }
}

fn run_op(op: Op, shape: SseFpShape, ab: (u128, u128), mx: u32) -> Obs {
    run_code(&reg_code(op, shape, 1, 2, false), ab, mx, 0)
}

/// Whole-state check: nothing but xmm1, MXCSR and RIP changed (and RFLAGS for COMISS/UCOMISS).
fn assert_only(observed: &Obs, dst: Option<u128>, mx: u32, label: &str) {
    assert!(observed.advanced(), "{label}: {:?}", observed.exit);
    let mut expected = observed.before.clone();
    if let Some(value) = dst {
        set_xmm(&mut expected, 1, value);
    }
    expected.sse.as_mut().unwrap().mxcsr = mx;
    expected.rip += observed.length as u64;
    expected.rflags = observed.after.rflags;
    assert_eq!(observed.after, expected, "{label}");
}

fn assert_fault(observed: &Obs, mx: u32, label: &str) {
    assert_eq!(observed.exit, xm(), "{label}");
    let mut expected = observed.before.clone();
    expected.sse.as_mut().unwrap().mxcsr = mx;
    assert_eq!(observed.after, expected, "{label}");
}

const HI: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;

fn lane(low: u32) -> u128 {
    HI & !0xffff_ffff | u128::from(low)
}

fn lane64(low: u64) -> u128 {
    HI & !0xffff_ffff_ffff_ffff | u128::from(low)
}

const QNAN: u32 = 0x7fc0_0001;
const SNAN: u32 = 0x7fa0_0000;
const DEFAULT_NAN: u32 = 0xffc0_0000;

// ---- SQRT ----

#[test]
fn sqrt_scalar_and_packed_values_and_flags() {
    // (shape, source, destination before, result, MXCSR before, MXCSR after)
    let rne = MXCSR;
    let down = MXCSR | 1 << 13;
    let up = MXCSR | 2 << 13;
    let toward_zero = MXCSR | 3 << 13;
    let ss: [(u32, u32, u32, u32); 14] = [
        (0x4080_0000, 0x4000_0000, rne, rne),        // sqrt(4) = 2
        (0xbf80_0000, DEFAULT_NAN, rne, rne | IE),   // sqrt(-1): the default NaN
        (0x8000_0000, 0x8000_0000, rne, rne),        // sqrt(-0) = -0
        (0x7f80_0000, 0x7f80_0000, rne, rne),        // sqrt(+inf)
        (0xff80_0000, DEFAULT_NAN, rne, rne | IE),   // sqrt(-inf)
        (SNAN, SNAN | 0x0040_0000, rne, rne | IE),   // an SNaN is quieted
        (QNAN, QNAN, rne, rne),                      // a QNaN passes unchanged
        (0x4000_0000, 0x3fb5_04f3, rne, rne | PE),   // sqrt(2), nearest
        (0x4000_0000, 0x3fb5_04f3, down, down | PE), // down
        (0x4000_0000, 0x3fb5_04f4, up, up | PE),     // up
        (0x4000_0000, 0x3fb5_04f3, toward_zero, toward_zero | PE), // toward zero
        (0x0000_0001, 0x1a35_04f3, rne, rne | DE | PE), // a denormal source: DE, and inexact
        (0x0000_0001, 0, rne | DAZ, rne | DAZ),      // DAZ: sqrt(+0)
        (0x8000_0001, DEFAULT_NAN, rne, rne | IE),   // a negative denormal: invalid
    ];
    for (source, result, before, after) in ss {
        let observed = run_op(Op::Sqrt, SS, (HI, lane(source)), before);
        let label = format!("sqrtss {source:#x} mxcsr {before:#x}");
        assert_only(&observed, Some(lane(result)), after, &label);
        // The scalar result keeps the destination's upper bits and never reads the destination's element.
        assert_eq!(observed.dst(1) >> 32, HI >> 32, "{label}");
    }
    let sd: [(u64, u64, u32, u32); 5] = [
        (0x4010_0000_0000_0000, 0x4000_0000_0000_0000, rne, rne),
        (0x4000_0000_0000_0000, 0x3ff6_a09e_667f_3bcd, rne, rne | PE),
        (
            0x4000_0000_0000_0000,
            0x3ff6_a09e_667f_3bcc,
            down,
            down | PE,
        ),
        (0xbff0_0000_0000_0000, 0xfff8_0000_0000_0000, rne, rne | IE),
        (0x7ff4_0000_0000_0000, 0x7ffc_0000_0000_0000, rne, rne | IE),
    ];
    for (source, result, before, after) in sd {
        let observed = run_op(Op::Sqrt, SD, (HI, lane64(source)), before);
        assert_only(
            &observed,
            Some(lane64(result)),
            after,
            &format!("sqrtsd {source:#x}"),
        );
        assert_eq!(observed.dst(1) >> 64, HI >> 64);
    }
    let packed = run_op(
        Op::Sqrt,
        PS,
        (HI, s4([0x4080_0000, 0x4110_0000, 0x3e80_0000, 0x4180_0000])),
        rne,
    );
    assert_only(
        &packed,
        Some(s4([0x4000_0000, 0x4040_0000, 0x3f00_0000, 0x4080_0000])),
        rne,
        "sqrtps",
    );
    let packed = run_op(
        Op::Sqrt,
        PD,
        (HI, d2([0x4010_0000_0000_0000, 0x4002_0000_0000_0000])),
        rne,
    );
    assert_only(
        &packed,
        Some(d2([0x4000_0000_0000_0000, 0x3ff8_0000_0000_0000])),
        rne,
        "sqrtpd",
    );
    // One invalid lane makes the packed form invalid; the other lanes still get their result.
    let mixed = run_op(
        Op::Sqrt,
        PS,
        (HI, s4([0x4080_0000, 0xbf80_0000, 0x4110_0000, 0x4180_0000])),
        rne,
    );
    assert_only(
        &mixed,
        Some(s4([0x4000_0000, DEFAULT_NAN, 0x4040_0000, 0x4080_0000])),
        rne | IE,
        "sqrtps one invalid lane",
    );
}

#[test]
fn sqrt_unmasked_invalid_faults_without_writing() {
    let observed = run_op(Op::Sqrt, SS, (HI, lane(0xbf80_0000)), MXCSR & !0x80);
    assert_fault(&observed, (MXCSR & !0x80) | IE, "unmasked IM");
    // The denormal-operand exception, unmasked.
    let observed = run_op(Op::Sqrt, SS, (HI, lane(1)), MXCSR & !0x100);
    assert_fault(&observed, (MXCSR & !0x100) | DE, "unmasked DM");
    // The precision exception, unmasked: sqrt(2) is inexact.
    let observed = run_op(Op::Sqrt, SS, (HI, lane(0x4000_0000)), MXCSR & !0x1000);
    assert_fault(&observed, (MXCSR & !0x1000) | PE, "unmasked PM");
}

// ---- MIN / MAX ----

#[test]
fn min_max_second_operand_rules() {
    // (a, b, min, max, flags after) for the scalar binary32 form: the second source wins for equal zeros and
    // for any NaN (a signaling NaN is not quieted), and every NaN is invalid, quiet or signaling.
    let cases: [(u32, u32, u32, u32, u32); 12] = [
        (0x3f80_0000, 0x4000_0000, 0x3f80_0000, 0x4000_0000, 0),
        (0x4000_0000, 0x3f80_0000, 0x3f80_0000, 0x4000_0000, 0),
        (0xbf80_0000, 0x3f80_0000, 0xbf80_0000, 0x3f80_0000, 0),
        (0x0000_0000, 0x8000_0000, 0x8000_0000, 0x8000_0000, 0), // +0 then -0: the second operand
        (0x8000_0000, 0x0000_0000, 0x0000_0000, 0x0000_0000, 0), // -0 then +0: the second operand
        (QNAN, 0x3f80_0000, 0x3f80_0000, 0x3f80_0000, IE),
        (0x3f80_0000, QNAN, QNAN, QNAN, IE),
        (0x3f80_0000, SNAN, SNAN, SNAN, IE),
        (SNAN, 0x3f80_0000, 0x3f80_0000, 0x3f80_0000, IE),
        (0x7f80_0000, 0xff80_0000, 0xff80_0000, 0x7f80_0000, 0),
        (0x0000_0001, 0x3f80_0000, 0x0000_0001, 0x3f80_0000, DE), // a denormal operand
        (0x0000_0001, QNAN, QNAN, QNAN, IE),                      // a NaN takes priority over DE
    ];
    for (a, b, min, max, flags) in cases {
        for (op, expected) in [(Op::Min, min), (Op::Max, max)] {
            let observed = run_op(op, SS, (lane(a), lane(b)), MXCSR);
            let label = format!("{op:?}ss {a:#x} {b:#x}");
            assert_only(&observed, Some(lane(expected)), MXCSR | flags, &label);
        }
    }
    // DAZ: the denormal operand compares as a zero and raises nothing.
    let observed = run_op(Op::Min, SS, (lane(1), lane(0x8000_0000)), MXCSR | DAZ);
    assert_only(
        &observed,
        Some(lane(0x8000_0000)),
        MXCSR | DAZ,
        "min ss DAZ",
    );
    // Unmasked IM: a quiet NaN faults (QNaN is invalid for MIN/MAX), the destination is not written.
    let observed = run_op(Op::Max, SS, (lane(0x3f80_0000), lane(QNAN)), MXCSR & !0x80);
    assert_fault(&observed, (MXCSR & !0x80) | IE, "max unmasked IM");
    // Packed: lane by lane.
    let a = s4([0x3f80_0000, 0x4000_0000, QNAN, 0x8000_0000]);
    let b = s4([0x4000_0000, 0x3f80_0000, 0x3f80_0000, 0x0000_0000]);
    let observed = run_op(Op::Min, PS, (a, b), MXCSR);
    assert_only(
        &observed,
        Some(s4([0x3f80_0000, 0x3f80_0000, 0x3f80_0000, 0x0000_0000])),
        MXCSR | IE,
        "minps",
    );
    let observed = run_op(
        Op::Max,
        PD,
        (
            d2([0x4000_0000_0000_0000, 1]),
            d2([0x3ff0_0000_0000_0000, 2]),
        ),
        MXCSR,
    );
    assert_only(
        &observed,
        Some(d2([0x4000_0000_0000_0000, 2])),
        MXCSR | DE,
        "maxpd",
    );
    // Scalar double keeps the destination's upper half.
    let observed = run_op(
        Op::Min,
        SD,
        (lane64(0x4000_0000_0000_0000), lane64(0x3ff0_0000_0000_0000)),
        MXCSR,
    );
    assert_only(
        &observed,
        Some(lane64(0x3ff0_0000_0000_0000)),
        MXCSR,
        "minsd",
    );
}

// ---- COMISS / UCOMISS ----

fn compare_code(comi: bool, double: bool, rex_w: bool) -> Vec<u8> {
    reg_code(
        if comi { Op::Comi } else { Op::Ucomi },
        if double { SD } else { SS },
        1,
        2,
        rex_w,
    )
}

#[test]
fn comiss_ucomiss_flags() {
    let inf = f64::INFINITY;
    // (a, b, ZF|PF|CF) and whether a quiet NaN is involved.
    let cases: [(f64, f64, u64); 9] = [
        (1.0, 2.0, CF),
        (2.0, 1.0, 0),
        (1.0, 1.0, ZF),
        (-0.0, 0.0, ZF),
        (0.0, -0.0, ZF),
        (inf, 1.0, 0),
        (-inf, inf, CF),
        (inf, inf, ZF),
        (-1.0, -2.0, 0),
    ];
    for double in [false, true] {
        let bits = |x: f64| {
            if double {
                u128::from(x.to_bits())
            } else {
                u128::from((x as f32).to_bits())
            }
        };
        for comi in [true, false] {
            for rex_w in [false, true] {
                let code = compare_code(comi, double, rex_w);
                // RFLAGS starts with all six arithmetic flags set: OF, SF and AF must be cleared.
                for (a, b, flags) in cases {
                    let observed = run_code(
                        &code,
                        (HI & !0xffff_ffff_ffff_ffff | bits(a), bits(b)),
                        MXCSR,
                        ARITH,
                    );
                    let label = format!("comi={comi} double={double} {a} {b}");
                    assert_only(&observed, None, MXCSR, &label);
                    assert_eq!(observed.after.rflags & ARITH, flags, "{label}");
                    assert_eq!(
                        observed.after.rflags & !ARITH,
                        observed.before.rflags & !ARITH,
                        "{label}: the other flags are preserved"
                    );
                }
                // A quiet NaN: unordered (ZF, PF, CF); invalid for COMISS only.
                let nan = if double {
                    0x7ff8_0000_0000_0001u128
                } else {
                    u128::from(QNAN)
                };
                for (a, b) in [(nan, bits(1.0)), (bits(1.0), nan), (nan, nan)] {
                    let observed = run_code(&code, (a, b), MXCSR, 0);
                    assert_only(&observed, None, MXCSR | if comi { IE } else { 0 }, "qnan");
                    assert_eq!(observed.after.rflags & ARITH, ZF | PF | CF);
                }
                // A signaling NaN is invalid for both.
                let snan = if double {
                    0x7ff4_0000_0000_0000u128
                } else {
                    u128::from(SNAN)
                };
                let observed = run_code(&code, (snan, bits(1.0)), MXCSR, 0);
                assert_only(&observed, None, MXCSR | IE, "snan");
                assert_eq!(observed.after.rflags & ARITH, ZF | PF | CF);
            }
        }
    }
}

#[test]
fn comiss_exceptions_leave_eflags_alone() {
    let code = compare_code(true, false, false);
    // Unmasked IM: COMISS with a quiet NaN faults (#XM), EFLAGS and the destination are unchanged.
    let observed = run_code(&code, (lane(QNAN), lane(0x3f80_0000)), MXCSR & !0x80, ARITH);
    assert_fault(&observed, (MXCSR & !0x80) | IE, "comiss qnan, IM unmasked");
    assert_eq!(observed.after.rflags, observed.before.rflags);
    // UCOMISS with a quiet NaN does not fault; with a signaling NaN it does.
    let ucomiss = compare_code(false, false, false);
    let observed = run_code(&ucomiss, (lane(QNAN), lane(0x3f80_0000)), MXCSR & !0x80, 0);
    assert_only(&observed, None, MXCSR & !0x80, "ucomiss qnan, IM unmasked");
    assert_eq!(observed.after.rflags & ARITH, ZF | PF | CF);
    let observed = run_code(
        &ucomiss,
        (lane(SNAN), lane(0x3f80_0000)),
        MXCSR & !0x80,
        ARITH,
    );
    assert_fault(&observed, (MXCSR & !0x80) | IE, "ucomiss snan, IM unmasked");
    assert_eq!(observed.after.rflags, observed.before.rflags);
    // The denormal-operand exception, masked and unmasked; DAZ.
    let observed = run_code(&code, (lane(1), lane(0)), MXCSR, 0);
    assert_only(&observed, None, MXCSR | DE, "denormal operand");
    assert_eq!(
        observed.after.rflags & ARITH,
        0,
        "a denormal is greater than zero"
    );
    let observed = run_code(&code, (lane(1), lane(0)), MXCSR | DAZ, 0);
    assert_only(&observed, None, MXCSR | DAZ, "DAZ");
    assert_eq!(observed.after.rflags & ARITH, ZF, "DAZ: equal to zero");
    let observed = run_code(&code, (lane(1), lane(0)), MXCSR & !0x100, ARITH);
    assert_fault(&observed, (MXCSR & !0x100) | DE, "unmasked DM");
    assert_eq!(observed.after.rflags, observed.before.rflags);
    // Without CR4.OSXMMEXCPT the unmasked exception is #UD (vector 6).
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR);
    cpu.sse.as_mut().unwrap().mxcsr = MXCSR & !0x80;
    set_xmm(&mut cpu, 1, lane(QNAN));
    let mut ram = Ram::new(&code);
    let before = cpu.clone();
    let (result, after) = run_fp_init(cpu, &mut ram);
    assert_eq!(
        result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 6,
            error_code: None,
            linear_address: None
        })
    );
    assert_eq!(after.rflags, before.rflags);
    assert_eq!(mxcsr(&after), (MXCSR & !0x80) | IE);
    // In the bare run the vector is unknown (no CR4): Unsupported, MXCSR untouched.
    let mut bare = self::cpu();
    bare.sse.as_mut().unwrap().mxcsr = MXCSR & !0x80;
    set_xmm(&mut bare, 1, lane(QNAN));
    let snapshot = bare.clone();
    assert_eq!(
        step(&mut bare, &mut Ram::new(&code)),
        Ok(StepOutcome::Unsupported { opcode: 0x2f })
    );
    assert_eq!(bare.rflags, snapshot.rflags);
}

// ---- CMPxx ----

#[test]
fn cmp_predicates_on_every_relation() {
    // Relations (less, equal, greater, unordered) as lane values (a, b): a < b, a = b, a > b, a ? b.
    let pairs: [(u32, u32); 4] = [
        (0x3f80_0000, 0x4000_0000),
        (0x4000_0000, 0x4000_0000),
        (0x4000_0000, 0x3f80_0000),
        (QNAN, 0x3f80_0000),
    ];
    // EQ, LT, LE, UNORD, NEQ, NLT, NLE, ORD over (less, equal, greater, unordered).
    let truth: [[bool; 4]; 8] = [
        [false, true, false, false],
        [true, false, false, false],
        [true, true, false, false],
        [false, false, false, true],
        [true, false, true, true],
        [false, true, true, true],
        [false, false, true, true],
        [true, true, true, false],
    ];
    for (predicate, row) in truth.iter().enumerate() {
        let signals_quiet_nan = matches!(predicate, 1 | 2 | 5 | 6);
        // Packed: the four relations in the four lanes (the IE flag of the quiet NaN follows the predicate).
        let a = s4(pairs.map(|(a, _)| a));
        let b = s4(pairs.map(|(_, b)| b));
        let expected = s4(row.map(|t| if t { u32::MAX } else { 0 }));
        let observed = run_op(Op::Cmp(predicate as u8), PS, (a, b), MXCSR);
        assert_only(
            &observed,
            Some(expected),
            MXCSR | if signals_quiet_nan { IE } else { 0 },
            &format!("cmpps predicate {predicate}"),
        );
        // Scalar: each relation alone, upper bits of the destination preserved.
        for (index, (x, y)) in pairs.iter().enumerate() {
            let observed = run_op(Op::Cmp(predicate as u8), SS, (lane(*x), lane(*y)), MXCSR);
            let mask = if row[index] { u32::MAX } else { 0 };
            let nan = index == 3;
            assert_only(
                &observed,
                Some(lane(mask)),
                MXCSR | if nan && signals_quiet_nan { IE } else { 0 },
                &format!("cmpss predicate {predicate} relation {index}"),
            );
        }
        // Double precision, packed and scalar.
        let two = [(1.0f64, 2.0f64), (f64::NAN, 1.0)];
        let a = d2(two.map(|(a, _)| a.to_bits()));
        let b = d2(two.map(|(_, b)| b.to_bits()));
        let expected = d2([
            if row[0] { u64::MAX } else { 0 },
            if row[3] { u64::MAX } else { 0 },
        ]);
        let observed = run_op(Op::Cmp(predicate as u8), PD, (a, b), MXCSR);
        assert_only(
            &observed,
            Some(expected),
            MXCSR | if signals_quiet_nan { IE } else { 0 },
            &format!("cmppd predicate {predicate}"),
        );
        let observed = run_op(
            Op::Cmp(predicate as u8),
            SD,
            (lane64(2.0f64.to_bits()), lane64(2.0f64.to_bits())),
            MXCSR,
        );
        assert_only(
            &observed,
            Some(lane64(if row[1] { u64::MAX } else { 0 })),
            MXCSR,
            &format!("cmpsd predicate {predicate}"),
        );
    }
}

#[test]
fn cmp_signaling_nan_and_unmasked_exception() {
    // A signaling NaN is invalid for every predicate; EQ and UNORD do not signal for a quiet NaN.
    for predicate in 0..8u8 {
        let observed = run_op(
            Op::Cmp(predicate),
            SS,
            (lane(SNAN), lane(0x3f80_0000)),
            MXCSR,
        );
        assert!(observed.advanced());
        assert_eq!(observed.mxcsr(), MXCSR | IE, "snan predicate {predicate}");
    }
    let observed = run_op(
        Op::Cmp(0),
        SS,
        (lane(QNAN), lane(0x3f80_0000)),
        MXCSR & !0x80,
    );
    assert_only(
        &observed,
        Some(lane(0)),
        MXCSR & !0x80,
        "eq quiet nan: no exception",
    );
    let observed = run_op(
        Op::Cmp(1),
        SS,
        (lane(QNAN), lane(0x3f80_0000)),
        MXCSR & !0x80,
    );
    assert_fault(&observed, (MXCSR & !0x80) | IE, "lt quiet nan, IM unmasked");
    // The denormal-operand exception and DAZ.
    let observed = run_op(Op::Cmp(2), SS, (lane(1), lane(0)), MXCSR);
    assert_only(
        &observed,
        Some(lane(0)),
        MXCSR | DE,
        "denormal operand: 2^-149 <= 0 is false",
    );
    let observed = run_op(Op::Cmp(2), SS, (lane(1), lane(0)), MXCSR | DAZ);
    assert_only(&observed, Some(lane(u32::MAX)), MXCSR | DAZ, "DAZ: 0 <= 0");
}

#[test]
fn cmp_reserved_predicate_bits_are_refused_without_change() {
    // The legacy encoding reserves imm8 bits 7:3 (SDM); the hardware ignores them, the model refuses.
    for predicate in [0x08u8, 0x0f, 0x10, 0x1f, 0x20, 0x80, 0xff] {
        for shape in [PS, PD, SS, SD] {
            let code = reg_code(Op::Cmp(predicate), shape, 1, 2, false);
            let mut cpu = cpu();
            let before = cpu.clone();
            assert_eq!(
                step(&mut cpu, &mut Ram::new(&code)),
                Ok(StepOutcome::Unsupported { opcode: 0xc2 }),
                "{predicate:#x}"
            );
            assert_eq!(cpu, before);
        }
    }
}

// ---- CVTDQ2PS / CVTPS2DQ / CVTTPS2DQ ----

#[test]
fn cvtdq2ps_rounds_by_rc() {
    let ints = s4([1, 0xffff_ffff, 0x7fff_ffff, 0x0100_0001]);
    let nearest = run_op(Op::Cvtdq2ps, PS, (HI, ints), MXCSR);
    assert_only(
        &nearest,
        Some(s4([0x3f80_0000, 0xbf80_0000, 0x4f00_0000, 0x4b80_0000])),
        MXCSR | PE,
        "nearest",
    );
    let up = run_op(Op::Cvtdq2ps, PS, (HI, ints), MXCSR | 2 << 13);
    assert_only(
        &up,
        Some(s4([0x3f80_0000, 0xbf80_0000, 0x4f00_0000, 0x4b80_0001])),
        MXCSR | 2 << 13 | PE,
        "up",
    );
    let exact = run_op(
        Op::Cvtdq2ps,
        PS,
        (HI, s4([0, 0x8000_0000, 16_777_216, 0xff00_0000])),
        MXCSR,
    );
    assert_only(
        &exact,
        Some(s4([0, 0xcf00_0000, 0x4b80_0000, 0xcb80_0000])),
        MXCSR,
        "exact",
    );
    // The unmasked precision exception faults.
    let unmasked = run_op(Op::Cvtdq2ps, PS, (HI, ints), MXCSR & !0x1000);
    assert_fault(&unmasked, (MXCSR & !0x1000) | PE, "unmasked PM");
}

#[test]
fn cvtps2dq_and_cvttps2dq_rounding_indefinite_and_flags() {
    // 1.5, 2.5, -1.5, 0.5: nearest-even, down, up, and the truncating form.
    let floats = s4([0x3fc0_0000, 0x4020_0000, 0xbfc0_0000, 0x3f00_0000]);
    let cases: [(Op, u32, [u32; 4]); 5] = [
        (Op::Cvtps2dq, MXCSR, [2, 2, 0xffff_fffe, 0]),
        (Op::Cvtps2dq, MXCSR | 1 << 13, [1, 2, 0xffff_fffe, 0]),
        (Op::Cvtps2dq, MXCSR | 2 << 13, [2, 3, 0xffff_ffff, 1]),
        (Op::Cvtps2dq, MXCSR | 3 << 13, [1, 2, 0xffff_ffff, 0]),
        (Op::Cvttps2dq, MXCSR, [1, 2, 0xffff_ffff, 0]),
    ];
    for (op, mx, result) in cases {
        let observed = run_op(op, PS, (HI, floats), mx);
        assert_only(
            &observed,
            Some(s4(result)),
            mx | PE,
            &format!("{op:?} {mx:#x}"),
        );
    }
    // Exact values raise nothing; the integer indefinite 80000000H with IE for NaN, infinity and out of range.
    let exact = run_op(
        Op::Cvtps2dq,
        PS,
        (HI, s4([0x3f80_0000, 0xc000_0000, 0, 0xcf00_0000])),
        MXCSR,
    );
    assert_only(
        &exact,
        Some(s4([1, 0xffff_fffe, 0, 0x8000_0000])),
        MXCSR,
        "exact, -2^31 is in range",
    );
    for bad in [
        QNAN,
        SNAN,
        0x7f80_0000,
        0xff80_0000,
        0x4f00_0000,
        0xcf00_0001,
        0x7f7f_ffff,
    ] {
        for op in [Op::Cvtps2dq, Op::Cvttps2dq] {
            let lanes = s4([0x3f80_0000, bad, 0x4000_0000, 0x4040_0000]);
            let observed = run_op(op, PS, (HI, lanes), MXCSR);
            assert_only(
                &observed,
                Some(s4([1, 0x8000_0000, 2, 3])),
                MXCSR | IE,
                &format!("{op:?} {bad:#x}"),
            );
            let unmasked = run_op(op, PS, (HI, lanes), MXCSR & !0x80);
            assert_fault(
                &unmasked,
                (MXCSR & !0x80) | IE,
                &format!("{op:?} {bad:#x} unmasked"),
            );
        }
    }
    // A denormal source sets no denormal-operand flag (the SDM lists invalid and precision only); DAZ makes it zero.
    let observed = run_op(
        Op::Cvtps2dq,
        PS,
        (HI, s4([1, 0x8000_0001, 0x007f_ffff, 0])),
        MXCSR,
    );
    assert_only(
        &observed,
        Some(s4([0, 0, 0, 0])),
        MXCSR | PE,
        "denormal sources",
    );
    let observed = run_op(
        Op::Cvtps2dq,
        PS,
        (HI, s4([1, 0x8000_0001, 0x007f_ffff, 0])),
        MXCSR | DAZ,
    );
    assert_only(
        &observed,
        Some(s4([0, 0, 0, 0])),
        MXCSR | DAZ,
        "denormal sources, DAZ",
    );
    let observed = run_op(
        Op::Cvtps2dq,
        PS,
        (HI, s4([1, 0x8000_0001, 0x007f_ffff, 0])),
        MXCSR & !0x100,
    );
    assert_only(
        &observed,
        Some(s4([0, 0, 0, 0])),
        (MXCSR & !0x100) | PE,
        "DM unmasked is irrelevant",
    );
}

// ---- gates ----

#[test]
fn cr0_cr4_gates_and_lock() {
    for (op, shape) in all_forms() {
        let code = reg_code(op, shape, 1, 2, false);
        for (cr0, cr4, vector) in [
            (CR0_BASE | 1 << 2, CR4_ALL, 6u8),       // CR0.EM
            (CR0_BASE | 1 << 3, CR4_ALL, 7),         // CR0.TS: #NM
            (CR0_BASE, CR4_PAE | CR4_OSXMMEXCPT, 6), // CR4.OSFXSR = 0
        ] {
            let mut cpu = fp_init_cpu(cr0, cr4);
            set_xmm(&mut cpu, 1, lane(0x3f80_0000));
            let before = cpu.clone();
            let (result, after) = run_fp_init(cpu, &mut Ram::new(&code));
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector,
                    error_code: None,
                    linear_address: None
                }),
                "{op:?} {shape:?}"
            );
            assert_eq!(after, before, "{op:?} {shape:?}");
        }
        // LOCK is #UD.
        let mut locked = vec![0xf0];
        locked.extend(&code);
        let mut cpu = cpu();
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&locked)),
            Ok(fault_of(6, None)),
            "lock {op:?} {shape:?}"
        );
        assert_eq!(cpu, before);
    }
}

// ---- memory operands ----

/// The addressing forms: ModRM/SIB/displacement bytes (reg field zero), REX.B, the register setup and the address.
fn address_forms(
    delta: u64,
) -> Vec<(
    &'static str,
    Vec<u8>,
    bool,
    Vec<(od_isa::Register, u64)>,
    u64,
)> {
    use od_isa::Register::{R8, R13, Rax, Rbp, Rbx};
    let base = DATA + 0x100 + delta;
    vec![
        ("[rax]", vec![0x00], false, vec![(Rax, base)], base),
        ("[rbx]", vec![0x03], false, vec![(Rbx, base)], base),
        (
            "[rbp+0x10]",
            vec![0x45, 0x10],
            false,
            vec![(Rbp, base - 0x10)],
            base,
        ),
        (
            "[rbx+0x120]",
            vec![0x83, 0x20, 0x01, 0, 0],
            false,
            vec![(Rbx, base - 0x120)],
            base,
        ),
        (
            "[rax+rbx*4]",
            vec![0x04, 0x98],
            false,
            vec![(Rax, base - 0x40), (Rbx, 0x10)],
            base,
        ),
        ("[r8]", vec![0x00], true, vec![(R8, base)], base),
        (
            "[r13+8]",
            vec![0x45, 0x08],
            true,
            vec![(R13, base - 8)],
            base,
        ),
    ]
}

fn test_value(shape: SseFpShape, op: Op) -> u128 {
    match (op, shape.is_double()) {
        (Op::Cvtdq2ps, _) => s4([3, 0xffff_fff0, 0x7fff_ff00, 0x0100_0001]),
        (_, false) => s4([0x4080_0000, 0xbfc0_0000, 0x4020_0000, 0x3f00_0000]),
        (_, true) => d2([0x4010_0000_0000_0000, 0xbff8_0000_0000_0000]),
    }
}

#[test]
fn memory_source_equals_register_source() {
    for (op, shape) in all_forms() {
        let source = test_value(shape, op);
        let first = s4([0x4000_0000, 0x4040_0000, 0x3f80_0000, 0xc000_0000]);
        let width = match shape {
            PS | PD => 16,
            SS => 4,
            SD => 8,
        };
        let from_register = run_code(
            &reg_code(op, shape, 1, 2, false),
            (first, source),
            MXCSR,
            ARITH,
        );
        assert!(from_register.advanced(), "{op:?} {shape:?}");
        for (name, address, rex_b, registers, effective) in address_forms(0) {
            let code = mem_code(op, shape, 1, &address, rex_b);
            let mut cpu = cpu();
            cpu.rflags = FLAGS | ARITH;
            set_xmm(&mut cpu, 1, first);
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let mut ram = Ram::new(&code);
            ram.put(effective, &source.to_le_bytes()[..width]);
            ram.put(effective + width as u64, &[0xa5; 16]);
            let mut expected = cpu.clone();
            set_xmm(&mut expected, 1, xmm(&from_register.after, 1));
            expected.sse.as_mut().unwrap().mxcsr = from_register.mxcsr();
            expected.rflags = from_register.after.rflags;
            expected.rip += code.len() as u64;
            assert_eq!(
                step(&mut cpu, &mut ram),
                Ok(StepOutcome::Advanced),
                "{op:?} {shape:?} {name}"
            );
            assert_eq!(cpu, expected, "{op:?} {shape:?} {name}");
            assert_eq!(
                ram.reads,
                vec![(effective, width)],
                "{op:?} {shape:?} {name}"
            );
            assert!(ram.writes.is_empty());
        }
    }
}

#[test]
fn memory_alignment_faults_and_unaligned_scalars() {
    for (op, shape) in all_forms() {
        let width = match shape {
            PS | PD => 16,
            SS => 4,
            SD => 8,
        };
        for delta in [1u64, 4, 8, 12] {
            let (name, address, rex_b, registers, effective) = address_forms(delta).remove(1);
            let code = mem_code(op, shape, 1, &address, rex_b);
            let mut cpu = cpu();
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            ram.put(effective, &test_value(shape, op).to_le_bytes());
            let outcome = step(&mut cpu, &mut ram);
            if matches!(shape, PS | PD) {
                // Type 2: a packed 128-bit memory operand must be 16-byte aligned: #GP(0), no read.
                assert_eq!(
                    outcome,
                    Ok(fault_of(13, Some(0))),
                    "{op:?} {shape:?} {name} +{delta}"
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            } else {
                // Type 3: no alignment requirement for the scalar forms.
                assert_eq!(
                    outcome,
                    Ok(StepOutcome::Advanced),
                    "{op:?} {shape:?} {name} +{delta}"
                );
                assert_eq!(ram.reads, vec![(effective, width)]);
            }
        }
    }
}

#[test]
fn memory_faults_are_not_partial_and_noncanonical_is_gp() {
    for (op, shape) in all_forms() {
        let (_, address, rex_b, _, _) = address_forms(0).remove(1);
        let code = mem_code(op, shape, 1, &address, rex_b);
        // A page fault of the backend at the source: nothing changes.
        let mut cpu = cpu();
        cpu.gpr[od_isa::Register::Rbx as usize] = DATA + 0x1000;
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA + 0x1000);
        match step(&mut cpu, &mut ram) {
            Ok(StepOutcome::Fault(fault)) => assert_eq!(fault.vector, 14, "{op:?} {shape:?}"),
            Err(MemoryError::Architectural(fault)) => {
                assert_eq!(fault.vector, 14, "{op:?} {shape:?}")
            }
            other => panic!("{op:?} {shape:?}: {other:?}"),
        }
        assert_eq!(cpu, before, "{op:?} {shape:?}");
        // A non-canonical address: #GP(0).
        let mut cpu = self::cpu();
        cpu.gpr[od_isa::Register::Rbx as usize] = 0x0000_8000_0000_0000;
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&code)),
            Ok(fault_of(13, Some(0))),
            "{op:?} {shape:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn rip_relative_source_counts_the_immediate_byte() {
    // CMPPS xmm1, [rip+disp32], imm8: the displacement is relative to the end of the whole instruction.
    let predicate = 5u8; // NLT
    let mut code = vec![0x0f, 0xc2, 0x0d, 0, 0, 0, 0, predicate];
    let disp = (DATA as i64 - (CODE as i64 + code.len() as i64)) as i32;
    code[3..7].copy_from_slice(&disp.to_le_bytes());
    let mut cpu = cpu();
    set_xmm(
        &mut cpu,
        1,
        s4([0x3f80_0000, 0x4000_0000, 0x4040_0000, 0x4080_0000]),
    );
    let mut ram = Ram::new(&code);
    ram.put(
        DATA,
        &s4([0x4000_0000, 0x4000_0000, 0x4000_0000, 0x4000_0000]).to_le_bytes(),
    );
    assert_eq!(step(&mut cpu, &mut ram), Ok(StepOutcome::Advanced));
    assert_eq!(xmm(&cpu, 1), s4([0, u32::MAX, u32::MAX, u32::MAX]));
    assert_eq!(cpu.rip, CODE + code.len() as u64);
    assert_eq!(ram.reads, vec![(DATA, 16)]);
}

// ---- decoding ----

fn decode(bytes: &[u8]) -> Result<long_decode::Instruction, DecodeError> {
    let mut ram = Ram::new(bytes);
    long_decode::decode(&mut ram, CODE, AccessContext::default())
}

#[test]
fn decoder_produces_the_general_operations() {
    use SseFpArithKind::{Max, Min, Sqrt};
    use SseFpMiscKind::{Cmp, Comi, Cvtdq2ps, Cvtps2dq, Cvttps2dq, Ucomi};
    let arith: [(&[u8], SseFpArithKind, SseFpShape, u8); 6] = [
        (&[0x0f, 0x51, 0x0b], Sqrt, PS, 1),
        (&[0x66, 0x0f, 0x51, 0x0b], Sqrt, PD, 1),
        (&[0xf3, 0x0f, 0x5d, 0x0b], Min, SS, 1),
        (&[0xf2, 0x0f, 0x5d, 0x13], Min, SD, 2),
        (&[0x0f, 0x5f, 0x0b], Max, PS, 1),
        (&[0x66, 0x44, 0x0f, 0x5f, 0x0b], Max, PD, 9),
    ];
    for (bytes, kind, shape, destination) in arith {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        let Operation::SseFpArith {
            kind: k,
            shape: s,
            destination: d,
            source: Sse3Source::Memory(_),
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!((k, s, d), (kind, shape, destination), "{bytes:02x?}");
    }
    let misc: [(&[u8], SseFpMiscKind, SseFpShape, u8, u8); 10] = [
        (&[0x0f, 0x2f, 0x0b], Comi, SS, 1, 0),
        (&[0x66, 0x0f, 0x2f, 0x0b], Comi, SD, 1, 0),
        (&[0x0f, 0x2e, 0x0b], Ucomi, SS, 1, 0),
        (&[0x66, 0x0f, 0x2e, 0x0b], Ucomi, SD, 1, 0),
        (&[0x0f, 0xc2, 0x0b, 0x05], Cmp, PS, 1, 5),
        (&[0x66, 0x0f, 0xc2, 0x0b, 0x01], Cmp, PD, 1, 1),
        (&[0xf3, 0x0f, 0xc2, 0x0b, 0x07], Cmp, SS, 1, 7),
        (&[0x0f, 0x5b, 0x0b], Cvtdq2ps, PS, 1, 0),
        (&[0x66, 0x0f, 0x5b, 0x0b], Cvtps2dq, PS, 1, 0),
        (&[0xf3, 0x0f, 0x5b, 0x0b], Cvttps2dq, PS, 1, 0),
    ];
    for (bytes, kind, shape, destination, immediate) in misc {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        let Operation::SseFpMisc {
            kind: k,
            shape: s,
            destination: d,
            source: Sse3Source::Memory(_),
            immediate: i,
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!(
            (k, s, d, i),
            (kind, shape, destination, immediate),
            "{bytes:02x?}"
        );
    }
    // REX.R and REX.B extend the registers; REX.W is ignored.
    let instruction = decode(&[0x66, 0x4d, 0x0f, 0xc2, 0xca, 0x03]).unwrap();
    assert_eq!(
        instruction.operation,
        Operation::SseFpMisc {
            kind: Cmp,
            shape: PD,
            destination: 9,
            source: Sse3Source::Register(10),
            immediate: 3
        }
    );
}

#[test]
fn decoder_refuses_what_it_does_not_model_and_locked_forms_are_invalid() {
    // Unsupported: F2/F3 on COMISx/UCOMISx, F2 on 0F 5B, 66 together with F2/F3, a repeated prefix, FS/GS.
    for bytes in [
        &[0xf3, 0x0f, 0x2f, 0x0b][..],
        &[0xf2, 0x0f, 0x2e, 0x0b],
        &[0xf2, 0x0f, 0x5b, 0x0b],
        &[0x66, 0xf2, 0x0f, 0x51, 0x0b],
        &[0x66, 0xf3, 0x0f, 0x5d, 0x0b],
        &[0xf2, 0xf2, 0x0f, 0x5f, 0x0b],
        &[0x66, 0x66, 0x0f, 0xc2, 0x0b, 0x01],
        &[0x64, 0x0f, 0x51, 0x0b],
        &[0x65, 0x0f, 0x2f, 0x0b],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Unsupported { .. })),
            "{bytes:02x?}: {:?}",
            decode(bytes).map(|i| i.operation)
        );
    }
    for bytes in [
        &[0xf0, 0x0f, 0x51, 0x0b][..],
        &[0xf0, 0x66, 0x0f, 0x5d, 0x0b],
        &[0xf0, 0x0f, 0x2f, 0xc1],
        &[0xf0, 0xf3, 0x0f, 0xc2, 0x0b, 0x01],
        &[0xf0, 0x0f, 0x5b, 0x0b],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Invalid { .. })),
            "{bytes:02x?}"
        );
    }
}

/// The narrow profile decoder handles the REX-less register forms and the executor rewrites them into the
/// general operation; with REX.W the general decoder handles the same instruction. Both give the same state.
#[test]
fn narrow_register_forms_and_general_forms_agree() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let pool = [
        0x0000_0000u32,
        0x8000_0000,
        0x3f80_0000,
        0xbf80_0000,
        0x4000_0000,
        0x7f80_0000,
        0xff80_0000,
        QNAN,
        SNAN,
        1,
        0x007f_ffff,
        0x4f00_0000,
        0xcf00_0000,
        0x4b80_0001,
        0x3fc0_0000,
    ];
    for (op, shape) in all_forms() {
        for round in 0..300 {
            let mut vector = || {
                s4(core::array::from_fn(|_| {
                    pool[(next() % pool.len() as u64) as usize]
                })) | u128::from(next()) << 64 & if round % 3 == 0 { u128::MAX } else { 0 }
            };
            let (a, b) = (vector(), vector());
            let mx = [MXCSR, MXCSR | DAZ, MXCSR & !0x80, 0, MXCSR | 3 << 13][round % 5];
            let narrow = run_code(
                &reg_code(op, shape, 1, 2, false),
                (a, b),
                mx,
                ARITH * (round as u64 & 1),
            );
            let general = run_code(
                &reg_code(op, shape, 1, 2, true),
                (a, b),
                mx,
                ARITH * (round as u64 & 1),
            );
            assert_eq!(narrow.exit, general.exit, "{op:?} {shape:?} {a:#x} {b:#x}");
            assert_eq!(
                xmm(&narrow.after, 1),
                xmm(&general.after, 1),
                "{op:?} {shape:?}"
            );
            assert_eq!(narrow.mxcsr(), general.mxcsr(), "{op:?} {shape:?}");
            assert_eq!(
                narrow.after.rflags, general.after.rflags,
                "{op:?} {shape:?}"
            );
        }
    }
}

// ---- host arithmetic ----

#[test]
fn sqrt_and_conversions_match_the_host_on_random_operands() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..3000 {
        let x = f32::from_bits(next() as u32 & 0x7fff_ffff);
        if !x.is_nan() {
            let observed = run_op(Op::Sqrt, SS, (HI, lane(x.to_bits())), MXCSR);
            assert_eq!(observed.dst(1) as u32, x.sqrt().to_bits(), "sqrtss {x:e}");
        }
        let y = f64::from_bits(next() & 0x7fff_ffff_ffff_ffff);
        if !y.is_nan() {
            let observed = run_op(Op::Sqrt, SD, (HI, lane64(y.to_bits())), MXCSR);
            assert_eq!(observed.dst(1) as u64, y.sqrt().to_bits(), "sqrtsd {y:e}");
        }
        // CVTDQ2PS: Rust's integer to float cast rounds to nearest even.
        let ints: [i32; 4] = core::array::from_fn(|_| next() as i32 >> (next() % 32));
        let observed = run_op(Op::Cvtdq2ps, PS, (HI, s4(ints.map(|i| i as u32))), MXCSR);
        assert_eq!(
            observed.dst(1),
            s4(ints.map(|i| (i as f32).to_bits())),
            "{ints:?}"
        );
        // CVTPS2DQ rounds to nearest even, CVTTPS2DQ truncates: compare in the range that fits.
        let floats: [f32; 4] = core::array::from_fn(|_| {
            let magnitude = (next() % 4_000_000) as f32 / 64.0;
            if next() & 1 == 0 {
                magnitude
            } else {
                -magnitude
            }
        });
        let bits = s4(floats.map(f32::to_bits));
        let nearest = run_op(Op::Cvtps2dq, PS, (HI, bits), MXCSR);
        assert_eq!(
            nearest.dst(1),
            s4(floats.map(|f| f.round_ties_even() as i32 as u32)),
            "{floats:?}"
        );
        let truncated = run_op(Op::Cvttps2dq, PS, (HI, bits), MXCSR);
        assert_eq!(
            truncated.dst(1),
            s4(floats.map(|f| f.trunc() as i32 as u32)),
            "{floats:?}"
        );
    }
}

#[test]
fn min_max_match_the_host_comparison_on_random_operands() {
    let mut seed = 0x1234_5678_9abc_def1u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..3000 {
        let (x, y) = (next() as u32, next() as u32);
        let (fx, fy) = (f32::from_bits(x), f32::from_bits(y));
        let min = run_op(Op::Min, SS, (lane(x), lane(y)), MXCSR);
        let max = run_op(Op::Max, SS, (lane(x), lane(y)), MXCSR);
        // The SDM pseudo code: the second operand unless the first is strictly less (greater).
        let expected_min = if fx < fy { x } else { y };
        let expected_max = if fx > fy { x } else { y };
        assert_eq!(min.dst(1) as u32, expected_min, "min {x:#x} {y:#x}");
        assert_eq!(max.dst(1) as u32, expected_max, "max {x:#x} {y:#x}");
    }
}

// ---- hardware vectors ----

const GOLDEN: &[G] = &[
    g(
        "sx_sqrtss_38_2",
        "f3450f51ce",
        (9, 0x5cfe02dbaf37d79fc0feba5e3fc00000),
        (14, 0xcfd20f9735e3e6cc51668a0600800000),
        0xfe80,
        0x0,
        ok(0x5cfe02dbaf37d79fc0feba5e20000000, 0xfe80, 0x0),
    ),
    g(
        "s_sqrtss_14_1_4_in",
        "f30f51ca",
        (1, 0xe6dcc9294aaabdf5ec3f91377fc00001),
        (2, 0x6d304f57bda7e5a98e3876b080000000),
        0x1fc0,
        0x0,
        ok(0xe6dcc9294aaabdf5ec3f913780000000, 0x1fc0, 0x0),
    ),
    g(
        "sx_sqrtss_59_2",
        "f3450f51ce",
        (9, 0x37c66d0c773cfe302d4f2c664b000000),
        (14, 0x6a8a5958826d7fa0724cd5cd7fa00000),
        0x1d80,
        0x0,
        ok(0x37c66d0c773cfe302d4f2c667fe00000, 0x1d81, 0x0),
    ),
    g(
        "s_sqrtss_26_23_5_in",
        "f30f51ca",
        (1, 0xfd154e8b4dde11f18794eeb500400000),
        (2, 0xa5c21c7221ca679faabea1d380000001),
        0x9f80,
        0x0,
        ok(0xfd154e8b4dde11f18794eeb5ffc00000, 0x9f81, 0x0),
    ),
    g(
        "sx_sqrtss_0_12",
        "f3450f51ce",
        (9, 0x63c309571d336ab4852fb39e7f800000),
        (14, 0x94c9d91d5991b55ec1363ec6ff800000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_sqrtss_30_11_7_in",
        "f30f51ca",
        (1, 0x377adca28e1612d95248398c00800001),
        (2, 0x255612a95131fb58dc96c336ff800000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_sqrtss_44_31_3_in",
        "f30f51ca",
        (1, 0x24873231e5583cd1189e697f40a00000),
        (2, 0x19d8dbb1e90d019cf5a45aad7f7fffff),
        0x7f80,
        0x0,
        ok(0x24873231e5583cd1189e697f5f7fffff, 0x7fa0, 0x0),
    ),
    g(
        "s_sqrtss_17_43_5_in",
        "f30f51ca",
        (1, 0x8205bad906073011a55177b37f800001),
        (2, 0x485d012a9d217a7dc453879600ffffff),
        0x9f80,
        0x0,
        ok(0x8205bad906073011a55177b3203504f3, 0x9fa0, 0x0),
    ),
    g(
        "sx_sqrtss_62_10",
        "f3450f51ce",
        (9, 0xf26b6087b47a649cbbc7bd200800000),
        (14, 0xfba5f0e7df344f932296a6423fc00000),
        0xf80,
        0x0,
        fault(0xfa0),
    ),
    g(
        "s_sqrtss_14_31_7_in",
        "f30f51ca",
        (1, 0xf697c59ebbfce5fe39470f827fc00001),
        (2, 0x87f528e64720e3a9ce62925e7f7fffff),
        0x0,
        0x0,
        fault(0x20),
    ),
    g(
        "s_sqrtss_39_27_5_in",
        "f30f51ca",
        (1, 0x9be312e4742d9031584b86bc4b7fffff),
        (2, 0xfabda2883a760b9aa5f9c96300000003),
        0x9f80,
        0x0,
        ok(0x9be312e4742d9031584b86bc1a9cc471, 0x9fa2, 0x0),
    ),
    g(
        "s_sqrtss_10_27_0_in",
        "f30f51ca",
        (1, 0xaa606cc930da4894ea042ee07f800000),
        (2, 0x204222c928865fd8936d805200000003),
        0x1f80,
        0x0,
        ok(0xaa606cc930da4894ea042ee01a9cc471, 0x1fa2, 0x0),
    ),
    g(
        "rb_sqrtss_150",
        "f30f51df",
        (3, 0x14958bd5f4fbb5830b4aec81bfb8c7cf),
        (7, 0xc006c278cfed30c4ec07cee4002a8c3a),
        0xf614,
        0x0,
        fault(0xf616),
    ),
    g(
        "s_sqrtss_8_22_7_in",
        "f30f51ca",
        (1, 0xeb98c0a330447235914440153f800001),
        (2, 0x9f958b03a3af4f141a69de8a00000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "rb_sqrtss_279",
        "f3450f51ec",
        (13, 0xe8c122ff764423fffd7491cfe2f6e7),
        (12, 0xff0c575380174ef93fe251480062dcb5),
        0xf80,
        0x0,
        fault(0xfa2),
    ),
    g(
        "sx_sqrtss_62_21",
        "f3450f51ce",
        (9, 0x8f79bf34f6ac4885eddd8f657f800001),
        (14, 0xcd28422f7145f538b62b622700000003),
        0xf80,
        0x0,
        fault(0xfa2),
    ),
    g(
        "rb_sqrtss_291",
        "f3450f51c1",
        (8, 0x3f9688da0066d3f80925cf418b26a5b6),
        (9, 0xc031b71a30340a9e74ee404d3023be0b),
        0x66a1,
        0x0,
        fault(0x66a1),
    ),
    g(
        "rb_sqrtss_26",
        "f3410f51f9",
        (7, 0x142af34200f72c7c7f7e3215bfc8829d),
        (9, 0x805655ac00c57431014614557f64b37c),
        0xe6e5,
        0x0,
        fault(0xe6e5),
    ),
    g(
        "s_sqrtsd_37_16_3_in",
        "f20f51ca",
        (1, 0xd3f854ddef15c0d73ca0000000000000),
        (2, 0xea2ed4c4037e3706fff8123456789abc),
        0x7f80,
        0x0,
        ok(0xd3f854ddef15c0d7fff8123456789abc, 0x7f80, 0x0),
    ),
    g(
        "s_sqrtsd_0_1_4_in",
        "f20f51ca",
        (1, 0x1f28c277eba9cb930000000000000000),
        (2, 0x5046f2b4801083e78000000000000000),
        0x1fc0,
        0x0,
        ok(0x1f28c277eba9cb938000000000000000, 0x1fc0, 0x0),
    ),
    g(
        "s_sqrtsd_42_29_5_in",
        "f20f51ca",
        (1, 0xf285cf7cf369d50020000000000000),
        (2, 0x32789a2d325b5dc8010000000000000),
        0x9f80,
        0x0,
        ok(0xf285cf7cf369d5fff8000000000000, 0x9f81, 0x0),
    ),
    g(
        "s_sqrtsd_11_5_5_in",
        "f20f51ca",
        (1, 0xe527d60f55dc31a8fff0000000000000),
        (2, 0x9a033caf3968f77ac000000000000000),
        0x9f80,
        0x0,
        ok(0xe527d60f55dc31a8fff8000000000000, 0x9f81, 0x0),
    ),
    g(
        "sx_sqrtsd_64_12",
        "f2450f51ce",
        (9, 0xe5c576d104d8b5e43fefffffffffffff),
        (14, 0x2f5952f012247efffff0000000000001),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "s_sqrtsd_0_5_7_in",
        "f20f51ca",
        (1, 0x4d374e619e3d9c560000000000000000),
        (2, 0x8b236a0a9915affc000000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_sqrtsd_13_8_4_in",
        "f20f51ca",
        (1, 0x357b953a790950aafff8000000000000),
        (2, 0xdce63bb85e68a57e3ff0000000000001),
        0x1fc0,
        0x0,
        ok(0x357b953a790950aa3ff0000000000000, 0x1fe0, 0x0),
    ),
    g(
        "s_sqrtsd_12_42_2_in",
        "f20f51ca",
        (1, 0xf2feb4af3152ad307ff8000000000000),
        (2, 0x2c0f5b33054c7200020000000000000),
        0x5f80,
        0x0,
        ok(0xf2feb4af3152ad302006a09e667f3bcd, 0x5fa0, 0x0),
    ),
    g(
        "s_sqrtsd_25_43_7_in",
        "f20f51ca",
        (1, 0xa139b4bcfdd1bbc4800fffffffffffff),
        (2, 0x244b1b19a7486cb9001fffffffffffff),
        0x0,
        0x0,
        fault(0x20),
    ),
    g(
        "s_sqrtsd_22_9_7_in",
        "f20f51ca",
        (1, 0x57902c50ea119da30000000000000001),
        (2, 0x1808b42acd02a3ec3fefffffffffffff),
        0x0,
        0x0,
        fault(0x20),
    ),
    g(
        "s_sqrtsd_44_22_3_in",
        "f20f51ca",
        (1, 0xb0468024ff6c82a74014000000000000),
        (2, 0x2ea321a90284b3e90000000000000001),
        0x7f80,
        0x0,
        ok(0xb0468024ff6c82a71e60000000000000, 0x7f82, 0x0),
    ),
    g(
        "s_sqrtsd_15_22_1_in",
        "f20f51ca",
        (1, 0x56a5ca30fe2a8bd37fffffffffffffff),
        (2, 0x87005dc196fcec180000000000000001),
        0x3f80,
        0x0,
        ok(0x56a5ca30fe2a8bd31e60000000000000, 0x3f82, 0x0),
    ),
    g(
        "s_sqrtsd_20_22_7_in",
        "f20f51ca",
        (1, 0x29fddead80c02f277ff7ffffffffffff),
        (2, 0x978f46fc566792a30000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_sqrtsd_16_22_7_in",
        "f20f51ca",
        (1, 0x1a687f0870a7b58fff8123456789abc),
        (2, 0x9f0853dd8ce999950000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_sqrtsd_36_24_3_in",
        "f20f51ca",
        (1, 0x1e579e003fcd9b383cb0000000000000),
        (2, 0x4367ccaeaf2e264d000fffffffffffff),
        0x7f80,
        0x0,
        ok(0x1e579e003fcd9b381ffffffffffffffe, 0x7fa2, 0x0),
    ),
    g(
        "s_sqrtsd_34_24_2_in",
        "f20f51ca",
        (1, 0x6897c43c3c44e6597fd0000000000000),
        (2, 0x20cf7410bcee0ef3000fffffffffffff),
        0x5f80,
        0x0,
        ok(0x6897c43c3c44e6591fffffffffffffff, 0x5fa2, 0x0),
    ),
    g(
        "sx_sqrtsd_17_28",
        "f2450f51ce",
        (9, 0xacc2544796aec71ffff7ffffffffffff),
        (14, 0xb15f6fce89fe4d500008000000000000),
        0x4f80,
        0x0,
        fault(0x4fa2),
    ),
    g(
        "sx_sqrtsd_34_13",
        "f2450f51ce",
        (9, 0xdbb0e31da8fab4d30000000000000001),
        (14, 0x720eaa53402b568e000fffffffffffff),
        0x8f80,
        0x0,
        fault(0x8fa2),
    ),
    g(
        "rb_sqrtsd_271",
        "f2410f51ca",
        (1, 0x3ca64ef1ac73bc19f628b44196e97d89),
        (10, 0x432dde798772ed7abcb2ab7b6e836b5e),
        0x3817,
        0x0,
        fault(0x3817),
    ),
    g(
        "rb_sqrtsd_60",
        "f2410f51d3",
        (2, 0x28d452548858d83ff796e25fc79dce),
        (11, 0x800497b0280aec3bbff986a083fa9910),
        0x5e19,
        0x0,
        fault(0x5e19),
    ),
    g(
        "p_sqrtps_69_44",
        "410f51d5",
        (2, 0xffc0000000ffffff7effffff3f93f2d3),
        (13, 0x3055c008ffc00000bf86232b00000003),
        0x1780,
        0x0,
        ok(0x37e9ec52ffc00000ffc000001a9cc471, 0x17a3, 0x0),
    ),
    g(
        "p_sqrtps_0_47",
        "440f51f9",
        (15, 0x7fffff0b4fc49bffbfffffbf800000),
        (1, 0x308c92d563fc0000080000001),
        0x1f80,
        0x0,
        ok(0x1a9cc4712420785c3f9cc471ffc00000, 0x1fa3, 0x0),
    ),
    g(
        "p_sqrtps_69_30",
        "0f51f4",
        (6, 0x400000007f8000003400000040000000),
        (4, 0xffc00000ff8000017f80000040a00000),
        0x1780,
        0x0,
        ok(0xffc00000ffc000017f800000400f1bbd, 0x17a1, 0x0),
    ),
    g(
        "p_sqrtps_66_20",
        "410f51ed",
        (5, 0x74bf5b4effc12345ff7fffff4b800000),
        (13, 0x3fc00000807fffff4b80000040a00000),
        0x1e80,
        0x0,
        ok(0x3f9cc471ffc0000045800000400f1bbd, 0x1ea1, 0x0),
    ),
    g(
        "p_sqrtps_64_15",
        "0f51c7",
        (0, 0xbfc55ed1bf845a2b010601980068b158),
        (7, 0x7fa000004b7fffffffc00000),
        0x5faa,
        0x0,
        ok(0x7fe0000045800000ffc00000, 0x5fab, 0x0),
    ),
    g(
        "p_sqrtps_55_49",
        "0f51e0",
        (4, 0xff7fffff7effffffff80000000000000),
        (0, 0x3b00ac04e8070566780000000),
        0x6fc0,
        0x0,
        ok(0xffc000008000000080000000, 0x6fc1, 0x0),
    ),
    g(
        "p_sqrtps_40_10",
        "410f51f4",
        (6, 0x3f80000001000000ff8000004b7fffff),
        (12, 0xffffff000000017f00000000390d74),
        0x9780,
        0x0,
        ok(0x203504f31a3504f35f3504f31faae978, 0x97a2, 0x0),
    ),
    g(
        "p_sqrtps_40_30",
        "440f51e9",
        (13, 0xb0189b627e800000f31f64277f800000),
        (1, 0xffc1234500000001400000007e800000),
        0x9780,
        0x0,
        ok(0xffc123451a3504f33fb504f35f000000, 0x97a2, 0x0),
    ),
    g(
        "p_sqrtps_72_15",
        "0f51c3",
        (0, 0x7fc000013f00000074c24b757fc00001),
        (3, 0x33800000000000000b15766300e57dd3),
        0x1700,
        0x0,
        ok(0x398000000000000025439b95202b6420, 0x1720, 0x0),
    ),
    g(
        "p_sqrtps_3_23",
        "450f51d3",
        (10, 0xffbfffff7f800000c048558f0b1615bc),
        (11, 0x7fffffff40121e0c7fffffff3fc00000),
        0x7f80,
        0x0,
        ok(0x7fffffff3fc1680b7fffffff3f9cc470, 0x7fa0, 0x0),
    ),
    g(
        "p_sqrtps_64_59",
        "410f51c8",
        (1, 0x3f7fffff7f000000ffc000003f7fffff),
        (8, 0xffc000007e800000ffc000004b800000),
        0x5faa,
        0x0,
        ok(0xffc000005f000000ffc0000045800000, 0x5faa, 0x0),
    ),
    g(
        "p_sqrtps_62_19",
        "440f51f8",
        (15, 0x74a68391f49edb0a00000001cf96d2d6),
        (0, 0xbfdab6940000000030791e283e800000),
        0x1fbf,
        0x0,
        ok(0xffc000000000000037fc89143f000000, 0x1fbf, 0x0),
    ),
    g(
        "p_sqrtps_71_31",
        "410f51f6",
        (6, 0x40037b1b000000017fc0000180ec2749),
        (14, 0x7fffffff808000004b00000000400000),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "p_sqrtps_32_12",
        "410f51e0",
        (4, 0xff7fffff7fffffff7f56b192bfeb0635),
        (8, 0x815723f1c00000004b80000000000001),
        0x9f00,
        0x0,
        fault(0x9f03),
    ),
    g(
        "p_sqrtps_72_12",
        "450f51c3",
        (8, 0x3e8000000073a15c00ffffff30589778),
        (11, 0x7fc0000101000000bf80000080000001),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_sqrtps_26_47",
        "410f51e1",
        (4, 0x7fffffff403a96e17f80a4903fc00000),
        (9, 0x7f800000c0279892013cab4f4b7fffff),
        0x7f00,
        0x0,
        fault(0x7f01),
    ),
    g(
        "p_sqrtps_74_23",
        "450f51e7",
        (12, 0x7effffff800000014f9c7a2c00800000),
        (15, 0x3f800001ffc123453074b90600000003),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "p_sqrtps_74_13",
        "440f51db",
        (11, 0x40a000007f000000cff0ac6bcfaa82ee),
        (3, 0x4b000000008000014b80000000643a0a),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "rb_sqrtps_294",
        "0f51f3",
        (6, 0xcfb04677f4a57e93cfbc58d00b3bf06f),
        (3, 0x400e753f5911c67bbfc54e5a7f5e4d87),
        0x2d85,
        0x0,
        fault(0x2da5),
    ),
    g(
        "rb_sqrtps_180",
        "450f51c7",
        (8, 0x801965240b6d2b607f29da76b0447974),
        (15, 0x7ff02ca430300a0700d858b680737c8a),
        0x6fc0,
        0x0,
        fault(0x6fe0),
    ),
    g(
        "p_sqrtps_61_32",
        "0f51c5",
        (0, 0x802925d93f8000017effffff7f000000),
        (5, 0xf48058cfb01565613f7fffff80800000),
        0x80c0,
        0x0,
        fault(0x80e1),
    ),
    g(
        "p_sqrtps_61_42",
        "410f51ed",
        (5, 0x4b800000749fa7a000000000ff7fffff),
        (13, 0x8000017e800000ff80000134000000),
        0x80c0,
        0x0,
        fault(0x80e1),
    ),
    g(
        "p_sqrtps_70_50",
        "410f51df",
        (3, 0x3e8000007f800001ff800001305d9e8c),
        (15, 0x301000000007fffff74c7535d),
        0xf80,
        0x0,
        fault(0xfa2),
    ),
    g(
        "p_sqrtps_42_56",
        "450f51f0",
        (14, 0x4b7fffff807fffff8092a9b0),
        (8, 0x10000003f000000000bd4787fc00001),
        0x8f80,
        0x0,
        fault(0x8fa2),
    ),
    g(
        "p_sqrtps_73_55",
        "450f51ee",
        (13, 0xff800000400000003fc000007fffffff),
        (14, 0x8098a09600000003f5c452384b000000),
        0x380,
        0x0,
        fault(0x3a3),
    ),
    g(
        "p_sqrtps_73_46",
        "450f51d5",
        (10, 0x7f000000338000004b8000007fa0d0e3),
        (13, 0x7f8b5545ff7fffff007fffff3e800000),
        0x380,
        0x0,
        fault(0x3a3),
    ),
    g(
        "rb_sqrtps_259",
        "450f51e5",
        (12, 0xff0853ad8b2cd024807a0b0dff05c00a),
        (13, 0x14f7ea7f1d5496ffccc9b88b4099f8),
        0xa2a3,
        0x0,
        fault(0xa2a3),
    ),
    g(
        "p_sqrtps_65_1",
        "410f51d2",
        (2, 0x7f7fffff807fffffcfb26b7540000000),
        (10, 0x7fc000007fc00000ff14afe000000001),
        0xfbf,
        0x0,
        fault(0xfbf),
    ),
    g(
        "p_sqrtpd_69_29",
        "66450f51f6",
        (14, 0x3ff00000000000010000000000000001),
        (14, 0x3ff00000000000010000000000000001),
        0x1780,
        0x0,
        ok(0x3ff00000000000001e60000000000000, 0x17a2, 0x0),
    ),
    g(
        "p_sqrtpd_1_9",
        "66450f51fb",
        (15, 0x7fffffffffffffff7fffffffffffffff),
        (11, 0x3ff0000000000001000bdcff381dbda1),
        0x3f80,
        0x0,
        ok(0x3ff00000000000001ffb8df23efa73a0, 0x3fa2, 0x0),
    ),
    g(
        "rb_sqrtpd_12",
        "66450f51f0",
        (14, 0x801b96b271faf8737ff380ee648fbd98),
        (8, 0xffe6a67b4122fc857fe97ab0b9d9e1a6),
        0x1ea8,
        0x0,
        ok(0xfff80000000000005fec8dd91d052f04, 0x1ea9, 0x0),
    ),
    g(
        "p_sqrtpd_19_7",
        "66450f51d8",
        (11, 0x4014000000000000432fe8668cc19643),
        (8, 0x80000000000000018000000000000000),
        0x2f80,
        0x0,
        ok(0xfff80000000000008000000000000000, 0x2f81, 0x0),
    ),
    g(
        "p_sqrtpd_74_37",
        "66410f51c2",
        (0, 0x40080000000000000010000000000001),
        (10, 0x3cb0000000000000001716c22b489c60),
        0x1a00,
        0x0,
        ok(0x3e5000000000000020033869cbc7d48d, 0x1a20, 0x0),
    ),
    g(
        "p_sqrtpd_0_15",
        "660f51c7",
        (0, 0x100000000000010000000000000000),
        (7, 0x7ff80000000000014014000000000000),
        0x1f80,
        0x0,
        ok(0x7ff80000000000014001e3779b97f4a8, 0x1fa0, 0x0),
    ),
    g(
        "p_sqrtpd_50_37",
        "660f51f1",
        (6, 0x433fffffffffffff80211b17d37dbba2),
        (1, 0x7ff6e9b6db5565dc0020000000000000),
        0xfb80,
        0x0,
        ok(0x7ffee9b6db5565dc2006a09e667f3bcc, 0xfba1, 0x0),
    ),
    g(
        "p_sqrtpd_0_12",
        "66410f51d6",
        (2, 0x3ff0000000000001000b4d93fdc33e26),
        (14, 0x7ff40000000000000020000000000000),
        0x1f80,
        0x0,
        ok(0x7ffc0000000000002006a09e667f3bcd, 0x1fa1, 0x0),
    ),
    g(
        "p_sqrtpd_62_55",
        "66410f51f9",
        (7, 0xbffed89851682dca8029de69daba2291),
        (9, 0x80100000000000007ff0000000000001),
        0x1fbf,
        0x0,
        ok(0xfff80000000000007ff8000000000001, 0x1fbf, 0x0),
    ),
    g(
        "p_sqrtpd_42_5",
        "66450f51e7",
        (12, 0xfb7d332c737d0137bffd69e92ae5a3af),
        (15, 0x7ff00000000000007fffffffffffffff),
        0x8f80,
        0x0,
        ok(0x7ff00000000000007fffffffffffffff, 0x8f80, 0x0),
    ),
    g(
        "rb_sqrtpd_149",
        "66410f51f2",
        (6, 0x800deb64742c40d81b733dcb7354a8e5),
        (10, 0xbca05ba1a24a63b500000e80abfa2d7f),
        0x7780,
        0x0,
        ok(0xfff80000000000001fbe7741dee3076d, 0x77a3, 0x0),
    ),
    g(
        "p_sqrtpd_1_14",
        "660f51fc",
        (7, 0x400bab6c64ff36e4),
        (4, 0xc32e963aebd64b1b0002de9274e2ad70),
        0x3f80,
        0x0,
        ok(0xfff80000000000001feb1a5d967ac642, 0x3fa3, 0x0),
    ),
    g(
        "p_sqrtpd_36_11",
        "66450f51d6",
        (10, 0x8005b73658357484c000000000000000),
        (14, 0x1fff8123456789abc),
        0x9d80,
        0x0,
        ok(0x1e60000000000000fff8123456789abc, 0x9d82, 0x0),
    ),
    g(
        "p_sqrtpd_5_11",
        "660f51d1",
        (2, 0xfff00000000000013ff8000000000000),
        (1, 0x17ff8000000000000),
        0x9f80,
        0x0,
        ok(0x1e600000000000007ff8000000000000, 0x9f82, 0x0),
    ),
    g(
        "p_sqrtpd_67_28",
        "66410f51e6",
        (4, 0x44d2615d75e757fd0000000000000),
        (14, 0x1c001661b4da9fa20),
        0x1d80,
        0x0,
        ok(0x1e60000000000000fff8000000000000, 0x1d83, 0x0),
    ),
    g(
        "p_sqrtpd_48_33",
        "66450f51f2",
        (14, 0x7fdfffffffffffffffeab1998e84d04c),
        (10, 0x1c00861780646c74f),
        0xfd80,
        0x0,
        ok(0x1e60000000000000fff8000000000000, 0xfd83, 0x0),
    ),
    g(
        "p_sqrtpd_45_56",
        "660f51c0",
        (0, 0x7ff16e2f231cb6050008000000000000),
        (0, 0x7ff16e2f231cb6050008000000000000),
        0x7f40,
        0x0,
        fault(0x7f41),
    ),
    g(
        "p_sqrtpd_56_22",
        "66450f51f5",
        (14, 0x4006330262f170698011971a73003e8e),
        (13, 0x7ff8000000000001bff9d7ed407ce5fe),
        0x2000,
        0x0,
        fault(0x2001),
    ),
    g(
        "p_sqrtpd_66_7",
        "660f51c0",
        (0, 0x7fffffffffffffff0000000000000001),
        (0, 0x7fffffffffffffff0000000000000001),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_sqrtpd_27_23",
        "66440f51f6",
        (14, 0x7ff4000000000000400348987e71ef50),
        (6, 0xfff80000000000000008000000000000),
        0x7e80,
        0x0,
        fault(0x7e82),
    ),
    g(
        "p_sqrtpd_73_33",
        "66410f51df",
        (3, 0x3cbb298d21f84a65c000000000000000),
        (15, 0x3ff00000000000017fdfffffffffffff),
        0x380,
        0x0,
        fault(0x3a0),
    ),
    g(
        "p_sqrtpd_73_2",
        "66440f51cc",
        (9, 0x37fd450dd77fd3231),
        (4, 0x7fefffffffffffff3ffce6d45b04f7d6),
        0x380,
        0x0,
        fault(0x3a0),
    ),
    g(
        "p_sqrtpd_59_40",
        "66450f51f8",
        (15, 0x80000000000007ff0000000000001),
        (8, 0xfffffffffffffbff07bf6f364f6b8),
        0x8000,
        0x0,
        fault(0x8003),
    ),
    g(
        "p_sqrtpd_32_5",
        "66410f51c6",
        (0, 0x7fe3d3be756ae36e000fffffffffffff),
        (14, 0x801c17e895e46cd40000000000000003),
        0x9f00,
        0x0,
        fault(0x9f03),
    ),
    g(
        "p_sqrtpd_73_27",
        "66450f51d4",
        (10, 0x7fefffffffffffff80010e06ac81f49e),
        (12, 0x1fffffffffffff000a05cabfe7c1d9),
        0x380,
        0x0,
        fault(0x3a2),
    ),
    g(
        "p_sqrtpd_13_30",
        "660f51f6",
        (6, 0x8000000000000433fffffffffffff),
        (6, 0x8000000000000433fffffffffffff),
        0xf80,
        0x0,
        fault(0xfa2),
    ),
    g(
        "p_sqrtpd_70_14",
        "66450f51c9",
        (9, 0xffe5d41e82bb7a4c4008000000000000),
        (9, 0xffe5d41e82bb7a4c4008000000000000),
        0xf80,
        0x0,
        fault(0xfa1),
    ),
    g(
        "p_sqrtpd_61_14",
        "66450f51c6",
        (8, 0xfff80000000000007fd305268f5ffb38),
        (14, 0x3feffffffffffffffff77ac66eb986e3),
        0x80c0,
        0x0,
        fault(0x80e1),
    ),
    g(
        "p_sqrtpd_54_59",
        "66450f51ca",
        (9, 0x200000000000000010000000000001),
        (10, 0x11e224d39c8d78017d1fb5cb0560a),
        0xef80,
        0x0,
        fault(0xefa3),
    ),
    g(
        "p_sqrtpd_19_41",
        "660f51e5",
        (4, 0x3cb00000000000007ff0000000000000),
        (5, 0x802d32eb90a162530000000000000003),
        0x2f80,
        0x0,
        fault(0x2fa3),
    ),
    g(
        "rb_sqrtpd_232",
        "66410f51fc",
        (7, 0xbca94a37a6f425820008adfae09c8c54),
        (12, 0x3ca8e5d6bc7c2039432148c0a62ea9e8),
        0xcfb6,
        0x0,
        fault(0xcfb6),
    ),
    g(
        "p_sqrtpd_65_58",
        "66410f51e6",
        (4, 0x1bcaaf8ceb230d946),
        (14, 0x40080000000000008000000000000001),
        0xfbf,
        0x0,
        fault(0xfbf),
    ),
    g(
        "sx_minss_14_9",
        "f3450f5dce",
        (9, 0xc5d0cf3f566816219e8858f03f800001),
        (14, 0xa519605f559ce0a3414547db3e800000),
        0x5d80,
        0x0,
        ok(0xc5d0cf3f566816219e8858f03e800000, 0x5d80, 0x0),
    ),
    g(
        "s_minss_38_44_2_in",
        "f30f5dca",
        (1, 0xc2e4710448433bab02980f6d4b000000),
        (2, 0xcaa7922815150c14355a611840a00000),
        0x5f80,
        0x0,
        ok(0xc2e4710448433bab02980f6d40a00000, 0x5f80, 0x0),
    ),
    g(
        "s_minss_16_14_5_in",
        "f30f5dca",
        (1, 0xd0c76ccf11eacf9f3041a7a9ffc12345),
        (2, 0x4c75c3d0184a23b63e39f3617fc00001),
        0x9f80,
        0x0,
        ok(0xd0c76ccf11eacf9f3041a7a97fc00001, 0x9f81, 0x0),
    ),
    g(
        "s_minss_4_17_3_in",
        "f30f5dca",
        (1, 0x59e5d6da5ddce17a47fb309c40000000),
        (2, 0xf490880431931c05fad883a67f800001),
        0x7f80,
        0x0,
        ok(0x59e5d6da5ddce17a47fb309c7f800001, 0x7f81, 0x0),
    ),
    g(
        "s_minss_34_15_7_in",
        "f30f5dca",
        (1, 0xee4fe52535ffb6e9a66a50a67e800000),
        (2, 0x567c16a9e6a4bf20a1e3b7777fffffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_minss_63_2",
        "f3450f5dce",
        (9, 0x25ee08f0241d1e362a8c779bffc00000),
        (14, 0x580db2cd76cedff2ccccd58b4b7fffff),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_minss_38_24_5_in",
        "f30f5dca",
        (1, 0xbd08057d718a6593dbff91824b000000),
        (2, 0x85627e18839f25121f92cb26007fffff),
        0x9f80,
        0x0,
        ok(0xbd08057d718a6593dbff9182007fffff, 0x9f82, 0x0),
    ),
    g(
        "s_minss_24_24_2_in",
        "f30f5dca",
        (1, 0xcd9b6101ba10f0e4e7a0fedd007fffff),
        (2, 0x3a494b857a7deb88f4df728f007fffff),
        0x5f80,
        0x0,
        ok(0xcd9b6101ba10f0e4e7a0fedd007fffff, 0x5f82, 0x0),
    ),
    g(
        "s_minss_27_3_7_in",
        "f30f5dca",
        (1, 0xe5d17160521a09d97ce7444500000003),
        (2, 0x2c4f8405dd412b7b43b54cfebf800000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_minss_37_22_7_in",
        "f30f5dca",
        (1, 0x8733b3fcb2807884097aa3c833800000),
        (2, 0xfdd27f5a64bc351b260ff05e00000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_minsd_12_0",
        "f2450f5dce",
        (9, 0x59ee9ad441e862677fefffffffffffff),
        (14, 0xb97cee2a87e2e70dbff0000000000000),
        0x5f00,
        0x0,
        ok(0x59ee9ad441e86267bff0000000000000, 0x5f00, 0x0),
    ),
    g(
        "sx_minsd_11_25",
        "f2450f5dce",
        (9, 0x6490badcdd8299e3cb0000000000000),
        (14, 0x6cdbe93c8f2f8d25001fffffffffffff),
        0x2f80,
        0x0,
        ok(0x6490badcdd8299e001fffffffffffff, 0x2f80, 0x0),
    ),
    g(
        "s_minsd_44_21_4_in",
        "f20f5dca",
        (1, 0xa637c6947a530ecc4014000000000000),
        (2, 0x77d0a0d773f66f51fff7ffffffffffff),
        0x1fc0,
        0x0,
        ok(0xa637c6947a530eccfff7ffffffffffff, 0x1fc1, 0x0),
    ),
    g(
        "s_minsd_14_10_4_in",
        "f20f5dca",
        (1, 0xd9be7405e9d32b5b7ff8000000000001),
        (2, 0xc10115bd9254b4c27ff0000000000000),
        0x1fc0,
        0x0,
        ok(0xd9be7405e9d32b5b7ff0000000000000, 0x1fc1, 0x0),
    ),
    g(
        "sx_minsd_51_2",
        "f2450f5dce",
        (9, 0x817cdf14df4f6432001fffffffffffff),
        (14, 0xffc2edc8aa2291617ff0000000000001),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "s_minsd_11_12_7_in",
        "f20f5dca",
        (1, 0xf2eb7fb2f8f728fcfff0000000000000),
        (2, 0x9e627ced6fa7adaa7ff8000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_minsd_33_23_5_in",
        "f20f5dca",
        (1, 0x49fc309fd9d68217fe0000000000000),
        (2, 0x98ca5158af1caea98000000000000001),
        0x9f80,
        0x0,
        ok(0x49fc309fd9d68218000000000000001, 0x9f82, 0x0),
    ),
    g(
        "s_minsd_24_41_1_in",
        "f20f5dca",
        (1, 0xeb29f001da00baf3000fffffffffffff),
        (2, 0x1faeaab055f4154d7fdfffffffffffff),
        0x3f80,
        0x0,
        ok(0xeb29f001da00baf3000fffffffffffff, 0x3f82, 0x0),
    ),
    g(
        "sx_minsd_1_29",
        "f2450f5dce",
        (9, 0xe95753c99613c3e08000000000000001),
        (14, 0x4c9b4cebdd64b2314000000000000000),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "s_minsd_22_24_7_in",
        "f20f5dca",
        (1, 0x1ae6bf23c5b91c5b0000000000000001),
        (2, 0xab39cf70e168bb35000fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "rb_minps_264",
        "410f5df6",
        (6, 0x405ca7fc00100611017048ee00175817),
        (14, 0x815641e9b041f897c0554ff1bfbfca88),
        0xf80,
        0x0,
        ok(0x815641e9b041f897c0554ff1bfbfca88, 0xf82, 0x0),
    ),
    g(
        "p_minps_24_29",
        "0f5dd0",
        (2, 0xb0742de07f7fffff807fffff7f800000),
        (0, 0x807fffff4b8000003f0000004b800000),
        0x5780,
        0x0,
        ok(0xb0742de04b800000807fffff4b800000, 0x5782, 0x0),
    ),
    g(
        "p_minps_55_46",
        "0f5dd4",
        (2, 0x7fc00001bf800000f4e9e0a500ffffff),
        (4, 0xff800000000000030100000034000000),
        0x6fc0,
        0x0,
        ok(0xff800000bf800000f4e9e0a500ffffff, 0x6fc1, 0x0),
    ),
    g(
        "p_minps_25_29",
        "450f5dc7",
        (8, 0x11dec92808000003f000000ff800000),
        (15, 0x7fc000017f7fffff00ffffff33800000),
        0x4f80,
        0x0,
        ok(0x7fc000018080000000ffffffff800000, 0x4f81, 0x0),
    ),
    g(
        "p_minps_48_0",
        "410f5dc2",
        (0, 0x8000007fc00000004c62e07fc00000),
        (10, 0xbf8ade5000400000f4d0a7ce00c73c65),
        0xfd80,
        0x0,
        ok(0xbf8ade5000400000f4d0a7ce00c73c65, 0xfd83, 0x0),
    ),
    g(
        "p_minps_2_23",
        "440f5de9",
        (13, 0x3f7fffff007fffffbf9b7b1f3f800000),
        (1, 0x340000007f46ddffffbfffff7fbfffff),
        0x5f80,
        0x0,
        ok(0x34000000007fffffffbfffff7fbfffff, 0x5f83, 0x0),
    ),
    g(
        "rb_minps_100",
        "440f5dfe",
        (15, 0x81363d93c019b4f5e72737eb81329e81),
        (6, 0xcfb5bbf0cfe3c7da814ffc377f05bf8a),
        0x6de0,
        0x0,
        ok(0xcfb5bbf0cfe3c7dae72737eb81329e81, 0x6de0, 0x0),
    ),
    g(
        "p_minps_60_59",
        "440f5de8",
        (13, 0xff8000003e8000003e800000c0000000),
        (0, 0x800000007f3aa3a54b8000000075b61f),
        0x40,
        0x0,
        ok(0xff8000003e8000003e800000c0000000, 0x40, 0x0),
    ),
    g(
        "p_minps_57_3",
        "440f5de9",
        (13, 0x8000007fc0000100ffffff3fc00000),
        (1, 0x7fc00000808000003f7fffff0b6a698b),
        0x4000,
        0x0,
        fault(0x4001),
    ),
    g(
        "p_minps_32_13",
        "450f5ded",
        (13, 0x4b800000ffbfffffbf80000074f0e2d8),
        (13, 0x4b800000ffbfffffbf80000074f0e2d8),
        0x9f00,
        0x0,
        fault(0x9f01),
    ),
    g(
        "p_minps_72_43",
        "410f5ddd",
        (3, 0x3f000000400000000025d5b8cfe9b502),
        (13, 0x17f800001340000007fbfffff),
        0x1700,
        0x0,
        fault(0x1703),
    ),
    g(
        "p_minps_66_51",
        "440f5dc5",
        (8, 0x7fc000017f7471f2003f39dc00444be0),
        (5, 0x7f00000000800000ff7fffffbf800000),
        0x1e80,
        0x0,
        fault(0x1e83),
    ),
    g(
        "p_minps_58_7",
        "440f5dc7",
        (8, 0x3f0000004fdcad5700f90fb5f7f2d920),
        (7, 0x8075659534000000007fffff2351c7c9),
        0x6000,
        0x0,
        fault(0x6002),
    ),
    g(
        "p_minps_15_2",
        "410f5df4",
        (6, 0x300ee6b500ffffff00000003ff7fffff),
        (12, 0xae5f1a3fc0000074ec3a5e80000001),
        0x3e80,
        0x0,
        fault(0x3e82),
    ),
    g(
        "p_minpd_63_22",
        "66440f5dd1",
        (10, 0xffefffffffffffff001fffffffffffff),
        (1, 0x3ca84e942c3e45700000000000000003),
        0x1f95,
        0x0,
        ok(0xffefffffffffffff0000000000000003, 0x1f97, 0x0),
    ),
    g(
        "p_minpd_19_7",
        "660f5de6",
        (4, 0x43400000000000004014000000000000),
        (6, 0x3ca0000000000000800e9f694350bcfa),
        0x2f80,
        0x0,
        ok(0x3ca0000000000000800e9f694350bcfa, 0x2f82, 0x0),
    ),
    g(
        "rb_minpd_181",
        "66440f5de0",
        (12, 0x40056042cbeffdf07fd72d0e0d72334a),
        (0, 0x80053f8bd269c7533ff57329acd8ab91),
        0xedc8,
        0x0,
        ok(0x80000000000000003ff57329acd8ab91, 0xedc8, 0x0),
    ),
    g(
        "p_minpd_71_41",
        "66440f5ddf",
        (11, 0xfff00000000000004330000000000000),
        (7, 0x567a5eee95965daf7fefffffffffffff),
        0x1f00,
        0x0,
        ok(0xfff00000000000004330000000000000, 0x1f00, 0x0),
    ),
    g(
        "p_minpd_73_31",
        "66410f5deb",
        (5, 0x9ebfa24ac95c67fd564ddffc54eab),
        (11, 0x7ff7ffffffffffff3ca0000000000000),
        0x380,
        0x0,
        ok(0x7ff7ffffffffffff3ca0000000000000, 0x381, 0x0),
    ),
    g(
        "p_minpd_3_27",
        "66440f5dca",
        (9, 0x7fffffffffffffff0010000000000001),
        (2, 0x100000000000004330000000000000),
        0x7f80,
        0x0,
        ok(0x100000000000000010000000000001, 0x7f81, 0x0),
    ),
    g(
        "rb_minpd_222",
        "66410f5dc8",
        (1, 0xffe543b0a7327dbc000351fbdcef7a81),
        (8, 0x8009581ed7c15bd7fffe444c501c8006),
        0x1f80,
        0x0,
        ok(0xffe543b0a7327dbcfffe444c501c8006, 0x1f83, 0x0),
    ),
    g(
        "p_minpd_36_0",
        "66440f5df7",
        (14, 0x7fdffffffffffffffff0000000000001),
        (7, 0x8001a57fee4db6893ff8000000000000),
        0x9d80,
        0x0,
        ok(0x8001a57fee4db6893ff8000000000000, 0x9d83, 0x0),
    ),
    g(
        "p_minpd_71_13",
        "66440f5de7",
        (12, 0x3ff00000000000007fdfffffffffffff),
        (7, 0xfff80000000000004132872569f235c4),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_minpd_20_30",
        "66410f5de5",
        (4, 0x3ca00000000000007ff0000000000001),
        (13, 0xbff00000000000003ff2f07ea9a3cf38),
        0x5f00,
        0x0,
        fault(0x5f01),
    ),
    g(
        "p_minpd_58_7",
        "66410f5dde",
        (3, 0x10000000000001fff0000000000000),
        (14, 0xfff8000000000000000f6461eec91592),
        0x6000,
        0x0,
        fault(0x6003),
    ),
    g(
        "p_minpd_72_33",
        "66410f5de1",
        (4, 0x7ff7ffffffffffff7fe994fdf5e4cc58),
        (9, 0x38008118aea76daae),
        0x1700,
        0x0,
        fault(0x1703),
    ),
    g(
        "p_minpd_59_55",
        "66450f5ddb",
        (11, 0x800fffffffffffff4008000000000000),
        (11, 0x800fffffffffffff4008000000000000),
        0x8000,
        0x0,
        fault(0x8002),
    ),
    g(
        "p_minpd_46_39",
        "66440f5dc6",
        (8, 0x80000000000000013ffd949fdc486885),
        (6, 0x7fe0000000000000ffefffffffffffff),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "rb_minps_267",
        "0f5dc8",
        (1, 0x7f7a3516006f15a174e9d5900094c72e),
        (0, 0xf4977cc98042c5bf812689b9ff866f09),
        0xb49f,
        0x0,
        fault(0xb49f),
    ),
    g(
        "rb_minps_64",
        "0f5dcb",
        (1, 0xd1fbd2bbcfb05e5fffad17c98162117d),
        (3, 0xffd3d287b0714cd8b03fc75dbfb1b95f),
        0xac01,
        0x0,
        fault(0xac01),
    ),
    g(
        "rb_minpd_281",
        "66410f5de7",
        (4, 0xfff9088aa9f5ccbfc329b240d9ae3110),
        (15, 0x801efee6f791e938800671feae0437eb),
        0xde6f,
        0x0,
        fault(0xde6f),
    ),
    g(
        "rb_minpd_26",
        "66440f5de2",
        (12, 0x5fce3095a21d27ff7cb9402e00d71),
        (2, 0x3ff4a27d17c5a070ffda47752bbd1e7e),
        0x646f,
        0x0,
        fault(0x646f),
    ),
    g(
        "rb_minss_246",
        "f30f5dd3",
        (2, 0x74da81137f2c4cc830705af5c03aeb4e),
        (3, 0x513d24ef8b4a79baffc285da80227de2),
        0xa2aa,
        0x0,
        fault(0xa2aa),
    ),
    g(
        "rb_minss_38",
        "f30f5dc4",
        (0, 0xcfd5d74080cdac850074fab6800b4227),
        (4, 0x74e84c350004bc5d00287bf1bff24cc7),
        0x416,
        0x0,
        fault(0x416),
    ),
    g(
        "rb_minsd_240",
        "f2440f5dc2",
        (8, 0x2face83744640f7ff862e223286639),
        (2, 0x43267dd13aa06a567ffe255db428d893),
        0x4e63,
        0x0,
        fault(0x4e63),
    ),
    g(
        "rb_minsd_75",
        "f20f5ddd",
        (3, 0xffeca71cb3e5542980063ac6b6665b2a),
        (5, 0x47a0d9eb6358dcf03ff7c5c5e749fa5d),
        0x5827,
        0x0,
        fault(0x5827),
    ),
    g(
        "s_maxss_39_35_7_in",
        "f30f5fca",
        (1, 0x33b28f490a6dc54867e2f79b4b7fffff),
        (2, 0x29b35dbc328434586c09c2373e800000),
        0x0,
        0x0,
        ok(0x33b28f490a6dc54867e2f79b4b7fffff, 0x0, 0x0),
    ),
    g(
        "s_maxss_10_27_4_in",
        "f30f5fca",
        (1, 0x2d5a7f90d6bd99913511f25a7f800000),
        (2, 0x87637701821295b3cc67290200000003),
        0x1fc0,
        0x0,
        ok(0x2d5a7f90d6bd99913511f25a7f800000, 0x1fc0, 0x0),
    ),
    g(
        "s_maxss_20_23_6_in",
        "f30f5fca",
        (1, 0x174232bfe9529e356aae9c77fbfffff),
        (2, 0x6b80f280e6045029bc2d43aa80000001),
        0xffc0,
        0x0,
        ok(0x174232bfe9529e356aae9c780000000, 0xffc1, 0x0),
    ),
    g(
        "s_maxss_0_12_6_in",
        "f30f5fca",
        (1, 0x492ef3c9ff6d11ed4e6713ab00000000),
        (2, 0x27e67870fc3ed6d6d4b1002f7fc00000),
        0xffc0,
        0x0,
        ok(0x492ef3c9ff6d11ed4e6713ab7fc00000, 0xffc1, 0x0),
    ),
    g(
        "rb_maxss_106",
        "f3440f5fff",
        (15, 0x80f421647f88befad373d7ed006f2412),
        (7, 0x4fc735e23f9fefac8051f1d3ffed377e),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "s_maxss_18_38_7_in",
        "f30f5fca",
        (1, 0x1e1d953034f675e2a855242dff800001),
        (2, 0xb88db609f845c1a8d3e03a414b000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_maxss_39_27_3_in",
        "f30f5fca",
        (1, 0xcfbee25d2157405f61c4d7164b7fffff),
        (2, 0x59c0728536338529ff3b049800000003),
        0x7f80,
        0x0,
        ok(0xcfbee25d2157405f61c4d7164b7fffff, 0x7f82, 0x0),
    ),
    g(
        "s_maxss_23_23_1_in",
        "f30f5fca",
        (1, 0x5ab0235e49033614b9e169a080000001),
        (2, 0x6654d027ab62751a3ccb6be80000001),
        0x3f80,
        0x0,
        ok(0x5ab0235e49033614b9e169a080000001, 0x3f82, 0x0),
    ),
    g(
        "sx_maxss_38_10",
        "f3450f5fce",
        (9, 0x977cfe9ba91f04dab3fb47253e800000),
        (14, 0xeccbdf41530c79c7191f96d800000001),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "s_maxss_35_22_7_in",
        "f30f5fca",
        (1, 0xe5b244ef1dab2a6a35e2ba1a3e800000),
        (2, 0xaba97b7e304a48661c57bf0700000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_maxsd_42_6_7_in",
        "f20f5fca",
        (1, 0xeef11f111884df120020000000000000),
        (2, 0xa95e6a9d9e41dd634008000000000000),
        0x0,
        0x0,
        ok(0xeef11f111884df124008000000000000, 0x0, 0x0),
    ),
    g(
        "sx_maxsd_24_3",
        "f2450f5fce",
        (9, 0x864b28e093746e4f4008000000000000),
        (14, 0xecc2f21fe56c2f9fc000000000000000),
        0x9f00,
        0x0,
        ok(0x864b28e093746e4f4008000000000000, 0x9f00, 0x0),
    ),
    g(
        "s_maxsd_21_0_5_in",
        "f20f5fca",
        (1, 0x2d65e6ba15919b88fff7ffffffffffff),
        (2, 0x83aecf4fccd764b20000000000000000),
        0x9f80,
        0x0,
        ok(0x2d65e6ba15919b880000000000000000, 0x9f81, 0x0),
    ),
    g(
        "s_maxsd_15_0_0_in",
        "f20f5fca",
        (1, 0xc9dc4168d43702b87fffffffffffffff),
        (2, 0xe7aeb9ce9ae903b30000000000000000),
        0x1f80,
        0x0,
        ok(0xc9dc4168d43702b80000000000000000, 0x1f81, 0x0),
    ),
    g(
        "s_maxsd_41_19_7_in",
        "f20f5fca",
        (1, 0x557f9c29fa346c637fdfffffffffffff),
        (2, 0xd935784cd33cc5867ff4000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_maxsd_22_16_7_in",
        "f20f5fca",
        (1, 0xb97acc72bb25de8f0000000000000001),
        (2, 0x1e8adc3711dd3cb9fff8123456789abc),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_maxsd_6_1",
        "f2450f5fce",
        (9, 0x62a71be6ad08a2a80000000000000003),
        (14, 0xfcadd3763db6737f0020000000000000),
        0x3f00,
        0x0,
        ok(0x62a71be6ad08a2a80020000000000000, 0x3f02, 0x0),
    ),
    g(
        "s_maxsd_22_39_1_in",
        "f20f5fca",
        (1, 0x54994b1d50ac92a90000000000000001),
        (2, 0x898d9a60fd01f217433fffffffffffff),
        0x3f80,
        0x0,
        ok(0x54994b1d50ac92a9433fffffffffffff, 0x3f82, 0x0),
    ),
    g(
        "s_maxsd_38_25_7_in",
        "f20f5fca",
        (1, 0x8dd41ee656c3f4a84330000000000000),
        (2, 0x387f5d4e1e589ae3800fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_maxsd_25_30_7_in",
        "f20f5fca",
        (1, 0xb427dd9f3e42c0f8800fffffffffffff),
        (2, 0x9db59ff114cf07070010000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "p_maxps_70_7",
        "0f5fe3",
        (4, 0x7f80000100fe4c50807fffff7f800001),
        (3, 0x80350c4d8b1447cbc00000003fc00000),
        0xf80,
        0x0,
        ok(0x80350c4d00fe4c50807fffff3fc00000, 0xf83, 0x0),
    ),
    g(
        "p_maxps_28_31",
        "410f5fc6",
        (0, 0x4b0000004b0000007f8000004b800000),
        (14, 0x807fffff80c000bd3fc00000ffc00000),
        0x7d80,
        0x0,
        ok(0x4b0000004b0000007f800000ffc00000, 0x7d83, 0x0),
    ),
    g(
        "p_maxps_61_26",
        "0f5ff2",
        (6, 0x404000007f0000004b0000007fbfffff),
        (2, 0xc067a3b73e80000000ffffff00800000),
        0x80c0,
        0x0,
        ok(0x404000007f0000004b00000000800000, 0x80c1, 0x0),
    ),
    g(
        "p_maxps_2_19",
        "410f5ff9",
        (7, 0x3f7fffffb064449b808000007fc00001),
        (9, 0x7e8000007fa00000c05b255c34000000),
        0x5f80,
        0x0,
        ok(0x7e8000007fa000008080000034000000, 0x5f81, 0x0),
    ),
    g(
        "p_maxps_69_55",
        "440f5fe1",
        (12, 0x3f000000008000003380000034000000),
        (1, 0xffffff3f8000004b80000000000001),
        0x1780,
        0x0,
        ok(0x3f0000003f8000004b80000034000000, 0x1782, 0x0),
    ),
    g(
        "p_maxps_5_41",
        "440f5ff1",
        (14, 0x76adf253402877d93f7fffff80000000),
        (1, 0x7f000000002a93f3007fffff3f000000),
        0x9f80,
        0x0,
        ok(0x7f000000402877d93f7fffff3f000000, 0x9f82, 0x0),
    ),
    g(
        "rb_maxps_238",
        "450f5fdf",
        (11, 0x7f750171c070d6bd811a134a01472372),
        (15, 0xb9877ebfb1f48c7f4df3d43fdb2919),
        0x3e80,
        0x0,
        ok(0x7f750171bfb1f48c7f4df3d43fdb2919, 0x3e80, 0x0),
    ),
    g(
        "p_maxps_49_45",
        "440f5fcd",
        (9, 0x7f800000007fffff8000000100000001),
        (5, 0x7e800000bf8000003f7fffff3f7fffff),
        0x7dc0,
        0x0,
        ok(0x7f800000000000003f7fffff3f7fffff, 0x7dc0, 0x0),
    ),
    g(
        "rb_maxps_70",
        "450f5fd2",
        (10, 0x3ca04b01115346b0123233007c3083),
        (10, 0x3ca04b01115346b0123233007c3083),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_maxps_46_25",
        "450f5fda",
        (11, 0xff7fffff300aa97a7effffff80402387),
        (10, 0x7effffff000000007f00000000000001),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "p_maxps_71_32",
        "450f5fc5",
        (8, 0x4e193fc00000003f000000ffc00000),
        (13, 0x80d25990005638627fc0000174b9a11c),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "p_maxps_9_27",
        "0f5ffb",
        (7, 0x7fc000018b5b440a0b7d0a61cff8ff32),
        (3, 0x8b283a913f800001004af06e00800001),
        0x1e80,
        0x0,
        fault(0x1e83),
    ),
    g(
        "p_maxps_71_54",
        "450f5fdb",
        (11, 0x3071658f74b014d64fe6e5337fffffff),
        (11, 0x3071658f74b014d64fe6e5337fffffff),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_maxps_33_4",
        "440f5fe1",
        (12, 0xffffff000000013f80000080e0028b),
        (1, 0xffc00000ff075a8a7fadf3053fc00000),
        0x1f40,
        0x0,
        fault(0x1f41),
    ),
    g(
        "p_maxpd_67_42",
        "660f5fc4",
        (0, 0xfff80000000000007ffd084f67d40446),
        (4, 0x30010000000000000),
        0x1d80,
        0x0,
        ok(0x30010000000000000, 0x1d81, 0x0),
    ),
    g(
        "p_maxpd_50_0",
        "66450f5ffd",
        (15, 0x3cb0fe8f13e7235d3ca0000000000000),
        (13, 0x7ff7fffffffffffffffab0a7473b1b63),
        0xfb80,
        0x0,
        ok(0x7ff7fffffffffffffffab0a7473b1b63, 0xfb81, 0x0),
    ),
    g(
        "p_maxpd_62_35",
        "66450f5fdc",
        (11, 0xbff0000000000000c00325e45f551b36),
        (12, 0xc0000000000000007fd0000000000000),
        0x1fbf,
        0x0,
        ok(0xbff00000000000007fd0000000000000, 0x1fbf, 0x0),
    ),
    g(
        "p_maxpd_64_10",
        "66450f5ffa",
        (15, 0x80000000000000013fe0000000000000),
        (10, 0x7fd72e4ca6c91a923ca0000000000000),
        0x5faa,
        0x0,
        ok(0x7fd72e4ca6c91a923fe0000000000000, 0x5faa, 0x0),
    ),
    g(
        "p_maxpd_70_14",
        "66410f5fc4",
        (0, 0x8000000000000000800fffffffffffff),
        (12, 0x80000000000000013cb0000000000000),
        0xf80,
        0x0,
        ok(0x80000000000000003cb0000000000000, 0xf82, 0x0),
    ),
    g(
        "p_maxpd_26_44",
        "660f5ff2",
        (6, 0xc0000000000000008010000000000000),
        (2, 0x17fdfffffffffffff),
        0x7f00,
        0x0,
        ok(0x17fdfffffffffffff, 0x7f02, 0x0),
    ),
    g(
        "p_maxpd_50_48",
        "660f5fcc",
        (1, 0xfff7ffffffffffff000fffffffffffff),
        (4, 0x7fe00000000000007fd0000000000000),
        0xfb80,
        0x0,
        ok(0x7fe00000000000007fd0000000000000, 0xfb83, 0x0),
    ),
    g(
        "p_maxpd_16_38",
        "660f5fc6",
        (0, 0x3ff61fa32d142b4b7ff8000000000001),
        (6, 0xfffffffffffff7ff0000000000001),
        0x3d80,
        0x0,
        ok(0x3ff61fa32d142b4b7ff0000000000001, 0x3d83, 0x0),
    ),
    g(
        "p_maxpd_58_37",
        "66450f5fd4",
        (10, 0xfff80000000000003ca0000000000000),
        (12, 0xc000000000000000fff0000000000001),
        0x6000,
        0x0,
        fault(0x6001),
    ),
    g(
        "p_maxpd_7_12",
        "66450f5fd3",
        (10, 0xfff7ffffffffffff8010000000000000),
        (11, 0x7ff40000000000007fefffffffffffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "p_maxpd_66_10",
        "66450f5fde",
        (11, 0x3ff80000000000008009efb37223fb0c),
        (14, 0x7fefffffffffffff7ff0000000000000),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_maxpd_21_54",
        "66440f5fc7",
        (8, 0xffe4a859ce5f72910000000000000001),
        (7, 0x3ff0527b5441acc0),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "p_maxpd_71_9",
        "66440f5ff2",
        (14, 0xc007be72754eea1e0010000000000001),
        (2, 0xfff61bb3a5f5d92f0008000000000000),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "p_maxpd_9_45",
        "660f5fc3",
        (0, 0xfff7ffffffffffff3fd0000000000000),
        (3, 0x7fd00000000000000000000000000001),
        0x1e80,
        0x0,
        fault(0x1e83),
    ),
    g(
        "rb_maxps_282",
        "440f5fde",
        (11, 0xb07f46153fc6a1f5ff83b4deb06da1dc),
        (6, 0xf4b12dc90112f7580046639b74b9e9ab),
        0xea21,
        0x0,
        fault(0xea21),
    ),
    g(
        "rb_maxps_218",
        "410f5fd7",
        (2, 0x7fa38a5e3ff003e08116dc4d4fa5e7b8),
        (15, 0xff0869d87ffcba9eff6432b0f4f8cd46),
        0x157,
        0x0,
        fault(0x157),
    ),
    g(
        "rb_maxpd_190",
        "66450f5ffa",
        (15, 0xc0017b78b33f7f573ff54e29f8bbf790),
        (10, 0xfffb3bca8e3e12a7ffee698a220e1ff4),
        0xad23,
        0x0,
        fault(0xad23),
    ),
    g(
        "rb_maxpd_74",
        "660f5ff0",
        (6, 0x7fe57853936191de3ca8d14c8b1c3577),
        (0, 0xbff57360e357d80d000925d358925bb3),
        0x8213,
        0x0,
        fault(0x8213),
    ),
    g(
        "rb_maxss_251",
        "f30f5ffc",
        (7, 0x4f92ee338b704e8800d4ed50ffc6d660),
        (4, 0x7fbc695c3fb30de30027ed7452af503c),
        0xb921,
        0x0,
        fault(0xb921),
    ),
    g(
        "rb_maxss_224",
        "f3440f5fd2",
        (10, 0x3f97bedb005355d6f4f09eabff8c5d7c),
        (2, 0xff0e1f9f8b33e561867fff4f74864a0e),
        0xfe61,
        0x0,
        fault(0xfe61),
    ),
    g(
        "rb_maxsd_181",
        "f2450f5fd4",
        (10, 0x3ca9d0f8ae3c38a6fff2cfb5b2c8daf4),
        (12, 0xfff2c0a258c40b9effff1057b156d902),
        0xe659,
        0x0,
        fault(0xe659),
    ),
    g(
        "rb_maxsd_280",
        "f20f5fc9",
        (1, 0x8018dffa605785fd80080dced572edf8),
        (1, 0x8018dffa605785fd80080dced572edf8),
        0xee93,
        0x0,
        fault(0xee93),
    ),
    g(
        "s_comisss_42_42_6_in",
        "0f2fca",
        (1, 0x70a7518bce1de54ba966744c01000000),
        (2, 0x5b2434e802f57ff4bdb571db01000000),
        0xffc0,
        0xcd5,
        ok(0x70a7518bce1de54ba966744c01000000, 0xffc0, 0x40),
    ),
    g(
        "s_comisss_29_29_4_in",
        "0f2fca",
        (1, 0x919bb9c71c5c4c5b7275a97d80800000),
        (2, 0xf0ce07dcb7f5b93c166e863a80800000),
        0x1fc0,
        0x0,
        ok(0x919bb9c71c5c4c5b7275a97d80800000, 0x1fc0, 0x40),
    ),
    g(
        "s_comisss_37_39_6_in",
        "0f2fca",
        (1, 0x818dc308c1f9586340eabbd033800000),
        (2, 0xa505b55db9bc8d30042f68734b7fffff),
        0xffc0,
        0x8d5,
        ok(0x818dc308c1f9586340eabbd033800000, 0xffc0, 0x1),
    ),
    g(
        "s_comisss_28_8_6_in",
        "0f2fca",
        (1, 0x81148fefc9b8fab0b67d5d3100800000),
        (2, 0x4ea2508437f8a4218344f51e3f800001),
        0xffc0,
        0x8d5,
        ok(0x81148fefc9b8fab0b67d5d3100800000, 0xffc0, 0x1),
    ),
    g(
        "s_comisss_34_8_7_in",
        "0f2fca",
        (1, 0x17496e5165fc7de1468cfc497e800000),
        (2, 0xb5f5b717c14272c0c33d499e3f800001),
        0x0,
        0x8d5,
        ok(0x17496e5165fc7de1468cfc497e800000, 0x0, 0x0),
    ),
    g(
        "s_comisss_6_3_4_in",
        "0f2fca",
        (1, 0x20dafbb0a6c1c53a10dd307940400000),
        (2, 0x1706ab62c8e66474d7e66286bf800000),
        0x1fc0,
        0x0,
        ok(0x20dafbb0a6c1c53a10dd307940400000, 0x1fc0, 0x0),
    ),
    g(
        "sx_comisss_45_21",
        "450f2fce",
        (9, 0xaebdff9a5bcb9fbaa1d75cdd7fbfffff),
        (14, 0xf6725015616c53196026f1f0bf800000),
        0x77c0,
        0x0,
        ok(0xaebdff9a5bcb9fbaa1d75cdd7fbfffff, 0x77c1, 0x45),
    ),
    g(
        "s_comisss_22_14_4_in",
        "0f2fca",
        (1, 0xd18f412e623e185c9abd19b800000001),
        (2, 0x2262ffee90221c07667a9d3a7fc00001),
        0x1fc0,
        0x0,
        ok(0xd18f412e623e185c9abd19b800000001, 0x1fc1, 0x45),
    ),
    g(
        "sx_comisss_50_20",
        "450f2fce",
        (9, 0xe22ead31c2447ed41f9adcea7fa00000),
        (14, 0x27d429f61375d5b1cdd6fc3500ffffff),
        0x6000,
        0x0,
        fault(0x6001),
    ),
    g(
        "s_comisss_8_14_7_in",
        "0f2fca",
        (1, 0xefca57b484f252e8667ddc8d3f800001),
        (2, 0x407f643c2a5b3077eeee2b727fc00001),
        0x0,
        0xcd5,
        fault(0x1),
    ),
    g(
        "s_comisss_26_41_2_in",
        "0f2fca",
        (1, 0xb6295657c82c137e87a7b70c00400000),
        (2, 0xda6bed0c572dca156fe2a4497effffff),
        0x5f80,
        0xcd5,
        ok(0xb6295657c82c137e87a7b70c00400000, 0x5f82, 0x1),
    ),
    g(
        "s_comisss_1_26_0_in",
        "0f2fca",
        (1, 0xe6ad4f30f3ebc75018dae45680000000),
        (2, 0x76ab6d8eb63772e50688941500400000),
        0x1f80,
        0xcd5,
        ok(0xe6ad4f30f3ebc75018dae45680000000, 0x1f82, 0x1),
    ),
    g(
        "rb_comisss_181",
        "450f2fd3",
        (10, 0x80487a30ff1e6e8fc03143620067fd88),
        (11, 0x7f512d0300770339807e77998046492d),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "sx_comisss_1_22",
        "450f2fce",
        (9, 0x48626a3cbafd7355d6ffcd14007fffff),
        (14, 0x20eeb9c372d61111d1f025fb00000001),
        0x1e80,
        0xcd5,
        fault(0x1e82),
    ),
    g(
        "sx_comisss_46_25",
        "450f2fce",
        (9, 0x5ccf5a11d8b0528d347092a13e800000),
        (14, 0x4044b902284f59317f6f66e100400000),
        0xef80,
        0xcd5,
        ok(0x5ccf5a11d8b0528d347092a13e800000, 0xef82, 0x0),
    ),
    g(
        "s_comisss_22_5_3_in",
        "0f2fca",
        (1, 0x9f776fbdd8f8651953c3368300000001),
        (2, 0xfa64c7abad6fd472c5b672c0c0000000),
        0x7f80,
        0xcd5,
        ok(0x9f776fbdd8f8651953c3368300000001, 0x7f82, 0x0),
    ),
    g(
        "sx_comisss_32_8",
        "450f2fce",
        (9, 0x718d3fda1ed41780f60b91e800000001),
        (14, 0x61dc8c8e7c25c9cde4dc743900000001),
        0x9780,
        0xcd5,
        ok(0x718d3fda1ed41780f60b91e800000001, 0x9782, 0x40),
    ),
    g(
        "s_comisss_22_22_2_in",
        "0f2fca",
        (1, 0x22f958e72f8ebac05b4660d000000001),
        (2, 0x6ce81301afaf0b5cb1d84c8a00000001),
        0x5f80,
        0x0,
        ok(0x22f958e72f8ebac05b4660d000000001, 0x5f82, 0x40),
    ),
    g(
        "rb_comisss_14",
        "410f2fcb",
        (1, 0x26c891ff680c8d0078a6cbff91eb64),
        (11, 0xff7652fe4037bc25cfc58bfcb069fdd3),
        0xd7f7,
        0x0,
        ok(0x26c891ff680c8d0078a6cbff91eb64, 0xd7f7, 0x45),
    ),
    g(
        "sx_comisss_54_18",
        "450f2fce",
        (9, 0x858f998cbd9997fcb9bd465300800000),
        (14, 0x60fb6f2fe14a4434e3e24c3c7fa00000),
        0x1fbf,
        0xcd5,
        ok(0x858f998cbd9997fcb9bd465300800000, 0x1fbf, 0x45),
    ),
    g(
        "s_comissd_43_43_7_in",
        "660f2fca",
        (1, 0xb15663d971ba5b27001fffffffffffff),
        (2, 0x2e4b94e4b8e068ab001fffffffffffff),
        0x0,
        0xcd5,
        ok(0xb15663d971ba5b27001fffffffffffff, 0x0, 0x40),
    ),
    g(
        "s_comissd_0_0_4_in",
        "660f2fca",
        (1, 0x53134b53d33511e50000000000000000),
        (2, 0x96e4c9cd88639a090000000000000000),
        0x1fc0,
        0xcd5,
        ok(0x53134b53d33511e50000000000000000, 0x1fc0, 0x40),
    ),
    g(
        "sx_comissd_17_10",
        "66450f2fce",
        (9, 0xd7782d43a0e286963fd0000000000000),
        (14, 0x63987b6d8a5988e7fefffffffffffff),
        0x4f80,
        0x0,
        ok(0xd7782d43a0e286963fd0000000000000, 0x4f80, 0x1),
    ),
    g(
        "s_comissd_36_41_2_in",
        "660f2fca",
        (1, 0x5bbe5e0b2654ecf83cb0000000000000),
        (2, 0x96b8dd030d5bf5877fdfffffffffffff),
        0x5f80,
        0x0,
        ok(0x5bbe5e0b2654ecf83cb0000000000000, 0x5f80, 0x1),
    ),
    g(
        "rb_comissd_215",
        "66450f2ff8",
        (15, 0xbcb7744f7eef44e67fda6d5bc1ef3dad),
        (8, 0xffe39452727fd541c00374320c653302),
        0x5d80,
        0x0,
        ok(0xbcb7744f7eef44e67fda6d5bc1ef3dad, 0x5d80, 0x0),
    ),
    g(
        "s_comissd_31_2_6_in",
        "660f2fca",
        (1, 0xf2704dc61eff48c37fefffffffffffff),
        (2, 0x98759b22a1dfcb1f3ff0000000000000),
        0xffc0,
        0x8d5,
        ok(0xf2704dc61eff48c37fefffffffffffff, 0xffc0, 0x0),
    ),
    g(
        "sx_comissd_26_11",
        "66450f2fce",
        (9, 0x117f7d01781a4d727ff7ffffffffffff),
        (14, 0xff1d47b8dc8d24d2fff8123456789abc),
        0x9e80,
        0xcd5,
        ok(0x117f7d01781a4d727ff7ffffffffffff, 0x9e81, 0x45),
    ),
    g(
        "s_comissd_28_21_5_in",
        "660f2fca",
        (1, 0xbb9c1b114539c0a70010000000000000),
        (2, 0x7f6d130fd608203efff7ffffffffffff),
        0x9f80,
        0x8d5,
        ok(0xbb9c1b114539c0a70010000000000000, 0x9f81, 0x45),
    ),
    g(
        "sx_comissd_12_1",
        "66450f2fce",
        (9, 0x4a5fd51cbdeea700fff8000000000000),
        (14, 0xf02f057b049ad6563ca0000000000000),
        0x5f00,
        0x8d5,
        fault(0x5f01),
    ),
    g(
        "s_comissd_8_12_7_in",
        "660f2fca",
        (1, 0x1274cb2c5ba3da633ff0000000000001),
        (2, 0xa1b7da3cba8a57bf7ff8000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_comissd_32_27_5_in",
        "660f2fca",
        (1, 0x3e4eb9cba93b1bf1ffefffffffffffff),
        (2, 0xa939bb648915b7190000000000000003),
        0x9f80,
        0x0,
        ok(0x3e4eb9cba93b1bf1ffefffffffffffff, 0x9f82, 0x1),
    ),
    g(
        "s_comissd_0_27_1_in",
        "660f2fca",
        (1, 0xd0e31f2c35b45e1b0000000000000000),
        (2, 0xce91d035829bcd560000000000000003),
        0x3f80,
        0x8d5,
        ok(0xd0e31f2c35b45e1b0000000000000000, 0x3f82, 0x1),
    ),
    g(
        "s_comissd_30_25_7_in",
        "660f2fca",
        (1, 0x43b45c746e3b61950010000000000001),
        (2, 0x1556c1eebd79a141800fffffffffffff),
        0x0,
        0xcd5,
        fault(0x2),
    ),
    g(
        "s_comissd_6_26_7_in",
        "660f2fca",
        (1, 0x4a1029ca750cb8294008000000000000),
        (2, 0x6cf100ecfd9afd3a0008000000000000),
        0x0,
        0x8d5,
        fault(0x2),
    ),
    g(
        "s_comissd_35_23_3_in",
        "660f2fca",
        (1, 0x45f8c42a089d8d5e3fd0000000000000),
        (2, 0x8cb61af149738f968000000000000001),
        0x7f80,
        0x0,
        ok(0x45f8c42a089d8d5e3fd0000000000000, 0x7f82, 0x0),
    ),
    g(
        "s_comissd_36_23_2_in",
        "660f2fca",
        (1, 0xc1ef44c51855b6e63cb0000000000000),
        (2, 0x1e8c17f3147fc5c38000000000000001),
        0x5f80,
        0x0,
        ok(0xc1ef44c51855b6e63cb0000000000000, 0x5f82, 0x0),
    ),
    g(
        "sx_comissd_60_10",
        "66450f2fce",
        (9, 0x21141e494cffe46800fffffffffffff),
        (14, 0x3aac56d281d3f01f800fffffffffffff),
        0x1b80,
        0xcd5,
        ok(0x21141e494cffe46800fffffffffffff, 0x1b82, 0x40),
    ),
    g(
        "s_comissd_22_22_1_in",
        "660f2fca",
        (1, 0x3b5a43f320b134f70000000000000001),
        (2, 0xb8bb013a2dc1b8e80000000000000001),
        0x3f80,
        0x0,
        ok(0x3b5a43f320b134f70000000000000001, 0x3f82, 0x40),
    ),
    g(
        "sx_comissd_57_21",
        "66450f2fce",
        (9, 0x9ac891e7b7f22fab7ff8000000000001),
        (14, 0xe26935e6c5458f6e4340000000000000),
        0xfbf,
        0xcd5,
        ok(0x9ac891e7b7f22fab7ff8000000000001, 0xfbf, 0x45),
    ),
    g(
        "sx_comissd_57_22",
        "66450f2fce",
        (9, 0xf5f2d8ca4af89ca1fff7ffffffffffff),
        (14, 0xfe3df3b129512aa0008000000000000),
        0xfbf,
        0x8d5,
        ok(0xf5f2d8ca4af89ca1fff7ffffffffffff, 0xfbf, 0x45),
    ),
    g(
        "rb_comisss_262",
        "450f2fd9",
        (11, 0xf4b1cc14015155dd7fa9605a40113ace),
        (9, 0x3c0de2f4f5f2a100674513ff8a2890),
        0xd019,
        0x8d5,
        fault(0xd019),
    ),
    g(
        "rb_comisss_188",
        "410f2fd8",
        (3, 0x4fa9d363800abd7bcfa909407fbb1dcd),
        (8, 0x4613d56320ecb9f4e3dd76b07807df),
        0xc855,
        0xcd5,
        fault(0xc855),
    ),
    g(
        "rb_comissd_199",
        "660f2fdc",
        (3, 0xcfcbed2192a6d000ff3b41ba6c853),
        (4, 0xffee9b344fd4fb713cbac1c8658849bb),
        0xb01f,
        0xcd5,
        fault(0xb01f),
    ),
    g(
        "rb_comissd_95",
        "66410f2fd5",
        (2, 0x61711493be0dd31c7fece298527bbb91),
        (13, 0xffea67aa349541f980096ed1b965b67f),
        0xc83f,
        0x0,
        fault(0xc83f),
    ),
    g(
        "s_ucomisss_30_30_7_in",
        "0f2eca",
        (1, 0x37dc6850dc9bb79e4bbfb47c00800001),
        (2, 0x83988b3db7542c0b13b5e81300800001),
        0x0,
        0x0,
        ok(0x37dc6850dc9bb79e4bbfb47c00800001, 0x0, 0x40),
    ),
    g(
        "s_ucomisss_1_24_4_in",
        "0f2eca",
        (1, 0x819e93ed60dbe33bbf92e92d80000000),
        (2, 0x9a9e636e623528ff1494308e007fffff),
        0x1fc0,
        0xcd5,
        ok(0x819e93ed60dbe33bbf92e92d80000000, 0x1fc0, 0x40),
    ),
    g(
        "s_ucomisss_37_45_7_in",
        "0f2eca",
        (1, 0xc502423d7bc7cbf3c54c8c8f33800000),
        (2, 0x37002261cb7328c1a604e3db3fc00000),
        0x0,
        0xcd5,
        ok(0xc502423d7bc7cbf3c54c8c8f33800000, 0x0, 0x1),
    ),
    g(
        "s_ucomisss_27_39_4_in",
        "0f2eca",
        (1, 0xf6a23f936e0e40d01eacc24300000003),
        (2, 0x47e144b4eff47662cc4eccd44b7fffff),
        0x1fc0,
        0x8d5,
        ok(0xf6a23f936e0e40d01eacc24300000003, 0x1fc0, 0x1),
    ),
    g(
        "sx_ucomisss_55_9",
        "450f2ece",
        (9, 0x987e1def833c8fe7485f419d3e800000),
        (14, 0x87a8f0e4559b9930a04a16ab00000000),
        0x1f95,
        0x8d5,
        ok(0x987e1def833c8fe7485f419d3e800000, 0x1f95, 0x0),
    ),
    g(
        "s_ucomisss_36_28_6_in",
        "0f2eca",
        (1, 0x307d902d6f3c823a0812385734000000),
        (2, 0x107488cf54585ee814c624600800000),
        0xffc0,
        0x8d5,
        ok(0x307d902d6f3c823a0812385734000000, 0xffc0, 0x0),
    ),
    g(
        "s_ucomisss_44_16_7_in",
        "0f2eca",
        (1, 0x887a49f4b519b0d1ca64dd3d40a00000),
        (2, 0xe4d4d30e4349a6170510dadbffc12345),
        0x0,
        0x8d5,
        ok(0x887a49f4b519b0d1ca64dd3d40a00000, 0x0, 0x45),
    ),
    g(
        "s_ucomisss_15_40_6_in",
        "0f2eca",
        (1, 0x1cb5cc7525d3cedff0d81d2a7fffffff),
        (2, 0x13f739aa2bd792b4d2d7c30f4b800000),
        0xffc0,
        0x8d5,
        ok(0x1cb5cc7525d3cedff0d81d2a7fffffff, 0xffc0, 0x45),
    ),
    g(
        "s_ucomisss_22_17_2_in",
        "0f2eca",
        (1, 0x84e68dbf23c99fcc447f6b6e00000001),
        (2, 0xf493bbfa4ebbad5b650440227f800001),
        0x5f80,
        0xcd5,
        ok(0x84e68dbf23c99fcc447f6b6e00000001, 0x5f81, 0x45),
    ),
    g(
        "s_ucomisss_1_18_2_in",
        "0f2eca",
        (1, 0x19d116b9206ab965ac6650f880000000),
        (2, 0x804207b11e186354874b9abbff800001),
        0x5f80,
        0xcd5,
        ok(0x19d116b9206ab965ac6650f880000000, 0x5f81, 0x45),
    ),
    g(
        "sx_ucomisss_63_0",
        "450f2ece",
        (9, 0x7e78e65aa768e77cdab7aa6f7f800001),
        (14, 0x18f7a61997b00feb7b8f12cf80800000),
        0x1f00,
        0x8d5,
        fault(0x1f01),
    ),
    g(
        "s_ucomisss_19_41_7_in",
        "0f2eca",
        (1, 0x71d526df7d08c6a03fa3ab6e7fa00000),
        (2, 0x37784f3e56d279e1d523d5117effffff),
        0x0,
        0x8d5,
        fault(0x1),
    ),
    g(
        "sx_ucomisss_22_13",
        "450f2ece",
        (9, 0x5f4a4603e09e7360780c95d2c0000000),
        (14, 0x3f32c27e7346b523290b09c600000001),
        0x7780,
        0x8d5,
        ok(0x5f4a4603e09e7360780c95d2c0000000, 0x7782, 0x1),
    ),
    g(
        "s_ucomisss_0_22_5_in",
        "0f2eca",
        (1, 0xc29b2c7ddbb15de9937f774900000000),
        (2, 0xd8f02720833b536e599211df00000001),
        0x9f80,
        0x0,
        ok(0xc29b2c7ddbb15de9937f774900000000, 0x9f82, 0x1),
    ),
    g(
        "sx_ucomisss_51_12",
        "450f2ece",
        (9, 0x36e528018226b67379cc54e807fffff),
        (14, 0x2a0671aff48ef201aa72559000000003),
        0x8000,
        0x8d5,
        fault(0x8002),
    ),
    g(
        "s_ucomisss_26_40_7_in",
        "0f2eca",
        (1, 0x1925dd52f45682f34557845d00400000),
        (2, 0x97d342788a3bf3f6b964a6f34b800000),
        0x0,
        0x8d5,
        fault(0x2),
    ),
    g(
        "s_ucomisss_36_27_3_in",
        "0f2eca",
        (1, 0x35cd0cd16d34f240cc8c9fa34000000),
        (2, 0x5dae704fdd95681e6509a8f800000003),
        0x7f80,
        0x0,
        ok(0x35cd0cd16d34f240cc8c9fa34000000, 0x7f82, 0x0),
    ),
    g(
        "s_ucomisss_45_22_2_in",
        "0f2eca",
        (1, 0x77502eaa42d3f23af4e465e53fc00000),
        (2, 0xd79ff46993b0bd6a6992aa2300000001),
        0x5f80,
        0x0,
        ok(0x77502eaa42d3f23af4e465e53fc00000, 0x5f82, 0x0),
    ),
    g(
        "s_ucomisss_26_26_3_in",
        "0f2eca",
        (1, 0x6bbddf2538cca654a34536ce00400000),
        (2, 0xf4da6b6623b5dc9322e952d600400000),
        0x7f80,
        0x8d5,
        ok(0x6bbddf2538cca654a34536ce00400000, 0x7f82, 0x40),
    ),
    g(
        "s_ucomisss_24_24_0_in",
        "0f2eca",
        (1, 0x3991965452c321a68dff9fb7007fffff),
        (2, 0x75d44bd1a1bec94e6863179007fffff),
        0x1f80,
        0x8d5,
        ok(0x3991965452c321a68dff9fb7007fffff, 0x1f82, 0x40),
    ),
    g(
        "s_ucomissd_38_38_7_in",
        "660f2eca",
        (1, 0xa9175de6fd855bd84330000000000000),
        (2, 0x3e5f81a72cff7f9b4330000000000000),
        0x0,
        0x8d5,
        ok(0xa9175de6fd855bd84330000000000000, 0x0, 0x40),
    ),
    g(
        "s_ucomissd_0_0_1_in",
        "660f2eca",
        (1, 0x98ea1e783e0ed6b70000000000000000),
        (2, 0x227e33b599bdd56f0000000000000000),
        0x3f80,
        0x0,
        ok(0x98ea1e783e0ed6b70000000000000000, 0x3f80, 0x40),
    ),
    g(
        "sx_ucomissd_57_17",
        "66450f2ece",
        (9, 0xb7b4ec8ca0cfc4d00010000000000000),
        (14, 0xb4c72a0178c80d9e0020000000000000),
        0xfbf,
        0x8d5,
        ok(0xb7b4ec8ca0cfc4d00010000000000000, 0xfbf, 0x1),
    ),
    g(
        "s_ucomissd_42_39_0_in",
        "660f2eca",
        (1, 0xb3e0769ad226e51a0020000000000000),
        (2, 0xeb2098003e74e0d433fffffffffffff),
        0x1f80,
        0x8d5,
        ok(0xb3e0769ad226e51a0020000000000000, 0x1f80, 0x1),
    ),
    g(
        "s_ucomissd_31_34_3_in",
        "660f2eca",
        (1, 0x211414482e2bdedb7fefffffffffffff),
        (2, 0x655a0696087148f17fd0000000000000),
        0x7f80,
        0xcd5,
        ok(0x211414482e2bdedb7fefffffffffffff, 0x7f80, 0x0),
    ),
    g(
        "s_ucomissd_9_37_6_in",
        "660f2eca",
        (1, 0xd3b3f8f2b30e07a53fefffffffffffff),
        (2, 0x8dc442c2f852b1ec3ca0000000000000),
        0xffc0,
        0x8d5,
        ok(0xd3b3f8f2b30e07a53fefffffffffffff, 0xffc0, 0x0),
    ),
    g(
        "s_ucomissd_44_15_3_in",
        "660f2eca",
        (1, 0x3da452c512177b444014000000000000),
        (2, 0x94a5242932f740a17fffffffffffffff),
        0x7f80,
        0x0,
        ok(0x3da452c512177b444014000000000000, 0x7f80, 0x45),
    ),
    g(
        "s_ucomissd_32_12_2_in",
        "660f2eca",
        (1, 0x50efa85b6fa27ae9ffefffffffffffff),
        (2, 0xa26c5477651f78da7ff8000000000000),
        0x5f80,
        0x0,
        ok(0x50efa85b6fa27ae9ffefffffffffffff, 0x5f80, 0x45),
    ),
    g(
        "s_ucomissd_42_17_6_in",
        "660f2eca",
        (1, 0x6a327da854ffc440020000000000000),
        (2, 0xe7b5ccf4b20e325e7ff0000000000001),
        0xffc0,
        0xcd5,
        ok(0x6a327da854ffc440020000000000000, 0xffc1, 0x45),
    ),
    g(
        "s_ucomissd_4_19_2_in",
        "660f2eca",
        (1, 0x640e56a945c357e54000000000000000),
        (2, 0x25d5ea34ed79f82b7ff4000000000000),
        0x5f80,
        0x0,
        ok(0x640e56a945c357e54000000000000000, 0x5f81, 0x45),
    ),
    g(
        "s_ucomissd_38_21_7_in",
        "660f2eca",
        (1, 0x3e84d632f50161804330000000000000),
        (2, 0x95f93c0d2576c4d7fff7ffffffffffff),
        0x0,
        0x8d5,
        fault(0x1),
    ),
    g(
        "s_ucomissd_20_13_7_in",
        "660f2eca",
        (1, 0xb1f2e03d221330c87ff7ffffffffffff),
        (2, 0xe1e691aa65ffeceffff8000000000000),
        0x0,
        0x8d5,
        fault(0x1),
    ),
    g(
        "sx_ucomissd_17_22",
        "66450f2ece",
        (9, 0x541cf7d98962b49a8000000000000001),
        (14, 0x4d62ea87f552b20433fffffffffffff),
        0x4f80,
        0x8d5,
        ok(0x541cf7d98962b49a8000000000000001, 0x4f82, 0x1),
    ),
    g(
        "s_ucomissd_1_24_1_in",
        "660f2eca",
        (1, 0xcf436f42eb0902088000000000000000),
        (2, 0x1eb215cf8ba215f1000fffffffffffff),
        0x3f80,
        0x0,
        ok(0xcf436f42eb0902088000000000000000, 0x3f82, 0x1),
    ),
    g(
        "sx_ucomissd_58_27",
        "66450f2ece",
        (9, 0xefff9ad4e17532c47fd0000000000000),
        (14, 0x25bc3f52d36cb7cd8000000000000001),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "s_ucomissd_1_22_7_in",
        "660f2eca",
        (1, 0xf4c8d93c7f97d98c8000000000000000),
        (2, 0xf7f9a12b626832c40000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_ucomissd_28_21",
        "66450f2ece",
        (9, 0x25f8033d7d8313140010000000000000),
        (14, 0xf20087cab712fb598000000000000001),
        0x9d80,
        0x8d5,
        ok(0x25f8033d7d8313140010000000000000, 0x9d82, 0x0),
    ),
    g(
        "s_ucomissd_10_24_0_in",
        "660f2eca",
        (1, 0xdc09cd434f0b63f57ff0000000000000),
        (2, 0xdf2598d38c58ab29000fffffffffffff),
        0x1f80,
        0xcd5,
        ok(0xdc09cd434f0b63f57ff0000000000000, 0x1f82, 0x0),
    ),
    g(
        "s_ucomissd_27_27_2_in",
        "660f2eca",
        (1, 0xc17a561026cbd34b0000000000000003),
        (2, 0xf5124d5700e67c40000000000000003),
        0x5f80,
        0x8d5,
        ok(0xc17a561026cbd34b0000000000000003, 0x5f82, 0x40),
    ),
    g(
        "s_ucomissd_26_26_2_in",
        "660f2eca",
        (1, 0xbf167f80f6150add0008000000000000),
        (2, 0x2ca54ee1aa8554d20008000000000000),
        0x5f80,
        0x8d5,
        ok(0xbf167f80f6150add0008000000000000, 0x5f82, 0x40),
    ),
    g(
        "rb_ucomisss_251",
        "0f2eeb",
        (5, 0x801246077fa6da2a00763c433fc284be),
        (3, 0xffd89cc4802f911dccc02844807d5c00),
        0x9683,
        0x8d5,
        fault(0x9683),
    ),
    g(
        "rb_ucomisss_15",
        "450f2ec3",
        (8, 0xc014ab6f7489a75ab26e5c2280db709c),
        (11, 0x3026f241c07673047489459e001ef343),
        0xb687,
        0x8d5,
        fault(0xb687),
    ),
    g(
        "rb_ucomissd_190",
        "660f2ee0",
        (4, 0xd62d2b4f1f62a3ea000aeb78995ec462),
        (0, 0x4322569c480193a4801fac71c254b4a1),
        0x162f,
        0xcd5,
        fault(0x162f),
    ),
    g(
        "rb_ucomissd_53",
        "66440f2ec6",
        (8, 0x83cc731b2bd1fc32a5c9504a69e89),
        (6, 0x32b83df0d5b4c78600012ad266012fc1),
        0xdc03,
        0x8d5,
        fault(0xdc03),
    ),
    g(
        "sx_cmpss_45_11",
        "f3450fc2ce05",
        (9, 0xd4e8e8eab2bb075761c1dfe04b7fffff),
        (14, 0x1e2c93173c6de9e26968bb2b7f000000),
        0x77c0,
        0x0,
        ok(0xd4e8e8eab2bb075761c1dfe000000000, 0x77c0, 0x0),
    ),
    g(
        "sx_cmpss_32_26",
        "f3450fc2ce05",
        (9, 0xcebf0d8af0b8017eb4b2b29240a00000),
        (14, 0x8c7e70956d23921e5f8c60a67f000000),
        0x9780,
        0x0,
        ok(0xcebf0d8af0b8017eb4b2b29200000000, 0x9780, 0x0),
    ),
    g(
        "s_cmpss_43_13_4_i00",
        "f30fc2ca00",
        (1, 0x61348b7a45ca40c247a6dee700ffffff),
        (2, 0x516b10489b66826624f51181ffc00000),
        0x1ec0,
        0x0,
        ok(0x61348b7a45ca40c247a6dee700000000, 0x1ec0, 0x0),
    ),
    g(
        "s_cmpss_8_44_0_i00",
        "f30fc2ca00",
        (1, 0x24891bcfc8e663e6acd36b163f800001),
        (2, 0xdcd6b324190ebe6e5746a5ef40a00000),
        0x1f80,
        0x0,
        ok(0x24891bcfc8e663e6acd36b1600000000, 0x1f80, 0x0),
    ),
    g(
        "s_cmpss_30_43_2_i02",
        "f30fc2ca02",
        (1, 0xc0db60e261f4ce24f814d8d700800001),
        (2, 0xbaef66c4ec9048328424970c00ffffff),
        0x0,
        0x0,
        ok(0xc0db60e261f4ce24f814d8d7ffffffff, 0x0, 0x0),
    ),
    g(
        "s_cmpss_1_32_4_i02",
        "f30fc2ca02",
        (1, 0x32b3d775afe7ae286e1e9ae280000000),
        (2, 0xa9ee1579ea4a930239fc9f76ff7fffff),
        0x1ec0,
        0x0,
        ok(0x32b3d775afe7ae286e1e9ae200000000, 0x1ec0, 0x0),
    ),
    g(
        "sx_cmpss_47_16",
        "f3450fc2ce04",
        (9, 0x5e66536bd0376c0a67a26be8ffc12345),
        (14, 0xd68fed0b5b5e2de9ae2a93547fc00001),
        0x6fc0,
        0x0,
        ok(0x5e66536bd0376c0a67a26be8ffffffff, 0x6fc0, 0x0),
    ),
    g(
        "s_cmpss_2_23_1_i04",
        "f30fc2ca04",
        (1, 0xd91628908f9630e7a00455883f800000),
        (2, 0x3600509b3ec5bc7dede25e9380000001),
        0x1fc0,
        0x0,
        ok(0xd91628908f9630e7a0045588ffffffff, 0x1fc0, 0x0),
    ),
    g(
        "sx_cmpss_1_14",
        "f3450fc2ce07",
        (9, 0xead4eac0e495a4951953c7e933800000),
        (14, 0x698ffaf0fde21e9281622713fc00000),
        0x1e80,
        0x0,
        ok(0xead4eac0e495a4951953c7e9ffffffff, 0x1e80, 0x0),
    ),
    g(
        "s_cmpss_22_39_1_i07",
        "f30fc2ca07",
        (1, 0x8ddc5bcf78858f7249eda6d00000001),
        (2, 0x9afb491fe46626e30d5406224b7fffff),
        0x1fc0,
        0x0,
        ok(0x8ddc5bcf78858f7249eda6dffffffff, 0x1fc0, 0x0),
    ),
    g(
        "s_cmpss_37_34_0_i01",
        "f30fc2ca01",
        (1, 0x9fe2abd0310f54116282dc8433800000),
        (2, 0xc6f35ee134c962c7f172c97c7e800000),
        0x1f80,
        0x0,
        ok(0x9fe2abd0310f54116282dc84ffffffff, 0x1f80, 0x0),
    ),
    g(
        "sx_cmpss_12_22",
        "f3450fc2ce01",
        (9, 0x325cf9388ec086f05263059440a00000),
        (14, 0xa81182f730916329f365922d34000000),
        0x5f00,
        0x0,
        ok(0x325cf9388ec086f05263059400000000, 0x5f00, 0x0),
    ),
    g(
        "s_cmpss_38_34_4_i06",
        "f30fc2ca06",
        (1, 0xc83e20e31d0e379cb3d8326c4b000000),
        (2, 0x9cd3c3578481a86410d4c2a37e800000),
        0x1ec0,
        0x0,
        ok(0xc83e20e31d0e379cb3d8326c00000000, 0x1ec0, 0x0),
    ),
    g(
        "s_cmpss_2_33_0_i06",
        "f30fc2ca06",
        (1, 0x40d8777c7c6d6c2b333f443e3f800000),
        (2, 0x40f073c4ca92c32235d661667f000000),
        0x1f80,
        0x0,
        ok(0x40d8777c7c6d6c2b333f443e00000000, 0x1f80, 0x0),
    ),
    g(
        "sx_cmpss_2_22",
        "f3450fc2ce03",
        (9, 0x1c56e23eb7dbe162bb7d783a7fc00001),
        (14, 0xe4c2dca9599f8807108ebeacffc12345),
        0x1d80,
        0x0,
        ok(0x1c56e23eb7dbe162bb7d783affffffff, 0x1d80, 0x0),
    ),
    g(
        "sx_cmpss_9_29",
        "f3450fc2ce03",
        (9, 0xf08e3139d7fa8927f5a666a2bf800000),
        (14, 0x2b0808b19e3278da707d003c7e800000),
        0x3b80,
        0x0,
        ok(0xf08e3139d7fa8927f5a666a200000000, 0x3b80, 0x0),
    ),
    g(
        "sx_cmpss_58_1",
        "f3450fc2ce02",
        (9, 0xa62db0d838e344550d5673657fbfffff),
        (14, 0x9785f24f9ade0782a62ce9f87f800000),
        0x1e80,
        0x0,
        ok(0xa62db0d838e344550d56736500000000, 0x1e81, 0x0),
    ),
    g(
        "s_cmpss_16_38_1_i02",
        "f30fc2ca02",
        (1, 0x4a12626706e2fce5816f0297ffc12345),
        (2, 0x36932914c71852eb654221854b000000),
        0x1fc0,
        0x0,
        ok(0x4a12626706e2fce5816f029700000000, 0x1fc1, 0x0),
    ),
    g(
        "sx_cmpss_36_12",
        "f3450fc2ce06",
        (9, 0xf7edb33bc2694e38a4f6038980800000),
        (14, 0x7070f02532ae8fda89e8c2f07fffffff),
        0xff00,
        0x0,
        fault(0xff01),
    ),
    g(
        "s_cmpss_14_45_2_i06",
        "f30fc2ca06",
        (1, 0xc5f2039687272aaa047120887fc00001),
        (2, 0x231cd67f66b3e91c497161ef3fc00000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpss_3_28",
        "f3450fc2ce06",
        (9, 0x134945401307c348c99560a07fffffff),
        (14, 0xa0b6dfe37c69b25ee57a0ecff800000),
        0x1b80,
        0x0,
        ok(0x134945401307c348c99560a0ffffffff, 0x1b81, 0x0),
    ),
    g(
        "s_cmpss_19_13_4_i06",
        "f30fc2ca06",
        (1, 0x8c84eb0f4c5c8b7587a3e6e77fa00000),
        (2, 0x4a01ffb72b8dd74f02cf3fb8ffc00000),
        0x1ec0,
        0x0,
        ok(0x8c84eb0f4c5c8b7587a3e6e7ffffffff, 0x1ec1, 0x0),
    ),
    g(
        "sx_cmpss_22_5",
        "f3450fc2ce01",
        (9, 0x4094d9cc0e674a2aaa0276853f000000),
        (14, 0x4c7bf5a7536087f954a37e9effbfffff),
        0x7780,
        0x0,
        ok(0x4094d9cc0e674a2aaa02768500000000, 0x7781, 0x0),
    ),
    g(
        "s_cmpss_19_36_4_i01",
        "f30fc2ca01",
        (1, 0x58f58577d49f0bb8e3ab8c47fa00000),
        (2, 0xb7a96f3905a98fb8e66ad39634000000),
        0x1ec0,
        0x0,
        ok(0x58f58577d49f0bb8e3ab8c400000000, 0x1ec1, 0x0),
    ),
    g(
        "sx_cmpss_66_23",
        "f3450fc2ce01",
        (9, 0x31759164ec87b8094ab1af2d7e800000),
        (14, 0xe7e1399ef6ab03381b464ecbffbfffff),
        0x1a00,
        0x0,
        fault(0x1a01),
    ),
    g(
        "sx_cmpss_12_15",
        "f3450fc2ce01",
        (9, 0xcb020218f9df7181b5ec903c7f800001),
        (14, 0x75ddd5029944b1ee2ad3b6577f800001),
        0x5f00,
        0x0,
        fault(0x5f01),
    ),
    g(
        "s_cmpss_30_12_0_i05",
        "f30fc2ca05",
        (1, 0x8f4a427a46ec4cb96c3bfb7c00800001),
        (2, 0xd813b4d261dbaa1b90edf7877fc00000),
        0x1f80,
        0x0,
        ok(0x8f4a427a46ec4cb96c3bfb7cffffffff, 0x1f81, 0x0),
    ),
    g(
        "s_cmpss_2_16_4_i05",
        "f30fc2ca05",
        (1, 0x3924d3c1324ea83947d563ff3f800000),
        (2, 0x4831cb8f549de6dd273fca15ffc12345),
        0x1ec0,
        0x0,
        ok(0x3924d3c1324ea83947d563ffffffffff, 0x1ec1, 0x0),
    ),
    g(
        "rb_cmpss_205",
        "f30fc2e003",
        (4, 0xf8324b450134f87f0000e22100d7fbdb),
        (0, 0x74d124f8006a44a1bfcce95a7fb01a21),
        0x3780,
        0x0,
        ok(0xf8324b450134f87f0000e221ffffffff, 0x3781, 0x0),
    ),
    g(
        "s_cmpss_0_18_0_i03",
        "f30fc2ca03",
        (1, 0x25785af83105f122cb2eff9900000000),
        (2, 0xe653865158bd16d82fadbd31ff800001),
        0x1f80,
        0x0,
        ok(0x25785af83105f122cb2eff99ffffffff, 0x1f81, 0x0),
    ),
    g(
        "sx_cmpss_51_28",
        "f3450fc2ce07",
        (9, 0x94a45eaa9e0928755fc759a600000003),
        (14, 0x24f5b70f9d2d70c2c9f24cc77fbfffff),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "s_cmpss_17_3_3_i07",
        "f30fc2ca07",
        (1, 0xb737d067fa0d137d4c3ea06d7f800001),
        (2, 0x6a6a479b73d0e483f9d9bb0cbf800000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "sx_cmpss_13_24",
        "f3450fc2ce00",
        (9, 0x7fee2ed2b913e39b8b34a3b2ffbfffff),
        (14, 0x66c78ed691b19f07477e74f97f800000),
        0x5e80,
        0x0,
        ok(0x7fee2ed2b913e39b8b34a3b200000000, 0x5e81, 0x0),
    ),
    g(
        "s_cmpss_19_13_1_i00",
        "f30fc2ca00",
        (1, 0xa9406f72938d0403a80caf117fa00000),
        (2, 0x22cc1418bed74298535dc3eeffc00000),
        0x1fc0,
        0x0,
        ok(0xa9406f72938d0403a80caf1100000000, 0x1fc1, 0x0),
    ),
    g(
        "s_cmpss_41_18_3_i05",
        "f30fc2ca05",
        (1, 0xc2356200d1262f0bb1b44947effffff),
        (2, 0xc593a29a45b6996725042dbfff800001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_cmpss_43_14_2_i05",
        "f30fc2ca05",
        (1, 0xf50625fbcb2e9fff44b4cc0300ffffff),
        (2, 0x2014f90509fa40ef89ae292c7fc00001),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpss_43_23",
        "f3450fc2ce07",
        (9, 0xb6e67f1a495c80a5538ed29b7f800001),
        (14, 0xa2f85f768e90322cc5b159c37fc00001),
        0x7bc0,
        0x0,
        ok(0xb6e67f1a495c80a5538ed29b00000000, 0x7bc1, 0x0),
    ),
    g(
        "sx_cmpss_42_19",
        "f3450fc2ce07",
        (9, 0xa33a9548211178e71eab6ae67fa00000),
        (14, 0x4873ff857950307fb5b89cedff800001),
        0xfb80,
        0x0,
        ok(0xa33a9548211178e71eab6ae600000000, 0xfb81, 0x0),
    ),
    g(
        "s_cmpss_39_17_3_i04",
        "f30fc2ca04",
        (1, 0x1b1b5b2a99fd77cc2cf556ac4b7fffff),
        (2, 0xc10e28c9522840d7d4eb89e7f800001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_cmpss_4_18_3_i04",
        "f30fc2ca04",
        (1, 0x846639d46b926d2936baf7240000000),
        (2, 0x75735e0cefd36e5af6ab6177ff800001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "sx_cmpss_43_6",
        "f3450fc2ce04",
        (9, 0x787ce9696c02a030a9d27cc83f000000),
        (14, 0x1df14b435ff1d808bcb71696ff800001),
        0x7bc0,
        0x0,
        ok(0x787ce9696c02a030a9d27cc8ffffffff, 0x7bc1, 0x0),
    ),
    g(
        "s_cmpss_4_17_1_i04",
        "f30fc2ca04",
        (1, 0x991a162af2783749010149d140000000),
        (2, 0xb801a14686adfaf88a5e48c7f800001),
        0x1fc0,
        0x0,
        ok(0x991a162af2783749010149d1ffffffff, 0x1fc1, 0x0),
    ),
    g(
        "sx_cmpss_64_5",
        "f3450fc2ce01",
        (9, 0x6d6fcc424c3df7e6705f70f2007fffff),
        (14, 0xb5d4e318ccbb23a11d7a567b33800000),
        0x1700,
        0x0,
        ok(0x6d6fcc424c3df7e6705f70f2ffffffff, 0x1702, 0x0),
    ),
    g(
        "s_cmpss_22_35_0_i01",
        "f30fc2ca01",
        (1, 0x37d56cd27f59caceee58491600000001),
        (2, 0xc2377bb803214692cb20aa8b3e800000),
        0x1f80,
        0x0,
        ok(0x37d56cd27f59caceee584916ffffffff, 0x1f82, 0x0),
    ),
    g(
        "sx_cmpss_49_13",
        "f3450fc2ce04",
        (9, 0x29f87844332d1429608d2a1140a00000),
        (14, 0x134211f5e3ce8f23aa32b50300400000),
        0x4000,
        0x0,
        fault(0x4002),
    ),
    g(
        "s_cmpss_0_27_2_i04",
        "f30fc2ca04",
        (1, 0xc7df3dd21cd9ecfa2ba8b4b700000000),
        (2, 0x9f95501b5d24bf5888fd9f1700000003),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "rb_cmpss_95",
        "f3450fc2c206",
        (8, 0xff3425878b065bbc80181a45800f6877),
        (10, 0x807668d17fef2804804499377f1e6ac4),
        0x1b80,
        0x0,
        ok(0xff3425878b065bbc80181a4500000000, 0x1b82, 0x0),
    ),
    g(
        "s_cmpss_11_26_3_i06",
        "f30fc2ca06",
        (1, 0xa8a6779311495dd0316332f8ff800000),
        (2, 0x1835d51bf6ac8cb663ca813c00400000),
        0x1f00,
        0x0,
        ok(0xa8a6779311495dd0316332f800000000, 0x1f02, 0x0),
    ),
    g(
        "s_cmpss_33_26_0_i07",
        "f30fc2ca07",
        (1, 0xaf7838d8058a0d49a83e227c7f000000),
        (2, 0x104fe3763df1c312f93d6c2300400000),
        0x1f80,
        0x0,
        ok(0xaf7838d8058a0d49a83e227cffffffff, 0x1f82, 0x0),
    ),
    g(
        "s_cmpss_3_23_3_i07",
        "f30fc2ca07",
        (1, 0xe5dc1cc1c3608d9f514d1db2bf800000),
        (2, 0xc5a3425f09e2179373f1011a80000001),
        0x1f00,
        0x0,
        ok(0xe5dc1cc1c3608d9f514d1db2ffffffff, 0x1f02, 0x0),
    ),
    g(
        "s_cmpss_27_35_2_i02",
        "f30fc2ca02",
        (1, 0xac26ef697b83f3eaa481691200000003),
        (2, 0xacf564465ae6e0280018e6463e800000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpss_27_25_2_i02",
        "f30fc2ca02",
        (1, 0xa58dea7d67c5368e99eba34c00000003),
        (2, 0xe38eebbf59e2c139008f8c11807fffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpss_18_14",
        "f3450fc2ce00",
        (9, 0x5a8f9196a8d14fa9606979a140000000),
        (14, 0x444e9031a770be9a82eb6079807fffff),
        0x7f00,
        0x0,
        ok(0x5a8f9196a8d14fa9606979a100000000, 0x7f02, 0x0),
    ),
    g(
        "s_cmpss_22_0_3_i00",
        "f30fc2ca00",
        (1, 0x37182ae708602ab79b758fba00000001),
        (2, 0x9c01e2f07e426d0e6e4cc45600000000),
        0x1f00,
        0x0,
        ok(0x37182ae708602ab79b758fba00000000, 0x1f02, 0x0),
    ),
    g(
        "s_cmpss_37_25_2_i05",
        "f30fc2ca05",
        (1, 0x9d18f6f2a722f3bbe64135d333800000),
        (2, 0x7378a75a07fee1be3b3d7383807fffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpss_27_29_2_i05",
        "f30fc2ca05",
        (1, 0x872d86627303c3f5de18953000000003),
        (2, 0xaf22b4369ed0e95cd14daaec80800000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpss_58_24",
        "f3450fc2ce07",
        (9, 0xa2e59328591ee028a00b3fff40400000),
        (14, 0xe65cb29e62e32543cbd854ba00000003),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "s_cmpss_27_0_2_i07",
        "f30fc2ca07",
        (1, 0xaf058cb05fde35fb87372d7e00000003),
        (2, 0xa40448af6d0088702156ff3a00000000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpss_60_25",
        "f3450fc2ce04",
        (9, 0x562300416030f702bdcb9ac37f800000),
        (14, 0x5124a164f3283e342a41ad46807fffff),
        0x1b80,
        0x0,
        ok(0x562300416030f702bdcb9ac3ffffffff, 0x1b82, 0x0),
    ),
    g(
        "s_cmpss_2_22_3_i04",
        "f30fc2ca04",
        (1, 0x25e01a436e30baee697cafdd3f800000),
        (2, 0x47833a7385733282b14bc87700000001),
        0x1f00,
        0x0,
        ok(0x25e01a436e30baee697cafddffffffff, 0x1f02, 0x0),
    ),
    g(
        "sx_cmpss_18_22",
        "f3450fc2ce02",
        (9, 0xaa45300814a6d89f052e70597fc00001),
        (14, 0xf6bfdd955540dcb4a279c41bf800000),
        0x7f00,
        0x0,
        fault(0x7f01),
    ),
    g(
        "s_cmpss_17_1_3_i02",
        "f30fc2ca02",
        (1, 0xada543c31dee7a9e8d1153727f800001),
        (2, 0xdb179af302370c742e27a7df80000000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_cmpss_41_17_3_i00",
        "f30fc2ca00",
        (1, 0x61c19ca1b861518931064d997effffff),
        (2, 0x4973bde6628f475d6a43d2ce7f800001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_cmpss_28_20_2_i00",
        "f30fc2ca00",
        (1, 0x2194c62f278d32e5bfae34100800000),
        (2, 0x65739fad4e1b41cba469c3117fbfffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_cmpss_33_25_0_i02",
        "f30fc2ca02",
        (1, 0xd9e74f6616460359875a28f77f000000),
        (2, 0x2a2028a7a6bb91d5efdda467807fffff),
        0x1f80,
        0x0,
        ok(0xd9e74f6616460359875a28f700000000, 0x1f82, 0x0),
    ),
    g(
        "s_cmpss_4_24_0_i02",
        "f30fc2ca02",
        (1, 0x19beb42904d6954c5f35a73540000000),
        (2, 0xefbea45081390c4c673a61bf007fffff),
        0x1f80,
        0x0,
        ok(0x19beb42904d6954c5f35a73500000000, 0x1f82, 0x0),
    ),
    g(
        "sx_cmpss_19_15",
        "f3450fc2ce01",
        (9, 0x8921a12645ba2f2a8a01d13e7f000000),
        (14, 0xeb53a67de6501ec3bde22287007fffff),
        0x7e80,
        0x0,
        fault(0x7e82),
    ),
    g(
        "s_cmpss_10_22_2_i01",
        "f30fc2ca01",
        (1, 0x7210634e0a2a2204714f024c7f800000),
        (2, 0xb24eb18efb7c3d51ce99ddb400000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpss_37_27_2_i06",
        "f30fc2ca06",
        (1, 0x307c1fcb19652427cc1a6bd433800000),
        (2, 0x348b2aefe226851fd54c066600000003),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpss_26_30_2_i06",
        "f30fc2ca06",
        (1, 0x9260ba3c7b831e8fc4d0b62900400000),
        (2, 0xad9d2a077185a5ab4313f06a00800001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpss_2_28",
        "f3450fc2ce05",
        (9, 0x9019fef42c3b8dc06ecd8dae00ffffff),
        (14, 0x7d49c783cb98343d0a16b8eb00000003),
        0x1d80,
        0x0,
        ok(0x9019fef42c3b8dc06ecd8daeffffffff, 0x1d82, 0x0),
    ),
    g(
        "s_cmpss_26_44_3_i05",
        "f30fc2ca05",
        (1, 0x87a33be720b816b2fd550c0100400000),
        (2, 0xc44f4de778f05e341fff129c40a00000),
        0x1f00,
        0x0,
        ok(0x87a33be720b816b2fd550c0100000000, 0x1f02, 0x0),
    ),
    g(
        "sx_cmpss_9_10",
        "f3450fc2ce03",
        (9, 0xe56af7719b9b35a45160c364b7fffff),
        (14, 0x831f7ca42ff5703ebeb3028900400000),
        0x3b80,
        0x0,
        ok(0xe56af7719b9b35a45160c3600000000, 0x3b82, 0x0),
    ),
    g(
        "s_cmpss_30_23_3_i03",
        "f30fc2ca03",
        (1, 0x40db06230265d4fbb4b428fa00800001),
        (2, 0xa39b3887565472b18d4fc7780000001),
        0x1f00,
        0x0,
        ok(0x40db06230265d4fbb4b428fa00000000, 0x1f02, 0x0),
    ),
    g(
        "sx_cmpss_1_26",
        "f3450fc2ce00",
        (9, 0xc684f9c911b44ca3e446a9bc00400000),
        (14, 0xa5f8abcd4cba8cf1fea44be2ff800000),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "s_cmpss_5_22_2_i00",
        "f30fc2ca00",
        (1, 0x1fe90049200552f6736b897fc0000000),
        (2, 0xfb5be53c95af1989392fd7b200000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpss_25_18",
        "f3450fc2ce03",
        (9, 0x7e8f234ac687d71521fc4bbe00000001),
        (14, 0x6d62382a9f9a82c173923048ff800001),
        0x1f40,
        0x0,
        fault(0x1f41),
    ),
    g(
        "s_cmpss_32_20_2_i03",
        "f30fc2ca03",
        (1, 0xce5a78781410413f642afd9bff7fffff),
        (2, 0x71c69c6fbeab2d77347cd1977fbfffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpss_66_26",
        "f3450fc2ce03",
        (9, 0x3200e7b275388d7c1ac31e0e00800000),
        (14, 0x3be48e907adee14e0ba7e24b007fffff),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "s_cmpss_22_33_2_i03",
        "f30fc2ca03",
        (1, 0x9b8c0c2dd9fdb7ba1ff93d5000000001),
        (2, 0xc8de56801b7dfa8b40fef8337f000000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "rb_cmpsd_78",
        "f20fc2e001",
        (4, 0xc32625d027b4efa37fd19171d4559191),
        (0, 0x3cbf2b347c50fee4c0019915110fb1dc),
        0x0,
        0x0,
        ok(0xc32625d027b4efa30000000000000000, 0x0, 0x0),
    ),
    g(
        "s_cmpsd_39_25_1_i01",
        "f20fc2ca01",
        (1, 0x2064abb568b3792c433fffffffffffff),
        (2, 0x33c5ba1d2d810004800fffffffffffff),
        0x1fc0,
        0x0,
        ok(0x2064abb568b3792c0000000000000000, 0x1fc0, 0x0),
    ),
    g(
        "sx_cmpsd_37_14",
        "f2450fc2ce04",
        (9, 0x40758120e415a1cd7fefffffffffffff),
        (14, 0xddd536b2c0302c10000000000000003),
        0x7f40,
        0x0,
        ok(0x40758120e415a1cdffffffffffffffff, 0x7f40, 0x0),
    ),
    g(
        "s_cmpsd_0_13_1_i04",
        "f20fc2ca04",
        (1, 0x655b4adeed6f78750000000000000000),
        (2, 0x4a0b51017d8639a5fff8000000000000),
        0x1fc0,
        0x0,
        ok(0x655b4adeed6f7875ffffffffffffffff, 0x1fc0, 0x0),
    ),
    g(
        "rb_cmpsd_75",
        "f2410fc2d302",
        (2, 0x7fdb0807bd90212a8022a343f6d77a0a),
        (11, 0xc00e515ccbc297ebc00e9c89c83c66b7),
        0x41e5,
        0x0,
        ok(0x7fdb0807bd90212a0000000000000000, 0x41e5, 0x0),
    ),
    g(
        "s_cmpsd_11_8_2_i02",
        "f20fc2ca02",
        (1, 0xbf56171e674a256afff0000000000000),
        (2, 0x7047819eaa5496e03ff0000000000001),
        0x0,
        0x0,
        ok(0xbf56171e674a256affffffffffffffff, 0x0, 0x0),
    ),
    g(
        "s_cmpsd_41_39_1_i06",
        "f20fc2ca06",
        (1, 0xacdd1045ad15f0467fdfffffffffffff),
        (2, 0x23e6b979490c2077433fffffffffffff),
        0x1fc0,
        0x0,
        ok(0xacdd1045ad15f046ffffffffffffffff, 0x1fc0, 0x0),
    ),
    g(
        "sx_cmpsd_61_27",
        "f2450fc2ce06",
        (9, 0xc53dc7e1bb3779f3ca0000000000000),
        (14, 0x25b51d5dad3714694330000000000000),
        0x1780,
        0x0,
        ok(0xc53dc7e1bb3779f0000000000000000, 0x1780, 0x0),
    ),
    g(
        "sx_cmpsd_48_8",
        "f2450fc2ce03",
        (9, 0x6a9fc6b5915f8514c000000000000000),
        (14, 0x7b0d6b0f9d5c28c03fd0000000000000),
        0x2000,
        0x0,
        ok(0x6a9fc6b5915f85140000000000000000, 0x2000, 0x0),
    ),
    g(
        "s_cmpsd_28_15_3_i03",
        "f20fc2ca03",
        (1, 0xda6633106cd68ce20010000000000000),
        (2, 0x28bf6cef0253fdc77fffffffffffffff),
        0x1f00,
        0x0,
        ok(0xda6633106cd68ce2ffffffffffffffff, 0x1f00, 0x0),
    ),
    g(
        "sx_cmpsd_60_15",
        "f2450fc2ce05",
        (9, 0x1a93117e08651f333fd0000000000000),
        (14, 0x7e9af3882df3a8363ff0000000000001),
        0x1b80,
        0x0,
        ok(0x1a93117e08651f330000000000000000, 0x1b80, 0x0),
    ),
    g(
        "s_cmpsd_35_9_0_i05",
        "f20fc2ca05",
        (1, 0x272aa15864355bb83fd0000000000000),
        (2, 0x6c8b7f0657a870733fefffffffffffff),
        0x1f80,
        0x0,
        ok(0x272aa15864355bb80000000000000000, 0x1f80, 0x0),
    ),
    g(
        "s_cmpsd_41_8_4_i07",
        "f20fc2ca07",
        (1, 0xa5b8a5e6724a899e7fdfffffffffffff),
        (2, 0x415412433e104abb3ff0000000000001),
        0x1ec0,
        0x0,
        ok(0xa5b8a5e6724a899effffffffffffffff, 0x1ec0, 0x0),
    ),
    g(
        "s_cmpsd_2_32_2_i07",
        "f20fc2ca07",
        (1, 0x77179419ecd443bd3ff0000000000000),
        (2, 0x28d6c4604ea33a03ffefffffffffffff),
        0x0,
        0x0,
        ok(0x77179419ecd443bdffffffffffffffff, 0x0, 0x0),
    ),
    g(
        "rb_cmpsd_180",
        "f2450fc2ee00",
        (13, 0xbcb6857273ce80708008d585a36a61e0),
        (14, 0x3ffbd569f9a6a67e3ca81ef5e8030394),
        0x9ba7,
        0x0,
        ok(0xbcb6857273ce80700000000000000000, 0x9ba7, 0x0),
    ),
    g(
        "s_cmpsd_2_43_3_i00",
        "f20fc2ca00",
        (1, 0xd7cef411fe536f6e3ff0000000000000),
        (2, 0x98b07d3879a2beb5001fffffffffffff),
        0x1f00,
        0x0,
        ok(0xd7cef411fe536f6e0000000000000000, 0x1f00, 0x0),
    ),
    g(
        "s_cmpsd_31_15_2_i02",
        "f20fc2ca02",
        (1, 0x70066f859fab71507fefffffffffffff),
        (2, 0xc99e69d0f3f8e9157fffffffffffffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_cmpsd_13_40_2_i02",
        "f20fc2ca02",
        (1, 0x24c27ebb3049604ffff8000000000000),
        (2, 0x2b58ee3ad54be7924340000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpsd_59_7",
        "f2450fc2ce06",
        (9, 0xb6e2b3e2f60948fdfff7ffffffffffff),
        (14, 0x86842ce203210ea74330000000000000),
        0x1d80,
        0x0,
        ok(0xb6e2b3e2f60948fdffffffffffffffff, 0x1d81, 0x0),
    ),
    g(
        "sx_cmpsd_22_3",
        "f2450fc2ce06",
        (9, 0x46a3ef34b40b30527ff7ffffffffffff),
        (14, 0xfbec4c76e34699c2fff7ffffffffffff),
        0x7780,
        0x0,
        ok(0x46a3ef34b40b3052ffffffffffffffff, 0x7781, 0x0),
    ),
    g(
        "s_cmpsd_34_13_4_i01",
        "f20fc2ca01",
        (1, 0xad737f0765e9d74e7fd0000000000000),
        (2, 0xb3bf94785cfafeeafff8000000000000),
        0x1ec0,
        0x0,
        ok(0xad737f0765e9d74e0000000000000000, 0x1ec1, 0x0),
    ),
    g(
        "s_cmpsd_4_18_0_i01",
        "f20fc2ca01",
        (1, 0x8b373f4fb957b424000000000000000),
        (2, 0x7079dc48944129d8fff0000000000001),
        0x1f80,
        0x0,
        ok(0x8b373f4fb957b420000000000000000, 0x1f81, 0x0),
    ),
    g(
        "s_cmpsd_44_16_0_i02",
        "f20fc2ca02",
        (1, 0x8735ced8d22bd744014000000000000),
        (2, 0xe797b2d92c3b40ffff8123456789abc),
        0x1f80,
        0x0,
        ok(0x8735ced8d22bd740000000000000000, 0x1f81, 0x0),
    ),
    g(
        "s_cmpsd_19_20_0_i02",
        "f20fc2ca02",
        (1, 0x18f5895b7c32282b7ff4000000000000),
        (2, 0x671455d84d8ea9047ff7ffffffffffff),
        0x1f80,
        0x0,
        ok(0x18f5895b7c32282b0000000000000000, 0x1f81, 0x0),
    ),
    g(
        "s_cmpsd_20_1_2_i06",
        "f20fc2ca06",
        (1, 0xde6549c307667d7d7ff7ffffffffffff),
        (2, 0x27495ab95f64f3e48000000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_cmpsd_0_16_2_i06",
        "f20fc2ca06",
        (1, 0x81c9d6045210f9dd0000000000000000),
        (2, 0x73b5489ebeb6b6bafff8123456789abc),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpsd_46_5",
        "f2450fc2ce07",
        (9, 0x72d1371732b9c1130000000000000001),
        (14, 0xb97a83f621b06bd27ff7ffffffffffff),
        0xef80,
        0x0,
        ok(0x72d1371732b9c1130000000000000000, 0xef81, 0x0),
    ),
    g(
        "s_cmpsd_0_18_0_i07",
        "f20fc2ca07",
        (1, 0x1c58981fa76f51e80000000000000000),
        (2, 0x9405cf8c51cbe96cfff0000000000001),
        0x1f80,
        0x0,
        ok(0x1c58981fa76f51e80000000000000000, 0x1f81, 0x0),
    ),
    g(
        "sx_cmpsd_44_2",
        "f2450fc2ce05",
        (9, 0xfcbeecceec2f04b8fff8000000000000),
        (14, 0x68f6ff3936d1ba2d3ff0000000000001),
        0xf780,
        0x0,
        ok(0xfcbeecceec2f04b8ffffffffffffffff, 0xf781, 0x0),
    ),
    g(
        "sx_cmpsd_53_21",
        "f2450fc2ce05",
        (9, 0xd1f9df5fd4138ba7000fffffffffffff),
        (14, 0xa19e7704f09c05f4fff7ffffffffffff),
        0x80c0,
        0x0,
        ok(0xd1f9df5fd4138ba7ffffffffffffffff, 0x80c1, 0x0),
    ),
    g(
        "sx_cmpsd_51_19",
        "f2450fc2ce00",
        (9, 0x75914908bc9cb516fff7ffffffffffff),
        (14, 0xdd60eeb98f694fca7fffffffffffffff),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "s_cmpsd_6_19_3_i00",
        "f20fc2ca00",
        (1, 0xbd4e21e2c226af44008000000000000),
        (2, 0xcc59c503431e742d7ff4000000000000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "s_cmpsd_34_21_1_i03",
        "f20fc2ca03",
        (1, 0x5c9527a088b8f4f87fd0000000000000),
        (2, 0xd5191879255ca331fff7ffffffffffff),
        0x1fc0,
        0x0,
        ok(0x5c9527a088b8f4f8ffffffffffffffff, 0x1fc1, 0x0),
    ),
    g(
        "sx_cmpsd_14_5",
        "f2450fc2ce03",
        (9, 0x4dfaefce1823ed8afff0000000000000),
        (14, 0xf03bdda1c9efa853fff0000000000001),
        0x5d80,
        0x0,
        ok(0x4dfaefce1823ed8affffffffffffffff, 0x5d81, 0x0),
    ),
    g(
        "rb_cmpsd_80",
        "f2410fc2fa07",
        (7, 0x8016a9d9ce2d8524fff697b4aad5b2a9),
        (10, 0x3ca76d1134cfe3917ffb378dad1e9455),
        0x5f00,
        0x0,
        fault(0x5f01),
    ),
    g(
        "s_cmpsd_3_17_3_i07",
        "f20fc2ca07",
        (1, 0x1fd9a1401fb4d4d5bff0000000000000),
        (2, 0xed06609d55bb0b837ff0000000000001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "sx_cmpsd_25_11",
        "f2450fc2ce03",
        (9, 0xfff4edcdcc51c44d433fffffffffffff),
        (14, 0xa98050aec65191d57ff4000000000000),
        0x1f40,
        0x0,
        fault(0x1f41),
    ),
    g(
        "s_cmpsd_1_20_2_i03",
        "f20fc2ca03",
        (1, 0x32bdfab1b2468ec68000000000000000),
        (2, 0x483d3bf4b63564677ff7ffffffffffff),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpsd_13_19",
        "f2450fc2ce00",
        (9, 0xf1ef6f715c35512ffff0000000000001),
        (14, 0xe31e854ada52f50a7fefffffffffffff),
        0x5e80,
        0x0,
        ok(0xf1ef6f715c35512f0000000000000000, 0x5e81, 0x0),
    ),
    g(
        "s_cmpsd_19_0_1_i00",
        "f20fc2ca00",
        (1, 0x17628128e25604177ff4000000000000),
        (2, 0xec53f0b42c8cf60e0000000000000000),
        0x1fc0,
        0x0,
        ok(0x17628128e25604170000000000000000, 0x1fc1, 0x0),
    ),
    g(
        "s_cmpsd_32_12_2_i05",
        "f20fc2ca05",
        (1, 0x1f8bce292f937b4ffefffffffffffff),
        (2, 0xf76bf970326a92977ff8000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_cmpsd_15_11_2_i05",
        "f20fc2ca05",
        (1, 0x8165a93d118c222b7fffffffffffffff),
        (2, 0x3b1cd1eb8ed20343fff0000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpsd_40_29",
        "f2450fc2ce04",
        (9, 0x79ec122efdcbdd907fefffffffffffff),
        (14, 0xc4d54df072d31a8b7ff4000000000000),
        0xfd80,
        0x0,
        ok(0x79ec122efdcbdd90ffffffffffffffff, 0xfd81, 0x0),
    ),
    g(
        "s_cmpsd_20_27_4_i04",
        "f20fc2ca04",
        (1, 0xf69bdb7b28e799e27ff7ffffffffffff),
        (2, 0x78c30181e9e77e230000000000000003),
        0x1ec0,
        0x0,
        ok(0xf69bdb7b28e799e2ffffffffffffffff, 0x1ec1, 0x0),
    ),
    g(
        "s_cmpsd_35_22_3_i06",
        "f20fc2ca06",
        (1, 0x8cae2a87f1a3f2433fd0000000000000),
        (2, 0xd593b2c6a85fd520000000000000001),
        0x1f00,
        0x0,
        ok(0x8cae2a87f1a3f243ffffffffffffffff, 0x1f02, 0x0),
    ),
    g(
        "s_cmpsd_22_35_3_i06",
        "f20fc2ca06",
        (1, 0x92a3c6c7c99edaa60000000000000001),
        (2, 0x2a69da20b16919063fd0000000000000),
        0x1f00,
        0x0,
        ok(0x92a3c6c7c99edaa60000000000000000, 0x1f02, 0x0),
    ),
    g(
        "sx_cmpsd_66_28",
        "f2450fc2ce03",
        (9, 0xcaf4d56719b63c29000fffffffffffff),
        (14, 0x25038b42af5451a67fdfffffffffffff),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "s_cmpsd_8_23_2_i03",
        "f20fc2ca03",
        (1, 0xb28947fe0be5464f3ff0000000000001),
        (2, 0x662f9b9a7c2898398000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_29_25_3_i02",
        "f20fc2ca02",
        (1, 0x3bae1c993ee325718010000000000000),
        (2, 0xfcf7fae8d1129606800fffffffffffff),
        0x1f00,
        0x0,
        ok(0x3bae1c993ee32571ffffffffffffffff, 0x1f02, 0x0),
    ),
    g(
        "s_cmpsd_9_24_0_i02",
        "f20fc2ca02",
        (1, 0x588a333f1885795d3fefffffffffffff),
        (2, 0xc5436340111159a9000fffffffffffff),
        0x1f80,
        0x0,
        ok(0x588a333f1885795d0000000000000000, 0x1f82, 0x0),
    ),
    g(
        "sx_cmpsd_22_21",
        "f2450fc2ce07",
        (9, 0x5ae6ee656e8ba8c80008000000000000),
        (14, 0x9f619d362c12aa700000000000000001),
        0x7780,
        0x0,
        ok(0x5ae6ee656e8ba8c8ffffffffffffffff, 0x7782, 0x0),
    ),
    g(
        "s_cmpsd_34_22_0_i07",
        "f20fc2ca07",
        (1, 0xab6ab8741eead3c17fd0000000000000),
        (2, 0x920c4df7598768e0000000000000001),
        0x1f80,
        0x0,
        ok(0xab6ab8741eead3c1ffffffffffffffff, 0x1f82, 0x0),
    ),
    g(
        "s_cmpsd_29_24_2_i05",
        "f20fc2ca05",
        (1, 0xba96e0f63c0060b18010000000000000),
        (2, 0x4efb754c1f508113000fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_27_10_2_i05",
        "f20fc2ca05",
        (1, 0x9dadbe30602842b80000000000000003),
        (2, 0xfb5f6761096b34667ff0000000000000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_45_22_0_i01",
        "f20fc2ca01",
        (1, 0x3370e3e2aaad48c13ff8000000000000),
        (2, 0xe37c2385221c67b50000000000000001),
        0x1f80,
        0x0,
        ok(0x3370e3e2aaad48c10000000000000000, 0x1f82, 0x0),
    ),
    g(
        "s_cmpsd_4_26_3_i01",
        "f20fc2ca01",
        (1, 0xad0878c29360f95f4000000000000000),
        (2, 0xa7e68ca30ab22f20008000000000000),
        0x1f00,
        0x0,
        ok(0xad0878c29360f95f0000000000000000, 0x1f02, 0x0),
    ),
    g(
        "rb_cmpsd_261",
        "f2410fc2f005",
        (6, 0xc3220d6288db2a164320c8c2b18c3910),
        (8, 0xbff99329927e7e0e00039fd2cb91c8c9),
        0x7d80,
        0x0,
        ok(0xc3220d6288db2a16ffffffffffffffff, 0x7d82, 0x0),
    ),
    g(
        "s_cmpsd_4_23_3_i05",
        "f20fc2ca05",
        (1, 0xdc8280de6c7bdbc44000000000000000),
        (2, 0x9250a3f1c1e7a998000000000000001),
        0x1f00,
        0x0,
        ok(0xdc8280de6c7bdbc4ffffffffffffffff, 0x1f02, 0x0),
    ),
    g(
        "s_cmpsd_26_23_2_i02",
        "f20fc2ca02",
        (1, 0x280bb156a35b0b7b0008000000000000),
        (2, 0xde3fd6af5741c46f8000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_1_26_2_i02",
        "f20fc2ca02",
        (1, 0xed3968c2993dbb0e8000000000000000),
        (2, 0x5dd94edc300534c0008000000000000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpsd_15_23",
        "f2450fc2ce03",
        (9, 0xec29ab3d2bd70a907fe0000000000000),
        (14, 0x26a9813670cbe80c8000000000000001),
        0x5b80,
        0x0,
        ok(0xec29ab3d2bd70a900000000000000000, 0x5b82, 0x0),
    ),
    g(
        "s_cmpsd_0_25_3_i03",
        "f20fc2ca03",
        (1, 0xffb398777e55d330000000000000000),
        (2, 0xe0b6079a82d78686800fffffffffffff),
        0x1f00,
        0x0,
        ok(0xffb398777e55d330000000000000000, 0x1f02, 0x0),
    ),
    g(
        "s_cmpsd_37_24_0_i00",
        "f20fc2ca00",
        (1, 0xbd9e85638c29d51f3ca0000000000000),
        (2, 0xa569e7f3f6e80a19000fffffffffffff),
        0x1f80,
        0x0,
        ok(0xbd9e85638c29d51f0000000000000000, 0x1f82, 0x0),
    ),
    g(
        "s_cmpsd_43_27_3_i00",
        "f20fc2ca00",
        (1, 0xb27d154fbc88deba001fffffffffffff),
        (2, 0xe4d7d1fa15d791700000000000000003),
        0x1f00,
        0x0,
        ok(0xb27d154fbc88deba0000000000000000, 0x1f02, 0x0),
    ),
    g(
        "rb_cmpsd_252",
        "f2410fc2cb00",
        (1, 0xbff253c492ebdf94800d1e022d878c00),
        (11, 0xbcbd0de47e6469c7ffe3491d64665844),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "s_cmpsd_23_39_2_i00",
        "f20fc2ca00",
        (1, 0xca24aa2b6fd6d7308000000000000001),
        (2, 0x6c4cb857381fc25a433fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpsd_50_15",
        "f2450fc2ce01",
        (9, 0xf4da8fc6636bc54e0010000000000000),
        (14, 0xf12337f7fe16d22c0008000000000000),
        0x6000,
        0x0,
        fault(0x6002),
    ),
    g(
        "s_cmpsd_24_30_2_i01",
        "f20fc2ca01",
        (1, 0xf954847488468ab5000fffffffffffff),
        (2, 0x3d9c22de2081891d0010000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_33_19_2_i04",
        "f20fc2ca04",
        (1, 0x52c26dc8013150977fe0000000000000),
        (2, 0xc85917b95e11e75f7ff4000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "s_cmpsd_23_19_2_i04",
        "f20fc2ca04",
        (1, 0x1f4d1d07c26ab9ed8000000000000001),
        (2, 0xee0d5fac3cac93db7ff4000000000000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "sx_cmpsd_50_20",
        "f2450fc2ce01",
        (9, 0xa0ca2fed310340e13fe0000000000000),
        (14, 0xea27f68da73167ab7ff7ffffffffffff),
        0x6000,
        0x0,
        fault(0x6001),
    ),
    g(
        "s_cmpsd_19_43_3_i01",
        "f20fc2ca01",
        (1, 0x8e82377b6af494d77ff4000000000000),
        (2, 0x9619086744d38f0001fffffffffffff),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "rb_cmpsd_265",
        "f2410fc2cc04",
        (1, 0x7fd9eedc7b435e5e0010b12a500eaa4b),
        (12, 0xfff5b4f10c8d2a580008278834282f25),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "sx_cmpsd_58_12",
        "f2450fc2ce04",
        (9, 0x27cf51b07d88a9387fd0000000000000),
        (14, 0x46d408bfc60b3c840008000000000000),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "sx_cmpsd_1_5",
        "f2450fc2ce07",
        (9, 0x9f4cd3765d882cc7bff0000000000000),
        (14, 0x9f554185acfca289800fffffffffffff),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "s_cmpsd_22_6_2_i07",
        "f20fc2ca07",
        (1, 0x6a73177a5e6663270000000000000001),
        (2, 0x66f713fb3c10621e4008000000000000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "sx_cmpsd_5_6",
        "f2450fc2ce04",
        (9, 0x1af5e81fe6980130800fffffffffffff),
        (14, 0x5a6fc9f5ef1e430433fffffffffffff),
        0xf80,
        0x0,
        ok(0x1af5e81fe6980130ffffffffffffffff, 0xf82, 0x0),
    ),
    g(
        "s_cmpsd_4_27_3_i04",
        "f20fc2ca04",
        (1, 0x6b5122550bc7ca7c4000000000000000),
        (2, 0xb4811a2032280ddb0000000000000003),
        0x1f00,
        0x0,
        ok(0x6b5122550bc7ca7cffffffffffffffff, 0x1f02, 0x0),
    ),
    g(
        "s_cmpsd_37_25_2_i06",
        "f20fc2ca06",
        (1, 0xad611be06a17078c3ca0000000000000),
        (2, 0x45bf2c9183b0e85f800fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "s_cmpsd_10_25_2_i06",
        "f20fc2ca06",
        (1, 0xbd87ef0c04c70f1a7ff0000000000000),
        (2, 0xe685d16546a52f4c800fffffffffffff),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "p_cmpps_67_15",
        "440fc2d907",
        (11, 0x74f236e580000000807fffff42ba387e),
        (1, 0xb5a22c9a000000033400000040a00000),
        0x1d80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1d82, 0x0),
    ),
    g(
        "p_cmpps_13_46",
        "450fc2cf07",
        (9, 0x8000018080000000a115b63f800000),
        (15, 0x31507b4fac36d07effffff7e800000),
        0xf80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0xf82, 0x0),
    ),
    g(
        "p_cmpps_69_47",
        "450fc2c707",
        (8, 0x4b7fffff3f956a17cfb57ce37fc00001),
        (15, 0xff8000014019bb89bf8000003fc00000),
        0x1780,
        0x0,
        ok(0xffffffffffffffff00000000, 0x1781, 0x0),
    ),
    g(
        "p_cmpps_35_47",
        "450fc2ce07",
        (9, 0xff4c677c3f8000010b1059780b2cdbfc),
        (14, 0x330bfd6b40a0000000400000ff800001),
        0x1ec0,
        0x0,
        ok(0xffffffffffffffffffffffff00000000, 0x1ec1, 0x0),
    ),
    g(
        "p_cmpps_65_33",
        "450fc2dc02",
        (11, 0xbf800000ff80000100ffffff00f910ec),
        (12, 0x4b0000007fbfffffaf6f29d2c0000000),
        0xfbf,
        0x0,
        ok(0xffffffff000000000000000000000000, 0xfbf, 0x0),
    ),
    g(
        "p_cmpps_61_52",
        "410fc2c102",
        (0, 0x34000000bf80000000003eefff7fffff),
        (9, 0xc00000003f8000017f0000003fc00000),
        0x80c0,
        0x0,
        ok(0xffffffffffffffffffffffff, 0x80c0, 0x0),
    ),
    g(
        "rb_cmpps_58",
        "450fc2fd06",
        (15, 0xf4b700cb0054ff24e9ac785400f91a59),
        (13, 0x80ae55bcffdd1e588177b928b0561899),
        0x8c8a,
        0x0,
        ok(0xffffffff00000000ffffffff, 0x8c8b, 0x0),
    ),
    g(
        "p_cmpps_47_52",
        "450fc2d006",
        (10, 0x7fc00000c00000007f8000007fcab021),
        (8, 0xffbfffff808000003f0000007fc00001),
        0x7ec0,
        0x0,
        ok(0xffffffff00000000ffffffffffffffff, 0x7ec1, 0x0),
    ),
    g(
        "p_cmpps_70_13",
        "0fc2c707",
        (0, 0xdc4f08f08089964b807fffff4b7fffff),
        (7, 0x7f800001ffc123450b4a76e30002703e),
        0xf80,
        0x0,
        ok(0xffffffffffffffff, 0xf83, 0x0),
    ),
    g(
        "p_cmpps_40_21",
        "450fc2eb07",
        (13, 0x74c439183f800001008000013f000000),
        (11, 0x7fbfffff800000017fffffffbfa62d06),
        0x9780,
        0x0,
        ok(0xffffffff00000000ffffffff, 0x9783, 0x0),
    ),
    g(
        "p_cmpps_54_43",
        "410fc2d405",
        (2, 0x7fffffff008000010000000300800000),
        (12, 0x7fb3b0fd40a000003fe5130cff663ade),
        0xef80,
        0x0,
        ok(0xffffffff0000000000000000ffffffff, 0xef83, 0x0),
    ),
    g(
        "p_cmpps_5_53",
        "440fc2ea05",
        (13, 0x7fffff007fffffffbfffffff5b3e97),
        (2, 0x338000007fc2bd783f80000000ffffff),
        0x9f80,
        0x0,
        ok(0xffffffffffffffff00000000, 0x9f83, 0x0),
    ),
    g(
        "p_cmpps_72_6",
        "450fc2ea03",
        (13, 0x7fc000007effffff800000013f000000),
        (10, 0x800000338000004b0000004f229508),
        0x1700,
        0x0,
        ok(0xffffffff000000000000000000000000, 0x1702, 0x0),
    ),
    g(
        "p_cmpps_0_10",
        "0fc2fb03",
        (7, 0x40a0000040000000005a164eff4fb73c),
        (3, 0x4b00000040a00000b02fe2a73f800001),
        0x1f80,
        0x0,
        ok(0x0, 0x1f82, 0x0),
    ),
    g(
        "rb_cmpps_27",
        "410fc2e506",
        (4, 0x30613ea2f4e62d59cff2037f8083308a),
        (13, 0xd50b11ffcc0e783fc05c8a00200d81),
        0x1780,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1783, 0x0),
    ),
    g(
        "p_cmpps_40_11",
        "410fc2e006",
        (4, 0xff7fffff007fffff4febd93080bfef12),
        (8, 0xb07c3598ffc123457fa0000000000003),
        0x9780,
        0x0,
        ok(0xffffffffffffffff00000000, 0x9783, 0x0),
    ),
    g(
        "p_cmpps_69_46",
        "440fc2f200",
        (14, 0x7f80d029c00000000000000380000001),
        (2, 0x33800000000000004666f13a74b07507),
        0x1780,
        0x0,
        ok(0x0, 0x1783, 0x0),
    ),
    g(
        "p_cmpps_12_48",
        "440fc2d100",
        (10, 0x3f8000017fc00001010000003fc7c2c4),
        (1, 0xcfe0e63900000000ffa49ce6805c6849),
        0x1780,
        0x0,
        ok(0x0, 0x1783, 0x0),
    ),
    g(
        "rb_cmpps_283",
        "0fc2e405",
        (4, 0x7ffc8ebf74d25c55815836410040bf2a),
        (4, 0x7ffc8ebf74d25c55815836410040bf2a),
        0x77c0,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x77c1, 0x0),
    ),
    g(
        "p_cmpps_2_4",
        "410fc2f005",
        (6, 0xb12e9797f8000007effffff80800000),
        (8, 0x7fc000007f8000013fac066c00fc3ccc),
        0x5f80,
        0x0,
        ok(0xffffffffffffffffffffffff00000000, 0x5f81, 0x0),
    ),
    g(
        "p_cmpps_30_14",
        "0fc2c200",
        (0, 0x4b7fffff006920f100ffffffc0000000),
        (2, 0x3f7fffff3f80000140a000007fc00000),
        0x7780,
        0x0,
        ok(0x0, 0x7782, 0x0),
    ),
    g(
        "p_cmpps_8_5",
        "0fc2fe00",
        (7, 0x3f800000c02953d0305eef18007fffff),
        (6, 0x7fc000007f7fffff7f80000080800000),
        0x1f00,
        0x0,
        ok(0x0, 0x1f02, 0x0),
    ),
    g(
        "rb_cmpps_91",
        "410fc2e802",
        (5, 0x8b51528a0059991c0b3757c44fe86b97),
        (8, 0x16f90edfff950a174b63ce27f05e8d1),
        0x7780,
        0x0,
        ok(0xffffffff00000000ffffffffffffffff, 0x7781, 0x0),
    ),
    g(
        "p_cmpps_1_0",
        "450fc2d702",
        (10, 0x403c64a27fc000016e82f920ffc00000),
        (15, 0x7effffff010000000000000034000000),
        0x3f80,
        0x0,
        ok(0xffffffff000000000000000000000000, 0x3f81, 0x0),
    ),
    g(
        "p_cmpps_69_36",
        "0fc2d502",
        (2, 0x74caca5d008000007fbfffff80000000),
        (5, 0x5f36f7f4aeb3edff8000010115508a),
        0x1780,
        0x0,
        ok(0xffffffff, 0x1783, 0x0),
    ),
    g(
        "p_cmpps_16_33",
        "450fc2c802",
        (9, 0x80b21d0d812882183e8000000b2e37e8),
        (8, 0x7fffffff014a89adc0000000802b349b),
        0x3d80,
        0x0,
        ok(0xffffffff0000000000000000, 0x3d83, 0x0),
    ),
    g(
        "rb_cmpps_86",
        "410fc2e501",
        (4, 0xc011727180be75c07f2d199c3fc5ebe9),
        (13, 0x8165c4083f99bdb400f9b9500b36b323),
        0xc02f,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0xc02f, 0x0),
    ),
    g(
        "p_cmpps_30_32",
        "410fc2fc01",
        (7, 0x8b40e4a53fc0000000ffffff4b7fffff),
        (12, 0x7effffffcfd54f83816467ee00800000),
        0x7780,
        0x0,
        ok(0xffffffff000000000000000000000000, 0x7780, 0x0),
    ),
    g(
        "p_cmpps_73_15",
        "440fc2d901",
        (11, 0x800000003ffcd9a3ffc1234500ffffff),
        (1, 0x4cf595fbc054fbb430562ca3ff8a5961),
        0x380,
        0x0,
        ok(0xffffffff000000000000000000000000, 0x381, 0x0),
    ),
    g(
        "p_cmpps_43_46",
        "410fc2dd01",
        (3, 0x7f7fffff007fffffc0000000ffc00000),
        (13, 0x40ed34143fc000007fc00000524cee01),
        0xfc0,
        0x0,
        ok(0xffffffff0000000000000000, 0xfc1, 0x0),
    ),
    g(
        "p_cmpps_40_44",
        "440fc2c001",
        (8, 0xbf8f4d84800000014055b07a0021f5dc),
        (0, 0x7fc000007f7ef7e3ffc0000080000000),
        0x9780,
        0x0,
        ok(0xffffffff0000000000000000, 0x9783, 0x0),
    ),
    g(
        "p_cmpps_1_9",
        "0fc2c101",
        (0, 0x807fffff7fc0000040a000007fffffff),
        (1, 0xff800000c0000000000000017fc00001),
        0x3f80,
        0x0,
        ok(0x0, 0x3f83, 0x0),
    ),
    g(
        "p_cmpps_70_50",
        "410fc2e800",
        (5, 0x7fffffffffc12345ff800001f4fa44c2),
        (8, 0x809897b88b067770807fffffff800000),
        0xf80,
        0x0,
        ok(0x0, 0xf81, 0x0),
    ),
    g(
        "p_cmpps_41_36",
        "410fc2dd00",
        (3, 0x7fa0000001000000ff800001ff800001),
        (13, 0x7f800000ffc12345bfabbfa03e800000),
        0x17c0,
        0x0,
        ok(0x0, 0x17c1, 0x0),
    ),
    g(
        "rb_cmpps_24",
        "0fc2fe04",
        (7, 0xc066521bffbf78bbc0335bbd810bafa8),
        (6, 0x74ccb2bb8119983730384a00011ca91b),
        0x3e80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x3e81, 0x0),
    ),
    g(
        "p_cmpps_25_46",
        "0fc2d404",
        (2, 0xffc123457fbfffff40a0000080800000),
        (4, 0x4b00000030025f3b01000000ff7fffff),
        0x4f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x4f81, 0x0),
    ),
    g(
        "p_cmpps_50_41",
        "440fc2cc04",
        (9, 0xffbfffff007fffffd10f32a87f938363),
        (4, 0x3f0000007f8000000080000000ffffff),
        0xfb80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0xfb83, 0x0),
    ),
    g(
        "p_cmpps_1_27",
        "0fc2c404",
        (0, 0x3f000000ffbfffff00ffffff807fffff),
        (4, 0x8000003034e695340000003f7fffff),
        0x3f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x3f83, 0x0),
    ),
    g(
        "p_cmpps_48_19",
        "450fc2d003",
        (10, 0xffbfffff7fffffffcfd6d3a2007fffff),
        (8, 0x7fffff7fc000000b2c966200800001),
        0xfd80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0xfd83, 0x0),
    ),
    g(
        "p_cmpps_2_31",
        "450fc2c603",
        (8, 0x3f8000004b8000007f80000100400000),
        (14, 0x3f000000ff8000013f80000001135129),
        0x5f80,
        0x0,
        ok(0xffffffffffffffff00000000, 0x5f83, 0x0),
    ),
    g(
        "p_cmpps_62_28",
        "0fc2c103",
        (0, 0xbfa6048c80800000bf9ad7457e800000),
        (1, 0x7f49e578e7c0eaf73f7fffff7fc00000),
        0x1fbf,
        0x0,
        ok(0xffffffff, 0x1fbf, 0x0),
    ),
    g(
        "p_cmpps_35_19",
        "0fc2c403",
        (0, 0x4b0000008b1182643fc00000ff08790e),
        (4, 0x7f2115f48b279f63007fffffffc12345),
        0x1ec0,
        0x0,
        ok(0xffffffff, 0x1ec0, 0x0),
    ),
    g(
        "p_cmpps_70_3",
        "440fc2e305",
        (12, 0x10000003f7fffff81787e677f448ba6),
        (3, 0x14000000000acc35100000003),
        0xf80,
        0x0,
        ok(0xffffffff0000000000000000ffffffff, 0xf82, 0x0),
    ),
    g(
        "p_cmpps_1_4",
        "410fc2e905",
        (5, 0x40000000000000038170dacf3f800001),
        (9, 0xffffff7f000000bfb2ced87f0cafb6),
        0x3f80,
        0x0,
        ok(0xffffffff00000000ffffffff00000000, 0x3f82, 0x0),
    ),
    g(
        "p_cmpps_55_8",
        "440fc2e703",
        (12, 0x8000007fffffff4b0000008b23990e),
        (7, 0xff97f7d33f7fffff7fa000007fc00000),
        0x6fc0,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x6fc1, 0x0),
    ),
    g(
        "p_cmpps_1_40",
        "440fc2c303",
        (8, 0xff8000017f8000013f7fffff7f000000),
        (3, 0x4b7fffffcfaa4c267e8000007fc00001),
        0x3f80,
        0x0,
        ok(0xffffffffffffffff00000000ffffffff, 0x3f81, 0x0),
    ),
    g(
        "rb_cmpps_172",
        "440fc2fa04",
        (15, 0xcfd6a46c81340ab43fef18ff80656ea6),
        (2, 0x3fa99a836441e3c4806f8050c0422dd6),
        0x3780,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x3782, 0x0),
    ),
    g(
        "p_cmpps_2_38",
        "410fc2cb04",
        (1, 0xc0000000b03038dc7fffffff00800001),
        (11, 0x80800000003d989700000003406783a5),
        0x5f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x5f82, 0x0),
    ),
    g(
        "p_cmpps_52_32",
        "0fc2dd06",
        (3, 0xc0000000807fffff4b7fffffbf800000),
        (5, 0x8b506615ff7fffff0080000174990f75),
        0xf780,
        0x0,
        ok(0xffffffffffffffff00000000, 0xf782, 0x0),
    ),
    g(
        "p_cmpps_1_34",
        "0fc2ce06",
        (1, 0x400000003fc00000004000000b641500),
        (6, 0x3ffae041ff800000f4e76f197e800000),
        0x3f80,
        0x0,
        ok(0xffffffffffffffffffffffff00000000, 0x3f82, 0x0),
    ),
    g(
        "p_cmpps_71_52",
        "440fc2ee04",
        (13, 0xbff22aa5b002d1a4cffb971540000000),
        (6, 0x7f7fffff40000000bf8000007f800000),
        0x1f00,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1f00, 0x0),
    ),
    g(
        "p_cmpps_13_25",
        "0fc2c904",
        (1, 0x800000010000007f7fffff3fc00000),
        (1, 0x800000010000007f7fffff3fc00000),
        0xf80,
        0x0,
        ok(0x0, 0xf80, 0x0),
    ),
    g(
        "rb_cmpps_118",
        "0fc2fc00",
        (7, 0x3072c3690056b60b0b0047b4ff2ccc35),
        (4, 0xbfebb809005ca19200687a85003acaec),
        0x4d8f,
        0x0,
        ok(0x0, 0x4d8f, 0x0),
    ),
    g(
        "p_cmpps_25_5",
        "440fc2da00",
        (11, 0x807fffff00e938067e8000007f800000),
        (2, 0x7fcb1c15008000010b3a7764cfa52e9f),
        0x4f80,
        0x0,
        ok(0x0, 0x4f80, 0x0),
    ),
    g(
        "p_cmpps_73_55",
        "450fc2ef02",
        (13, 0x8145f7733400000080149ce900800000),
        (15, 0x74bde4c93f7fffff0b77a27a0178ac36),
        0x380,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x382, 0x0),
    ),
    g(
        "p_cmpps_8_51",
        "0fc2c202",
        (0, 0x33800000ff1ff5a54000000080a50d37),
        (2, 0x6b840c78ceeee06400abacfd00000001),
        0x1f00,
        0x0,
        ok(0xffffffffffffffff00000000ffffffff, 0x1f02, 0x0),
    ),
    g(
        "p_cmpps_64_50",
        "0fc2c906",
        (1, 0x808000000040000000ffffff303eb87d),
        (1, 0x808000000040000000ffffff303eb87d),
        0x5faa,
        0x0,
        ok(0x0, 0x5faa, 0x0),
    ),
    g(
        "p_cmpps_60_16",
        "0fc2da06",
        (3, 0x80bab6d8f4d5623ac0506e0233800000),
        (2, 0x3e8000007f32d6153ffb00f6ff800000),
        0x40,
        0x0,
        ok(0xffffffff, 0x40, 0x0),
    ),
    g(
        "rb_cmpps_239",
        "410fc2f501",
        (6, 0xf4a8d3fa0b530969bff03a8a8071b2f1),
        (13, 0x406d86ab008a796d80e8e69c8b5fb05b),
        0x9f00,
        0x0,
        ok(0xffffffff00000000ffffffff00000000, 0x9f02, 0x0),
    ),
    g(
        "p_cmpps_48_5",
        "0fc2e701",
        (4, 0x7f7fffff307cb03e7f80000080000001),
        (7, 0x3cffe57f8000007f0000003e800000),
        0xfd80,
        0x0,
        ok(0xffffffff00000000ffffffff, 0xfd82, 0x0),
    ),
    g(
        "rb_cmpps_132",
        "0fc2cc07",
        (1, 0xff1297b0807ac0a760963a13b0b97ef9),
        (4, 0x80f9d9f03049cd9000749d2c811f2f4d),
        0x9963,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x9963, 0x0),
    ),
    g(
        "p_cmpps_32_41",
        "440fc2ce07",
        (9, 0x40e4170b6428e17fc0000180000000),
        (6, 0x7fffffffbf8000008b72ff3b34000000),
        0x9f00,
        0x0,
        ok(0xffffffff00000000ffffffff, 0x9f00, 0x0),
    ),
    g(
        "rb_cmpps_144",
        "410fc2e405",
        (4, 0x803268efc05df665804692db81228846),
        (12, 0x8165cf6b8097870b4f9b71194054fec4),
        0x6a76,
        0x0,
        ok(0xffffffff000000000000000000000000, 0x6a76, 0x0),
    ),
    g(
        "p_cmpps_58_41",
        "440fc2f205",
        (14, 0xbf800000f4947cd874a9a95c3f800000),
        (2, 0x1531f590b720ca54b800000ff7fffff),
        0x6000,
        0x0,
        ok(0xffffffffffffffff, 0x6000, 0x0),
    ),
    g(
        "p_cmpps_74_15",
        "410fc2c906",
        (1, 0xffc123453f7fffff3e800000817c91f3),
        (9, 0x7f0000007fc000000000000000645c22),
        0x1a00,
        0x0,
        fault(0x1a03),
    ),
    g(
        "p_cmpps_26_40",
        "410fc2cb06",
        (1, 0xff80000140a00000ff8000003f000000),
        (11, 0xb07f34a1ff800000000000033e800000),
        0x7f00,
        0x0,
        fault(0x7f03),
    ),
    g(
        "p_cmpps_66_46",
        "440fc2c600",
        (8, 0x40400000bf98800c40000000807fffff),
        (6, 0xb01f5b0f3f8000003f00000080000001),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmpps_21_5",
        "450fc2dd00",
        (11, 0x380134a1100800001bf800000),
        (13, 0x4b0000004b7fffff4b800000ff3396e7),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "rb_cmpps_109",
        "410fc2e803",
        (5, 0xbe035f403e8c1cb0654891c066f3ee),
        (8, 0x74fe10e2f482a91d802b314374f60e33),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmpps_21_39",
        "440fc2ca03",
        (9, 0x7ca48dff80000074866768ffc12345),
        (2, 0xc0000000ffc0000040000000bf800000),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "p_cmpps_58_54",
        "440fc2e503",
        (12, 0x4b7fffff7fbfffff7fa000004b7fffff),
        (5, 0x5a309f7fc00001400000003fc00000),
        0x6000,
        0x0,
        fault(0x6003),
    ),
    g(
        "p_cmpps_7_11",
        "0fc2da03",
        (3, 0xc0000000bfcf29fb0040000000800000),
        (2, 0x7f8000014b7fffff40a000003e800000),
        0x0,
        0x0,
        fault(0x3),
    ),
    g(
        "p_cmpps_57_42",
        "410fc2da05",
        (3, 0xcfe765d6ff8000007f80000000400000),
        (10, 0xbf800000010000007f8000007fbfffff),
        0x4000,
        0x0,
        fault(0x4001),
    ),
    g(
        "p_cmpps_45_35",
        "450fc2cf05",
        (9, 0x3f7fffff80056fb9ffbfffff4fec83b1),
        (15, 0xff800001ff4aa8a10079bcab00800001),
        0x7f40,
        0x0,
        fault(0x7f41),
    ),
    g(
        "p_cmpps_66_0",
        "410fc2f401",
        (6, 0x80000000007fffff0078472c0034707d),
        (12, 0x4000000030075dd4c01883223f800001),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmpps_7_46",
        "0fc2e501",
        (4, 0x1def7fd79f48e26d50a745952),
        (5, 0xd92125074063722b3e800000ff800000),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "rb_cmpps_238",
        "440fc2e702",
        (12, 0xffef59818b5c6865748c72b0cf80ae9f),
        (7, 0x74fa77be7f4aa7998b4402db0b6c4cdb),
        0x5f00,
        0x0,
        fault(0x5f01),
    ),
    g(
        "p_cmpps_8_27",
        "410fc2f602",
        (6, 0x7f82033140400000ffbfffff33800000),
        (14, 0x4000007fffffffff80000000800000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_cmpps_56_6",
        "440fc2f900",
        (15, 0x7f800001c064064c808000003fc00000),
        (1, 0x40a0000000000001008000007fa00000),
        0x2000,
        0x0,
        fault(0x2003),
    ),
    g(
        "p_cmpps_44_44",
        "0fc2d900",
        (3, 0x8b5f102a8b0989c54b800000ff800000),
        (1, 0x7fc000007f4d737b807fffff7fbfffff),
        0xff00,
        0x0,
        fault(0xff03),
    ),
    g(
        "rb_cmpps_299",
        "440fc2c507",
        (8, 0x801eb0b47fe05f8fffe3e7167470f499),
        (5, 0xcf9719b8b0331b949c2b251074d709a8),
        0x4000,
        0x0,
        fault(0x4002),
    ),
    g(
        "p_cmpps_9_31",
        "450fc2e507",
        (12, 0x4b80000000400000bf800000007fffff),
        (13, 0xf4be06dc0000000100800000ffde8187),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmpps_59_1",
        "450fc2d804",
        (11, 0xffc123457e8000008000000040000000),
        (8, 0xcfc9be9d7fbfffff00000000306fd704),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "p_cmpps_7_33",
        "0fc2c804",
        (1, 0x3f8000007effffff7fb419a77f742455),
        (0, 0x404000003f0000007e800000bf800000),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "rb_cmpps_295",
        "450fc2cb01",
        (9, 0x8d9d2f0e00715d6f302f7b2f3f975204),
        (11, 0x8b5f163340295aa230058ac4ffc6478c),
        0xc932,
        0x0,
        fault(0xc933),
    ),
    g(
        "p_cmpps_14_28",
        "410fc2c601",
        (0, 0xbf8000003fddf2eeffbfffffffc00000),
        (14, 0x7fffffff7f7fffff7f5faf9e3f800001),
        0x3f00,
        0x0,
        fault(0x3f01),
    ),
    g(
        "p_cmpps_74_47",
        "450fc2dc05",
        (11, 0xbfa505037fc00000000000017fbfffff),
        (12, 0xff7fffff3f800000800000000055a622),
        0x1a00,
        0x0,
        fault(0x1a03),
    ),
    g(
        "p_cmpps_21_42",
        "450fc2cb05",
        (9, 0x4b80000080000001749b8ad1b021b090),
        (11, 0x340000007e80000040000000ffc12345),
        0x5e80,
        0x0,
        fault(0x5e83),
    ),
    g(
        "p_cmpps_72_7",
        "450fc2e001",
        (12, 0xffacd9623f8000004ff9128e007fffff),
        (8, 0x1ff7fffffff80000040463530),
        0x1700,
        0x0,
        fault(0x1703),
    ),
    g(
        "p_cmpps_8_12",
        "450fc2f401",
        (14, 0x4b7fffff807fffff4b7fffff7fa00000),
        (12, 0x338000004b000000808000004b800000),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "rb_cmpps_189",
        "0fc2e902",
        (5, 0x8174dfd5ff03b965002e0cf9ff3d752e),
        (1, 0x30498a7958cf72db304c41190b0a7525),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "p_cmpps_15_33",
        "450fc2fe02",
        (15, 0xb04858a780000001006bc30380ca2730),
        (14, 0xff46c3a5bf800000000000014b800000),
        0x3e80,
        0x0,
        fault(0x3e82),
    ),
    g(
        "p_cmpps_57_23",
        "440fc2c002",
        (8, 0x163f8ba3fc0000000d93536ff51e280),
        (0, 0x3f800001807fffff7f80000174c6d88f),
        0x4000,
        0x0,
        fault(0x4003),
    ),
    g(
        "p_cmpps_14_15",
        "440fc2fd02",
        (15, 0xc00000007e800000749b169c304f6fd9),
        (5, 0x7fa000004b0000008000000100000001),
        0x3f00,
        0x0,
        fault(0x3f03),
    ),
    g(
        "p_cmpps_60_29",
        "440fc2c600",
        (8, 0xff8000013fc000003fc000007fc00000),
        (6, 0x7f800000400000007f00000080000000),
        0x40,
        0x0,
        fault(0x41),
    ),
    g(
        "p_cmpps_14_36",
        "440fc2eb00",
        (13, 0x7effffff4b8000007f0000007fa00000),
        (3, 0x7effffffffc1234500ffffff007fffff),
        0x3f00,
        0x0,
        fault(0x3f01),
    ),
    g(
        "p_cmpps_74_12",
        "440fc2fe06",
        (15, 0x1000000bf800000007fffff4b800000),
        (6, 0x7f0274464b7fffffff7fffff40400000),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "p_cmpps_21_36",
        "0fc2e606",
        (4, 0x7effffff3f7fffff8002097800400000),
        (6, 0xff8000007e8000000100000080000000),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "p_cmpps_66_25",
        "440fc2c904",
        (9, 0x304362037effffff4040000001000000),
        (1, 0xffc0000000bdda0580007670bf800000),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmpps_57_52",
        "410fc2e004",
        (4, 0x4fbaf5e1807fffffff80000000800001),
        (8, 0x8b625ee1251c83473f8000013e800000),
        0x4000,
        0x0,
        fault(0x4002),
    ),
    g(
        "p_cmpps_72_42",
        "0fc2da03",
        (3, 0xffc00000ffc123457fbfffff4b800000),
        (2, 0x3f7fffff400000004b8000004b000000),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_cmpps_26_55",
        "440fc2d703",
        (10, 0xffc000007fa000004b0000003fc00000),
        (7, 0x7f8000003f00000034000000c0000000),
        0x7f00,
        0x0,
        fault(0x7f01),
    ),
    g(
        "p_cmpps_71_54",
        "0fc2c504",
        (0, 0x800000007fc0000000000000ffbfffff),
        (5, 0x8058ca98ff8000013fc000003fc00000),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "p_cmpps_20_29",
        "440fc2cc04",
        (9, 0xff7e8c73000000004b000000ff800001),
        (4, 0x7fffff00000003400000007f000000),
        0x5f00,
        0x0,
        fault(0x5f03),
    ),
    g(
        "p_cmpps_60_14",
        "410fc2ec07",
        (5, 0x3f80000080d26e10ff80000134000000),
        (12, 0x34b000000ff8000017fbfffff),
        0x40,
        0x0,
        fault(0x41),
    ),
    g(
        "p_cmpps_32_38",
        "440fc2d207",
        (10, 0xb066137b008000003400000000000000),
        (2, 0x3fc00000ffbfffff8000000034000000),
        0x9f00,
        0x0,
        fault(0x9f01),
    ),
    g(
        "p_cmpps_56_0",
        "410fc2f906",
        (7, 0x4fd0c280ff800001008000014b000000),
        (9, 0xff800001002d7a843f0000007f7fffff),
        0x2000,
        0x0,
        fault(0x2001),
    ),
    g(
        "p_cmpps_57_28",
        "410fc2f106",
        (6, 0x3027fde5000000007f80000033800000),
        (9, 0x7f29e39f7fbffffff4a866f640000000),
        0x4000,
        0x0,
        fault(0x4001),
    ),
    g(
        "p_cmpps_56_29",
        "450fc2c107",
        (8, 0x800000018071c9b8007fffff00000000),
        (9, 0xd02dc86a0009eca77fa0000040000000),
        0x2000,
        0x0,
        fault(0x2003),
    ),
    g(
        "p_cmpps_21_17",
        "0fc2ce07",
        (1, 0x400000007fbfffff80495471807fffff),
        (6, 0xffc123454fe3a1da800000003f000000),
        0x5e80,
        0x0,
        fault(0x5e83),
    ),
    g(
        "p_cmpps_34_25",
        "450fc2d505",
        (10, 0x800000340000007f0bf9dc33800000),
        (13, 0x3ff034e8006e17af80000000b056b37f),
        0x9e80,
        0x0,
        fault(0x9e82),
    ),
    g(
        "p_cmpps_9_58",
        "410fc2fd05",
        (7, 0x40a0000040000000f4bffcb840a00000),
        (13, 0x3e800000000000013f7fffff3f8de221),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmppd_65_54",
        "66450fc2ea07",
        (13, 0xfff00000000000003fe0000000000000),
        (10, 0x7ff7ffffffffffff7fe0000000000000),
        0xfbf,
        0x0,
        ok(0xffffffffffffffff, 0xfbf, 0x0),
    ),
    g(
        "p_cmppd_0_42",
        "66450fc2dd07",
        (11, 0x7fd5e269e11a659c0017be8900f9da83),
        (13, 0x40000000000000004008000000000000),
        0x1f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1f80, 0x0),
    ),
    g(
        "p_cmppd_40_40",
        "660fc2ff01",
        (7, 0x80100000000000007ff7ffffffffffff),
        (7, 0x80100000000000007ff7ffffffffffff),
        0x9780,
        0x0,
        ok(0x0, 0x9781, 0x0),
    ),
    g(
        "p_cmppd_5_37",
        "66410fc2ee01",
        (5, 0x8010000000000000fff8000000000000),
        (14, 0xfff00000000000003ca0000000000000),
        0x9f80,
        0x0,
        ok(0x0, 0x9f81, 0x0),
    ),
    g(
        "p_cmppd_50_40",
        "66440fc2ce04",
        (9, 0x7ff0000000000001800f955f7eb528ff),
        (6, 0x3ff0000000000001fff0000000000000),
        0xfb80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0xfb83, 0x0),
    ),
    g(
        "p_cmppd_0_32",
        "66440fc2c504",
        (8, 0x4b8cf0665793d3ca0000000000000),
        (5, 0x43300000000000007ff7ffffffffffff),
        0x1f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1f83, 0x0),
    ),
    g(
        "p_cmppd_69_57",
        "66410fc2f104",
        (6, 0x40000000000000008010000000000000),
        (9, 0x10000000000001000fffffffffffff),
        0x1780,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1782, 0x0),
    ),
    g(
        "p_cmppd_2_42",
        "66410fc2c504",
        (0, 0x80000000000000007fe817396d72479a),
        (13, 0xfffffffffffff3cb0000000000000),
        0x5f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x5f82, 0x0),
    ),
    g(
        "p_cmppd_68_39",
        "66410fc2f905",
        (7, 0x8009fb4a34ffd0f00000000000000003),
        (9, 0xfff00000000000014001e3ce3e2cd837),
        0x1b80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1b83, 0x0),
    ),
    g(
        "p_cmppd_1_6",
        "660fc2d505",
        (2, 0x433fffffffffffff8009cbf2940bff1b),
        (5, 0xfff8123456789abc4008000000000000),
        0x3f80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x3f83, 0x0),
    ),
    g(
        "p_cmppd_72_50",
        "66410fc2f507",
        (6, 0xc0000000000000008010000000000000),
        (13, 0x3ff80000000000008000000000000001),
        0x1700,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1702, 0x0),
    ),
    g(
        "p_cmppd_17_28",
        "660fc2ff07",
        (7, 0x30020000000000000),
        (7, 0x30020000000000000),
        0x3b80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x3b82, 0x0),
    ),
    g(
        "p_cmppd_68_24",
        "66410fc2c904",
        (1, 0x7ff40000000000008010000000000000),
        (9, 0x7ff80000000000013cbbfb8ad2da3870),
        0x1b80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1b81, 0x0),
    ),
    g(
        "p_cmppd_5_27",
        "66410fc2db04",
        (3, 0xfff0000000000001bffff224cedaf695),
        (11, 0x40000000000000007fd0000000000000),
        0x9f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x9f81, 0x0),
    ),
    g(
        "p_cmppd_70_47",
        "660fc2cb00",
        (1, 0xffefffffffffffff3cb0000000000000),
        (3, 0x7fefffffffffffff43237ad490301dde),
        0xf80,
        0x0,
        ok(0x0, 0xf80, 0x0),
    ),
    g(
        "p_cmppd_35_18",
        "66440fc2ec00",
        (13, 0x7fd00000000000000000000000000000),
        (4, 0xbff727526d8b1bb94327bd3f735083a4),
        0x1ec0,
        0x0,
        ok(0x0, 0x1ec0, 0x0),
    ),
    g(
        "p_cmppd_68_34",
        "66450fc2ed06",
        (13, 0x25e13cc3c1cab17fffffffffffffff),
        (13, 0x25e13cc3c1cab17fffffffffffffff),
        0x1b80,
        0x0,
        ok(0xffffffffffffffff, 0x1b81, 0x0),
    ),
    g(
        "p_cmppd_1_49",
        "66440fc2e206",
        (12, 0x7ff8000000000001bff0968566f418bd),
        (2, 0x4008000000000000bca03b6b7e9521a3),
        0x3f80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x3f81, 0x0),
    ),
    g(
        "rb_cmppd_37",
        "660fc2fa02",
        (7, 0xc32bd88d694696770018cd96b7a43d87),
        (2, 0x80234cdb1b23eb7fc001463df9383396),
        0x1780,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1780, 0x0),
    ),
    g(
        "p_cmppd_0_33",
        "66440fc2d602",
        (10, 0x3fe0000000000000ffed9418777145c9),
        (6, 0x400ecb5c16a164c67fe0000000000000),
        0x1f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1f80, 0x0),
    ),
    g(
        "p_cmppd_53_11",
        "660fc2dd02",
        (3, 0x7fd0000000000000fff0000000000000),
        (5, 0x7feffffffffffffffff0000000000001),
        0x77c0,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x77c1, 0x0),
    ),
    g(
        "p_cmppd_36_15",
        "66450fc2fa02",
        (15, 0xfff0000000000000fff0000000000000),
        (10, 0xfff00000000000007ff0000000000001),
        0x9d80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x9d81, 0x0),
    ),
    g(
        "rb_cmppd_27",
        "66440fc2c802",
        (9, 0x8012608ee08abed77fe7aceae1ce8608),
        (0, 0xb994bc810370f3cbbe84d769caa7b),
        0xb91c,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0xb91e, 0x0),
    ),
    g(
        "p_cmppd_52_6",
        "66410fc2fc02",
        (7, 0x35de887c37b4416f5),
        (12, 0x7fe0000000000000e9bbe54b130891c1),
        0xf780,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0xf782, 0x0),
    ),
    g(
        "p_cmppd_70_55",
        "66450fc2e703",
        (12, 0x3fefffffffffffff801e03cb7beb2924),
        (15, 0x7fe0000000000000fff8123456789abc),
        0xf80,
        0x0,
        ok(0xffffffffffffffff, 0xf80, 0x0),
    ),
    g(
        "p_cmppd_40_32",
        "66410fc2df03",
        (3, 0xffeffffffffffffffffb8b581756cade),
        (15, 0x3fd00000000000007fefffffffffffff),
        0x9780,
        0x0,
        ok(0xffffffffffffffff, 0x9780, 0x0),
    ),
    g(
        "p_cmppd_51_17",
        "66440fc2eb05",
        (13, 0x43400000000000003ff8000000000000),
        (3, 0x7ffdc65d87e3bc59fff0000000000001),
        0x7bc0,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x7bc1, 0x0),
    ),
    g(
        "p_cmppd_2_58",
        "66440fc2d605",
        (10, 0xffefffffffffffff0008000000000000),
        (6, 0x40080000000000007ff789d5e99f28f8),
        0x5f80,
        0x0,
        ok(0xffffffffffffffff, 0x5f81, 0x0),
    ),
    g(
        "p_cmppd_70_29",
        "66440fc2df00",
        (11, 0x800000000000080036a07f8666ddb),
        (7, 0x3cb0000000000000000fffffffffffff),
        0xf80,
        0x0,
        ok(0x0, 0xf82, 0x0),
    ),
    g(
        "p_cmppd_2_2",
        "66450fc2fd00",
        (15, 0x3fd0000000000000a213f8f15048ca4c),
        (13, 0x8000000000000001),
        0x5f80,
        0x0,
        ok(0x0, 0x5f82, 0x0),
    ),
    g(
        "p_cmppd_62_12",
        "66410fc2d001",
        (2, 0x3fefffffffffffff7fe0000000000000),
        (8, 0x7fe00000000000003ff8000000000000),
        0x1fbf,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1fbf, 0x0),
    ),
    g(
        "p_cmppd_7_55",
        "66440fc2d101",
        (10, 0x3fefffffffffffff3fe0000000000000),
        (1, 0x7fd00000000000000000000000000000),
        0x0,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x0, 0x0),
    ),
    g(
        "p_cmppd_64_16",
        "66440fc2f207",
        (14, 0x7ff40000000000003fe0000000000000),
        (2, 0xbca6a08720b4200b0008000000000000),
        0x5faa,
        0x0,
        ok(0xffffffffffffffff, 0x5fab, 0x0),
    ),
    g(
        "p_cmppd_37_7",
        "660fc2c807",
        (1, 0x37ff0000000000001),
        (0, 0xbca84b17d45f6e963cb0000000000000),
        0x1dc0,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1dc1, 0x0),
    ),
    g(
        "p_cmppd_50_7",
        "66440fc2c406",
        (8, 0x4340000000000000000a147f51466378),
        (4, 0x3fe0000000000000802d028d3a10228b),
        0xfb80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0xfb82, 0x0),
    ),
    g(
        "p_cmppd_26_9",
        "66450fc2ca06",
        (9, 0x3fd0000000000000ffd223402ff337e3),
        (10, 0x7fdfffffffffffff0000000000000001),
        0x7f00,
        0x0,
        ok(0x0, 0x7f02, 0x0),
    ),
    g(
        "p_cmppd_66_18",
        "660fc2c004",
        (0, 0xb57385cb0597c2defff8123456789abc),
        (0, 0xb57385cb0597c2defff8123456789abc),
        0x1e80,
        0x0,
        ok(0xffffffffffffffff, 0x1e80, 0x0),
    ),
    g(
        "p_cmppd_10_41",
        "660fc2ca04",
        (1, 0xc0000000000000007ff8000000000001),
        (2, 0xbcb292ee0a9708264014000000000000),
        0x1d80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x1d80, 0x0),
    ),
    g(
        "rb_cmppd_157",
        "660fc2fc05",
        (7, 0xc0008593892346fd3ca3417bfe1973f0),
        (4, 0xbcbcac6b2f38b973c0090bf65e8197c5),
        0x3b80,
        0x0,
        ok(0xffffffffffffffff, 0x3b80, 0x0),
    ),
    g(
        "p_cmppd_59_7",
        "66440fc2c205",
        (8, 0x20000000000000bca4ee68fc4e361a),
        (2, 0xffefffffffffffff4008000000000000),
        0x8000,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x8000, 0x0),
    ),
    g(
        "p_cmppd_70_42",
        "66450fc2c605",
        (8, 0x3ca0000000000000ffd518f8a3cd6158),
        (14, 0x50c5faab9ca000063b0b7e7e0b38),
        0xf80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0xf82, 0x0),
    ),
    g(
        "p_cmppd_3_47",
        "66410fc2ce05",
        (1, 0x5f553ad4d66b7c00c2ddc93ddfd93),
        (14, 0x80100000000000008005dabf8103231d),
        0x7f80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x7f82, 0x0),
    ),
    g(
        "p_cmppd_71_11",
        "660fc2fe03",
        (7, 0x10000000000000800fffffffffffff),
        (6, 0x7fd9bcf7d68a8fb10000000000000001),
        0x1f00,
        0x0,
        ok(0x0, 0x1f02, 0x0),
    ),
    g(
        "p_cmppd_2_13",
        "66450fc2ce03",
        (9, 0x7fdfffffffffffffffec18aa6da53f8d),
        (14, 0x3ca00000000000000000000000000003),
        0x5f80,
        0x0,
        ok(0x0, 0x5f82, 0x0),
    ),
    g(
        "p_cmppd_31_37",
        "66440fc2e306",
        (12, 0x3ff80000000000008000000000000001),
        (3, 0x80000000000000017ff77198d9787296),
        0x6f80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x6f83, 0x0),
    ),
    g(
        "p_cmppd_38_47",
        "66440fc2c906",
        (9, 0x8000000000000001fff0000000000001),
        (1, 0x3ff8331e3df0ee6b7ff0000000000000),
        0x9b80,
        0x0,
        ok(0xffffffffffffffff, 0x9b83, 0x0),
    ),
    g(
        "rb_cmppd_57",
        "66450fc2ce00",
        (9, 0xfff56a3a1612ecb37ff01d59350c5b94),
        (14, 0x3ff0626174f950bc0018050eb829923b),
        0x7dc0,
        0x0,
        ok(0x0, 0x7dc1, 0x0),
    ),
    g(
        "p_cmppd_68_51",
        "660fc2c400",
        (0, 0x3ca028c381f60fadfff8123456789abc),
        (4, 0xfff00000000000010029d45bfb2372d9),
        0x1b80,
        0x0,
        ok(0x0, 0x1b81, 0x0),
    ),
    g(
        "p_cmppd_42_59",
        "66450fc2c602",
        (8, 0x80000000000000017ff4000000000000),
        (14, 0x80208ab4ed4522b13ff0000000000000),
        0x8f80,
        0x0,
        ok(0x0, 0x8f83, 0x0),
    ),
    g(
        "p_cmppd_68_36",
        "660fc2f302",
        (6, 0x3fefffffffffffff4340000000000000),
        (3, 0x80000000000007ff4000000000000),
        0x1b80,
        0x0,
        ok(0x0, 0x1b83, 0x0),
    ),
    g(
        "p_cmppd_67_37",
        "66450fc2f207",
        (14, 0x3fff7ffffffffffff),
        (10, 0xffd5c37b84dda8013ff0000000000000),
        0x1d80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1d83, 0x0),
    ),
    g(
        "p_cmppd_1_3",
        "660fc2cf07",
        (1, 0x8010000000000000802f2393c118c3a8),
        (7, 0x80000000000000017ff4000000000000),
        0x3f80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x3f83, 0x0),
    ),
    g(
        "rb_cmppd_235",
        "660fc2d301",
        (2, 0x3cb852164d1f260abff2fc109cfa307d),
        (3, 0x80021fce8b2c427e3ff5ee9fffbae6e6),
        0x1180,
        0x0,
        ok(0xffffffffffffffff, 0x1182, 0x0),
    ),
    g(
        "p_cmppd_40_1",
        "66440fc2c601",
        (8, 0x7fefffffffffffffbca828e9c9829eee),
        (6, 0x43300000000000000000000000000001),
        0x9780,
        0x0,
        ok(0xffffffffffffffff, 0x9782, 0x0),
    ),
    g(
        "p_cmppd_40_44",
        "66410fc2ea03",
        (5, 0x7ff0000000000001bcaa667a9d4ce0da),
        (10, 0x100000000000018000a0bfee385975),
        0x9780,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x9783, 0x0),
    ),
    g(
        "p_cmppd_67_3",
        "66410fc2f903",
        (7, 0x80000000000000000000000000003),
        (9, 0x7ff1bb2bd4d6360d80062a50458cd64d),
        0x1d80,
        0x0,
        ok(0xffffffffffffffff0000000000000000, 0x1d83, 0x0),
    ),
    g(
        "p_cmppd_49_2",
        "66410fc2e803",
        (5, 0xbcbace5622701b19fff0000000000001),
        (8, 0x80000000000007fea67ce287ce28b),
        0x7dc0,
        0x0,
        ok(0xffffffffffffffff, 0x7dc1, 0x0),
    ),
    g(
        "p_cmppd_6_6",
        "66450fc2fa03",
        (15, 0x800fffffffffffff7ff7ffffffffffff),
        (10, 0xbcbabbefc9869746001fffffffffffff),
        0xffc0,
        0x0,
        ok(0xffffffffffffffff, 0xffc1, 0x0),
    ),
    g(
        "rb_cmppd_274",
        "66410fc2f006",
        (6, 0x3cbc14c96fd658213cb9e02d545716c9),
        (8, 0x7fee9f6d90868bff400cfbeb6413449f),
        0x7f80,
        0x0,
        ok(0x0, 0x7f80, 0x0),
    ),
    g(
        "p_cmppd_22_1",
        "66410fc2c406",
        (0, 0x3ff0000000000001433fffffffffffff),
        (12, 0x1fffffffffffffbcbb72b5cf7097f6),
        0x5d80,
        0x0,
        ok(0xffffffffffffffffffffffffffffffff, 0x5d80, 0x0),
    ),
    g(
        "p_cmppd_19_46",
        "66450fc2d800",
        (11, 0x38067b6d0563c7ff4000000000000),
        (8, 0x40089130f7a75ad97fdfffffffffffff),
        0x2f80,
        0x0,
        ok(0x0, 0x2f83, 0x0),
    ),
    g(
        "p_cmppd_3_2",
        "66440fc2d200",
        (10, 0x7febf0c48ef43a0700005c8d19581a21),
        (2, 0xfff00000000000017fed47f417450da9),
        0x7f80,
        0x0,
        ok(0x0, 0x7f83, 0x0),
    ),
    g(
        "p_cmppd_52_24",
        "66440fc2e901",
        (13, 0x800ffffffffffffffff7ffffffffffff),
        (1, 0xbff00000000000008000000000000000),
        0xf780,
        0x0,
        ok(0x0, 0xf783, 0x0),
    ),
    g(
        "p_cmppd_13_46",
        "660fc2f901",
        (7, 0x7ffe05a6969d5438c000000000000000),
        (1, 0x10008000000000000),
        0xf80,
        0x0,
        ok(0xffffffffffffffff, 0xf83, 0x0),
    ),
    g(
        "p_cmppd_59_12",
        "66450fc2ff02",
        (15, 0xfffffffffffff3cb0000000000000),
        (15, 0xfffffffffffff3cb0000000000000),
        0x8000,
        0x0,
        fault(0x8002),
    ),
    g(
        "p_cmppd_58_24",
        "66410fc2cb02",
        (1, 0x8024dbda4a60414c3fefffffffffffff),
        (11, 0x40140000000000008000000000000001),
        0x6000,
        0x0,
        fault(0x6002),
    ),
    g(
        "p_cmppd_57_58",
        "66440fc2de01",
        (11, 0x7ff00000000000014330000000000000),
        (6, 0x3ff00000000000017fdfffffffffffff),
        0x4000,
        0x0,
        fault(0x4001),
    ),
    g(
        "p_cmppd_8_21",
        "66440fc2e901",
        (13, 0x7ff80000000000004014000000000000),
        (1, 0x802721e2376eb7613ff0000000000001),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_cmppd_45_54",
        "66410fc2f902",
        (7, 0xc326464302425df87fe0000000000000),
        (9, 0x4324c134bda525077ff263cb654a4eae),
        0x7f40,
        0x0,
        fault(0x7f41),
    ),
    g(
        "p_cmppd_7_18",
        "66450fc2db02",
        (11, 0x7ff8000000000001c053ed5a634a13f3),
        (11, 0x7ff8000000000001c053ed5a634a13f3),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "p_cmppd_59_6",
        "66440fc2c304",
        (8, 0x7ff12874ff5b489b800dcc98609aaa51),
        (3, 0x80100000000000003ca0000000000000),
        0x8000,
        0x0,
        fault(0x8003),
    ),
    g(
        "p_cmppd_8_33",
        "660fc2ea04",
        (5, 0x80000000000000017ff4000000000000),
        (2, 0xbcae6f173329947fc000000000000000),
        0x1f00,
        0x0,
        fault(0x1f03),
    ),
    g(
        "p_cmppd_74_57",
        "66410fc2d402",
        (2, 0x7fffffffffffffffc32ab24938266bec),
        (12, 0xfff3cb908967cfc0800e6813eee55df6),
        0x1a00,
        0x0,
        fault(0x1a03),
    ),
    g(
        "p_cmppd_7_25",
        "660fc2c402",
        (0, 0x7ff7ffffffffffff80083a38a5550816),
        (4, 0xc0000000000000003caed4424e2ecae4),
        0x0,
        0x0,
        fault(0x3),
    ),
    g(
        "p_cmppd_66_59",
        "66440fc2f901",
        (15, 0xe4ea420333ff10000000000000000),
        (1, 0x37fefffffffffffff),
        0x1e80,
        0x0,
        fault(0x1e82),
    ),
    g(
        "p_cmppd_21_46",
        "66450fc2d601",
        (10, 0x43400000000000007fd0000000000000),
        (14, 0x7fefffffffffffff0008000000000000),
        0x5e80,
        0x0,
        fault(0x5e82),
    ),
    g(
        "p_cmppd_60_11",
        "66440fc2e805",
        (13, 0x7ff0000000000001c32f02be7655c387),
        (0, 0x3fefffffffffffff000fffffffffffff),
        0x40,
        0x0,
        fault(0x41),
    ),
    g(
        "rb_cmppd_19",
        "66440fc2f105",
        (14, 0x7ff4239a264d2dd4802356a19cd6b691),
        (1, 0x7fdb9cb36a65570f3cb01d13596945de),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_cmppd_72_17",
        "66440fc2d706",
        (10, 0xfff8123456789abcfff0000000000000),
        (7, 0x3cb00000000000008000000000000000),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_cmppd_7_19",
        "66440fc2f306",
        (14, 0xfff1a4865c7e482a0000000000000001),
        (3, 0xfff00000000000017ff0000000000001),
        0x0,
        0x0,
        fault(0x1),
    ),
    g(
        "p_cmppd_74_58",
        "66410fc2e300",
        (4, 0x7ff00000000000003ff0000000000000),
        (11, 0x3ff8000000000000000fffffffffffff),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "p_cmppd_7_17",
        "660fc2f600",
        (6, 0x7fe39967f0ba53c00000000000000001),
        (6, 0x7fe39967f0ba53c00000000000000001),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "p_cmppd_57_3",
        "660fc2e805",
        (5, 0x800fffffffffffffbcbd7159e2ce8d28),
        (0, 0x7fdfffffffffffffbff9a895d2bfa42c),
        0x4000,
        0x0,
        fault(0x4002),
    ),
    g(
        "p_cmppd_7_48",
        "660fc2c805",
        (1, 0x13ff8000000000000),
        (0, 0x8010000000000000400013e287a5dfd3),
        0x0,
        0x0,
        fault(0x2),
    ),
    g(
        "p_cmppd_46_32",
        "66410fc2ee05",
        (5, 0x2a39e8346fcb22800fffffffffffff),
        (14, 0x3fff0000000000001),
        0xfe80,
        0x0,
        fault(0xfe83),
    ),
    g(
        "p_cmppd_58_13",
        "66450fc2c105",
        (8, 0x9ac9560123cc08000000000000001),
        (9, 0x7fd00000000000007ff8000000000001),
        0x6000,
        0x0,
        fault(0x6003),
    ),
    g(
        "rb_cmppd_39",
        "66440fc2d907",
        (11, 0xc32a4e5707d683f60018d026ebdc453f),
        (1, 0x800d4ad01e78f4f43ff8115d8c0c5e00),
        0xfe80,
        0x0,
        fault(0xfe82),
    ),
    g(
        "p_cmppd_56_38",
        "66450fc2e307",
        (12, 0x1fffffffffffff0000000000000001),
        (11, 0x800fffffffffffff800fffffffffffff),
        0x2000,
        0x0,
        fault(0x2002),
    ),
    g(
        "p_cmppd_72_40",
        "66450fc2d900",
        (11, 0x800fffffffffffff7ff0000000000001),
        (9, 0x8759b22722d997fffffffffffffff),
        0x1700,
        0x0,
        fault(0x1703),
    ),
    g(
        "p_cmppd_20_42",
        "660fc2fb00",
        (7, 0x7ff0000000000001800b226d6737cbba),
        (3, 0x400800000000000080292eebb9123f63),
        0x5f00,
        0x0,
        fault(0x5f03),
    ),
    g(
        "p_cmppd_72_24",
        "66410fc2f107",
        (6, 0x7ff54d81533c0232fff7ffffffffffff),
        (9, 0x7ff00000000000007fffffffffffffff),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_cmppd_14_19",
        "66410fc2c907",
        (1, 0x7ff7ffffffffffff7ff8000000000001),
        (9, 0x3cae148109ae018d0000000000000001),
        0x3f00,
        0x0,
        fault(0x3f01),
    ),
    g(
        "p_cmppd_60_50",
        "66410fc2d003",
        (2, 0xfff8123456789abc3ff8000000000000),
        (8, 0xc0046e8b7287d665fff0000000000001),
        0x40,
        0x0,
        fault(0x41),
    ),
    g(
        "p_cmppd_59_2",
        "660fc2e403",
        (4, 0x7ff7ffffffffffff0010000000000001),
        (4, 0x7ff7ffffffffffff0010000000000001),
        0x8000,
        0x0,
        fault(0x8001),
    ),
    g(
        "p_cmppd_57_50",
        "66410fc2d703",
        (2, 0x7ff80000000000013ff0000000000001),
        (15, 0xbff464663d203f3a80037667bed22009),
        0x4000,
        0x0,
        fault(0x4002),
    ),
    g(
        "p_cmppd_58_41",
        "66450fc2e303",
        (12, 0xfffffffffffff3fefffffffffffff),
        (11, 0x1fff8123456789abc),
        0x6000,
        0x0,
        fault(0x6002),
    ),
    g(
        "p_cmppd_72_53",
        "66410fc2da04",
        (3, 0xc9fca39d4356b15c7ff4000000000000),
        (10, 0x3ff4d4fc8a4a7d5d3fefffffffffffff),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_cmppd_8_39",
        "66440fc2c204",
        (8, 0x3fefffffffffffff800fffffffffffff),
        (2, 0x7fd9c401809e275c7ff4000000000000),
        0x1f00,
        0x0,
        fault(0x1f01),
    ),
    g(
        "p_cmppd_74_53",
        "66410fc2fb04",
        (7, 0x8009694e1861e8943fe0000000000000),
        (11, 0x7fdfffffffffffff3ca0000000000000),
        0x1a00,
        0x0,
        fault(0x1a02),
    ),
    g(
        "p_cmppd_34_47",
        "660fc2c404",
        (0, 0x3ff80000000000000010000000000001),
        (4, 0x1fff0000000000000),
        0x9e80,
        0x0,
        fault(0x9e82),
    ),
    g(
        "p_cmppd_59_9",
        "66410fc2ec06",
        (5, 0x8013d77db8ef6d0e801fafef30af7533),
        (12, 0x17fffffffffffffff),
        0x8000,
        0x0,
        fault(0x8003),
    ),
    g(
        "p_cmppd_9_1",
        "66440fc2d706",
        (10, 0x800fffffffffffff800fffffffffffff),
        (7, 0xfff00000000000007ff0000000000001),
        0x1e80,
        0x0,
        fault(0x1e83),
    ),
    g(
        "rb_cmppd_200",
        "660fc2eb00",
        (5, 0x24fd717320b2b47ff488f1c1bd339c),
        (3, 0x3525f56405bb4759bcaedf791df5dd1b),
        0x6326,
        0x0,
        fault(0x6327),
    ),
    g(
        "p_cmppd_14_46",
        "66410fc2c400",
        (0, 0x3fd00000000000003ca2fa3de58f75a1),
        (12, 0x7ff00000000000013ff0000000000001),
        0x3f00,
        0x0,
        fault(0x3f01),
    ),
    g(
        "p_cmppd_59_59",
        "66450fc2cc07",
        (9, 0xbcb404781a1501928000000000000001),
        (12, 0xfff7ffffffffffff4340000000000000),
        0x8000,
        0x0,
        fault(0x8003),
    ),
    g(
        "p_cmppd_56_22",
        "66450fc2f907",
        (15, 0xffffffffffffffff0000000000001),
        (9, 0x800fffffffffffff3ff0277aecb1cef0),
        0x2000,
        0x0,
        fault(0x2003),
    ),
    g(
        "p_cmppd_57_12",
        "66440fc2f403",
        (14, 0x3ffd37928556d8060000000000000001),
        (4, 0x7ff7ffffffffffffbcbe420149b378a1),
        0x4000,
        0x0,
        fault(0x4003),
    ),
    g(
        "p_cmppd_46_51",
        "66440fc2fd03",
        (15, 0x7ff40000000000008000000000000000),
        (5, 0xfff00000000000018000000000000001),
        0xfe80,
        0x0,
        fault(0xfe83),
    ),
    g(
        "rb_cmppd_186",
        "660fc2f906",
        (7, 0x7f1c1c45cd165432d0bde22fa48d7),
        (1, 0x3cb0a3757bf1a4d70026d80e96f20460),
        0x7298,
        0x0,
        fault(0x729a),
    ),
    g(
        "p_cmppd_15_16",
        "66410fc2f306",
        (6, 0x3cb00000000000007ff0000000000000),
        (11, 0x1f8a27c1440a3a0000000000000003),
        0x3e80,
        0x0,
        fault(0x3e82),
    ),
    g(
        "p_cmppd_66_48",
        "660fc2f801",
        (7, 0x3ca52cf64ad4e8038000000000000000),
        (0, 0x8000000000000fff8000000000000),
        0x1e80,
        0x0,
        fault(0x1e83),
    ),
    g(
        "p_cmppd_14_5",
        "66440fc2d601",
        (10, 0x7ff80000000000010010000000000000),
        (6, 0xc000000000000000800a03914d1ded35),
        0x3f00,
        0x0,
        fault(0x3f03),
    ),
    g(
        "rb_cmpps_257",
        "440fc2fe02",
        (15, 0xb607f05f4aee5a97fdfd71c827ea204),
        (6, 0x80f469c800f94d7a802b085fcf8ed7ab),
        0x2855,
        0x0,
        fault(0x2855),
    ),
    g(
        "rb_cmpps_156",
        "450fc2de02",
        (11, 0x7f01c19f74b20a6f1751efa7002efe56),
        (14, 0xff89e2813fb33ce3014266d4f4fac3a1),
        0x8d77,
        0x0,
        fault(0x8d77),
    ),
    g(
        "rb_cmpps_72",
        "450fc2e006",
        (12, 0xe50a1bf5bfa1296d0b6c898acfb46ea8),
        (8, 0x7fe490cff4ce854600ece066000e170e),
        0xac4b,
        0x0,
        fault(0xac4b),
    ),
    g(
        "rb_cmpps_103",
        "440fc2e103",
        (12, 0xf4e85847507b0fe6ffbf37fb0b5c6937),
        (1, 0x3031e26401393d070b5a0af9306acda2),
        0x5c2b,
        0x0,
        fault(0x5c2b),
    ),
    g(
        "rb_cmpps_237",
        "440fc2da03",
        (11, 0xb06e1578802d213600d08a44300f9dae),
        (2, 0xeb65339d7fbc1bcb4f81d4e168d92546),
        0xc421,
        0x0,
        fault(0xc421),
    ),
    g(
        "rb_cmpps_113",
        "0fc2d701",
        (2, 0xff8e04d2bfb1e96c7f6417a40108c87c),
        (7, 0x407bf5d710e8c7d6810fe8e4805da20b),
        0x910b,
        0x0,
        fault(0x910b),
    ),
    g(
        "rb_cmpps_183",
        "0fc2c807",
        (1, 0x7f30a90045fbecbfe581ca7fc7af9e),
        (0, 0x4fb16d2a007374297f99c9e90b60154c),
        0x2961,
        0x0,
        fault(0x2961),
    ),
    g(
        "rb_cmpps_297",
        "410fc2c307",
        (0, 0x8b3f84fcaf744134306a815140021c9c),
        (11, 0xff12fac700687b2e8b2fb14ff4d0c42c),
        0x3212,
        0x0,
        fault(0x3212),
    ),
    g(
        "rb_cmpps_185",
        "410fc2fc05",
        (7, 0x7f34efe6801e64bcc06cee80cfd1a31e),
        (12, 0x809370ef801b91167f6dbc15303c3342),
        0xc416,
        0x0,
        fault(0xc416),
    ),
    g(
        "rb_cmpps_285",
        "450fc2c504",
        (8, 0xed062f80052f4300d1ad80134f2211),
        (13, 0x7fc3ff15f4a8ca7f81797f56f4d4c5a0),
        0xd627,
        0x0,
        fault(0xd627),
    ),
    g(
        "rb_cmppd_8",
        "66440fc2f702",
        (14, 0xc32be31330b18f1cbcbd997407bb36e8),
        (7, 0xd36cbbedb87cb002756690d971f7b),
        0x261b,
        0x0,
        fault(0x261b),
    ),
    g(
        "rb_cmppd_22",
        "66410fc2e907",
        (5, 0x800752907d3662887fed445a5187437d),
        (9, 0xbfa898b8fc40b682d8cdf784c9843504),
        0xe627,
        0x0,
        fault(0xe627),
    ),
    g(
        "rb_cmppd_63",
        "66440fc2d607",
        (10, 0xc0bcf99074c4a000ded9158f5d212),
        (6, 0xbffe30f00347cf3094a7b3d3d0db08ed),
        0xf836,
        0x0,
        fault(0xf836),
    ),
    g(
        "rb_cmppd_50",
        "66450fc2c504",
        (8, 0xffdcb208fae59c6affe14c201d11eefe),
        (13, 0x80181914a41466018009282a39683e76),
        0xb0a6,
        0x0,
        fault(0xb0a6),
    ),
    g(
        "rb_cmppd_71",
        "66450fc2db04",
        (11, 0x800994bbe4269a46bca65f17adb7eb44),
        (11, 0x800994bbe4269a46bca65f17adb7eb44),
        0x5016,
        0x0,
        fault(0x5016),
    ),
    g(
        "rb_cmppd_66",
        "660fc2dc01",
        (3, 0x800ddf823df8f4077fd1e9ae2efe397b),
        (4, 0xbcbdc453690ff92f7fed63f7f32d41c9),
        0x1826,
        0x0,
        fault(0x1826),
    ),
    g(
        "rb_cmppd_101",
        "660fc2eb03",
        (5, 0xcdb275cec668e7fe970daeaf01ad6),
        (3, 0x19d74253802be7c329e8bf7221df8e),
        0x448f,
        0x0,
        fault(0x448f),
    ),
    g(
        "rb_cmppd_247",
        "66450fc2d703",
        (10, 0x7ff443345d0019207feaf110ae501031),
        (15, 0xbfff8b345dc9b62a3cbb58a23faf87bf),
        0xed7f,
        0x0,
        fault(0xed7f),
    ),
    g(
        "rb_cmppd_240",
        "66450fc2cb05",
        (9, 0x7ffd03f0e98fa6f4ffe5b524c88491b2),
        (11, 0xbe26f3f18fe42eaf7fd248652c4c7e2a),
        0x453b,
        0x0,
        fault(0x453b),
    ),
    g(
        "rb_cmppd_277",
        "66450fc2c605",
        (8, 0x7fdecce34e8f730d0022c2e493724bf8),
        (14, 0xbcb30c0b7fc0a7e27ff71fde0d74bf1e),
        0x131b,
        0x0,
        fault(0x131b),
    ),
    g(
        "rb_cmppd_256",
        "66410fc2ec00",
        (5, 0x43254fe83468d932bcba6b23ae1f070f),
        (12, 0xffdbacbf9a0409418008c74301462cb3),
        0x481e,
        0x0,
        fault(0x481e),
    ),
    g(
        "rb_cmppd_279",
        "66410fc2d600",
        (2, 0xfec0e7876384700037fec045cce7f),
        (14, 0x2d405172db96fe8003c7ee3f8eab42),
        0x689b,
        0x0,
        fault(0x689b),
    ),
    g(
        "rb_cmpss_8",
        "f3410fc2c300",
        (0, 0x8000fec5b0537b348091f6d5ff79db8d),
        (11, 0xc0206d5e00ef447d812711caffa63b84),
        0x5123,
        0x0,
        fault(0x5123),
    ),
    g(
        "rb_cmpss_64",
        "f3410fc2db07",
        (3, 0x8065a802801790458110c30ebfabb75f),
        (11, 0x7fe673797fb2e5af80161eca004af14e),
        0xa3f,
        0x0,
        fault(0xa3f),
    ),
    g(
        "rb_cmpss_125",
        "f3450fc2d203",
        (10, 0x813f7f08004b4f2a7f11b792806d6cf6),
        (10, 0x813f7f08004b4f2a7f11b792806d6cf6),
        0x9487,
        0x0,
        fault(0x9487),
    ),
    g(
        "rb_cmpss_251",
        "f3450fc2cc03",
        (9, 0x80a1e178f49ad61f815047c37faa9e4e),
        (12, 0x8056f5b0ff78b118306526df801a19d5),
        0x9247,
        0x0,
        fault(0x9247),
    ),
    g(
        "rb_cmpss_183",
        "f3410fc2c502",
        (0, 0x80a9343d804d0a590b5500ea803a87ab),
        (13, 0xffad8121016f1d52ffb7d31837dabb61),
        0xbe9f,
        0x0,
        fault(0xbe9f),
    ),
    g(
        "rb_cmpss_268",
        "f3440fc2c002",
        (8, 0xf488b36d748f16940b3726c8ff9bfe1a),
        (0, 0x80e0b2ffc040a99a810582e1749dc638),
        0x8851,
        0x0,
        fault(0x8851),
    ),
    g(
        "rb_cmpss_195",
        "f3440fc2eb06",
        (13, 0x400b53bf4035d3f57f37a6b57fbfe7ea),
        (3, 0xb07c4d2d7f7920c900785f9b016d4b02),
        0xf107,
        0x0,
        fault(0xf107),
    ),
    g(
        "rb_cmpsd_11",
        "f20fc2de02",
        (3, 0xffe83ee655b1c5fb8009370486ecb57f),
        (6, 0xbffd49b6847490ddc0005ada7099ef71),
        0x5603,
        0x0,
        fault(0x5603),
    ),
    g(
        "rb_cmpsd_15",
        "f2440fc2e600",
        (12, 0x164b7b4fb316aebca853292e0923c4),
        (6, 0x7fdae948ebfeb8020002969d52aac0e9),
        0x12bf,
        0x0,
        fault(0x12bf),
    ),
    g(
        "rb_cmpsd_26",
        "f2440fc2d005",
        (10, 0x80225e689c302630bffc93b15416fcb7),
        (0, 0x1c323daeaad9c000b53a48c6fed9c),
        0x3a37,
        0x0,
        fault(0x3a37),
    ),
    g(
        "rb_cmpsd_82",
        "f2440fc2e707",
        (12, 0x1dfb20755152a83cb6e231fd284843),
        (7, 0x1f77b8ecf8e89b7ff52e618fa8a5d2),
        0x1d2b,
        0x0,
        fault(0x1d2b),
    ),
    g(
        "rb_cmpsd_84",
        "f2440fc2cc07",
        (9, 0x54f99a2b85c1d62100087d88fed0f973),
        (4, 0xbcb57738c264151e3cb6dbeddb27f46d),
        0x8032,
        0x0,
        fault(0x8032),
    ),
    g(
        "rb_cmpsd_111",
        "f2410fc2d501",
        (2, 0x7fff76411afc33eb7c8d0d1902d23814),
        (13, 0xbff4692a96fe54e87ff5ad68471c0153),
        0x5065,
        0x0,
        fault(0x5065),
    ),
    g(
        "rb_cmpsd_282",
        "f20fc2e801",
        (5, 0xfffbff68f896c56200080a5712508272),
        (0, 0xfff2f0ba68b8fc967fd949489e888f10),
        0xba8e,
        0x0,
        fault(0xba8e),
    ),
    g(
        "p_cvtdq2psps_69_6",
        "450f5be5",
        (12, 0x80492f33ffffffffc00000007f800000),
        (13, 0x80000000ff87be94010000034827744f),
        0x1780,
        0x0,
        ok(0xcf000000caf082d84b8000024e904ee9, 0x17a0, 0x0),
    ),
    g(
        "p_cvtdq2psps_51_52",
        "410f5bf6",
        (6, 0xbf8b34537fffffff74ec35897fffff80),
        (14, 0x403d1a6e0100000112345678),
        0x7bc0,
        0x0,
        ok(0x4e807a344b8000004d91a2b3, 0x7be0, 0x0),
    ),
    g(
        "p_cvtdq2psps_64_22",
        "410f5bd0",
        (2, 0x811f391b7fd9a1a07ffffffe7ffffffe),
        (8, 0x801ffd2700ffffff8000000080000001),
        0x5faa,
        0x0,
        ok(0xceffc0054b7fffffcf000000ceffffff, 0x5faa, 0x0),
    ),
    g(
        "p_cvtdq2psps_0_44",
        "440f5bd6",
        (10, 0x112345678feffffffff000001),
        (6, 0x3f800000c00000007f8000000000ffff),
        0x1f80,
        0x0,
        ok(0x4e7e0000ce8000004eff0000477fff00, 0x1f80, 0x0),
    ),
    g(
        "rb_cvtdq2psps_39",
        "410f5bd4",
        (2, 0x123456780000007f7fffffc012345678),
        (12, 0x1000001cfe718ec010000030b2a0296),
        0xed4f,
        0x0,
        fault(0xed6f),
    ),
    g(
        "p_cvtdq2psps_7_12",
        "410f5bfc",
        (7, 0x7f800000ff0000017f80000000000001),
        (12, 0xf48a486cff00000100d0fb373046a9cb),
        0x0,
        0x0,
        fault(0x20),
    ),
    g(
        "rb_cvtdq2psps_107",
        "410f5bf2",
        (6, 0x15c7853feffffffbfa994ed7fffffff),
        (10, 0x1234567800800001804b5540cfe0465e),
        0x6b2e,
        0x0,
        fault(0x6b2e),
    ),
    g(
        "p_cvtdq2psps_65_15",
        "450f5bcd",
        (9, 0x8548d20000000100ffffff7f4a62ef),
        (13, 0x80000001ffffff8101000000ffffffff),
        0xfbf,
        0x0,
        fault(0xfbf),
    ),
    g(
        "p_cvtps2dqps_50_18",
        "660f5be9",
        (5, 0x3fc000008b1764ccbf000000cfc61e9a),
        (1, 0x3f8000004040000000400000007fffff),
        0xfb80,
        0x0,
        ok(0x1000000030000000000000000, 0xfba0, 0x0),
    ),
    g(
        "p_cvtps2dqps_8_14",
        "660f5bfc",
        (7, 0x7e800000ff800001bf7fffff71808c34),
        (4, 0x4040000000400000c020000001000000),
        0x1f00,
        0x0,
        ok(0x300000000fffffffe00000000, 0x1f20, 0x0),
    ),
    g(
        "p_cvtps2dqps_68_17",
        "66410f5be3",
        (4, 0x3f8000010046a79e8000000080588cc3),
        (11, 0x3f000001cf0000014b000000bfc00000),
        0x1b80,
        0x0,
        ok(0x18000000000800000fffffffe, 0x1ba1, 0x0),
    ),
    g(
        "p_cvtps2dqps_53_49",
        "66440f5bf6",
        (14, 0xae42d19a006e49abbfe9790500800001),
        (6, 0xff800001c04c5e164effffc0ffc12345),
        0x77c0,
        0x0,
        ok(0x80000000fffffffd7fffe00080000000, 0x77e1, 0x0),
    ),
    g(
        "rb_cvtps2dqps_159",
        "66410f5bef",
        (5, 0xc02000007ff0b230ff97e47c3f800000),
        (15, 0x7e800000000af7c1800000014f000001),
        0x1ec0,
        0x0,
        ok(0x80000000000000000000000080000000, 0x1ec1, 0x0),
    ),
    g(
        "p_cvtps2dqps_4_56",
        "66410f5bd6",
        (2, 0xffbfffffbf9d95dc7fc0000180800000),
        (14, 0x400000004effffbf3f8000007fc00001),
        0x1fc0,
        0x0,
        ok(0x27fffdf800000000180000000, 0x1fc1, 0x0),
    ),
    g(
        "p_cvtps2dqps_72_31",
        "66450f5bf1",
        (14, 0x7e80000080b23d6d4eff0000c0000000),
        (9, 0x40400000beffffff406c3843ffc00000),
        0x1700,
        0x0,
        fault(0x1701),
    ),
    g(
        "p_cvtps2dqps_44_53",
        "66440f5bd6",
        (10, 0xf731e4c60b1916c54effffbfbfc00000),
        (6, 0x7f7fffff74dfb2b79c258b9cbf000000),
        0xff00,
        0x0,
        fault(0xff01),
    ),
    g(
        "rb_cvtps2dqps_256",
        "66440f5bde",
        (11, 0x7f2ca5fc3f8000007e800000006ea703),
        (6, 0x14eca84effffff4b80000000000003),
        0x411a,
        0x0,
        fault(0x413a),
    ),
    g(
        "p_cvtps2dqps_19_38",
        "66410f5bc7",
        (0, 0xf4c44adb3f0000017f8000014effffc0),
        (15, 0xc000000080000001bfc000004b7fffff),
        0x2f80,
        0x0,
        fault(0x2fa0),
    ),
    g(
        "p_cvtps2dqps_70_9",
        "66410f5bc7",
        (0, 0x4f0000014f0000007f0000003f000001),
        (15, 0xcf7fffff807fffff7f800001015163df),
        0xf80,
        0x0,
        fault(0xfa1),
    ),
    g(
        "p_cvtps2dqps_55_10",
        "66450f5bc7",
        (8, 0xbf8000003f0000010070005f012a85f6),
        (15, 0xc0200000f49857cc0080000034000000),
        0x6fc0,
        0x0,
        fault(0x6fe1),
    ),
    g(
        "rb_cvtps2dqps_40",
        "66410f5bfc",
        (7, 0x3f00000074f53ecde8aaea24802bbf5e),
        (12, 0x4fc3d479ffc123454b800000ff912891),
        0xbfc9,
        0x0,
        ok(0x80000000800000000100000080000000, 0xbfc9, 0x0),
    ),
    g(
        "p_cvtps2dqps_45_40",
        "660f5bc4",
        (0, 0x3f8000014f0000004effffff3e800000),
        (4, 0x4effffbf3f80000000400000007fffff),
        0x7f40,
        0x0,
        ok(0x7fffdf80000000010000000000000000, 0x7f40, 0x0),
    ),
    g(
        "rb_cvtps2dqps_203",
        "66440f5bc9",
        (9, 0xff8000007fffffffff1955067ff7f466),
        (1, 0x805b473d3effffffbffe25907f800000),
        0x9217,
        0x0,
        fault(0x9217),
    ),
    g(
        "p_cvtps2dqps_65_3",
        "660f5bd1",
        (2, 0x3fc000007fffffff00301e5dcf000001),
        (1, 0x4075aae73e800000bf80000034000000),
        0xfbf,
        0x0,
        fault(0xfbf),
    ),
    g(
        "rb_cvttps2dqps_298",
        "f3450f5bee",
        (13, 0x7f033ab200ffffffffc12345c934b795),
        (14, 0xffe6d3a14f00000074fb4f5e3f000000),
        0xf780,
        0x0,
        ok(0x80000000800000008000000000000000, 0xf7a1, 0x0),
    ),
    g(
        "p_cvttps2dqps_46_47",
        "f3440f5be8",
        (13, 0x3effffffbeffffffbfc000007fbfffff),
        (0, 0xcf000001000000030b1567c980000001),
        0xfe80,
        0x0,
        ok(0x80000000000000000000000000000000, 0xfea1, 0x0),
    ),
    g(
        "rb_cvttps2dqps_92",
        "f3450f5bec",
        (13, 0xffc00000ffb8d6464effffbfffb2c876),
        (12, 0x35e5b8cf0000013f8000017f9a2fd7),
        0x1f95,
        0x0,
        ok(0x800000000000000180000000, 0x1fb5, 0x0),
    ),
    g(
        "p_cvttps2dqps_41_41",
        "f3410f5bf6",
        (6, 0x4b000000bfc00000000782f18b585ce3),
        (14, 0x80000100ffffffbffbadfc002c02de),
        0x17c0,
        0x0,
        ok(0xffffffff00000000, 0x17e0, 0x0),
    ),
    g(
        "p_cvttps2dqps_61_12",
        "f30f5beb",
        (5, 0x34000000ff7fffffbf00000100aee903),
        (3, 0x14f0000017f3b861b7e800000),
        0x80c0,
        0x0,
        ok(0x800000008000000080000000, 0x80c1, 0x0),
    ),
    g(
        "p_cvttps2dqps_70_39",
        "f3410f5bdb",
        (3, 0x7fffff7e8000008073c35dff52b101),
        (11, 0xff8000017f0000007fffffff5e6885c0),
        0xf80,
        0x0,
        ok(0x80000000800000008000000080000000, 0xf81, 0x0),
    ),
    g(
        "p_cvttps2dqps_62_53",
        "f3440f5bde",
        (11, 0x40000000fca8b5e04b8000002438e815),
        (6, 0x10168357fbfffff802a22e301000000),
        0x1fbf,
        0x0,
        ok(0x800000000000000000000000, 0x1fbf, 0x0),
    ),
    g(
        "p_cvttps2dqps_62_16",
        "f3450f5bc0",
        (8, 0x8000007fa00000cfe5f9c34ff764ac),
        (8, 0x8000007fa00000cfe5f9c34ff764ac),
        0x1fbf,
        0x0,
        ok(0x800000008000000080000000, 0x1fbf, 0x0),
    ),
    g(
        "p_cvttps2dqps_74_48",
        "f3450f5bd3",
        (10, 0xffc00000800000017fa000003fc89d90),
        (11, 0x338000003f7fffff004000007fc00001),
        0x1a00,
        0x0,
        fault(0x1a01),
    ),
    g(
        "p_cvttps2dqps_56_42",
        "f30f5bed",
        (5, 0xffffff340000007fa000008b32e98a),
        (5, 0xffffff340000007fa000008b32e98a),
        0x2000,
        0x0,
        fault(0x2001),
    ),
    g(
        "p_cvttps2dqps_73_21",
        "f30f5bd4",
        (2, 0x406cae7a7fb8ddb98b78b05f30262f2a),
        (4, 0x817579c04b7fffff3f0000010011b52b),
        0x380,
        0x0,
        fault(0x3a0),
    ),
    g(
        "p_cvttps2dqps_54_56",
        "f3450f5bca",
        (9, 0x4b7fffff4effffc0405d302400ffffff),
        (10, 0x3fc000003f000001ceffffffbf000001),
        0xef80,
        0x0,
        fault(0xefa0),
    ),
    g(
        "rb_cvttps2dqps_193",
        "f3440f5bea",
        (13, 0x3f8000008b45006f813e16797f5191ee),
        (2, 0xb57c59401761d3c0f48ec37f48eacad),
        0x2f80,
        0x0,
        fault(0x2fa1),
    ),
    g(
        "p_cvttps2dqps_43_15",
        "f3450f5bd1",
        (10, 0xc039cb6fcf7fffff7fa000003f800000),
        (9, 0x1000000402000007f7dbab33f800001),
        0xfc0,
        0x0,
        fault(0xfe1),
    ),
    g(
        "rb_cvttps2dqps_238",
        "f3450f5bf3",
        (14, 0xf23945e93effffff4effffc07fa8ace9),
        (11, 0x1028199fd5590a63400000040506cbe),
        0x4839,
        0x0,
        fault(0x4839),
    ),
    g(
        "p_cvttps2dqps_65_37",
        "f3450f5bd4",
        (10, 0x7fbfffffbf7fffff80000001beffffff),
        (12, 0x4b8000004effffff000000003e800000),
        0xfbf,
        0x0,
        fault(0xfbf),
    ),
];
