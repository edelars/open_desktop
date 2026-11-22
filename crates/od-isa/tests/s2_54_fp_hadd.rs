//! S2-54 (FP part): ADDSUBPS/ADDSUBPD, HADDPS/HADDPD and HSUBPS/HSUBPD on XMM registers and 128-bit memory,
//! binary32 (`F2` prefix, "PS") and binary64 (`66` prefix, "PD"), run through the general SSE floating-point
//! core of S2-37 (`sse_fp.rs`): the same MXCSR flags, masks, DAZ/FTZ, rounding control and unmasked-exception
//! policy as ADDPS/SUBPS and ADDPD/SUBPD.
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A ADDSUBPD pp. 3-26..3-27, ADDSUBPS pp. 3-28..3-29, HADDPD pp. 3-433..3-435, HADDPS pp. 3-436..3-438,
//! HSUBPD pp. 3-442..3-444, HSUBPS pp. 3-445..3-447 (opcodes `66 0F D0`, `F2 0F D0`, `66 0F 7C`, `F2 0F 7C`,
//! `66 0F 7D`, `F2 0F 7D`; the "128-bit Legacy SSE Version" operation); exception class Vol. 2A Table 2-19
//! (Type 2: a 128-bit memory operand must be 16-byte aligned, `#GP(0)` otherwise; numeric exceptions
//! overflow, underflow, invalid, precision, denormal); Vol. 1 sections 4.8.3.5, 4.9.1, 4.9.2, 10.2.3, 11.5.
//!
//! Operation (legacy SSE version, DEST = SRC1 = the first operand, SRC = the second):
//! - ADDSUBPS: `DEST[31:0] = DEST[31:0] - SRC[31:0]`, `DEST[63:32] = DEST[63:32] + SRC[63:32]`, `[95:64]`
//!   subtracts, `[127:96]` adds; ADDSUBPD: `[63:0]` subtracts, `[127:64]` adds.
//! - HADDPS: `DEST[31:0] = DEST[63:32] + DEST[31:0]`, `DEST[63:32] = DEST[127:96] + DEST[95:64]`,
//!   `DEST[95:64] = SRC[63:32] + SRC[31:0]`, `DEST[127:96] = SRC[127:96] + SRC[95:64]`; HADDPD:
//!   `DEST[63:0] = DEST[127:64] + DEST[63:0]`, `DEST[127:64] = SRC[127:64] + SRC[63:0]`.
//! - HSUBPS/HSUBPD: the same pairs with `low - high` (`SRC1[31:0] - SRC1[63:32]`).
//!
//! Where the SDM text does not fix an observable detail, the expected value is the one the physical x86 oracle
//! gave (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; the program loads MXCSR with LDMXCSR and runs the
//! instruction directly): which NaN of a horizontal pair wins (the LOW element, in the order the SDM
//! pseudocode writes the sum the other way round: the hardware treats the low element as the first source
//! operand), the MXCSR flags a faulting instruction leaves, the result of every MXCSR mask/RC/DAZ/FTZ
//! combination. The GOLDEN table below is hardware output, not model output (work/stage8/s2_54_fp_oracle/:
//! gen_fp.py, pick_golden.py, oracle.c, compare.py; the whole run was 103098 cases, see the stage 8 report).
//!
//! Not asserted here: the CPUID gate (S2-57; the SSE3 bit is not declared), the encodings the hardware
//! tolerates (no/`F3` prefix is `#UD` on hardware and Unsupported in the model, a repeated or conflicting
//! mandatory prefix and FS/GS stay Unsupported), the JIT (refused, see od-jit), CPL3 (refused, see
//! cpl3_profile_set).
use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{
    self, DecodeError, Operation, Sse3Source, SseFpArithKind as Kind, SseFpShape as Shape,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const DATA: u64 = 0x8000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
/// All six exceptions masked, round to nearest, no DAZ, no FTZ, no flag set.
const MXCSR: u32 = 0x1f80;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;
const KINDS: [Kind; 3] = [Kind::AddSub, Kind::HAdd, Kind::HSub];
const SHAPES: [Shape; 2] = [Shape::Packed32, Shape::Packed64];

fn opcode(kind: Kind) -> u8 {
    match kind {
        Kind::AddSub => 0xd0,
        Kind::HAdd => 0x7c,
        Kind::HSub => 0x7d,
        _ => unreachable!("S2-37 kinds are not in this file"),
    }
}
/// The mandatory prefix: `F2` selects binary32, `66` binary64.
fn prefix(shape: Shape) -> u8 {
    match shape {
        Shape::Packed32 => 0xf2,
        Shape::Packed64 => 0x66,
        _ => unreachable!("the SSE3 forms are packed only"),
    }
}
fn name(kind: Kind, shape: Shape) -> String {
    let op = match kind {
        Kind::AddSub => "addsub",
        Kind::HAdd => "hadd",
        Kind::HSub => "hsub",
        _ => unreachable!(),
    };
    let suffix = if shape == Shape::Packed32 { "ps" } else { "pd" };
    format!("{op}{suffix}")
}
/// `[F2|66] [REX] 0F op /r` with a register source.
fn reg_code(kind: Kind, shape: Shape, dst: u8, src: u8, rex_w: bool) -> Vec<u8> {
    let mut code = vec![prefix(shape)];
    let rex = 0x40 | u8::from(rex_w) << 3 | u8::from(dst >= 8) << 2 | u8::from(src >= 8);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(kind), 0xc0 | (dst & 7) << 3 | (src & 7)]);
    code
}

struct Ram {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below (an architectural fault of the
    /// backend, which the instruction must not turn into a partial transfer).
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

/// Distinct GPRs, distinct XMM registers, a flag word with many bits set and an x87 state: any unwanted
/// write is visible in a whole-state comparison. The state carries no CR0 or CR4 (the bare run).
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

/// The state that the FP-init CR0/CR4 profile pair admits (CPU running, no interrupt shadow, x87
/// without selectors) plus the given control registers.
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

fn step(
    cpu: &mut LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(privilege))
}

fn run_fp_init(
    mut cpu: LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> (execute::RunResult, LongModeState) {
    let result = execute::run_with_cr4_profile(
        &mut cpu,
        ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(privilege),
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

fn xm() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 19,
        error_code: None,
        linear_address: None,
    })
}

/// A memory addressing form: REX.B, the ModRM/SIB/displacement tail (the reg field is added) and the register
/// that holds the base (displacement 0x10 for `[rbp+0x10]`).
struct Form {
    label: &'static str,
    rex_b: bool,
    tail: &'static [u8],
    base: Register,
    disp: u64,
}
const FORMS: [Form; 4] = [
    Form {
        label: "[rax]",
        rex_b: false,
        tail: &[0x00],
        base: Register::Rax,
        disp: 0,
    },
    Form {
        label: "[rbp+0x10]",
        rex_b: false,
        tail: &[0x45, 0x10],
        base: Register::Rbp,
        disp: 0x10,
    },
    Form {
        label: "[rsp]",
        rex_b: false,
        tail: &[0x04, 0x24],
        base: Register::Rsp,
        disp: 0,
    },
    Form {
        label: "[r8] (REX.B)",
        rex_b: true,
        tail: &[0x00],
        base: Register::R8,
        disp: 0,
    },
];
fn mem_code(kind: Kind, shape: Shape, form: &Form, dst: u8) -> Vec<u8> {
    let mut code = vec![prefix(shape)];
    let rex = 0x40 | u8::from(dst >= 8) << 2 | u8::from(form.rex_b);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(kind), form.tail[0] | (dst & 7) << 3]);
    code.extend(&form.tail[1..]);
    code
}
/// A state whose base register holds `address - disp`, so that the effective address is `address`.
fn mem_cpu(form: &Form, address: u64, cr0: u64, cr4: u64) -> LongModeState {
    let mut cpu = fp_init_cpu(cr0, cr4);
    cpu.gpr[form.base as usize] = address.wrapping_sub(form.disp);
    cpu
}
fn lanes32(lanes: [u32; 4]) -> u128 {
    lanes
        .iter()
        .rev()
        .fold(0, |acc, lane| acc << 32 | u128::from(*lane))
}
fn lanes64(lanes: [u64; 2]) -> u128 {
    lanes
        .iter()
        .rev()
        .fold(0, |acc, lane| acc << 64 | u128::from(*lane))
}
fn f32s(lanes: [f32; 4]) -> u128 {
    lanes32(lanes.map(f32::to_bits))
}
fn f64s(lanes: [f64; 2]) -> u128 {
    lanes64(lanes.map(f64::to_bits))
}
const FP_INIT_CR4: u64 = CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT;
/// The result of one register-form instruction under the FP-init profile pair with CR4.OSXMMEXCPT = 1.
struct Observed {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    length: usize,
}
fn run_code(
    code: &[u8],
    (dst, src): (u8, u8),
    mxcsr: u32,
    (a, b): (u128, u128),
    privilege: PrivilegeLevel,
) -> Observed {
    let mut cpu = fp_init_cpu(CR0_BASE, FP_INIT_CR4);
    cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
    set_xmm(&mut cpu, usize::from(dst), a);
    if src != dst {
        set_xmm(&mut cpu, usize::from(src), b);
    }
    let before = cpu.clone();
    let mut ram = Ram::new(code);
    let (result, after) = run_fp_init(cpu, &mut ram, privilege);
    assert!(ram.reads.is_empty() && ram.writes.is_empty());
    Observed {
        exit: result.exit,
        before,
        after,
        length: code.len(),
    }
}
/// Register-form run on xmm1 (destination) and xmm2 (source); the result vector and MXCSR, which must be Ok.
fn run_reg(kind: Kind, shape: Shape, mxcsr: u32, a: u128, b: u128) -> (u128, u32) {
    let observed = run_code(
        &reg_code(kind, shape, 1, 2, false),
        (1, 2),
        mxcsr,
        (a, b),
        PrivilegeLevel::Ring0,
    );
    assert_eq!(
        observed.exit,
        RunExit::InstructionBudgetExhausted,
        "{}",
        name(kind, shape)
    );
    (xmm(&observed.after, 1), mxcsr_of(&observed.after))
}
fn mxcsr_of(cpu: &LongModeState) -> u32 {
    mxcsr(cpu)
}
/// The destination and the source registers of a register-form encoding (prefix, optional REX, 0F, op, ModRM).
fn registers_of(code: &[u8]) -> (u8, u8) {
    let mut at = 1;
    let mut rex = 0;
    if (0x40..=0x4f).contains(&code[at]) {
        rex = code[at];
        at += 1;
    }
    assert_eq!(code[at], 0x0f);
    let modrm = code[at + 2];
    assert_eq!(modrm >> 6, 3);
    (
        (modrm >> 3 & 7) | (rex & 4) << 1,
        (modrm & 7) | (rex & 1) << 3,
    )
}
fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}
/// What a hardware vector produced: the destination register and MXCSR, or the MXCSR a faulting (#XM)
/// instruction left (the destination is not written).
#[derive(Clone, Copy, Debug)]
enum Gold {
    Done(u128, u32),
    Fault(u32),
}
use Gold::{Done, Fault};
/// (instruction bytes, MXCSR before, destination register before, source register before, hardware result).
type Golden = (&'static str, u32, u128, u128, Gold);

/// Vectors from the physical oracle (work/stage8/s2_54_fp_oracle/pick_golden.py).
#[rustfmt::skip]
const GOLDEN: &[Golden] = &[
    ("660fd0ca", 0x1f80, 0x00000000000000000000000000000000, 0xfff7ffffffffffff7ff0000000000000, Done(0xfffffffffffffffffff0000000000000, 0x1f81)),
    ("660fd0ca", 0x5f80, 0x7ff00000000000000000000000000000, 0x4340000000000000ffefffffffffffff, Done(0x7ff00000000000007fefffffffffffff, 0x5f80)),
    ("660fd0ca", 0x7f80, 0xfff00000000000000000000000000000, 0x433fffffffffffff001fffffffffffff, Done(0xfff0000000000000801fffffffffffff, 0x7f80)),
    ("660fd0ca", 0xffc0, 0x7ff80000000000010000000000000000, 0xfff8000000000000fff0000000000000, Done(0x7ff80000000000017ff0000000000000, 0xffc0)),
    ("660fd0ca", 0x0000, 0x7fffffffffffffff0000000000000000, 0x3ff00000000000004008000000000000, Done(0x7fffffffffffffffc008000000000000, 0x0000)),
    ("660fd0ca", 0x5f80, 0x3ff00000000000000000000000000000, 0x7ff7ffffffffffff7fd0000000000000, Done(0x7fffffffffffffffffd0000000000000, 0x5f81)),
    ("660fd0ca", 0x1fc0, 0x7ff7ffffffffffff0000000000000000, 0x00000000000000037fd0000000000000, Done(0x7fffffffffffffffffd0000000000000, 0x1fc1)),
    ("660fd0ca", 0x1f80, 0x000fffffffffffff0000000000000000, 0xfff00000000000007ff4000000000000, Done(0xfff00000000000007ffc000000000000, 0x1f83)),
    ("660fd0ca", 0x3f80, 0x800fffffffffffff0000000000000000, 0x8000000000000001433fffffffffffff, Done(0x8010000000000000c33fffffffffffff, 0x3f82)),
    ("660fd0ca", 0x5f80, 0x00080000000000000000000000000000, 0x00000000000000008000000000000000, Done(0x00080000000000000000000000000000, 0x5f82)),
    ("660fd0ca", 0x7f80, 0x00000000000000030000000000000000, 0x80100000000000004000000000000000, Done(0x800ffffffffffffdc000000000000000, 0x7f82)),
    ("660fd0ca", 0x9f80, 0x80100000000000000000000000000000, 0x800fffffffffffff001fffffffffffff, Done(0x801fffffffffffff801fffffffffffff, 0x9f82)),
    ("660fd0ca", 0x7f80, 0xbff00000000000000000000000000000, 0xfff00000000000018000000000000000, Done(0xfff80000000000010000000000000000, 0x7f81)),
    ("660fd0ca", 0x3f80, 0x7fe00000000000000000000000000000, 0x7ff80000000000007fffffffffffffff, Done(0x7ff80000000000007fffffffffffffff, 0x3f80)),
    ("660fd0ca", 0x0000, 0x433fffffffffffff0000000000000000, 0x3fefffffffffffff0000000000000000, Fault(0x0020)),
    ("660fd0ca", 0x1fc0, 0x40000000000000000000000000000000, 0x3fe00000000000004008000000000000, Done(0x4004000000000000c008000000000000, 0x1fc0)),
    ("660fd0ca", 0x1f80, 0x43400000000000000000000000000000, 0xbff00000000000004340000000000000, Done(0x433fffffffffffffc340000000000000, 0x1f80)),
    ("660fd0ca", 0x9f80, 0xc0000000000000000000000000000000, 0x3ff80000000000000000000000000000, Done(0xbfe00000000000000000000000000000, 0x9f80)),
    ("660fd0ca", 0x0000, 0x3fe00000000000000000000000000000, 0xfff7ffffffffffff4008000000000000, Fault(0x0001)),
    ("660fd0ca", 0x3f80, 0x3fefffffffffffff0000000000000000, 0x3feffffffffffffffff0000000000001, Done(0x3ffffffffffffffffff8000000000001, 0x3f81)),
    ("660fd0ca", 0x9f80, 0x7ff80000000000008000000000000000, 0x7fefffffffffffff7ff4000000000000, Done(0x7ff80000000000007ffc000000000000, 0x9f81)),
    ("660fd0ca", 0xffc0, 0xfff7ffffffffffff8000000000000000, 0x3ff00000000000007fffffffffffffff, Done(0xffffffffffffffff7fffffffffffffff, 0xffc1)),
    ("660fd0ca", 0x0000, 0x00000000000000018000000000000000, 0xffefffffffffffffc000000000000000, Fault(0x0002)),
    ("660fd0ca", 0x1f80, 0x80000000000000018000000000000000, 0x80000000000000007ff0000000000000, Done(0x8000000000000001fff0000000000000, 0x1f82)),
    ("660fd0ca", 0x0000, 0xbff00000000000007ff7ffffffffffff, 0x7fe00000000000003ff0000000000000, Fault(0x0001)),
    ("660fd0ca", 0x0000, 0xfff80000000000000008000000000000, 0x3ff80000000000007fd0000000000000, Fault(0x0002)),
    ("660fd0ca", 0x0000, 0xfff7ffffffffffff0008000000000000, 0x0020000000000000ffefffffffffffff, Fault(0x0003)),
    ("660fd0ca", 0x0000, 0x7fe00000000000000010000000000001, 0x7fdfffffffffffff8010000000000000, Fault(0x0028)),
    ("660fd0ca", 0x0000, 0x00000000000000007fefffffffffffff, 0x40080000000000003fe0000000000000, Fault(0x0020)),
    ("660fd0ca", 0x0000, 0x80000000000000013ff0000000000001, 0x001fffffffffffff7ff0000000000001, Fault(0x0003)),
    ("f20fd0ca", 0x5f80, 0xff800000ff8000017f80000000000000, 0x800000007fbfffff338000007f7fffff, Done(0xff800000ffc000017f800000ff7fffff, 0x5f81)),
    ("f20fd0ca", 0x7f80, 0x7fffffff00000000ff80000000000000, 0xffbfffff3f8000003f0000007fc00000, Done(0x7fffffffbf800000ff8000007fc00000, 0x7f81)),
    ("f20fd0ca", 0x9f80, 0x40a00000bf800000ffc0000000000000, 0x4b7fffffffbfffff3f80000140a00000, Done(0x4b800002ffffffffffc00000c0a00000, 0x9f81)),
    ("f20fd0ca", 0x0000, 0x340000007fc000017fffffff00000000, 0xffc123453f800000807fffff00ffffff, Done(0xffc123457fc000017fffffff80ffffff, 0x0000)),
    ("f20fd0ca", 0x1f80, 0xff800001c0000000ffc1234500000000, 0x000000017f8000003f0000007f800001, Done(0xffc00001ff800000ffc123457fc00001, 0x1f81)),
    ("f20fd0ca", 0x1fc0, 0x3fc000007fc000007fbfffff00000000, 0xc000000040000000c00000007fffffff, Done(0xbf0000007fc000007fffffff7fffffff, 0x1fc1)),
    ("f20fd0ca", 0xffc0, 0x800000007fffffff4040000000000000, 0xff800000008000007fc000004b800000, Done(0xff8000007fffffff7fc00000cb800000, 0xffc0)),
    ("f20fd0ca", 0x0000, 0x80000001007fffff3f00000000000000, 0x7fc0000040a00000400000003f7fffff, Fault(0x0002)),
    ("f20fd0ca", 0x7f80, 0x340000007e80000000000000ff800000, 0x7fc000007f800000ffc12345bf800000, Done(0x7fc00000ff800000ffc12345ff800000, 0x7f80)),
    ("f20fd0ca", 0x3f80, 0xff80000040a0000000ffffff7fc00001, 0x7f7fffff7f8000007fffffff3f800000, Done(0xff800000ff8000007fffffff7fc00001, 0x3f80)),
    ("f20fd0ca", 0x9f80, 0x404000007e8000007fc000017fffffff, 0x4b7fffff7f8000007f0000003f7fffff, Done(0x4b800001ff8000007fc000017fffffff, 0x9f80)),
    ("f20fd0ca", 0x3f80, 0xffc123457fbfffff0000000080000000, 0x4b7fffff34000000338000003f7fffff, Done(0xffc123457fffffff33800000bf7fffff, 0x3f81)),
    ("f20fd0ca", 0x1f80, 0x404000007f0000007fffffff80000000, 0x3e8000007e80000040a00000c0000000, Done(0x405000007e8000007fffffff40000000, 0x1f80)),
    ("f20fd0ca", 0x3f80, 0x7effffff7f800000ffc1234580000000, 0xff8000008000000140a000004b800000, Done(0xff8000007f800000ffc12345cb800000, 0x3f82)),
    ("f20fd0ca", 0x0000, 0x7fa000004b0000000000000180000000, 0x80000001ff8000007fbfffff01000000, Fault(0x0001)),
    ("f20fd0ca", 0x0000, 0x340000003f8000014040000080000000, 0x7f800000338000000080000140000000, Fault(0x0020)),
    ("f20fd0ca", 0x0000, 0x00ffffff7fc000013e8000007fbfffff, 0x80800000ff8000007fc0000100800001, Fault(0x0001)),
    ("f20fd0ca", 0x0000, 0xff8000007e8000003f7fffff00000001, 0x7f80000180000001bf800000c0000000, Fault(0x0003)),
    ("f20fd0ca", 0x0000, 0xbf800000000000017fc00001807fffff, 0x808000000000000100000000bf800000, Fault(0x0002)),
    ("f20fd0ca", 0x0000, 0x4b800000008000003400000000000003, 0x0100000000800001c00000007fc00000, Fault(0x0030)),
    ("f20fd0ca", 0x1f80, 0x000000017fffffff7fc000013f800000, 0x800000014040000080000001c0000000, Done(0x000000007fffffff7fc0000140400000, 0x1f82)),
    ("f20fd0ca", 0x5f80, 0x400000003f000000ffc123453f800000, 0x3f8000007fc00001338000003e800000, Done(0x404000007fc00001ffc123453f400000, 0x5f80)),
    ("f20fd0ca", 0xffc0, 0x3f0000007fc000007fbfffff3f800000, 0x7fbfffff00ffffff4b7fffff807fffff, Done(0x7fffffff7fc000007fffffff3f800000, 0xffc1)),
    ("f20fd0ca", 0x0000, 0x00000003ffc12345808000003f800000, 0x00ffffff00ffffffff800001807fffff, Fault(0x0003)),
    ("f20fd0ca", 0x9f80, 0x800000013f800001bf8000003f800000, 0xff8000003fc000003f0000007f800000, Done(0xff800000befffffcbf000000ff800000, 0x9f82)),
    ("f20fd0ca", 0x0000, 0x8000000000800000007fffff7f7fffff, 0x7fc000007f7fffff7fc00001ffc00000, Fault(0x0020)),
    ("f20fd0ca", 0x5f80, 0x4b7fffff4000000080000001bf800000, 0xffc000003400000000800001ffc12345, Done(0xffc000003fffffff00800000ffc12345, 0x5f82)),
    ("f20fd0ca", 0x1fc0, 0x40000000400000007effffffbf800000, 0x004000007fc00001ffc123453e800000, Done(0x400000007fc00001ffc12345bfa00000, 0x1fc0)),
    ("f20fd0ca", 0x7f80, 0x0000000300000000ffc0000040400000, 0x007fffff80000000ff8000004b7fffff, Done(0x0080000200000000ffc00000cb7ffffc, 0x7f82)),
    ("f20fd0ca", 0x1f80, 0x000000017fc000017fffffff3f7fffff, 0x00000001000000037e8000007f800001, Done(0x000000027fc000017fffffff7fc00001, 0x1f83)),
    ("660f7cca", 0x1f80, 0x00000000000000000000000000000000, 0x80100000000000008000000000000000, Done(0x80100000000000000000000000000000, 0x1f80)),
    ("660f7cca", 0x5f80, 0x7ff00000000000000000000000000000, 0x3fe00000000000003ff0000000000001, Done(0x3ff80000000000017ff0000000000000, 0x5f80)),
    ("660f7cca", 0x9f80, 0xfff80000000000000000000000000000, 0x00000000000000007ff0000000000001, Done(0x7ff8000000000001fff8000000000000, 0x9f81)),
    ("660f7cca", 0x1f80, 0xfff8123456789abc0000000000000000, 0x00080000000000007ff4000000000000, Done(0x7ffc000000000000fff8123456789abc, 0x1f81)),
    ("660f7cca", 0x0000, 0x80000000000000010000000000000000, 0xc0000000000000008000000000000001, Fault(0x0002)),
    ("660f7cca", 0x1f80, 0x000fffffffffffff0000000000000000, 0x3ca00000000000003cb0000000000000, Done(0x3cb8000000000000000fffffffffffff, 0x1f82)),
    ("660f7cca", 0x1fc0, 0x00100000000000000000000000000000, 0x7ff80000000000004008000000000000, Done(0x7ff80000000000000010000000000000, 0x1fc0)),
    ("660f7cca", 0xffc0, 0x00100000000000010000000000000000, 0x800ffffffffffffffff7ffffffffffff, Done(0xffffffffffffffff0010000000000001, 0xffc1)),
    ("660f7cca", 0x3f80, 0x7fe00000000000000000000000000000, 0xfff7ffffffffffff3ca0000000000000, Done(0xffffffffffffffff7fe0000000000000, 0x3f81)),
    ("660f7cca", 0x5f80, 0x7fd00000000000000000000000000000, 0xfff00000000000014014000000000000, Done(0xfff80000000000017fd0000000000000, 0x5f81)),
    ("660f7cca", 0x7f80, 0x3fd00000000000000000000000000000, 0x800fffffffffffff7fffffffffffffff, Done(0x7fffffffffffffff3fd0000000000000, 0x7f80)),
    ("660f7cca", 0x1fc0, 0x40000000000000000000000000000000, 0x7ff4000000000000001fffffffffffff, Done(0x7ffc0000000000004000000000000000, 0x1fc1)),
    ("660f7cca", 0x7f80, 0x001fffffffffffff0000000000000000, 0x7ff00000000000017ff0000000000000, Done(0x7ff8000000000001001fffffffffffff, 0x7f81)),
    ("660f7cca", 0x9f80, 0xc0000000000000000000000000000000, 0x00000000000000003fefffffffffffff, Done(0x3fefffffffffffffc000000000000000, 0x9f80)),
    ("660f7cca", 0xffc0, 0x40080000000000000000000000000000, 0x7fffffffffffffff7ff8000000000001, Done(0x7ff80000000000014008000000000000, 0xffc0)),
    ("660f7cca", 0x0000, 0x3fe00000000000000000000000000000, 0x7ff00000000000007ff8000000000000, Done(0x7ff80000000000003fe0000000000000, 0x0000)),
    ("660f7cca", 0x3f80, 0x3fefffffffffffff0000000000000000, 0x7fdfffffffffffff7ff0000000000000, Done(0x7ff00000000000003fefffffffffffff, 0x3f80)),
    ("660f7cca", 0x9f80, 0x3ff0000000000001fff8000000000000, 0x00100000000000010000000000000003, Done(0x0010000000000004fff8000000000000, 0x9f82)),
    ("660f7cca", 0x0000, 0x7ff80000000000018000000000000000, 0x7fe00000000000004008000000000000, Fault(0x0020)),
    ("660f7cca", 0x3f80, 0x000fffffffffffff8000000000000000, 0x80100000000000000008000000000000, Done(0x8008000000000000000fffffffffffff, 0x3f82)),
    ("660f7cca", 0x0000, 0x40080000000000008000000000000000, 0x7fe00000000000007ff7ffffffffffff, Fault(0x0001)),
    ("660f7cca", 0x0000, 0x7ff40000000000007ff7ffffffffffff, 0xfff80000000000007fe0000000000000, Fault(0x0001)),
    ("660f7cca", 0x0000, 0x7fefffffffffffff000fffffffffffff, 0x7ff0000000000001fff8000000000000, Fault(0x0003)),
    ("660f7cca", 0x0000, 0x4008000000000000800fffffffffffff, 0x00100000000000014014000000000000, Fault(0x0002)),
    ("660f7cca", 0x0000, 0xfff7ffffffffffff3ff0000000000000, 0x3fd00000000000004008000000000000, Fault(0x0001)),
    ("660f7cca", 0x0000, 0x3fe0000000000000ffefffffffffffff, 0x001fffffffffffff3fefffffffffffff, Fault(0x0020)),
    ("660f7cca", 0x1f80, 0x7ff4000000000000c000000000000000, 0x00000000000000017ff0000000000000, Done(0x7ff00000000000007ffc000000000000, 0x1f83)),
    ("660f7cca", 0x0000, 0x0008000000000000c000000000000000, 0x7fefffffffffffff7ff7ffffffffffff, Fault(0x0003)),
    ("660f7cca", 0x5f80, 0xc000000000000000c000000000000000, 0x001fffffffffffff8000000000000001, Done(0x001ffffffffffffec010000000000000, 0x5f82)),
    ("660f7cca", 0x7f80, 0xc0000000000000004008000000000000, 0x7ff0000000000000800fffffffffffff, Done(0x7ff00000000000003ff0000000000000, 0x7f82)),
    ("f20f7cca", 0x5f80, 0xffbfffff807fffff7f80000000000000, 0x01000000ffbfffff4b7fffff7f800001, Done(0xffffffff7fc00001ffffffff7f800000, 0x5f81)),
    ("f20f7cca", 0x0000, 0x00ffffff338000007fffffff00000000, 0x404000007f8000003fc000007fc00000, Fault(0x0020)),
    ("f20f7cca", 0x9f80, 0x000000007fc00000ffbfffff00000000, 0x8000000034000000bf800000ffbfffff, Done(0x34000000ffffffff7fc00000ffffffff, 0x9f81)),
    ("f20f7cca", 0x0000, 0x7f800001ffbfffff8000000100000000, 0x3f00000001000000ffc1234501000000, Fault(0x0003)),
    ("f20f7cca", 0x7f80, 0xff8000007effffffbf80000000000000, 0x7fbfffff7f800001c00000003e800000, Done(0x7fc00001bfe00000ff800000bf800000, 0x7f81)),
    ("f20f7cca", 0x1fc0, 0x4b800000004000003400000000000000, 0x00000000ffc00000807fffff7fc00001, Done(0xffc000007fc000014b80000034000000, 0x1fc0)),
    ("f20f7cca", 0xffc0, 0x0040000000ffffff4040000000000000, 0x7fffffff000000013f7fffffffc12345, Done(0x7fffffffffc1234500ffffff40400000, 0xffc0)),
    ("f20f7cca", 0x0000, 0x00ffffff3e8000003f00000000000000, 0xc00000003fc000007fbfffff00800000, Fault(0x0001)),
    ("f20f7cca", 0x3f80, 0x3f00000040a000003f7fffff00000000, 0x7fa00000000000007f8000004b000000, Done(0x7fe000007f80000040b000003f7fffff, 0x3f81)),
    ("f20f7cca", 0x5f80, 0x01000000807fffff007fffff7f800000, 0x7f8000004b000000007fffff007fffff, Done(0x7f80000000fffffe008000017f800000, 0x5f82)),
    ("f20f7cca", 0x9f80, 0xc00000007fc00001800000007fc00000, 0x4b7fffff000000004040000040a00000, Done(0x4b7fffff410000007fc000017fc00000, 0x9f80)),
    ("f20f7cca", 0x7f80, 0x4b800000800000007fffffff7fc00000, 0xff80000040a000008000000000000001, Done(0xff800000000000014b8000007fc00000, 0x7f82)),
    ("f20f7cca", 0x9f80, 0x3f000000338000004b800000ffc00000, 0x807fffff807fffff404000003f000000, Done(0x80fffffe406000003f000001ffc00000, 0x9f82)),
    ("f20f7cca", 0x7f80, 0x404000003fc000007f80000080000000, 0xbf8000003f800001c00000003f800001, Done(0x34000000bf7ffffe409000007f800000, 0x7f80)),
    ("f20f7cca", 0x0000, 0x7f7fffff00ffffff7fc0000180000000, 0x00000003340000004b7fffff00000001, Fault(0x0002)),
    ("f20f7cca", 0x1fc0, 0x7fc0000100800000bf80000080000000, 0x40000000c0000000ff800001ffbfffff, Done(0x00000000ffffffff7fc00001bf800000, 0x1fc1)),
    ("f20f7cca", 0x0000, 0x7e8000003fc000004b00000080000000, 0x7fbfffff4b000000ffbfffffffc12345, Fault(0x0001)),
    ("f20f7cca", 0xffc0, 0xff8000014b7fffffc000000080000000, 0x00000001400000008000000100ffffff, Done(0x4000000000ffffffffc00001c0000000, 0xffc1)),
    ("f20f7cca", 0x0000, 0x7f800000ffc000004040000080000000, 0x3f000000ff800000ffc1234580000001, Done(0xff800000ffc12345ffc0000040400000, 0x0000)),
    ("f20f7cca", 0x0000, 0x7fffffff3400000000400000ffbfffff, 0x3f7fffff7fbfffff4b8000007fbfffff, Fault(0x0001)),
    ("f20f7cca", 0x0000, 0x800000013e800000807fffff00000001, 0x00000003ffc00000000000007fa00000, Fault(0x0003)),
    ("f20f7cca", 0x0000, 0xffc00000ffc000007fc00001807fffff, 0x40400000ffc000004b00000080800000, Fault(0x0020)),
    ("f20f7cca", 0x0000, 0x7fc0000040a000008080000000400000, 0xffc123453f8000007f80000001000000, Fault(0x0002)),
    ("f20f7cca", 0x1f80, 0x7f000000ffc0000000ffffff80800000, 0x0000000380000000ffc0000034000000, Done(0x00000003ffc00000ffc00000007fffff, 0x1f82)),
    ("f20f7cca", 0x5f80, 0x3fc0000034000000000000003f800000, 0x40000000000000003e8000007fffffff, Done(0x400000007fffffff3fc000013f800000, 0x5f80)),
    ("f20f7cca", 0x1f80, 0x008000008000000033800000bf800000, 0x4b000000ff800000ff800000ff800000, Done(0xff800000ff80000000800000bf7fffff, 0x1f80)),
    ("f20f7cca", 0x1f80, 0x80000000bf800000c0000000bf800000, 0xff800001bf800000400000007f800001, Done(0xffc000017fc00001bf800000c0400000, 0x1f81)),
    ("f20f7cca", 0x3f80, 0x4000000080000000ffc0000040000000, 0x7fc000018000000140400000c0000000, Done(0x7fc000013f80000040000000ffc00000, 0x3f80)),
    ("f20f7cca", 0x1f80, 0xbf8000004040000040a0000040000000, 0x0000000080000001ff8000017f7fffff, Done(0x80000001ffc000014000000040e00000, 0x1f83)),
    ("f20f7cca", 0x3f80, 0x80000001ffc123453e80000040400000, 0x3f00000000000000807fffff80000001, Done(0x3f00000080800000ffc1234540500000, 0x3f82)),
    ("660f7dca", 0x5f80, 0x7ff00000000000000000000000000000, 0xfff80000000000007fdfffffffffffff, Done(0xfff8000000000000fff0000000000000, 0x5f80)),
    ("660f7dca", 0x7f80, 0xfff00000000000000000000000000000, 0xfff8123456789abc7ff0000000000000, Done(0xfff8123456789abc7ff0000000000000, 0x7f80)),
    ("660f7dca", 0x1fc0, 0x7ff80000000000000000000000000000, 0x7ff0000000000001ffefffffffffffff, Done(0x7ff80000000000017ff8000000000000, 0x1fc1)),
    ("660f7dca", 0x7f80, 0x7ff40000000000000000000000000000, 0x3ff80000000000007fffffffffffffff, Done(0x7fffffffffffffff7ffc000000000000, 0x7f81)),
    ("660f7dca", 0xffc0, 0x00000000000000010000000000000000, 0x00100000000000000010000000000000, Done(0x00000000000000000000000000000000, 0xffc0)),
    ("660f7dca", 0x0000, 0x80000000000000010000000000000000, 0x7fd0000000000000fff0000000000000, Fault(0x0002)),
    ("660f7dca", 0x1f80, 0x000fffffffffffff0000000000000000, 0x3ca00000000000000000000000000000, Done(0xbca0000000000000800fffffffffffff, 0x1f82)),
    ("660f7dca", 0x3f80, 0x800fffffffffffff0000000000000000, 0xfff80000000000003ca0000000000000, Done(0xfff8000000000000000fffffffffffff, 0x3f82)),
    ("660f7dca", 0x5f80, 0x00080000000000000000000000000000, 0x7ff8000000000001c000000000000000, Done(0x7ff80000000000018008000000000000, 0x5f82)),
    ("660f7dca", 0x7f80, 0x00000000000000030000000000000000, 0x4330000000000000fff8123456789abc, Done(0xfff8123456789abc8000000000000003, 0x7f82)),
    ("660f7dca", 0x9f80, 0x80100000000000000000000000000000, 0xffefffffffffffff7ff7ffffffffffff, Done(0x7fffffffffffffff0010000000000000, 0x9f81)),
    ("660f7dca", 0x1fc0, 0x40000000000000000000000000000000, 0xc000000000000000fff0000000000000, Done(0xfff0000000000000c000000000000000, 0x1fc0)),
    ("660f7dca", 0x1f80, 0x43400000000000000000000000000000, 0x7ff8000000000001fff0000000000001, Done(0xfff8000000000001c340000000000000, 0x1f81)),
    ("660f7dca", 0x3f80, 0x7fdfffffffffffff0000000000000000, 0x7ff80000000000014340000000000000, Done(0x7ff8000000000001ffdfffffffffffff, 0x3f80)),
    ("660f7dca", 0x5f80, 0x00200000000000000000000000000000, 0x7ff80000000000017ff0000000000001, Done(0x7ff80000000000018020000000000000, 0x5f81)),
    ("660f7dca", 0x0000, 0x3fe00000000000000000000000000000, 0x3ff00000000000014008000000000000, Done(0x3fffffffffffffffbfe0000000000000, 0x0000)),
    ("660f7dca", 0x1f80, 0x3ff00000000000010000000000000000, 0xfff8123456789abc3fe0000000000000, Done(0xfff8123456789abcbff0000000000001, 0x1f80)),
    ("660f7dca", 0x9f80, 0x00000000000000037ff0000000000000, 0x7fffffffffffffffffefffffffffffff, Done(0x7fffffffffffffff7ff0000000000000, 0x9f82)),
    ("660f7dca", 0x9f80, 0x7ff80000000000008000000000000000, 0x3ff80000000000003ff8000000000000, Done(0x00000000000000007ff8000000000000, 0x9f80)),
    ("660f7dca", 0xffc0, 0xfff80000000000008000000000000000, 0x3fd0000000000000fff0000000000001, Done(0xfff8000000000001fff8000000000000, 0xffc1)),
    ("660f7dca", 0x3f80, 0xfff8123456789abc8000000000000000, 0xc000000000000000fff7ffffffffffff, Done(0xfffffffffffffffffff8123456789abc, 0x3f81)),
    ("660f7dca", 0x1f80, 0x80000000000000018000000000000000, 0x7ff40000000000000000000000000001, Done(0x7ffc0000000000000000000000000001, 0x1f83)),
    ("660f7dca", 0x0000, 0x40080000000000008000000000000000, 0x7fefffffffffffffffefffffffffffff, Fault(0x0008)),
    ("660f7dca", 0x0000, 0x00000000000000037ff7ffffffffffff, 0x001fffffffffffff0000000000000000, Fault(0x0001)),
    ("660f7dca", 0x0000, 0xc0000000000000000008000000000000, 0xc0000000000000007fd0000000000000, Fault(0x0002)),
    ("660f7dca", 0x0000, 0x001fffffffffffff0010000000000000, 0x3fe00000000000003cb0000000000000, Fault(0x0010)),
    ("660f7dca", 0x0000, 0xfff7ffffffffffff3ff0000000000000, 0x000fffffffffffff4330000000000000, Fault(0x0003)),
    ("660f7dca", 0x0000, 0x3ff80000000000003ff0000000000000, 0x7ff7ffffffffffff4330000000000000, Fault(0x0001)),
    ("660f7dca", 0x0000, 0xc0000000000000003ff0000000000000, 0x40140000000000008010000000000000, Fault(0x0020)),
    ("660f7dca", 0x0000, 0xffefffffffffffff7fefffffffffffff, 0x0010000000000001ffefffffffffffff, Fault(0x0028)),
    ("f20f7dca", 0x3f80, 0xff7fffff7fa000008000000000000000, 0x7fa00000008000008000000080800000, Done(0x7fe00000808000007fe0000000000000, 0x3f81)),
    ("f20f7dca", 0x5f80, 0xc0000000ffc123457f80000000000000, 0x7fa0000000ffffffbf800000ffc00000, Done(0x7fe00000ffc00000ffc12345ff800000, 0x5f81)),
    ("f20f7dca", 0x0000, 0x010000003f7fffff7fffffff00000000, 0x007fffff807fffff3fc0000000ffffff, Fault(0x0002)),
    ("f20f7dca", 0x1fc0, 0x00000001800000007fbfffff00000000, 0xff7fffff00000001400000007fffffff, Done(0x7f7fffff7fffffff800000007fffffff, 0x1fc1)),
    ("f20f7dca", 0x7f80, 0x000000007fa00000bf80000000000000, 0x3e800000c0000000ffbfffff4b7fffff, Done(0xc0100000ffffffff7fe000003f800000, 0x7f81)),
    ("f20f7dca", 0x0000, 0xff800001000000037f7fffff00000000, 0x000000004b800000ffc12345ffc12345, Fault(0x0001)),
    ("f20f7dca", 0x1fc0, 0x7e800000800000004000000000000000, 0x00000000ffc123450000000040a00000, Done(0xffc1234540a00000fe800000c0000000, 0x1fc0)),
    ("f20f7dca", 0x1f80, 0x00ffffffffbfffff4b80000000000000, 0x00000001ff8000007fffffff807fffff, Done(0xff8000007fffffffffffffffcb800000, 0x1f83)),
    ("f20f7dca", 0xffc0, 0x00800000008000004040000000000000, 0x800000017f7fffff7effffffffc00000, Done(0x7f7fffffffc0000000000000c0400000, 0xffc0)),
    ("f20f7dca", 0x0000, 0x8000000140a000003f00000000000000, 0x7fa00000807fffff3380000040000000, Fault(0x0003)),
    ("f20f7dca", 0x1f80, 0x4b7fffff400000003f80000100000000, 0x00ffffff004000007fc0000080000000, Done(0x80bfffff7fc00000cb7ffffdbf800001, 0x1f82)),
    ("f20f7dca", 0x0000, 0x4b7fffffbf800000c00000007f800000, 0x7e800000ff7fffff4b8000003f800001, Fault(0x0028)),
    ("f20f7dca", 0x0000, 0x404000003e80000000ffffff7fc00000, 0x40400000c00000003f800001ffc00000, Done(0xc0a00000ffc00000c03000007fc00000, 0x0000)),
    ("f20f7dca", 0x3f80, 0xffc0000000800001ff8000007fc00001, 0x000000003e800000ff80000000000001, Done(0x3e8000007f800000ffc000007fc00001, 0x3f82)),
    ("f20f7dca", 0x9f80, 0x007fffffff800000000000017fffffff, 0x01000000ff800000007fffff7fffffff, Done(0xff8000007fffffffff8000007fffffff, 0x9f82)),
    ("f20f7dca", 0x1f80, 0x008000017f8000007fffffff80000000, 0x4b00000000000000ff8000004b800000, Done(0xcb0000007f8000007f8000007fffffff, 0x1f80)),
    ("f20f7dca", 0x3f80, 0x7f0000007f000000ffc1234580000000, 0x3f800001340000004b8000004b800000, Done(0xbf8000008000000080000000ffc12345, 0x3f80)),
    ("f20f7dca", 0xffc0, 0xc0000000800000013fc0000080000000, 0x3f8000017f800001ffc00000ff800000, Done(0x7fc00001ffc0000040000000bfc00000, 0xffc1)),
    ("f20f7dca", 0x5f80, 0x00000003008000013f7fffff80000000, 0x807fffff00000001800000007f7fffff, Done(0x008000007f7fffff007ffffebf7fffff, 0x5f82)),
    ("f20f7dca", 0x0000, 0xffc123457f7fffff3e8000007fbfffff, 0x7effffffffbfffff7fc0000101000000, Fault(0x0001)),
    ("f20f7dca", 0x0000, 0xff80000000ffffffff7fffff80000001, 0x80000000808000003f800000ff7fffff, Fault(0x0002)),
    ("f20f7dca", 0x7f80, 0x7fc000003e8000007f800000807fffff, 0x01000000ffc000000080000001000000, Done(0xffc00000008000007fc00000ff800000, 0x7f82)),
    ("f20f7dca", 0x0000, 0x808000003f7fffffffc000003f800000, 0x3f8000013e800000ffc123457f7fffff, Fault(0x0020)),
    ("f20f7dca", 0x0000, 0x400000003f8000014b7fffffff7fffff, 0x00ffffff008000014b000000ffc12345, Fault(0x0030)),
    ("f20f7dca", 0x7f80, 0x01000000ffc0000000000000bf800000, 0x004000007fc000007fffffff40000000, Done(0x7fc000007fffffffffc00000bf800000, 0x7f80)),
    ("f20f7dca", 0x9f80, 0xff80000000800000ff800001bf800000, 0x007fffff7fffffff3f800000ff800000, Done(0x7fffffffff8000007f800000ffc00001, 0x9f81)),
    ("f20f7dca", 0x0000, 0xff8000004b0000000080000000ffffff, 0x800000004b7fffff3f00000000000000, Fault(0x0010)),
    ("f20f7dca", 0x1f80, 0xffbfffff008000007f8000013f000000, 0x7f800000ffc000003f8000004b7fffff, Done(0xffc000004b7ffffeffffffff7fc00001, 0x1f81)),
    ("f20f7dca", 0x5f80, 0x7f80000000ffffff3f8000003f800001, 0xff800000bf800000800000007e800000, Done(0x7f8000007e800000ff80000034000000, 0x5f80)),
    ("f20f7dca", 0x9f80, 0x000000003f0000007fc000003f7fffff, 0x8000000033800000ffc12345ffc00000, Done(0x33800000ffc000003f0000007fc00000, 0x9f80)),
    ("660fd0dc", 0x9f80, 0x4340000000000000ffefffffffffffff, 0x7ff00000000000000000000000000000, Done(0x7ff0000000000000ffefffffffffffff, 0x9f80)),
    ("660fd0dc", 0xffc0, 0x433fffffffffffff001fffffffffffff, 0xfff00000000000000000000000000000, Done(0xfff0000000000000001fffffffffffff, 0xffc0)),
    ("660fd0dc", 0x3f80, 0xfff8000000000000fff0000000000000, 0x7ff80000000000010000000000000000, Done(0xfff8000000000000fff0000000000000, 0x3f80)),
    ("660fd0dc", 0x5f80, 0x3ff00000000000004008000000000000, 0x7fffffffffffffff0000000000000000, Done(0x7fffffffffffffff4008000000000000, 0x5f80)),
    ("660fd0dc", 0x0000, 0x00000000000000037fd0000000000000, 0x7ff7ffffffffffff0000000000000000, Fault(0x0001)),
    ("660fd0dc", 0x1fc0, 0x8000000000000001433fffffffffffff, 0x800fffffffffffff0000000000000000, Done(0x8000000000000000433fffffffffffff, 0x1fc0)),
    ("660fd0dc", 0x0000, 0x0000000000000000800fffffffffffff, 0x00100000000000000000000000000000, Fault(0x0002)),
    ("660fd0dc", 0x7f80, 0xbff00000000000004340000000000000, 0x43400000000000000000000000000000, Done(0x433fffffffffffff4340000000000000, 0x7f80)),
    ("660fd0dc", 0x1f80, 0x3ff80000000000000000000000000000, 0xc0000000000000000000000000000000, Done(0xbfe00000000000000000000000000000, 0x1f80)),
    ("660fd0dc", 0x0000, 0x7ff00000000000010000000000000003, 0x001fffffffffffff8000000000000000, Fault(0x0003)),
    ("f20fd0dc", 0x5f80, 0xffc123453f800000807fffff00ffffff, 0x340000007fc000017fffffff00000000, Done(0xffc123457fc000017fffffff00ffffff, 0x5f80)),
    ("f20fd0dc", 0x0000, 0x33800000ff8000010100000040a00000, 0x004000003f0000000080000000000000, Fault(0x0003)),
    ("f20fd0dc", 0x1fc0, 0xff7fffff000000037f8000007e800000, 0x807fffff7f7fffff7f00000000000000, Done(0xff7fffffff7fffff7f8000007e800000, 0x1fc0)),
    ("f20fd0dc", 0xffc0, 0x807fffff338000007fffffffff7fffff, 0x3f7fffff800000013e80000000000000, Done(0x3f7fffff338000007fffffffff7fffff, 0xffc0)),
    ("f20fd0dc", 0x0000, 0xc00000003f7fffff7fc0000180000001, 0x01000000000000033400000000000000, Fault(0x0002)),
    ("f20fd0dc", 0x0000, 0x7f0000007fc000003fc0000080000000, 0x40a000007f8000014000000000000000, Fault(0x0001)),
    ("f20fd0dc", 0x3f80, 0xff800000008000007fc000004b800000, 0x800000007fffffff4040000000000000, Done(0xff8000007fffffff7fc000004b800000, 0x3f80)),
    ("f20fd0dc", 0x1f80, 0x4b7fffff7f8000007f0000003f7fffff, 0x404000007e8000007fc000017fffffff, Done(0x4b8000017f8000007fc000017fffffff, 0x1f80)),
    ("f20fd0dc", 0x7f80, 0x3e8000007e80000040a00000c0000000, 0x404000007f0000007fffffff80000000, Done(0x40500000fe8000007fffffffc0000000, 0x7f80)),
    ("f20fd0dc", 0x9f80, 0x3f8000007fc00001338000003e800000, 0x400000003f000000ffc123453f800000, Done(0x404000007fc00001ffc12345bf400000, 0x9f80)),
    ("660f7cdc", 0x7f80, 0x80100000000000008000000000000000, 0x00000000000000000000000000000000, Done(0x00000000000000008010000000000000, 0x7f80)),
    ("660f7cdc", 0x1fc0, 0x00080000000000007fe0000000000000, 0x80000000000000000000000000000000, Done(0x00000000000000007fe0000000000000, 0x1fc0)),
    ("660f7cdc", 0x9f80, 0x3fe00000000000003ff0000000000001, 0x7ff00000000000000000000000000000, Done(0x7ff00000000000003ff8000000000001, 0x9f80)),
    ("660f7cdc", 0xffc0, 0x7fdfffffffffffff0000000000000003, 0xbff00000000000000000000000000000, Done(0xbff00000000000007fdfffffffffffff, 0xffc0)),
    ("660f7cdc", 0x0000, 0x7ff4000000000000001fffffffffffff, 0x40000000000000000000000000000000, Fault(0x0001)),
    ("660f7cdc", 0x1f80, 0x00000000000000003fefffffffffffff, 0xc0000000000000000000000000000000, Done(0xc0000000000000003fefffffffffffff, 0x1f80)),
    ("660f7cdc", 0x3f80, 0x7fffffffffffffff7ff8000000000001, 0x40080000000000000000000000000000, Done(0x40080000000000007ff8000000000001, 0x3f80)),
    ("660f7cdc", 0x5f80, 0x7ff00000000000007ff8000000000000, 0x3fe00000000000000000000000000000, Done(0x3fe00000000000007ff8000000000000, 0x5f80)),
    ("660f7cdc", 0x0000, 0x4014000000000000000fffffffffffff, 0x3ff0000000000000fff0000000000001, Fault(0x0003)),
    ("660f7cdc", 0x0000, 0x3fe0000000000000000fffffffffffff, 0x00000000000000038000000000000000, Fault(0x0002)),
    ("f20f7cdc", 0x1fc0, 0x4b800000000000033f7fffffff800000, 0x7f7fffff004000008000000000000000, Done(0x7f7fffff000000004b800000ff800000, 0x1fc0)),
    ("f20f7cdc", 0x0000, 0x008000007effffff7effffff40a00000, 0x007fffff3fc000007fbfffff00000000, Fault(0x0003)),
    ("f20f7cdc", 0x0000, 0x00000000ffc00000807fffff7fc00001, 0x4b800000004000003400000000000000, Fault(0x0002)),
    ("f20f7cdc", 0x0000, 0x000000017fbfffff008000017f7fffff, 0x7fa000007fc000014000000000000000, Fault(0x0001)),
    ("f20f7cdc", 0x1f80, 0x4b7fffff000000004040000040a00000, 0xc00000007fc00001800000007fc00000, Done(0x7fc000017fc000004b7fffff41000000, 0x1f80)),
    ("f20f7cdc", 0xffc0, 0xbf8000003f800001c00000003f800001, 0x404000003fc000007f80000080000000, Done(0x409000007f80000034000000bf7ffffe, 0xffc0)),
    ("f20f7cdc", 0x5f80, 0x3f000000ff800000ffc1234580000001, 0x7f800000ffc000004040000080000000, Done(0xffc0000040400000ff800000ffc12345, 0x5f80)),
    ("f20f7cdc", 0x9f80, 0x40000000000000003e8000007fffffff, 0x3fc0000034000000000000003f800000, Done(0x3fc000013f800000400000007fffffff, 0x9f80)),
    ("f20f7cdc", 0x7f80, 0x4b000000ff800000ff800000ff800000, 0x008000008000000033800000bf800000, Done(0x00800000bf7fffffff800000ff800000, 0x7f80)),
    ("f20f7cdc", 0x3f80, 0x00000003ffc000003400000034000000, 0xffc0000000ffffffffc000003f7fffff, Done(0xffc00000ffc00000ffc0000034800000, 0x3f80)),
    ("660f7ddc", 0x9f80, 0xfff80000000000007fdfffffffffffff, 0x7ff00000000000000000000000000000, Done(0xfff0000000000000fff8000000000000, 0x9f80)),
    ("660f7ddc", 0xffc0, 0xfff8123456789abc7ff0000000000000, 0xfff00000000000000000000000000000, Done(0x7ff0000000000000fff8123456789abc, 0xffc0)),
    ("660f7ddc", 0x0000, 0x7ff0000000000001ffefffffffffffff, 0x7ff80000000000000000000000000000, Fault(0x0001)),
    ("660f7ddc", 0x3f80, 0xfff8123456789abc3cb0000000000000, 0x00100000000000010000000000000000, Done(0x8010000000000001fff8123456789abc, 0x3f80)),
    ("660f7ddc", 0x5f80, 0x3ff00000000000014008000000000000, 0x3fe00000000000000000000000000000, Done(0xbfe00000000000003fffffffffffffff, 0x5f80)),
    ("660f7ddc", 0x7f80, 0xfff8123456789abc3fe0000000000000, 0x3ff00000000000010000000000000000, Done(0xbff0000000000001fff8123456789abc, 0x7f80)),
    ("660f7ddc", 0x1fc0, 0x4330000000000000800fffffffffffff, 0x3fefffffffffffff0000000000000000, Done(0xbfefffffffffffffc330000000000000, 0x1fc0)),
    ("660f7ddc", 0x1f80, 0x3ff80000000000003ff8000000000000, 0x7ff80000000000008000000000000000, Done(0x7ff80000000000000000000000000000, 0x1f80)),
    ("660f7ddc", 0x0000, 0xfff8000000000000ffefffffffffffff, 0x00000000000000038000000000000000, Fault(0x0002)),
    ("660f7ddc", 0x0000, 0x7ff7ffffffffffff3ff0000000000000, 0x00080000000000003ff0000000000000, Fault(0x0003)),
    ("f20f7ddc", 0xffc0, 0xbf800000340000007e800000807fffff, 0x0000000140a00000ff80000000000000, Done(0x40a000007f8000003f800001fe800000, 0xffc0)),
    ("f20f7ddc", 0x0000, 0x7f8000014b0000003f7fffff7f7fffff, 0x3f8000007fa000007fc0000000000000, Fault(0x0001)),
    ("f20f7ddc", 0x0000, 0xff7fffff00000001400000007fffffff, 0x00000001800000007fbfffff00000000, Fault(0x0003)),
    ("f20f7ddc", 0x5f80, 0x40400000c00000003f800001ffc00000, 0x404000003e80000000ffffff7fc00000, Done(0xc03000007fc00000c0a00000ffc00000, 0x5f80)),
    ("f20f7ddc", 0x7f80, 0x4b00000000000000ff8000004b800000, 0x008000017f8000007fffffff80000000, Done(0x7f8000007fffffffcb0000007f800000, 0x7f80)),
    ("f20f7ddc", 0x1fc0, 0x3f800001340000004b8000004b800000, 0x7f0000007f000000ffc1234580000000, Done(0x00000000ffc12345bf80000000000000, 0x1fc0)),
    ("f20f7ddc", 0x0000, 0x4040000040a000000100000000ffffff, 0x80000000007fffff3e80000080000000, Fault(0x0002)),
    ("f20f7ddc", 0x3f80, 0x3f000000000000003f00000040000000, 0x338000003e800000400000003f800000, Done(0x3e7ffffcbf800000bf0000003fc00000, 0x3f80)),
    ("f20f7ddc", 0x9f80, 0xff800000bf800000800000007e800000, 0x7f80000000ffffff3f8000003f800001, Done(0xff800000340000007f8000007e800000, 0x9f80)),
    ("f20f7ddc", 0x1f80, 0x8000000033800000ffc12345ffc00000, 0x000000003f0000007fc000003f7fffff, Done(0x3f0000007fc0000033800000ffc00000, 0x1f80)),
    ("66450fd0e9", 0x1f80, 0x3cb00cd18cfb70223ff7ebc69d24e52f, 0x8000000000000000fff8000000000000, Done(0x3cb00cd18cfb7022fff8000000000000, 0x1f80)),
    ("66450fd0c3", 0x1f80, 0x800fffffffffffff8000000000000000, 0xfff7ffffffffffffc32c4bbc5e156b6f, Done(0xffffffffffffffff432c4bbc5e156b6f, 0x1f81)),
    ("660fd0e1", 0x3f00, 0x7ff7ffffffffffff7ff0000000000000, 0x8027697ca5a177387fe0000000000000, Fault(0x3f01)),
    ("66450fd0e0", 0x3f00, 0x3ff2da2c42c956e93fefffffffffffff, 0x3fd00000000000007fffffffffffffff, Done(0x3ff6da2c42c956e97fffffffffffffff, 0x3f00)),
    ("660fd0d2", 0x3f80, 0x7fdfffffffffffff433fffffffffffff, 0x7fdfffffffffffff433fffffffffffff, Done(0x7fefffffffffffff8000000000000000, 0x3f80)),
    ("66450fd0d8", 0x5f00, 0x7ff00000000000007ff8000000000001, 0xffd879241fdd0fde7fffffffffffffff, Done(0x7ff00000000000007ff8000000000001, 0x5f00)),
    ("66410fd0e9", 0x5f00, 0xc000000000000000fff7ffffffffffff, 0xfff00000000000000008000000000000, Fault(0x5f01)),
    ("66450fd0f3", 0x7f00, 0xc000000000000000e4d9efac0864672f, 0x7fffffffffffffff7ff7ffffffffffff, Fault(0x7f01)),
    ("66450fd0cd", 0x7f00, 0x7fdfffffffffffff3cb0000000000000, 0x7ff80000000000017ff8000000000000, Done(0x7ff80000000000017ff8000000000000, 0x7f00)),
    ("66440fd0ee", 0x5f80, 0x40140000000000007fdfffffffffffff, 0x3fd0000000000000fff8123456789abc, Done(0x4015000000000000fff8123456789abc, 0x5f80)),
    ("66450fd0f5", 0x9f00, 0xbff00000000000008001919f042b69ea, 0x00100000000000007ff4000000000000, Fault(0x9f01)),
    ("66410fd0ff", 0x9f00, 0x3ff0000000000001fff8123456789abc, 0x3ff00000000000013ff0000000000000, Done(0x4000000000000001fff8123456789abc, 0x9f00)),
    ("66440fd0c1", 0x1f40, 0x4000000000000000bff0000000000000, 0xc0000000000000000008000000000000, Done(0x0000000000000000bff0000000000000, 0x1f40)),
    ("66440fd0d2", 0x1f40, 0x7ff0000000000000fff8123456789abc, 0x7ff0000000000001432d29781ba6e1f3, Fault(0x1f41)),
    ("66450fd0df", 0x7f80, 0x4014000000000000fff8123456789abc, 0xc008a7ddf5777d85c000000000000000, Done(0x3ffeb044151104f6fff8123456789abc, 0x7f80)),
    ("660fd0cf", 0xff00, 0x80171c307ff3c3597ffa100fd454ef73, 0xfff8123456789abc0000000000000001, Done(0xfff8123456789abc7ffa100fd454ef73, 0xff00)),
    ("66450fd0e9", 0x7f40, 0x7fd00000000000004000000000000000, 0xfff7ffffffffffffc328b8b9c3819b9c, Fault(0x7f41)),
    ("66410fd0de", 0x7f40, 0xfff8123456789abc80068ce9f9c14136, 0x8016891fab00843f0008000000000000, Done(0xfff8123456789abc8000000000000000, 0x7f40)),
    ("66440fd0dc", 0x1fc0, 0x3ff00000000000013ff0000000000000, 0xbff00000000000004014000000000000, Done(0x3cb0000000000000c010000000000000, 0x1fc0)),
    ("66440fd0d2", 0x9f80, 0x7ff00000000000004008000000000000, 0x3ff00000000000014000000000000000, Done(0x7ff00000000000003ff0000000000000, 0x9f80)),
    ("660fd0dc", 0x80c0, 0x800fffffffffffff0000000000000000, 0x3ff80000000000008000000000000001, Done(0x3ff80000000000000000000000000000, 0x80c0)),
    ("66440fd0de", 0xffc0, 0x00000000000000008010000000000000, 0x3cba9d8e886d97ea8003e817034e36fe, Done(0x3cba9d8e886d97ea8010000000000000, 0xffc0)),
    ("66440fd0e4", 0x0000, 0x7fe00000000000007ff456a89a69a4e7, 0xfff8123456789abc801c4f07314248d9, Fault(0x0001)),
    ("660fd0cd", 0x0000, 0xfff8ce009b0041e10000000000000000, 0x7fffffffffffffff433fffffffffffff, Done(0xfff8ce009b0041e1c33fffffffffffff, 0x0000)),
    ("f2410fd0d5", 0x1f80, 0x7fc00000bf800000a8cda5bc7f800001, 0x80000000ffa5ebf90000000040000000, Done(0x7fc00000ffe5ebf9a8cda5bc7fc00001, 0x1f81)),
    ("f2450fd0f6", 0x1f80, 0x3fa3073e00d95bf00000000000800000, 0x3fa3073e00d95bf00000000000800000, Done(0x4023073e000000000000000000000000, 0x1f80)),
    ("f2450fd0c9", 0x3f00, 0xbf80000033800000bf80000073b2b36d, 0xbf80000033800000bf80000073b2b36d, Done(0xc000000080000000c000000080000000, 0x3f00)),
    ("f2440fd0fe", 0x3f00, 0x7fbfffff7fffffff7f0000003f800000, 0x7fa00000800000007effffffce4225dc, Fault(0x3f01)),
    ("f2450fd0e4", 0x3f80, 0xbf8dea64008000010080000040a00000, 0xbf8dea64008000010080000040a00000, Done(0xc00dea64800000000100000080000000, 0x3f80)),
    ("f2440fd0ec", 0x5f00, 0x00800001e41268c77f7fffff7f800001, 0x400000007fc00000ff80000134000000, Fault(0x5f01)),
    ("f2450fd0ed", 0x5f00, 0xd0cb10bac00000004b8000007fc00000, 0xd0cb10bac00000004b8000007fc00000, Done(0xd14b10ba000000004c0000007fc00000, 0x5f00)),
    ("f2410fd0f8", 0x7f00, 0x7fc0000000ffffff7fc00000ffc00000, 0x4b7fffff008000007f0000003f800000, Done(0x7fc00000007fffff7fc00000ffc00000, 0x7f00)),
    ("f2410fd0ff", 0x7f00, 0x34000000ff8000014040000033800000, 0x7fbfffffff80000040a000007f800000, Fault(0x7f01)),
    ("f2450fd0db", 0x5f80, 0x338000002d90d3ce7e800000ffc12345, 0x338000002d90d3ce7e800000ffc12345, Done(0x34000000000000007f000000ffc12345, 0x5f80)),
    ("f2450fd0cf", 0x9f00, 0x7f800000017e9584ff800000ffe90c45, 0x05bcf6ad00ffffff404000003fc00000, Done(0x7f80000000fd2b09ff800000ffe90c45, 0x9f00)),
    ("f20fd0de", 0x9f00, 0x405537163e800000bf8000004b000000, 0xcf9fdc937fbfffff3f8000004034ff8b, Fault(0x9f01)),
    ("f2440fd0f6", 0x1f40, 0x807fffffff800001810c3a8e7e800000, 0xffd8bbd5bf80000040400000b06f50af, Fault(0x1f41)),
    ("f2440fd0cc", 0x1f40, 0x000000033f800000c0000000807fffff, 0x40000000004000007f8000007fc00000, Done(0x400000003f8000007f8000007fc00000, 0x1f40)),
    ("f2450fd0e4", 0x7f80, 0x3f7fffff7fffffff80000000bfc57170, 0x3f7fffff7fffffff80000000bfc57170, Done(0x3fffffff7fffffff8000000000000000, 0x7f80)),
    ("f20fd0f6", 0xff00, 0x0b342298f4dbf1d37fc000003e800000, 0x0b342298f4dbf1d37fc000003e800000, Done(0x0bb42298000000007fc0000000000000, 0xff00)),
    ("f2440fd0fe", 0x7f40, 0x3fc1b93840a000003f7fffff7fc00001, 0x3f000000000000037f8000004fb7fbd6, Done(0x4000dc9c40a000007f8000007fc00001, 0x7f40)),
    ("f2450fd0cb", 0x7f40, 0x800000007fa000003f8000013f000000, 0x8b7c93864b8000007f800000ffbfffff, Fault(0x7f41)),
    ("f20fd0d8", 0x1fc0, 0x7fc00001807fffff007fffff0085ab5e, 0xc352005e00000000007fffffff800000, Done(0x7fc0000180000000000000007f800000, 0x1fc0)),
    ("f20fd0e4", 0x9f80, 0xbff196de7e800000ff8000004b800000, 0xbff196de7e800000ff8000004b800000, Done(0xc07196de00000000ff80000000000000, 0x9f80)),
    ("f2410fd0ec", 0x80c0, 0xbfb1eb0d815e7542ffc1234540400000, 0x00633e76010000004078996dffc00000, Done(0xbfb1eb0d81af3aa1ffc12345ffc00000, 0x80c0)),
    ("f20fd0db", 0xffc0, 0x340000003f80000063842b6e00ffffff, 0x340000003f80000063842b6e00ffffff, Done(0x348000000000000064042b6e00000000, 0xffc0)),
    ("f20fd0e4", 0x0000, 0x40a00000fff2a8a0008000004b7fffff, 0x40a00000fff2a8a0008000004b7fffff, Done(0x41200000fff2a8a00100000000000000, 0x0000)),
    ("f2410fd0c7", 0x0000, 0x01000000340000007f800001ffc00000, 0xc00000004b0000007e80000080ecdce2, Fault(0x0001)),
    ("66410f7cd3", 0x1f80, 0x7ff0000000000001800fffffffffffff, 0x7fdfffffffffffff7fdfffffffffffff, Done(0x7fefffffffffffff7ff8000000000001, 0x1f81)),
    ("66410f7cc4", 0x1f80, 0x7fdffffffffffffffff8123456789abc, 0x7ff80000000000018000000000000000, Done(0x7ff8000000000001fff8123456789abc, 0x1f80)),
    ("66440f7cfd", 0x3f00, 0x3fe00000000000007ff4000000000000, 0x7ff80000000000007fd0000000000000, Fault(0x3f01)),
    ("66440f7cd4", 0x3f00, 0xfff8123456789abc7ff8000000000001, 0x7ff8000000000001001fffffffffffff, Done(0x7ff80000000000017ff8000000000001, 0x3f00)),
    ("66440f7cfe", 0x3f80, 0x3fe00000000000003ca0000000000000, 0x40140000000000007ff8000000000001, Done(0x7ff80000000000013fe0000000000001, 0x3f80)),
    ("660f7cf0", 0x5f00, 0x7ff80000000000013ff8000000000000, 0x0023df25e021dda47ff0000000000001, Fault(0x5f01)),
    ("66410f7cca", 0x5f00, 0x802fc4d0116faba98000000000000000, 0x0020000000000000fff8123456789abc, Done(0xfff8123456789abc802fc4d0116faba9, 0x5f00)),
    ("66450f7ce4", 0x7f00, 0x3fd00000000000007fffffffffffffff, 0x3fd00000000000007fffffffffffffff, Done(0x7fffffffffffffff7fffffffffffffff, 0x7f00)),
    ("66410f7cfd", 0x7f00, 0x40034ad5db4f93c0fff8000000000000, 0x4005556fd7034387fff7ffffffffffff, Fault(0x7f01)),
    ("660f7cc8", 0x5f80, 0x3cb00000000000003cb0000000000000, 0x7fffffffffffffff0010000000000001, Done(0x7fffffffffffffff3cc0000000000000, 0x5f80)),
    ("66450f7cd0", 0x9f00, 0x8000000000000000fff7ffffffffffff, 0xbff0000000000000fff0000000000000, Fault(0x9f01)),
    ("66410f7cdd", 0x9f00, 0xffd793afdfb8b3f90000000000000000, 0x00000000000000004330000000000000, Done(0x4330000000000000ffd793afdfb8b3f9, 0x9f00)),
    ("660f7cc8", 0x1f40, 0xffefffffffffffff7ff7ffffffffffff, 0x8010000000000000c00613e0989f309a, Fault(0x1f41)),
    ("66410f7cd7", 0x1f40, 0x3ca2b38b29cc75b43cb0000000000000, 0x7feffffffffffffffff8000000000000, Done(0xfff80000000000003cb959c594e63ada, 0x1f40)),
    ("66450f7cc9", 0x7f80, 0x7fe00000000000000000000000000000, 0x7fe00000000000000000000000000000, Done(0x7fe00000000000007fe0000000000000, 0x7f80)),
    ("66450f7ce3", 0xff00, 0x7ff00000000000007fefffffffffffff, 0x7fe33324c7caa5f97ff8000000000000, Done(0x7ff80000000000007ff0000000000000, 0xff00)),
    ("66440f7cd3", 0x7f40, 0x7fffffffffffffff2ceb433a1896ceb9, 0x0010000000000001fff8000000000000, Done(0xfff80000000000007fffffffffffffff, 0x7f40)),
    ("66450f7cea", 0x7f40, 0x7fe6a8cf8be360d57ff4000000000000, 0x7ff400000000000080244856dd3f1699, Fault(0x7f41)),
    ("66410f7cd0", 0x1fc0, 0xffefffffffffffff7fdfffffffffffff, 0x3ff00000000000003fe0000000000000, Done(0x3ff8000000000000ffdfffffffffffff, 0x1fc0)),
    ("66440f7cef", 0x9f80, 0x7fffffffffffffff0000000000000001, 0x433fffffffffffff3ff0000000000000, Done(0x43400000000000007fffffffffffffff, 0x9f80)),
    ("66440f7cfb", 0x80c0, 0x800fffffffffffff3ff8000000000000, 0x7ff8000000000001bffb50db2e9b26d0, Done(0x7ff80000000000013ff8000000000000, 0x80c0)),
    ("66440f7cc4", 0xffc0, 0x43300000000000000000000000000000, 0xffd53b69e79b21fffff8000000000000, Done(0xfff80000000000004330000000000000, 0xffc0)),
    ("66410f7cd1", 0x0000, 0x3ca0000000000000c000000000000000, 0xffdfb31f27c432f37ff4000000000000, Fault(0x0001)),
    ("660f7ce1", 0x0000, 0xbff00000000000003fd0000000000000, 0x400623f345b18614fff8000000000000, Done(0xfff8000000000000bfe8000000000000, 0x0000)),
    ("f2440f7cd7", 0x1f80, 0x00ffffff00ffffff7e800000ffc12345, 0x3e8000003f000000807fffff7fc00001, Done(0x3f4000007fc00001017fffffffc12345, 0x1f80)),
    ("f2440f7cd3", 0x1f80, 0x33800000338000000080000001000000, 0xff8000014b7fffff34000000c0000000, Done(0xffc00001bfffffff3400000001400000, 0x1f81)),
    ("f20f7cca", 0x3f00, 0x00800000ffc000005afef0d1ffbfffff, 0x7f7fffff3fc000007e8000004fe88bf7, Fault(0x3f01)),
    ("f20f7cc0", 0x3f00, 0x80862949ffc00000c000000000000000, 0x80862949ffc00000c000000000000000, Done(0xffc00000c0000000ffc00000c0000000, 0x3f00)),
    ("f2410f7ced", 0x3f80, 0x809fef2900df805800ffffff80800000, 0x3f800001c0000000f19e762e7fc00001, Done(0xbf7ffffe7fc00001003f912f007fffff, 0x3f80)),
    ("f2410f7cd3", 0x5f00, 0x7fbfffff7e8000003380000040a00000, 0x30126f6b340000004b800000b304d337, Fault(0x5f01)),
    ("f2440f7cdb", 0x5f00, 0x8000000040a00000bff005f17fc00000, 0x000000003e8000007e8000007f1edc32, Done(0x3e8000007f5edc3240a000007fc00000, 0x5f00)),
    ("f2450f7cc0", 0x7f00, 0x7e80000034000000b02fe36c7f800001, 0x7e80000034000000b02fe36c7f800001, Fault(0x7f01)),
    ("f2450f7cf6", 0x7f00, 0x3f000000bf80000000000000ff800000, 0x3f000000bf80000000000000ff800000, Done(0xbf000000ff800000bf000000ff800000, 0x7f00)),
    ("f20f7cff", 0x5f80, 0x3f7fffff80000000400000007f800000, 0x3f7fffff80000000400000007f800000, Done(0x3f7fffff7f8000003f7fffff7f800000, 0x5f80)),
    ("f2450f7cfd", 0x9f00, 0x800000007f000000404000007f800000, 0x7f000000ffbfffff4000000000ffffff, Fault(0x9f01)),
    ("f2440f7ceb", 0x9f00, 0xff7fffff7ffb0cdb7fffffff0b0c5f6a, 0x3380000000000000ffc46bb9304a76a6, Done(0x33800000ffc46bb97ffb0cdb7fffffff, 0x9f00)),
    ("f20f7cf9", 0x1f40, 0xffbfffffffbfffff7f000000807fffff, 0xff800001ffc000003f7fffff00800000, Fault(0x1f41)),
    ("f2440f7cfe", 0x1f40, 0x4b000000ffc00000bfde3bc57fc00001, 0x000000007f0000004b80000000000003, Done(0x7f0000004b800000ffc000007fc00001, 0x1f40)),
    ("f20f7ced", 0x7f80, 0x30514695ffc1234540a0000040a00000, 0x30514695ffc1234540a0000040a00000, Done(0xffc1234541200000ffc1234541200000, 0x7f80)),
    ("f2440f7cde", 0xff00, 0x7fc00001ff80000001000000ff800000, 0xffc00000bfa1ca167f80000079ee5fc8, Done(0xffc000007f8000007fc00001ff800000, 0xff00)),
    ("f20f7cfe", 0x7f40, 0x00000001f49272d1007fffff7fa00000, 0xb056aa4740a000004040000080d80c3b, Fault(0x7f41)),
    ("f20f7cc9", 0x7f40, 0x00000000ffc000000101bb3e007fffff, 0x00000000ffc000000101bb3e007fffff, Done(0xffc000000101bb3effc000000101bb3e, 0x7f40)),
    ("f2440f7cc2", 0x1fc0, 0x80000000807fffff8031f66bcfc90b7d, 0x74ef981c004000000000000080000000, Done(0x74ef981c0000000080000000cfc90b7d, 0x1fc0)),
    ("f20f7cf1", 0x9f80, 0x340000007fc00000ffc1234540166f73, 0xff7fffff7f7fffff7effffffffc12345, Done(0x00000000ffc123457fc00000ffc12345, 0x9f80)),
    ("f2410f7ce9", 0x80c0, 0x403482bcbf8e22df7fffffff80000000, 0x806b8ca10b3c76b4000000003037f0d4, Done(0x0b3c76b43037f0d43fdae2997fffffff, 0x80c0)),
    ("f20f7cde", 0xffc0, 0x008000018000000000000001007fffff, 0x800000003e800000800000003f800000, Done(0x3e8000003f8000000080000100000000, 0xffc0)),
    ("f2410f7ce5", 0x0000, 0x7f7fffff40a00000ff800001ffc37868, 0xbf8000004040000074a74b1a7f6ec8a6, Fault(0x0001)),
    ("f2450f7ce4", 0x0000, 0x7fc00001800000007fc0000000000001, 0x7fc00001800000007fc0000000000001, Done(0x7fc000017fc000007fc000017fc00000, 0x0000)),
    ("660f7de6", 0x1f80, 0xfff00000000000010000000000000001, 0x00000000000000004330000000000000, Done(0x4330000000000000fff8000000000001, 0x1f81)),
    ("660f7df1", 0x1f80, 0xfff80000000000007fdd2719a32448ff, 0x3ff00000000000004000000000000000, Done(0x3ff0000000000000fff8000000000000, 0x1f80)),
    ("66410f7de5", 0x3f00, 0x7ff40000000000008023f5af34fd01ff, 0xbca11de6cfd2e66c3fefffffffffffff, Fault(0x3f01)),
    ("66440f7de3", 0x3f00, 0xbcb1dad9b02a87a8fff0000000000000, 0xfff0000000000000bcb1a2976d646cad, Done(0x7ff0000000000000fff0000000000000, 0x3f00)),
    ("660f7dcd", 0x3f80, 0x000bcbcfe939bcc4fff8000000000000, 0xc3241b5ff2b5e6537ff8000000000001, Done(0x7ff8000000000001fff8000000000000, 0x3f80)),
    ("660f7de4", 0x5f00, 0x43300000000000004014000000000000, 0x43300000000000004014000000000000, Done(0xc32ffffffffffff6c32ffffffffffff6, 0x5f00)),
    ("66450f7dcc", 0x5f00, 0xc321f813a27e14f8c000000000000000, 0x800fffffffffffff7ff0000000000001, Fault(0x5f01)),
    ("660f7dd2", 0x7f00, 0x7ff00000000000007fd0000000000000, 0x7ff00000000000007fd0000000000000, Done(0xfff0000000000000fff0000000000000, 0x7f00)),
    ("66440f7dfc", 0x7f00, 0x7ff610862505f0847ff77a669c84fa41, 0x3cab423dc1a74a993cb73c2606f4abbd, Fault(0x7f01)),
    ("660f7ded", 0x5f80, 0x7ff80000000000010010000000000000, 0x7ff80000000000010010000000000000, Done(0x7ff80000000000017ff8000000000001, 0x5f80)),
    ("660f7dd4", 0x9f00, 0xfff7fffffffffffffff0000000000000, 0xfff8123456789abc3cbf4ca53c9220a5, Fault(0x9f01)),
    ("660f7deb", 0x9f00, 0x433fffffffffffff433fffffffffffff, 0xc0000000000000004328d0612a326507, Done(0x4328d0612a32650b0000000000000000, 0x9f00)),
    ("66450f7de9", 0x1f40, 0x7fdfffffffffffffc54c1beb57fdc2ef, 0x7ff288b1105b00144008000000000000, Fault(0x1f41)),
    ("66440f7df0", 0x1f40, 0x0000000000000003800351133560f99d, 0xfff8000000000000800fffffffffffff, Done(0xfff80000000000008000000000000000, 0x1f40)),
    ("66450f7ddb", 0x7f80, 0x8000000000000000001fffffffffffff, 0x8000000000000000001fffffffffffff, Done(0x001fffffffffffff001fffffffffffff, 0x7f80)),
    ("66410f7df7", 0xff00, 0x4340000000000000433fffffffffffff, 0xfff00000000000007fefffffffffffff, Done(0x7ff0000000000000bff0000000000000, 0xff00)),
    ("660f7dd7", 0x7f40, 0x43300000000000007ff4000000000000, 0x4014000000000000fff8000000000000, Fault(0x7f41)),
    ("66450f7dc9", 0x7f40, 0x000fffffffffffff7fefffffffffffff, 0x000fffffffffffff7fefffffffffffff, Done(0x7fefffffffffffff7fefffffffffffff, 0x7f40)),
    ("66450f7dfc", 0x1fc0, 0x000fffffffffffff4014000000000000, 0xfff8000000000000bcb814017bd93ec3, Done(0xfff80000000000004014000000000000, 0x1fc0)),
    ("66440f7dc4", 0x9f80, 0x40000000000000003ff0000000000001, 0x3cb1f0b3e708d9158000000000000000, Done(0xbcb1f0b3e708d915bfeffffffffffffe, 0x9f80)),
    ("66410f7dc4", 0x80c0, 0xfff80000000000003ff0000000000000, 0x80100000000000008010000000000000, Done(0x0000000000000000fff8000000000000, 0x80c0)),
    ("66410f7de3", 0xffc0, 0x940f472cc8bbd6c2000b870c7581e149, 0x7fe00000000000007fe0000000000000, Done(0x0000000000000000140f472cc8bbd6c2, 0xffc0)),
    ("66410f7dea", 0x0000, 0xfff7dfbdf1bb258c8010000000000000, 0x4014000000000000400180a860c06f4a, Fault(0x0001)),
    ("66450f7dc0", 0x0000, 0x3ca00000000000008000000000000000, 0x3ca00000000000008000000000000000, Done(0xbca0000000000000bca0000000000000, 0x0000)),
    ("f20f7dc7", 0x1f80, 0x7fc00000338000007f4beb837fbfffff, 0x8080000000800000ff80000040400000, Done(0x010000007f8000007fc000007fffffff, 0x1f81)),
    ("f20f7dff", 0x1f80, 0x7fc00001ffdd5b457fc000017fd7610f, 0x7fc00001ffdd5b457fc000017fd7610f, Done(0xffdd5b457fd7610fffdd5b457fd7610f, 0x1f80)),
    ("f2440f7dcf", 0x3f00, 0x7fffffff800000003f800000b5cde5c6, 0x3fc000003f8000007f7fffff7f805bce, Fault(0x3f01)),
    ("f2410f7dd2", 0x3f00, 0x7fffffff749ee1317f43e38f7f800000, 0x3e8000007fffffff7ff6332a00000003, Done(0x7fffffff7ff6332a7fffffff7f800000, 0x3f00)),
    ("f20f7dc9", 0x3f80, 0x8009d139ffc123457ffffffff4e33200, 0x8009d139ffc123457ffffffff4e33200, Done(0xffc123457fffffffffc123457fffffff, 0x3f80)),
    ("f2440f7df9", 0x5f00, 0xf484730833800000cfca14fc3f800001, 0x000000037fa000003400000074c81e24, Fault(0x5f01)),
    ("f20f7de9", 0x5f00, 0xff7fffffff8000000080000000ffffff, 0x00800001ff80000080362932ffc00000, Done(0xff800000ffc00000ff800000007fffff, 0x5f00)),
    ("f2450f7de3", 0x7f00, 0x7fbfffff800c9a3aff39785240400000, 0x4b000000f49a30560000000001000000, Fault(0x7f01)),
    ("f2450f7de4", 0x7f00, 0xbf800000ff80000001000000ffc5a7c4, 0xbf800000ff80000001000000ffc5a7c4, Done(0xff800000ffc5a7c4ff800000ffc5a7c4, 0x7f00)),
    ("f2450f7ddb", 0x5f80, 0xffc123450000000378383e2d7fc00000, 0xffc123450000000378383e2d7fc00000, Done(0xffc123457fc00000ffc123457fc00000, 0x5f80)),
    ("f2450f7dde", 0x9f00, 0x3f000000c0000000bf800000ff800001, 0xc00ff755cfe3f3a380cbee92ff2f11b7, Fault(0x9f01)),
    ("f2440f7df8", 0x9f00, 0x7fc00000000000037f8000007f000000, 0x7fffffff80000001404000003fc00000, Done(0x7fffffffbfc000007fc00000ff800000, 0x9f00)),
    ("f2450f7dfb", 0x1f40, 0x7f80000174f9b5807fa000007f800001, 0x310c44bb4b000000c00000004b800000, Fault(0x1f41)),
    ("f2450f7ddb", 0x1f40, 0x3f8000013f800000807fffff3f800001, 0x3f8000013f800000807fffff3f800001, Done(0xb40000003f800001b40000003f800001, 0x1f40)),
    ("f2440f7dfe", 0x7f80, 0x7fc00001ff7fffffbfd64f12403ae241, 0x40400000c0000000005e2361ffeb7cec, Done(0xc0a00000ffeb7cec7fc00001409304e5, 0x7f80)),
    ("f20f7dc9", 0xff00, 0x3f800000404000007fc0000174b96055, 0x3f800000404000007fc0000174b96055, Done(0x400000007fc00001400000007fc00001, 0xff00)),
    ("f2440f7de8", 0x7f40, 0xb019e277808000003f800001404a9fc6, 0x80800000ffbfffff807fffff6f5617be, Fault(0x7f41)),
    ("f2440f7df0", 0x7f40, 0xff4d7541ff7fffff7fc000007fc00001, 0x7fc00001002c8636ffc000007fffffff, Done(0x7fc000017ffffffffe4a2af87fc00001, 0x7f40)),
    ("f2450f7de4", 0x1fc0, 0x3f7fffff000edea380000001807fffff, 0x3f7fffff000edea380000001807fffff, Done(0xbf7fffff00000000bf7fffff00000000, 0x1fc0)),
    ("f20f7ddd", 0x9f80, 0x3f87f8e140393bda000000017fc92270, 0xbfca9343ffc12345ffc00000016432a8, Done(0xffc12345ffc000003fea7ed37fc92270, 0x9f80)),
    ("f2450f7dff", 0x80c0, 0xc069198b000000017fffffff3f800000, 0xc069198b000000017fffffff3f800000, Done(0x4069198b7fffffff4069198b7fffffff, 0x80c0)),
    ("f20f7df9", 0xffc0, 0x7fc000007fffffff0040000000800000, 0xffc1234580800000807fffff80800000, Done(0xffc12345808000007fffffff00800000, 0xffc0)),
    ("f2440f7dde", 0x0000, 0x4612191f74d91cab7fc0000000800001, 0x7fa00000007fffff3fc00000cfa41a0a, Fault(0x0001)),
    ("f20f7dfe", 0x0000, 0x7fffffffbf80000040a00000ffc12345, 0x01000000008000007fc0000174db2da7, Done(0x808000007fc000017fffffffffc12345, 0x0000)),
    ("66410fd0d7", 0x1ec0, 0x000da8832f516c7efe43b4b0a46412f7, 0x432d8e7eb969ba94ffffb4d5dd11634f, Done(0x432d8e7eb969ba94ffffb4d5dd11634f, 0x1ec0)),
    ("660fd0c1", 0x77c0, 0x800547acdf46ac400002603e7791e3af, 0xbffbaa3b88cc44c200242f3799866327, Done(0xbffbaa3b88cc44c280242f3799866327, 0x77c0)),
    ("66450fd0c4", 0x5f00, 0x000b023218f79d088007b968285bf437, 0x7ffeca3c863b4765fffb575adc4552f3, Done(0x7ffeca3c863b4765fffb575adc4552f3, 0x5f00)),
    ("66450fd0c9", 0x1f00, 0xffdd389f65c111f4bcb3dbbbcb84485f, 0xffdd389f65c111f4bcb3dbbbcb84485f, Done(0xffed389f65c111f40000000000000000, 0x1f00)),
    ("660fd0ed", 0x9d80, 0xc008cddc893eb02fc3265164034d2086, 0xc008cddc893eb02fc3265164034d2086, Done(0xc018cddc893eb02f0000000000000000, 0x9d80)),
    ("66440fd0f7", 0x9f00, 0xfff629458e94de97c321e63d4719232f, 0xbcb434d0accc89013ca556077b9ffde9, Fault(0x9f01)),
    ("660fd0ed", 0x9f80, 0x0029d1a196e9eb27432c019b23937d54, 0x0029d1a196e9eb27432c019b23937d54, Done(0x0039d1a196e9eb270000000000000000, 0x9f80)),
    ("66450fd0c3", 0xdd40, 0x000adb2ce3d3aaf93cbc40765b193654, 0x001a2f6a30ebb8e7bcbfbfaf7076b026, Done(0x001a2f6a30ebb8e73cce0012e5c7f33d, 0xdd40)),
    ("66450fd0e4", 0x80c0, 0xbff0fe0fe8647389800cd5512cc3b331, 0xbff0fe0fe8647389800cd5512cc3b331, Done(0xc000fe0fe86473890000000000000000, 0x80c0)),
    ("660fd0e1", 0x5f00, 0x7ff42aecc90eb0c87fe50f01b2b07fe6, 0x7fe85c0e827d83da3ff80f00207b10c0, Fault(0x5f01)),
    ("660fd0fb", 0x2000, 0xfff0cb03e899b5cbfff65b57e2f47b90, 0x001adbdf24b6c74e7ff72c70d837972f, Fault(0x2001)),
    ("66450fd0ff", 0x1f80, 0x3ff34b581acffc907fd736c1679436f8, 0x3ff34b581acffc907fd736c1679436f8, Done(0x40034b581acffc900000000000000000, 0x1f80)),
    ("66410fd0e8", 0x0040, 0xffdf54ba3f475b51000047daea4b8885, 0xc0000e184dc35ce2fff70f72f07f1571, Fault(0x0041)),
    ("66450fd0e4", 0x5f80, 0xbcbaa3a4ff16b2a2d7ca08366246ce25, 0xbcbaa3a4ff16b2a2d7ca08366246ce25, Done(0xbccaa3a4ff16b2a20000000000000000, 0x5f80)),
    ("66450fd0db", 0x3f00, 0xc32c96dca735826e4fb0925a6f99f88d, 0xc32c96dca735826e4fb0925a6f99f88d, Done(0xc33c96dca735826e8000000000000000, 0x3f00)),
    ("66440fd0e2", 0x1e80, 0x800db9e6f5589d37002d184b7245a73a, 0x80251bebd461e98e00010a356eea4c1d, Fault(0x1e82)),
    ("f2410fd0de", 0x0040, 0x40166ab8cf904b08008d2a50ffb389a2, 0x8086fb0f400273848b62989d8b6d70a9, Fault(0x0041)),
    ("f2450fd0c9", 0x5780, 0xff80f4fe46a722d980c50aecff7c6444, 0xff80f4fe46a722d980c50aecff7c6444, Done(0xffc0f4fe0000000081450aec00000000, 0x5781)),
    ("f20fd0d8", 0x7f40, 0x80457a3030629d1281673d3e401b6fd5, 0x7f93a5113fd7c0457f9eacbdff654dd2, Fault(0x7f41)),
    ("f20fd0f6", 0x3fc2, 0x4f8b039c302e44be74c37515004fb9d6, 0x4f8b039c302e44be74c37515004fb9d6, Done(0x500b039c800000007543751580000000, 0x3fc2)),
    ("f2450fd0e4", 0x9d80, 0x303c7348f4c05459bf9ea81aff678120, 0x303c7348f4c05459bf9ea81aff678120, Done(0x30bc734800000000c01ea81a00000000, 0x9d80)),
    ("f2440fd0f9", 0x7dc0, 0x8070ccf13fba92030043b18e7f875281, 0x4fcb98613fb4010dcff633f18b6fd8ee, Done(0x4fcb98613d521ec0cff633f17fc75281, 0x7dc1)),
    ("f20fd0c0", 0x8042, 0x00b80ccf8b15c2e30b1d09ba00b58dcc, 0x00b80ccf8b15c2e30b1d09ba00b58dcc, Done(0x01380ccf000000000b9d09ba00000000, 0x8042)),
    ("f20fd0e4", 0x5b80, 0x80fa5a9ccfc57f198094a1ba7442cd43, 0x80fa5a9ccfc57f198094a1ba7442cd43, Done(0x817a5a9c000000008114a1ba00000000, 0x5b80)),
    ("f2440fd0cb", 0x7f00, 0x000e836d80abcb9fcff91e7f7f02134f, 0x7fe7e5417f95cdd9407156d7c02c6823, Fault(0x7f01)),
    ("f20fd0e4", 0x37ce, 0x80c0f746ffa1ddccf4ed4654f4fe05c6, 0x80c0f746ffa1ddccf4ed4654f4fe05c6, Done(0x8140f746ffe1ddccf56d465480000000, 0x37cf)),
    ("f2450fd0ed", 0x1fc0, 0x3064d8c300cf6704b03b6ecbffd4b1fa, 0x3064d8c300cf6704b03b6ecbffd4b1fa, Done(0x30e4d8c300000000b0bb6ecbffd4b1fa, 0x1fc0)),
    ("f2450fd0c0", 0x1a00, 0x44f85d6c7f440628b06bb134b027a60f, 0x44f85d6c7f440628b06bb134b027a60f, Done(0x45785d6c00000000b0ebb13400000000, 0x1a00)),
    ("f2450fd0c7", 0x4000, 0x303d4e877005e959407830ff74e6184e, 0x00ccb13b7fb51f61bfd2c58af4a18085, Fault(0x4001)),
    ("f2450fd0c0", 0x1f00, 0x8076c90a0125585f0b023639802fd6f0, 0x8076c90a0125585f0b023639802fd6f0, Done(0x80ed9214000000000b82363900000000, 0x1f02)),
    ("f2450fd0c3", 0x8000, 0x7f82914d7b5ca5a574eea70effbecf60, 0x7f928ecb7f62d9fe7f74183aa686d7e2, Fault(0x8001)),
    ("f2450fd0c2", 0x17c0, 0x8131aeafc046c76f805365db40047560, 0x00dd47d3c01754e6bfa2be44005a3c52, Done(0x8086158bbf3dca24bfa2be4440047560, 0x17c0)),
    ("66450f7cc5", 0x7f40, 0x3ca1b12e1d748e65bcb21ff101a5b08f, 0xffe74860f783dbc8fff0e3a13404400a, Fault(0x7f41)),
    ("66450f7cf0", 0x80c0, 0x7ff9288d3681c21a3cb8d59835bb9cc2, 0x7fffd8dbff2835cb3ff0e9a8175a794d, Done(0x7fffd8dbff2835cb7ff9288d3681c21a, 0x80c0)),
    ("66450f7ce8", 0x7f00, 0x3cabc077d3533b42c327865d4484c65f, 0xfff4d74a3a5daef84fb221a5dc5d46e8, Fault(0x7f01)),
    ("660f7cc1", 0x4000, 0xfffc8100f9dd2f52bff7335ee992915e, 0x459c97d39433f740fff94ce14a1b07e9, Done(0xfff94ce14a1b07e9fffc8100f9dd2f52, 0x4000)),
    ("660f7cde", 0x7bc0, 0x3cbff5b266335749bcbea89cbc63a1da, 0xbffd90f5db60a0334009edfff7d53447, Done(0x3ff64b0a1449c85b3c74d15a9cfb56f0, 0x7bc0)),
    ("660f7cef", 0x1f00, 0x4007554a89b73dab00149b69d864adbf, 0x7ffcf3e19f58a3affff3813696c8e288, Fault(0x1f01)),
    ("66450f7cdb", 0x5f80, 0xbcb030292baaf3297ffdfa71cb7dbf45, 0xbcb030292baaf3297ffdfa71cb7dbf45, Done(0x7ffdfa71cb7dbf457ffdfa71cb7dbf45, 0x5f80)),
    ("66450f7cec", 0x3f80, 0xfff4d527d5902c143ca3646f79567e37, 0x7fef67e1fea43ae5fffc70bf428bdd51, Done(0xfffc70bf428bdd51fffcd527d5902c14, 0x3f81)),
    ("66410f7ced", 0x1f40, 0x7fd675427b479a68002d242ac4ce2169, 0x7ff734f83336572f0015745bc7156722, Fault(0x1f41)),
    ("660f7cd2", 0x3e80, 0xc005ea9d33990193c0062da258458309, 0xc005ea9d33990193c0062da258458309, Done(0xc0160c1fc5ef424ec0160c1fc5ef424e, 0x3e80)),
    ("66450f7cf8", 0x1e80, 0xffff536acd023b8b00215614badad8a3, 0x400631d18c4c7edc3ff1e573d446d630, Done(0x400f248b766fe9f4ffff536acd023b8b, 0x1e80)),
    ("66410f7cd4", 0x9f00, 0xffe9d0ddaa193a7080189578356bc6ca, 0x00240063a568137f7ff7c78c7c5cb4d9, Fault(0x9f01)),
    ("66410f7cfc", 0xff00, 0x802b80453924a4d37ffd694c3b3a6cdd, 0x3ff12a3fc7f3e2c7fff9d860e8f2952f, Done(0xfff9d860e8f2952f7ffd694c3b3a6cdd, 0xff00)),
    ("66410f7cff", 0x1f40, 0x800de5113a040008000514f1e62b2b28, 0x7ffbe2acd159d060ef2e68b956e4eacc, Done(0x7ffbe2acd159d0600000000000000000, 0x1f40)),
    ("66450f7cf2", 0x1fc0, 0xffdb24c3d97133e3ffd6b6abbb15f8b5, 0xbff486ff7effb8e40009720f39c3ae57, Done(0xbff486ff7effb8e4ffe8edb7ca43964c, 0x1fc0)),
    ("660f7cfe", 0x7e80, 0x7ff8ad24118055d8180426275e82fa1e, 0x800010a8d3aab7fc7fff20c3d9853b5e, Done(0x7fff20c3d9853b5e7ff8ad24118055d8, 0x7e80)),
    ("f20f7cf5", 0x1fc0, 0xf4b10e568081fa0ed5b360874003e50c, 0xc00245bb405f5f94306f84fe3ff4d7be, Done(0x3fba33b23ff4d7bef4b10e56d5b36087, 0x1fe0)),
    ("f20f7ccd", 0x3f00, 0x3fee0de1ff9623e680de1a54b07c1bff, 0x8b7a1aa70b3a28d2bfdf758d0b748f3f, Fault(0x3f01)),
    ("f2410f7ccc", 0x7ec0, 0x8025b8716bbca068806a177b4024b4df, 0x017f68f1801456f20061d3ecffe8f35a, Done(0x017f68f1ffe8f35a6bbca0684024b4df, 0x7ec0)),
    ("f2440f7cfc", 0x1f40, 0xffbd113d8124406afff23b1774dfe4dd, 0x8136d93e8029105300b857ad3ffb4bcc, Fault(0x1f41)),
    ("f2450f7ccc", 0xc4d8, 0xf4cb098c001feed663ed746affe78653, 0x7fdd79578825bc7080a81bd07f8db00e, Done(0x7fdd79577fcdb00ef4cb098cffe78653, 0xc4d9)),
    ("f2410f7ccd", 0x9f80, 0xc03e19360096559b4fc5336edfffbc3d, 0xcfaf27adf4d3080fcf94ce080134f87c, Done(0xf4d3080fcf94ce08c03e1936dfffbc3d, 0x9fa0)),
    ("f2410f7cf5", 0x1f80, 0x4032850d7f00c7a47fcbc0603d8cd90d, 0x80dd75d1815be9883facd0f74fcde819, Done(0x81a552384fcde8197f00c7a47fcbc060, 0x1fa0)),
    ("f20f7cd1", 0x3f80, 0x7fd5fddd010dc0bd815b371b4064d29c, 0xb031837ccfe766460092b1f24014e420, Done(0xcfe766474014e4207fd5fddd4064d29b, 0x3fa0)),
    ("f2440f7ce7", 0x3341, 0x303cc61ecffa0f35007272e9bfeef714, 0xffac2a8a7fac900a8107aa8bb0005b37, Fault(0x3341)),
    ("f2410f7cff", 0x7f40, 0xffc848a6ffeec7a9ffa19bda80981397, 0x402e2a274fcf1323ff2bdf62404b69c5, Fault(0x7f41)),
    ("f20f7ce2", 0x5f80, 0x8b0791de8b16fcb000a2b16db04622b8, 0x0b6482c3bff26e4d8083636ae86098fa, Done(0xbff26e4ce86098fa8b8f4747b04622b7, 0x5fa0)),
    ("f2440f7cef", 0xc6db, 0xffd0135700159d973fcbf369c04ff266, 0x8142a8ef005052c5000176a9001be9e2, Done(0x8142a8ef00000000ffd01357bfd3f163, 0xc6db)),
    ("f2410f7ccc", 0x1f00, 0x4fc53633bf8bef74f4872d0da0c9b381, 0xffbba7d63fafe12e0150212f4f922839, Fault(0x1f01)),
    ("f2450f7cdc", 0x7c5c, 0x8b155d0e807233988b67342f7fcedbb1, 0x0177dc0780355b340b0333bf8b3dad23, Done(0x0177dc078a69e5908b155d0e7fcedbb1, 0x7c5c)),
    ("f2440f7ccd", 0x1fc0, 0x806a0d2c8b7892af0005b8864fa186ec, 0x7f1d24040027d759b000024530560c89, Done(0x7f1d24042fac14888b7892af4fa186ec, 0x1fc0)),
    ("f2410f7ce4", 0xef80, 0x003b5c7b009594e03fcdb4f2c005b678, 0x3fc6c309bfe5db16c023c8347fdb63e4, Done(0xbe78c0687fdb63e400d0f15bbef6dff8, 0xef82)),
    ("660f7dfe", 0x8c41, 0x2ef4b3312931ce40fff33a7758e04779, 0xffe84c4139c09f007fd36ff10099f600, Fault(0x8c41)),
    ("66410f7df6", 0x3f00, 0x7fe8621c5209f695802aa620e680798b, 0x432e30db7fc43dd67ff5fcc4a426220f, Fault(0x3f01)),
    ("660f7de0", 0x3b40, 0xffec4a40ffce1d848008e3f030382a63, 0xffe3921fc45a391f0001ebaa34c14be0, Done(0x7fe3921fc45a391f7fec4a40ffce1d84, 0x3b40)),
    ("66440f7dc2", 0x80c0, 0x0007b8ad66cd3415000a81be572946f9, 0x7ff946608fe0487bffdc0217af007674, Done(0x7ff946608fe0487b0000000000000000, 0x80c0)),
    ("660f7dca", 0x0040, 0x3ff45e85da54ca183cbd13c0e80baa4b, 0x7ff7c5d51052cd3b801be9739bc76795, Fault(0x0041)),
    ("66450f7dee", 0xff00, 0x7fd43fc124bbf5cc7fe94b4fc957ea79, 0x7ffec81c4ce7d56100005151b1bb5727, Done(0x7ffec81c4ce7d5617fde56de6df3df26, 0xff00)),
    ("66450f7dc9", 0x0040, 0xfffeeb0a0884e4930012a944d0cfb0cf, 0xfffeeb0a0884e4930012a944d0cfb0cf, Done(0xfffeeb0a0884e493fffeeb0a0884e493, 0x0040)),
    ("66440f7dee", 0x1b80, 0x3cbb4e130b1286a7fffb65e9c5527fb8, 0xbffe55110f7e6fb44004668b0be19e24, Done(0x4011c889c9d06afffffb65e9c5527fb8, 0x1b80)),
    ("66450f7dd2", 0x3f80, 0xfff7acdfabbb969c8020e9117324db11, 0xfff7acdfabbb969c8020e9117324db11, Done(0xffffacdfabbb969cffffacdfabbb969c, 0x3f81)),
    ("660f7de4", 0x7e80, 0x3cbb0292efe9e789bcb9530e850ac91d, 0x3cbb0292efe9e789bcb9530e850ac91d, Done(0xbcca2ad0ba7a5853bcca2ad0ba7a5853, 0x7e80)),
    ("66450f7ddb", 0x3b80, 0x7fdc3c72744d1bacfffdc7d758029a94, 0x7fdc3c72744d1bacfffdc7d758029a94, Done(0xfffdc7d758029a94fffdc7d758029a94, 0x3b80)),
    ("66450f7de8", 0x5f00, 0xfffc7ce6b4c89e43002cf1c0a63affa0, 0x3cafd7c33d6d79613ca866832d5debba, Done(0xbc8dc500403e369cfffc7ce6b4c89e43, 0x5f00)),
    ("66410f7dfd", 0x1f00, 0xfff3dd28275456f9bca773ab291fbf85, 0x80223e22777f806dc322a6ffaeb0b68d, Fault(0x1f01)),
    ("66450f7dff", 0x9d80, 0xfffaa23dd019adcf0003031b74a8e1e5, 0xfffaa23dd019adcf0003031b74a8e1e5, Done(0xfffaa23dd019adcffffaa23dd019adcf, 0x9d80)),
    ("660f7dc2", 0x3641, 0xfff2c4bcd1ad01a7bcade4eca630911a, 0x432341f9f39ea96ca46282e20742f831, Fault(0x3641)),
    ("66450f7ddd", 0x7dc0, 0x800596130b502155000b9cbb687bbc1d, 0x3ff6396aac55ce4880085c309d205810, Done(0xbff6396aac55ce480000000000000000, 0x7dc0)),
    ("f2410f7de0", 0x1fc0, 0x3ff16b5fcfedd45e009375093fb1b755, 0x0192d5c320b578a4b05a5bbfff280a7d, Done(0x20b578a4ff280a7dcfedd45e3fb1b755, 0x1fe0)),
    ("f2440f7df5", 0x4000, 0xf4bafb0b814161907f8dac0db0208b44, 0x80cc70607f87995180cdf5a081216d07, Fault(0x4001)),
    ("f2410f7df6", 0xffc0, 0x402a40328b7015e1ff720544001b5d89, 0x8b05fd519f18b1d0ff2cc0dd012fc13c, Done(0x9f18b1cf7f2cc0ddc02a40327f720544, 0xffe0)),
    ("f20f7de8", 0x1bc0, 0x8027f9b23fe1e8048128259d7fd7f126, 0x3fdbb71a8050ad4b0b0f108d805b9b01, Done(0xbfdbb71a8b0f108d3fe1e8047fd7f126, 0x1bc0)),
    ("f2450f7dc0", 0x4491, 0x300a5409b077e481bfcb4df23f82528c, 0x300a5409b077e481bfcb4df23f82528c, Done(0xb0c11c454026d03fb0c11c454026d03f, 0x4491)),
    ("f2450f7dd0", 0x7f80, 0x401a9cdd7f24bb26ff5550a3305d62be, 0xf4938fc540347770c048710874c24aa8, Done(0x74938fc574c24aa87f24bb257f5550a3, 0x7fa0)),
    ("f2450f7df1", 0x0040, 0x80e05a08f4f132f47fb192f07fb4fd20, 0x5a02a899816e8a3b81186b54c07e0fbe, Fault(0x0041)),
    ("f20f7dc6", 0xff00, 0x7faa44b67f72cc8d4fd67f0f7f7e8682, 0xb0441dd27f14b9f1ff75d3c67f2876e7, Fault(0xff01)),
    ("f2450f7dcd", 0x5f80, 0x3fa43ebac04ebd49808cf9de3f815645, 0x80b7c1bf8b259ce5401491e4b00473aa, Done(0x8b259cdfc01491e4c0906e533f815646, 0x5fa0)),
    ("f2440f7ded", 0x1e80, 0xed186596bfe328fd3fd9eaf30b222e84, 0x802b7c2ab006a8be4018a542004175cf, Fault(0x1e82)),
    ("f20f7ded", 0x3f80, 0x4ff36eba3fbf41d0cfef3744f48b9495, 0x4ff36eba3fbf41d0cfef3744f48b9495, Done(0xcff36ebaf48b9495cff36ebaf48b9495, 0x3fa0)),
    ("f20f7ded", 0x1dc0, 0x8b200384805f20447fa6c34500e70033, 0x8b200384805f20447fa6c34500e70033, Done(0x0b2003847fe6c3450b2003847fe6c345, 0x1dc1)),
    ("f2450f7ddb", 0x1f80, 0xf4e0fa52400564e8ff7c571c00c5954b, 0xf4e0fa52400564e8ff7c571c00c5954b, Done(0x74e0fa527f7c571c74e0fa527f7c571c, 0x1fa0)),
    ("f2410f7dc5", 0x7152, 0x00204c2cc00e3ff800366498ff7b33e0, 0x803be293b04aae638049780e74865b92, Done(0xb04aae6374865b92c00e3ff8ff7b33e0, 0x7152)),
    ("f2410f7de2", 0x1700, 0xf49bb3217f836ed974aace7f811907ab, 0x1828c65180b89a767fc209f47496ce9b, Fault(0x1701)),
    ("f2410f7dd8", 0x7ec0, 0x7fedcc58815645f20b32022800708c48, 0xf4c4d4c2f49c573a4f89b5427ff2615b, Done(0x73a1f6207ff2615b7fedcc588b320228, 0x7ec0)),
    ("660fd0ee", 0x1e80, 0x3ff00000000000007ff0000000000000, 0x00000000000000017fdfffffffffffff, Fault(0x1e82)),
    ("660fd0ee", 0x1e80, 0xfff7fffffffffffffff7ffffffffffff, 0xfff8000000000000bcaa08ce20de45a7, Done(0xffffffffffffffffffffffffffffffff, 0x1e81)),
    ("660fd0ee", 0x7e80, 0x000fffffffffffff7ff7ffffffffffff, 0x7ff7ffffffffffff7fe0000000000000, Done(0x7fffffffffffffff7fffffffffffffff, 0x7e81)),
    ("660fd0ee", 0x7e80, 0x3ff00000000000008010000000000000, 0x80000000000000010010000000000001, Fault(0x7e82)),
    ("660fd0ee", 0x7d80, 0x7ff0000000000001000fffffffffffff, 0xfff00000000000010000000000000003, Done(0x7ff8000000000001000ffffffffffffc, 0x7d83)),
    ("660fd0ee", 0x1b80, 0x7ff40000000000000008000000000000, 0x0010000000000000001f4600184b7d60, Done(0x7ffc00000000000080174600184b7d60, 0x1b83)),
    ("660fd0ee", 0x1f00, 0x7fdffffffffffffffff0000000000000, 0x7fdfffffffffffff7ff0000000000000, Done(0x7feffffffffffffffff0000000000000, 0x1f00)),
    ("660fd0ee", 0x1f00, 0x000fffffffffffff3cb0000000000000, 0x0000000000000001fff0000000000000, Done(0x00100000000000007ff0000000000000, 0x1f02)),
    ("660fd0ee", 0x1f00, 0x40000000000000004340000000000000, 0xfff00000000000014340000000000000, Fault(0x1f01)),
    ("660fd0ee", 0x7f00, 0x7ff40000000000007ff4000000000000, 0xfff8123456789abc001a0250d36efbc9, Fault(0x7f01)),
    ("660fd0ee", 0x7f00, 0x000fd0e36affa9f93ca0000000000000, 0x8000000000000000bcafa00d81f3a6ea, Done(0x000fd0e36affa9f93cb7d006c0f9d375, 0x7f02)),
    ("660fd0ee", 0x7f00, 0x7fffffffffffffff3cad60bca432aa9f, 0xffe5474ed655a8e93ca0000000000000, Done(0x7fffffffffffffff3c9ac1794865553e, 0x7f00)),
    ("f20fd0ee", 0x1e80, 0x000000003f8000004b7fffff007fffff, 0x40400000007fffff00000001815c2522, Fault(0x1e82)),
    ("f20fd0ee", 0x1e80, 0x7fbfffff7fc000013f9cb2d77fc00001, 0x8000000140000000800000007f100e08, Done(0x7fffffff7fc000013f9cb2d77fc00001, 0x1e81)),
    ("f20fd0ee", 0x7e80, 0x00000001007fffff81308d73ff7fffff, 0x008000010040000074d8d7e10055448a, Fault(0x7e82)),
    ("f20fd0ee", 0x7e80, 0x007fffff403c76543fc00000ff800000, 0x7fc00001000000007fa6c20cff800000, Done(0x7fc00001403c76547fe6c20cffc00000, 0x7e81)),
    ("f20fd0ee", 0x1d80, 0x0040000000ffffffff80000033800000, 0x7f800001000000013e8000007fc00001, Done(0x7fc0000100fffffeff8000007fc00001, 0x1d83)),
    ("f20fd0ee", 0x7d80, 0x8000000080000001b03408957fbbec42, 0x807fffff80800000ffc000003f7fffff, Done(0x807fffff007fffffffc000007ffbec42, 0x7d83)),
    ("f20fd0ee", 0x7b80, 0x3f7fffff82acb5367fffffff0071fe69, 0x3f7fffff00800000811b6d2cffc12345, Done(0x3fffffff82b4b5367fffffffffc12345, 0x7b80)),
    ("f20fd0ee", 0x1f00, 0xbf83119f400000007effffff7fffffff, 0x7fc0000181083db67f8000013fb8bcba, Fault(0x1f01)),
    ("f20fd0ee", 0x1f00, 0x8000000080854c07ffc1234540a00000, 0x8069f37e807fffff807df6457fffffff, Done(0x8069f37e80054c08ffc123457fffffff, 0x1f02)),
    ("f20fd0ee", 0x1f00, 0x40a0000000800000cf97fb1b7fc00001, 0x400000007fc000008000000001000000, Done(0x40e000007fc00000cf97fb1b7fc00001, 0x1f00)),
    ("f20fd0ee", 0x7f00, 0x3f0000007fa000007fbfffff00800001, 0xb00fe10f7f0000007f7fffff7fe1238c, Fault(0x7f01)),
    ("f20fd0ee", 0x7f00, 0x8011a40080caad0f7e80000001000000, 0x00000003ffc123457fffffff00000003, Done(0x8011a3fdffc123457fffffff00fffffd, 0x7f02)),
    ("660f7cee", 0x1e80, 0x40140000000000003ca0000000000000, 0x000fffffffffffff3ff0000000000001, Fault(0x1e82)),
    ("660f7cee", 0x1e80, 0x80100000000000007fff95e2b2cf54df, 0x3fefffffffffffff7ff0000000000001, Done(0x7ff80000000000017fff95e2b2cf54df, 0x1e81)),
    ("660f7cee", 0x7e80, 0xfff00000000000017ff8000000000000, 0x4330000000000000fff0000000000000, Done(0xfff00000000000007ff8000000000000, 0x7e81)),
    ("660f7cee", 0x7e80, 0xbff817de5d08b539433fffffffffffff, 0x800da7314df1c3f83cb0000000000000, Fault(0x7e82)),
    ("660f7cee", 0x7d80, 0x8004d5af187f3b2d001c1fad5d163379, 0x7ff7ffffffffffff4008000000000000, Done(0x7fffffffffffffff001749fe4496f84c, 0x7d83)),
    ("660f7cee", 0x1b80, 0x7ff40000000000000010000000000001, 0x00080000000000000000000000000003, Done(0x00080000000000037ffc000000000000, 0x1b83)),
    ("660f7cee", 0x1f00, 0x7ff8000000000001fff334df8d98cee0, 0x7ffffffffffffffffff8000000000000, Fault(0x1f01)),
    ("660f7cee", 0x1f00, 0x00000000000000004000f307ede40f37, 0x80100000000000000000000000000003, Done(0x800ffffffffffffd4000f307ede40f37, 0x1f02)),
    ("660f7cee", 0x1f00, 0x7ff00000000000007ff8000000000001, 0x7ff00000000000003ff0000000000000, Done(0x7ff00000000000007ff8000000000001, 0x1f00)),
    ("660f7cee", 0x7f00, 0xfff00000000000007fffb049692076fb, 0x00000000000000004014000000000000, Done(0x40140000000000007fffb049692076fb, 0x7f00)),
    ("660f7cee", 0x7f00, 0x7fffffffffffffff4008000000000000, 0x800efed3d704c330000fffffffffffff, Done(0x0001012c28fb3ccf7fffffffffffffff, 0x7f02)),
    ("660f7cee", 0x7f00, 0xfff8000000000000fff0000000000001, 0x7ff80000000000017ff0000000000000, Fault(0x7f01)),
    ("f20f7cee", 0x1e80, 0xf4dd92567fc000000111e52b7fa00000, 0x00000001ffc12345405304ad7f800000, Done(0xffc123457f8000007fc000007fe00000, 0x1e81)),
    ("f20f7cee", 0x1e80, 0x338000008005787e0000000040a00000, 0x004000007fc00000001d80a33f7fffff, Fault(0x1e82)),
    ("f20f7cee", 0x7e80, 0x7fc000017fffffff00ffffffffc12345, 0x338000003f7fffff7effffffffc12345, Done(0x3f800000ffc123457fffffffffc12345, 0x7e80)),
    ("f20f7cee", 0x7e80, 0x004000003f8000010080000033800000, 0x7fc0000000000003008000007e800000, Fault(0x7e82)),
    ("f20f7cee", 0x7e80, 0xff800000808000007fa0000000800000, 0x7fa000000b71f0b180399d5e7fffffff, Done(0x7fe000007fffffffff8000007fe00000, 0x7e81)),
    ("f20f7cee", 0x1d80, 0x0000000100000000004000008164d4fd, 0x7f8000010000000334000000ffd3ddd3, Done(0x7fc00001ffd3ddd3000000018144d4fd, 0x1d83)),
    ("f20f7cee", 0x7d80, 0x00800001008000017f8000013e800000, 0x0000000300000003338000007fc00000, Done(0x000000067fc00000010000017fc00001, 0x7d83)),
    ("f20f7cee", 0x1b80, 0x0080000000000001ffc00000cfbe25ce, 0x4b800000ffc000003f800000ffc00000, Done(0xffc00000ffc0000000800001ffc00000, 0x1b82)),
    ("f20f7cee", 0x1f00, 0x404000003e8000003f0000004b000000, 0xff8000018074978800000003ffc12345, Fault(0x1f01)),
    ("f20f7cee", 0x1f00, 0x7effffff3f80000000ffffff01000000, 0x3fc00000ffe96194ff80000034000000, Done(0xffe96194ff8000007effffff01800000, 0x1f20)),
    ("f20f7cee", 0x1f00, 0xff80000040400000ff8000007f000000, 0x7f8000004b00000040a00000bf800000, Done(0x7f80000040800000ff800000ff800000, 0x1f00)),
    ("f20f7cee", 0x7f00, 0xff800000ffc000007effffff7f800000, 0x00ffffff40a000007f8000018164a938, Fault(0x7f01)),
    ("660f7dee", 0x1e80, 0x7fffffffffffffff7ff4000000000000, 0x7fd38292ea61e53ffff8123456789abc, Done(0xfff8123456789abc7ffc000000000000, 0x1e81)),
    ("660f7dee", 0x1e80, 0xfffb46e67369908a7fdfffffffffffff, 0x00000000000000000004c409c725d8b0, Fault(0x1e82)),
    ("660f7dee", 0x7e80, 0xfff00000000000007ff4000000000000, 0xfff80000000000007fe3cda84493d5e9, Done(0xfff80000000000007ffc000000000000, 0x7e81)),
    ("660f7dee", 0x7e80, 0x43400000000000000000000000000001, 0x3ff80000000000003fe0000000000000, Fault(0x7e82)),
    ("660f7dee", 0x1d80, 0x43400000000000007ff4000000000000, 0x80000000000000008000000000000001, Done(0x80000000000000017ffc000000000000, 0x1d83)),
    ("660f7dee", 0x7b80, 0x7ff00000000000000000000000000003, 0x3ff00000000000007ff0000000000000, Done(0x7ff0000000000000fff0000000000000, 0x7b82)),
    ("660f7dee", 0x1f00, 0x40140000000000004014000000000000, 0x00200000000000003ca0000000000000, Done(0x3ca00000000000000000000000000000, 0x1f20)),
    ("660f7dee", 0x1f00, 0x00100000000000013ff0000000000000, 0x80000000000000017ff428810828906d, Fault(0x1f01)),
    ("660f7dee", 0x1f00, 0x7fffffffffffffff4014000000000000, 0x000dbbde51791d238008f8afcfe9a5f3, Done(0x8016b48e2162c3167fffffffffffffff, 0x1f02)),
    ("660f7dee", 0x1f00, 0x00000000000000004014000000000000, 0x3ca0000000000000bca50ffd93112d86, Done(0xbcb287fec98896c34014000000000000, 0x1f00)),
    ("660f7dee", 0x7f00, 0x001ffffffffffffffff8123456789abc, 0x4330000000000000fff0000000000000, Done(0xfff0000000000000fff8123456789abc, 0x7f00)),
    ("660f7dee", 0x7f00, 0x7ff40000000000004008000000000000, 0x3fd00000000000007ff8000000000000, Fault(0x7f01)),
    ("f20f7dee", 0x1e80, 0x7f000000ffc00000ffbfffff7effffff, 0x013cd5c500000000c34101787fc00000, Done(0x813cd5c57fc00000ffc00000ffffffff, 0x1e81)),
    ("f20f7dee", 0x1e80, 0x80000000c00000007fc0000080000000, 0xbf80000080000000ff80000000800000, Done(0x3f8000007f800000c00000007fc00000, 0x1e80)),
    ("f20f7dee", 0x1e80, 0x338000007e800000803898ad7fc00000, 0x80f34896807fffff80000001c06918bb, Fault(0x1e82)),
    ("f20f7dee", 0x7e80, 0x00800000007fffff400000003f000000, 0x00400000008000013400000000ffffff, Fault(0x7e82)),
    ("f20f7dee", 0x7e80, 0x004000007fc000017fffffff7f2cf8e8, 0x7f8000017f0000003e8000007fee1310, Done(0x7fc000017fee13107fc000017fffffff, 0x7e81)),
    ("f20f7dee", 0x1b80, 0x7f80000000000003ffbfffff4b800000, 0x7effffff7fc000017f8000017fc00001, Done(0x7fc000017fc00001ff800000ffffffff, 0x1b83)),
    ("f20f7dee", 0x7b80, 0xf5a4688affbfffff0000000080249c0a, 0xff800000008000017f80000000400000, Done(0x7f800000ff800000ffffffff80249c0a, 0x7b83)),
    ("f20f7dee", 0x1f00, 0xffbfffff40a0000040265f2a7fc00000, 0x01000000307c275effa7aec700800001, Fault(0x1f01)),
    ("f20f7dee", 0x1f00, 0x800000007fffffffcfa9a0594b000000, 0x4b7fffffff800000800000000012e2a0, Done(0xff8000000012e2a07fffffff4fa9e059, 0x1f02)),
    ("f20f7dee", 0x7f00, 0xffc0000000400000bf80000040400000, 0x7fc000017fc00000ff8000007effffff, Done(0x7fc000007f800000ffc0000040800000, 0x7f00)),
    ("f20f7dee", 0x7f00, 0x7f6fdec33f0000007e8000004b7fffff, 0x00400000ffbfffff3fc0000033800000, Fault(0x7f01)),
    ("f20f7dee", 0x7f00, 0x7f80000074aa1b32ff7fffffff7fffff, 0x74e718da7f800000007fffff80745a9f, Done(0x7f80000080f45a9eff80000000000000, 0x7f02)),
];

fn golden_expectation(
    code: &[u8],
    mxcsr: u32,
    a: u128,
    b: u128,
    gold: Gold,
    privilege: PrivilegeLevel,
) {
    let (dst, src) = registers_of(code);
    let observed = run_code(code, (dst, src), mxcsr, (a, b), privilege);
    let label = format!("{code:02x?} mxcsr {mxcsr:#06x} a {a:#034x} b {b:#034x}");
    match gold {
        Done(value, flags) => {
            assert_eq!(
                observed.exit,
                RunExit::InstructionBudgetExhausted,
                "{label}"
            );
            let mut expected = observed.before.clone();
            expected.rip += observed.length as u64;
            set_xmm(&mut expected, usize::from(dst), value);
            expected.sse.as_mut().unwrap().mxcsr = flags;
            assert_eq!(observed.after, expected, "{label}");
        }
        Fault(flags) => {
            assert_eq!(observed.exit, xm(), "{label}");
            let mut expected = observed.before.clone();
            expected.sse.as_mut().unwrap().mxcsr = flags;
            assert_eq!(observed.after, expected, "{label}");
        }
    }
}
#[test]
fn golden_vectors_from_the_physical_oracle_replay_exactly() {
    assert!(GOLDEN.len() > 300);
    for &(hex, mxcsr, a, b, gold) in GOLDEN {
        let code = from_hex(hex);
        golden_expectation(&code, mxcsr, a, b, gold, PrivilegeLevel::Ring3);
        golden_expectation(&code, mxcsr, a, b, gold, PrivilegeLevel::Ring0);
    }
}
#[test]
fn the_golden_table_covers_every_form_every_outcome_and_the_unmasked_faults() {
    let mut forms = std::collections::BTreeSet::new();
    let (mut done, mut faults, mut flagged) = (0, 0, 0);
    for &(hex, _, _, _, gold) in GOLDEN {
        let code = from_hex(hex);
        forms.insert((code[0], code[code.len() - 2]));
        match gold {
            Done(_, flags) => {
                done += 1;
                flagged += usize::from(flags & 0x3f != 0);
            }
            Fault(_) => faults += 1,
        }
    }
    assert_eq!(forms.len(), 6);
    assert!(
        done > 200 && faults > 20 && flagged > 60,
        "{done} {faults} {flagged}"
    );
}
#[test]
fn documented_element_order_on_exact_binary32_and_binary64_values() {
    // dest = [1, 2, 3, 4], src = [10, 20, 30, 40] (element 0 first).
    let (d32, s32) = (f32s([1.0, 2.0, 3.0, 4.0]), f32s([10.0, 20.0, 30.0, 40.0]));
    let expected32 = [
        (Kind::HAdd, f32s([3.0, 7.0, 30.0, 70.0])),
        (Kind::HSub, f32s([-1.0, -1.0, -10.0, -10.0])),
        (Kind::AddSub, f32s([-9.0, 22.0, -27.0, 44.0])),
    ];
    for (kind, expected) in expected32 {
        assert_eq!(
            run_reg(kind, Shape::Packed32, MXCSR, d32, s32),
            (expected, MXCSR),
            "{}",
            name(kind, Shape::Packed32)
        );
    }
    let (d64, s64) = (f64s([1.0, 2.0]), f64s([10.0, 20.0]));
    let expected64 = [
        (Kind::HAdd, f64s([3.0, 30.0])),
        (Kind::HSub, f64s([-1.0, -10.0])),
        (Kind::AddSub, f64s([-9.0, 22.0])),
    ];
    for (kind, expected) in expected64 {
        assert_eq!(
            run_reg(kind, Shape::Packed64, MXCSR, d64, s64),
            (expected, MXCSR),
            "{}",
            name(kind, Shape::Packed64)
        );
    }
    // The destination is the first source: with dst == src the pairs of the destination fill both halves.
    let code = reg_code(Kind::HAdd, Shape::Packed32, 1, 1, false);
    let observed = run_code(&code, (1, 1), MXCSR, (d32, 0), PrivilegeLevel::Ring0);
    assert_eq!(xmm(&observed.after, 1), f32s([3.0, 7.0, 3.0, 7.0]));
    let code = reg_code(Kind::HSub, Shape::Packed64, 1, 1, false);
    let observed = run_code(&code, (1, 1), MXCSR, (d64, 0), PrivilegeLevel::Ring0);
    assert_eq!(xmm(&observed.after, 1), f64s([-1.0, -1.0]));
}
#[test]
fn every_register_pair_of_every_form_computes_the_same_lanes() {
    for kind in KINDS {
        for shape in SHAPES {
            for dst in 0..16u8 {
                for src in 0..16u8 {
                    let a32 = f32s([1.0, 2.0, 4.0, 8.0].map(|v| v * f32::from(dst + 1)));
                    let b32 = f32s([16.0, 32.0, 64.0, 128.0].map(|v| v * f32::from(src + 1)));
                    let a64 = f64s([1.0, 2.0].map(|v| v * f64::from(dst + 1)));
                    let b64 = f64s([16.0, 32.0].map(|v| v * f64::from(src + 1)));
                    let (a, b) = if shape == Shape::Packed32 {
                        (a32, b32)
                    } else {
                        (a64, b64)
                    };
                    let code = reg_code(kind, shape, dst, src, false);
                    let observed =
                        run_code(&code, (dst, src), MXCSR, (a, b), PrivilegeLevel::Ring0);
                    let b = if dst == src { a } else { b };
                    let (expected, _) = run_reg(kind, shape, MXCSR, a, b);
                    assert_eq!(observed.exit, RunExit::InstructionBudgetExhausted);
                    assert_eq!(
                        xmm(&observed.after, usize::from(dst)),
                        expected,
                        "{} xmm{dst}, xmm{src}",
                        name(kind, shape)
                    );
                    // Nothing but the destination and the PC changed.
                    let mut unchanged = observed.after.clone();
                    unchanged.rip = observed.before.rip;
                    set_xmm(
                        &mut unchanged,
                        usize::from(dst),
                        xmm(&observed.before, usize::from(dst)),
                    );
                    assert_eq!(unchanged, observed.before);
                }
            }
        }
    }
}
/// A reference for the three forms on exact binary32 values (integers below 2^20: no rounding, no flags).
fn reference32(kind: Kind, a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    match kind {
        Kind::AddSub => [a[0] - b[0], a[1] + b[1], a[2] - b[2], a[3] + b[3]],
        Kind::HAdd => [a[0] + a[1], a[2] + a[3], b[0] + b[1], b[2] + b[3]],
        _ => [a[0] - a[1], a[2] - a[3], b[0] - b[1], b[2] - b[3]],
    }
}
fn reference64(kind: Kind, a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    match kind {
        Kind::AddSub => [a[0] - b[0], a[1] + b[1]],
        Kind::HAdd => [a[0] + a[1], b[0] + b[1]],
        _ => [a[0] - a[1], b[0] - b[1]],
    }
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}
#[test]
fn round_to_nearest_even_matches_the_host_arithmetic_on_random_operands() {
    let mut rng = Rng(0x54_f0);
    // Normal operands with moderate exponents: no overflow, no underflow, no NaN, no denormal.
    let f32_operand = |rng: &mut Rng| {
        let exponent = 0x60 + (rng.next() % 0x40) as u32;
        f32::from_bits(
            ((rng.next() as u32) & 0x8000_0000) | exponent << 23 | (rng.next() as u32 & 0x7f_ffff),
        )
    };
    let f64_operand = |rng: &mut Rng| {
        let exponent = 0x380 + rng.next() % 0x80;
        f64::from_bits((rng.next() & 1 << 63) | exponent << 52 | (rng.next() & 0xf_ffff_ffff_ffff))
    };
    for kind in KINDS {
        for _ in 0..2000 {
            let a = [(); 4].map(|()| f32_operand(&mut rng));
            let b = [(); 4].map(|()| f32_operand(&mut rng));
            let (value, mxcsr) = run_reg(kind, Shape::Packed32, MXCSR, f32s(a), f32s(b));
            let expected = reference32(kind, a, b);
            assert_eq!(
                value,
                f32s(expected),
                "{} {a:?} {b:?}",
                name(kind, Shape::Packed32)
            );
            assert_eq!(mxcsr & !0x20, MXCSR, "only the precision flag may be set");
            let a = [(); 2].map(|()| f64_operand(&mut rng));
            let b = [(); 2].map(|()| f64_operand(&mut rng));
            let (value, mxcsr) = run_reg(kind, Shape::Packed64, MXCSR, f64s(a), f64s(b));
            assert_eq!(
                value,
                f64s(reference64(kind, a, b)),
                "{} {a:?} {b:?}",
                name(kind, Shape::Packed64)
            );
            assert_eq!(mxcsr & !0x20, MXCSR, "only the precision flag may be set");
        }
    }
}
#[test]
fn the_low_element_of_a_horizontal_pair_is_the_first_operand_for_nan_selection() {
    // Hardware (not the SDM pseudocode text, which writes the sum high + low): when both elements of a pair
    // are quiet NaNs the LOW element wins; a signaling NaN is quieted (and sets IE); a lone NaN wins over a
    // number.
    let (q1, q2) = (0x7fc0_0001u32, 0x7fc0_0002u32);
    let d = lanes32([q1, q2, 0x3f80_0000, q2]);
    let s = lanes32([q2, q1, 0x7fa0_0000, q1]);
    for kind in [Kind::HAdd, Kind::HSub] {
        let (value, mxcsr) = run_reg(kind, Shape::Packed32, MXCSR, d, s);
        // Pair 0 of the destination: q1 (low) wins; pair 1: 1.0 and q2 -> q2; pair 0 of the source: q2 low
        // vs q1 -> q2; pair 1 of the source: the signaling NaN 0x7fa00000 (low) is first -> quieted.
        assert_eq!(
            value,
            lanes32([q1, q2, q2, 0x7fa0_0000 | 0x0040_0000]),
            "{kind:?}"
        );
        assert_eq!(mxcsr, MXCSR | 1, "{kind:?}: IE for the signaling NaN");
    }
    let (q1, q2) = (0x7ff8_0000_0000_0001u64, 0x7ff8_0000_0000_0002u64);
    for kind in [Kind::HAdd, Kind::HSub] {
        let (value, _) = run_reg(
            kind,
            Shape::Packed64,
            MXCSR,
            lanes64([q1, q2]),
            lanes64([q2, q1]),
        );
        assert_eq!(value, lanes64([q1, q2]), "{kind:?}");
    }
    // ADDSUB: the destination element is the first operand.
    let (n1, n2) = (0x7fc0_0001u32, 0x7fc0_0002u32);
    let (value, _) = run_reg(
        Kind::AddSub,
        Shape::Packed32,
        MXCSR,
        lanes32([n1, 0x3f80_0000, n1, n2]),
        lanes32([n2, n2, 0x3f80_0000, n1]),
    );
    assert_eq!(value, lanes32([n1, n2, n1, n2]));
}
#[test]
fn invalid_operations_in_each_form_return_the_default_nan_and_set_ie() {
    let inf = f32::INFINITY;
    // HADD: +inf + -inf; HSUB: +inf - +inf; ADDSUB: element 0 subtracts (inf - inf), element 1 adds (inf + inf).
    let default32 = 0xffc0_0000u32;
    let three = 3.0f32.to_bits();
    let (value, mxcsr) = run_reg(
        Kind::HAdd,
        Shape::Packed32,
        MXCSR,
        f32s([inf, -inf, 1.0, 2.0]),
        f32s([1.0, 2.0, -inf, inf]),
    );
    assert_eq!(value, lanes32([default32, three, three, default32]));
    assert_eq!(mxcsr, MXCSR | 1);
    let (value, mxcsr) = run_reg(
        Kind::HSub,
        Shape::Packed32,
        MXCSR,
        f32s([inf, inf, 5.0, 3.0]),
        f32s([inf, -inf, 1.0, inf]),
    );
    assert_eq!(
        value,
        lanes32([default32, 2.0f32.to_bits(), inf.to_bits(), (-inf).to_bits()])
    );
    assert_eq!(mxcsr, MXCSR | 1);
    let (value, mxcsr) = run_reg(
        Kind::AddSub,
        Shape::Packed64,
        MXCSR,
        f64s([f64::INFINITY, f64::INFINITY]),
        f64s([f64::INFINITY, f64::NEG_INFINITY]),
    );
    assert_eq!(
        value,
        lanes64([0xfff8_0000_0000_0000, 0xfff8_0000_0000_0000])
    );
    assert_eq!(mxcsr, MXCSR | 1);
    let (value, mxcsr) = run_reg(
        Kind::AddSub,
        Shape::Packed64,
        MXCSR,
        f64s([f64::INFINITY, f64::INFINITY]),
        f64s([f64::NEG_INFINITY, f64::INFINITY]),
    );
    assert_eq!(value, f64s([f64::INFINITY, f64::INFINITY]));
    assert_eq!(mxcsr, MXCSR);
}
#[test]
fn an_unmasked_invalid_operation_raises_xm_or_ud_and_leaves_the_destination() {
    // HSUBPD: the destination pair inf - inf is invalid; IM = 0 makes it fault.
    let (a, b) = (f64s([f64::INFINITY, f64::INFINITY]), f64s([1.0, 2.0]));
    let unmasked = MXCSR & !0x80;
    let code = reg_code(Kind::HSub, Shape::Packed64, 1, 2, false);
    for (cr4, vector) in [(FP_INIT_CR4, 19u8), (FP_INIT_CR4 & !CR4_OSXMMEXCPT, 6)] {
        let mut cpu = fp_init_cpu(CR0_BASE, cr4);
        cpu.sse.as_mut().unwrap().mxcsr = unmasked;
        set_xmm(&mut cpu, 1, a);
        set_xmm(&mut cpu, 2, b);
        let mut ram = Ram::new(&code);
        let mut expected = cpu.clone();
        expected.sse.as_mut().unwrap().mxcsr = unmasked | 1;
        let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: None,
                linear_address: None
            })
        );
        assert_eq!(after, expected, "vector {vector}");
    }
    // The same operation with IM = 1 delivers the default NaN and sets IE.
    let (value, mxcsr) = run_reg(Kind::HSub, Shape::Packed64, MXCSR, a, b);
    assert_eq!(value, lanes64([0xfff8_0000_0000_0000, (-1.0f64).to_bits()]));
    assert_eq!(mxcsr, MXCSR | 1);
}
#[test]
fn daz_flush_denormal_operand_and_rounding_control_apply_to_every_form() {
    let tiny = f32::from_bits(1); // the smallest denormal
    let dm = 0x2; // DE
    // Denormal operand: DE is set, the result is computed with the denormal value (1.0 + 2^-149 rounds to 1.0).
    let (value, mxcsr) = run_reg(
        Kind::HAdd,
        Shape::Packed32,
        MXCSR,
        f32s([1.0, tiny, tiny, tiny]),
        f32s([0.0; 4]),
    );
    assert_eq!(value, f32s([1.0, tiny + tiny, 0.0, 0.0]));
    assert_eq!(mxcsr, MXCSR | dm | 0x20);
    // DAZ: the denormals are zeros; DE stays clear and the sums are exact.
    let (value, mxcsr) = run_reg(
        Kind::HAdd,
        Shape::Packed32,
        MXCSR | 0x40,
        f32s([1.0, tiny, tiny, tiny]),
        f32s([0.0; 4]),
    );
    assert_eq!(value, f32s([1.0, 0.0, 0.0, 0.0]));
    assert_eq!(mxcsr, MXCSR | 0x40);
    // FTZ: a tiny nonzero result (smallest normal minus a denormal) becomes zero with UE and PE.
    let min_normal = f32::MIN_POSITIVE;
    let (value, mxcsr) = run_reg(
        Kind::AddSub,
        Shape::Packed32,
        MXCSR | 0x8000,
        f32s([min_normal, 0.0, 0.0, 0.0]),
        f32s([tiny, 0.0, 0.0, 0.0]),
    );
    assert_eq!(value, f32s([0.0, 0.0, 0.0, 0.0]));
    assert_eq!(mxcsr, MXCSR | 0x8000 | 0x10 | 0x20 | dm);
    // Rounding control: 1 + 2^-24 is a tie at binary32 precision.
    let epsilon = f32::from_bits(0x3380_0000); // 2^-24
    for (rc, expected) in [
        (0u32, 1.0f32),
        (1, 1.0),
        (2, f32::from_bits(0x3f80_0001)),
        (3, 1.0),
    ] {
        let (value, mxcsr) = run_reg(
            Kind::HAdd,
            Shape::Packed32,
            MXCSR | rc << 13,
            f32s([1.0, epsilon, 0.0, 0.0]),
            f32s([0.0; 4]),
        );
        assert_eq!(value, f32s([expected, 0.0, 0.0, 0.0]), "rc {rc}");
        assert_eq!(mxcsr, MXCSR | rc << 13 | 0x20, "rc {rc}");
    }
}

#[test]
fn every_addressing_form_reads_the_operand_once_and_matches_the_register_form() {
    for kind in KINDS {
        for shape in SHAPES {
            let (a, b) = if shape == Shape::Packed32 {
                (
                    f32s([1.5, -2.25, 3.0, 4.75]),
                    f32s([10.0, 0.5, -30.0, 40.0]),
                )
            } else {
                (f64s([1.5, -2.25]), f64s([10.0, 0.5]))
            };
            let (expected, expected_mxcsr) = run_reg(kind, shape, MXCSR, a, b);
            for form in &FORMS {
                for dst in [0u8, 7, 9] {
                    let code = mem_code(kind, shape, form, dst);
                    let mut cpu = mem_cpu(form, DATA + 0x40, CR0_BASE, FP_INIT_CR4);
                    set_xmm(&mut cpu, usize::from(dst), a);
                    let before = cpu.clone();
                    let mut ram = Ram::new(&code);
                    ram.put(DATA + 0x40, &b.to_le_bytes());
                    let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let label = format!("{} {} xmm{dst}", name(kind, shape), form.label);
                    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{label}");
                    assert_eq!(ram.reads, vec![(DATA + 0x40, 16)], "{label}");
                    assert!(ram.writes.is_empty(), "{label}");
                    assert_eq!(xmm(&after, usize::from(dst)), expected, "{label}");
                    assert_eq!(mxcsr(&after), expected_mxcsr, "{label}");
                    let mut unchanged = after.clone();
                    unchanged.rip = before.rip;
                    set_xmm(&mut unchanged, usize::from(dst), a);
                    assert_eq!(unchanged, before, "{label}");
                    assert_eq!(after.rip, before.rip + code.len() as u64, "{label}");
                }
            }
        }
    }
}
#[test]
fn a_misaligned_memory_operand_is_gp0_before_any_read_and_changes_nothing() {
    for kind in KINDS {
        for shape in SHAPES {
            for form in &FORMS {
                for offset in [1u64, 2, 4, 8, 12, 15] {
                    let code = mem_code(kind, shape, form, 3);
                    let cpu = mem_cpu(form, DATA + offset, CR0_BASE, FP_INIT_CR4);
                    let before = cpu.clone();
                    let mut ram = Ram::new(&code);
                    let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let label = format!("{} {} +{offset}", name(kind, shape), form.label);
                    assert_eq!(
                        result.exit,
                        RunExit::Fault(ArchitecturalFault {
                            vector: 13,
                            error_code: Some(0),
                            linear_address: None
                        }),
                        "{label}"
                    );
                    assert!(ram.reads.is_empty(), "{label}");
                    assert_eq!(after, before, "{label}");
                }
            }
        }
    }
}
#[test]
fn non_canonical_addresses_fault_before_alignment_and_before_memory() {
    for kind in KINDS {
        for shape in SHAPES {
            for form in &FORMS {
                for address in [
                    0x0000_8000_0000_0000u64,
                    0x0000_8000_0000_0001,
                    0xffff_7fff_ffff_fff0,
                ] {
                    let code = mem_code(kind, shape, form, 2);
                    let cpu = mem_cpu(form, address, CR0_BASE, FP_INIT_CR4);
                    let before = cpu.clone();
                    let mut ram = Ram::new(&code);
                    let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    // #SS(0) for an RSP/RBP base, #GP(0) for every other base.
                    let vector = if matches!(form.base, Register::Rsp | Register::Rbp) {
                        12
                    } else {
                        13
                    };
                    let label = format!("{} {} {address:#x}", name(kind, shape), form.label);
                    assert_eq!(
                        result.exit,
                        RunExit::Fault(ArchitecturalFault {
                            vector,
                            error_code: Some(0),
                            linear_address: None
                        }),
                        "{label}"
                    );
                    assert!(ram.reads.is_empty(), "{label}");
                    assert_eq!(after, before, "{label}");
                }
            }
        }
    }
}
#[test]
fn a_backend_fault_leaves_cpu_mxcsr_and_memory_untouched() {
    for kind in KINDS {
        for shape in SHAPES {
            let form = &FORMS[0];
            let code = mem_code(kind, shape, form, 1);
            let mut cpu = mem_cpu(form, DATA, CR0_BASE, FP_INIT_CR4);
            cpu.sse.as_mut().unwrap().mxcsr = MXCSR & !0x80;
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            ram.fault_page = Some(DATA);
            let snapshot = ram.bytes.clone();
            let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
            assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        linear_address: Some(DATA),
                        ..
                    })
                ),
                "{:?}",
                result.exit
            );
            assert_eq!(after, before);
            assert_eq!(ram.bytes, snapshot);
        }
    }
}
#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_before_any_address_or_memory_check() {
    let cases: [(u64, u64, Option<u8>); 6] = [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE, Some(6)),
    ];
    for kind in KINDS {
        for shape in SHAPES {
            for form in [None, Some(&FORMS[0]), Some(&FORMS[1])] {
                let code = match form {
                    None => reg_code(kind, shape, 1, 2, false),
                    Some(form) => mem_code(kind, shape, form, 3),
                };
                for (cr0, cr4, vector) in cases {
                    // A misaligned canonical address: the control register gates come first.
                    let mut cpu = match form {
                        None => fp_init_cpu(cr0, cr4),
                        Some(form) => mem_cpu(form, DATA + 1, cr0, cr4),
                    };
                    cpu.sse.as_mut().unwrap().mxcsr = MXCSR;
                    let before = cpu.clone();
                    let mut ram = Ram::new(&code);
                    let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let label = format!(
                        "{} cr0 {cr0:#x} cr4 {cr4:#x} memory {}",
                        name(kind, shape),
                        form.is_some()
                    );
                    match vector {
                        Some(vector) => {
                            assert_eq!(
                                result.exit,
                                RunExit::Fault(ArchitecturalFault {
                                    vector,
                                    error_code: None,
                                    linear_address: None
                                }),
                                "{label}"
                            );
                            assert_eq!(after, before, "{label}");
                            assert!(ram.reads.is_empty(), "{label}");
                        }
                        None if form.is_some() => {
                            assert!(matches!(result.exit, RunExit::Fault(_)), "{label}");
                            assert_eq!(after, before, "{label}");
                        }
                        None => {
                            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{label}")
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn lock_is_invalid_opcode_for_every_form_and_prefix_order() {
    for kind in KINDS {
        for shape in SHAPES {
            let body = [0x0f, opcode(kind), 0x00];
            let codes = [
                [&[0xf0, prefix(shape)][..], &body].concat(),
                [&[prefix(shape), 0xf0][..], &body].concat(),
                [&[0xf0, prefix(shape), 0x41][..], &body].concat(),
            ];
            for code in codes {
                let outcome =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default());
                assert!(
                    matches!(outcome, Err(DecodeError::Invalid { .. })),
                    "{} {code:02x?}: {outcome:?}",
                    name(kind, shape)
                );
            }
        }
    }
}
#[test]
fn the_decoder_returns_the_general_operation_for_the_f2_and_66_forms_only() {
    for kind in KINDS {
        for shape in SHAPES {
            for (dst, src, rex_w) in [(1u8, 2u8, false), (3, 12, true), (9, 4, false)] {
                let code = reg_code(kind, shape, dst, src, rex_w);
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(
                    decoded.operation,
                    Operation::SseFpArith {
                        kind,
                        shape,
                        destination: dst,
                        source: Sse3Source::Register(src)
                    },
                    "{}",
                    name(kind, shape)
                );
                assert_eq!(usize::from(decoded.length), code.len());
            }
            let code = mem_code(kind, shape, &FORMS[1], 9);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            let Operation::SseFpArith {
                kind: k,
                shape: s,
                destination,
                source: Sse3Source::Memory(operand),
            } = decoded.operation
            else {
                panic!("{}: {:?}", name(kind, shape), decoded.operation);
            };
            assert_eq!((k, s, destination), (kind, shape, 9));
            assert_eq!(operand.displacement, 0x10);
            assert_eq!(usize::from(decoded.length), code.len());
            // The prefixes the SDM does not define for the opcode (none, F3, and the scalar-style F2/66 swap
            // does not exist for these forms) are refused, never decoded as another shape.
            for refused in [
                &[][..],
                &[0xf3][..],
                &[0xf2, 0xf3][..],
                &[0x66, 0x66][..],
                &[0x66, 0xf2][..],
                &[0x64, prefix(shape)][..],
            ] {
                let code = [refused, &[0x0f, opcode(kind), 0xca]].concat();
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default());
                assert!(
                    !matches!(decoded, Ok(ref d) if matches!(d.operation, Operation::SseFpArith { .. })),
                    "{} {code:02x?}",
                    name(kind, shape)
                );
            }
        }
    }
}
#[test]
fn refused_prefix_sequences_are_unsupported_without_a_change() {
    for kind in KINDS {
        for shape in SHAPES {
            let tail = [0x0f, opcode(kind), 0xca];
            let wrong = if shape == Shape::Packed32 { 0x66 } else { 0xf2 };
            let codes = [
                [&[][..], &tail].concat(),
                [&[0xf3][..], &tail].concat(),
                [&[0x64, prefix(shape)][..], &tail].concat(),
                [&[0x65, prefix(shape)][..], &tail].concat(),
                [&[prefix(shape), prefix(shape)][..], &tail].concat(),
                [&[wrong, prefix(shape)][..], &tail].concat(),
            ];
            for code in codes {
                let mut cpu = cpu();
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0).unwrap();
                assert!(
                    matches!(outcome, StepOutcome::Unsupported { .. }),
                    "{} {code:02x?}: {outcome:?}",
                    name(kind, shape)
                );
                assert_eq!(cpu, before);
            }
        }
    }
}
/// Operands that make the first element of the form an invalid operation (inf - inf, inf + -inf).
fn invalid_operands(kind: Kind, shape: Shape) -> (u128, u128) {
    let (pos, neg) = (f64::INFINITY, f64::NEG_INFINITY);
    let (a, b): ([f64; 2], [f64; 2]) = match kind {
        Kind::HAdd => ([pos, neg], [pos, neg]),
        Kind::HSub => ([pos, pos], [pos, pos]),
        _ => ([pos, pos], [pos, pos]),
    };
    if shape == Shape::Packed32 {
        let narrow = |v: [f64; 2]| f32s([v[0] as f32, v[1] as f32, v[0] as f32, v[1] as f32]);
        (narrow(a), narrow(b))
    } else {
        (f64s(a), f64s(b))
    }
}
fn cpu_without_control_registers(mxcsr: u32, a: u128, b: u128) -> LongModeState {
    let mut cpu = cpu();
    cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
    set_xmm(&mut cpu, 1, a);
    set_xmm(&mut cpu, 2, b);
    cpu
}
#[test]
fn the_bare_run_executes_masked_forms_refuses_reserved_mxcsr_bits_and_unmasked_exceptions() {
    for kind in KINDS {
        for shape in SHAPES {
            let (a, b) = if shape == Shape::Packed32 {
                (f32s([1.0, 2.0, 3.0, 4.0]), f32s([5.0, 6.0, 7.0, 8.0]))
            } else {
                (f64s([1.0, 2.0]), f64s([5.0, 6.0]))
            };
            let code = reg_code(kind, shape, 1, 2, false);
            let mut cpu = cpu_without_control_registers(MXCSR, a, b);
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(
                (xmm(&cpu, 1), mxcsr(&cpu)),
                run_reg(kind, shape, MXCSR, a, b)
            );
            assert_eq!(cpu.rip, before.rip + code.len() as u64);
            for reserved in [1u32 << 16, 1 << 31, 0xffff_0000] {
                let mut cpu = cpu_without_control_registers(MXCSR | reserved, a, b);
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Unsupported {
                        opcode: opcode(kind)
                    }),
                    "{} {reserved:#x}",
                    name(kind, shape)
                );
                assert_eq!(cpu, before);
            }
            // An unmasked exception without CR4 in the state: the vector is not known, nothing changes.
            let (a, b) = invalid_operands(kind, shape);
            let mut cpu = cpu_without_control_registers(MXCSR & !0x80, a, b);
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Unsupported {
                    opcode: opcode(kind)
                }),
                "{}",
                name(kind, shape)
            );
            assert_eq!(cpu, before);
        }
    }
    // A state without SSE is Unsupported and changes nothing.
    for kind in KINDS {
        let mut cpu = cpu();
        cpu.sse = None;
        let before = cpu.clone();
        let code = reg_code(kind, Shape::Packed32, 1, 2, false);
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Unsupported {
                opcode: opcode(kind)
            })
        );
        assert_eq!(cpu, before);
    }
}
#[test]
fn flags_are_sticky_and_accumulate_over_all_elements() {
    let tiny = f32::from_bits(1);
    let (inf, max) = (f32::INFINITY, f32::MAX);
    // HADDPS: element 0 = 1 + denormal (inexact, DE), element 1 = inf + -inf (invalid), element 2 =
    // max + max (overflow with the default delivered infinity: OE and PE), element 3 = 1 + 2 (exact).
    let a = f32s([1.0, tiny, inf, -inf]);
    let b = f32s([max, max, 1.0, 2.0]);
    let (value, mxcsr) = run_reg(Kind::HAdd, Shape::Packed32, MXCSR, a, b);
    assert_eq!(
        value,
        lanes32([
            1.0f32.to_bits(),
            0xffc0_0000,
            inf.to_bits(),
            3.0f32.to_bits()
        ])
    );
    assert_eq!(mxcsr & 0x3f, 0x1 | 0x2 | 0x8 | 0x20, "IE DE OE PE");
    // The flags that were set before stay set.
    let (_, mxcsr) = run_reg(Kind::HAdd, Shape::Packed32, MXCSR | 0x3f, a, b);
    assert_eq!(mxcsr & 0x3f, 0x3f);
}
#[test]
fn the_privilege_level_does_not_change_a_register_form() {
    // The privilege only selects the access context: Ring0 and Ring3 give identical results for a register
    // form (no CPL3 gate in the executor; the CPL3 profile set refuses the form separately).
    let code = reg_code(Kind::HAdd, Shape::Packed32, 1, 2, false);
    let a = f32s([1.0, 2.0, 3.0, 4.0]);
    let b = f32s([5.0, 6.0, 7.0, 8.0]);
    let ring0 = run_code(&code, (1, 2), MXCSR, (a, b), PrivilegeLevel::Ring0);
    let ring3 = run_code(&code, (1, 2), MXCSR, (a, b), PrivilegeLevel::Ring3);
    assert_eq!(ring0.after, ring3.after);
}
