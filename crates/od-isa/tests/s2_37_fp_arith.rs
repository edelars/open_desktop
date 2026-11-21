//! S2-37: the general SSE floating-point core and its first slice, ADD/SUB/MUL/DIV in the forms
//! PS (no prefix), PD (66), SS (F3) and SD (F2): ADDPS/ADDPD/ADDSS/ADDSD, SUBPS/SUBPD/SUBSS/SUBSD,
//! MULPS/MULPD/MULSS/MULSD and DIVPS/DIVPD/DIVSS/DIVSD, with any XMM or memory source.
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A ADDPD pp. 3-16..3-18, ADDPS pp. 3-19..3-21, ADDSD pp. 3-22..3-23, ADDSS pp. 3-24..3-25, DIVPD
//! pp. 3-267..3-269, DIVPS pp. 3-270..3-272, DIVSD pp. 3-273..3-274, DIVSS pp. 3-275..3-276; Vol. 2B MULPD
//! pp. 4-142..4-144, MULPS pp. 4-145..4-147, MULSD pp. 4-148..4-149, MULSS pp. 4-150..4-151, SUBPD
//! pp. 4-687..4-689, SUBPS pp. 4-690..4-692, SUBSD pp. 4-693..4-694, SUBSS pp. 4-695..4-696; exception classes
//! Vol. 2A Table 2-19 (Type 2: the packed forms, a legacy-SSE 128-bit memory operand must be 16-byte aligned)
//! and Table 2-20 (Type 3: the scalar forms); Vol. 1 section 4.8.3.5 Table 4-8 (SSE NaN rules), 4.9.1
//! and 4.9.2 (exception conditions and priority), 10.2.3 (MXCSR, DAZ, FTZ), 11.5.2 and 11.5.3 (SIMD
//! floating-point exceptions, masked and unmasked responses, Table 11-1, Figure 11-9).
//!
//! Where the SDM text does not fix an observable detail, the expected value is the one the physical x86
//! oracle gave (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; the program loads MXCSR with LDMXCSR and runs the
//! instruction directly): the flags that a faulting instruction leaves in MXCSR, the exact-overflow case without
//! PE, divide-by-zero over denormal operand, QNaN over divide-by-zero. The tables GOLDEN and HAND below are
//! hardware results, not model output (work/stage8/s2_37_oracle/: gen_cases.py, gen_hand.py, pick_golden.py,
//! oracle.c). The tests that need no hardware value compare the model against itself in a different form
//! (memory form against the register form, the general decoder against the narrow one) or against the host's
//! own IEEE arithmetic (round-to-nearest-even on random operands).
//!
//! Not asserted here: the CPUID gate (S2-57), "#AC" of the scalar forms (an unaligned scalar operand at CPL3
//! with RFLAGS.AC = 1 is Unsupported instead of guessed, as in S2-34), FS/GS overrides and the sequences of
//! mandatory prefixes the hardware tolerates (they stay Unsupported), the JIT (refused, see od-jit), CPL3
//! (refused, see cpl3_profile_set).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{
    self, Operation, Sse3Source, SseFpArithKind as Kind, SseFpShape as Shape,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
/// All six exceptions masked, round to nearest, no DAZ, no FTZ, no flag set.
const MXCSR: u32 = 0x1f80;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;

const KINDS: [Kind; 4] = [Kind::Add, Kind::Sub, Kind::Mul, Kind::Div];
const SHAPES: [Shape; 4] = [
    Shape::Packed32,
    Shape::Packed64,
    Shape::Scalar32,
    Shape::Scalar64,
];

/// What a vector of the tables must produce: the destination register and the MXCSR afterwards, or a
/// SIMD floating-point exception (#XM) with the MXCSR the fault leaves behind (destination unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Ok(u128, u32),
    Fault(u32),
}

fn opcode(kind: Kind) -> u8 {
    match kind {
        Kind::Add => 0x58,
        Kind::Mul => 0x59,
        Kind::Sub => 0x5c,
        Kind::Div => 0x5e,
    }
}

fn prefix(shape: Shape) -> &'static [u8] {
    match shape {
        Shape::Packed32 => &[],
        Shape::Packed64 => &[0x66],
        Shape::Scalar32 => &[0xf3],
        Shape::Scalar64 => &[0xf2],
    }
}

fn is_packed(shape: Shape) -> bool {
    matches!(shape, Shape::Packed32 | Shape::Packed64)
}

/// Bytes of the memory source: 16 for the packed forms, the element for the scalar ones.
fn memory_bytes(shape: Shape) -> usize {
    match shape {
        Shape::Packed32 | Shape::Packed64 => 16,
        Shape::Scalar32 => 4,
        Shape::Scalar64 => 8,
    }
}

fn name(kind: Kind, shape: Shape) -> String {
    let op = match kind {
        Kind::Add => "add",
        Kind::Sub => "sub",
        Kind::Mul => "mul",
        Kind::Div => "div",
    };
    let suffix = match shape {
        Shape::Packed32 => "ps",
        Shape::Packed64 => "pd",
        Shape::Scalar32 => "ss",
        Shape::Scalar64 => "sd",
    };
    format!("{op}{suffix}")
}

/// [66|F2|F3] [REX] 0F op /r with a register source. REX.W is ignored by the instruction; with it the
/// narrow profile decoder refuses the encoding and the general decoder handles it.
fn reg_code(kind: Kind, shape: Shape, dst: u8, src: u8, rex_w: bool) -> Vec<u8> {
    let mut code = prefix(shape).to_vec();
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

fn gp0() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

fn ss0() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 12,
        error_code: Some(0),
        linear_address: None,
    })
}

fn ud() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
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

/// The outcome of one register-form instruction under the FP-init profile pair with CR4.OSXMMEXCPT = 1.
struct Observed {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    length: usize,
}

fn run_reg(
    (kind, shape): (Kind, Shape),
    (dst, src): (u8, u8),
    mxcsr: u32,
    (a, b): (u128, u128),
    rex_w: bool,
    privilege: PrivilegeLevel,
) -> Observed {
    let code = reg_code(kind, shape, dst, src, rex_w);
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT);
    cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
    set_xmm(&mut cpu, usize::from(dst), a);
    if src != dst {
        set_xmm(&mut cpu, usize::from(src), b);
    }
    let before = cpu.clone();
    let mut ram = Ram::new(&code);
    let (result, after) = run_fp_init(cpu, &mut ram, privilege);
    assert!(ram.reads.is_empty() && ram.writes.is_empty());
    Observed {
        exit: result.exit,
        before,
        after,
        length: code.len(),
    }
}

/// The destination register and MXCSR of a completed instruction, or the MXCSR of a faulting one.
fn outcome_of(observed: &Observed, dst: usize) -> Expect {
    match observed.exit {
        RunExit::InstructionBudgetExhausted => {
            Expect::Ok(xmm(&observed.after, dst), mxcsr(&observed.after))
        }
        RunExit::Fault(_) => Expect::Fault(mxcsr(&observed.after)),
        ref other => panic!("unexpected exit {other:?}"),
    }
}

/// Whole-state check of one register-form run against an expectation: only the destination XMM, MXCSR
/// and RIP may change (a fault changes MXCSR only), and a fault is #XM.
fn check_observed(observed: &Observed, dst: usize, expect: Expect, label: &str) {
    let mut expected = observed.before.clone();
    match expect {
        Expect::Ok(result, mxcsr) => {
            assert_eq!(
                observed.exit,
                RunExit::InstructionBudgetExhausted,
                "{label}"
            );
            set_xmm(&mut expected, dst, result);
            expected.sse.as_mut().unwrap().mxcsr = mxcsr;
            expected.rip += observed.length as u64;
        }
        Expect::Fault(mxcsr) => {
            assert_eq!(observed.exit, xm(), "{label}");
            expected.sse.as_mut().unwrap().mxcsr = mxcsr;
        }
    }
    assert_eq!(observed.after, expected, "{label}");
}

type Golden = (Kind, Shape, u32, u128, u128, Expect);
type Hand = (&'static str, Kind, Shape, u32, u128, u128, Expect);

// ---- memory addressing forms ----

enum Disp {
    None,
    D8(i8),
    D32(i32),
    /// RIP-relative: the absolute target; disp32 is computed from the end of the instruction.
    Rip(u64),
}

/// One memory addressing form: the bytes around the mandatory prefix and opcode, the registers that make
/// the effective address and the effective address that the SDM rules (Vol. 1 3.7.5, Vol. 2A 2.1.5 and
/// 2.2.1.6) give for them. Every address is 16-byte aligned.
struct Form {
    name: &'static str,
    /// Legacy prefixes before the mandatory prefix (67, null segment overrides).
    extra: &'static [u8],
    /// REX bits W/X/B (R comes from the XMM number); 0 means no REX byte unless R is needed.
    rex: u8,
    mode: u8,
    rm: u8,
    sib: Option<u8>,
    disp: Disp,
    regs: &'static [(Register, u64)],
    address: u64,
}

#[allow(clippy::too_many_arguments)]
const fn form(
    name: &'static str,
    extra: &'static [u8],
    rex: u8,
    (mode, rm, sib): (u8, u8, Option<u8>),
    disp: Disp,
    regs: &'static [(Register, u64)],
    address: u64,
) -> Form {
    Form {
        name,
        extra,
        rex,
        mode,
        rm,
        sib,
        disp,
        regs,
        address,
    }
}

const FORMS: [Form; 23] = [
    form(
        "[rax]",
        &[],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8010)],
        0x8010,
    ),
    form(
        "[rbx]",
        &[],
        0,
        (0, 3, None),
        Disp::None,
        &[(Register::Rbx, 0x8120)],
        0x8120,
    ),
    form(
        "[rsp] (SIB)",
        &[],
        0,
        (0, 4, Some(0x24)),
        Disp::None,
        &[(Register::Rsp, 0x8230)],
        0x8230,
    ),
    form(
        "[rbp+0x10]",
        &[],
        0,
        (1, 5, None),
        Disp::D8(0x10),
        &[(Register::Rbp, 0x8320)],
        0x8330,
    ),
    form(
        "[rbp-0x20]",
        &[],
        0,
        (1, 5, None),
        Disp::D8(-0x20),
        &[(Register::Rbp, 0x8360)],
        0x8340,
    ),
    form(
        "[rbx+0x120] disp32",
        &[],
        0,
        (2, 3, None),
        Disp::D32(0x120),
        &[(Register::Rbx, 0x8000)],
        0x8120,
    ),
    form(
        "[rax+rcx*4+0x10]",
        &[],
        0,
        (1, 4, Some(0x88)),
        Disp::D8(0x10),
        &[(Register::Rax, 0x8000), (Register::Rcx, 4)],
        0x8020,
    ),
    form(
        "[r8] (REX.B)",
        &[],
        1,
        (0, 0, None),
        Disp::None,
        &[(Register::R8, 0x8440)],
        0x8440,
    ),
    form(
        "[r13+0x10] (REX.B, disp8)",
        &[],
        1,
        (1, 5, None),
        Disp::D8(0x10),
        &[(Register::R13, 0x8650)],
        0x8660,
    ),
    form(
        "[r12] (REX.B, SIB)",
        &[],
        1,
        (0, 4, Some(0x24)),
        Disp::None,
        &[(Register::R12, 0x8570)],
        0x8570,
    ),
    form(
        "[rbx+r9*8+0x20] (REX.X)",
        &[],
        2,
        (1, 4, Some(0xcb)),
        Disp::D8(0x20),
        &[(Register::Rbx, 0x8100), (Register::R9, 2)],
        0x8130,
    ),
    form(
        "[disp32] (SIB, no base)",
        &[],
        0,
        (0, 4, Some(0x25)),
        Disp::D32(0x8800),
        &[],
        0x8800,
    ),
    form(
        "[r9*4+disp32] (REX.X, no base)",
        &[],
        2,
        (0, 4, Some(0x8d)),
        Disp::D32(0x8700),
        &[(Register::R9, 4)],
        0x8710,
    ),
    form(
        "[rip+disp32] forward",
        &[],
        0,
        (0, 5, None),
        Disp::Rip(0x8900),
        &[],
        0x8900,
    ),
    form(
        "[rip+disp32] backward",
        &[],
        0,
        (0, 5, None),
        Disp::Rip(0x1000),
        &[],
        0x1000,
    ),
    form(
        "[rip+disp32] with REX.B (ignored)",
        &[],
        1,
        (0, 5, None),
        Disp::Rip(0x8910),
        &[(Register::R13, 0x8f00)],
        0x8910,
    ),
    form(
        "[rax] REX.W (ignored)",
        &[],
        8,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8a10)],
        0x8a10,
    ),
    form(
        "[eax] 67 truncates to 32 bits",
        &[0x67],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0xdead_beef_0000_8a40)],
        0x8a40,
    ),
    form(
        "[eax+ecx*4+0x10] 67 wraps in 32 bits",
        &[0x67],
        0,
        (1, 4, Some(0x88)),
        Disp::D8(0x10),
        &[
            (Register::Rax, 0xffff_ffff_ffff_fff0),
            (Register::Rcx, 0x1_0000_0004),
        ],
        0x10,
    ),
    form(
        "[rax] cs override (ignored)",
        &[0x2e],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8b00)],
        0x8b00,
    ),
    form(
        "[rax] ss override (ignored)",
        &[0x36],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8b10)],
        0x8b10,
    ),
    form(
        "[rax] ds override (ignored)",
        &[0x3e],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8b20)],
        0x8b20,
    ),
    form(
        "[rax] es override (ignored)",
        &[0x26],
        0,
        (0, 0, None),
        Disp::None,
        &[(Register::Rax, 0x8b30)],
        0x8b30,
    ),
];

fn mem_code(kind: Kind, shape: Shape, form: &Form, dst: u8) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    code.extend_from_slice(prefix(shape));
    let rex = form.rex | u8::from(dst >= 8) << 2;
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend([
        0x0f,
        opcode(kind),
        form.mode << 6 | (dst & 7) << 3 | form.rm,
    ]);
    if let Some(sib) = form.sib {
        code.push(sib);
    }
    match form.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend(value.to_le_bytes()),
        Disp::Rip(target) => {
            let end = CODE + code.len() as u64 + 4;
            code.extend((target.wrapping_sub(end) as i64 as i32).to_le_bytes());
        }
    }
    code
}

fn setup(form: &Form) -> LongModeState {
    let mut cpu = cpu();
    for (register, value) in form.regs {
        cpu.gpr[*register as usize] = *value;
    }
    cpu
}

fn lanes32(lanes: [u32; 4]) -> u128 {
    lanes.iter().enumerate().fold(0, |acc, (index, lane)| {
        acc | u128::from(*lane) << (32 * index)
    })
}

fn lanes64(lanes: [u64; 2]) -> u128 {
    u128::from(lanes[0]) | u128::from(lanes[1]) << 64
}

/// Operand pairs that exercise exact results, inexact results, a denormal operand, overflow and tiny results
/// in every lane position.
fn sample_pair(shape: Shape, salt: usize) -> (u128, u128) {
    match shape {
        Shape::Packed32 | Shape::Scalar32 => {
            let a = [
                [0x3f80_0000, 0x3dcc_cccd, 0x7f7f_ffff, 0x0000_0001],
                [0x4040_0000, 0x0080_0001, 0xbf80_0000, 0x7fc0_1234],
                [0x3f80_0000, 0x3f80_0000, 0x3f80_0000, 0x3f80_0000],
            ][salt % 3];
            let b = [
                [0x4040_0000, 0x3e4c_cccd, 0x7f7f_ffff, 0x4000_0000],
                [0x3f40_0000, 0x3f40_0000, 0x3f80_0000, 0x3f80_0000],
                [0x4040_0000, 0x4000_0000, 0x3f80_0000, 0x0000_0000],
            ][salt % 3];
            (lanes32(a), lanes32(b))
        }
        Shape::Packed64 | Shape::Scalar64 => {
            let a = [
                [0x3ff0_0000_0000_0000, 0x3fb9_9999_9999_999a],
                [0x4008_0000_0000_0000, 0x0010_0000_0000_0001],
                [0x7fef_ffff_ffff_ffff, 0x0000_0000_0000_0001],
            ][salt % 3];
            let b = [
                [0x4008_0000_0000_0000, 0x3fc9_9999_9999_999a],
                [0x3fe8_0000_0000_0000, 0x3fe8_0000_0000_0000],
                [0x7fef_ffff_ffff_ffff, 0x4000_0000_0000_0000],
            ][salt % 3];
            (lanes64(a), lanes64(b))
        }
    }
}

// ---- tables of hardware results ----

#[rustfmt::skip]
const GOLDEN: &[Golden] = &[
    (Kind::Add, Shape::Packed64, 0x3e80, 0x3ff00000000000017ff6a0a613bed601, 0x3ca00000000000000010000000000001, Expect::Ok(0x3ff00000000000017ffea0a613bed601, 0x3ea1)),
    (Kind::Add, Shape::Packed64, 0x5b80, 0x00100000000000010000000000000001, 0x0010000000000000ffefffffffffffff, Expect::Ok(0x0020000000000001ffeffffffffffffe, 0x5ba2)),
    (Kind::Add, Shape::Packed64, 0x7e80, 0x4340000000000000c3270f9599fd7209, 0xbff0000000000000fff0000000000001, Expect::Ok(0x433ffffffffffffffff8000000000001, 0x7e81)),
    (Kind::Add, Shape::Packed64, 0x7d80, 0x00040e2da543085a432d1b2a1b33f417, 0x7ff7ffffffffffff800e64d08bc2c89d, Expect::Ok(0x7fffffffffffffff432d1b2a1b33f416, 0x7da3)),
    (Kind::Add, Shape::Packed64, 0x7d80, 0x80000000000000017ff1d1826f8b3bfe, 0x0010000000000000800fffffffffffff, Expect::Ok(0x000fffffffffffff7ff9d1826f8b3bfe, 0x7d83)),
    (Kind::Add, Shape::Packed64, 0x7780, 0xc0027e896af716310010000000000000, 0x7ff8000000000001ffefffffffffffff, Expect::Ok(0x7ff8000000000001ffeffffffffffffe, 0x77a0)),
    (Kind::Add, Shape::Packed64, 0xfb80, 0xfff00000000000010020000000000000, 0x80219e8881831e3a433fffffffffffff, Expect::Ok(0xfff8000000000001433fffffffffffff, 0xfba1)),
    (Kind::Add, Shape::Packed64, 0x77c0, 0x800c8261dd85c8443ca0000000000000, 0x7fd1769ecda98391fff0000000000000, Expect::Ok(0x7fd1769ecda98391fff0000000000000, 0x77c0)),
    (Kind::Add, Shape::Packed64, 0xef80, 0x00000000000000017fffffffffffffff, 0x002481237ed9b91dfff0000000000001, Expect::Fault(0xefa3)),
    (Kind::Add, Shape::Packed32, 0x1b80, 0x7fffffff7f8000017fc308603f8647ce, 0x7fa00000ffbfffff7f0000007fbfffff, Expect::Ok(0x7fffffff7fc000017fc308607fffffff, 0x1b81)),
    (Kind::Add, Shape::Packed32, 0x3d80, 0x014705097f800000008000007fc00000, 0x407b6121010000007f00000080000000, Expect::Ok(0x407b61217f8000007f0000007fc00000, 0x3da0)),
    (Kind::Add, Shape::Packed32, 0x2f80, 0x40a0000001000000bfa2db0c80000001, 0x7f0000007fbfffff3f0000000b5373b0, Expect::Fault(0x2fa3)),
    (Kind::Add, Shape::Packed32, 0x5f00, 0x4b8000003f7fffff7f7fffff40000000, 0x0000000100780ed0300d3297ff649863, Expect::Ok(0x4b8000013f8000007f800000ff649862, 0x5f2a)),
    (Kind::Add, Shape::Packed32, 0x5d80, 0x4b800000ff8000010123feba7e800000, 0x8aa06916ff7fffff7f7fffffbfd32167, Expect::Ok(0x4b800000ffc000017f8000007e800000, 0x5da9)),
    (Kind::Add, Shape::Packed32, 0x5d80, 0x7fbfffff800000008009caed7fa00000, 0x40a00000807fffff00ffffffc06b8bf1, Expect::Ok(0x7fffffff807fffff00f635127fe00000, 0x5d83)),
    (Kind::Add, Shape::Packed32, 0x5f80, 0x807fffff808000000080000100000000, 0x0b7414e7ffbfffff7e800000c0000000, Expect::Ok(0x0b7414e4ffffffff7e800001c0000000, 0x5fa3)),
    (Kind::Add, Shape::Packed32, 0x9d80, 0x74857d04bf8000007fa000004b7fffff, 0x7fffffff0000000174fff5617e800000, Expect::Ok(0x7fffffffbf8000007fe000007e800000, 0x9da3)),
    (Kind::Add, Shape::Packed32, 0x1dc0, 0xbfaf8186800000000000000380000000, 0x7fbfffff007fffff8127e0ff4b800000, Expect::Ok(0x7fffffff000000008127e0ff4b800000, 0x1dc1)),
    (Kind::Add, Shape::Packed32, 0x9780, 0x004000007efffffffffa6cc4305ca9c3, 0xffbfffff007fffff80b8d76974c9405b, Expect::Ok(0xffffffff7efffffffffa6cc474c9405b, 0x97a3)),
    (Kind::Add, Shape::Packed32, 0x8f80, 0xff7fffff000903c07495bab900000001, 0xc5a7c3037fbfffffff8000004b000000, Expect::Fault(0x8fa3)),
    (Kind::Add, Shape::Packed32, 0x8f80, 0x400000007f091da83fc0000000800001, 0x7f8000017f4c1e143f7fffff00000003, Expect::Fault(0x8fab)),
    (Kind::Add, Shape::Packed32, 0xff00, 0xffc12345800000007fc0000000400000, 0x4b7fffff00000001bfca5afa00000000, Expect::Ok(0xffc12345000000007fc0000000000000, 0xff32)),
    (Kind::Add, Shape::Packed32, 0x7f40, 0xff8000007fbfffff7f8000013f000000, 0x4fc66430bcb3f9603380000040000000, Expect::Fault(0x7f41)),
    (Kind::Add, Shape::Packed32, 0x7f40, 0x7f8000007effffff802d47c500000000, 0x7fc000017f243ac9ffc000003f000000, Expect::Ok(0x7fc000017f7fffffffc000003f000000, 0x7f68)),
    (Kind::Add, Shape::Packed32, 0xf780, 0x3f000000b3addcea008000007e800000, 0xff800000800000017e8000007f800001, Expect::Ok(0xff800000b3addcea7e8000007fc00001, 0xf7a3)),
    (Kind::Add, Shape::Packed32, 0x77c0, 0x01000000008000017fc000007f800001, 0xffe72752807fffff004000004b000000, Expect::Ok(0xffe72752008000017fc000007fc00001, 0x77c1)),
    (Kind::Add, Shape::Packed32, 0x1f00, 0x3e80000040a000007fffffff7e800000, 0x7fffffffffef7117803bc14d3f800000, Expect::Ok(0x7fffffffffef71177fffffff7e800000, 0x1f20)),
    (Kind::Div, Shape::Packed64, 0x3e80, 0x3fd00000000000003cb0000000000000, 0x7fd2beac2c14dd40432922f2880d00d1, Expect::Ok(0x00036a0e181e34e039745e68159506ac, 0x3eb0)),
    (Kind::Div, Shape::Packed64, 0x3d80, 0x00000000000000037fd0000000000000, 0xbcaeb71fbe86ac90fff6c5d02fa66135, Expect::Ok(0x802900f93451785ffffec5d02fa66135, 0x3da3)),
    (Kind::Div, Shape::Packed64, 0x3d80, 0x80297c207a0f57fc000fffffffffffff, 0x7ff00000000000003ff0000000000000, Expect::Ok(0x8000000000000000000fffffffffffff, 0x3d82)),
    (Kind::Div, Shape::Packed64, 0x3b80, 0x7fdfffffffffffffc000000000000000, 0x3ca9bbd562ec4598ffef24bd198129fd, Expect::Fault(0x3bb8)),
    (Kind::Div, Shape::Packed64, 0x3780, 0x0000000000000003001fffffffffffff, 0x7fd00000000000000020000000000000, Expect::Fault(0x3792)),
    (Kind::Div, Shape::Packed64, 0x3f80, 0xffeb9ca3c9a454db0020000000000000, 0x3cbc8dea87f26fe18000000000000001, Expect::Ok(0xfff0000000000000c340000000000000, 0x3faa)),
    (Kind::Div, Shape::Packed64, 0x5e80, 0x432990c949c64ee70010000000000001, 0x4000000000000000bca263573581e4a4, Expect::Ok(0x431990c949c64ee7835bd81b9d460de5, 0x5ea0)),
    (Kind::Div, Shape::Packed64, 0x7d80, 0x3cb00000000000000010000000000000, 0xfff0000000000001bffd50f8b1078d42, Expect::Ok(0xfff80000000000018008bb7af669ded0, 0x7db1)),
    (Kind::Div, Shape::Packed64, 0x5f80, 0x8014c86eeed35be03ff0000000000001, 0x3ca00000000000008000000000000001, Expect::Ok(0x8364c86eeed35be0ffefffffffffffff, 0x5faa)),
    (Kind::Div, Shape::Packed64, 0x6f80, 0x7ff8000000000001001fffffffffffff, 0x3ca00000000000003ca0000000000000, Expect::Ok(0x7ff8000000000001036fffffffffffff, 0x6f80)),
    (Kind::Div, Shape::Packed64, 0x6f80, 0x7ff40000000000003cb0000000000000, 0x7ff80000000000010000000000000001, Expect::Ok(0x7ffc0000000000007fd0000000000000, 0x6f83)),
    (Kind::Div, Shape::Packed64, 0x9d80, 0x7ff00000000000007fd0000000000000, 0x3cb4ce011421edd23cb4624b3109175d, Expect::Ok(0x7ff00000000000007ff0000000000000, 0x9da8)),
    (Kind::Div, Shape::Packed64, 0x8f80, 0xfff0000000000001ffe351c66c049191, 0x3fe00000000000004000000000000000, Expect::Ok(0xfff8000000000001ffd351c66c049191, 0x8f81)),
    (Kind::Div, Shape::Packed64, 0xfe80, 0x432a88ba3b7e8551ffd5b18558076b83, 0x00100000000000017ffbb5c487a44506, Expect::Ok(0x7fefffffffffffff7ffbb5c487a44506, 0xfea8)),
    (Kind::Div, Shape::Packed64, 0xfd80, 0x7fffffffffffffff4008000000000000, 0x00080000000000007fffffffffffffff, Expect::Ok(0x7fffffffffffffff7fffffffffffffff, 0xfd80)),
    (Kind::Div, Shape::Packed64, 0x7bc0, 0xfffd3c308e5fba4c000fffffffffffff, 0x3ff0000000000001001fffffffffffff, Expect::Ok(0xfffd3c308e5fba4c0000000000000000, 0x7bc0)),
    (Kind::Div, Shape::Packed64, 0xf780, 0x5e6fdf86426ab0d7001590c06441cf9b, 0x00172cb1cb9fc502bcbb73cb25d27463, Expect::Ok(0x7fefffffffffffff8349234e2348820e, 0xf7a8)),
    (Kind::Div, Shape::Packed64, 0xf780, 0xfff8123456789abc0000000000000003, 0x8007a8aa1ed00c790000000000000001, Expect::Ok(0xfff8123456789abc4008000000000000, 0xf782)),
    (Kind::Div, Shape::Packed64, 0xef80, 0x00100000000000013ff8000000000000, 0x000ffffffffffffffff8123456789abc, Expect::Fault(0xefa2)),
    (Kind::Div, Shape::Packed64, 0xef80, 0x0019ebfb5fab5ba8000fffffffffffff, 0x3cb0000000000000800fffffffffffff, Expect::Ok(0x0359ebfb5fab5ba8bff0000000000000, 0xef82)),
    (Kind::Div, Shape::Packed64, 0x6000, 0xfff0000000000001ffd15a75a92d7063, 0x3fe00000000000008000000000000001, Expect::Fault(0x6003)),
    (Kind::Div, Shape::Packed64, 0x6000, 0x40140000000000003fe0000000000000, 0x40080000000000000010000000000001, Expect::Fault(0x6020)),
    (Kind::Div, Shape::Packed64, 0x9f80, 0xfff00000000000003fe0000000000000, 0xffe518b3258ee87c7fee3e70f6c03b49, Expect::Ok(0x7ff00000000000000000000000000000, 0x9fb0)),
    (Kind::Div, Shape::Packed64, 0x40, 0x000058648ab52fff3ff0000000000000, 0x400641d9023f3c207fffffffffffffff, Expect::Ok(0x00000000000000007fffffffffffffff, 0x40)),
    (Kind::Div, Shape::Packed64, 0x1e80, 0x7fd15d78bd6b5981c32d8fc05662477c, 0x3ca983d8010dbb37002afc17cfdcfbc1, Expect::Ok(0x7ff0000000000000fff0000000000000, 0x1ea8)),
    (Kind::Div, Shape::Packed64, 0x380, 0x00000000000000008007b08cc10f0ef6, 0xffda5273cf13c900bff0000000000000, Expect::Fault(0x392)),
    (Kind::Div, Shape::Packed32, 0x1f80, 0x000000013e8000008080000000400000, 0xffc00000cff2df237f7fffffff800001, Expect::Ok(0xffc00000ae06eb4480000000ffc00001, 0x1fb1)),
    (Kind::Div, Shape::Packed32, 0x3f00, 0x7fc000007fbfffff7effffffff800001, 0x404e9d07802f9926ff7fffff00522a8d, Expect::Fault(0x3f01)),
    (Kind::Div, Shape::Packed32, 0x3d80, 0x400000007f8000007f800000b03196a2, 0x4f93c49affbfffff3fc000007fc00000, Expect::Ok(0x2fddc0c8ffffffff7f8000007fc00000, 0x3da1)),
    (Kind::Div, Shape::Packed32, 0x3b80, 0x01000000bfc5dd1b3f800001ff5f8506, 0x305f01ce74ff1f443fc00000016262ee, Expect::Fault(0x3ba8)),
    (Kind::Div, Shape::Packed32, 0x3780, 0x0040000000400000007fffff7fa00000, 0x3f800000338000003fc0000000800000, Expect::Fault(0x37b3)),
    (Kind::Div, Shape::Packed32, 0x3f80, 0x004cc270800000010b2771b17f709a80, 0x000000003fc000000085b74680b2b2e3, Expect::Ok(0x7f800000800000014a204953ff800000, 0x3fbe)),
    (Kind::Div, Shape::Packed32, 0x5d80, 0x3f80000040a000008b0e32844b7fffff, 0x7f800001c00000004b7fffff002ba59c, Expect::Ok(0x7fc00001c02000008011c6507f800000, 0x5dbb)),
    (Kind::Div, Shape::Packed32, 0x5b80, 0x7f800001404000003e8000008b61b71b, 0xbf8000007fbfffff40a0000081156ee6, Expect::Ok(0x7fc000017fffffff3d4ccccd49c15747, 0x5ba1)),
    (Kind::Div, Shape::Packed32, 0x7f00, 0x3f800000d82b3a7f80000000804cfb2f, 0x7f0000003fc000007e8000005a83676d, Expect::Ok(0x00400000d7e44dfe8000000080000000, 0x7f32)),
    (Kind::Div, Shape::Packed32, 0x7e80, 0x005d43bf80c936483f800000ff800001, 0x80000000010000007f7fffff30135d00, Expect::Ok(0xff800000bf49364800200000ffc00001, 0x7eb5)),
    (Kind::Div, Shape::Packed32, 0x7d80, 0x01299a2bffbfffff7f000000803fe2e2, 0x00de3e187f8000007e80000033800000, Expect::Ok(0x3fc35d27ffffffff400000008bff8b88, 0x7da3)),
    (Kind::Div, Shape::Packed32, 0x7b80, 0x3fc000003f0000007effffff3f7fffff, 0x7f0000007fa00000000000003f800001, Expect::Ok(0x006000007fe000007f8000003f7ffffd, 0x7ba5)),
    (Kind::Div, Shape::Packed32, 0x7780, 0x80342d6c3e800000807fffffff800001, 0x7fffffff40000000000000010b40f5a5, Expect::Ok(0x7fffffff3e000000cafffffeffc00001, 0x7783)),
    (Kind::Div, Shape::Packed32, 0x9f00, 0x8063bfeb80000000815b1355f4ad2fc5, 0x7f7fffffb05a09e20100000000800001, Expect::Ok(0x8000000000000000bfdb1355ff800000, 0x9f3a)),
    (Kind::Div, Shape::Packed32, 0x9d80, 0x4b8000007f80000140a00000bf800000, 0x80000001c0000000c04e0eaf7f09d6b5, Expect::Ok(0xff8000007fc00001bfc6c79480000000, 0x9dbb)),
    (Kind::Div, Shape::Packed32, 0x1bc0, 0x3fcd11f97fbfffff3f7fffff7effffff, 0xffbfffff7fffffff00382ca6147044e1, Expect::Fault(0x1bed)),
    (Kind::Div, Shape::Packed32, 0x7f80, 0x00ffffff3fc000003f0000003f800001, 0x7fffffff4b8000003f000000bf800000, Expect::Ok(0x7fffffff33c000003f800000bf800001, 0x7f80)),
    (Kind::Div, Shape::Packed32, 0x9780, 0x000000007fa000000b4df32c800363e1, 0x748248be808000007effffff4b7fffff, Expect::Fault(0x97b3)),
    (Kind::Div, Shape::Packed32, 0x9780, 0x3fb1e017800000017fc00000ffbfffff, 0x80000001811fefb43fa83e9800800000, Expect::Ok(0xff800000334ce1ab7fc00000ffffffff, 0x97ab)),
    (Kind::Div, Shape::Packed32, 0x17c0, 0xbf800000817cfcf4bfdd11bcff800000, 0x808000003fc00000007fffff7f800001, Expect::Ok(0x7e8000008128a8a3ff8000007fc00001, 0x17e5)),
    (Kind::Div, Shape::Packed32, 0x8f80, 0x40000000ff7fffff7f7fffff7effffff, 0x000000007f8000013f99e116ffbfffff, Expect::Fault(0x8fa5)),
    (Kind::Div, Shape::Packed32, 0xfe80, 0xffc000003f80000174d588e17fffffff, 0x74acd98300ffffff3fb8f0857fc00000, Expect::Ok(0xffc000007e0000017493ca8d7fffffff, 0xfea0)),
    (Kind::Div, Shape::Packed32, 0x7ec0, 0x3fc00000805f4986c00000007f800000, 0x814788c27fbfffff0069f8b000400000, Expect::Ok(0xfdf6556d7fffffffff8000007f800000, 0x7ee5)),
    (Kind::Div, Shape::Packed32, 0xfd80, 0x7fbfffffff800001400000007fbfffff, 0xc000ba00c00000008000000001000000, Expect::Fault(0xfd85)),
    (Kind::Div, Shape::Packed32, 0xfd80, 0x40000000ffbfffff7f7fffffffbfffff, 0xffc00000bf9dd1193400000080000000, Expect::Ok(0xffc00000ffffffff7f7fffffffffffff, 0xfda9)),
    (Kind::Div, Shape::Packed32, 0x9f80, 0xffbfffff3f000000b04572770b0989a6, 0x0b12ba2f340000003f8000013f000000, Expect::Ok(0xffffffff4a800000b04572750b8989a6, 0x9fa1)),
    (Kind::Div, Shape::Packed32, 0x9f80, 0x40a000003fc0000000136c04ffc12345, 0x800000008b10630b7fffffff7fc00001, Expect::Ok(0xff800000f42a35997fffffffffc12345, 0x9fa4)),
    (Kind::Div, Shape::Packed32, 0x9f80, 0x4b00000080000001403370da7fffffff, 0x34000000008000013f8000013f7fffff, Expect::Ok(0x56800000b3fffffe403370d97fffffff, 0x9fa2)),
    (Kind::Div, Shape::Packed32, 0x1780, 0x7fa0000000000001400000007fffffff, 0x400000004b80000040000000ff7fffff, Expect::Fault(0x1793)),
    (Kind::Div, Shape::Packed32, 0xf80, 0x00ffffff00800001ff80000000000003, 0x40a000007fc00000014551f87e800000, Expect::Fault(0xfb2)),
    (Kind::Div, Shape::Packed32, 0x380, 0x0027d76c7fbfffff300edcba7f7fffff, 0x7f7fffff4b7fffff40a000007f000000, Expect::Fault(0x3b3)),
    (Kind::Div, Shape::Packed32, 0x1a00, 0x4b7fffffff8000014f9ada0b7fd30aa3, 0x008000013f00000000dffa87007fffff, Expect::Fault(0x1a01)),
    (Kind::Div, Shape::Packed32, 0x1f00, 0x400000008b16c0983f8000003fabf56f, 0x3400000000000003ff212dc680000000, Expect::Ok(0x4b800000d4c900cb8032d357ff800000, 0x1f36)),
    (Kind::Div, Shape::Packed32, 0x1e80, 0xff5f28d8800000007effffff00800001, 0x404000003fc00000301b7891c0000000, Expect::Ok(0xfe94c5e5800000007f80000080400000, 0x1eb8)),
    (Kind::Mul, Shape::Packed64, 0x1b80, 0x7fefffffffffffff0010000000000001, 0x43400000000000004014000000000000, Expect::Fault(0x1ba8)),
    (Kind::Mul, Shape::Packed64, 0x1780, 0x000fffffffffffff3ff0000000000000, 0xf1df16eb21f4d2c57fecec36204d6fb6, Expect::Ok(0xb1ff16eb21f4d2c37fecec36204d6fb6, 0x17a2)),
    (Kind::Mul, Shape::Packed64, 0x3f00, 0xfff0000000000000001fffffffffffff, 0x7fe00000000000000000000000000001, Expect::Ok(0xfff00000000000000000000000000000, 0x3f32)),
    (Kind::Mul, Shape::Packed64, 0x3d80, 0xffefffffffffffffc00e19d98f888b22, 0xfff80000000000007fe68f9d02c5f72d, Expect::Ok(0xfff8000000000000fff0000000000000, 0x3da8)),
    (Kind::Mul, Shape::Packed64, 0x3b80, 0x4330000000000000fff0000000000001, 0xfff00000000000003fe0000000000000, Expect::Ok(0xfff0000000000000fff8000000000001, 0x3b81)),
    (Kind::Mul, Shape::Packed64, 0x3780, 0xffefffffffffffff433fffffffffffff, 0x3fe00000000000000008000000000000, Expect::Ok(0xffdfffffffffffff034fffffffffffff, 0x3782)),
    (Kind::Mul, Shape::Packed64, 0x3780, 0xfff8123456789abc7ff0000000000000, 0x7fffffffffffffff433fffffffffffff, Expect::Ok(0xfff8123456789abc7ff0000000000000, 0x3780)),
    (Kind::Mul, Shape::Packed64, 0x2f80, 0x43300000000000003ca09a5b95f407d1, 0x3fefffffffffffff7ff8000000000000, Expect::Ok(0x432fffffffffffff7ff8000000000000, 0x2f80)),
    (Kind::Mul, Shape::Packed64, 0x5780, 0x7ff00000000000014330000000000000, 0x7fdfffffffffffff4008000000000000, Expect::Ok(0x7ff80000000000014348000000000000, 0x5781)),
    (Kind::Mul, Shape::Packed64, 0x7f00, 0xbff0000000000000ffefffffffffffff, 0x800fffffffffffff7fd0000000000000, Expect::Ok(0x000fffffffffffffffefffffffffffff, 0x7f2a)),
    (Kind::Mul, Shape::Packed64, 0x7e80, 0xc32b049c6f030a134014000000000000, 0x00100000000000017ff8000000000001, Expect::Ok(0x834b049c6f030a147ff8000000000001, 0x7ea0)),
    (Kind::Mul, Shape::Packed64, 0x7e80, 0x7ff80000000000007fffffffffffffff, 0x8022fd26b08bf7653cac1fd104f79ff3, Expect::Ok(0x7ff80000000000007fffffffffffffff, 0x7e80)),
    (Kind::Mul, Shape::Packed64, 0x7780, 0xd195f177cc22e57f3fe0000000000000, 0x800fffffffffffff3fefffffffffffff, Expect::Ok(0x11b5f177cc22e57d3fdfffffffffffff, 0x77a2)),
    (Kind::Mul, Shape::Packed64, 0x9f00, 0xffff5c7856c90fd68000000000000000, 0x00080000000000007fffffffffffffff, Expect::Ok(0xffff5c7856c90fd67fffffffffffffff, 0x9f00)),
    (Kind::Mul, Shape::Packed64, 0x9e80, 0x00100000000000007fd05647985577a2, 0x3ca00000000000007fefffffffffffff, Expect::Ok(0x00000000000000007ff0000000000000, 0x9eb8)),
    (Kind::Mul, Shape::Packed64, 0x9d80, 0x7fe54d3f6fa8c2314008000000000000, 0xfff8000000000000800a874ceb4fb366, Expect::Ok(0xfff8000000000000801f95e6c1ef1a32, 0x9d82)),
    (Kind::Mul, Shape::Packed64, 0x9b80, 0x3fd00000000000000010000000000001, 0x000fc87fe43847f27ff4000000000000, Expect::Ok(0x00000000000000007ffc000000000000, 0x9bb3)),
    (Kind::Mul, Shape::Packed64, 0x17c0, 0x7ff8aa443d1b86d44000000000000000, 0x7fd00000000000004000000000000000, Expect::Ok(0x7ff8aa443d1b86d44010000000000000, 0x17c0)),
    (Kind::Mul, Shape::Packed64, 0x8f80, 0x432ec473d494624d4340000000000000, 0x432434c462e809633fd0000000000000, Expect::Fault(0x8fa0)),
    (Kind::Mul, Shape::Packed64, 0xff00, 0x80000000000000000008000000000000, 0x00000000000000034330000000000000, Expect::Ok(0x80000000000000000340000000000000, 0xff02)),
    (Kind::Mul, Shape::Packed64, 0xfe80, 0x7ff80000000000017fe0000000000000, 0x3ca3758e82af1bc77cf89b517a5a36ae, Expect::Ok(0x7ff80000000000017fefffffffffffff, 0xfea8)),
    (Kind::Mul, Shape::Packed64, 0xfe80, 0xfff00000000000018000000000000001, 0x7ff7ffffffffffff000f038bbd7c329b, Expect::Fault(0xfe83)),
    (Kind::Mul, Shape::Packed64, 0x7bc0, 0x7ff7ffffffffffff7fdfffffffffffff, 0x0000000000000001bca6b541b61d0ba1, Expect::Ok(0x7ffffffffffffffffc96b541b61d0ba0, 0x7be1)),
    (Kind::Mul, Shape::Packed64, 0xf780, 0xc00b27d353ca56530000000000000001, 0xc0000000000000007fefffffffffffff, Expect::Ok(0x401b27d353ca56533ccfffffffffffff, 0xf782)),
    (Kind::Mul, Shape::Packed64, 0x6fc0, 0x8000000000000001c000000000000000, 0x3cb0000000000000ffefffffffffffff, Expect::Fault(0x6fe8)),
    (Kind::Mul, Shape::Packed64, 0x2000, 0xffd105e314a40109fff0000000000000, 0x40140000000000003ff0000000000000, Expect::Fault(0x2028)),
    (Kind::Mul, Shape::Packed64, 0x1d80, 0x000e187e7f8d02ca7ff36b1618a89132, 0x7fffffffffffffff0010538df1634d53, Expect::Ok(0x7fffffffffffffff7ffb6b1618a89132, 0x1d81)),
    (Kind::Mul, Shape::Packed64, 0x1d80, 0x3cbd6a72850cfcafc0089bcf1b78fc24, 0x0021777e2f3956997fefffffffffffff, Expect::Ok(0x0000000000000004fff0000000000000, 0x1db8)),
    (Kind::Mul, Shape::Packed64, 0x380, 0x0000000000000001400357922296bd84, 0x96c834e996850babfc8ed5ee9c955d79, Expect::Fault(0x3b2)),
    (Kind::Mul, Shape::Packed64, 0x1e80, 0x43300000000000008000000000000000, 0x7fdfffffffffffff3ff0249dcd95621e, Expect::Ok(0x7ff00000000000008000000000000000, 0x1ea8)),
    (Kind::Mul, Shape::Packed32, 0x1f80, 0x00400000ff800000400000007fc00000, 0xc077af5e0066e0b740a0000080000001, Expect::Ok(0x80f7af5eff800000412000007fc00000, 0x1f82)),
    (Kind::Mul, Shape::Packed32, 0x1d80, 0x4b7fffff3f7fffff00ffffff00ffffff, 0x7f7fffff00800001ff80000074bee47b, Expect::Ok(0x7f80000000800000ff800000363ee47a, 0x1da8)),
    (Kind::Mul, Shape::Packed32, 0x1b80, 0x007fffff000000037e8000007f800000, 0x4b000000ff8000013f8000007fffffff, Expect::Ok(0x0bfffffeffc000017e8000007fffffff, 0x1b83)),
    (Kind::Mul, Shape::Packed32, 0x3f00, 0x80000000812375e3bf800000bf800000, 0x4fccf7ad811723fa7fc0000000400000, Expect::Ok(0x80000000000000007fc0000080400000, 0x3f32)),
    (Kind::Mul, Shape::Packed32, 0x3f80, 0x3fc000003f8000017f0000003f82927c, 0x0000000100000003a71e66b500000003, Expect::Ok(0x0000000100000003e69e66b500000003, 0x3fb2)),
    (Kind::Mul, Shape::Packed32, 0x5f00, 0x80000001404000003f80000040400000, 0xc0000000340000007f8000007effffff, Expect::Ok(0x0000000234c000007f8000007f800000, 0x5f2a)),
    (Kind::Mul, Shape::Packed32, 0x7e80, 0x0b2951c2404000007f4c1cd24b000000, 0xff7fffff74f7f6807f65f113f4d5361b, Expect::Ok(0xcb2951c175b9f8e07f7fffffff7fffff, 0x7ea8)),
    (Kind::Mul, Shape::Packed32, 0x7d80, 0xff800001ffc123457fffffff7fffffff, 0x00800000ffc12345010000007f800000, Expect::Ok(0xffc00001ffc123457fffffff7fffffff, 0x7d81)),
    (Kind::Mul, Shape::Packed32, 0x1dc0, 0x40400000ff800000ffc00000ff9a68ec, 0x00000000ff7fffff3400000000ffffff, Expect::Ok(0x000000007f800000ffc00000ffda68ec, 0x1dc1)),
    (Kind::Mul, Shape::Packed32, 0x9b80, 0x3f800000010a67ea400000007fc00000, 0x800000007e800000338000007f800000, Expect::Ok(0x80000000400a67ea340000007fc00000, 0x9b80)),
    (Kind::Mul, Shape::Packed32, 0x9b80, 0x404000004020f889ff8000007f800000, 0x0098fe477f2617357effffff80000001, Expect::Fault(0x9baa)),
    (Kind::Mul, Shape::Packed32, 0x7f80, 0x4b8000007e8000007fbf07f27f800000, 0x340000008b74c2ab338000007f3cc58a, Expect::Ok(0x40000000ca74c2ab7fff07f27f800000, 0x7f81)),
    (Kind::Mul, Shape::Packed32, 0x7f40, 0xf4d9b7277fa000003f00000080000001, 0x000000000000000080000000ff800000, Expect::Fault(0x7f41)),
    (Kind::Mul, Shape::Packed32, 0x7f40, 0xffc123458b3f3c7a7f23671b7f000000, 0x800000000000000300400000ff7fffff, Expect::Ok(0xffc123458000000000000000ff7fffff, 0x7f68)),
    (Kind::Mul, Shape::Packed32, 0xfe80, 0x004000003f000000b055f32c00400000, 0x8b732409ff8000014b800000815bc407, Expect::Fault(0xfe83)),
    (Kind::Mul, Shape::Packed32, 0x7ec0, 0x80a5d172007fffff7fc000013fc00000, 0x3380000080000001808000003f7fffff, Expect::Ok(0x80000000800000007fc000013fbfffff, 0x7ef0)),
    (Kind::Mul, Shape::Packed32, 0xfd80, 0x800bad3c3fc00000406c840740000000, 0x7f7fffff404000007f635c4180000000, Expect::Ok(0xbebad3bf409000007f7fffff80000000, 0xfdaa)),
    (Kind::Mul, Shape::Packed32, 0x7bc0, 0x7fbfffff40400000c0563f7f00000001, 0x004000007f7fffff004000004b800000, Expect::Fault(0x7be9)),
    (Kind::Mul, Shape::Packed32, 0xf780, 0x40a00000017f30c77e8000007f000000, 0xffc12345ffc12345ff0ffb7a7f800001, Expect::Ok(0xffc12345ffc12345ff7fffff7fc00001, 0xf7a9)),
    (Kind::Mul, Shape::Packed32, 0x77c0, 0x00000001c00000004fb6b52280692a66, 0x7fffffff7fc000003f0000000b08a49f, Expect::Ok(0x7fffffff7fc000004f36b52280000000, 0x77c0)),
    (Kind::Mul, Shape::Packed32, 0xef80, 0x404000007f000000008000007effffff, 0xc0000000c02b3235ff7fffffff7fffff, Expect::Fault(0xefa8)),
    (Kind::Mul, Shape::Packed32, 0xef80, 0x7f8000003f000000003ede3680800000, 0x401e6020ff800001cfe9faca407ce4ec, Expect::Fault(0xefa3)),
    (Kind::Mul, Shape::Packed32, 0xef80, 0xffbfffffe995310500919ee07fc00000, 0x004000007e8000008073fea17fc00001, Expect::Fault(0xefbb)),
    (Kind::Mul, Shape::Packed32, 0x4000, 0xcfeabf65ff7fffff7fbfffff7f7fffff, 0x7fa000003e8000004040000000ffffff, Expect::Fault(0x4001)),
    (Kind::Mul, Shape::Packed32, 0x9f80, 0x7fbfffff4b7fffffc000000000800000, 0x3380000080000001bfd0e00300400000, Expect::Ok(0x7fffffff80ffffff4050e00300000000, 0x9fb3)),
    (Kind::Mul, Shape::Packed32, 0x80c0, 0xffc123457f80000100800001cfc8a96e, 0x000000018080000040a000004b000000, Expect::Fault(0x80e1)),
    (Kind::Mul, Shape::Packed32, 0x1f00, 0x7e8000004f83d4d2bf800000b061e740, 0x7effffff3f800001ff79ee3a4b7fffff, Expect::Ok(0x7f8000004f83d4d37f79ee3abc61e73f, 0x1f28)),
    (Kind::Mul, Shape::Packed32, 0x1a00, 0x7fc000003f000000ff7fffff3f9f02c1, 0x4b000000b0250cc6c00000003f7fffff, Expect::Fault(0x1a28)),
    (Kind::Mul, Shape::Packed32, 0x1f00, 0x3ffc036e0b6ced86b0647e7c4f961bf1, 0x300d73cc80d03dc1ffc12345816b0793, Expect::Ok(0x308b3fe280000000ffc123459189d017, 0x1f30)),
    (Kind::Sub, Shape::Packed64, 0xf80, 0xffefffffffffffff3cab5c54e961b8b6, 0x0002cba05fe60a6d7ff4000000000000, Expect::Fault(0xfa3)),
    (Kind::Sub, Shape::Packed64, 0x2f80, 0xfff0000000000001ffdc8719f9bb2515, 0xc009fa5564028e527fefffffffffffff, Expect::Fault(0x2fa9)),
    (Kind::Sub, Shape::Packed64, 0x3f80, 0x40000000000000007ff4000000000000, 0xbca79f28f38858edc000000000000000, Expect::Ok(0x40000000000000007ffc000000000000, 0x3fa1)),
    (Kind::Sub, Shape::Packed64, 0x3f80, 0x8003913b9e69ef6d7fefffffffffffff, 0x0007a8cdb9ddd6f77ff7ffffffffffff, Expect::Ok(0x800b3a095847c6647fffffffffffffff, 0x3f83)),
    (Kind::Sub, Shape::Packed64, 0x4f80, 0x4d073aeb14ca3bc97fe0b0ae599a99f0, 0x7ff00000000000010010000000000000, Expect::Fault(0x4fa1)),
    (Kind::Sub, Shape::Packed64, 0x1f40, 0x3ffb1cc1b38c6dab433fffffffffffff, 0x7ff8000000000001000fffffffffffff, Expect::Ok(0x7ff8000000000001433fffffffffffff, 0x1f40)),
    (Kind::Sub, Shape::Packed64, 0x9d80, 0x7ff8000000000001bff4633ff5955139, 0x001fffffffffffff81c39c8cddbdb255, Expect::Ok(0x7ff8000000000001bff4633ff5955139, 0x9da0)),
    (Kind::Sub, Shape::Packed64, 0x9d80, 0x7ff4000000000000ffe90b805cfe7fb1, 0x3ca3613e998c48ef0004c81fe09fb7bc, Expect::Ok(0x7ffc000000000000ffe90b805cfe7fb1, 0x9da3)),
    (Kind::Sub, Shape::Packed64, 0x9780, 0xc00efb06e85672ed3fd0000000000000, 0x800fffffffffffff7fd0000000000000, Expect::Ok(0xc00efb06e85672edffd0000000000000, 0x97a2)),
    (Kind::Sub, Shape::Packed64, 0xef80, 0x000fffffffffffff3fd0000000000000, 0x7fffffffffffffff3ca0000000000000, Expect::Ok(0x7fffffffffffffff3fcffffffffffffc, 0xef80)),
    (Kind::Sub, Shape::Packed64, 0x2000, 0xbcbc300fed13aea94008000000000000, 0x7fefffffffffffff0010b6525abc083d, Expect::Fault(0x2028)),
    (Kind::Sub, Shape::Packed64, 0x8000, 0x7fe188936f585064fff0000000000000, 0x8008810957fa47620000000000000003, Expect::Fault(0x8002)),
    (Kind::Sub, Shape::Packed64, 0xffc0, 0xfff7ffffffffffff4008000000000000, 0xbff0000000000000bcad109ca4a7623c, Expect::Ok(0xffffffffffffffff4008000000000000, 0xffe1)),
    (Kind::Sub, Shape::Packed64, 0xf80, 0x8009ee125c9f5405ffdbf7c495a7271e, 0x00000000000000007fefffffffffffff, Expect::Fault(0xfaa)),
    (Kind::Sub, Shape::Packed32, 0x3e80, 0xff80000180000000b026fa2d4f955ea3, 0x7fa000007fa000004b0000007f7fffff, Expect::Ok(0xffc000017fe00000cb000001ff7fffff, 0x3ea1)),
    (Kind::Sub, Shape::Packed32, 0x5f00, 0xc0000000807fffff0b7c80d1f4cafcb3, 0xff800001017c5a587f8000017effffff, Expect::Fault(0x5f03)),
    (Kind::Sub, Shape::Packed32, 0x7780, 0x40a00000ff800001800000018b3b7d32, 0x7effffffff1ca5b34b0000007effffff, Expect::Ok(0xfefffffeffc00001cb000000feffffff, 0x77a3)),
    (Kind::Sub, Shape::Packed32, 0x7780, 0xbf800000802d18647fc0000080000000, 0xbf8000007fc000014b7fffff4faedcf8, Expect::Ok(0x000000007fc000017fc00000cfaedcf8, 0x7780)),
    (Kind::Sub, Shape::Packed32, 0x9e80, 0x404000007f800001cfdf707300400000, 0x80800000c0000000ffa9a2b74b000000, Expect::Fault(0x9e83)),
    (Kind::Sub, Shape::Packed32, 0x1bc0, 0x7f5b3f5bff800001c00000007fa00000, 0x0000000100ffffff400000003f800001, Expect::Ok(0x7f5b3f5bffc00001c08000007fe00000, 0x1bc1)),
    (Kind::Sub, Shape::Packed32, 0x7f80, 0x01000000ff7fffff807fffff016688e3, 0x7fffffff7e800000000000017fffffff, Expect::Ok(0x7fffffffff7fffff808000007fffffff, 0x7faa)),
    (Kind::Sub, Shape::Packed32, 0x2000, 0x4b7fffff3f80000100000000ff7fffff, 0x00800000808000007fb4411b8166de37, Expect::Fault(0x2001)),
    (Kind::Sub, Shape::Packed32, 0x1a00, 0x7f104d94c01a48db7e8000003f7fffff, 0x402763d43f800001bf8000003f7fffff, Expect::Ok(0x7f104d94c05a48dc7e80000000000000, 0x1a20)),
    (Kind::Add, Shape::Packed64, 0xccf4, 0x7ff5881658e25097ffd0c529df1762a3, 0x002f7a20ac38d3890016976ab7242bd6, Expect::Fault(0xccf5)),
    (Kind::Add, Shape::Packed32, 0x4011, 0xcfa8dabfcfc37c34f4c78ca17fd34717, 0x285ba5c4ff8fad9c806cc8ad7fb8faee, Expect::Fault(0x4013)),
    (Kind::Add, Shape::Packed32, 0xb60b, 0x8b18833eff65a207015e493780023588, 0x806c26ab305eb958a52e3e2c0b7b4cc3, Expect::Fault(0xb60b)),
    (Kind::Add, Shape::Scalar64, 0x5774, 0x4321db7c7de25cd40018a9fc2a0598c7, 0xbcb3fd122d8bbd0e0003d33a8516207b, Expect::Ok(0x4321db7c7de25cd40018a9fc2a0598c7, 0x5774)),
    (Kind::Add, Shape::Scalar64, 0x6998, 0xc32395f99e88bf26ffdeaec2ac4ad2a1, 0x801767e50328ae320027d596738009b6, Expect::Fault(0x69b8)),
    (Kind::Add, Shape::Scalar64, 0xac36, 0x3ff3a11d6e7088b6800adfca7bb624eb, 0xfffa13499f78c1b600241953bdc42b61, Expect::Fault(0xac36)),
    (Kind::Add, Shape::Scalar32, 0xf810, 0xcf8684f67f43708dbfc6d5897ff045cf, 0xf49daf01814fe288b067469d807efbc4, Expect::Ok(0xcf8684f67f43708dbfc6d5897ff045cf, 0xf810)),
    (Kind::Add, Shape::Scalar32, 0x96e9, 0xf487ce5280ad62caf4fc70beff60f5d8, 0x74cb87398b2608eb9baf3fb4f531d577, Expect::Ok(0xf487ce5280ad62caf4fc70beff60f5e3, 0x96e9)),
    (Kind::Div, Shape::Packed64, 0xada, 0x3fff1419a9b4878b000f225b4a096218, 0x905888f653f9e4633cb54d0a8d9a30e5, Expect::Fault(0xafa)),
    (Kind::Div, Shape::Packed64, 0x1765, 0x8018383bb98107cb002485652706e4da, 0xfff5fd56df545cacbcb340b12715db5b, Expect::Fault(0x1765)),
    (Kind::Div, Shape::Packed64, 0x3e80, 0x7fda4ca43692b8f27ff1d7b890c4f78e, 0xbcab25a3d89bf013ffe81d277d0af101, Expect::Ok(0xfff00000000000007ff9d7b890c4f78e, 0x3ea9)),
    (Kind::Div, Shape::Packed64, 0x3d3f, 0xffe851e80392cba400274c94552c8338, 0xc00fadf16becbebac0037a396645b164, Expect::Ok(0x7fc890e683552183801323aecfcf7306, 0x3d3f)),
    (Kind::Div, Shape::Packed64, 0x5f80, 0xffe289f9c346b0c30004df85692b87e2, 0x9cb257eb9c9c5dd7c00b4e4be93b4723, Expect::Ok(0x7ff000000000000080016d7ee5db9679, 0x5fba)),
    (Kind::Div, Shape::Packed32, 0x637d, 0x40522c30011a6b8100e307923f828b7f, 0x3fb4625dff599e0a00696747f48f51e8, Expect::Fault(0x637d)),
    (Kind::Div, Shape::Packed32, 0xc58e, 0x09af4994cffa892d895e4a3080f5b4d8, 0x810efe4fff4bd0f0b03d93cb0b3047fa, Expect::Fault(0xc5ae)),
    (Kind::Div, Shape::Scalar64, 0x2198, 0xc006db6676725080432e124104a4fce0, 0x4008054cceeaf3ca00019b8211d679ab, Expect::Fault(0x21ba)),
    (Kind::Div, Shape::Scalar32, 0xb6e8, 0xff908aea012d7dbd8075124f7f957bdf, 0x3fded5e73f9884948b6a0e9b80f79783, Expect::Ok(0xff908aea012d7dbd8075124f7fd57bdf, 0xb6e9)),
    (Kind::Mul, Shape::Packed64, 0x2398, 0x3ca8f9d0e7b5acaf800130e407a40cbc, 0xc00b70c8c8dd7765b6442b502885d508, Expect::Fault(0x23ba)),
    (Kind::Mul, Shape::Packed64, 0x40, 0x892f52abb5c614533ff43af032dd4261, 0x7fe78f270b55af4bffea6c494829ef2a, Expect::Fault(0x68)),
    (Kind::Mul, Shape::Packed64, 0x533d, 0x000cf956f65fa11abcaa921c9a388b55, 0x00038613d2fdea1b40028db90cdac0fd, Expect::Fault(0x533f)),
    (Kind::Mul, Shape::Packed64, 0x8e3c, 0x800f3a78da0896cc801e20a10128c7d7, 0x0020cd604cf577bc000da712fa6ad2a5, Expect::Fault(0x8e3e)),
    (Kind::Mul, Shape::Packed64, 0xc19f, 0x00295d416f1a2619001cfcc38970ab8e, 0x3ca8dce0896c8d7655ba2a42c5456d45, Expect::Fault(0xc1bf)),
    (Kind::Mul, Shape::Packed64, 0xf780, 0xaf7e8d8dbdc8309fe3b0a476b79b9939, 0x3cb2466ca09e487e000a84bc9d728ef3, Expect::Ok(0xac4172dd17e9069ea3c5e1b66cc7cc7f, 0xf7a2)),
    (Kind::Mul, Shape::Packed64, 0xfd80, 0xfffc90acef73253ebca1788a50a776ea, 0x3cb23cdd33c4a3163ca9978caba201f5, Expect::Ok(0xfffc90acef73253eb95bf1d3518244d2, 0xfda0)),
    (Kind::Mul, Shape::Packed64, 0x5780, 0xffe2f0a0b7060f957ffab506f74f0d14, 0x802fe23ff1eb73888025fd6c5e5d6277, Expect::Ok(0x4022df04f946781b7ffab506f74f0d14, 0x57a0)),
    (Kind::Mul, Shape::Packed64, 0x865b, 0xfff83abefb68d261c003bf815182c29b, 0x3ca3ac4908bbb0a1ffe47369e90c771f, Expect::Fault(0x867b)),
    (Kind::Mul, Shape::Packed32, 0x308d, 0xf4a07581ff3db147ff84935180eada29, 0x4fe24f17b05f12d54fa0e32900110499, Expect::Fault(0x308f)),
    (Kind::Mul, Shape::Packed32, 0x6419, 0x40427dc4ff3a6388006c81f64f86b247, 0x4fa7a708010da56c3fe427d43f9d2f00, Expect::Fault(0x641b)),
    (Kind::Mul, Shape::Scalar64, 0x92d1, 0x4006f5238f79ce020023b3b8692a807e, 0xa01e71f1f9d086d9d364fb4e99574edf, Expect::Ok(0x4006f5238f79ce029399d61aa95c01d4, 0x92f1)),
    (Kind::Sub, Shape::Packed64, 0x1f80, 0x3ca5fd19e63d16bf40011fc715a58896, 0x4008f9a0543669737febd4a9c00407d2, Expect::Ok(0xc008f9a054366973ffebd4a9c00407d2, 0x1fa0)),
    (Kind::Sub, Shape::Packed64, 0x8f80, 0x400b72e3cdb5f069801b6b10d2298b0d, 0xfff767440e7bdf40ffeb5cee1ecf54c0, Expect::Fault(0x8fa1)),
    (Kind::Sub, Shape::Packed64, 0x8a34, 0x800f78e51d5e5cfd7ffa4bac5e557354, 0xbcbf0f0ec496fcaf7fef6889a8c034a6, Expect::Fault(0x8a36)),
    (Kind::Sub, Shape::Packed64, 0xcc25, 0x800674e7433aa1587ff5b4e9531857fe, 0xbcb1d0753385acdf3cae44d606179a2a, Expect::Fault(0xcc27)),
    (Kind::Sub, Shape::Packed64, 0x83ea, 0x3ffa0dd6e6ceb760fffb55bd910b4f35, 0x3ffbb6b77862aa723ff634cbebd20c40, Expect::Ok(0xbfba8e09193f3120fffb55bd910b4f35, 0x83ea)),
    (Kind::Sub, Shape::Packed32, 0xfccf, 0x40172786802a88fe805bc55cf4c33db2, 0x0b55586fbfcdc7acb041bda78043d3c2, Expect::Ok(0x401727853fcdc7ac3041bda7f4c33db2, 0xfcef)),
    (Kind::Sub, Shape::Packed32, 0x5a71, 0xb4f73714f4bc923c30507a5cff38bdee, 0x814289ba3008f9e58156cd68bfa5bcbd, Expect::Ok(0xb4f73713f4bc923c30507a5dff38bded, 0x5a71)),
    (Kind::Sub, Shape::Packed32, 0xfdd7, 0x0012b639b82bd20381620c8b80b3e5be, 0xb05f640e3ff34b0c809c5428ff1407a6, Expect::Ok(0x305f640ebff34c638113e2777f1407a5, 0xfdf7)),
    (Kind::Sub, Shape::Packed32, 0x5775, 0x7f7caba3b773f7730058773a40720262, 0x303523b6ff0f818ef4a871484fdcea05, Expect::Ok(0x7f7caba37f0f818e74a87148cfdcea04, 0x5775)),
    (Kind::Sub, Shape::Packed32, 0x58d3, 0x54863e7e0043985480675b09b047d9a0, 0x7f74521bfffd10340013265cff472238, Expect::Ok(0xff74521afffd1034800000007f472238, 0x58f3)),
    (Kind::Sub, Shape::Scalar64, 0x1ba3, 0x4322986e337681150005307966d04804, 0xbcb412da8febb01a00250bdc98db0960, Expect::Ok(0x4322986e337681158022739fe572e55e, 0x1ba3)),
    (Kind::Sub, Shape::Scalar32, 0xdf07, 0x4fa497fd7f8c95697f2843b28054a781, 0x80c15cdc017575987f3d7410bfad6b93, Expect::Ok(0x4fa497fd7f8c95697f2843b23fad6b93, 0xdf27)),
    (Kind::Add, Shape::Scalar64, 0x3f80, 0x87cc5813e9aaf73bfff8000000000000, 0xa0e39e0551ae76283ff8000000000000, Expect::Ok(0x87cc5813e9aaf73bfff8000000000000, 0x3f80)),
    (Kind::Add, Shape::Scalar64, 0xffc0, 0x533f9588e4d44868ffefffffffffffff, 0x5e9f93544f772719ffefffffffffffff, Expect::Ok(0x533f9588e4d44868ffefffffffffffff, 0xffe8)),
    (Kind::Add, Shape::Scalar64, 0x7f80, 0x8d13c21cbb154b31433fffffffffffff, 0xf99938b7d3212c307fefffffffffffff, Expect::Ok(0x8d13c21cbb154b317fefffffffffffff, 0x7fa0)),
    (Kind::Add, Shape::Scalar64, 0x3f80, 0x6ee1dd2bb930b0f8c000000000000000, 0xebf600b73adf1962ffefffffffffffff, Expect::Ok(0x6ee1dd2bb930b0f8fff0000000000000, 0x3fa8)),
    (Kind::Add, Shape::Scalar32, 0x7f80, 0x5c2db491ace0fdcb19b34b5f7f000000, 0xa4e4a96d268d64094c8cb09a7fc00000, Expect::Ok(0x5c2db491ace0fdcb19b34b5f7fc00000, 0x7f80)),
    (Kind::Add, Shape::Scalar32, 0x0, 0x368b102f1f215616b568422b00ffffff, 0x14606bdededdf5d035e27a0480800000, Expect::Fault(0x10)),
    (Kind::Add, Shape::Scalar32, 0x1f80, 0x1204b18e77ca6da8c7638d2640a00000, 0x6a9e4fae3a2a7d306c3b59393f800001, Expect::Ok(0x1204b18e77ca6da8c7638d2640c00000, 0x1fa0)),
    (Kind::Div, Shape::Scalar32, 0x3f80, 0xaf58cc7715407e5e296719d57e800000, 0xd7fe1c8cbddafc0bedff916e00000000, Expect::Ok(0xaf58cc7715407e5e296719d57f800000, 0x3f84)),
    (Kind::Mul, Shape::Scalar64, 0x7f80, 0xf962f8358b4c8a3bfff8000000000000, 0x5bf15e2cd0f28e3b4000000000000000, Expect::Ok(0xf962f8358b4c8a3bfff8000000000000, 0x7f80)),
    (Kind::Sub, Shape::Scalar32, 0x0, 0x7145579cde874566b37e17637f000000, 0xdbc618630cb64a0fb7f3e7ba7fa00000, Expect::Fault(0x1)),
    (Kind::Sub, Shape::Scalar32, 0x7f80, 0xf9d77d7ccc6f48c231576c3133800000, 0x795ee6d6c4eeaf0f1b0ddaf100400000, Expect::Ok(0xf9d77d7ccc6f48c231576c31337fffff, 0x7fa2)),
    (Kind::Add, Shape::Scalar64, 0x5e80, 0x53a2b271a7249fea7fefffffffffffff, 0x4deae3326cbe48a20010000000000001, Expect::Ok(0x53a2b271a7249fea7ff0000000000000, 0x5ea8)),
    (Kind::Add, Shape::Scalar64, 0x4f80, 0x5995e8c736523e754014000000000000, 0x976de820af1412567fefffffffffffff, Expect::Fault(0x4fa8)),
    (Kind::Add, Shape::Scalar64, 0x7f00, 0xff2b75d6d1e7f054bff0000000000000, 0x0c2cf22a0b2a0d977fdfffffffffffff, Expect::Ok(0xff2b75d6d1e7f0547fdffffffffffffe, 0x7f20)),
    (Kind::Add, Shape::Scalar64, 0x1f40, 0x915559c4117031d54008000000000000, 0x14ed8c2fc136a76b8000000000000001, Expect::Ok(0x915559c4117031d54008000000000000, 0x1f40)),
    (Kind::Add, Shape::Scalar64, 0x1fbf, 0xf756cdfbaa1e8f533fefffffffffffff, 0x1e93d6f081057eef0000000000000003, Expect::Ok(0xf756cdfbaa1e8f533fefffffffffffff, 0x1fbf)),
    (Kind::Add, Shape::Scalar64, 0x1700, 0x72a4982683b7960f0020000000000000, 0x49e37db33e1947af0000000000000003, Expect::Ok(0x72a4982683b7960f0020000000000002, 0x1722)),
    (Kind::Add, Shape::Scalar32, 0x5b80, 0x27641130e8c40a56ae6b995a80000000, 0xb45d6596611f34f38cc46515ff7fffff, Expect::Ok(0x27641130e8c40a56ae6b995aff7fffff, 0x5b80)),
    (Kind::Add, Shape::Scalar32, 0x5780, 0xd7bd0eed24ed252935969142007fffff, 0x02d5564658058e284db165cf7f7fffff, Expect::Ok(0xd7bd0eed24ed2529359691427f800000, 0x57aa)),
    (Kind::Add, Shape::Scalar32, 0x6f80, 0x0da54f8996aa21d260b34f1034000000, 0x67e8100d1ca29b4af7b7f90a3f800000, Expect::Ok(0x0da54f8996aa21d260b34f103f800001, 0x6f80)),
    (Kind::Add, Shape::Scalar32, 0x9f00, 0xf95531d49790ba56aa288d0540400000, 0x4e7f8b1f761b7de3d831236a7effffff, Expect::Ok(0xf95531d49790ba56aa288d057effffff, 0x9f20)),
    (Kind::Add, Shape::Scalar32, 0x1f40, 0x93dd50a06e335fcb3bd7a74d7f7fffff, 0x16729c2c673165899918be4d7f000000, Expect::Ok(0x93dd50a06e335fcb3bd7a74d7f800000, 0x1f68)),
    (Kind::Add, Shape::Scalar32, 0x9d80, 0xd26e381ca26519306ac19543007fffff, 0xb29c2b4cebbf0fca5ae35cd680000001, Expect::Ok(0xd26e381ca26519306ac1954300000000, 0x9db2)),
    (Kind::Add, Shape::Scalar32, 0xfd80, 0xcbbac55de12d4c7f7bb57dfa7fbfffff, 0x6bd3cdb64451a1b896687b617f800001, Expect::Ok(0xcbbac55de12d4c7f7bb57dfa7fffffff, 0xfd81)),
    (Kind::Add, Shape::Scalar32, 0x8000, 0x4ea677ce14a1368c9f209e0700000000, 0x19a552d5f007820051874b0d7effffff, Expect::Ok(0x4ea677ce14a1368c9f209e077effffff, 0x8000)),
    (Kind::Div, Shape::Scalar64, 0x1f00, 0x3ba4ef2a4fd02e4a7fdfffffffffffff, 0x0430f686160e7c6a3ca0000000000000, Expect::Ok(0x3ba4ef2a4fd02e4a7ff0000000000000, 0x1f28)),
    (Kind::Div, Shape::Scalar64, 0x5e80, 0xb5fb4da89529b7337ff8000000000001, 0xd226e38d3b31ab72fff0000000000000, Expect::Ok(0xb5fb4da89529b7337ff8000000000001, 0x5e80)),
    (Kind::Div, Shape::Scalar64, 0x5780, 0x75ebce7a570275353fd0000000000000, 0x425acfdc133b415b7fd0000000000000, Expect::Fault(0x5790)),
    (Kind::Div, Shape::Scalar64, 0x7e80, 0x5cfb8058984430427fefffffffffffff, 0x5ab7aab24ff6cf150010000000000000, Expect::Ok(0x5cfb8058984430427fefffffffffffff, 0x7ea8)),
    (Kind::Div, Shape::Scalar64, 0x7780, 0xbb6e277b424d10913ff0000000000001, 0xa54fb858fd295d8c000fffffffffffff, Expect::Ok(0xbb6e277b424d10917fd0000000000002, 0x77a2)),
    (Kind::Div, Shape::Scalar64, 0x7780, 0xcb6be082dc09aa700020000000000000, 0xe7a212ce726848444340000000000000, Expect::Fault(0x7790)),
    (Kind::Div, Shape::Scalar64, 0x1f40, 0x60a59621241601cac000000000000000, 0xaf93453b202185127fdfffffffffffff, Expect::Ok(0x60a59621241601ca8010000000000001, 0x1f60)),
    (Kind::Div, Shape::Scalar64, 0x9d80, 0x60194c0fdcd85d833fefffffffffffff, 0xd6b531e374ac31420008000000000000, Expect::Ok(0x60194c0fdcd85d837fdfffffffffffff, 0x9d82)),
    (Kind::Div, Shape::Scalar64, 0x9d80, 0x23ab5b779230883c3ff0000000000001, 0x351c2d908ddad0fa3fefffffffffffff, Expect::Ok(0x23ab5b779230883c3ff0000000000002, 0x9da0)),
    (Kind::Div, Shape::Scalar64, 0xfc0, 0xbe8063c4c48be509433fffffffffffff, 0xbeba9f0ec5815bf40000000000000001, Expect::Ok(0xbe8063c4c48be5097ff0000000000000, 0xfc4)),
    (Kind::Div, Shape::Scalar64, 0x7f40, 0xa3fa91b3bd047a637fd0000000000000, 0xa1b0d64d22d409013ff8000000000000, Expect::Ok(0xa3fa91b3bd047a637fc5555555555555, 0x7f60)),
    (Kind::Div, Shape::Scalar64, 0xfd80, 0xd4fe65e8d4ad7f7c7fefffffffffffff, 0x38c99b08bf38bfc53ca0000000000000, Expect::Ok(0xd4fe65e8d4ad7f7c7fefffffffffffff, 0xfda8)),
    (Kind::Div, Shape::Scalar64, 0xf780, 0x612d0db94d58644c3fd0000000000000, 0xcffe46fd3fa40fb43ff8000000000000, Expect::Ok(0x612d0db94d58644c3fc5555555555555, 0xf7a0)),
    (Kind::Div, Shape::Scalar64, 0xf780, 0xfa36ab8b6a6e8c014014000000000000, 0xc8086b76532c83390000000000000001, Expect::Ok(0xfa36ab8b6a6e8c017fefffffffffffff, 0xf7aa)),
    (Kind::Div, Shape::Scalar64, 0xf780, 0xed2cfd70f54cdc853cb0000000000000, 0x7a22d4e620160db77fefffffffffffff, Expect::Fault(0xf7b0)),
    (Kind::Div, Shape::Scalar64, 0x4000, 0x05bd360510351e480000000000000003, 0xa145b8e5bc67c4468000000000000000, Expect::Fault(0x4004)),
    (Kind::Div, Shape::Scalar64, 0x8000, 0xac56c3857a0ddcd17fe0000000000000, 0xc2db87b20665d6893fefffffffffffff, Expect::Fault(0x8020)),
    (Kind::Div, Shape::Scalar64, 0xfbf, 0xb0fe53571a4d6d040000000000000001, 0x4f1f34a3324e16f13ff8000000000000, Expect::Fault(0xfbf)),
    (Kind::Div, Shape::Scalar64, 0xf80, 0x175ae0b14572562dffefffffffffffff, 0x57b30fe6ee509d240010000000000001, Expect::Fault(0xfa8)),
    (Kind::Div, Shape::Scalar64, 0x380, 0x846f98dde76c9a164330000000000000, 0x678536193b79bf910010000000000001, Expect::Fault(0x3a8)),
    (Kind::Div, Shape::Scalar64, 0x3d80, 0xe1a0cd10f8319c2cffefffffffffffff, 0x4b52d6259cec98ef3fefffffffffffff, Expect::Ok(0xe1a0cd10f8319c2cfff0000000000000, 0x3da8)),
    (Kind::Div, Shape::Scalar64, 0x3b80, 0x39b8104b2f72fa7e0000000000000001, 0x43eeb7e5b31168e13ff0000000000001, Expect::Ok(0x39b8104b2f72fa7e0000000000000000, 0x3bb2)),
    (Kind::Div, Shape::Scalar32, 0x5e80, 0x23e1af62381cb18d12d6a65c3f800001, 0x45c6d8f49fd95c578522f6a47f7fffff, Expect::Ok(0x23e1af62381cb18d12d6a65c00200001, 0x5eb0)),
    (Kind::Div, Shape::Scalar32, 0x7f00, 0x7792b0ac677db91aa502398b7f800001, 0x2f04884b66fc8347158db2df7fc00001, Expect::Fault(0x7f01)),
    (Kind::Div, Shape::Scalar32, 0x7d80, 0x9640347c836a9dbb9848eceb00000001, 0x1537ca7b8accdadbd4ccc0484b7fffff, Expect::Ok(0x9640347c836a9dbb9848eceb00000000, 0x7db2)),
    (Kind::Div, Shape::Scalar32, 0x7d80, 0x15fefba7d3fb4f3485a8c25b40400000, 0x31753e0b2f06cf8fa2f7ce0b00800001, Expect::Ok(0x15fefba7d3fb4f3485a8c25b7f3ffffe, 0x7da0)),
    (Kind::Div, Shape::Scalar32, 0x6f80, 0x305b6a0b21fb89991c3d3c6300000003, 0x15b583737a3e2d002ae81c82c0000000, Expect::Fault(0x6fb2)),
    (Kind::Div, Shape::Scalar32, 0x6f80, 0x3ee7d8fb81913cda99c9e0e04b7fffff, 0x2429e6d5bf971f03b7e160e000ffffff, Expect::Fault(0x6fa8)),
    (Kind::Div, Shape::Scalar32, 0x9f00, 0x09556d67773d04ea2b20a957ff800001, 0x3b60ea6486f6414bf44e27d5c0000000, Expect::Fault(0x9f01)),
    (Kind::Div, Shape::Scalar32, 0x9e80, 0xee4425d860adad710d5d851000800001, 0x704a9dd8e86a834e4d3aa12eff7fffff, Expect::Ok(0xee4425d860adad710d5d851080000000, 0x9eb0)),
    (Kind::Div, Shape::Scalar32, 0x9b80, 0x5e2eaa89d523ffe2f125bb614b800000, 0xd4017266b171c1cc2e50f2efff800000, Expect::Ok(0x5e2eaa89d523ffe2f125bb6180000000, 0x9b80)),
    (Kind::Div, Shape::Scalar32, 0x9780, 0x27891aeda5d97ca07f4bc6d87f000000, 0x381fcfb8e1a495c32887e43000800001, Expect::Ok(0x27891aeda5d97ca07f4bc6d87f800000, 0x97a8)),
    (Kind::Div, Shape::Scalar32, 0x1b80, 0xab71abf7ace5486459023482ff7fffff, 0x8c914b7ab1ef9876556c490f007fffff, Expect::Fault(0x1baa)),
    (Kind::Div, Shape::Scalar32, 0x1b80, 0x99222152519ef5cde9c6042140400000, 0x00fcfe78836e49ee98d6391500000000, Expect::Ok(0x99222152519ef5cde9c604217f800000, 0x1b84)),
    (Kind::Div, Shape::Scalar32, 0xfd80, 0x6188bb59c5c430c3d59c7caa40400000, 0x62da94f5d133ad20d845cc2e3f800001, Expect::Ok(0x6188bb59c5c430c3d59c7caa403ffffe, 0xfda0)),
    (Kind::Div, Shape::Scalar32, 0xfd80, 0x9b29dd96197406591d55adce807fffff, 0x6204cf3325d598a31db0692b00400000, Expect::Ok(0x9b29dd96197406591d55adcebffffffe, 0xfd82)),
    (Kind::Div, Shape::Scalar32, 0x77c0, 0x3f4d52b0957beec015d264fd34000000, 0x666bb110a94cc25528a1aef180000000, Expect::Ok(0x3f4d52b0957beec015d264fdff800000, 0x77c4)),
    (Kind::Div, Shape::Scalar32, 0x8000, 0x67b380bde255af6e6ff9bba2ff7fffff, 0x5b3e2d5bd3f1495d01987ae801000000, Expect::Fault(0x8008)),
    (Kind::Div, Shape::Scalar32, 0x1f95, 0x4209635587bb71f937a6f22400ffffff, 0x831a1dfaeff4ff98d76c045b00000003, Expect::Ok(0x4209635587bb71f937a6f2244aaaaaaa, 0x1f97)),
    (Kind::Div, Shape::Scalar32, 0xf80, 0xb1c38b53386bde0eed7430d17e800000, 0x871c46999babfbea8654f9e033800000, Expect::Fault(0xfa8)),
    (Kind::Div, Shape::Scalar32, 0x1a00, 0x22f469bf78e3be8dfaa1e5c600800001, 0x58f6330c8c78beefcc44acd47e800000, Expect::Ok(0x22f469bf78e3be8dfaa1e5c600000000, 0x1a30)),
    (Kind::Div, Shape::Scalar32, 0x1a00, 0x80ac5be6ce98cde425e86c3d80000001, 0xc3b1e327ff79c076c207d0f440400000, Expect::Fault(0x1a02)),
    (Kind::Mul, Shape::Scalar64, 0x5d80, 0x7ee755618abec2077fefffffffffffff, 0xa2fcc6515be0dcde4000000000000000, Expect::Ok(0x7ee755618abec2077ff0000000000000, 0x5da8)),
    (Kind::Mul, Shape::Scalar64, 0x5b80, 0xf80bd59affb90d0b4008000000000000, 0x5267f6a30e4ebd8b7fdfffffffffffff, Expect::Fault(0x5ba8)),
    (Kind::Mul, Shape::Scalar64, 0x9f00, 0x93fc545e6e418412ffefffffffffffff, 0x3c7a5cb2e8ccc6aa433fffffffffffff, Expect::Ok(0x93fc545e6e418412fff0000000000000, 0x9f28)),
    (Kind::Mul, Shape::Scalar64, 0x1ec0, 0x788f2bf81bf185a07fdfffffffffffff, 0xb2d81b0208da2c127fe0000000000000, Expect::Ok(0x788f2bf81bf185a07ff0000000000000, 0x1ee8)),
    (Kind::Mul, Shape::Scalar64, 0xff00, 0xff432860fa1395f23fe0000000000000, 0x79416a990ce0168c3ff8000000000000, Expect::Ok(0xff432860fa1395f23fe8000000000000, 0xff00)),
    (Kind::Mul, Shape::Scalar64, 0x7dc0, 0x5b61ae9f73554dd8ffefffffffffffff, 0x7a56707f2ed5423c0020000000000000, Expect::Ok(0x5b61ae9f73554dd8c01fffffffffffff, 0x7dc0)),
    (Kind::Mul, Shape::Scalar64, 0x7bc0, 0x3d43b72a3af4b1a7fff0000000000001, 0xe7733bd73b0c8a127fffffffffffffff, Expect::Ok(0x3d43b72a3af4b1a7fff8000000000001, 0x7bc1)),
    (Kind::Mul, Shape::Scalar64, 0x40, 0x886bfb83a2ddf3f17ff7ffffffffffff, 0x33b6505f99c603af7fd0000000000000, Expect::Fault(0x41)),
    (Kind::Mul, Shape::Scalar64, 0xf80, 0xfbe27171b5ecf52d4008000000000000, 0xd2cb8aa33f1a51da800fffffffffffff, Expect::Fault(0xfa2)),
    (Kind::Mul, Shape::Scalar64, 0x380, 0xd86e493816b38b014340000000000000, 0x4e8ab7c8f15c1f344014000000000000, Expect::Ok(0xd86e493816b38b014364000000000000, 0x380)),
    (Kind::Mul, Shape::Scalar64, 0x3d80, 0xddada910afcb031d7fe0000000000000, 0xf95c472aaaed01927fd0000000000000, Expect::Ok(0xddada910afcb031d7fefffffffffffff, 0x3da8)),
    (Kind::Mul, Shape::Scalar32, 0x5d80, 0x8dfe2bff74cefa0dc84b07297f000000, 0x5e6aca0f1e47346f449f4d7affc12345, Expect::Ok(0x8dfe2bff74cefa0dc84b0729ffc12345, 0x5d80)),
    (Kind::Mul, Shape::Scalar32, 0x5b80, 0xa7f6cf016d45875c8deab7fe80000001, 0x944c8be32edfce4e6739cab17f800000, Expect::Ok(0xa7f6cf016d45875c8deab7feff800000, 0x5b82)),
    (Kind::Mul, Shape::Scalar32, 0x7b80, 0xdb3e17d7618a581948e4ff94ffc00000, 0x9c398a790742a614f377bd363f7fffff, Expect::Ok(0xdb3e17d7618a581948e4ff94ffc00000, 0x7b80)),
    (Kind::Mul, Shape::Scalar32, 0x9e80, 0xaa5555d5ebd03aade369d28a7fffffff, 0xe9d2b4f79e9824da3b136cdb7fc00001, Expect::Ok(0xaa5555d5ebd03aade369d28a7fffffff, 0x9e80)),
    (Kind::Mul, Shape::Scalar32, 0x9d80, 0x587de7c185869a753820550900000003, 0x8759a54925c6304527937fa24b7fffff, Expect::Ok(0x587de7c185869a753820550901bfffff, 0x9da2)),
    (Kind::Mul, Shape::Scalar32, 0x1dc0, 0x3eb8835e6ff41064ece6ea077e800000, 0x40ea9cb8a5d66edd6681a9107f000000, Expect::Ok(0x3eb8835e6ff41064ece6ea077f800000, 0x1de8)),
    (Kind::Mul, Shape::Scalar32, 0x1bc0, 0x02e4ea45992bb2fb20e38fc201000000, 0xb553267a4ad5d3f72616041a80800000, Expect::Ok(0x02e4ea45992bb2fb20e38fc280000000, 0x1bf0)),
    (Kind::Mul, Shape::Scalar32, 0xfc0, 0x201fa2095379fa59e86640303e800000, 0x688e5fa4b659015919b7682a00800001, Expect::Fault(0xff0)),
    (Kind::Mul, Shape::Scalar32, 0xfb80, 0xb230de081806525d41ce07d24b800000, 0x33ebe72d73c839ee640732bf00800000, Expect::Ok(0xb230de081806525d41ce07d20c800000, 0xfb80)),
    (Kind::Mul, Shape::Scalar32, 0x6000, 0x8c79f7b164e51a558b06d86a00000001, 0x0cc5fd6fbb82848dde48d7d77f7fffff, Expect::Fault(0x6002)),
    (Kind::Mul, Shape::Scalar32, 0x40, 0xdf1eeb9c280c1a0a32654c5080000001, 0xbfe1f8de3e61b0529fe3fc3d807fffff, Expect::Ok(0xdf1eeb9c280c1a0a32654c5000000000, 0x40)),
    (Kind::Mul, Shape::Scalar32, 0x1700, 0xf01e46b210bddc4959e3dc423f7fffff, 0xb431fb01de0c5695af69bef87f7fffff, Expect::Ok(0xf01e46b210bddc4959e3dc427f7ffffe, 0x1720)),
    (Kind::Mul, Shape::Scalar32, 0x380, 0xcba8cc9aefe139ca1dd2ae7d00ffffff, 0x472d3377c03be1f90117938600800000, Expect::Fault(0x390)),
    (Kind::Mul, Shape::Scalar32, 0x3f00, 0xa567fe740538da6c725283653fc00000, 0x1792ebef0645ddac32f8b71e807fffff, Expect::Ok(0xa567fe740538da6c7252836580bfffff, 0x3f22)),
    (Kind::Mul, Shape::Scalar32, 0x3b80, 0xee5b5724839f6a1e4683fedb7f000000, 0x4d7426547d45f1e654441b457e800000, Expect::Fault(0x3b88)),
    (Kind::Mul, Shape::Scalar32, 0x3b80, 0x728efc53577334170b0c95814b7fffff, 0xcf0fc076ed4abe61c6583ab1ff7fffff, Expect::Fault(0x3ba8)),
    (Kind::Mul, Shape::Scalar32, 0x3b80, 0x2d6dfa4a7ff9ab33ea025fabbf800000, 0x221145be0fd31039a14cf140007fffff, Expect::Ok(0x2d6dfa4a7ff9ab33ea025fab807fffff, 0x3b82)),
    (Kind::Sub, Shape::Scalar64, 0x2f80, 0xfa9b62666df609c1000fffffffffffff, 0xe555ca243b7429a40000000000000003, Expect::Ok(0xfa9b62666df609c1000ffffffffffffc, 0x2f82)),
    (Kind::Sub, Shape::Scalar64, 0x5e80, 0x036fb8c7b3387cc27fefffffffffffff, 0xf239de617159da3d7ff0000000000001, Expect::Ok(0x036fb8c7b3387cc27ff8000000000001, 0x5e81)),
    (Kind::Sub, Shape::Scalar64, 0x6f80, 0x615a51a9a82aa920000fffffffffffff, 0x2b5394a0adf11d8b000fffffffffffff, Expect::Ok(0x615a51a9a82aa9200000000000000000, 0x6f82)),
    (Kind::Sub, Shape::Scalar64, 0x9f00, 0xc0642009cf71e690800fffffffffffff, 0x7d640e9b648258280008000000000000, Expect::Ok(0xc0642009cf71e6908017ffffffffffff, 0x9f02)),
    (Kind::Sub, Shape::Scalar64, 0xfe80, 0xb51d3cbaa18e3d500000000000000003, 0x219325427dd659020000000000000001, Expect::Fault(0xfe82)),
    (Kind::Sub, Shape::Scalar64, 0x2000, 0xca39588a4cf00de37ff0000000000000, 0xa094d815d15846ba0000000000000003, Expect::Fault(0x2002)),
    (Kind::Sub, Shape::Scalar64, 0x8000, 0x53d7a56a22d14e84ffefffffffffffff, 0x2860feabf48653837fd0000000000000, Expect::Fault(0x8028)),
    (Kind::Sub, Shape::Scalar32, 0x5e80, 0x3980cb1892a97de60de0a53e7fa00000, 0xd37e3cbe0ad86ff77696c1b1ff800000, Expect::Ok(0x3980cb1892a97de60de0a53e7fe00000, 0x5e81)),
    (Kind::Sub, Shape::Scalar32, 0x9f00, 0x57a6c13be4ea1def7fbf338600400000, 0x3f43ec0e007bd2b71ebc593b01000000, Expect::Ok(0x57a6c13be4ea1def7fbf338680c00000, 0x9f02)),
    (Kind::Sub, Shape::Scalar32, 0x9d80, 0x51091865d77af2d2677673cc7f800001, 0xd9a621bed783f4b2f169d58b3f000000, Expect::Ok(0x51091865d77af2d2677673cc7fc00001, 0x9d81)),
    (Kind::Sub, Shape::Scalar32, 0x9b80, 0x2e80a3031938118c0ea7f4fdbf800000, 0x59f4dd80bbe250941fb98a8e00000001, Expect::Ok(0x2e80a3031938118c0ea7f4fdbf800000, 0x9ba2)),
    (Kind::Sub, Shape::Scalar32, 0x4000, 0xb0a38810721162bac553e0b340000000, 0x19b799c09c6ad859cee65c017f800001, Expect::Fault(0x4001)),
    (Kind::Sub, Shape::Scalar32, 0x80c0, 0x38f00bf4c521faa4325d63793f7fffff, 0x0bebf408600da8a3502ee5084b000000, Expect::Fault(0x80e0)),
    (Kind::Sub, Shape::Scalar32, 0xf80, 0x885cadc005d317e4f1899e4d00000003, 0x2a947fe2ce600f682da0b6704b800000, Expect::Fault(0xfa2)),
    (Kind::Sub, Shape::Scalar32, 0x3d80, 0x7fc230636c6a6bc5f315995200000000, 0x8cf7600dc42a9971580882f67effffff, Expect::Ok(0x7fc230636c6a6bc5f3159952feffffff, 0x3d80)),
];

#[rustfmt::skip]
const HAND: &[Hand] = &[
    (
        "0.1f + 0.2f, RC=rn: 0.1f = 0x3dcccccd, 0.2f = 0x3e4ccccd, the exact sum lies between 0x3e999999 and 0x3e99999a",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0xf918446eb790a52b9a8c06de3dcccccd,
        0x0c4c387d68e4eb7d08b023b33e4ccccd,
        Expect::Ok(0xf918446eb790a52b9a8c06de3e99999a, 0x1fa0),
    ),
    (
        "1/3 (single), RC=rn: inexact, rounded by the mode",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xe2968a1d400a84e4fdb76b883f800000,
        0xe84ede06d54a081bf1dceef640400000,
        Expect::Ok(0xe2968a1d400a84e4fdb76b883eaaaaab, 0x1fa0),
    ),
    (
        "-1/3 (single), RC=rn",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x93243d42a7e68984c6743c8bbf800000,
        0x5a8b926eabfab5f66760abc440400000,
        Expect::Ok(0x93243d42a7e68984c6743c8bbeaaaaab, 0x1fa0),
    ),
    (
        "MAX + MAX masked overflow, RC=rn: infinity or the largest finite number (SDM Table 4-11), OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x2ec9dc43933faf617853108c7f7fffff,
        0x7239b3603b3163c395294b857f7fffff,
        Expect::Ok(0x2ec9dc43933faf617853108c7f800000, 0x1fa8),
    ),
    (
        "-MAX - MAX masked overflow, RC=rn",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x1609de8adeeb5752e963ef52ff7fffff,
        0xfc976859dcf5bc5fd1c3ce147f7fffff,
        Expect::Ok(0x1609de8adeeb5752e963ef52ff800000, 0x1fa8),
    ),
    (
        "1.0 - 1.0, RC=rn: exact zero is +0 except toward -inf",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x4c21cc9d0f34b43a8ade088e3f800000,
        0x3f9a084c54395e2217b8cf7f3f800000,
        Expect::Ok(0x4c21cc9d0f34b43a8ade088e00000000, 0x1f80),
    ),
    (
        "(+0) + (-0), RC=rn",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0xacf2438319c5ef7e68f3d84100000000,
        0xd990c4015b34a975a6da1a7280000000,
        Expect::Ok(0xacf2438319c5ef7e68f3d84100000000, 0x1f80),
    ),
    (
        "(-0) + (-0), RC=rn",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0xacdc3028de9e15b7a810f44f80000000,
        0x32602b09f369163840674b8280000000,
        Expect::Ok(0xacdc3028de9e15b7a810f44f80000000, 0x1f80),
    ),
    (
        "3 * 0.1f, RC=rn: inexact product",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0x5e390439ee2558dec30705f640400000,
        0x70f07b8a8136920f2658a2af3dcccccd,
        Expect::Ok(0x5e390439ee2558dec30705f63e99999a, 0x1fa0),
    ),
    (
        "min normal * 0.75, RC=rn: tiny and inexact, masked underflow, UE and PE, denormal result",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0x36ddcc35273ade2002eee4f600800001,
        0xe9218cf7b9009d37449be3863f400000,
        Expect::Ok(0x36ddcc35273ade2002eee4f600600001, 0x1fb0),
    ),
    (
        "0.1f + 0.2f, RC=rd: 0.1f = 0x3dcccccd, 0.2f = 0x3e4ccccd, the exact sum lies between 0x3e999999 and 0x3e99999a",
        Kind::Add,
        Shape::Scalar32,
        0x3f80,
        0x8900387911b94d4acde0c3223dcccccd,
        0xe7f7f3738cc16124afbe6cc93e4ccccd,
        Expect::Ok(0x8900387911b94d4acde0c3223e999999, 0x3fa0),
    ),
    (
        "1/3 (single), RC=rd: inexact, rounded by the mode",
        Kind::Div,
        Shape::Scalar32,
        0x3f80,
        0x68cf4f0dea3cfb7b0133ab333f800000,
        0xb4d5e7ad69efcb41eb98393140400000,
        Expect::Ok(0x68cf4f0dea3cfb7b0133ab333eaaaaaa, 0x3fa0),
    ),
    (
        "-1/3 (single), RC=rd",
        Kind::Div,
        Shape::Scalar32,
        0x3f80,
        0x2bb77da39046fe776943da29bf800000,
        0x62d50aee614cf171483b2aec40400000,
        Expect::Ok(0x2bb77da39046fe776943da29beaaaaab, 0x3fa0),
    ),
    (
        "MAX + MAX masked overflow, RC=rd: infinity or the largest finite number (SDM Table 4-11), OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x3f80,
        0x73bdfa6fb88afc03f25dc3427f7fffff,
        0x1f35d30eddf685f9f58a661f7f7fffff,
        Expect::Ok(0x73bdfa6fb88afc03f25dc3427f7fffff, 0x3fa8),
    ),
    (
        "-MAX - MAX masked overflow, RC=rd",
        Kind::Sub,
        Shape::Scalar32,
        0x3f80,
        0x7640ec3cee8bf53e24b5d841ff7fffff,
        0xc68ae96439209552491061367f7fffff,
        Expect::Ok(0x7640ec3cee8bf53e24b5d841ff800000, 0x3fa8),
    ),
    (
        "1.0 - 1.0, RC=rd: exact zero is +0 except toward -inf",
        Kind::Sub,
        Shape::Scalar32,
        0x3f80,
        0x7954546c76e7a65299e3f56e3f800000,
        0x53a7979d02688c6d0f77f43c3f800000,
        Expect::Ok(0x7954546c76e7a65299e3f56e80000000, 0x3f80),
    ),
    (
        "(+0) + (-0), RC=rd",
        Kind::Add,
        Shape::Scalar32,
        0x3f80,
        0x03ed54d4459826d581673ed800000000,
        0xd62c47d63fa54844982130f680000000,
        Expect::Ok(0x03ed54d4459826d581673ed880000000, 0x3f80),
    ),
    (
        "(-0) + (-0), RC=rd",
        Kind::Add,
        Shape::Scalar32,
        0x3f80,
        0x1ba81d9b16bd47b11f0a583280000000,
        0xa0e046672424e260f77b9af380000000,
        Expect::Ok(0x1ba81d9b16bd47b11f0a583280000000, 0x3f80),
    ),
    (
        "3 * 0.1f, RC=rd: inexact product",
        Kind::Mul,
        Shape::Scalar32,
        0x3f80,
        0x4747eaf30a679e1056272f5540400000,
        0xa0599cb318f0b4727dcf76263dcccccd,
        Expect::Ok(0x4747eaf30a679e1056272f553e999999, 0x3fa0),
    ),
    (
        "min normal * 0.75, RC=rd: tiny and inexact, masked underflow, UE and PE, denormal result",
        Kind::Mul,
        Shape::Scalar32,
        0x3f80,
        0xc018c5eded102b31b1d62e6400800001,
        0xb2e45643ad87ff3aac8b2e583f400000,
        Expect::Ok(0xc018c5eded102b31b1d62e6400600000, 0x3fb0),
    ),
    (
        "0.1f + 0.2f, RC=ru: 0.1f = 0x3dcccccd, 0.2f = 0x3e4ccccd, the exact sum lies between 0x3e999999 and 0x3e99999a",
        Kind::Add,
        Shape::Scalar32,
        0x5f80,
        0x09faf09c5412fb1a10fd26533dcccccd,
        0xa40014ec40f600431bc19fe73e4ccccd,
        Expect::Ok(0x09faf09c5412fb1a10fd26533e99999a, 0x5fa0),
    ),
    (
        "1/3 (single), RC=ru: inexact, rounded by the mode",
        Kind::Div,
        Shape::Scalar32,
        0x5f80,
        0x87d2caf9132bf077d56cf9863f800000,
        0x3d8a4aa0cd4765fb4e761f6b40400000,
        Expect::Ok(0x87d2caf9132bf077d56cf9863eaaaaab, 0x5fa0),
    ),
    (
        "-1/3 (single), RC=ru",
        Kind::Div,
        Shape::Scalar32,
        0x5f80,
        0xf58d0c8ca2f9a6ee1667ff5dbf800000,
        0x27fa55a64d3a7ef31d88e87140400000,
        Expect::Ok(0xf58d0c8ca2f9a6ee1667ff5dbeaaaaaa, 0x5fa0),
    ),
    (
        "MAX + MAX masked overflow, RC=ru: infinity or the largest finite number (SDM Table 4-11), OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x5f80,
        0x66d919f770bac3ee7cea15f67f7fffff,
        0x5bc5f2d0a354044ffd0e42297f7fffff,
        Expect::Ok(0x66d919f770bac3ee7cea15f67f800000, 0x5fa8),
    ),
    (
        "-MAX - MAX masked overflow, RC=ru",
        Kind::Sub,
        Shape::Scalar32,
        0x5f80,
        0x904bd02d12fd36aef7796e4eff7fffff,
        0xd5fa5f8e7d55538c19a155ae7f7fffff,
        Expect::Ok(0x904bd02d12fd36aef7796e4eff7fffff, 0x5fa8),
    ),
    (
        "1.0 - 1.0, RC=ru: exact zero is +0 except toward -inf",
        Kind::Sub,
        Shape::Scalar32,
        0x5f80,
        0x48156f77a43a84e9a14a5e723f800000,
        0xe39b7cb15edb601cd7a451cd3f800000,
        Expect::Ok(0x48156f77a43a84e9a14a5e7200000000, 0x5f80),
    ),
    (
        "(+0) + (-0), RC=ru",
        Kind::Add,
        Shape::Scalar32,
        0x5f80,
        0xadcb5062006fb748d06e910b00000000,
        0x65f737d7f9320e62ad7a08c480000000,
        Expect::Ok(0xadcb5062006fb748d06e910b00000000, 0x5f80),
    ),
    (
        "(-0) + (-0), RC=ru",
        Kind::Add,
        Shape::Scalar32,
        0x5f80,
        0xf941a714ba0d1bb718813f9d80000000,
        0x7c1ac9dbfb70204980f370fb80000000,
        Expect::Ok(0xf941a714ba0d1bb718813f9d80000000, 0x5f80),
    ),
    (
        "3 * 0.1f, RC=ru: inexact product",
        Kind::Mul,
        Shape::Scalar32,
        0x5f80,
        0xdb79e5f1baa14d88f6afe84e40400000,
        0xe7a50534270509c112c0e8773dcccccd,
        Expect::Ok(0xdb79e5f1baa14d88f6afe84e3e99999a, 0x5fa0),
    ),
    (
        "min normal * 0.75, RC=ru: tiny and inexact, masked underflow, UE and PE, denormal result",
        Kind::Mul,
        Shape::Scalar32,
        0x5f80,
        0xa30ec021b1397d22e0e91a3e00800001,
        0xd3c9b3afe2df8a01aa212ceb3f400000,
        Expect::Ok(0xa30ec021b1397d22e0e91a3e00600001, 0x5fb0),
    ),
    (
        "0.1f + 0.2f, RC=rz: 0.1f = 0x3dcccccd, 0.2f = 0x3e4ccccd, the exact sum lies between 0x3e999999 and 0x3e99999a",
        Kind::Add,
        Shape::Scalar32,
        0x7f80,
        0xcbac19eb3f8b1b0f8657f21e3dcccccd,
        0xff871c8558e3650dc5034cf43e4ccccd,
        Expect::Ok(0xcbac19eb3f8b1b0f8657f21e3e999999, 0x7fa0),
    ),
    (
        "1/3 (single), RC=rz: inexact, rounded by the mode",
        Kind::Div,
        Shape::Scalar32,
        0x7f80,
        0x354b0e4cd0cbed588c9de9803f800000,
        0x59f45de0ce9a4d3591ba1b1340400000,
        Expect::Ok(0x354b0e4cd0cbed588c9de9803eaaaaaa, 0x7fa0),
    ),
    (
        "-1/3 (single), RC=rz",
        Kind::Div,
        Shape::Scalar32,
        0x7f80,
        0x297e2f094d7726ef5a2ec1d7bf800000,
        0xd89e7c7920622cc29ceb0ad840400000,
        Expect::Ok(0x297e2f094d7726ef5a2ec1d7beaaaaaa, 0x7fa0),
    ),
    (
        "MAX + MAX masked overflow, RC=rz: infinity or the largest finite number (SDM Table 4-11), OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x7f80,
        0xc23d06865a07981849fdaaf67f7fffff,
        0xac69e73d971e2144ca68badb7f7fffff,
        Expect::Ok(0xc23d06865a07981849fdaaf67f7fffff, 0x7fa8),
    ),
    (
        "-MAX - MAX masked overflow, RC=rz",
        Kind::Sub,
        Shape::Scalar32,
        0x7f80,
        0x3221709703d03fa446a9ae39ff7fffff,
        0xe4dd944f2638c06818f7c9957f7fffff,
        Expect::Ok(0x3221709703d03fa446a9ae39ff7fffff, 0x7fa8),
    ),
    (
        "1.0 - 1.0, RC=rz: exact zero is +0 except toward -inf",
        Kind::Sub,
        Shape::Scalar32,
        0x7f80,
        0x312931e4d50ab84c0cc50a713f800000,
        0x006f141a25ab27d37a1d270a3f800000,
        Expect::Ok(0x312931e4d50ab84c0cc50a7100000000, 0x7f80),
    ),
    (
        "(+0) + (-0), RC=rz",
        Kind::Add,
        Shape::Scalar32,
        0x7f80,
        0x4014280cd1abbfcab265349c00000000,
        0x9d7699cc8baa959ec54c188180000000,
        Expect::Ok(0x4014280cd1abbfcab265349c00000000, 0x7f80),
    ),
    (
        "(-0) + (-0), RC=rz",
        Kind::Add,
        Shape::Scalar32,
        0x7f80,
        0xe649fc277a7cca42d304638280000000,
        0x14b4ac4a7e9aadf6f3e436af80000000,
        Expect::Ok(0xe649fc277a7cca42d304638280000000, 0x7f80),
    ),
    (
        "3 * 0.1f, RC=rz: inexact product",
        Kind::Mul,
        Shape::Scalar32,
        0x7f80,
        0xe148ca23d76efdb2d6bf1eb240400000,
        0xec4768d12e93609b8a82bcf43dcccccd,
        Expect::Ok(0xe148ca23d76efdb2d6bf1eb23e999999, 0x7fa0),
    ),
    (
        "min normal * 0.75, RC=rz: tiny and inexact, masked underflow, UE and PE, denormal result",
        Kind::Mul,
        Shape::Scalar32,
        0x7f80,
        0x94183a278478ce576ee4472900800001,
        0x3d1923be68dbdd7d35a2eee03f400000,
        Expect::Ok(0x94183a278478ce576ee4472900600000, 0x7fb0),
    ),
    (
        "1.0 + 2.0 exact: no flag",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x3a92fda3e5b9dcdac857f6493f800000,
        0xf76dfbdeec6af41a8561d81640000000,
        Expect::Ok(0x3a92fda3e5b9dcdac857f64940400000, 0x1f80),
    ),
    (
        "5 - 3 = 2: destination minus source",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x314b1ae3b46b49a9b10ef19d40a00000,
        0x11331d73b34e69855817dcb940400000,
        Expect::Ok(0x314b1ae3b46b49a9b10ef19d40000000, 0x1f80),
    ),
    (
        "3 - 5 = -2: destination minus source",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x05ba39b3e440af7cedf2a17140400000,
        0x207186b848ef180d75f7647f40a00000,
        Expect::Ok(0x05ba39b3e440af7cedf2a171c0000000, 0x1f80),
    ),
    (
        "6 / 3 = 2: destination divided by source",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x4ec56d725c4536cbe502777340c00000,
        0xb9fb5d2ef2441d23a6d803e140400000,
        Expect::Ok(0x4ec56d725c4536cbe502777340000000, 0x1f80),
    ),
    (
        "3 / 6 = 0.5",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x1d85a640ceb224b3b057195640400000,
        0x60cc1831f2d0218d005fbb0640c00000,
        Expect::Ok(0x1d85a640ceb224b3b05719563f000000, 0x1f80),
    ),
    (
        "1 / +0 = +inf, ZE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xe0501d95ac54f6f1044064523f800000,
        0x3264abb7cc005906865c4aec00000000,
        Expect::Ok(0xe0501d95ac54f6f1044064527f800000, 0x1f84),
    ),
    (
        "-1 / +0 = -inf, ZE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xa68856ea9c2a5d6edefc7b67bf800000,
        0x69e71f68173d06cbaf36394b00000000,
        Expect::Ok(0xa68856ea9c2a5d6edefc7b67ff800000, 0x1f84),
    ),
    (
        "1 / -0 = -inf, ZE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x44097118bf9320687a6cbf903f800000,
        0xa01365aebfc21ba10506efd480000000,
        Expect::Ok(0x44097118bf9320687a6cbf90ff800000, 0x1f84),
    ),
    (
        "0 / 0 = default NaN, IE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x58b08bcc17c2850263c1bf1700000000,
        0xcaf2e9ffc3b2e4abcdac73c300000000,
        Expect::Ok(0x58b08bcc17c2850263c1bf17ffc00000, 0x1f81),
    ),
    (
        "inf / inf = default NaN, IE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x3a03f1b39aee97e537d4a8297f800000,
        0x1e6747835afd0e31ec8da468ff800000,
        Expect::Ok(0x3a03f1b39aee97e537d4a829ffc00000, 0x1f81),
    ),
    (
        "inf / 0 = inf, no exception",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x2851e5adb4c7830e392d9ed97f800000,
        0x7ac07268a274be0642c6aaf100000000,
        Expect::Ok(0x2851e5adb4c7830e392d9ed97f800000, 0x1f80),
    ),
    (
        "1 / inf = 0",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0x57d804b9acdf2dee61178cce3f800000,
        0x4f927b7a90e149185ea696837f800000,
        Expect::Ok(0x57d804b9acdf2dee61178cce00000000, 0x1f80),
    ),
    (
        "0 * inf = default NaN, IE",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0x1ff45eeef1a2e2e19619c30d00000000,
        0x7c4b25b4f2c6ab2e1de608467f800000,
        Expect::Ok(0x1ff45eeef1a2e2e19619c30dffc00000, 0x1f81),
    ),
    (
        "inf * -2 = -inf",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0xc37df01ec8ab36b4be4670b77f800000,
        0xa4e75240a512cb6a962bc70dc0000000,
        Expect::Ok(0xc37df01ec8ab36b4be4670b7ff800000, 0x1f80),
    ),
    (
        "inf - inf = default NaN, IE",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x77d298c1fc8e2d496296cd5c7f800000,
        0xf17f9530b609670bd8c33b4c7f800000,
        Expect::Ok(0x77d298c1fc8e2d496296cd5cffc00000, 0x1f81),
    ),
    (
        "inf + -inf = default NaN, IE",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0xd14d71b4feb82fa5a91d08957f800000,
        0x19da3ff77bac1e4b6062c03fff800000,
        Expect::Ok(0xd14d71b4feb82fa5a91d0895ffc00000, 0x1f81),
    ),
    (
        "inf + inf = inf",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x1fb80397e142cdde9ff292c07f800000,
        0x332ac5c92abe828777fed2ba7f800000,
        Expect::Ok(0x1fb80397e142cdde9ff292c07f800000, 0x1f80),
    ),
    (
        "-inf - inf = -inf",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x26fbead983023595267cd817ff800000,
        0xda6b3101bfe2e7d036039f237f800000,
        Expect::Ok(0x26fbead983023595267cd817ff800000, 0x1f80),
    ),
    (
        "QNaN + 1.0 = QNaN, no flag",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x7b67c8dcceefab85cff225347fc12345,
        0xb5d04683c0c30a658514cc143f800000,
        Expect::Ok(0x7b67c8dcceefab85cff225347fc12345, 0x1f80),
    ),
    (
        "1.0 + QNaN = QNaN, no flag",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x25cbd4369a1256831b23b1bf3f800000,
        0x9405dbad010b2a8f5d7f340a7fc12345,
        Expect::Ok(0x25cbd4369a1256831b23b1bf7fc12345, 0x1f80),
    ),
    (
        "SNaN + 1.0 = quieted SNaN, IE",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x29bfe76e371fd9d1df7161097f800001,
        0x55043d969c7d51c1789f30693f800000,
        Expect::Ok(0x29bfe76e371fd9d1df7161097fc00001, 0x1f81),
    ),
    (
        "1.0 * SNaN = quieted SNaN, IE",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0x79ac2dc99451f664bb548dcd3f800000,
        0x001c66f19cbf07b7bb069f3c7f800001,
        Expect::Ok(0x79ac2dc99451f664bb548dcd7fc00001, 0x1f81),
    ),
    (
        "QNaN + QNaN2: the first source operand wins",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x0c3a2b833a0e69d771c6766e7fc00000,
        0x6febac3d9911ba09f756e9737fc12345,
        Expect::Ok(0x0c3a2b833a0e69d771c6766e7fc00000, 0x1f80),
    ),
    (
        "QNaN2 + QNaN: the first source operand wins",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x4c57688d49f599ef9d92c2637fc12345,
        0xbb3c971ef10d5b6519099ff37fc00000,
        Expect::Ok(0x4c57688d49f599ef9d92c2637fc12345, 0x1f80),
    ),
    (
        "SNaN + SNaN2: first operand quieted, IE",
        Kind::Sub,
        Shape::Scalar32,
        0x1f80,
        0x44743ddc71ea1e8d06d9099f7f800001,
        0x04ef78558e8f40fe875c2a707fa00000,
        Expect::Ok(0x44743ddc71ea1e8d06d9099f7fc00001, 0x1f81),
    ),
    (
        "QNaN + SNaN: the first source operand (the QNaN) wins, IE for the SNaN",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x27b33053ee0f34d65a916af37fc00000,
        0x24db07dfb76c1819cb1a96987f800001,
        Expect::Ok(0x27b33053ee0f34d65a916af37fc00000, 0x1f81),
    ),
    (
        "SNaN + QNaN: the first operand, quieted, IE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xf59618eb76ac6e6b9ffd4ef97f800001,
        0xd341de3592f032bcc14b5a757fc00000,
        Expect::Ok(0xf59618eb76ac6e6b9ffd4ef97fc00001, 0x1f81),
    ),
    (
        "QNaN / 0: QNaN handling has precedence over divide-by-zero, no ZE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xf1b2ef720ef1c756b8fb127f7fc00000,
        0x387ab8ac2ecbe16a2f78ae9900000000,
        Expect::Ok(0xf1b2ef720ef1c756b8fb127f7fc00000, 0x1f80),
    ),
    (
        "SNaN / 0: invalid has precedence, IE only",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xd9838b34627da5372933c1497f800001,
        0x698ef583f118e6a01807399300000000,
        Expect::Ok(0xd9838b34627da5372933c1497fc00001, 0x1f81),
    ),
    (
        "denormal + 1.0 masked: DE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x1f80,
        0x11bef85b253d22df823d0f1400000001,
        0x959dbbe9d98f71e95a25fbe83f800000,
        Expect::Ok(0x11bef85b253d22df823d0f143f800000, 0x1fa2),
    ),
    (
        "denormal + 1.0 with DAZ: the denormal is zero, exact, no flag",
        Kind::Add,
        Shape::Scalar32,
        0x1fc0,
        0xedd514021bf6c18aeb89ea9500000001,
        0x5b5ab94172a7b1cdfb0ff4253f800000,
        Expect::Ok(0xedd514021bf6c18aeb89ea953f800000, 0x1fc0),
    ),
    (
        "denormal + 1.0 with DM unmasked: #XM before anything is computed, DE",
        Kind::Add,
        Shape::Scalar32,
        0x1e80,
        0x37a0fdf0064cb4c4e949ed4100000001,
        0x07d90e2824a0fa4a6f17cb243f800000,
        Expect::Fault(0x1e82),
    ),
    (
        "denormal + 1.0 with DAZ and DM unmasked: no denormal exception",
        Kind::Add,
        Shape::Scalar32,
        0x1ec0,
        0x6f033c47652a86ae10532f0100000001,
        0xce2e8ce1e10770f017f068743f800000,
        Expect::Ok(0x6f033c47652a86ae10532f013f800000, 0x1ec0),
    ),
    (
        "denormal / 0: divide-by-zero has precedence over the denormal operand, ZE only",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xc3e978b96e7bd10cd71899aa00000003,
        0x72be82799f005053a5b3e0f100000000,
        Expect::Ok(0xc3e978b96e7bd10cd71899aa7f800000, 0x1f84),
    ),
    (
        "denormal * 1.0 exact denormal result: DE, tiny but exact, no UE",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0xa882ca8fa5bd3aed6ffb81ba00000003,
        0x99314dc1ff33a4489be403323f800000,
        Expect::Ok(0xa882ca8fa5bd3aed6ffb81ba00000003, 0x1f82),
    ),
    (
        "denormal * 1.0 with FTZ: flushed to +0, UE and PE, and DE",
        Kind::Mul,
        Shape::Scalar32,
        0x9f80,
        0xafa09760332acfcd8b87820600000003,
        0xbcb8941e30fa5dee1c5cb87f3f800000,
        Expect::Ok(0xafa09760332acfcd8b87820600000000, 0x9fb2),
    ),
    (
        "denormal * 1.0 with FTZ and DAZ: zero, no flag",
        Kind::Mul,
        Shape::Scalar32,
        0x9fc0,
        0x6a725b9a4f4e9ce697e0d75600000003,
        0xbfd4f54ae60fd95900b86a6a3f800000,
        Expect::Ok(0x6a725b9a4f4e9ce697e0d75600000000, 0x9fc0),
    ),
    (
        "min normal / 2 exact denormal: tiny but exact, no UE, no PE",
        Kind::Div,
        Shape::Scalar32,
        0x1f80,
        0xa2337d38af7d53db50e5342200800000,
        0xa2eae5c6b0a9ef02a9c1961540000000,
        Expect::Ok(0xa2337d38af7d53db50e5342200400000, 0x1f80),
    ),
    (
        "min normal / 2 with FTZ: flushed to +0, UE and PE",
        Kind::Div,
        Shape::Scalar32,
        0x9f80,
        0xfa5adacb826680dea3e9b09a00800000,
        0xfbd56ad0dd349cc39d1b759240000000,
        Expect::Ok(0xfa5adacb826680dea3e9b09a00000000, 0x9fb0),
    ),
    (
        "min normal / 2 with UM unmasked: underflow is reported for every nonzero tiny result, #XM, UE",
        Kind::Div,
        Shape::Scalar32,
        0x1780,
        0xb58d7c3caf88e72fa63f19ae00800000,
        0x4819cbc1aa526c910877bf6940000000,
        Expect::Fault(0x1790),
    ),
    (
        "min normal / 2 with UM unmasked and FTZ: FTZ is ignored when UM is clear",
        Kind::Div,
        Shape::Scalar32,
        0x9780,
        0xf22880170bf5f32f17af05b500800000,
        0x3cd2d96e93bf5bd310f5d53040000000,
        Expect::Fault(0x9790),
    ),
    (
        "3 denormal * 0.5 masked: tie rounds to even 0x00000002, UE and PE",
        Kind::Mul,
        Shape::Scalar32,
        0x1f80,
        0xe509bd86e9ac623c4a82296800000003,
        0x2b60669b4e3a7d386d78c8873f000000,
        Expect::Ok(0xe509bd86e9ac623c4a82296800000002, 0x1fb2),
    ),
    (
        "MAX + MAX with OM unmasked: exact doubled value, #XM, OE only (no PE)",
        Kind::Add,
        Shape::Scalar32,
        0x1b80,
        0x2b424187e54e3ae7dbc75cab7f7fffff,
        0x04d05c48f70c4e48a89ea57c7f7fffff,
        Expect::Fault(0x1b88),
    ),
    (
        "MAX + 2^127 with OM unmasked: inexact, #XM, OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0x1b80,
        0x7126b3d408efd49746de5d057f7fffff,
        0xe8a4015dc5e7ca82da281f5e7f000000,
        Expect::Fault(0x1ba8),
    ),
    (
        "MAX + MAX with OM masked and PM unmasked: infinity is inexact, #XM, OE and PE",
        Kind::Add,
        Shape::Scalar32,
        0xf80,
        0x06b1259b535c14c964ba59807f7fffff,
        0xd9a7cd2859ce8731f3c6faa37f7fffff,
        Expect::Fault(0xfa8),
    ),
    (
        "1/3 with PM unmasked: #XM, PE, destination unchanged",
        Kind::Div,
        Shape::Scalar32,
        0xf80,
        0xbdd6808d28847e2408759f3d3f800000,
        0x2c5ccd62ef785a78c5535cb940400000,
        Expect::Fault(0xfa0),
    ),
    (
        "1/3 with PM unmasked, RC=toward zero",
        Kind::Div,
        Shape::Scalar32,
        0x6f80,
        0xc8078f80851ae2c64122c9423f800000,
        0xd45dcb107f7297820994d3fa40400000,
        Expect::Fault(0x6fa0),
    ),
    (
        "1.0 + 2.0 exact with every exception unmasked: no exception",
        Kind::Add,
        Shape::Scalar32,
        0x0,
        0x3434c77e59f6fdcb0208fde93f800000,
        0x7a832ec94a1e195c9a70a7a940000000,
        Expect::Ok(0x3434c77e59f6fdcb0208fde940400000, 0x0),
    ),
    (
        "SNaN + 1.0 with IM unmasked: #XM, IE, destination unchanged",
        Kind::Add,
        Shape::Scalar32,
        0x1f00,
        0x683d6231d693ea54065b13807f800001,
        0x663797bc3510f1cb49f5b7283f800000,
        Expect::Fault(0x1f01),
    ),
    (
        "QNaN + 1.0 with IM unmasked: no exception",
        Kind::Add,
        Shape::Scalar32,
        0x1f00,
        0xa6c7fcb67f29054b3cf9b1537fc00000,
        0x58d3ed8c8e570432becd8d973f800000,
        Expect::Ok(0xa6c7fcb67f29054b3cf9b1537fc00000, 0x1f00),
    ),
    (
        "inf - inf with IM unmasked: #XM, IE",
        Kind::Sub,
        Shape::Scalar32,
        0x1f00,
        0x3ebe05f063169414dc6763327f800000,
        0x345d32b7a10cbd2322da53f37f800000,
        Expect::Fault(0x1f01),
    ),
    (
        "1 / 0 with ZM unmasked: #XM, ZE",
        Kind::Div,
        Shape::Scalar32,
        0x1d80,
        0x1457e5d8a3aa9924e96b320e3f800000,
        0xac3b51014c95921c2c97f8f500000000,
        Expect::Fault(0x1d84),
    ),
    (
        "0 / 0 with ZM unmasked: invalid, not divide-by-zero, masked IE",
        Kind::Div,
        Shape::Scalar32,
        0x1d80,
        0x889f0baefd15974282c785d900000000,
        0xdb26dec1eafa4b9cb38bf25100000000,
        Expect::Ok(0x889f0baefd15974282c785d9ffc00000, 0x1d81),
    ),
    (
        "sticky flags are kept: 1.0 + 2.0 with IE, ZE and PE already set",
        Kind::Add,
        Shape::Scalar32,
        0x1fa5,
        0xc3ee39ab87dc848cd43ed3673f800000,
        0xd25fd2dfa3c58df464744df340000000,
        Expect::Ok(0xc3ee39ab87dc848cd43ed36740400000, 0x1fa5),
    ),
    (
        "sticky flags are kept when a fault occurs: SNaN + 1.0 with IM unmasked and PE already set",
        Kind::Add,
        Shape::Scalar32,
        0x1f20,
        0x24a81f71e764b4e76dc479aa7f800001,
        0x01e2281c3612f12d8f4fba853f800000,
        Expect::Fault(0x1f21),
    ),
    (
        "0.1 + 0.2 (double), RC=rn: 0x3fb999999999999a + 0x3fc999999999999a",
        Kind::Add,
        Shape::Scalar64,
        0x1f80,
        0xfed82c5d378b96ff3fb999999999999a,
        0x0df6301e5d1a13fe3fc999999999999a,
        Expect::Ok(0xfed82c5d378b96ff3fd3333333333334, 0x1fa0),
    ),
    (
        "1/3 (double), RC=rn",
        Kind::Div,
        Shape::Scalar64,
        0x1f80,
        0x97a3c6a977231e6b3ff0000000000000,
        0xb621eb737d39a4414008000000000000,
        Expect::Ok(0x97a3c6a977231e6b3fd5555555555555, 0x1fa0),
    ),
    (
        "MAX + MAX (double) masked overflow, RC=rn",
        Kind::Add,
        Shape::Scalar64,
        0x1f80,
        0x31708152aedae6637fefffffffffffff,
        0x9032467b613530cd7fefffffffffffff,
        Expect::Ok(0x31708152aedae6637ff0000000000000, 0x1fa8),
    ),
    (
        "min normal * 0.75 (double), RC=rn: tiny and inexact",
        Kind::Mul,
        Shape::Scalar64,
        0x1f80,
        0x01c6786547a8ce950010000000000001,
        0xdcc44912cbee7a7a3fe8000000000000,
        Expect::Ok(0x01c6786547a8ce95000c000000000001, 0x1fb0),
    ),
    (
        "0.1 + 0.2 (double), RC=rd: 0x3fb999999999999a + 0x3fc999999999999a",
        Kind::Add,
        Shape::Scalar64,
        0x3f80,
        0xf5834e280a876cb93fb999999999999a,
        0xd34b25c6b91a59763fc999999999999a,
        Expect::Ok(0xf5834e280a876cb93fd3333333333333, 0x3fa0),
    ),
    (
        "1/3 (double), RC=rd",
        Kind::Div,
        Shape::Scalar64,
        0x3f80,
        0xd68d8eb48f7eed363ff0000000000000,
        0xa4aaedfab26031334008000000000000,
        Expect::Ok(0xd68d8eb48f7eed363fd5555555555555, 0x3fa0),
    ),
    (
        "MAX + MAX (double) masked overflow, RC=rd",
        Kind::Add,
        Shape::Scalar64,
        0x3f80,
        0xa8d8dd29235a07dc7fefffffffffffff,
        0x916c02c0a123ba5b7fefffffffffffff,
        Expect::Ok(0xa8d8dd29235a07dc7fefffffffffffff, 0x3fa8),
    ),
    (
        "min normal * 0.75 (double), RC=rd: tiny and inexact",
        Kind::Mul,
        Shape::Scalar64,
        0x3f80,
        0x35353e9fb77664d40010000000000001,
        0x42917373eb46064f3fe8000000000000,
        Expect::Ok(0x35353e9fb77664d4000c000000000000, 0x3fb0),
    ),
    (
        "0.1 + 0.2 (double), RC=ru: 0x3fb999999999999a + 0x3fc999999999999a",
        Kind::Add,
        Shape::Scalar64,
        0x5f80,
        0xde49aaeef5f261813fb999999999999a,
        0x94760db1783fe0253fc999999999999a,
        Expect::Ok(0xde49aaeef5f261813fd3333333333334, 0x5fa0),
    ),
    (
        "1/3 (double), RC=ru",
        Kind::Div,
        Shape::Scalar64,
        0x5f80,
        0xbd44745f01c342c83ff0000000000000,
        0x9e2980b9e9e411e34008000000000000,
        Expect::Ok(0xbd44745f01c342c83fd5555555555556, 0x5fa0),
    ),
    (
        "MAX + MAX (double) masked overflow, RC=ru",
        Kind::Add,
        Shape::Scalar64,
        0x5f80,
        0xf67ad7f03c5f480a7fefffffffffffff,
        0xd286f03c13a32f727fefffffffffffff,
        Expect::Ok(0xf67ad7f03c5f480a7ff0000000000000, 0x5fa8),
    ),
    (
        "min normal * 0.75 (double), RC=ru: tiny and inexact",
        Kind::Mul,
        Shape::Scalar64,
        0x5f80,
        0x44045ca9a83767260010000000000001,
        0x32d0738ee63083b63fe8000000000000,
        Expect::Ok(0x44045ca9a8376726000c000000000001, 0x5fb0),
    ),
    (
        "0.1 + 0.2 (double), RC=rz: 0x3fb999999999999a + 0x3fc999999999999a",
        Kind::Add,
        Shape::Scalar64,
        0x7f80,
        0x50904533ea9022703fb999999999999a,
        0x5d8e5c07255ceee63fc999999999999a,
        Expect::Ok(0x50904533ea9022703fd3333333333333, 0x7fa0),
    ),
    (
        "1/3 (double), RC=rz",
        Kind::Div,
        Shape::Scalar64,
        0x7f80,
        0x616e4f873d5a56583ff0000000000000,
        0x66b530cf2a627eb74008000000000000,
        Expect::Ok(0x616e4f873d5a56583fd5555555555555, 0x7fa0),
    ),
    (
        "MAX + MAX (double) masked overflow, RC=rz",
        Kind::Add,
        Shape::Scalar64,
        0x7f80,
        0x96174441e925fd557fefffffffffffff,
        0x889472f43ac4eafb7fefffffffffffff,
        Expect::Ok(0x96174441e925fd557fefffffffffffff, 0x7fa8),
    ),
    (
        "min normal * 0.75 (double), RC=rz: tiny and inexact",
        Kind::Mul,
        Shape::Scalar64,
        0x7f80,
        0x4bc149813085f2000010000000000001,
        0x215f78e1a44587a03fe8000000000000,
        Expect::Ok(0x4bc149813085f200000c000000000000, 0x7fb0),
    ),
    (
        "double denormal + 1.0 masked: DE and PE",
        Kind::Add,
        Shape::Scalar64,
        0x1f80,
        0x7e867334d34107a10000000000000001,
        0x9539d8639ce6923e3ff0000000000000,
        Expect::Ok(0x7e867334d34107a13ff0000000000000, 0x1fa2),
    ),
    (
        "double 1/0 = +inf, ZE",
        Kind::Div,
        Shape::Scalar64,
        0x1f80,
        0x7b3ae89129fbaac53ff0000000000000,
        0xf81e2b0287f98f750000000000000000,
        Expect::Ok(0x7b3ae89129fbaac57ff0000000000000, 0x1f84),
    ),
    (
        "double SNaN + 1.0: quieted, IE",
        Kind::Add,
        Shape::Scalar64,
        0x1f80,
        0xcffb3f68bb4022c67ff0000000000001,
        0xad56994cec713ea43ff0000000000000,
        Expect::Ok(0xcffb3f68bb4022c67ff8000000000001, 0x1f81),
    ),
    (
        "double 0 / 0: default NaN is 0xfff8000000000000",
        Kind::Div,
        Shape::Scalar64,
        0x1f80,
        0xe70487f44344eae00000000000000000,
        0xb6f1c3428a3ac4070000000000000000,
        Expect::Ok(0xe70487f44344eae0fff8000000000000, 0x1f81),
    ),
    (
        "double min normal / 2 with FTZ",
        Kind::Div,
        Shape::Scalar64,
        0x9f80,
        0x679bb056affbaf2a0010000000000000,
        0x705a7f9b9c1dc0074000000000000000,
        Expect::Ok(0x679bb056affbaf2a0000000000000000, 0x9fb0),
    ),
    (
        "ADDPS: four independent lanes (1+2, inf+1, QNaN+1, 0.5+0.25)",
        Kind::Add,
        Shape::Packed32,
        0x1f80,
        0x3f0000007fc000007f8000003f800000,
        0x3e8000003f8000003f80000040000000,
        Expect::Ok(0x3f4000007fc000007f80000040400000, 0x1f80),
    ),
    (
        "MULPS, Figure 11-9: lane0 denormal operand, lane1 exact, lane2 overflow, lane3 tiny and inexact: DE, OE, UE, PE",
        Kind::Mul,
        Shape::Packed32,
        0x1f80,
        0x008000017f7fffff3f80000000000001,
        0x3f400000400000004000000040000000,
        Expect::Ok(0x006000017f8000004000000000000002, 0x1fba),
    ),
    (
        "MULPS, Figure 11-9 with OM unmasked: one #XM for the whole instruction, all four lanes unchanged",
        Kind::Mul,
        Shape::Packed32,
        0x1b80,
        0x008000017f7fffff3f80000000000001,
        0x3f400000400000004000000040000000,
        Expect::Fault(0x1bba),
    ),
    (
        "MULPS, Figure 11-9 with DM unmasked: pre-computation #XM, only DE detected",
        Kind::Mul,
        Shape::Packed32,
        0x1e80,
        0x008000017f7fffff3f80000000000001,
        0x3f400000400000004000000040000000,
        Expect::Fault(0x1e82),
    ),
    (
        "DIVPS: lane0 1/0, lane1 0/0, lane2 1/3, lane3 inf/2 (ZE and IE)",
        Kind::Div,
        Shape::Packed32,
        0x1f80,
        0x7f8000003f800000000000003f800000,
        0x40000000404000000000000000000000,
        Expect::Ok(0x7f8000003eaaaaabffc000007f800000, 0x1fa5),
    ),
    (
        "SUBPS: operand order per lane",
        Kind::Sub,
        Shape::Packed32,
        0x1f80,
        0x408000003f8000004040000040a00000,
        0x3f8000003f80000040a0000040400000,
        Expect::Ok(0x4040000000000000c000000040000000, 0x1f80),
    ),
    (
        "ADDPS with SNaN in lane 3 and RC toward -inf",
        Kind::Add,
        Shape::Packed32,
        0x3f80,
        0x7f8000013dcccccdbf8000003f800000,
        0x3f8000003e4ccccd3f800000bf800000,
        Expect::Ok(0x7fc000013e9999998000000080000000, 0x3fa1),
    ),
    (
        "ADDPS with PM unmasked and only lane 2 inexact: #XM, PE, all lanes unchanged",
        Kind::Add,
        Shape::Packed32,
        0xf80,
        0x3f8000003dcccccd3f8000003f800000,
        0x3f8000003e4ccccd3f8000003f800000,
        Expect::Fault(0xfa0),
    ),
    (
        "ADDPD: two lanes (1+2, 0.1+0.2)",
        Kind::Add,
        Shape::Packed64,
        0x1f80,
        0x3fb999999999999a3ff0000000000000,
        0x3fc999999999999a4000000000000000,
        Expect::Ok(0x3fd33333333333344008000000000000, 0x1fa0),
    ),
    (
        "DIVPD: lane0 1/3, lane1 1/0",
        Kind::Div,
        Shape::Packed64,
        0x1f80,
        0x3ff00000000000003ff0000000000000,
        0x00000000000000004008000000000000,
        Expect::Ok(0x7ff00000000000003fd5555555555555, 0x1fa4),
    ),
    (
        "MULPD: lane0 overflow, lane1 exact; RC toward zero",
        Kind::Mul,
        Shape::Packed64,
        0x7f80,
        0x3ff00000000000007fefffffffffffff,
        0x40000000000000004000000000000000,
        Expect::Ok(0x40000000000000007fefffffffffffff, 0x7fa8),
    ),
    (
        "SUBPD: lane0 -0, lane1 tiny result; RC toward -inf",
        Kind::Sub,
        Shape::Packed64,
        0x3f80,
        0x00100000000000010000000000000000,
        0x00100000000000000000000000000000,
        Expect::Ok(0x00000000000000018000000000000000, 0x3f80),
    ),
];

// ---- tests ----

#[test]
fn golden_vectors_from_the_physical_oracle_replay_exactly() {
    assert!(GOLDEN.len() >= 300);
    for (index, (kind, shape, mxcsr, a, b, expect)) in GOLDEN.iter().enumerate() {
        // Alternate the canonical encoding (the narrow decoder claims it, the executor rewrites it) and the
        // REX.W one (the general decoder), which must behave identically.
        let observed = run_reg(
            (*kind, *shape),
            (1, 2),
            *mxcsr,
            (*a, *b),
            index % 2 == 1,
            PrivilegeLevel::Ring0,
        );
        check_observed(
            &observed,
            1,
            *expect,
            &format!(
                "{} mxcsr {mxcsr:#x} {a:#034x} {b:#034x}",
                name(*kind, *shape)
            ),
        );
    }
}

#[test]
fn curated_scenarios_from_the_physical_oracle_replay_exactly() {
    assert!(HAND.len() >= 120);
    for (description, kind, shape, mxcsr, a, b, expect) in HAND {
        for rex_w in [false, true] {
            for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
                let observed = run_reg((*kind, *shape), (1, 2), *mxcsr, (*a, *b), rex_w, privilege);
                check_observed(&observed, 1, *expect, description);
            }
        }
    }
}

#[test]
fn every_register_pair_of_every_form_computes_the_same_lanes() {
    // The reference is the (xmm1, xmm2) run of the same operand values; d == s adds a value to itself.
    for kind in KINDS {
        for shape in SHAPES {
            for dst in 0..16u8 {
                for src in 0..16u8 {
                    let a = u128::from_le_bytes(xmm_pattern(usize::from(dst) + 3));
                    let b = u128::from_le_bytes(xmm_pattern(usize::from(src) + 11));
                    let b = if src == dst { a } else { b };
                    let reference = run_reg(
                        (kind, shape),
                        (1, 2),
                        MXCSR,
                        (a, b),
                        false,
                        PrivilegeLevel::Ring0,
                    );
                    let expect = outcome_of(&reference, 1);
                    let observed = run_reg(
                        (kind, shape),
                        (dst, src),
                        MXCSR,
                        (a, b),
                        false,
                        PrivilegeLevel::Ring0,
                    );
                    check_observed(
                        &observed,
                        usize::from(dst),
                        expect,
                        &format!("{} xmm{dst},xmm{src}", name(kind, shape)),
                    );
                }
            }
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn single(&mut self) -> u32 {
        const EXPONENTS: [u32; 13] = [
            0, 0, 1, 2, 0xfe, 0xff, 0x7f, 0x80, 0x60, 0x9f, 0x16, 0xe9, 0x40,
        ];
        let e = if self.next() % 3 == 0 {
            1 + (self.next() % 0xfd) as u32
        } else {
            EXPONENTS[(self.next() % 13) as usize]
        };
        ((self.next() & 1) as u32) << 31 | e << 23 | (self.next() & 0x7f_ffff) as u32
    }

    fn double(&mut self) -> u64 {
        const EXPONENTS: [u64; 13] = [
            0, 0, 1, 2, 0x7fe, 0x7ff, 0x3ff, 0x400, 0x3cb, 0x432, 0x3ca, 0x7fd, 0x200,
        ];
        let e = if self.next() % 3 == 0 {
            1 + self.next() % 0x7fd
        } else {
            EXPONENTS[(self.next() % 13) as usize]
        };
        (self.next() & 1) << 63 | e << 52 | self.next() & 0xf_ffff_ffff_ffff
    }
}

#[test]
fn round_to_nearest_even_matches_the_host_arithmetic_on_random_operands() {
    // The host computes binary32 and binary64 with the IEEE default (round to nearest even, no DAZ, no FTZ):
    // an independent check of the values (not of the flags) of every non-NaN result; a NaN result must be a NaN.
    let mut rng = Rng(0x5eed_0000_0037_0001);
    for kind in KINDS {
        for _ in 0..4000 {
            let (x, y) = (rng.single(), rng.single());
            let (fx, fy) = (f32::from_bits(x), f32::from_bits(y));
            let host = match kind {
                Kind::Add => fx + fy,
                Kind::Sub => fx - fy,
                Kind::Mul => fx * fy,
                Kind::Div => fx / fy,
            };
            let observed = run_reg(
                (kind, Shape::Scalar32),
                (1, 2),
                MXCSR,
                (
                    0xaaaa_aaaa_bbbb_bbbb_cccc_cccc_0000_0000 | u128::from(x),
                    u128::from(y),
                ),
                false,
                PrivilegeLevel::Ring0,
            );
            let Expect::Ok(result, _) = outcome_of(&observed, 1) else {
                panic!("masked exceptions fault");
            };
            assert_eq!(
                result >> 32,
                0xaaaa_aaaa_bbbb_bbbb_cccc_cccc,
                "upper bits are the destination's"
            );
            let low = result as u32;
            if host.is_nan() {
                assert!(
                    f32::from_bits(low).is_nan(),
                    "{x:#x} {kind:?} {y:#x} -> {low:#x}"
                );
            } else {
                assert_eq!(low, host.to_bits(), "{x:#x} {kind:?} {y:#x}");
            }
        }
        for _ in 0..4000 {
            let (x, y) = (rng.double(), rng.double());
            let (fx, fy) = (f64::from_bits(x), f64::from_bits(y));
            let host = match kind {
                Kind::Add => fx + fy,
                Kind::Sub => fx - fy,
                Kind::Mul => fx * fy,
                Kind::Div => fx / fy,
            };
            let observed = run_reg(
                (kind, Shape::Scalar64),
                (1, 2),
                MXCSR,
                (0x1234_5678_9abc_def0 << 64 | u128::from(x), u128::from(y)),
                false,
                PrivilegeLevel::Ring0,
            );
            let Expect::Ok(result, _) = outcome_of(&observed, 1) else {
                panic!("masked exceptions fault");
            };
            assert_eq!(
                result >> 64,
                0x1234_5678_9abc_def0,
                "upper bits are the destination's"
            );
            let low = result as u64;
            if host.is_nan() {
                assert!(
                    f64::from_bits(low).is_nan(),
                    "{x:#x} {kind:?} {y:#x} -> {low:#x}"
                );
            } else {
                assert_eq!(low, host.to_bits(), "{x:#x} {kind:?} {y:#x}");
            }
        }
    }
}

#[test]
fn packed_lanes_equal_the_scalar_results_of_their_elements() {
    // Each lane of a packed instruction is the scalar instruction on the same elements; the flags are the OR.
    let mut rng = Rng(0x5eed_0000_0037_0002);
    for kind in KINDS {
        for round in 0..400 {
            let mx = [
                MXCSR,
                MXCSR | 0x2000,
                MXCSR | 0x4000,
                MXCSR | 0x6000,
                MXCSR | 0x8040,
            ][round % 5];
            let a: [u32; 4] = core::array::from_fn(|_| rng.single());
            let b: [u32; 4] = core::array::from_fn(|_| rng.single());
            let packed = run_reg(
                (kind, Shape::Packed32),
                (1, 2),
                mx,
                (lanes32(a), lanes32(b)),
                false,
                PrivilegeLevel::Ring0,
            );
            let Expect::Ok(result, flags) = outcome_of(&packed, 1) else {
                panic!("masked exceptions fault");
            };
            let mut expected_flags = mx;
            for lane in 0..4 {
                let scalar = run_reg(
                    (kind, Shape::Scalar32),
                    (1, 2),
                    mx & !0x3f,
                    (u128::from(a[lane]), u128::from(b[lane])),
                    false,
                    PrivilegeLevel::Ring0,
                );
                let Expect::Ok(lane_result, lane_flags) = outcome_of(&scalar, 1) else {
                    panic!("masked exceptions fault");
                };
                assert_eq!(
                    (result >> (32 * lane)) as u32,
                    lane_result as u32,
                    "{kind:?} lane {lane}"
                );
                expected_flags |= lane_flags & 0x3f;
            }
            assert_eq!(flags, expected_flags, "{kind:?} {a:x?} {b:x?}");
            let a: [u64; 2] = core::array::from_fn(|_| rng.double());
            let b: [u64; 2] = core::array::from_fn(|_| rng.double());
            let packed = run_reg(
                (kind, Shape::Packed64),
                (1, 2),
                mx,
                (lanes64(a), lanes64(b)),
                false,
                PrivilegeLevel::Ring0,
            );
            let Expect::Ok(result, flags) = outcome_of(&packed, 1) else {
                panic!("masked exceptions fault");
            };
            let mut expected_flags = mx;
            for lane in 0..2 {
                let scalar = run_reg(
                    (kind, Shape::Scalar64),
                    (1, 2),
                    mx & !0x3f,
                    (u128::from(a[lane]), u128::from(b[lane])),
                    false,
                    PrivilegeLevel::Ring0,
                );
                let Expect::Ok(lane_result, lane_flags) = outcome_of(&scalar, 1) else {
                    panic!("masked exceptions fault");
                };
                assert_eq!(
                    (result >> (64 * lane)) as u64,
                    lane_result as u64,
                    "{kind:?} lane {lane}"
                );
                expected_flags |= lane_flags & 0x3f;
            }
            assert_eq!(flags, expected_flags, "{kind:?} {a:x?} {b:x?}");
        }
    }
}

#[test]
fn every_addressing_form_reads_the_operand_once_and_matches_the_register_form() {
    for kind in KINDS {
        for shape in SHAPES {
            for (index, form) in FORMS.iter().enumerate() {
                let dst = (index % 16) as u8;
                let (a, b) = sample_pair(shape, index);
                let src = (dst + 5) % 16;
                let reference = run_reg(
                    (kind, shape),
                    (dst, src),
                    MXCSR,
                    (a, b),
                    false,
                    PrivilegeLevel::Ring0,
                );
                let expect = outcome_of(&reference, usize::from(dst));
                let Expect::Ok(result, flags) = expect else {
                    panic!("masked exceptions fault");
                };
                let code = mem_code(kind, shape, form, dst);
                let mut cpu = setup(form);
                set_xmm(&mut cpu, usize::from(dst), a);
                let mut ram = Ram::new(&code);
                ram.put(form.address, &b.to_le_bytes()[..memory_bytes(shape)]);
                let ram_before = ram.bytes.clone();
                let mut expected = cpu.clone();
                set_xmm(&mut expected, usize::from(dst), result);
                expected.sse.as_mut().unwrap().mxcsr = flags;
                expected.rip = CODE + code.len() as u64;
                let label = format!("{} {}", name(kind, shape), form.name);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                assert_eq!(cpu, expected, "{label}");
                assert_eq!(ram.reads, [(form.address, memory_bytes(shape))], "{label}");
                assert!(ram.writes.is_empty(), "{label}");
                assert_eq!(ram.bytes, ram_before, "{label}");
            }
        }
    }
}

#[test]
fn the_decoder_returns_the_general_operation_and_leaves_the_narrow_forms_to_the_profile_decoder() {
    for kind in KINDS {
        for shape in SHAPES {
            // REX.W is not a form of the narrow profile decoder: the general decoder takes it.
            let code = reg_code(kind, shape, 3, 12, true);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(
                decoded.operation,
                Operation::SseFpArith {
                    kind,
                    shape,
                    destination: 3,
                    source: Sse3Source::Register(12),
                },
                "{}",
                name(kind, shape)
            );
            assert_eq!(usize::from(decoded.length), code.len());
            let code = mem_code(kind, shape, &FORMS[3], 9);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            let Operation::SseFpArith {
                kind: decoded_kind,
                shape: decoded_shape,
                destination,
                source: Sse3Source::Memory(operand),
            } = decoded.operation
            else {
                panic!("{}: {:?}", name(kind, shape), decoded.operation);
            };
            assert_eq!((decoded_kind, decoded_shape, destination), (kind, shape, 9));
            assert_eq!(operand.displacement, 0x10);
            assert_eq!(usize::from(decoded.length), code.len());
            // The canonical register form is a narrow profile operation (no general operation here).
            let code = reg_code(kind, shape, 1, 2, false);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert!(
                !matches!(decoded.operation, Operation::SseFpArith { .. }),
                "{}",
                name(kind, shape)
            );
        }
    }
}

#[test]
fn unsupported_prefix_sequences_stay_refused_without_a_change() {
    // The hardware tolerates some of these (a repeated prefix, 66 with F2/F3, a REX before a legacy prefix):
    // the model does not guess. FS/GS have no segment base in this operand model.
    for kind in KINDS {
        for shape in SHAPES {
            let base = [prefix(shape), &[0x0f, opcode(kind), 0x00]].concat();
            let codes: Vec<Vec<u8>> = vec![
                [&[0x64][..], &base].concat(),
                [&[0x65][..], &base].concat(),
                [&[0x66, 0x66][..], &base].concat(),
                [&[0xf2, 0xf3][..], &base].concat(),
                [&[0x66, 0xf2, 0x0f, opcode(kind), 0x00][..]].concat(),
                [&[0x66, 0xf3, 0x0f, opcode(kind), 0x00][..]].concat(),
                [&[0x48, 0x66][..], &base].concat(),
            ];
            for code in codes {
                let mut cpu = cpu();
                cpu.gpr[Register::Rax as usize] = 0x8010;
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
                // 66 + F2/F3 is a valid scalar form for 66-less shapes only through the profile decoder: any
                // of these either executes as another instruction or is Unsupported, never a fault of this one.
                if let Ok(StepOutcome::Unsupported { .. }) = outcome {
                    assert_eq!(cpu, before, "{code:02x?}");
                    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{code:02x?}");
                }
            }
        }
    }
}

#[test]
fn a_misaligned_packed_operand_is_gp_and_a_misaligned_scalar_operand_executes() {
    for kind in KINDS {
        for shape in SHAPES {
            let code = mem_code(kind, shape, &FORMS[0], 3);
            for delta in [1u64, 2, 3, 4, 7, 8, 15] {
                let address = 0x8000 + delta;
                let (a, b) = sample_pair(shape, delta as usize);
                let mut cpu = cpu();
                cpu.gpr[Register::Rax as usize] = address;
                set_xmm(&mut cpu, 3, a);
                let mut ram = Ram::new(&code);
                ram.put(address, &b.to_le_bytes()[..memory_bytes(shape)]);
                let before = cpu.clone();
                let label = format!("{} {address:#x}", name(kind, shape));
                for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
                    let mut cpu = before.clone();
                    ram.reads.clear();
                    let outcome = step(&mut cpu, &mut ram, privilege);
                    if is_packed(shape) {
                        assert_eq!(outcome, Ok(gp0()), "{label}");
                        assert_eq!(cpu, before, "{label}");
                        assert!(ram.reads.is_empty(), "{label}");
                    } else {
                        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{label}");
                        assert_eq!(ram.reads, [(address, memory_bytes(shape))], "{label}");
                    }
                }
            }
            // RFLAGS.AC at CPL3 (CR0 not carried, so AM counts as set): #AC of the scalar forms is not modeled and
            // an unaligned scalar operand is refused before any read; packed misalignment stays #GP, aligned
            // operands and CPL0 are unaffected.
            let mut cpu = cpu();
            cpu.rflags |= 1 << 18;
            cpu.gpr[Register::Rax as usize] = 0x8001;
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring3);
            if is_packed(shape) {
                assert_eq!(outcome, Ok(gp0()));
            } else {
                assert_eq!(
                    outcome,
                    Ok(StepOutcome::Unsupported {
                        opcode: opcode(kind)
                    })
                );
            }
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty());
            cpu.gpr[Register::Rax as usize] = 0x8010;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring3),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(ram.reads, [(0x8010, memory_bytes(shape))]);
        }
    }
}

#[test]
fn non_canonical_addresses_fault_before_alignment_and_before_memory() {
    for kind in KINDS {
        for shape in SHAPES {
            for (base, tail) in [
                (Register::Rax, &[0x18][..]),
                (Register::Rbp, &[0x5d, 0x00][..]),
                (Register::Rsp, &[0x1c, 0x24][..]),
            ] {
                for address in [
                    0x0000_8000_0000_0000u64,
                    0x0000_8000_0000_0001,
                    0xffff_7fff_ffff_ffff,
                    0x7fff_ffff_ffff_ffff,
                ] {
                    let code = [prefix(shape), &[0x0f, opcode(kind)], tail].concat();
                    let mut cpu = cpu();
                    cpu.gpr[base as usize] = address;
                    let before = cpu.clone();
                    let mut ram = Ram::new(&code);
                    let expected = if base == Register::Rsp || base == Register::Rbp {
                        ss0()
                    } else {
                        gp0()
                    };
                    assert_eq!(
                        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                        Ok(expected),
                        "{} {base:?} {address:#x}",
                        name(kind, shape)
                    );
                    assert_eq!(cpu, before);
                    assert!(ram.reads.is_empty() && ram.writes.is_empty());
                }
            }
        }
    }
}

#[test]
fn an_operand_whose_last_byte_is_non_canonical_faults_before_memory() {
    for kind in KINDS {
        for shape in [Shape::Scalar32, Shape::Scalar64] {
            let code = mem_code(kind, shape, &FORMS[0], 1);
            for address in [
                0x0000_7fff_ffff_fffcu64,
                0x0000_7fff_ffff_fffd,
                0x0000_7fff_ffff_ffff,
            ] {
                if address + memory_bytes(shape) as u64 - 1 < 0x0000_8000_0000_0000 {
                    continue;
                }
                let mut cpu = cpu();
                cpu.gpr[Register::Rax as usize] = address;
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(gp0()),
                    "{} {address:#x}",
                    name(kind, shape)
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn backend_faults_leave_cpu_mxcsr_and_memory_untouched() {
    for kind in KINDS {
        for shape in SHAPES {
            let code = mem_code(kind, shape, &FORMS[0], 1);
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = 0x8010;
            // An operand that would set flags if it were computed: the fault comes first and MXCSR stays.
            set_xmm(&mut cpu, 1, sample_pair(shape, 1).0);
            let mut ram = Ram::new(&code);
            ram.fault_page = Some(0x8000);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(0x8010),
                })),
                "{}",
                name(kind, shape)
            );
            assert_eq!(cpu, before, "{}", name(kind, shape));
            assert_eq!(ram.bytes, ram_before, "{}", name(kind, shape));
            // An unmapped operand is the backend's error, again with nothing changed.
            let mut cpu = before.clone();
            cpu.gpr[Register::Rax as usize] = 0xfff0_0000;
            let before_unmapped = cpu.clone();
            let mut ram = Ram::new(&code);
            assert!(matches!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Err(MemoryError::Unmapped { .. })
            ));
            assert_eq!(cpu, before_unmapped);
        }
    }
}

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_before_any_address_or_memory_check() {
    let cases: [(u64, u64, Option<u8>); 8] = [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE, CR4_PAE, Some(6)),
        (CR0_BASE | 4, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE, Some(6)),
        (CR0_BASE | 4 | 8, CR4_PAE, Some(6)),
    ];
    for kind in KINDS {
        for shape in SHAPES {
            let codes = [
                (reg_code(kind, shape, 1, 2, false), false),
                (mem_code(kind, shape, &FORMS[0], 3), true),
                (mem_code(kind, shape, &FORMS[1], 3), true),
            ];
            for (code, memory_form) in codes {
                for (cr0, cr4, vector) in cases {
                    // A non-canonical misaligned address: the control register gates come first.
                    for address in [0x8000u64, 0x0000_8000_0000_0001, 0x8001] {
                        let mut cpu = fp_init_cpu(cr0, cr4);
                        cpu.gpr[Register::Rax as usize] = address;
                        cpu.gpr[Register::Rbx as usize] = address;
                        let mut ram = Ram::new(&code);
                        let before = cpu.clone();
                        let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                        let label = format!(
                            "{} cr0 {cr0:#x} cr4 {cr4:#x} rax {address:#x}",
                            name(kind, shape)
                        );
                        match vector {
                            Some(vector) => {
                                assert_eq!(
                                    result.exit,
                                    RunExit::Fault(ArchitecturalFault {
                                        vector,
                                        error_code: None,
                                        linear_address: None,
                                    }),
                                    "{label}"
                                );
                                assert_eq!(after, before, "{label}");
                                assert!(ram.reads.is_empty(), "{label}");
                            }
                            None if memory_form
                                && (address == 0x0000_8000_0000_0001
                                    || address == 0x8001 && is_packed(shape)) =>
                            {
                                assert!(matches!(result.exit, RunExit::Fault(_)), "{label}");
                                assert_eq!(after, before, "{label}");
                            }
                            None => assert_eq!(
                                result.exit,
                                RunExit::InstructionBudgetExhausted,
                                "{label}"
                            ),
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
            let (p, op) = (prefix(shape), opcode(kind));
            let codes: Vec<Vec<u8>> = vec![
                [&[0xf0][..], p, &[0x0f, op, 0x00]].concat(),
                [p, &[0xf0, 0x0f, op, 0x00]].concat(),
                [&[0xf0][..], p, &[0x0f, op, 0xc1]].concat(),
                [p, &[0xf0, 0x0f, op, 0xc1]].concat(),
                [&[0xf0][..], p, &[0x48, 0x0f, op, 0x10, 0x20]].concat(),
                [&[0xf0][..], p, &[0x0f, op, 0x05, 0, 0, 0, 0]].concat(),
                [&[0xf0, 0x2e][..], p, &[0x0f, op, 0x00]].concat(),
            ];
            for code in codes {
                let mut cpu = cpu();
                cpu.gpr[Register::Rax as usize] = 0x8010;
                let before = cpu.clone();
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(ud()),
                    "{} {code:02x?}",
                    name(kind, shape)
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
                assert!(matches!(
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()),
                    Err(long_decode::DecodeError::Invalid { .. })
                ));
            }
        }
    }
}

#[test]
fn an_unmasked_exception_raises_xm_or_ud_by_cr4_osxmmexcpt_and_leaves_the_destination_and_rip() {
    // 1/3 inexact with PM unmasked (a post-computation exception), SNaN + 1 with IM unmasked (a
    // pre-computation exception): the flag is set, the destination is not written, RIP stays.
    let cases: [(Kind, Shape, u32, u128, u128, u32); 2] = [
        (
            Kind::Div,
            Shape::Scalar32,
            0x0f80,
            0x3f80_0000,
            0x4040_0000,
            0x0fa0,
        ),
        (
            Kind::Add,
            Shape::Packed32,
            0x1f00,
            lanes32([0x7f80_0001, 0x3f80_0000, 0, 0]),
            lanes32([0x3f80_0000; 4]),
            0x1f01,
        ),
    ];
    for (kind, shape, mxcsr, a, b, flagged) in cases {
        for (cr4, vector) in [
            (CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT, 19u8),
            (CR4_PAE | CR4_OSFXSR, 6),
        ] {
            // register source
            let code = reg_code(kind, shape, 1, 2, false);
            let mut cpu = fp_init_cpu(CR0_BASE, cr4);
            cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
            set_xmm(&mut cpu, 1, a);
            set_xmm(&mut cpu, 2, b);
            let mut ram = Ram::new(&code);
            let mut expected = cpu.clone();
            expected.sse.as_mut().unwrap().mxcsr = flagged;
            let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector,
                    error_code: None,
                    linear_address: None,
                })
            );
            assert_eq!(after, expected);
            assert_eq!(after.rip, CODE);
            // memory source: one read happens before the arithmetic
            let code = mem_code(kind, shape, &FORMS[1], 1);
            let mut cpu = fp_init_cpu(CR0_BASE, cr4);
            cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
            cpu.gpr[Register::Rbx as usize] = 0x8120;
            set_xmm(&mut cpu, 1, a);
            let mut ram = Ram::new(&code);
            ram.put(0x8120, &b.to_le_bytes());
            let mut expected = cpu.clone();
            expected.sse.as_mut().unwrap().mxcsr = flagged;
            let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
            assert!(matches!(result.exit, RunExit::Fault(f) if f.vector == vector));
            assert_eq!(after, expected);
            assert_eq!(ram.reads, [(0x8120, memory_bytes(shape))]);
            // Without CR4 in the state the vector is not known: Unsupported, nothing changes (not even MXCSR).
            let mut cpu = cpu_without_control_registers(mxcsr, a, b);
            let before = cpu.clone();
            let mut ram = Ram::new(&reg_code(kind, shape, 1, 2, false));
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Unsupported {
                    opcode: opcode(kind)
                })
            );
            assert_eq!(cpu, before);
        }
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
fn the_bare_run_executes_masked_forms_and_refuses_an_mxcsr_with_reserved_bits() {
    for kind in KINDS {
        for shape in SHAPES {
            let (a, b) = sample_pair(shape, 0);
            let mut cpu = cpu_without_control_registers(MXCSR, a, b);
            let before = cpu.clone();
            let code = reg_code(kind, shape, 1, 2, false);
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced)
            );
            let reference = run_reg(
                (kind, shape),
                (1, 2),
                MXCSR,
                (a, b),
                false,
                PrivilegeLevel::Ring0,
            );
            assert_eq!(
                outcome_of(&reference, 1),
                Expect::Ok(xmm(&cpu, 1), mxcsr(&cpu))
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
        }
    }
}

#[test]
fn a_state_without_sse_is_unsupported_and_changes_nothing() {
    for kind in KINDS {
        for shape in SHAPES {
            let mut cpu = cpu();
            cpu.sse = None;
            let before = cpu.clone();
            let code = reg_code(kind, shape, 1, 2, false);
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
}

#[test]
fn a_scalar_form_keeps_the_destination_upper_bits_and_ignores_the_source_upper_bits() {
    for kind in KINDS {
        for (shape, width) in [(Shape::Scalar32, 32), (Shape::Scalar64, 64)] {
            let (a, b) = sample_pair(shape, 0);
            let low = (1u128 << width) - 1;
            let upper_a = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210u128 & !low;
            let upper_b = 0xa5a5_5a5a_dead_beef_0f0f_f0f0_1234_5678u128 & !low;
            let first = run_reg(
                (kind, shape),
                (1, 2),
                MXCSR,
                (upper_a | (a & low), upper_b | (b & low)),
                false,
                PrivilegeLevel::Ring0,
            );
            let second = run_reg(
                (kind, shape),
                (1, 2),
                MXCSR,
                (upper_a | (a & low), (b & low) | !low),
                false,
                PrivilegeLevel::Ring0,
            );
            let (Expect::Ok(first_result, first_flags), Expect::Ok(second_result, second_flags)) =
                (outcome_of(&first, 1), outcome_of(&second, 1))
            else {
                panic!("masked exceptions fault");
            };
            assert_eq!(first_result & !low, upper_a, "{}", name(kind, shape));
            assert_eq!(first_result, second_result, "{}", name(kind, shape));
            assert_eq!(first_flags, second_flags, "{}", name(kind, shape));
            // xmm2 (the source) is never written
            assert_eq!(xmm(&first.after, 2), upper_b | (b & low));
        }
    }
}

#[test]
fn flags_are_sticky_and_rflags_and_the_x87_state_are_untouched() {
    // 1/3 sets PE; a prior IE and ZE stay; no instruction of the slice reads or writes RFLAGS or x87.
    let (a, b) = (0x3f80_0000u128, 0x4040_0000u128);
    let observed = run_reg(
        (Kind::Div, Shape::Scalar32),
        (1, 2),
        MXCSR | 0x05,
        (a, b),
        false,
        PrivilegeLevel::Ring0,
    );
    assert_eq!(mxcsr(&observed.after), MXCSR | 0x05 | 0x20);
    assert_eq!(observed.after.rflags, observed.before.rflags);
    assert_eq!(observed.after.x87, observed.before.x87);
    assert_eq!(observed.after.gpr, observed.before.gpr);
}
