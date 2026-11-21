//! S2-37 (remainder of the conversions): CVTSI2SS (F3 [REX.W] 0F 2A), CVTSI2SD (F2 [REX.W] 0F 2A), CVTSS2SI (F3 [REX.W]
//! 0F 2D), CVTTSS2SI (F3 [REX.W] 0F 2C), CVTSD2SI (F2 [REX.W] 0F 2D), CVTTSD2SI (F2 [REX.W] 0F 2C), CVTDQ2PD (F3 0F E6),
//! CVTPD2DQ (F2 0F E6) and CVTTPD2DQ (66 0F E6) on the general SSE floating-point core, with any register or memory
//! operand, and the MMX conversions CVTPI2PS (0F 2A), CVTPI2PD (66 0F 2A), CVTPS2PI (0F 2D), CVTTPS2PI (0F 2C), CVTPD2PI
//! (66 0F 2D) and CVTTPD2PI (66 0F 2C) (Vol. 2A pp. 3-218, 3-223, 3-224, 3-231, 3-248, 3-252: two doublewords of an MMX
//! register or m64 to two binary32/binary64 in an XMM register, or two binary32/binary64 to two doublewords in an MMX
//! register; the instructions that use an MMX register put the x87 unit in MMX state).
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A CVTDQ2PD pp. 3-208..3-210 (two signed doublewords to two binary64, exact, no SIMD exception, Exceptions
//! Type 5), CVTPD2DQ pp. 3-214..3-217 and CVTTPD2DQ pp. 3-244..3-247 (two binary64 to two signed doublewords in the
//! low half, the high 64 bits zeroed, rounded by MXCSR.RC or truncated; SIMD exceptions invalid and precision;
//! Exceptions Type 2: a 128-bit memory operand must be 16-byte aligned), CVTSD2SI pp. 3-232..3-233, CVTSS2SI
//! pp. 3-242..3-243, CVTTSD2SI pp. 3-253..3-254, CVTTSS2SI pp. 3-255..3-256 (element 0 to a signed 32-bit or, with
//! REX.W, 64-bit integer; an out-of-range value, NaN or infinity raises invalid and delivers the integer indefinite
//! value 80000000H or 8000000000000000H when masked; SIMD exceptions invalid and precision; Type 3), CVTSI2SD
//! pp. 3-236..3-237 and CVTSI2SS pp. 3-238..3-239 (a signed 32-bit or 64-bit register or memory integer to element 0,
//! rounded by MXCSR.RC for the inexact cases, the bits above the element kept; SIMD exception precision; Type 3);
//! Vol. 1 sections 4.9.1/4.9.2, 10.2.3 and 11.5.2/11.5.3 (MXCSR, masked and unmasked SIMD floating-point exceptions).
//!
//! The GOLDEN table is hardware results (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; MXCSR loaded with LDMXCSR per
//! case), selected from 99 971 cases in work/stage8/s2_37_conv2_oracle/ (gen_cases.py, oracle.c, pick_golden.py), not
//! model output: 700 register-form vectors, one per bucket of (instruction, REX.W, rounding control, DAZ, FTZ, masks,
//! outcome, flags). The hardware also settles what the SDM leaves open for these forms: the conversions report no
//! denormal-operand exception (DAZ turns a denormal source into a zero), the destination register (the XMM element or
//! the general register) is not written when an unmasked exception faults, and the invalid result of the packed
//! forms is the indefinite value per lane.
//!
//! Not asserted here: the CPUID gate (S2-57), the alignment-check exception of the scalar forms (an unaligned scalar
//! operand at CPL3 with RFLAGS.AC = 1 is Unsupported), FS/GS overrides and the prefix sequences the hardware tolerates
//! (Unsupported), the JIT (refused, see od-jit), CPL3 (refused, see cpl3_profile_set).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{
    self, DecodeError, Operand, Operation, Sse3Source, SseFpMiscKind, SseFpShape,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const DATA: u64 = 0x8000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
/// All six exceptions masked, round to nearest, no DAZ, no FTZ, no flag set.
const MXCSR: u32 = 0x1f80;
const IE: u32 = 1;
const PE: u32 = 0x20;
const DAZ: u32 = 0x40;
const RC_DOWN: u32 = 1 << 13;
const RC_UP: u32 = 2 << 13;
const RC_ZERO: u32 = 3 << 13;
/// The invalid-operation mask (IM) and the precision mask (PM).
const IM: u32 = 0x80;
const PM: u32 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;
const CR4_ALL: u64 = CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT;

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

fn ctx() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring3,
        ..AccessContext::default()
    }
}

fn step(cpu: &mut LongModeState, ram: &mut Ram) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx())
}

fn run_fp_init(mut cpu: LongModeState, ram: &mut Ram) -> (execute::RunResult, LongModeState) {
    let result = execute::run_with_cr4_profile(
        &mut cpu,
        ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(),
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

/// A vector of four doublewords, lane 0 lowest.
fn dq(lanes: [u32; 4]) -> u128 {
    lanes
        .iter()
        .enumerate()
        .fold(0, |acc, (i, lane)| acc | u128::from(*lane) << (32 * i))
}

fn d2(lanes: [u64; 2]) -> u128 {
    u128::from(lanes[0]) | u128::from(lanes[1]) << 64
}

// ---- instruction forms ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Cvtsi2ss,
    Cvtsi2sd,
    Cvtss2si,
    Cvttss2si,
    Cvtsd2si,
    Cvttsd2si,
    Cvtdq2pd,
    Cvtpd2dq,
    Cvttpd2dq,
}

const ALL: [Op; 9] = [
    Op::Cvtsi2ss,
    Op::Cvtsi2sd,
    Op::Cvtss2si,
    Op::Cvttss2si,
    Op::Cvtsd2si,
    Op::Cvttsd2si,
    Op::Cvtdq2pd,
    Op::Cvtpd2dq,
    Op::Cvttpd2dq,
];

impl Op {
    fn prefix(self) -> u8 {
        match self {
            Op::Cvtsi2ss | Op::Cvtss2si | Op::Cvttss2si | Op::Cvtdq2pd => 0xf3,
            Op::Cvtsi2sd | Op::Cvtsd2si | Op::Cvttsd2si | Op::Cvtpd2dq => 0xf2,
            Op::Cvttpd2dq => 0x66,
        }
    }

    fn opcode(self) -> u8 {
        match self {
            Op::Cvtsi2ss | Op::Cvtsi2sd => 0x2a,
            Op::Cvtss2si | Op::Cvtsd2si => 0x2d,
            Op::Cvttss2si | Op::Cvttsd2si => 0x2c,
            Op::Cvtdq2pd | Op::Cvtpd2dq | Op::Cvttpd2dq => 0xe6,
        }
    }

    /// The integer operand is the source (general register or memory) and an XMM element the destination.
    fn from_int(self) -> bool {
        matches!(self, Op::Cvtsi2ss | Op::Cvtsi2sd)
    }

    /// An XMM element (or memory) is the source and a general register the destination.
    fn to_int(self) -> bool {
        matches!(
            self,
            Op::Cvtss2si | Op::Cvttss2si | Op::Cvtsd2si | Op::Cvttsd2si
        )
    }

    /// Whether REX.W is an operand-size bit of the form (the packed forms ignore it).
    fn has_wide(self) -> bool {
        self.from_int() || self.to_int()
    }

    /// The size of a memory source and whether it must be 16-byte aligned (Exceptions Types 2, 3 and 5).
    fn memory(self, wide: bool) -> (usize, bool) {
        match self {
            Op::Cvtsi2ss | Op::Cvtsi2sd => (if wide { 8 } else { 4 }, false),
            Op::Cvtss2si | Op::Cvttss2si => (4, false),
            Op::Cvtsd2si | Op::Cvttsd2si | Op::Cvtdq2pd => (8, false),
            Op::Cvtpd2dq | Op::Cvttpd2dq => (16, true),
        }
    }

    /// The opcode byte an Unsupported outcome reports.
    fn opcode_byte(self) -> u8 {
        self.opcode()
    }
}

/// The registers of the register forms: the integer source is RCX, the integer destination RDX, the XMM destination
/// xmm1 and the XMM source xmm2.
fn reg_code(op: Op, wide: bool) -> Vec<u8> {
    let mut code = vec![op.prefix()];
    if wide && op.has_wide() {
        code.push(0x48);
    }
    code.extend([0x0f, op.opcode()]);
    code.push(if op.from_int() {
        0xc9
    } else if op.to_int() {
        0xd2
    } else {
        0xca
    });
    code
}

/// A register form with REX.X set: the narrow profile decoder does not claim it, so the general decoder does.
fn general_reg_code(op: Op, wide: bool) -> Vec<u8> {
    let mut code = reg_code(op, wide);
    let at = code.iter().position(|b| *b == 0x0f).unwrap();
    if wide && op.has_wide() {
        code[at - 1] = 0x4a;
    } else {
        code.insert(at, 0x42);
    }
    code
}

/// A memory form: ModRM/SIB/displacement bytes (reg field zero) with the destination register number in the reg field.
fn mem_code(op: Op, wide: bool, dst: u8, address: &[u8], rex_b: bool) -> Vec<u8> {
    let mut code = vec![op.prefix()];
    let rex = u8::from(wide && op.has_wide()) << 3 | u8::from(rex_b);
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend([0x0f, op.opcode()]);
    code.push(address[0] | (dst & 7) << 3);
    code.extend(&address[1..]);
    code
}

fn set_inputs(cpu: &mut LongModeState, op: Op, src: u128, dest: u128) {
    if op.from_int() {
        cpu.gpr[Register::Rcx as usize] = src as u64;
        set_xmm(cpu, 1, dest);
    } else if op.to_int() {
        set_xmm(cpu, 2, src);
        cpu.gpr[Register::Rdx as usize] = dest as u64;
    } else {
        set_xmm(cpu, 2, src);
        set_xmm(cpu, 1, dest);
    }
}

/// The destination of the instruction after it ran: the general register for the integer results, xmm1 otherwise.
fn result_of(op: Op, cpu: &LongModeState) -> u128 {
    if op.to_int() {
        u128::from(cpu.gpr[Register::Rdx as usize])
    } else {
        xmm(cpu, 1)
    }
}

fn put_result(op: Op, cpu: &mut LongModeState, value: u128) {
    if op.to_int() {
        cpu.gpr[Register::Rdx as usize] = value as u64;
    } else {
        set_xmm(cpu, 1, value);
    }
}

// ---- hardware vectors ----

enum Expect {
    /// The destination afterwards (the 64-bit general register or the XMM register) and MXCSR.
    Ok(u128, u32),
    /// A SIMD floating-point exception (#XM) with the MXCSR the fault leaves behind.
    Fault(u32),
}

/// One hardware vector: instruction, REX.W, MXCSR before, source operand, destination before, outcome.
type Row = (Op, bool, u32, u128, u128, Expect);

fn run_golden(row: &Row) {
    let (op, wide, mxcsr, src, dest, expect) = row;
    let (op, wide) = (*op, *wide);
    let code = reg_code(op, wide);
    let name = format!("{op:?} wide={wide} mx={mxcsr:#x} src={src:#x} dest={dest:#x}");
    let build = |cr0: Option<u64>| {
        let mut cpu = match cr0 {
            Some(cr0) => fp_init_cpu(cr0, CR4_ALL),
            None => cpu(),
        };
        cpu.sse.as_mut().unwrap().mxcsr = *mxcsr;
        set_inputs(&mut cpu, op, *src, *dest);
        cpu
    };
    let expected_of = |before: &LongModeState| {
        let mut expected = before.clone();
        match expect {
            Expect::Ok(value, mx) => {
                put_result(op, &mut expected, *value);
                expected.sse.as_mut().unwrap().mxcsr = *mx;
                expected.rip += code.len() as u64;
            }
            Expect::Fault(mx) => expected.sse.as_mut().unwrap().mxcsr = *mx,
        }
        expected
    };
    // The FP-init profile pair with CR4.OSXMMEXCPT = 1 also runs the faulting vectors (#XM).
    let before = build(Some(CR0_BASE));
    let mut ram = Ram::new(&code);
    let (result, after) = run_fp_init(before.clone(), &mut ram);
    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
    match expect {
        Expect::Ok(..) => assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{name}"),
        Expect::Fault(_) => assert_eq!(result.exit, xm(), "{name}"),
    }
    assert_eq!(after, expected_of(&before), "{name}");
    // The bare run (no control registers) gives the same state for every completed instruction.
    if let Expect::Ok(..) = expect {
        let before = build(None);
        let mut bare = before.clone();
        assert_eq!(
            step(&mut bare, &mut Ram::new(&code)),
            Ok(StepOutcome::Advanced),
            "{name} (bare)"
        );
        assert_eq!(bare, expected_of(&before), "{name} (bare)");
    }
}

#[test]
fn hardware_vectors_register_forms() {
    assert!(GOLDEN.len() >= 700);
    for row in GOLDEN {
        run_golden(row);
    }
}

// ---- helpers for the value tests ----

struct Obs {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    length: usize,
}

/// One register-form instruction under the FP-init profile pair.
fn run_code(code: &[u8], (op, src, dest): (Op, u128, u128), mx: u32) -> Obs {
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_ALL);
    cpu.sse.as_mut().unwrap().mxcsr = mx;
    set_inputs(&mut cpu, op, src, dest);
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

/// Whole-state check of one register-form instruction: either only the destination, MXCSR and RIP changed (RFLAGS
/// and every other register stay), or #XM with only MXCSR changed.
fn check(op: Op, wide: bool, (src, dest): (u128, u128), mx: u32, expect: Expect, label: &str) {
    let observed = run_code(&reg_code(op, wide), (op, src, dest), mx);
    let mut expected = observed.before.clone();
    match expect {
        Expect::Ok(value, mx_out) => {
            assert_eq!(
                observed.exit,
                RunExit::InstructionBudgetExhausted,
                "{label}: {:?}",
                observed.exit
            );
            put_result(op, &mut expected, value);
            expected.sse.as_mut().unwrap().mxcsr = mx_out;
            expected.rip += observed.length as u64;
        }
        Expect::Fault(mx_out) => {
            assert_eq!(observed.exit, xm(), "{label}");
            expected.sse.as_mut().unwrap().mxcsr = mx_out;
        }
    }
    assert_eq!(observed.after, expected, "{label}");
}

const HI: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;
/// The integer destination register before the instruction in the value tests (its upper bits must be replaced).
const GP: u128 = 0xdead_beef_dead_beef;

/// The scalar results keep the destination bits above the element.
fn keep32(low: u32) -> u128 {
    HI & !0xffff_ffff | u128::from(low)
}

fn keep64(low: u64) -> u128 {
    HI & !0xffff_ffff_ffff_ffff | u128::from(low)
}

fn ok(value: u128, mx: u32) -> Expect {
    Expect::Ok(value, mx)
}

// ---- CVTSI2SS ----

#[test]
fn cvtsi2ss_rounds_by_rc_and_keeps_the_upper_bits() {
    // 16777217 = 2^24 + 1 is not a binary32: nearest even gives 2^24, up gives the next binary32.
    let v = 16_777_217u128;
    let cases: [(u32, u32); 4] = [
        (MXCSR, 0x4b80_0000),
        (MXCSR | RC_DOWN, 0x4b80_0000),
        (MXCSR | RC_UP, 0x4b80_0001),
        (MXCSR | RC_ZERO, 0x4b80_0000),
    ];
    for (mx, bits) in cases {
        check(
            Op::Cvtsi2ss,
            false,
            (v, HI),
            mx,
            ok(keep32(bits), mx | PE),
            "2^24+1",
        );
    }
    // The negative value mirrors it: down gives the next binary32 below.
    let n = u128::from(-16_777_217i32 as u32);
    let cases: [(u32, u32); 4] = [
        (MXCSR, 0xcb80_0000),
        (MXCSR | RC_DOWN, 0xcb80_0001),
        (MXCSR | RC_UP, 0xcb80_0000),
        (MXCSR | RC_ZERO, 0xcb80_0000),
    ];
    for (mx, bits) in cases {
        check(
            Op::Cvtsi2ss,
            false,
            (n, HI),
            mx,
            ok(keep32(bits), mx | PE),
            "-(2^24+1)",
        );
    }
    // Exact values raise nothing; the upper 32 bits of a 32-bit source register are ignored.
    check(
        Op::Cvtsi2ss,
        false,
        (16_777_216, HI),
        MXCSR,
        ok(keep32(0x4b80_0000), MXCSR),
        "2^24",
    );
    check(
        Op::Cvtsi2ss,
        false,
        (0, HI),
        MXCSR,
        ok(keep32(0), MXCSR),
        "zero",
    );
    check(
        Op::Cvtsi2ss,
        false,
        (0xffff_ffff, HI),
        MXCSR,
        ok(keep32(0xbf80_0000), MXCSR),
        "-1",
    );
    check(
        Op::Cvtsi2ss,
        false,
        (0xdead_beef_0000_0003, HI),
        MXCSR,
        ok(keep32(0x4040_0000), MXCSR),
        "the upper half of the register is ignored",
    );
    // The 64-bit source (REX.W).
    let max = u128::from(i64::MAX as u64);
    check(
        Op::Cvtsi2ss,
        true,
        (max, HI),
        MXCSR,
        ok(keep32(0x5f00_0000), MXCSR | PE),
        "i64::MAX rn",
    );
    check(
        Op::Cvtsi2ss,
        true,
        (max, HI),
        MXCSR | RC_ZERO,
        ok(keep32(0x5eff_ffff), MXCSR | RC_ZERO | PE),
        "i64::MAX rz",
    );
    check(
        Op::Cvtsi2ss,
        true,
        (max, HI),
        MXCSR | RC_DOWN,
        ok(keep32(0x5eff_ffff), MXCSR | RC_DOWN | PE),
        "i64::MAX rd",
    );
    check(
        Op::Cvtsi2ss,
        true,
        (max, HI),
        MXCSR | RC_UP,
        ok(keep32(0x5f00_0000), MXCSR | RC_UP | PE),
        "i64::MAX ru",
    );
    check(
        Op::Cvtsi2ss,
        true,
        (u128::from(i64::MIN as u64), HI),
        MXCSR,
        ok(keep32(0xdf00_0000), MXCSR),
        "i64::MIN is exact",
    );
}

#[test]
fn cvtsi2ss_unmasked_precision_faults_without_writing() {
    let mx = MXCSR & !PM;
    check(
        Op::Cvtsi2ss,
        false,
        (16_777_217, HI),
        mx,
        Expect::Fault(mx | PE),
        "inexact, PM clear",
    );
    check(
        Op::Cvtsi2ss,
        false,
        (16_777_216, HI),
        mx,
        ok(keep32(0x4b80_0000), mx),
        "exact, PM clear",
    );
    check(
        Op::Cvtsi2ss,
        true,
        (u128::from(i64::MAX as u64), HI),
        mx,
        Expect::Fault(mx | PE),
        "i64::MAX, PM clear",
    );
}

// ---- CVTSI2SD ----

#[test]
fn cvtsi2sd_is_exact_for_32_bits_and_rounds_64_bit_sources() {
    check(
        Op::Cvtsi2sd,
        false,
        (0x7fff_ffff, HI),
        MXCSR,
        ok(keep64(0x41df_ffff_ffc0_0000), MXCSR),
        "i32::MAX",
    );
    check(
        Op::Cvtsi2sd,
        false,
        (0xffff_ffff, HI),
        MXCSR,
        ok(keep64(0xbff0_0000_0000_0000), MXCSR),
        "-1",
    );
    check(
        Op::Cvtsi2sd,
        false,
        (0x8000_0000, HI),
        MXCSR,
        ok(keep64(0xc1e0_0000_0000_0000), MXCSR),
        "i32::MIN",
    );
    // 2^53 + 1 is not a binary64.
    let v = (1u128 << 53) + 1;
    check(
        Op::Cvtsi2sd,
        true,
        (v, HI),
        MXCSR,
        ok(keep64(0x4340_0000_0000_0000), MXCSR | PE),
        "2^53+1 rn",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (v, HI),
        MXCSR | RC_UP,
        ok(keep64(0x4340_0000_0000_0001), MXCSR | RC_UP | PE),
        "2^53+1 ru",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (v, HI),
        MXCSR | RC_DOWN,
        ok(keep64(0x4340_0000_0000_0000), MXCSR | RC_DOWN | PE),
        "2^53+1 rd",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (v, HI),
        MXCSR | RC_ZERO,
        ok(keep64(0x4340_0000_0000_0000), MXCSR | RC_ZERO | PE),
        "2^53+1 rz",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (v + 1, HI),
        MXCSR,
        ok(keep64(0x4340_0000_0000_0001), MXCSR),
        "2^53+2 is exact",
    );
    let max = u128::from(i64::MAX as u64);
    check(
        Op::Cvtsi2sd,
        true,
        (max, HI),
        MXCSR,
        ok(keep64(0x43e0_0000_0000_0000), MXCSR | PE),
        "i64::MAX rn",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (max, HI),
        MXCSR | RC_ZERO,
        ok(keep64(0x43df_ffff_ffff_ffff), MXCSR | RC_ZERO | PE),
        "i64::MAX rz",
    );
    check(
        Op::Cvtsi2sd,
        true,
        (u128::from(i64::MIN as u64), HI),
        MXCSR,
        ok(keep64(0xc3e0_0000_0000_0000), MXCSR),
        "i64::MIN",
    );
    // Unmasked precision: #XM, nothing written. A 32-bit source never raises it.
    let mx = MXCSR & !PM;
    check(
        Op::Cvtsi2sd,
        true,
        (v, HI),
        mx,
        Expect::Fault(mx | PE),
        "2^53+1, PM clear",
    );
    check(
        Op::Cvtsi2sd,
        false,
        (0x7fff_ffff, HI),
        mx,
        ok(keep64(0x41df_ffff_ffc0_0000), mx),
        "i32::MAX, PM clear",
    );
}

// ---- CVTSS2SI / CVTTSS2SI ----

#[test]
fn cvtss2si_rounds_by_rc_and_truncates_in_cvttss2si() {
    // 2.5, -2.5, 1.5, 0.5, -0.5, 3.0 as binary32 and the integers each rounding mode gives.
    let rows: [(u32, [(u32, i64); 4]); 4] = [
        (
            0x4020_0000,
            [
                (MXCSR, 2),
                (MXCSR | RC_DOWN, 2),
                (MXCSR | RC_UP, 3),
                (MXCSR | RC_ZERO, 2),
            ],
        ),
        (
            0xc020_0000,
            [
                (MXCSR, -2),
                (MXCSR | RC_DOWN, -3),
                (MXCSR | RC_UP, -2),
                (MXCSR | RC_ZERO, -2),
            ],
        ),
        (
            0x3fc0_0000,
            [
                (MXCSR, 2),
                (MXCSR | RC_DOWN, 1),
                (MXCSR | RC_UP, 2),
                (MXCSR | RC_ZERO, 1),
            ],
        ),
        (
            0x3f00_0000,
            [
                (MXCSR, 0),
                (MXCSR | RC_DOWN, 0),
                (MXCSR | RC_UP, 1),
                (MXCSR | RC_ZERO, 0),
            ],
        ),
    ];
    for (bits, expected) in rows {
        for (mx, value) in expected {
            let want32 = u128::from(value as i32 as u32);
            let want64 = u128::from(value as u64);
            check(
                Op::Cvtss2si,
                false,
                (u128::from(bits), GP),
                mx,
                ok(want32, mx | PE),
                "cvtss2si r32",
            );
            check(
                Op::Cvtss2si,
                true,
                (u128::from(bits), GP),
                mx,
                ok(want64, mx | PE),
                "cvtss2si r64",
            );
        }
        // Truncation ignores the rounding control.
        let truncated = (f32::from_bits(bits) as i64) as u64;
        for mx in [MXCSR, MXCSR | RC_DOWN, MXCSR | RC_UP, MXCSR | RC_ZERO] {
            check(
                Op::Cvttss2si,
                true,
                (u128::from(bits), GP),
                mx,
                ok(u128::from(truncated), mx | PE),
                "cvttss2si r64",
            );
            check(
                Op::Cvttss2si,
                false,
                (u128::from(bits), GP),
                mx,
                ok(u128::from(truncated as u32), mx | PE),
                "cvttss2si r32",
            );
        }
    }
    // An exact source raises nothing; a 32-bit result is zero-extended into the 64-bit register.
    check(
        Op::Cvtss2si,
        false,
        (0x4040_0000, GP),
        MXCSR,
        ok(3, MXCSR),
        "3.0",
    );
    check(
        Op::Cvtss2si,
        false,
        (0xc040_0000, GP),
        MXCSR,
        ok(0xffff_fffd, MXCSR),
        "-3.0 zero-extends",
    );
    check(
        Op::Cvtss2si,
        true,
        (0xc040_0000, GP),
        MXCSR,
        ok(0xffff_ffff_ffff_fffd, MXCSR),
        "-3.0 r64",
    );
}

#[test]
fn cvtss2si_range_limits_and_the_integer_indefinite() {
    // 2^31 does not fit 32 bits but fits 64; -2^31 fits both; -2^31-256 fits only 64 bits.
    check(
        Op::Cvtss2si,
        false,
        (0x4f00_0000, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR | IE),
        "2^31 r32",
    );
    check(
        Op::Cvtss2si,
        true,
        (0x4f00_0000, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR),
        "2^31 r64",
    );
    check(
        Op::Cvtss2si,
        false,
        (0x4eff_ffff, GP),
        MXCSR,
        ok(0x7fff_ff80, MXCSR),
        "2^31-128 r32",
    );
    check(
        Op::Cvtss2si,
        false,
        (0xcf00_0000, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR),
        "-2^31 r32 is exact, not invalid",
    );
    check(
        Op::Cvtss2si,
        true,
        (0xcf00_0000, GP),
        MXCSR,
        ok(0xffff_ffff_8000_0000, MXCSR),
        "-2^31 r64",
    );
    check(
        Op::Cvtss2si,
        false,
        (0xcf00_0001, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR | IE),
        "-2^31-256 r32",
    );
    check(
        Op::Cvtss2si,
        true,
        (0xcf00_0001, GP),
        MXCSR,
        ok(0xffff_ffff_7fff_ff00, MXCSR),
        "-2^31-256 r64",
    );
    // 2^63 and the NaNs and infinities give the 64-bit indefinite value.
    check(
        Op::Cvtss2si,
        true,
        (0x5f00_0000, GP),
        MXCSR,
        ok(0x8000_0000_0000_0000, MXCSR | IE),
        "2^63 r64",
    );
    check(
        Op::Cvtss2si,
        true,
        (0xdf00_0000, GP),
        MXCSR,
        ok(0x8000_0000_0000_0000, MXCSR),
        "-2^63 r64 is exact",
    );
    check(
        Op::Cvtss2si,
        true,
        (0x5eff_ffff, GP),
        MXCSR,
        ok(0x7fff_ff80_0000_0000, MXCSR),
        "2^63-2^39 r64",
    );
    for bits in [
        0x7fc0_0000u32,
        0xffc0_0000,
        0x7fa0_0000,
        0x7f80_0000,
        0xff80_0000,
    ] {
        for op in [Op::Cvtss2si, Op::Cvttss2si] {
            check(
                op,
                false,
                (u128::from(bits), GP),
                MXCSR,
                ok(0x8000_0000, MXCSR | IE),
                "special r32",
            );
            check(
                op,
                true,
                (u128::from(bits), GP),
                MXCSR,
                ok(0x8000_0000_0000_0000, MXCSR | IE),
                "special r64",
            );
        }
    }
    // Unmasked invalid operation: #XM, the register keeps its value.
    let mx = MXCSR & !IM;
    check(
        Op::Cvtss2si,
        false,
        (0x7fc0_0000, GP),
        mx,
        Expect::Fault(mx | IE),
        "NaN, IM clear",
    );
    check(
        Op::Cvttss2si,
        true,
        (0x5f00_0000, GP),
        mx,
        Expect::Fault(mx | IE),
        "2^63, IM clear",
    );
    // A source with the precision flag only is not affected by IM.
    check(
        Op::Cvtss2si,
        false,
        (0x3f00_0000, GP),
        mx,
        ok(0, mx | PE),
        "0.5, IM clear",
    );
}

#[test]
fn cvtss2si_denormal_sources_and_daz() {
    // The instructions report no denormal-operand exception: the smallest denormal rounds like any tiny value, and DAZ
    // turns it into an exact zero.
    check(
        Op::Cvtss2si,
        false,
        (1, GP),
        MXCSR,
        ok(0, MXCSR | PE),
        "denormal",
    );
    check(
        Op::Cvtss2si,
        false,
        (1, GP),
        MXCSR | RC_UP,
        ok(1, MXCSR | RC_UP | PE),
        "denormal, round up",
    );
    check(
        Op::Cvtss2si,
        false,
        (1, GP),
        MXCSR | DAZ,
        ok(0, MXCSR | DAZ),
        "DAZ",
    );
    check(
        Op::Cvtss2si,
        false,
        (1, GP),
        MXCSR | DAZ | RC_UP,
        ok(0, MXCSR | DAZ | RC_UP),
        "DAZ, round up",
    );
    check(
        Op::Cvttss2si,
        false,
        (0x8000_0001, GP),
        MXCSR,
        ok(0, MXCSR | PE),
        "negative denormal truncates",
    );
}

// ---- CVTSD2SI / CVTTSD2SI ----

#[test]
fn cvtsd2si_rounds_by_rc_and_range_limits() {
    // 2.5 and -2.5 as binary64.
    for (bits, expected) in [
        (
            0x4004_0000_0000_0000u64,
            [
                (MXCSR, 2i64),
                (MXCSR | RC_DOWN, 2),
                (MXCSR | RC_UP, 3),
                (MXCSR | RC_ZERO, 2),
            ],
        ),
        (
            0xc004_0000_0000_0000,
            [
                (MXCSR, -2),
                (MXCSR | RC_DOWN, -3),
                (MXCSR | RC_UP, -2),
                (MXCSR | RC_ZERO, -2),
            ],
        ),
    ] {
        for (mx, value) in expected {
            check(
                Op::Cvtsd2si,
                false,
                (u128::from(bits), GP),
                mx,
                ok(u128::from(value as i32 as u32), mx | PE),
                "r32",
            );
            check(
                Op::Cvtsd2si,
                true,
                (u128::from(bits), GP),
                mx,
                ok(u128::from(value as u64), mx | PE),
                "r64",
            );
        }
        for mx in [MXCSR, MXCSR | RC_UP] {
            let t = (f64::from_bits(bits) as i64) as u64;
            check(
                Op::Cvttsd2si,
                true,
                (u128::from(bits), GP),
                mx,
                ok(u128::from(t), mx | PE),
                "cvttsd2si r64",
            );
        }
    }
    // 2147483647.5 is the tie between i32::MAX and 2^31: nearest even is 2^31, out of range for 32 bits.
    let half_up = 0x41df_ffff_ffe0_0000u128;
    check(
        Op::Cvtsd2si,
        false,
        (half_up, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR | IE),
        "2147483647.5 rn r32",
    );
    check(
        Op::Cvtsd2si,
        true,
        (half_up, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR | PE),
        "2147483647.5 rn r64",
    );
    check(
        Op::Cvtsd2si,
        false,
        (half_up, GP),
        MXCSR | RC_DOWN,
        ok(0x7fff_ffff, MXCSR | RC_DOWN | PE),
        "2147483647.5 rd r32",
    );
    check(
        Op::Cvttsd2si,
        false,
        (half_up, GP),
        MXCSR,
        ok(0x7fff_ffff, MXCSR | PE),
        "2147483647.5 rz r32",
    );
    // -2147483648.5: nearest even is -2^31, which fits; rounding down gives -2^31-1, which does not.
    let half_down = 0xc1e0_0000_0010_0000u128;
    check(
        Op::Cvtsd2si,
        false,
        (half_down, GP),
        MXCSR,
        ok(0x8000_0000, MXCSR | PE),
        "-2147483648.5 rn r32",
    );
    check(
        Op::Cvtsd2si,
        false,
        (half_down, GP),
        MXCSR | RC_DOWN,
        ok(0x8000_0000, MXCSR | RC_DOWN | IE),
        "-2147483648.5 rd r32",
    );
    check(
        Op::Cvtsd2si,
        true,
        (half_down, GP),
        MXCSR | RC_DOWN,
        ok(0xffff_ffff_7fff_ffff, MXCSR | RC_DOWN | PE),
        "-2147483648.5 rd r64",
    );
    // The 64-bit limits.
    check(
        Op::Cvtsd2si,
        true,
        (0x43e0_0000_0000_0000, GP),
        MXCSR,
        ok(0x8000_0000_0000_0000, MXCSR | IE),
        "2^63",
    );
    check(
        Op::Cvtsd2si,
        true,
        (0x43df_ffff_ffff_ffff, GP),
        MXCSR,
        ok(0x7fff_ffff_ffff_fc00, MXCSR),
        "2^63-1024",
    );
    check(
        Op::Cvtsd2si,
        true,
        (0xc3e0_0000_0000_0000, GP),
        MXCSR,
        ok(0x8000_0000_0000_0000, MXCSR),
        "-2^63 is exact",
    );
    for bits in [
        0x7ff8_0000_0000_0000u64,
        0xfff8_0000_0000_0000,
        0x7ff4_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
    ] {
        for op in [Op::Cvtsd2si, Op::Cvttsd2si] {
            check(
                op,
                false,
                (u128::from(bits), GP),
                MXCSR,
                ok(0x8000_0000, MXCSR | IE),
                "special r32",
            );
            check(
                op,
                true,
                (u128::from(bits), GP),
                MXCSR,
                ok(0x8000_0000_0000_0000, MXCSR | IE),
                "special r64",
            );
        }
    }
    let mx = MXCSR & !IM;
    check(
        Op::Cvtsd2si,
        true,
        (0x7ff8_0000_0000_0000, GP),
        mx,
        Expect::Fault(mx | IE),
        "NaN, IM clear",
    );
    // Only element 0 is read: the upper 64 bits of the source play no part.
    check(
        Op::Cvtsd2si,
        false,
        (
            u128::from(0x4008_0000_0000_0000u64) | 0x7ff8_0000_0000_0000u128 << 64,
            GP,
        ),
        MXCSR,
        ok(3, MXCSR),
        "3.0 with a NaN above",
    );
    // DAZ: a denormal is an exact zero and no denormal-operand exception exists.
    check(
        Op::Cvtsd2si,
        false,
        (1, GP),
        MXCSR,
        ok(0, MXCSR | PE),
        "denormal",
    );
    check(
        Op::Cvtsd2si,
        false,
        (1, GP),
        MXCSR | DAZ,
        ok(0, MXCSR | DAZ),
        "DAZ",
    );
}

// ---- CVTDQ2PD ----

#[test]
fn cvtdq2pd_converts_the_two_low_dwords_exactly() {
    let src = dq([0xffff_ffff, 0x7fff_ffff, 0x1234_5678, 0x8000_0000]);
    let expected = d2([0xbff0_0000_0000_0000, 0x41df_ffff_ffc0_0000]);
    check(
        Op::Cvtdq2pd,
        false,
        (src, HI),
        MXCSR,
        ok(expected, MXCSR),
        "dwords 0 and 1",
    );
    // No SIMD exception exists: every mask clear, nothing faults and MXCSR is not touched.
    check(
        Op::Cvtdq2pd,
        false,
        (src, HI),
        0,
        ok(expected, 0),
        "all unmasked",
    );
    check(
        Op::Cvtdq2pd,
        false,
        (src, HI),
        MXCSR | RC_UP | DAZ,
        ok(expected, MXCSR | RC_UP | DAZ),
        "RC and DAZ do not matter",
    );
    check(
        Op::Cvtdq2pd,
        false,
        (dq([0x8000_0000, 0, 0, 0]), HI),
        MXCSR,
        ok(d2([0xc1e0_0000_0000_0000, 0]), MXCSR),
        "i32::MIN and zero",
    );
}

// ---- CVTPD2DQ / CVTTPD2DQ ----

#[test]
fn cvtpd2dq_rounds_by_rc_and_zeroes_the_high_half() {
    // 2.5 and -2.5.
    let src = d2([0x4004_0000_0000_0000, 0xc004_0000_0000_0000]);
    for (mx, lanes) in [
        (MXCSR, [2i32, -2]),
        (MXCSR | RC_DOWN, [2, -3]),
        (MXCSR | RC_UP, [3, -2]),
        (MXCSR | RC_ZERO, [2, -2]),
    ] {
        let want = dq([lanes[0] as u32, lanes[1] as u32, 0, 0]);
        check(
            Op::Cvtpd2dq,
            false,
            (src, HI),
            mx,
            ok(want, mx | PE),
            "cvtpd2dq",
        );
    }
    for mx in [MXCSR, MXCSR | RC_UP, MXCSR | RC_DOWN] {
        let want = dq([2, -2i32 as u32, 0, 0]);
        check(
            Op::Cvttpd2dq,
            false,
            (src, HI),
            mx,
            ok(want, mx | PE),
            "cvttpd2dq",
        );
    }
    // Exact lanes raise nothing; the destination was entirely replaced.
    let exact = d2([0x4008_0000_0000_0000, 0xc010_0000_0000_0000]);
    check(
        Op::Cvtpd2dq,
        false,
        (exact, HI),
        MXCSR,
        ok(dq([3, -4i32 as u32, 0, 0]), MXCSR),
        "3.0 and -4.0",
    );
    // REX.X is not an operand bit of the packed forms: the general decoder (not the narrow one) gives the same result.
    let observed = run_code(
        &general_reg_code(Op::Cvtpd2dq, false),
        (Op::Cvtpd2dq, exact, HI),
        MXCSR,
    );
    assert_eq!(xmm(&observed.after, 1), dq([3, -4i32 as u32, 0, 0]));
}

#[test]
fn cvtpd2dq_invalid_lanes_give_the_indefinite_value_per_lane() {
    // A NaN in lane 0 and 1.0 in lane 1; then 1e10 (out of range) and an infinity.
    let nan = 0x7ff8_0000_0000_0000u64;
    let one = 0x3ff0_0000_0000_0000u64;
    let big = 0x4202_a05f_2000_0000u64; // 1e10
    let inf = 0xfff0_0000_0000_0000u64;
    for op in [Op::Cvtpd2dq, Op::Cvttpd2dq] {
        check(
            op,
            false,
            (d2([nan, one]), HI),
            MXCSR,
            ok(dq([0x8000_0000, 1, 0, 0]), MXCSR | IE),
            "NaN, 1.0",
        );
        check(
            op,
            false,
            (d2([big, inf]), HI),
            MXCSR,
            ok(dq([0x8000_0000, 0x8000_0000, 0, 0]), MXCSR | IE),
            "1e10, -inf",
        );
        // Unmasked: #XM, the destination keeps all 128 bits.
        let mx = MXCSR & !IM;
        check(
            op,
            false,
            (d2([nan, one]), HI),
            mx,
            Expect::Fault(mx | IE),
            "IM clear",
        );
    }
    // -2147483648.5 under round-to-nearest is -2^31 (valid, PE); 2147483647.5 is 2^31 (invalid, IE only).
    check(
        Op::Cvtpd2dq,
        false,
        (d2([0xc1e0_0000_0010_0000, 0x3ff0_0000_0000_0000]), HI),
        MXCSR,
        ok(dq([0x8000_0000, 1, 0, 0]), MXCSR | PE),
        "-2147483648.5",
    );
    check(
        Op::Cvtpd2dq,
        false,
        (d2([0x41df_ffff_ffe0_0000, 0x3ff0_0000_0000_0000]), HI),
        MXCSR,
        ok(dq([0x8000_0000, 1, 0, 0]), MXCSR | IE),
        "2147483647.5",
    );
}

// ---- gates ----

/// A source value that converts without an exception, for the tests that do not look at the value.
fn test_value(op: Op) -> u128 {
    match op {
        Op::Cvtsi2ss | Op::Cvtsi2sd => 0xffff_ffff_ffff_fff9, // -7 as 32-bit and as 64-bit
        Op::Cvtss2si | Op::Cvttss2si => 0x4020_0000,          // 2.5
        Op::Cvtsd2si | Op::Cvttsd2si => 0x4004_0000_0000_0000, // 2.5
        Op::Cvtdq2pd => dq([0xffff_fff9, 5, 0, 0]),
        Op::Cvtpd2dq | Op::Cvttpd2dq => d2([0x4004_0000_0000_0000, 0xc004_0000_0000_0000]),
    }
}

#[test]
fn cr0_cr4_gates_and_lock() {
    for op in ALL {
        let code = reg_code(op, false);
        for (cr0, cr4, vector) in [
            (CR0_BASE | 1 << 2, CR4_ALL, 6u8),       // CR0.EM
            (CR0_BASE | 1 << 3, CR4_ALL, 7),         // CR0.TS: #NM
            (CR0_BASE, CR4_PAE | CR4_OSXMMEXCPT, 6), // CR4.OSFXSR = 0
        ] {
            let mut cpu = fp_init_cpu(cr0, cr4);
            set_inputs(&mut cpu, op, test_value(op), GP);
            let before = cpu.clone();
            let (result, after) = run_fp_init(cpu, &mut Ram::new(&code));
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector,
                    error_code: None,
                    linear_address: None
                }),
                "{op:?}"
            );
            assert_eq!(after, before, "{op:?}");
        }
        // LOCK is #UD, with a register or a memory operand.
        for body in [code.clone(), mem_code(op, false, 1, &[0x03], false)] {
            let mut locked = vec![0xf0];
            locked.extend(&body);
            let mut cpu = cpu();
            let before = cpu.clone();
            assert_eq!(
                step(&mut cpu, &mut Ram::new(&locked)),
                Ok(fault_of(6, None)),
                "lock {op:?}"
            );
            assert_eq!(cpu, before);
        }
    }
}

#[test]
fn unmasked_exception_without_cr4_is_unsupported_and_changes_nothing() {
    // The bare run has no CR4 to pick #XM or #UD from: the faulting forms stay Unsupported.
    for (op, wide, src, mx) in [
        (Op::Cvtss2si, false, 0x7fc0_0000u128, MXCSR & !IM),
        (Op::Cvttsd2si, true, 0x7ff8_0000_0000_0000, MXCSR & !IM),
        (Op::Cvtsi2sd, true, (1u128 << 53) + 1, MXCSR & !PM),
        (Op::Cvtsi2ss, false, 16_777_217, MXCSR & !PM),
        (Op::Cvtdq2pd, false, 0, MXCSR), // never faults: control
        (
            Op::Cvtpd2dq,
            false,
            d2([0x7ff8_0000_0000_0000, 0]),
            MXCSR & !IM,
        ),
        (
            Op::Cvttpd2dq,
            false,
            d2([0x3ff0_0000_0000_0000, 0x4004_0000_0000_0000]),
            MXCSR & !PM,
        ),
    ] {
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().mxcsr = mx;
        set_inputs(&mut cpu, op, src, GP);
        let before = cpu.clone();
        let code = reg_code(op, wide);
        let outcome = step(&mut cpu, &mut Ram::new(&code));
        if op == Op::Cvtdq2pd {
            assert_eq!(outcome, Ok(StepOutcome::Advanced));
        } else {
            assert_eq!(
                outcome,
                Ok(StepOutcome::Unsupported {
                    opcode: op.opcode_byte()
                }),
                "{op:?}"
            );
            assert_eq!(cpu, before, "{op:?}");
        }
    }
}

#[test]
fn reserved_mxcsr_bits_stop_the_conversions() {
    // LDMXCSR cannot create a value with bits 31:16 set; the model never lets an instruction run on one.
    for reserved in [0x0001_0000u32, 0x0100_0000, 0x8000_0000, 0xffff_0000] {
        let mx = MXCSR | reserved;
        for op in ALL {
            let mut cpu = cpu();
            cpu.sse.as_mut().unwrap().mxcsr = mx;
            set_inputs(&mut cpu, op, test_value(op), GP);
            let before = cpu.clone();
            let code = reg_code(op, false);
            assert_eq!(
                step(&mut cpu, &mut Ram::new(&code)),
                Ok(StepOutcome::Unsupported {
                    opcode: op.opcode_byte()
                }),
                "{op:?} {mx:#x}"
            );
            assert_eq!(cpu, before, "{op:?} {mx:#x}");
        }
    }
}

// ---- memory operands ----

/// The addressing forms: ModRM/SIB/displacement bytes (reg field zero), REX.B, the register setup and the address.
fn address_forms(delta: u64) -> Vec<(&'static str, Vec<u8>, bool, Vec<(Register, u64)>, u64)> {
    use Register::{R8, R13, Rax, Rbp, Rbx};
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

/// The (instruction, REX.W) pairs to run: the integer forms with and without REX.W, the packed forms once.
fn variants() -> Vec<(Op, bool)> {
    ALL.iter()
        .flat_map(|op| {
            if op.has_wide() {
                vec![(*op, false), (*op, true)]
            } else {
                vec![(*op, false)]
            }
        })
        .collect()
}

#[test]
fn memory_source_equals_register_source() {
    for (op, wide) in variants() {
        let source = test_value(op);
        let (width, _) = op.memory(wide);
        let from_register = run_code(&reg_code(op, wide), (op, source, GP), MXCSR);
        assert_eq!(
            from_register.exit,
            RunExit::InstructionBudgetExhausted,
            "{op:?} {wide}"
        );
        // The memory operand is the integer or the element. For a destination register of 3 or more (not RAX/RCX/RDX/
        // RBX, which the address forms use), the integer destination is the register in the reg field.
        let dst = if op.to_int() { 6u8 } else { 1 };
        for (name, address, rex_b, registers, effective) in address_forms(0) {
            let code = mem_code(op, wide, dst, &address, rex_b);
            let mut cpu = cpu();
            if op.to_int() {
                cpu.gpr[usize::from(dst)] = GP as u64;
            } else {
                set_xmm(&mut cpu, 1, GP | GP << 64);
            }
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let mut ram = Ram::new(&code);
            ram.put(effective, &source.to_le_bytes()[..width]);
            // The bytes after the operand differ from the register source's upper lanes: they must not matter.
            ram.put(effective + width as u64, &[0xa5; 16]);
            let mut expected = cpu.clone();
            if op.to_int() {
                expected.gpr[usize::from(dst)] = result_of(op, &from_register.after) as u64;
            } else {
                // The register-form run started from xmm1 = GP; run it again from the same destination.
                let again = run_code(&reg_code(op, wide), (op, source, GP | GP << 64), MXCSR);
                set_xmm(&mut expected, 1, xmm(&again.after, 1));
            }
            expected.sse.as_mut().unwrap().mxcsr = from_register.after.sse.as_ref().unwrap().mxcsr;
            expected.rip += code.len() as u64;
            assert_eq!(
                step(&mut cpu, &mut ram),
                Ok(StepOutcome::Advanced),
                "{op:?} {wide} {name}"
            );
            assert_eq!(cpu, expected, "{op:?} {wide} {name}");
            assert_eq!(ram.reads, vec![(effective, width)], "{op:?} {wide} {name}");
            assert!(ram.writes.is_empty());
        }
    }
}

#[test]
fn memory_alignment_faults_and_unaligned_scalars() {
    for (op, wide) in variants() {
        let (width, aligned) = op.memory(wide);
        for delta in [1u64, 4, 8, 12] {
            let (name, address, rex_b, registers, effective) = address_forms(delta).remove(1);
            let code = mem_code(op, wide, 6, &address, rex_b);
            let mut cpu = cpu();
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            ram.put(effective, &test_value(op).to_le_bytes());
            let outcome = step(&mut cpu, &mut ram);
            if aligned {
                // Type 2: a 128-bit memory operand must be 16-byte aligned: #GP(0), no read.
                assert_eq!(outcome, Ok(fault_of(13, Some(0))), "{op:?} {name} +{delta}");
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            } else {
                // Types 3 and 5: no alignment requirement.
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{op:?} {name} +{delta}");
                assert_eq!(ram.reads, vec![(effective, width)], "{op:?} +{delta}");
            }
        }
    }
}

#[test]
fn memory_faults_are_not_partial_and_noncanonical_is_gp() {
    for (op, wide) in variants() {
        let code = mem_code(op, wide, 6, &[0x03], false);
        // A page fault of the backend at the source: nothing changes.
        let mut cpu = cpu();
        cpu.gpr[Register::Rbx as usize] = DATA + 0x1000;
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA + 0x1000);
        match step(&mut cpu, &mut ram) {
            Ok(StepOutcome::Fault(fault)) => assert_eq!(fault.vector, 14, "{op:?}"),
            Err(MemoryError::Architectural(fault)) => assert_eq!(fault.vector, 14, "{op:?}"),
            other => panic!("{op:?}: {other:?}"),
        }
        assert_eq!(cpu, before, "{op:?}");
        // A non-canonical address: #GP(0).
        let mut cpu = self::cpu();
        cpu.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&code)),
            Ok(fault_of(13, Some(0))),
            "{op:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn the_source_is_read_for_exactly_its_size_at_the_end_of_a_page() {
    // The operand ends at the page boundary: a longer read would touch the next page.
    for (op, wide) in variants() {
        let (width, aligned) = op.memory(wide);
        let code = mem_code(op, wide, 6, &[0x03], false);
        let mut cpu = cpu();
        let at = DATA + 0x1000 - width as u64;
        // A 16-byte operand is aligned; the others end exactly at the boundary.
        assert!(!aligned || at % 16 == 0);
        cpu.gpr[Register::Rbx as usize] = at;
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA + 0x1000);
        ram.put(at, &test_value(op).to_le_bytes()[..width]);
        assert_eq!(
            step(&mut cpu, &mut ram),
            Ok(StepOutcome::Advanced),
            "{op:?} {wide}"
        );
        assert_eq!(ram.reads, vec![(at, width)], "{op:?} {wide}");
    }
}

#[test]
fn an_unmasked_exception_with_a_memory_source_changes_only_mxcsr() {
    // CVTTSD2SI r32, [rbx] of a NaN with IM clear, under the FP-init profile pair (CR4 carried).
    let code = mem_code(Op::Cvttsd2si, false, 6, &[0x03], false);
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_ALL);
    cpu.sse.as_mut().unwrap().mxcsr = MXCSR & !IM;
    cpu.gpr[Register::Rbx as usize] = DATA;
    cpu.gpr[Register::Rsi as usize] = GP as u64;
    let mut expected = cpu.clone();
    expected.sse.as_mut().unwrap().mxcsr = (MXCSR & !IM) | IE;
    let mut ram = Ram::new(&code);
    ram.put(DATA, &0x7ff8_0000_0000_0001u64.to_le_bytes());
    let (result, after) = run_fp_init(cpu, &mut ram);
    assert_eq!(result.exit, xm());
    assert_eq!(ram.reads, vec![(DATA, 8)]);
    assert_eq!(after, expected);
}

// ---- decoding ----

fn decode(bytes: &[u8]) -> Result<long_decode::Instruction, DecodeError> {
    let mut ram = Ram::new(bytes);
    long_decode::decode(&mut ram, CODE, AccessContext::default())
}

fn memory_of(operation: Operation) -> long_decode::MemoryOperand {
    match operation {
        Operation::SseCvtFromInt {
            source: Operand::Memory(memory),
            ..
        } => memory,
        Operation::SseCvtToInt {
            source: Sse3Source::Memory(memory),
            ..
        } => memory,
        Operation::SseFpMisc {
            source: Sse3Source::Memory(memory),
            ..
        } => memory,
        other => panic!("{other:?}"),
    }
}

#[test]
fn decoder_produces_the_general_operations() {
    // [rbx] forms are not narrow profile forms: the general decoder handles them.
    let from_int: [(&[u8], bool, u8, OperandSize); 4] = [
        (&[0xf3, 0x0f, 0x2a, 0x03], false, 0, OperandSize::Dword),
        (
            &[0xf3, 0x48, 0x0f, 0x2a, 0x0b],
            false,
            1,
            OperandSize::Qword,
        ),
        (&[0xf2, 0x0f, 0x2a, 0x13], true, 2, OperandSize::Dword),
        (
            &[0xf2, 0x4c, 0x0f, 0x2a, 0x4b, 0x10],
            true,
            9,
            OperandSize::Qword,
        ),
    ];
    for (bytes, double, destination, size) in from_int {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        let Operation::SseCvtFromInt {
            double: d,
            destination: x,
            source: Operand::Memory(memory),
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!(
            (d, x, memory.size),
            (double, destination, size),
            "{bytes:02x?}"
        );
        assert_eq!(
            memory.base,
            long_decode::AddressBase::Register(Register::Rbx)
        );
    }
    // A register source with a REX.X bit (not claimed by the narrow decoder): REX.B extends the integer register.
    let instruction = decode(&[0xf3, 0x4b, 0x0f, 0x2a, 0xc1]).unwrap();
    let Operation::SseCvtFromInt {
        double: false,
        destination: 0,
        source: Operand::Register(view),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (view.register(), view.size()),
        (Register::R9, OperandSize::Qword)
    );
    // The conversions to an integer: (bytes, double, truncate, destination, size, memory bytes).
    let to_int: [(&[u8], bool, bool, Register, OperandSize, OperandSize); 6] = [
        (
            &[0xf3, 0x0f, 0x2d, 0x03],
            false,
            false,
            Register::Rax,
            OperandSize::Dword,
            OperandSize::Dword,
        ),
        (
            &[0xf3, 0x48, 0x0f, 0x2c, 0x0b],
            false,
            true,
            Register::Rcx,
            OperandSize::Qword,
            OperandSize::Dword,
        ),
        (
            &[0xf2, 0x0f, 0x2d, 0x13],
            true,
            false,
            Register::Rdx,
            OperandSize::Dword,
            OperandSize::Qword,
        ),
        (
            &[0xf2, 0x48, 0x0f, 0x2c, 0x4b, 0x08],
            true,
            true,
            Register::Rcx,
            OperandSize::Qword,
            OperandSize::Qword,
        ),
        (
            &[0xf2, 0x44, 0x0f, 0x2d, 0x0b],
            true,
            false,
            Register::R9,
            OperandSize::Dword,
            OperandSize::Qword,
        ),
        (
            &[0xf3, 0x4c, 0x0f, 0x2d, 0x23],
            false,
            false,
            Register::R12,
            OperandSize::Qword,
            OperandSize::Dword,
        ),
    ];
    for (bytes, double, truncate, register, size, source_size) in to_int {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        let Operation::SseCvtToInt {
            double: d,
            truncate: t,
            dst,
            source: Sse3Source::Memory(memory),
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!(
            (d, t, dst.register(), dst.size()),
            (double, truncate, register, size),
            "{bytes:02x?}"
        );
        assert_eq!(memory.size, source_size, "{bytes:02x?}");
    }
    // A register source: REX.B extends the XMM register.
    let instruction = decode(&[0xf3, 0x4b, 0x0f, 0x2d, 0xc2]).unwrap();
    let Operation::SseCvtToInt {
        double: false,
        truncate: false,
        dst,
        source: Sse3Source::Register(10),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (dst.register(), dst.size()),
        (Register::Rax, OperandSize::Qword)
    );
    // The packed integer conversions.
    for (bytes, kind, shape) in [
        (
            &[0xf3, 0x0f, 0xe6, 0x03][..],
            SseFpMiscKind::Cvtdq2pd,
            SseFpShape::Packed32,
        ),
        (
            &[0xf2, 0x0f, 0xe6, 0x03],
            SseFpMiscKind::Cvtpd2dq,
            SseFpShape::Packed64,
        ),
        (
            &[0x66, 0x0f, 0xe6, 0x03],
            SseFpMiscKind::Cvttpd2dq,
            SseFpShape::Packed64,
        ),
    ] {
        let instruction = decode(bytes).unwrap();
        let operation = instruction.operation;
        let Operation::SseFpMisc {
            kind: k,
            shape: s,
            destination: 0,
            source: Sse3Source::Memory(_),
            immediate: 0,
        } = operation
        else {
            panic!("{bytes:02x?}: {operation:?}");
        };
        assert_eq!((k, s), (kind, shape), "{bytes:02x?}");
        let _ = memory_of(operation);
    }
    // With REX.X (not claimed by the narrow decoder) the register forms are general operations too.
    for (bytes, kind, shape) in [
        (
            &[0xf3, 0x4d, 0x0f, 0xe6, 0xca][..],
            SseFpMiscKind::Cvtdq2pd,
            SseFpShape::Packed32,
        ),
        (
            &[0xf2, 0x4d, 0x0f, 0xe6, 0xca],
            SseFpMiscKind::Cvtpd2dq,
            SseFpShape::Packed64,
        ),
        (
            &[0x66, 0x4d, 0x0f, 0xe6, 0xca],
            SseFpMiscKind::Cvttpd2dq,
            SseFpShape::Packed64,
        ),
    ] {
        assert_eq!(
            decode(bytes).unwrap().operation,
            Operation::SseFpMisc {
                kind,
                shape,
                destination: 9,
                source: Sse3Source::Register(10),
                immediate: 0
            },
            "{bytes:02x?}"
        );
    }
}

#[test]
fn memory_access_of_a_decoded_form_follows_the_instruction() {
    assert_eq!(
        SseFpMiscKind::Cvtdq2pd.source_access(SseFpShape::Packed32),
        (8, false)
    );
    assert_eq!(
        SseFpMiscKind::Cvtpd2dq.source_access(SseFpShape::Packed64),
        (16, true)
    );
    assert_eq!(
        SseFpMiscKind::Cvttpd2dq.source_access(SseFpShape::Packed64),
        (16, true)
    );
}

#[test]
fn decoder_refuses_what_it_does_not_model_and_locked_forms_are_invalid() {
    // Unsupported: 66 together with F2/F3, a repeated prefix, FS/GS, and 0F E6 without a mandatory prefix.
    for bytes in [
        &[0x66, 0xf2, 0x0f, 0x2a, 0x03][..],
        &[0x66, 0xf3, 0x0f, 0x2d, 0x03],
        &[0xf2, 0xf2, 0x0f, 0x2c, 0x03],
        &[0x66, 0x66, 0x0f, 0xe6, 0x03],
        &[0x64, 0xf3, 0x0f, 0x2a, 0x03],
        &[0x65, 0xf2, 0x0f, 0x2d, 0x03],
        &[0x0f, 0xe6, 0x03],
        &[0xf2, 0xf3, 0x0f, 0xe6, 0x03],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Unsupported { .. })),
            "{bytes:02x?}: {:?}",
            decode(bytes).map(|i| i.operation)
        );
    }
    for bytes in [
        &[0xf0, 0xf3, 0x0f, 0x2a, 0x03][..],
        &[0xf0, 0xf2, 0x0f, 0x2a, 0x03],
        &[0xf0, 0xf3, 0x0f, 0x2d, 0x03],
        &[0xf0, 0xf2, 0x0f, 0x2c, 0x03],
        &[0xf0, 0xf3, 0x0f, 0xe6, 0x03],
        &[0xf0, 0xf2, 0x0f, 0xe6, 0x03],
        &[0xf0, 0x66, 0x0f, 0xe6, 0x03],
        &[0xf3, 0xf0, 0x48, 0x0f, 0x2c, 0xc1],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Invalid { .. })),
            "{bytes:02x?}"
        );
    }
}

/// The narrow profile decoder handles the REX-less (or REX.W-only) forms and the executor rewrites them into the
/// general operation; with REX.X the general decoder handles the same instruction. Both give the same state. This
/// also checks the executor's rewrite: every narrow operation maps to the right instruction, width and operands.
#[test]
fn narrow_register_forms_and_general_forms_agree() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let f32_pool = [
        0x0000_0000u32,
        0x8000_0000,
        0x3f80_0000,
        0xbf80_0000,
        0x4020_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0001,
        0x7fa0_0000,
        1,
        0x007f_ffff,
        0x4f00_0000,
        0xcf00_0000,
        0x4eff_ffff,
        0x5f00_0000,
        0xdf00_0000,
        0x3fc0_0000,
    ];
    let f64_pool = [
        0u64,
        0x8000_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0x4004_0000_0000_0000,
        0xc004_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0x7ff8_0000_0000_0001,
        0x7ff4_0000_0000_0000,
        1,
        0x41e0_0000_0000_0000,
        0x41df_ffff_ffe0_0000,
        0xc1e0_0000_0010_0000,
        0x43e0_0000_0000_0000,
        0xc3e0_0000_0000_0000,
        0x3fe0_0000_0000_0000,
    ];
    let ints = [
        0u64,
        1,
        0xffff_ffff_ffff_ffff,
        0x7fff_ffff,
        0x8000_0000,
        0x1_0000_0001,
        0x7fff_ffff_ffff_ffff,
        0x8000_0000_0000_0000,
        0x20_0000_0000_0001,
        0x1000_0001,
    ];
    for (op, wide) in variants() {
        for round in 0..400 {
            let src: u128 = match op {
                Op::Cvtsi2ss | Op::Cvtsi2sd => {
                    let value = ints[(next() % ints.len() as u64) as usize]
                        ^ if round % 4 == 0 { next() >> 40 } else { 0 };
                    u128::from(value)
                }
                Op::Cvtss2si | Op::Cvttss2si => {
                    u128::from(f32_pool[(next() % f32_pool.len() as u64) as usize])
                        | u128::from(next()) << 32
                }
                Op::Cvtsd2si | Op::Cvttsd2si => {
                    u128::from(f64_pool[(next() % f64_pool.len() as u64) as usize])
                        | u128::from(next()) << 64
                }
                Op::Cvtdq2pd => u128::from(next()) | u128::from(next()) << 64,
                Op::Cvtpd2dq | Op::Cvttpd2dq => d2([
                    f64_pool[(next() % f64_pool.len() as u64) as usize],
                    f64_pool[(next() % f64_pool.len() as u64) as usize],
                ]),
            };
            let mx = [
                MXCSR,
                MXCSR | DAZ,
                MXCSR & !IM,
                0,
                MXCSR | 3 << 13,
                MXCSR & !PM,
            ][round % 6];
            let narrow = run_code(&reg_code(op, wide), (op, src, GP | GP << 64), mx);
            let general = run_code(&general_reg_code(op, wide), (op, src, GP | GP << 64), mx);
            assert_eq!(narrow.exit, general.exit, "{op:?} {wide} {src:#x}");
            assert_eq!(
                result_of(op, &narrow.after),
                result_of(op, &general.after),
                "{op:?} {wide} {src:#x}"
            );
            assert_eq!(
                narrow.after.sse.as_ref().unwrap().mxcsr,
                general.after.sse.as_ref().unwrap().mxcsr,
                "{op:?} {wide} {src:#x}"
            );
            assert_eq!(narrow.after.rflags, general.after.rflags, "{op:?}");
            assert_eq!(
                narrow.after.gpr, general.after.gpr,
                "{op:?} {wide} {src:#x}"
            );
        }
    }
}

/// The REX-less [rax] forms are narrow profile operations with a fixed destination (xmm0 or EAX/RAX); the executor
/// rewrites them into the general operation. A general [rbx] form with the same address and destination agrees.
#[test]
fn narrow_rax_forms_and_general_forms_agree() {
    for (op, wide) in variants() {
        let (width, _) = op.memory(wide);
        let source = test_value(op);
        let run = |address: u8, rex: Option<u8>| {
            let mut code = vec![op.prefix()];
            code.extend(rex);
            code.extend([0x0f, op.opcode(), address]);
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = DATA + 0x40;
            cpu.gpr[Register::Rbx as usize] = DATA + 0x40;
            let mut ram = Ram::new(&code);
            ram.put(DATA + 0x40, &source.to_le_bytes()[..width]);
            let outcome = step(&mut cpu, &mut ram);
            (outcome, cpu, ram.reads)
        };
        let rex = if wide && op.has_wide() {
            Some(0x48)
        } else {
            None
        };
        let (narrow_outcome, narrow_cpu, narrow_reads) = run(0x00, rex);
        let (general_outcome, mut general_cpu, general_reads) = run(0x03, rex);
        assert_eq!(narrow_outcome, Ok(StepOutcome::Advanced), "{op:?} {wide}");
        assert_eq!(general_outcome, narrow_outcome, "{op:?} {wide}");
        assert_eq!(narrow_reads, vec![(DATA + 0x40, width)], "{op:?} {wide}");
        assert_eq!(general_reads, narrow_reads);
        // The destination is xmm0 for the integer sources, EAX/RAX for the integer results: the general form in
        // the same register (reg field 0) agrees; RIP differs by nothing as both forms have the same length.
        general_cpu.rip = narrow_cpu.rip;
        assert_eq!(general_cpu, narrow_cpu, "{op:?} {wide}");
    }
}

// ---- host arithmetic ----

#[test]
fn integer_conversions_match_the_host_on_random_operands() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..3000 {
        // CVTSI2SS / CVTSI2SD round to nearest even exactly as the host's integer-to-float casts do.
        let raw = next() >> (next() % 64);
        let (v32, v64) = (raw as u32 as i32, raw as i64);
        let observed = run_code(
            &reg_code(Op::Cvtsi2ss, false),
            (Op::Cvtsi2ss, u128::from(raw), HI),
            MXCSR,
        );
        assert_eq!(
            xmm(&observed.after, 1) as u32,
            (v32 as f32).to_bits(),
            "cvtsi2ss {v32}"
        );
        let observed = run_code(
            &reg_code(Op::Cvtsi2ss, true),
            (Op::Cvtsi2ss, u128::from(raw), HI),
            MXCSR,
        );
        assert_eq!(
            xmm(&observed.after, 1) as u32,
            (v64 as f32).to_bits(),
            "cvtsi2ss {v64}"
        );
        let observed = run_code(
            &reg_code(Op::Cvtsi2sd, false),
            (Op::Cvtsi2sd, u128::from(raw), HI),
            MXCSR,
        );
        assert_eq!(
            xmm(&observed.after, 1) as u64,
            f64::from(v32).to_bits(),
            "cvtsi2sd {v32}"
        );
        let observed = run_code(
            &reg_code(Op::Cvtsi2sd, true),
            (Op::Cvtsi2sd, u128::from(raw), HI),
            MXCSR,
        );
        assert_eq!(
            xmm(&observed.after, 1) as u64,
            (v64 as f64).to_bits(),
            "cvtsi2sd {v64}"
        );
        assert_eq!(
            xmm(&observed.after, 1) >> 64,
            HI >> 64,
            "the upper half is kept"
        );
        // CVTTSS2SI / CVTTSD2SI truncate toward zero as the host's float-to-integer casts do for in-range values.
        let f = f32::from_bits(next() as u32);
        let d = f64::from_bits(next());
        let exponent = if round % 2 == 0 {
            (next() % 70) as u32
        } else {
            0
        };
        let (f, d) = (
            f / 2f32.powi(exponent as i32),
            d / 2f64.powi(exponent as i32),
        );
        if f.is_finite() && f.abs() < 2.0e9 {
            let observed = run_code(
                &reg_code(Op::Cvttss2si, false),
                (Op::Cvttss2si, u128::from(f.to_bits()), GP),
                MXCSR,
            );
            assert_eq!(
                observed.after.gpr[Register::Rdx as usize],
                u64::from(f as i32 as u32),
                "cvttss2si {f}"
            );
        }
        if f.is_finite() && f.abs() < 9.0e18 {
            let observed = run_code(
                &reg_code(Op::Cvttss2si, true),
                (Op::Cvttss2si, u128::from(f.to_bits()), GP),
                MXCSR,
            );
            assert_eq!(
                observed.after.gpr[Register::Rdx as usize],
                f as i64 as u64,
                "cvttss2si r64 {f}"
            );
        }
        if d.is_finite() && d.abs() < 2.0e9 {
            let observed = run_code(
                &reg_code(Op::Cvttsd2si, false),
                (Op::Cvttsd2si, u128::from(d.to_bits()), GP),
                MXCSR,
            );
            assert_eq!(
                observed.after.gpr[Register::Rdx as usize],
                u64::from(d as i32 as u32),
                "cvttsd2si {d}"
            );
            // Round to nearest even equals the host's round_ties_even for in-range values.
            let observed = run_code(
                &reg_code(Op::Cvtsd2si, true),
                (Op::Cvtsd2si, u128::from(d.to_bits()), GP),
                MXCSR,
            );
            assert_eq!(
                observed.after.gpr[Register::Rdx as usize],
                d.round_ties_even() as i64 as u64,
                "cvtsd2si {d}"
            );
        }
        if d.is_finite() && d.abs() < 9.0e18 {
            let observed = run_code(
                &reg_code(Op::Cvttsd2si, true),
                (Op::Cvttsd2si, u128::from(d.to_bits()), GP),
                MXCSR,
            );
            assert_eq!(
                observed.after.gpr[Register::Rdx as usize],
                d as i64 as u64,
                "cvttsd2si r64 {d}"
            );
        }
        // CVTDQ2PD is exact.
        let pair = next();
        let observed = run_code(
            &reg_code(Op::Cvtdq2pd, false),
            (Op::Cvtdq2pd, u128::from(pair), HI),
            MXCSR,
        );
        assert_eq!(
            xmm(&observed.after, 1),
            d2([
                f64::from(pair as u32 as i32).to_bits(),
                f64::from((pair >> 32) as u32 as i32).to_bits()
            ])
        );
    }
}

// ---- MMX conversions ----

/// CVTPI2PS, CVTPI2PD (the source is an MMX register or m64, the destination an XMM register) and CVTPS2PI, CVTTPS2PI,
/// CVTPD2PI, CVTTPD2PI (the destination is an MMX register). Intel SDM Vol. 2A pp. 3-218, 3-223, 3-224, 3-231, 3-248,
/// 3-252; the MMX register MMn is the low 64 bits of the physical x87 data register Rn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mmx {
    Cvtpi2ps,
    Cvtpi2pd,
    Cvtps2pi,
    Cvttps2pi,
    Cvtpd2pi,
    Cvttpd2pi,
}

const ALL_MMX: [Mmx; 6] = [
    Mmx::Cvtpi2ps,
    Mmx::Cvtpi2pd,
    Mmx::Cvtps2pi,
    Mmx::Cvttps2pi,
    Mmx::Cvtpd2pi,
    Mmx::Cvttpd2pi,
];

impl Mmx {
    fn packed_double(self) -> bool {
        matches!(self, Mmx::Cvtpi2pd | Mmx::Cvtpd2pi | Mmx::Cvttpd2pi)
    }

    fn opcode(self) -> u8 {
        match self {
            Mmx::Cvtpi2ps | Mmx::Cvtpi2pd => 0x2a,
            Mmx::Cvtps2pi | Mmx::Cvtpd2pi => 0x2d,
            Mmx::Cvttps2pi | Mmx::Cvttpd2pi => 0x2c,
        }
    }

    /// The source is the MMX register or m64 and the destination an XMM register.
    fn from_mm(self) -> bool {
        matches!(self, Mmx::Cvtpi2ps | Mmx::Cvtpi2pd)
    }

    /// The size of a memory source and whether it must be 16-byte aligned (Exceptions Types 2 and 5).
    fn memory(self) -> (usize, bool) {
        match self {
            Mmx::Cvtpd2pi | Mmx::Cvttpd2pi => (16, true),
            _ => (8, false),
        }
    }

    fn opcode_byte(self) -> u8 {
        self.opcode()
    }
}

/// The register forms: xmm1 and MMn for the conversions to XMM, MMn and xmm2 for the conversions to MMX.
fn mmx_reg_code(op: Mmx, mm: u8) -> Vec<u8> {
    let mut code = vec![];
    if op.packed_double() {
        code.push(0x66);
    }
    code.extend([0x0f, op.opcode()]);
    code.push(if op.from_mm() {
        0xc8 | mm
    } else {
        0xc0 | mm << 3 | 2
    });
    code
}

/// A memory form: the XMM (or MMX) register number in the ModRM reg field, the address bytes without a reg field.
fn mmx_mem_code(op: Mmx, reg: u8, address: &[u8], rex_b: bool) -> Vec<u8> {
    let mut code = vec![];
    if op.packed_double() {
        code.push(0x66);
    }
    if rex_b {
        code.push(0x41);
    }
    code.extend([0x0f, op.opcode()]);
    code.push(address[0] | (reg & 7) << 3);
    code.extend(&address[1..]);
    code
}

/// An 80-bit register image with the given low 64 bits and an exponent field that is not the MMX one.
fn reg80(data: u64) -> [u8; 10] {
    let mut raw = [0u8; 10];
    raw[..8].copy_from_slice(&data.to_le_bytes());
    raw[8] = 0xff;
    raw[9] = 0x3f;
    raw
}

/// The state with x87 TOP = 3, the tag word given and the physical register R[mm] holding the MMX data.
fn mmx_cpu(cr: bool, tag: u16, mm: usize, data: u64) -> LongModeState {
    let mut cpu = if cr {
        fp_init_cpu(CR0_BASE, CR4_ALL)
    } else {
        self::cpu()
    };
    let x87 = cpu.x87.as_mut().unwrap();
    x87.control_word = 0x37f;
    x87.status_word = 3 << 11;
    x87.tag_word = tag;
    x87.registers[mm] = reg80(data);
    cpu
}

/// What every MMX instruction does to the x87 state: TOP = 0, all tags valid; a destination register gets the result
/// in bits 63:0 and all ones in bits 79:64.
fn entered_mmx(before: &LongModeState, written: Option<(usize, u64)>) -> LongModeState {
    let mut after = before.clone();
    let x87 = after.x87.as_mut().unwrap();
    x87.status_word &= !(7 << 11);
    x87.tag_word = 0;
    if let Some((mm, value)) = written {
        let mut raw = [0xffu8; 10];
        raw[..8].copy_from_slice(&value.to_le_bytes());
        x87.registers[mm] = raw;
    }
    after
}

fn mm_value(cpu: &LongModeState, mm: usize) -> u64 {
    u64::from_le_bytes(
        cpu.x87.as_ref().unwrap().registers[mm][..8]
            .try_into()
            .unwrap(),
    )
}

/// One MMX hardware vector: instruction, MXCSR, the MMX register used, source (the MMX data or the XMM source),
/// destination XMM before (zero for the MMX destinations), outcome.
type MmxRow = (Mmx, u32, u8, u128, u128, Expect);

fn run_mmx_golden(row: &MmxRow) {
    let (op, mxcsr, mm, src, dest, expect) = row;
    let (op, mm) = (*op, usize::from(*mm));
    let code = mmx_reg_code(op, mm as u8);
    let name = format!("{op:?} mx={mxcsr:#x} mm={mm} src={src:#x} dest={dest:#x}");
    let build = |cr: bool| {
        let mut cpu = mmx_cpu(
            cr,
            0x33,
            mm,
            if op.from_mm() {
                *src as u64
            } else {
                0x1234_5678_9abc_def0
            },
        );
        cpu.sse.as_mut().unwrap().mxcsr = *mxcsr;
        if op.from_mm() {
            set_xmm(&mut cpu, 1, *dest);
        } else {
            set_xmm(&mut cpu, 2, *src);
        }
        cpu
    };
    let expected_of = |before: &LongModeState| match expect {
        Expect::Ok(value, mx) => {
            let mut expected = if op.from_mm() {
                entered_mmx(before, None)
            } else {
                entered_mmx(before, Some((mm, *value as u64)))
            };
            if op.from_mm() {
                set_xmm(&mut expected, 1, *value);
            }
            expected.sse.as_mut().unwrap().mxcsr = *mx;
            expected.rip += code.len() as u64;
            expected
        }
        Expect::Fault(mx) => {
            // The hardware has entered MMX state when it raises the SIMD exception; the destination is not written.
            let mut expected = entered_mmx(before, None);
            expected.sse.as_mut().unwrap().mxcsr = *mx;
            expected
        }
    };
    // The FP-init profile pair with CR4.OSXMMEXCPT = 1 also runs the faulting vectors (#XM).
    let before = build(true);
    let mut ram = Ram::new(&code);
    let (result, after) = run_fp_init(before.clone(), &mut ram);
    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
    match expect {
        Expect::Ok(..) => assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{name}"),
        Expect::Fault(_) => assert_eq!(result.exit, xm(), "{name}"),
    }
    assert_eq!(after, expected_of(&before), "{name}");
    if let Expect::Ok(..) = expect {
        let before = build(false);
        let mut bare = before.clone();
        assert_eq!(
            step(&mut bare, &mut Ram::new(&code)),
            Ok(StepOutcome::Advanced),
            "{name} (bare)"
        );
        assert_eq!(bare, expected_of(&before), "{name} (bare)");
    }
}

#[test]
fn hardware_vectors_mmx_register_forms() {
    assert!(GOLDEN_MMX.len() >= 190);
    for row in GOLDEN_MMX {
        run_mmx_golden(row);
    }
}

/// One MMX conversion on the register forms: MMn holds the data (CVTPI2PS/PD) or xmm2 the source.
fn run_mmx(op: Mmx, src: u128, mx: u32) -> (RunExit, LongModeState, LongModeState) {
    let mm = 5usize;
    let mut cpu = mmx_cpu(
        true,
        0x33,
        mm,
        if op.from_mm() {
            src as u64
        } else {
            0x1234_5678_9abc_def0
        },
    );
    cpu.sse.as_mut().unwrap().mxcsr = mx;
    if op.from_mm() {
        set_xmm(&mut cpu, 1, HI);
    } else {
        set_xmm(&mut cpu, 2, src);
    }
    let before = cpu.clone();
    let (result, after) = run_fp_init(cpu, &mut Ram::new(&mmx_reg_code(op, mm as u8)));
    (result.exit, before, after)
}

fn pair(lo: u32, hi: u32) -> u128 {
    u128::from(lo) | u128::from(hi) << 32
}

#[test]
fn cvtpi2ps_converts_two_dwords_keeps_the_high_half_and_enters_mmx_state() {
    // -1 in the high dword, 2^24 + 1 in the low one: the low result is inexact.
    let src = pair(16_777_217, 0xffff_ffff);
    for (mx, low) in [
        (MXCSR, 0x4b80_0000u32),
        (MXCSR | RC_DOWN, 0x4b80_0000),
        (MXCSR | RC_UP, 0x4b80_0001),
        (MXCSR | RC_ZERO, 0x4b80_0000),
    ] {
        let (exit, before, after) = run_mmx(Mmx::Cvtpi2ps, src, mx);
        assert_eq!(exit, RunExit::InstructionBudgetExhausted);
        let mut expected = entered_mmx(&before, None);
        set_xmm(
            &mut expected,
            1,
            HI & !0xffff_ffff_ffff_ffff | u128::from(0xbf80_0000u32) << 32 | u128::from(low),
        );
        expected.sse.as_mut().unwrap().mxcsr = mx | PE;
        expected.rip += 3;
        assert_eq!(after, expected, "{mx:#x}");
    }
    // Exact values raise nothing. The register source is not rewritten: its exponent field stays.
    let (_, before, after) = run_mmx(Mmx::Cvtpi2ps, pair(3, 0xffff_fffe), MXCSR);
    assert_eq!(
        xmm(&after, 1),
        HI & !0xffff_ffff_ffff_ffff | u128::from(0xc000_0000u32) << 32 | u128::from(0x4040_0000u32)
    );
    assert_eq!(after.sse.as_ref().unwrap().mxcsr, MXCSR);
    assert_eq!(
        after.x87.as_ref().unwrap().registers[5],
        before.x87.as_ref().unwrap().registers[5]
    );
    assert_eq!(after.x87.as_ref().unwrap().tag_word, 0);
    assert_eq!(after.x87.as_ref().unwrap().status_word & (7 << 11), 0);
    // Unmasked precision: #XM, no XMM change; the hardware has entered MMX state.
    let mx = MXCSR & !PM;
    let (exit, before, after) = run_mmx(Mmx::Cvtpi2ps, src, mx);
    assert_eq!(exit, xm());
    let mut expected = entered_mmx(&before, None);
    expected.sse.as_mut().unwrap().mxcsr = mx | PE;
    assert_eq!(after, expected);
}

#[test]
fn cvtpi2pd_is_exact_and_replaces_the_whole_destination() {
    let src = pair(0x7fff_ffff, 0xffff_fffe);
    let expected = d2([0x41df_ffff_ffc0_0000, 0xc000_0000_0000_0000]);
    for mx in [MXCSR, 0, MXCSR | RC_UP | DAZ] {
        let (exit, before, after) = run_mmx(Mmx::Cvtpi2pd, src, mx);
        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{mx:#x}");
        let mut want = entered_mmx(&before, None);
        set_xmm(&mut want, 1, expected);
        want.rip += 4;
        assert_eq!(after, want, "{mx:#x}");
    }
}

#[test]
fn cvtps2pi_and_cvttps2pi_write_the_mmx_register() {
    // 2.5 and -2.5 as binary32 in the low 64 bits of xmm2; the upper 64 bits play no part.
    let src = pair(0x4020_0000, 0xc020_0000) | u128::from(0x7fc0_0000u32) << 64;
    for (mx, lanes) in [
        (MXCSR, [2i32, -2]),
        (MXCSR | RC_DOWN, [2, -3]),
        (MXCSR | RC_UP, [3, -2]),
        (MXCSR | RC_ZERO, [2, -2]),
    ] {
        let (exit, before, after) = run_mmx(Mmx::Cvtps2pi, src, mx);
        assert_eq!(exit, RunExit::InstructionBudgetExhausted);
        let value = u64::from(lanes[0] as u32) | u64::from(lanes[1] as u32) << 32;
        let mut expected = entered_mmx(&before, Some((5, value)));
        expected.sse.as_mut().unwrap().mxcsr = mx | PE;
        expected.rip += 3;
        assert_eq!(after, expected, "{mx:#x}");
        assert_eq!(after.x87.as_ref().unwrap().registers[5][8..], [0xff, 0xff]);
        // CVTTPS2PI ignores the rounding control.
        let (_, _, truncated) = run_mmx(Mmx::Cvttps2pi, src, mx);
        assert_eq!(
            mm_value(&truncated, 5),
            u64::from(2u32) | u64::from(-2i32 as u32) << 32,
            "{mx:#x}"
        );
    }
    // A NaN or an out-of-range lane is the integer indefinite value and invalid.
    let (_, before, after) = run_mmx(Mmx::Cvtps2pi, pair(0x7fc0_0000, 0x3f80_0000), MXCSR);
    let mut expected = entered_mmx(&before, Some((5, 0x0000_0001_8000_0000)));
    expected.sse.as_mut().unwrap().mxcsr = MXCSR | IE;
    expected.rip += 3;
    assert_eq!(after, expected);
    // Unmasked: #XM, the MMX register is not written (the hardware has entered MMX state).
    let mx = MXCSR & !IM;
    let (exit, before, after) = run_mmx(Mmx::Cvttps2pi, pair(0x7f80_0000, 0), mx);
    assert_eq!(exit, xm());
    let mut expected = entered_mmx(&before, None);
    expected.sse.as_mut().unwrap().mxcsr = mx | IE;
    assert_eq!(after, expected);
}

#[test]
fn cvtpd2pi_and_cvttpd2pi_convert_two_doubles() {
    let src = d2([0x4004_0000_0000_0000, 0xc004_0000_0000_0000]);
    for (op, mx, lanes) in [
        (Mmx::Cvtpd2pi, MXCSR, [2i32, -2]),
        (Mmx::Cvtpd2pi, MXCSR | RC_UP, [3, -2]),
        (Mmx::Cvtpd2pi, MXCSR | RC_DOWN, [2, -3]),
        (Mmx::Cvttpd2pi, MXCSR | RC_UP, [2, -2]),
    ] {
        let (exit, before, after) = run_mmx(op, src, mx);
        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{op:?} {mx:#x}");
        let value = u64::from(lanes[0] as u32) | u64::from(lanes[1] as u32) << 32;
        let mut expected = entered_mmx(&before, Some((5, value)));
        expected.sse.as_mut().unwrap().mxcsr = mx | PE;
        expected.rip += 4;
        assert_eq!(after, expected, "{op:?} {mx:#x}");
    }
    // 1e10 does not fit: the indefinite value in that lane.
    let (_, _, after) = run_mmx(
        Mmx::Cvtpd2pi,
        d2([0x4202_a05f_2000_0000, 0x3ff0_0000_0000_0000]),
        MXCSR,
    );
    assert_eq!(mm_value(&after, 5), 0x0000_0001_8000_0000);
    assert_eq!(after.sse.as_ref().unwrap().mxcsr, MXCSR | IE);
}

#[test]
fn mmx_gates_and_the_pending_x87_exception() {
    for op in ALL_MMX {
        let code = mmx_reg_code(op, 5);
        for (cr0, cr4, vector) in [
            (CR0_BASE | 1 << 2, CR4_ALL, 6u8),       // CR0.EM
            (CR0_BASE | 1 << 3, CR4_ALL, 7),         // CR0.TS: #NM
            (CR0_BASE, CR4_PAE | CR4_OSXMMEXCPT, 6), // CR4.OSFXSR = 0
        ] {
            let mut cpu = mmx_cpu(true, 0x33, 5, 0x0102_0304_0506_0708);
            cpu.cr0 = Some(cr0);
            cpu.cr4 = Some(cr4);
            let before = cpu.clone();
            let (result, after) = run_fp_init(cpu, &mut Ram::new(&code));
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector,
                    error_code: None,
                    linear_address: None
                }),
                "{op:?}"
            );
            assert_eq!(after, before, "{op:?}");
        }
        // A pending unmasked x87 exception (ES set, an unmasked flag in the status word) is #MF with CR0.NE = 1.
        let mut cpu = mmx_cpu(true, 0x33, 5, 0x0102_0304_0506_0708);
        let x87 = cpu.x87.as_mut().unwrap();
        x87.control_word = 0x0340;
        x87.status_word |= 0x81;
        let before = cpu.clone();
        let (result, after) = run_fp_init(cpu, &mut Ram::new(&code));
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 16,
                error_code: None,
                linear_address: None
            }),
            "{op:?}"
        );
        assert_eq!(after, before, "{op:?}");
        // LOCK is #UD.
        let mut locked = vec![0xf0];
        locked.extend(&code);
        let mut cpu = self::cpu();
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&locked)),
            Ok(fault_of(6, None)),
            "lock {op:?}"
        );
        assert_eq!(cpu, before);
        // A reserved MXCSR bit is Unsupported.
        let mut cpu = mmx_cpu(false, 0x33, 5, 0x0102_0304_0506_0708);
        cpu.sse.as_mut().unwrap().mxcsr = MXCSR | 0x1_0000;
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&code)),
            Ok(StepOutcome::Unsupported {
                opcode: op.opcode_byte()
            }),
            "{op:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn mmx_memory_forms_sizes_alignment_and_the_x87_state() {
    for op in ALL_MMX {
        let (width, aligned) = op.memory();
        // The destination register number in the reg field: XMM 1 or MM 4 (avoiding the address registers).
        let reg = if op.from_mm() { 1 } else { 4 };
        for (name, address, rex_b, registers, effective) in address_forms(0) {
            let code = mmx_mem_code(op, reg, &address, rex_b);
            let mut cpu = mmx_cpu(false, 0x33, 4, 0x1122_3344_5566_7788);
            set_xmm(&mut cpu, 1, HI);
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            // 3.0 and 1.0 as two binary32, or 3 and 1 as two doublewords; for the double forms as binary64.
            let data: u128 = match op {
                Mmx::Cvtpi2ps | Mmx::Cvtpi2pd => pair(3, 1),
                Mmx::Cvtps2pi | Mmx::Cvttps2pi => pair(0x4040_0000, 0x3f80_0000),
                Mmx::Cvtpd2pi | Mmx::Cvttpd2pi => {
                    d2([0x4008_0000_0000_0000, 0x3ff0_0000_0000_0000])
                }
            };
            ram.put(effective, &data.to_le_bytes()[..width]);
            ram.put(effective + width as u64, &[0xa5; 16]);
            let mut cpu_after = cpu.clone();
            assert_eq!(
                step(&mut cpu_after, &mut ram),
                Ok(StepOutcome::Advanced),
                "{op:?} {name}"
            );
            assert_eq!(ram.reads, vec![(effective, width)], "{op:?} {name}");
            assert!(ram.writes.is_empty());
            let mut expected = match op {
                // No MMX register is touched: the x87 state is left alone (physical oracle).
                Mmx::Cvtpi2ps | Mmx::Cvtpi2pd => before.clone(),
                _ => entered_mmx(&before, Some((4, 0x0000_0001_0000_0003))),
            };
            match op {
                Mmx::Cvtpi2ps => set_xmm(
                    &mut expected,
                    1,
                    HI & !0xffff_ffff_ffff_ffff
                        | u128::from(0x3f80_0000u32) << 32
                        | u128::from(0x4040_0000u32),
                ),
                Mmx::Cvtpi2pd => set_xmm(
                    &mut expected,
                    1,
                    d2([0x4008_0000_0000_0000, 0x3ff0_0000_0000_0000]),
                ),
                _ => {}
            }
            expected.rip += code.len() as u64;
            assert_eq!(cpu_after, expected, "{op:?} {name}");
        }
        // Alignment: only the 128-bit forms need it.
        for delta in [1u64, 4, 8, 12] {
            let (name, address, rex_b, registers, effective) = address_forms(delta).remove(1);
            let code = mmx_mem_code(op, reg, &address, rex_b);
            let mut cpu = mmx_cpu(false, 0x33, 4, 0x1122_3344_5566_7788);
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            ram.put(effective, &[0u8; 16]);
            let outcome = step(&mut cpu, &mut ram);
            if aligned {
                assert_eq!(outcome, Ok(fault_of(13, Some(0))), "{op:?} {name} +{delta}");
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            } else {
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{op:?} {name} +{delta}");
                assert_eq!(ram.reads, vec![(effective, width)]);
            }
        }
        // A backend fault leaves everything unchanged, the x87 state included; a non-canonical address is #GP(0).
        let code = mmx_mem_code(op, reg, &[0x03], false);
        let mut cpu = mmx_cpu(false, 0x33, 4, 0x1122_3344_5566_7788);
        // The operand crosses into the faulting page (a 16-byte aligned operand lies entirely in it).
        cpu.gpr[Register::Rbx as usize] = if aligned {
            DATA + 0x1000
        } else {
            DATA + 0x1000 - width as u64 + 4
        };
        let before = cpu.clone();
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(DATA + 0x1000);
        match step(&mut cpu, &mut ram) {
            Ok(StepOutcome::Fault(fault)) => assert_eq!(fault.vector, 14, "{op:?}"),
            Err(MemoryError::Architectural(fault)) => assert_eq!(fault.vector, 14, "{op:?}"),
            other => panic!("{op:?}: {other:?}"),
        }
        assert_eq!(cpu, before, "{op:?}");
        let mut cpu = mmx_cpu(false, 0x33, 4, 0x1122_3344_5566_7788);
        cpu.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
        let before = cpu.clone();
        assert_eq!(
            step(&mut cpu, &mut Ram::new(&code)),
            Ok(fault_of(13, Some(0))),
            "{op:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn mmx_decoder_numbers_the_registers_and_refuses_the_prefix_mixes() {
    use SseFpMiscKind::{Cvtpd2pi, Cvtpi2pd, Cvtpi2ps, Cvtps2pi, Cvttpd2pi, Cvttps2pi};
    // (bytes, kind, shape, destination, source register): REX.R extends the XMM register, REX.B the XMM source only.
    let registers: [(&[u8], SseFpMiscKind, SseFpShape, u8, u8); 10] = [
        (
            &[0x44, 0x0f, 0x2a, 0xec],
            Cvtpi2ps,
            SseFpShape::Packed32,
            13,
            4,
        ), // XMM13, MM4
        (
            &[0x41, 0x0f, 0x2a, 0xc9],
            Cvtpi2ps,
            SseFpShape::Packed32,
            1,
            1,
        ), // REX.B does not extend the MM source
        (
            &[0x66, 0x44, 0x0f, 0x2a, 0xfb],
            Cvtpi2pd,
            SseFpShape::Packed64,
            15,
            3,
        ),
        (
            &[0x40, 0x0f, 0x2d, 0xd1],
            Cvtps2pi,
            SseFpShape::Packed32,
            2,
            1,
        ), // MM2, XMM1
        (
            &[0x41, 0x0f, 0x2d, 0xd1],
            Cvtps2pi,
            SseFpShape::Packed32,
            2,
            9,
        ), // REX.B extends the XMM source
        (
            &[0x44, 0x0f, 0x2c, 0xd1],
            Cvttps2pi,
            SseFpShape::Packed32,
            2,
            1,
        ), // REX.R does not extend the MM destination
        (
            &[0x66, 0x40, 0x0f, 0x2d, 0xe2],
            Cvtpd2pi,
            SseFpShape::Packed64,
            4,
            2,
        ),
        (
            &[0x66, 0x45, 0x0f, 0x2c, 0xf7],
            Cvttpd2pi,
            SseFpShape::Packed64,
            6,
            15,
        ),
        (
            &[0x4a, 0x0f, 0x2c, 0xd1],
            Cvttps2pi,
            SseFpShape::Packed32,
            2,
            1,
        ), // REX.W/X are ignored
        (
            &[0x66, 0x4a, 0x0f, 0x2a, 0xc2],
            Cvtpi2pd,
            SseFpShape::Packed64,
            0,
            2,
        ),
    ];
    for (bytes, kind, shape, destination, source) in registers {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        assert_eq!(
            instruction.operation,
            Operation::SseFpMisc {
                kind,
                shape,
                destination,
                source: Sse3Source::Register(source),
                immediate: 0
            },
            "{bytes:02x?}"
        );
    }
    // Memory forms: the source size follows the instruction.
    for (bytes, kind, size) in [
        (&[0x0f, 0x2a, 0x03][..], Cvtpi2ps, 8u8),
        (&[0x66, 0x0f, 0x2a, 0x03], Cvtpi2pd, 8),
        (&[0x0f, 0x2d, 0x03], Cvtps2pi, 8),
        (&[0x0f, 0x2c, 0x03], Cvttps2pi, 8),
        (&[0x66, 0x0f, 0x2d, 0x03], Cvtpd2pi, 16),
        (&[0x66, 0x0f, 0x2c, 0x03], Cvttpd2pi, 16),
    ] {
        let instruction = decode(bytes).unwrap();
        let Operation::SseFpMisc {
            kind: k,
            source: Sse3Source::Memory(_),
            ..
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!(k, kind, "{bytes:02x?}");
        assert_eq!(
            kind.source_access(if kind == Cvtpd2pi || kind == Cvttpd2pi {
                SseFpShape::Packed64
            } else {
                SseFpShape::Packed32
            })
            .0,
            u64::from(size)
        );
    }
    // F2/F3 select the scalar forms, 66 together with them and the other mixes stay refused.
    for bytes in [
        &[0x66, 0xf3, 0x0f, 0x2a, 0xc1][..],
        &[0xf2, 0x66, 0x0f, 0x2d, 0xc1],
        &[0x66, 0x66, 0x0f, 0x2a, 0xc1],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Unsupported { .. })),
            "{bytes:02x?}"
        );
    }
}

/// The narrow profile decoder handles the exact XMM0/MM1 and MM0/XMM1 register forms and the [rax] forms, which the
/// executor rewrites into the general operation; with a REX prefix the general decoder handles the same instruction.
#[test]
fn mmx_narrow_forms_and_general_forms_agree() {
    let mut seed = 0x1357_9bdf_2468_ace0u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let f32_pool = [
        0u32,
        0x8000_0000,
        0x3f80_0000,
        0xbf80_0000,
        0x4020_0000,
        0x7f80_0000,
        0x7fc0_0001,
        1,
        0x4f00_0000,
        0xcf00_0000,
        0x3fc0_0000,
    ];
    for op in ALL_MMX {
        for round in 0..300 {
            let lane = |n: &mut dyn FnMut() -> u64| -> u64 {
                match op {
                    Mmx::Cvtpi2ps | Mmx::Cvtpi2pd => n() & 0xffff_ffff,
                    Mmx::Cvtps2pi | Mmx::Cvttps2pi => {
                        u64::from(f32_pool[(n() % f32_pool.len() as u64) as usize])
                    }
                    Mmx::Cvtpd2pi | Mmx::Cvttpd2pi => [
                        0u64,
                        0x3ff0_0000_0000_0000,
                        0x4004_0000_0000_0000,
                        0x7ff8_0000_0000_0000,
                        0x41e0_0000_0000_0000,
                        0xc1e0_0000_0010_0000,
                    ][(n() % 6) as usize],
                }
            };
            let data: u128 = if matches!(op, Mmx::Cvtpd2pi | Mmx::Cvttpd2pi) {
                u128::from(lane(&mut next)) | u128::from(lane(&mut next)) << 64
            } else {
                u128::from(lane(&mut next))
                    | u128::from(lane(&mut next)) << 32
                    | u128::from(next()) << 64
            };
            let mx = [
                MXCSR,
                MXCSR | DAZ,
                MXCSR & !IM,
                0,
                MXCSR | 3 << 13,
                MXCSR & !PM,
            ][round % 6];
            let (xmm_n, mm_n) = if op.from_mm() {
                (0usize, 1usize)
            } else {
                (1, 0)
            };
            for memory in [false, true] {
                let mut codes = vec![];
                for rex in [false, true] {
                    let mut code = vec![];
                    if op.packed_double() {
                        code.push(0x66);
                    }
                    if rex {
                        code.push(0x40);
                    }
                    code.extend([0x0f, op.opcode()]);
                    // [rax]; otherwise xmm0, mm1 (CVTPI2PS/PD) or mm0, xmm1.
                    code.push(if memory { 0x00 } else { 0xc1 });
                    codes.push(code);
                }
                let mut states = vec![];
                for code in &codes {
                    let mut cpu = mmx_cpu(true, 0x0f, 1, 0x5555_6666_7777_8888);
                    cpu.x87.as_mut().unwrap().registers[0] = reg80(0x0102_0304_0506_0708);
                    cpu.sse.as_mut().unwrap().mxcsr = mx;
                    let mut ram = Ram::new(code);
                    if op.from_mm() {
                        cpu.x87.as_mut().unwrap().registers[mm_n] = reg80(data as u64);
                        set_xmm(&mut cpu, xmm_n, HI);
                    } else {
                        set_xmm(&mut cpu, xmm_n, data);
                    }
                    cpu.gpr[Register::Rax as usize] = DATA + 0x40;
                    ram.put(DATA + 0x40, &data.to_le_bytes());
                    let (result, mut after) = run_fp_init(cpu, &mut ram);
                    // The REX form is one byte longer.
                    if result.exit == RunExit::InstructionBudgetExhausted {
                        after.rip -= code.len() as u64;
                    }
                    states.push((result.exit, after, ram.reads));
                }
                assert_eq!(
                    states[0].0, states[1].0,
                    "{op:?} memory={memory} {data:#x} {mx:#x}"
                );
                assert_eq!(
                    states[0].1, states[1].1,
                    "{op:?} memory={memory} {data:#x} {mx:#x}"
                );
                assert_eq!(states[0].2, states[1].2, "{op:?} memory={memory}");
            }
        }
    }
}

// ---- hardware vectors ----

const GOLDEN: &[Row] = &[
    (
        Op::Cvtsi2sd,
        true,
        0x3f80,
        0xedcba988,
        0x4bc354e678e60a5a6b548066cfaf0057,
        Expect::Ok(0x4bc354e678e60a5a41edb97531000000, 0x3f80),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x7f80,
        0x20000000000003,
        0x6122671b257c27311e00310f3ece6769,
        Expect::Ok(0x6122671b257c27311e00310f40400000, 0x7f80),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x9f80,
        0xc000000000000001,
        0x3b4b0280bd5fd3afa4ff57ba44e8791,
        Expect::Ok(0x3b4b0280bd5fd3afa4ff57bde800000, 0x9fa0),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x54ce,
        0x3e,
        0x9ad0c7e7667ab5a793fcea1cbebe3740,
        Expect::Ok(0x9ad0c7e7667ab5a7404f000000000000, 0x54ce),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x3ac6,
        0x12e1933d49f,
        0xf0aff97cd121659b5f20858a6cbe0d51,
        Expect::Ok(0xf0aff97cd121659b41b933d49f000000, 0x3ac6),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x177b,
        0x5eef8c97fb98,
        0x98e7df2f8c0a107c415b754c84174139,
        Expect::Ok(0x98e7df2f8c0a107cc1dcda011a000000, 0x177b),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x21a1,
        0x63a30a8,
        0x9569ddc9feeeb1c5e0179beee5488e8b,
        Expect::Ok(0x9569ddc9feeeb1c54198e8c2a0000000, 0x21a1),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x8602,
        0xecacb,
        0xbb2ebb2fa9b31f594b74354d7979435,
        Expect::Ok(0xbb2ebb2fa9b31f5412d959600000000, 0x8602),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x63ce,
        0x33f3,
        0xf89787f7baabd4224664d75af9680af1,
        Expect::Ok(0xf89787f7baabd42240c9f98000000000, 0x63ce),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x7bf3,
        0x1b2d89e07eb,
        0x393763cb63411b95318dbd777326ffc9,
        Expect::Ok(0x393763cb63411b95c1c3b0fc0a800000, 0x7bf3),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x63b,
        0x27b3e8fab,
        0x66863f6089e3fd1f2caba1fac994807d,
        Expect::Ok(0x66863f6089e3fd1f41decfa3eac00000, 0x63b),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xc1ab,
        0x2ab2,
        0x8629530c29880bf871afe4970492f2cc,
        Expect::Ok(0x8629530c29880bf840c5590000000000, 0xc1ab),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x4d69,
        0x5,
        0xbd8d87604af459cc704a068ba1dd4a05,
        Expect::Ok(0xbd8d87604af459cc4014000000000000, 0x4d69),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x5b9c,
        0x4bb80296779585,
        0x119f644820f3e0adc7caa072a585e148,
        Expect::Ok(0x119f644820f3e0adc1da621a9ec00000, 0x5b9c),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x447e,
        0x1c2e,
        0x6e680d94f4a56c1b1c7fafcbb2ce3908,
        Expect::Ok(0x6e680d94f4a56c1b40bc2e0000000000, 0x447e),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x1e83,
        0xfb,
        0x6900c29d4f05bc807b30887ffe7fac50,
        Expect::Ok(0x6900c29d4f05bc80406f600000000000, 0x1e83),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x7821,
        0x0,
        0xece1d141657c4c3ce29247faa97f41a0,
        Expect::Ok(0xece1d141657c4c3c0000000000000000, 0x7821),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xc7b9,
        0x5bdad4a7,
        0xdad4a87991b1133ee69b8adc3b9b398b,
        Expect::Ok(0xdad4a87991b1133e41d6f6b529c00000, 0xc7b9),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x18da,
        0x1d516d50c579265,
        0x710e533a3248d1e5db6a93fc13135c19,
        Expect::Ok(0x710e533a3248d1e541a8af24ca000000, 0x18da),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xad81,
        0x92c1569ace,
        0x68021fcdfbd6d34067584f6b94f1f156,
        Expect::Ok(0x68021fcdfbd6d340c1cf54b299000000, 0xad81),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xb59a,
        0x4,
        0xbf1a019a20268e15ee9c24bbb17d2d18,
        Expect::Ok(0xbf1a019a20268e154010000000000000, 0xb59a),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x2179,
        0x61666d8,
        0xe12a4620e6c9c521ff47635a4c36007c,
        Expect::Ok(0xe12a4620e6c9c5214198599b60000000, 0x2179),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x704,
        0x1d9a7e3529,
        0xa3f32a5f42ccff5d8991cc2d23bae287,
        Expect::Ok(0xa3f32a5f42ccff5dc1d96072b5c00000, 0x704),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x2689,
        0x1e783068f4aae4,
        0x8994072352cd2a203676a5e777cec98c,
        Expect::Ok(0x8994072352cd2a2041da3d2ab9000000, 0x2689),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x1643,
        0x8b650d29,
        0x3a11c5132855e7faecaf4008f1b09e9d,
        Expect::Ok(0x3a11c5132855e7fac1dd26bcb5c00000, 0x1643),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x62ca,
        0x88d1,
        0x566a0c8d25ca421e4a6ca5b9deb10633,
        Expect::Ok(0x566a0c8d25ca421e40e11a2000000000, 0x62ca),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x826f,
        0x187eb2f2a,
        0x4ce8f5508d97e1be8e843dfbf0d139f1,
        Expect::Ok(0x4ce8f5508d97e1bec1de053435800000, 0x826f),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xe494,
        0x13d50b8,
        0xbe89abeabafb9d28317223f1f46b71f3,
        Expect::Ok(0xbe89abeabafb9d284173d50b80000000, 0xe494),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x9792,
        0x48171984b165a8,
        0x14509516578bf73c4fb265b6db79c7aa,
        Expect::Ok(0x14509516578bf73cc1ded3a696000000, 0x9792),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x61d,
        0x7,
        0xe5cee8712de38ac16c5877850ac4ccd1,
        Expect::Ok(0xe5cee8712de38ac1401c000000000000, 0x61d),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x30c7,
        0x6d,
        0x228cf2d92683ec9368aee5ba92b0cd43,
        Expect::Ok(0x228cf2d92683ec93405b400000000000, 0x30c7),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x99ee,
        0x2d8e3f15d,
        0x15a6f587a9b4d1c7a502b4d62bab0290,
        Expect::Ok(0x15a6f587a9b4d1c7c1c38e0751800000, 0x99ee),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xd86f,
        0xd259e1322b,
        0xb23d1faba3580d7bf21ec28be74bb415,
        Expect::Ok(0xb23d1faba3580d7b41d6784c8ac00000, 0xd86f),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x7391,
        0x7277c96,
        0x1f1d4a6d5f8208a20676f4d914edcf79,
        Expect::Ok(0x1f1d4a6d5f8208a2419c9df258000000, 0x7391),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x820a,
        0x2650dcf691c16e,
        0x53b01401d8d753dd388a86d57a0c59da,
        Expect::Ok(0x53b01401d8d753ddc1a2dc7d24000000, 0x820a),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xb4a4,
        0x749828b6873,
        0x2bc6f81873226ebc9283d7a62441a5a8,
        Expect::Ok(0x2bc6f81873226ebcc1df5d25e3400000, 0xb4a4),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x2591,
        0x54899022f,
        0x7692901ebfc294c304ae16b1ab090f1d,
        Expect::Ok(0x7692901ebfc294c341d226408bc00000, 0x2591),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x8e58,
        0x1b0ea30444,
        0xe109a126f4bc6095a030d9c079e3a9e3,
        Expect::Ok(0xe109a126f4bc609541ad460888000000, 0x8e58),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x70e9,
        0x18da1c654,
        0xc9167244049c6fe23e08394b3f1bc42e,
        Expect::Ok(0xc9167244049c6fe2c1dc978e6b000000, 0x70e9),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0xa17b,
        0x4fb46c909607a,
        0x9c237e3caaf10a72a54cb425cd03e6f0,
        Expect::Ok(0x9c237e3caaf10a72c1cb7b4fc3000000, 0xa17b),
    ),
    (
        Op::Cvtsi2sd,
        false,
        0x396f,
        0x1c1ae,
        0x63952c18e05d026cf5a0cd3adca83f39,
        Expect::Ok(0x63952c18e05d026c40fc1ae000000000, 0x396f),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x2e73,
        0x9134f,
        0xf79a60ca278c9648e03f3562892830b4,
        Expect::Ok(0xf79a60ca278c96484122269e00000000, 0x2e73),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x549c,
        0x372d5692ae9,
        0x136a0d6e383de1701a70ce6a99a1a9df,
        Expect::Ok(0x136a0d6e383de170428b96ab49574800, 0x549c),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x490e,
        0x1eb9f235243c2913,
        0x3344c0a10b843d7e1d8e5891bf723ed3,
        Expect::Fault(0x492e),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x6396,
        0x93a8,
        0x4f75fd675031201a9aa9fe80e1ec633,
        Expect::Ok(0x4f75fd67503120140e2750000000000, 0x6396),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xd899,
        0x3fa71b,
        0xda9708cc9790311c5054429040a78bb2,
        Expect::Ok(0xda9708cc9790311c414fd38d80000000, 0xd899),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x4734,
        0x47f50,
        0xbe6bf5d6271604a9abfdec65a3ae785a,
        Expect::Ok(0xbe6bf5d6271604a94111fd4000000000, 0x4734),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x8b0f,
        0x2005ff209c2,
        0x9bc8dd33752448a94130ae23b69f3b7c,
        Expect::Ok(0x9bc8dd33752448a9428002ff904e1000, 0x8b0f),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x7602,
        0x420,
        0xdaeb77da3bf5dd052caaa7f53cfafc76,
        Expect::Ok(0xdaeb77da3bf5dd054090800000000000, 0x7602),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x3098,
        0xb54442917e3,
        0xf10f9cd6c66e72d38d8aaca0e5d843b1,
        Expect::Ok(0xf10f9cd6c66e72d342a6a888522fc600, 0x3098),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xdde9,
        0x6ef73faba4c,
        0x5eb036d1f7949c1bce04ffcfd46b3f8c,
        Expect::Ok(0x5eb036d1f7949c1b429bbdcfeae93000, 0xdde9),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x9685,
        0xaa0dd21e3a33e,
        0x3fe167bd5d2247fef215f11b02471dd,
        Expect::Ok(0x3fe167bd5d2247f432541ba43c7467c, 0x9685),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x4309,
        0x478,
        0x403406de1e632228742cbf4bc589d579,
        Expect::Ok(0x403406de1e6322284091e00000000000, 0x4309),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xc3f4,
        0xb45d6f4d7c,
        0x124db4e11e4da81a47ce84472c34e808,
        Expect::Ok(0x124db4e11e4da81a42668bade9af8000, 0xc3f4),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x855d,
        0x11a292b,
        0x6b362170ccebf92e7d1e6cc703988eb,
        Expect::Ok(0x6b362170ccebf924171a292b0000000, 0x855d),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xf13d,
        0x321b33016c56,
        0x64d95453bba3ea118da311a93874d815,
        Expect::Ok(0x64d95453bba3ea1142c90d9980b62b00, 0xf13d),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x69c2,
        0x76402566ec985,
        0x702b277bdf7fba067066a5f1f54d99b6,
        Expect::Ok(0x702b277bdf7fba06431d900959bb2614, 0x69c2),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x3950,
        0x25fb76ba8,
        0xfcdaa28ea3691c53bd8391f8b288fcb9,
        Expect::Ok(0xfcdaa28ea3691c534202fdbb5d400000, 0x3950),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xcf9e,
        0x24a4f83f672ba6,
        0x74fdeea79257195d5216c149f9ff07ff,
        Expect::Ok(0x74fdeea79257195d4342527c1fb395d3, 0xcf9e),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x1ebe,
        0xe9957d,
        0xfa293d2bb5809c7fb87bc108d7f01e3b,
        Expect::Ok(0xfa293d2bb5809c7f416d32afa0000000, 0x1ebe),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xce4b,
        0xbc6d55bb,
        0x50b905873abaffdef50dbf27695a1cc6,
        Expect::Ok(0x50b905873abaffde41e78daab7600000, 0xce4b),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xa844,
        0x354f8ca8355,
        0xa9abe12cba511d03e01dd1f191433edd,
        Expect::Ok(0xa9abe12cba511d03428aa7c6541aa800, 0xa844),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x4f29,
        0x340583f8dd,
        0xf5017d274202af354166e4553e4428f1,
        Expect::Ok(0xf5017d274202af35424a02c1fc6e8000, 0x4f29),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x7ec9,
        0xf9ae63,
        0x839fbdda1e1f837cff5c1be0f03960ef,
        Expect::Ok(0x839fbdda1e1f837c416f35cc60000000, 0x7ec9),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x9a9d,
        0x21ad5,
        0x956eb9e9b7c0a8ec4c49cb153d7afb3f,
        Expect::Ok(0x956eb9e9b7c0a8ec4100d6a800000000, 0x9a9d),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x8004,
        0x2692070c68bed,
        0xbfbb7a601be6632afc201ca838545be5,
        Expect::Ok(0xbfbb7a601be6632a4303490386345f68, 0x8004),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xa75b,
        0xe7a,
        0x1fa7af83bcb916ef6f311311db931b5b,
        Expect::Ok(0x1fa7af83bcb916ef40acf40000000000, 0xa75b),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x799a,
        0xcd512,
        0xe03ee51f0608ca5a912c119f14ca425b,
        Expect::Ok(0xe03ee51f0608ca5a4129aa2400000000, 0x799a),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x87,
        0x19003e5cab68fb,
        0xfa1f0a21f13ed17462b7fae12ba19caf,
        Expect::Ok(0xfa1f0a21f13ed1744339003e5cab68fb, 0x87),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x5812,
        0x2e9edcbc74,
        0xa2e21743024ba8c84443e137eed6f39e,
        Expect::Ok(0xa2e21743024ba8c842474f6e5e3a0000, 0x5812),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x4c22,
        0x4f3,
        0x9c79eb84a5990895ecebdfa83e2fe8c1,
        Expect::Ok(0x9c79eb84a59908954093cc0000000000, 0x4c22),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x9fe0,
        0x1b32aed93a,
        0xd71d89c60e21790e6ac2bf12008b26a2,
        Expect::Ok(0xd71d89c60e21790e423b32aed93a0000, 0x9fe0),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x829b,
        0x3f44,
        0x6706bc46389c648de3ee24df75dcfe46,
        Expect::Ok(0x6706bc46389c648d40cfa20000000000, 0x829b),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x7284,
        0xd6,
        0x12d0282f5e718af242f1db57e1403606,
        Expect::Ok(0x12d0282f5e718af2406ac00000000000, 0x7284),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xb33e,
        0x519b,
        0xba4a6ad8d7341bff484b9b8d695953de,
        Expect::Ok(0xba4a6ad8d7341bff40d466c000000000, 0xb33e),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x930,
        0x7ad70d9d9,
        0xb34a8ebd13b4c08be1101bf03ea8af0e,
        Expect::Ok(0xb34a8ebd13b4c08b421eb5c367640000, 0x930),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xc880,
        0xe0f8cb,
        0x758672208bd08c8e16cb80a694aa77d5,
        Expect::Ok(0x758672208bd08c8e416c1f1960000000, 0xc880),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x2672,
        0x1cbc7d6c8dac6caf,
        0xf0eca25b9730889b02aa893fa04a788a,
        Expect::Fault(0x2672),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0xef47,
        0x29aada06acc874fc,
        0xc1e5f736c853dd7ba5f0ed40d255549a,
        Expect::Fault(0xef67),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x9c17,
        0x4938,
        0xb65de1db0c0043d1c1278e5205332359,
        Expect::Ok(0xb65de1db0c0043d1c1278e5246927000, 0x9c17),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x5760,
        0x4,
        0xf189f59d773531a9756d8340c115f64d,
        Expect::Ok(0xf189f59d773531a9756d834040800000, 0x5760),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xe26a,
        0x1369ccfa2,
        0xff3a13582101553b3156c69f1e2ed38,
        Expect::Fault(0xe26a),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x9d48,
        0x1d1db8656e,
        0xb94d98d7dc24260c59bf082b25f75dc1,
        Expect::Ok(0xb94d98d7dc24260c59bf082b4dedc32b, 0x9d68),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xe71b,
        0xd0f15e49f607b5,
        0x3f847972e72000c69b0e39d2eee3675c,
        Expect::Fault(0xe73b),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x64d9,
        0x6618b1,
        0xc91367a81988ed1d3b666adf360912a0,
        Expect::Ok(0xc91367a81988ed1d3b666adf4acc3162, 0x64d9),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x5c5e,
        0x11f2c55,
        0x7d644c78db943b96492af03ca03d330,
        Expect::Ok(0x7d644c78db943b96492af034b8f962b, 0x5c7e),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xacf9,
        0x213978,
        0xd26e4525f7561e65f3377784229a7230,
        Expect::Ok(0xd26e4525f7561e65f33777844a04e5e0, 0xacf9),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xc918,
        0x10318,
        0xc3e631bc3816925dc535d04a2fd85403,
        Expect::Ok(0xc3e631bc3816925dc535d04a47818c00, 0xc918),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xb3a2,
        0x1a4d8312d7,
        0x4493b146231a3c1c196132594a4f5747,
        Expect::Ok(0x4493b146231a3c1c196132594e9b0625, 0xb3a2),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xddd,
        0xaeeaea447ce992,
        0x4d521f5cbb82435fc4e9aee30a054107,
        Expect::Fault(0xdfd),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xcd6e,
        0x170,
        0x9a9f098b1f8763a12bad06689136b422,
        Expect::Ok(0x9a9f098b1f8763a12bad066843b80000, 0xcd6e),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x6b41,
        0x1b85e2b1c,
        0xac23793a9334303be54a6f03a2ad5f11,
        Expect::Fault(0x6b61),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x5108,
        0x122ad09,
        0x75398dfd306624453bd536cfe228f40,
        Expect::Ok(0x75398dfd306624453bd536c4b915685, 0x5128),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xbbb7,
        0x84e3,
        0x4ca4ea340188bed3e09bc68c130a814f,
        Expect::Ok(0x4ca4ea340188bed3e09bc68c4704e300, 0xbbb7),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x9b69,
        0x3ad9ce,
        0xa63b0009957a4c69526c52055f799732,
        Expect::Ok(0xa63b0009957a4c69526c52054a6b6738, 0x9b69),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x5ff6,
        0x647,
        0xf6c0084d7cb609a6d767e343cd5c5d11,
        Expect::Ok(0xf6c0084d7cb609a6d767e34344c8e000, 0x5ff6),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x1f31,
        0x39fc7107,
        0x16bd62698a1c047e8c3b387071d3fe0d,
        Expect::Ok(0x16bd62698a1c047e8c3b38704e67f1c4, 0x1f31),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xf413,
        0x3c,
        0x6903eb3c7207af88dc5edfbdbebb51df,
        Expect::Ok(0x6903eb3c7207af88dc5edfbd42700000, 0xf413),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xcc3c,
        0xd9f25fd,
        0xbeae708c3efc32870c6b442cafcdf83e,
        Expect::Fault(0xcc3c),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xebde,
        0x21d4cd719bc9,
        0xbffc369365ad1d108b301c36eb3e50aa,
        Expect::Fault(0xebfe),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xe116,
        0x4e,
        0xbfb1091597c1d24d755dde7e9981c87f,
        Expect::Ok(0xbfb1091597c1d24d755dde7e429c0000, 0xe116),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x2981,
        0x131e95ae162e,
        0x498a2b51cfb22ec803702df8c9290424,
        Expect::Fault(0x29a1),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xbc08,
        0x1df2f60,
        0x5455ee14f62e24769797486b8d072fc7,
        Expect::Ok(0x5455ee14f62e24769797486b4bef97b0, 0xbc08),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xbca0,
        0x72,
        0x4d3ec019ee766186f2f59b8350f4acf0,
        Expect::Ok(0x4d3ec019ee766186f2f59b8342e40000, 0xbca0),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x671c,
        0x3a18a9,
        0x19bd992cfe31a98a99c789a417a444a4,
        Expect::Ok(0x19bd992cfe31a98a99c789a44a6862a4, 0x671c),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x2d25,
        0x405b4b,
        0x3bb753a637847eb6807077b7b3b9250,
        Expect::Ok(0x3bb753a637847eb6807077b4a80b696, 0x2d25),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x335e,
        0x9,
        0x1796d19f45e61a877f162013d4bd0d35,
        Expect::Ok(0x1796d19f45e61a877f16201341100000, 0x335e),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x667,
        0x3ca61e9f1a6,
        0x60827d233468db2a8ae441ec04de08dd,
        Expect::Fault(0x667),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x3f7b,
        0x46097,
        0x98dd92d52a628f719c2892396feba74d,
        Expect::Ok(0x98dd92d52a628f719c289239488c12e0, 0x3f7b),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x98c7,
        0x2eab8bf4f22f7,
        0xaeff50a388ad08c7c5df1cf6b5551f8a,
        Expect::Ok(0xaeff50a388ad08c7c5df1cf6ce8161ba, 0x98e7),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xbb5c,
        0x3f255399cf18,
        0xd074cf3964b2d00bfdde705ca3283a48,
        Expect::Ok(0xd074cf3964b2d00bfdde705c4ea7339e, 0xbb7c),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x2308,
        0x1d3ffd26,
        0xb73f95b47211f5fedfee38510e6a1bd9,
        Expect::Fault(0x2328),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xf80,
        0x24871a416d0,
        0x1dc29caf17eba7d8ce0c255b5fa44306,
        Expect::Fault(0xfa0),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x64fd,
        0x59fe,
        0x1d12a68fada2af5f95e6a2899f26b841,
        Expect::Ok(0x1d12a68fada2af5f95e6a28946b3fc00, 0x64fd),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x50f4,
        0x9975,
        0xf6d00e14f3abf361607e862895f32dff,
        Expect::Ok(0xf6d00e14f3abf361607e862847197500, 0x50f4),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xa2bd,
        0x4b0a,
        0x6c879cdbbfe6706965f3ee495086375,
        Expect::Ok(0x6c879cdbbfe6706965f3ee446961400, 0xa2bd),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xbe6a,
        0x20798ad6a7e7e5,
        0x1244eb4d89b5d4a607b41bb612450f77,
        Expect::Ok(0x1244eb4d89b5d4a607b41bb6ce256061, 0xbe6a),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x889f,
        0x298,
        0xbb82582b0cad17beaf85881bf3dac187,
        Expect::Ok(0xbb82582b0cad17beaf85881b44260000, 0x889f),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x13cd,
        0xb,
        0xef35897c14169fe554ce4de08ffde591,
        Expect::Ok(0xef35897c14169fe554ce4de041300000, 0x13cd),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x724a,
        0x3fc12c9f3,
        0x6c29415d89b791ae9294f23b11498f06,
        Expect::Ok(0x6c29415d89b791ae9294f23bcc7b4d83, 0x726a),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x24bd,
        0x17f45f5,
        0x20cb58b34df5c0784577f090c70e49eb,
        Expect::Fault(0x24bd),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x8dbc,
        0x317e603859183,
        0xe65b73f0d8202c56d22c64c35a4c72fb,
        Expect::Fault(0x8dbc),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x1cda,
        0x32697f2e44,
        0xc8300336d88d76ecad0609194bd86afa,
        Expect::Ok(0xc8300336d88d76ecad0609194ed2fe5d, 0x1cfa),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x408b,
        0x879d8952fcd,
        0x975f7cd5beaf4d67e61e240aa872c2de,
        Expect::Fault(0x40ab),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x8487,
        0x1b7abd2,
        0x30f4786829e1c528b70ab39d0920126a,
        Expect::Ok(0x30f4786829e1c528b70ab39d4bdbd5e9, 0x8487),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xc743,
        0x7d96aea0b71,
        0x8e4f172fcc685c0b19083f58b06a01e2,
        Expect::Fault(0xc763),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xaef8,
        0x64a6f8e1,
        0xd6e89af3ba9accab71addff2bd698c53,
        Expect::Fault(0xaef8),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x8f0f,
        0x11090ad2,
        0xcc5c72f8bd2f9d3ba5597e3806a193b0,
        Expect::Fault(0x8f2f),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xc163,
        0x5a203b231,
        0x83b7d5b8016e50c598cfa55ab4f59f49,
        Expect::Fault(0xc163),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x1a16,
        0x18cb4d247c7daa,
        0xe0b499243915820a8b275129fc64dd73,
        Expect::Ok(0xe0b499243915820a8b27512959c65a69, 0x1a36),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x801e,
        0x8b608745424cbc9,
        0xc5dd5af251bf4f2fadb72d8631e11c46,
        Expect::Fault(0x803e),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xa510,
        0x1d8d,
        0xd942578043aa78243d6c7e2adb90dea4,
        Expect::Ok(0xd942578043aa78243d6c7e2a45ec6800, 0xa510),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x6f09,
        0x1ad721c,
        0x1354d3bca6e9d43b41d84bce41e83d80,
        Expect::Ok(0x1354d3bca6e9d43b41d84bce4bd6b90e, 0x6f09),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xf4c5,
        0x5a130640533166,
        0x3ef77a77afde4dc1f6a321d4125487be,
        Expect::Ok(0x3ef77a77afde4dc1f6a321d45ab4260c, 0xf4e5),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x8cb5,
        0x0,
        0x778d6b8962ce5691e6b5b011574628e3,
        Expect::Ok(0x778d6b8962ce5691e6b5b01100000000, 0x8cb5),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x5328,
        0x35c5f0,
        0xcff24bde29baf357c27f553b3b09a19b,
        Expect::Ok(0xcff24bde29baf357c27f553b4a5717c0, 0x5328),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x8e8e,
        0x1c77a7ed362ca4,
        0x9bff4d593160b5eb1a8218f956838e43,
        Expect::Fault(0x8eae),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x54b,
        0x9413791de03feb,
        0x504a0a8343aa32b9c5a23643ef079e,
        Expect::Fault(0x56b),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xf0e1,
        0x0,
        0x2f63e852bd7eecc42275e6145dfd3b17,
        Expect::Ok(0x2f63e852bd7eecc42275e61400000000, 0xf0e1),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x4150,
        0x1ced2192e57e6a,
        0x953ce0d050cd17294f5e69bc431fe631,
        Expect::Fault(0x4170),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x910d,
        0x19b5019,
        0xdcd17fdd9ee79fa9f3f513fe719dfe5a,
        Expect::Ok(0xdcd17fdd9ee79fa9f3f513fe4bcda80c, 0x912d),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x601c,
        0x14,
        0xabaea37f24a5fe7724ed86ea26a3178f,
        Expect::Ok(0xabaea37f24a5fe7724ed86ea41a00000, 0x601c),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x81db,
        0x798e0c2aa,
        0xc02ec3a3289cd055d484d4182220a56,
        Expect::Fault(0x81fb),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xaa44,
        0x3f7f9155017dc,
        0xf5dd835437436ba43313514e1907956d,
        Expect::Fault(0xaa64),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xc532,
        0x6be2a115a,
        0xb8a5a47bd8d4c5789a8ed2e2c1365f84,
        Expect::Fault(0xc532),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x5bc0,
        0x61ad54,
        0x51c26921981dedaf680a04f0060f3d2,
        Expect::Ok(0x51c26921981dedaf680a04f4ac35aa8, 0x5bc0),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xf6a6,
        0x1de4a0142,
        0x43d00dae2aaba6d7497e090545f90ab,
        Expect::Ok(0x43d00dae2aaba6d7497e0904fef2500, 0xf6a6),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xdc27,
        0x199d1be65a6229ca,
        0xb12f605b524e133aa4b1a5b6b744aebd,
        Expect::Ok(0xb12f605b524e133aa4b1a5b65dcce8e0, 0xdc27),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x860,
        0x3,
        0x97c016e9f60c9b5f36d861f19314ae0b,
        Expect::Ok(0x97c016e9f60c9b5f36d861f140400000, 0x860),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x5576,
        0xacb44c,
        0x8ca58dbb8ebf3eb4eeb4ff90dbe009a3,
        Expect::Ok(0x8ca58dbb8ebf3eb4eeb4ff904b2cb44c, 0x5576),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x33e1,
        0x5,
        0xd2fa082dfb462e872435978bcffb6435,
        Expect::Ok(0xd2fa082dfb462e872435978b40a00000, 0x33e1),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x47f9,
        0x14fb4b3,
        0x1b9b808467f392b23cedd7a57ebf9024,
        Expect::Fault(0x47f9),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x5dfc,
        0x5d0e25b481ce8a,
        0xad28c1a508ccd1fda543587a5325d076,
        Expect::Ok(0xad28c1a508ccd1fda543587a5aba1c4c, 0x5dfc),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x7818,
        0x3c2b3b782,
        0x8a0e3351ab7abea9672c74f4eebf52ef,
        Expect::Ok(0x8a0e3351ab7abea9672c74f45070aced, 0x7838),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x7c94,
        0xab,
        0xb8d9d99e133e8bd9a7a3ac8d18f39c42,
        Expect::Ok(0xb8d9d99e133e8bd9a7a3ac8d432b0000, 0x7c94),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x18cf,
        0xfa6b658,
        0x296f82b2b7d92ed4a7e63eda87ec419b,
        Expect::Ok(0x296f82b2b7d92ed4a7e63eda4d7a6b66, 0x18ef),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x88fd,
        0x5958c1aaa75fb43,
        0xe7a22bbfd4aa37431f9bb2cc06395739,
        Expect::Fault(0x88fd),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xa541,
        0x1064,
        0x8bc3c91d4c4949177d7bbe247c68192,
        Expect::Ok(0x8bc3c91d4c4949177d7bbe245832000, 0xa541),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x40c0,
        0x2330c64f9c6,
        0x75e8f60e0be95d9b78e821c7c761df0d,
        Expect::Fault(0x40e0),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x5dc5,
        0x26f854177,
        0xf0771ceb0b1719d25f1ffefb5bc559b0,
        Expect::Ok(0xf0771ceb0b1719d25f1ffefb501be151, 0x5de5),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x385d,
        0x4f3e,
        0x81091cfd89bc9f6915d173c94ce82dc7,
        Expect::Ok(0x81091cfd89bc9f6915d173c9469e7c00, 0x385d),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x2c25,
        0x1a37d62b05,
        0x734fab0bab1457521cb0977c8e62efe8,
        Expect::Fault(0x2c25),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0xc7a5,
        0x21e5,
        0x89032828607d195c5216685d58a5780b,
        Expect::Ok(0x89032828607d195c5216685d46079400, 0xc7a5),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x5e80,
        0x20000000000003,
        0xc75f7170f510edcda5dd1278c7ffeb05,
        Expect::Ok(0xc75f7170f510edcd4340000000000002, 0x5ea0),
    ),
    (
        Op::Cvtsi2sd,
        true,
        0x3b80,
        0x1fb482f6ca4c33ca,
        0xf4c44eaa3c53f737829a560e602e2b83,
        Expect::Ok(0xf4c44eaa3c53f73743bfb482f6ca4c33, 0x3ba0),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x40,
        0x7fffffff,
        0xa843dc10fa729ac8f12fce5f8be909af,
        Expect::Fault(0x60),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0x40,
        0xc000000000000001,
        0xa59e339340595b9924196ad11482f941,
        Expect::Ok(0xa59e339340595b9924196ad13f800000, 0x40),
    ),
    (
        Op::Cvtsi2ss,
        false,
        0xfbf,
        0x915601c6a90cb9b5,
        0xb88d32d46ee94c2de7031b71c3207c44,
        Expect::Fault(0xfbf),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x1dc0,
        0x1000001,
        0xe510f58fb090fbc9e68a9004b213f679,
        Expect::Ok(0xe510f58fb090fbc9e68a90044b800000, 0x1de0),
    ),
    (
        Op::Cvtsi2ss,
        true,
        0x1f00,
        0xffffffff00000001,
        0xf62848377cdbd98310e5180e6a1a61fd,
        Expect::Ok(0xf62848377cdbd98310e5180ecf800000, 0x1f20),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x7f80,
        0x5530296c8555d347fd0000000000000,
        0x537f94a0c6bf0094,
        Expect::Ok(0x80000000, 0x7f81),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x5f80,
        0xa080b60ffba23142c000000000000000,
        0xc5fabf05772c9d38,
        Expect::Ok(0xfffffffe, 0x5f80),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x9f80,
        0xefa917bd5bbabf227fe1000000000000,
        0x51e9c705d4c7386b,
        Expect::Ok(0x80000000, 0x9f81),
    ),
    (
        Op::Cvtss2si,
        false,
        0x9f80,
        0xb4a3d0e82404c848faf63c06ffc00000,
        0x14980d25a9539222,
        Expect::Ok(0x80000000, 0x9f81),
    ),
    (
        Op::Cvtss2si,
        false,
        0x0,
        0x8624a7afaad67fd5d5479313ffbfffff,
        0x3eee3fe20daac06b,
        Expect::Fault(0x1),
    ),
    (
        Op::Cvtss2si,
        true,
        0x9f80,
        0x257f5fb2475dc862387b1e51ffbfffff,
        0x792fa57abc41b395,
        Expect::Ok(0x8000000000000000, 0x9f81),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3f80,
        0x37589696ec35a3483d62825a00800000,
        0x641e0cc2e2329523,
        Expect::Ok(0x0, 0x3fa0),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x1fc0,
        0xc9d281cafcca7b577fffffffffffffff,
        0x3f40cf93b5e546f2,
        Expect::Ok(0x8000000000000000, 0x1fc1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x7f80,
        0xda1b364dd2851b0d4004000000000000,
        0x3bad7737e02f0864,
        Expect::Ok(0x2, 0x7fa0),
    ),
    (
        Op::Cvttss2si,
        false,
        0xffc0,
        0x3d94281b1b53f6b49ee5f8867f000000,
        0x7cdabee5704fee22,
        Expect::Ok(0x80000000, 0xffc1),
    ),
    (
        Op::Cvttss2si,
        false,
        0x7f80,
        0x40d352ae55c0db1df5ffb1987e800000,
        0x5f1e75eea064b680,
        Expect::Ok(0x80000000, 0x7f81),
    ),
    (
        Op::Cvttss2si,
        false,
        0x3f80,
        0xfbe68ce5db6305ad2c1d1c327effffff,
        0x58515defb94db822,
        Expect::Ok(0x80000000, 0x3f81),
    ),
    (
        Op::Cvttss2si,
        false,
        0x3f80,
        0xa98ff27fb1c9cf1b95e7197040000000,
        0x6cf775f43e558dd7,
        Expect::Ok(0x2, 0x3f80),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x8d36,
        0xef197e44e0a35c1641dfffffffc00000,
        0xfa23b8959a5a3aee,
        Expect::Ok(0x7fffffff, 0x8d36),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x1daf,
        0x4873459458e2501241dfffffffc00000,
        0xcf813d36185a9d2d,
        Expect::Ok(0x7fffffff, 0x1daf),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x149c,
        0x72c7fabe168b508a80037814dc9e34ac,
        0xb46c386d7eadcc5a,
        Expect::Ok(0x0, 0x14bc),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x9657,
        0x512c09d1a0bfd3e065475c8d917df1e0,
        0xd788c66c18b23f34,
        Expect::Fault(0x9657),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x35e8,
        0xfb5ee0eb17998c0b4004000000000000,
        0x87545d21958d7a9,
        Expect::Ok(0x2, 0x35e8),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xd7bd,
        0x4f8dbfce4d121928800fffffffffffff,
        0xbf2c13bedf124be5,
        Expect::Ok(0x0, 0xd7bd),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x306d,
        0xbbbd8e4b22bbcdff8000000000000000,
        0xfe31ab31bff64849,
        Expect::Ok(0x0, 0x306d),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xef69,
        0xf8d78f2af44c5cd843e0000000000001,
        0xe0d2f484a8a32efb,
        Expect::Fault(0xef69),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xcc30,
        0x39f1d812beec94b1c3e0000000000000,
        0xec2502a62d50c50d,
        Expect::Fault(0xcc31),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xc33a,
        0x68f875031084fac2fff8000000000000,
        0x456410d21f53df67,
        Expect::Fault(0xc33b),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xc1f5,
        0xa68ab9bccf0536e0801a5ecd7b1865fe,
        0xadd5fa1fc1bae43f,
        Expect::Fault(0xc1f5),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x2dba,
        0x3328c8d38f4c1ddc41dfffffffdfffff,
        0x2352b76b1ce0df2c,
        Expect::Fault(0x2dba),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xe571,
        0x4f039229e4baa2f07fd37e568762149e,
        0x72ec1af5146449e8,
        Expect::Fault(0xe571),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xeb94,
        0x2461bed66a5036a341e0000000000001,
        0x423f7af891c7fb3f,
        Expect::Ok(0x80000000, 0xeb95),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x5a48,
        0xb0106afb45e445a6000fe285964be1a8,
        0x900786ded600eae3,
        Expect::Ok(0x0, 0x5a48),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xc71d,
        0x2fda8c89c134641b3fefffffffffffff,
        0x78d677e7efe05091,
        Expect::Fault(0xc73d),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xdef3,
        0x4bde2090a908b0c641e0000000000000,
        0x24742b008359b0d4,
        Expect::Ok(0x80000000, 0xdef3),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xec0,
        0xfc11646709d7eef13fe0000000000000,
        0x39c5e3a0f6aae84b,
        Expect::Fault(0xee0),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x1466,
        0x9858b2a14b746cce4330000000000000,
        0xb5c958133148a58e,
        Expect::Fault(0x1467),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x207d,
        0x7b3d54eda127f808ac6c8a4cbac8b146,
        0x16c537bab1d2cbef,
        Expect::Fault(0x207d),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xddeb,
        0xae8b60a14f899132a0dff292e24901cf,
        0x6b5271a3a7cf0787,
        Expect::Ok(0x0, 0xddeb),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x4e99,
        0xde7aa187abb29803c3dfffffffffffff,
        0x3b58e61869403dc9,
        Expect::Ok(0x80000000, 0x4e99),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x3792,
        0x702cbef6d6a10819eed6af6a774504f7,
        0xa94a2874000dc601,
        Expect::Ok(0x80000000, 0x3793),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x38e2,
        0x9abf9e086bef68443a7ec5a04f76efa7,
        0xe00d6f2d86a6ae6a,
        Expect::Ok(0x0, 0x38e2),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xe118,
        0xd4c3a79a90666a228000000000000001,
        0xb8cb2f614b2a2b9a,
        Expect::Fault(0xe138),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x435c,
        0x3700c1f5d33e1aa93fd0000000000000,
        0x5c0bcdbcfb90a7d5,
        Expect::Fault(0x437c),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x43ab,
        0x49a131f28cd2ef330010000000000001,
        0xa0d5618d6bb3e067,
        Expect::Fault(0x43ab),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x8efd,
        0x827c57e08881f5347fffffffffffffff,
        0x9067dcd8dea23021,
        Expect::Ok(0x80000000, 0x8efd),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xa808,
        0xf52503da2dbb6bc575d621aee1284bd7,
        0xb8a012723d4740e8,
        Expect::Fault(0xa809),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xaeeb,
        0x7166371c4920917af6c2c4ed1a03f499,
        0x98be993d70f91a5b,
        Expect::Ok(0x80000000, 0xaeeb),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x6ede,
        0xc2a218231718c6896f3db7bae1122f6d,
        0xb1d1b9a8e1936ca7,
        Expect::Ok(0x80000000, 0x6edf),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xdb34,
        0xa2e3d9fb68571a437ff0000000000001,
        0xd8cbb363923566bf,
        Expect::Fault(0xdb35),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xb23b,
        0xa0591514a5154f5c8016690f323fe64d,
        0x4bcf1d2f72946bc8,
        Expect::Ok(0xffffffff, 0xb23b),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x1909,
        0xee2faffc2c4f7bcb8010000000000000,
        0xfec8c1a76a93a8a1,
        Expect::Ok(0x0, 0x1929),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xdf9a,
        0x64bfbd0d8f00ec057ffb1064fc4796bd,
        0x21f7e9d03a247b60,
        Expect::Ok(0x80000000, 0xdf9b),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xa1da,
        0x54b014ad2869892b2591a4a5b923bf91,
        0xb8ae9c83c462ed42,
        Expect::Fault(0xa1fa),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xa679,
        0x73cc26bdf6c3b6456f9c84e7476d9b09,
        0x4541c90c6bc7b063,
        Expect::Fault(0xa679),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xf218,
        0x5b6ccde7411ea635fff0000000000000,
        0x3e1ee730f3b9bc3e,
        Expect::Fault(0xf219),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xe5b3,
        0x525ad5c20c73cf6e7ff8000000000001,
        0xbb0128a61c9d24dc,
        Expect::Ok(0x80000000, 0xe5b3),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x7471,
        0xf1a47d8810b0fa7fc1e0000000000000,
        0x322406ed19d5b315,
        Expect::Ok(0x80000000, 0x7471),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x8920,
        0x759f4208a43e373dfff8123456789abc,
        0x1d6d46cb40e1ab0d,
        Expect::Fault(0x8921),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xd994,
        0xfda4c5a3b1556a093fdfffffffffffff,
        0x5caea54dfd6c828a,
        Expect::Ok(0x1, 0xd9b4),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x65e,
        0x2c7f2b0ff22aabb74bce3bdb26f30e9a,
        0x93fb2d7ecd4a2a,
        Expect::Fault(0x65f),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x4421,
        0xecba5a3249153f0dcc7fa3aae9edb5a8,
        0xa92fdeaf493beb65,
        Expect::Fault(0x4421),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xcf7a,
        0x874c7418de1e8a5941e0000000000001,
        0x598f16d96d025f0,
        Expect::Fault(0xcf7a),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x8059,
        0xae332a9403f2429d001fffffffffffff,
        0xaab99bed56579e86,
        Expect::Fault(0x8079),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xfb90,
        0x3473a57da7a91202c1e00000001fffff,
        0x9664912948f1a85a,
        Expect::Ok(0xffffffff80000000, 0xfbb0),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x24c6,
        0x719965631b960712f097e0cc0ed52fb8,
        0x9d7ac96a3980807e,
        Expect::Ok(0x8000000000000000, 0x24c7),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x5b57,
        0x303597473a7315f4be3d4d7d5897890b,
        0xb8b774aea8c27e3a,
        Expect::Ok(0x0, 0x5b77),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xddcd,
        0x995a0660181b2be7199e09f2c4a68f29,
        0x6e323e3c02870979,
        Expect::Ok(0x1, 0xdded),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xaa57,
        0xe94f18fe4dac126dd0336083ff513270,
        0x25c9c0bc18af520a,
        Expect::Fault(0xaa57),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x242b,
        0x8be11b381127836943e0000000000001,
        0xb9978846bec039aa,
        Expect::Fault(0x242b),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x2aab,
        0xf72e25a60e9eed1a4000000000000000,
        0x39da32cddf2f2a21,
        Expect::Ok(0x2, 0x2aab),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x3bdc,
        0x7fbfbba80d42f9d43521f7a26a3f8075,
        0xd93ebbd867fbe154,
        Expect::Ok(0x0, 0x3bfc),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x7f80,
        0x8173df891979e2bb4014000000000000,
        0x22dd268a85ceb6d0,
        Expect::Ok(0x5, 0x7f80),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x1906,
        0x99dc06bd8bb22e57bcb9acfbc86fc4fb,
        0x4ddcf4614cb9587b,
        Expect::Ok(0x0, 0x1926),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x5e1c,
        0x72d225973af39a0c3dfffffffffffff,
        0x78fbcd60a2156ea4,
        Expect::Ok(0x8000000000000400, 0x5e1c),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x405d,
        0x6655ece406ad57cf23be38c73398d7bf,
        0x41d88859dfb0aee2,
        Expect::Fault(0x407d),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xdb41,
        0xa32cd42c90721ffa7ff8000000000001,
        0x960fa0df5fabec47,
        Expect::Fault(0xdb41),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x31a8,
        0xb12babf3246cdc234014000000000000,
        0xf1f271629d14e017,
        Expect::Ok(0x5, 0x31a8),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x7264,
        0x8b596171ebdf329fc1e0000000200000,
        0x4c9ae2e9db9b99f6,
        Expect::Ok(0xffffffff7fffffff, 0x7264),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xe716,
        0xaed58500642e1894d07dfb776a5a1b5,
        0xafa1a202ea2c279c,
        Expect::Fault(0xe717),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x368a,
        0x9fee67d3a9a3d629400ec451c3121670,
        0x4494b7c2422f23fb,
        Expect::Ok(0x3, 0x36aa),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x6e2,
        0xf1a4c3acb8b24ed97ff8000000000000,
        0x153de3ea43425447,
        Expect::Ok(0x8000000000000000, 0x6e3),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x6788,
        0x8145693156272a27ac5936d9d444cd28,
        0x8ecbc3c81391a9c8,
        Expect::Fault(0x67a8),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x508c,
        0x15e18e835ae83e987fefa1de108156b5,
        0xa7328bfbf04cb497,
        Expect::Ok(0x8000000000000000, 0x508d),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x86ae,
        0xfe4b1ef262289545001fffffffffffff,
        0x78567348cc7041d4,
        Expect::Fault(0x86ae),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xa43a,
        0x4224fbb775df3382bff8000000000000,
        0x14ba0e2b98631dff,
        Expect::Fault(0xa43a),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xe0b3,
        0xd3e8f084d1067ac5bff8000000000000,
        0x8be2e0cca5b94e92,
        Expect::Fault(0xe0b3),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xd3ee,
        0x902a0c22f4cfe3454660ed54a66ca710,
        0x27562bce2908c430,
        Expect::Ok(0x8000000000000000, 0xd3ef),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xaf93,
        0x8c288c67e6446a277fe624fd36b814c1,
        0x9c0099e69d5247d8,
        Expect::Ok(0x8000000000000000, 0xaf93),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xe7d8,
        0xe3a68616e83959de800992c22cadc098,
        0x2fdbbb4312da3b62,
        Expect::Ok(0x0, 0xe7d8),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xe24f,
        0xa31ba2c56a1848297ff4000000000000,
        0x5aeb7c7ca8bed538,
        Expect::Fault(0xe24f),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x6f68,
        0x44ef1c703e349a50c3dfffffffffffff,
        0xfa1c0fe23f3788bc,
        Expect::Ok(0x8000000000000400, 0x6f68),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xece7,
        0x13dde7939f349b21bfe0000000000000,
        0xed0e906f8f27c6ad,
        Expect::Fault(0xece7),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xca7b,
        0xe04ee485d3eb5a387ff0000000000001,
        0xfdcd608adde62c44,
        Expect::Fault(0xca7b),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xfa7e,
        0xa6da866ce6c31b5c3ff8000000000000,
        0xe1fd615a2d87dd13,
        Expect::Ok(0x1, 0xfa7e),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xdd8a,
        0xa3c0b406bdceb156c32d57eff5994c99,
        0x94d558069a3444f2,
        Expect::Ok(0xfff15408053359b4, 0xddaa),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x6337,
        0x414b9d9e91592054a918245a651f3ec9,
        0x137f35fc72713cd9,
        Expect::Fault(0x6337),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x53b6,
        0x420e49216458ee643caf4286bc1ba776,
        0xb82a0d726ce4f7de,
        Expect::Ok(0x1, 0x53b6),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x485f,
        0xbedd9c0fe2cb2ef77ff7ffffffffffff,
        0x44aecfb38775d0ef,
        Expect::Fault(0x485f),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xba61,
        0x58f256f7b5b75646faf40b2c3ad6c1e3,
        0x421629f333cc7fac,
        Expect::Fault(0xba61),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xc615,
        0x4697721718802bfe0008000000000000,
        0x967228582ef20813,
        Expect::Fault(0xc635),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xee02,
        0xa24ff92b30a25ad000576b922bddbb8e,
        0x98dfb015df8396d,
        Expect::Fault(0xee22),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xfc41,
        0x891d2602f01b17903fb147ee173bdc63,
        0x65e57f51d437e496,
        Expect::Ok(0x0, 0xfc61),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x65af,
        0xc5277ed3b1ec02ab000a421322b83ac8,
        0xef8f0bd3714ef78e,
        Expect::Fault(0x65af),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xb22,
        0xc306be156277098800da6d8a2425687,
        0xd16ffc6454fef225,
        Expect::Fault(0xb22),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x76be,
        0x8378c6dcb40dcb343ca52063d5a450a0,
        0x40189ccbf6086b98,
        Expect::Ok(0x0, 0x76be),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xafe,
        0x2950fceaa2f3b5d6802aecaa62e088b4,
        0x9b643cdb1d447234,
        Expect::Fault(0xafe),
    ),
    (
        Op::Cvtsd2si,
        true,
        0xbb18,
        0xfb994b2c938028407ff8000000000001,
        0xb8b31d1640f508c1,
        Expect::Fault(0xbb19),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x9019,
        0x61a4226b1750014f4330000000000000,
        0x57fc12b63e5878b2,
        Expect::Ok(0x10000000000000, 0x9019),
    ),
    (
        Op::Cvtss2si,
        false,
        0x146f,
        0x12c76f20f14c75dd9f2074d77f7fffff,
        0x8e7be0a4d330a3a4,
        Expect::Fault(0x146f),
    ),
    (
        Op::Cvtss2si,
        false,
        0x4cda,
        0x49507b6260e5f2334ea5b067400cfb45,
        0xba8533c2631ab08c,
        Expect::Fault(0x4cfa),
    ),
    (
        Op::Cvtss2si,
        false,
        0x611e,
        0xbbc68cefd391a86d422f3627cb000001,
        0x34486b4f50d02331,
        Expect::Ok(0xff7fffff, 0x611e),
    ),
    (
        Op::Cvtss2si,
        false,
        0x61f9,
        0x5bff84792663038bda543a784701cd2b,
        0xaa651b7b72041357,
        Expect::Fault(0x61f9),
    ),
    (
        Op::Cvtss2si,
        false,
        0xbfc1,
        0x5ced62ee60e510ad20038ab028ea0c1e,
        0xfd75e5048544335b,
        Expect::Ok(0x0, 0xbfe1),
    ),
    (
        Op::Cvtss2si,
        false,
        0x1261,
        0x3c6831d8265760882261759d27baa4ef,
        0x6f898d81321ff1aa,
        Expect::Ok(0x0, 0x1261),
    ),
    (
        Op::Cvtss2si,
        false,
        0x9917,
        0x79dfd810df2a44ba182afb6674bd5b91,
        0x85b195445293fc49,
        Expect::Fault(0x9917),
    ),
    (
        Op::Cvtss2si,
        false,
        0x8cda,
        0xd5b0cbeb4cbf5336a20d3c801a62d815,
        0xf4b54cdecb7c21fb,
        Expect::Fault(0x8cfa),
    ),
    (
        Op::Cvtss2si,
        false,
        0xcd29,
        0xb8375602751580af30a9affdc44cb7c0,
        0xbd452888ff8baf9c,
        Expect::Fault(0xcd29),
    ),
    (
        Op::Cvtss2si,
        false,
        0x535b,
        0xdd6940ae15233a697d3f04243effffff,
        0x7b76e233724ab0dd,
        Expect::Ok(0x1, 0x537b),
    ),
    (
        Op::Cvtss2si,
        false,
        0xcf67,
        0x1155e3c9b7b7748c5a30cb383f7fffff,
        0xf59464f688b9e33b,
        Expect::Fault(0xcf67),
    ),
    (
        Op::Cvtss2si,
        false,
        0x1a00,
        0xcd2e5fa7b887b39f3402c601f49ce4a6,
        0x477f7452de5a4233,
        Expect::Fault(0x1a01),
    ),
    (
        Op::Cvtss2si,
        false,
        0x1780,
        0x1cf4f23f63c9aab20fab3a8f9057614a,
        0x9e4f2caf71add0ea,
        Expect::Ok(0x0, 0x17a0),
    ),
    (
        Op::Cvtss2si,
        false,
        0x2be1,
        0x62ee1e919a57a379e5e4f35abfc00000,
        0x52b7d4b64570f6cf,
        Expect::Fault(0x2be1),
    ),
    (
        Op::Cvtss2si,
        false,
        0x4150,
        0x30bed9ca539b893246f2bef380d28372,
        0xad7962da136c120c,
        Expect::Fault(0x4170),
    ),
    (
        Op::Cvtss2si,
        false,
        0xd2ab,
        0x73eb60d7ad1e1ec509f734e6cefffffe,
        0x658be35b61e99be2,
        Expect::Ok(0x80000100, 0xd2ab),
    ),
    (
        Op::Cvtss2si,
        false,
        0xd783,
        0x6ae66ac155446e0d5a25370bd60de76a,
        0x4bee027cfd14a354,
        Expect::Ok(0x80000000, 0xd783),
    ),
    (
        Op::Cvtss2si,
        false,
        0xb50b,
        0x32097d33cdade1bf2d46fa8f40400000,
        0xe7e982bf66fec332,
        Expect::Ok(0x3, 0xb50b),
    ),
    (
        Op::Cvtss2si,
        false,
        0xfcbb,
        0xc62e44e22f0981e24fbe5108813acb1a,
        0x264a7b3d3f316fb2,
        Expect::Ok(0x0, 0xfcbb),
    ),
    (
        Op::Cvtss2si,
        false,
        0xc9d3,
        0x65694800346f3d37cb78d64d0554e618,
        0x88f0748ae5823dc7,
        Expect::Fault(0xc9f3),
    ),
    (
        Op::Cvtss2si,
        false,
        0x58ec,
        0xf2735d578f01f9ab8e8aefa6ff800001,
        0xd4aa52966851c649,
        Expect::Ok(0x80000000, 0x58ed),
    ),
    (
        Op::Cvtss2si,
        false,
        0x530c,
        0xe407206171cadb9d003c798c931fd6b3,
        0x88fb9c1f0875a093,
        Expect::Ok(0x0, 0x532c),
    ),
    (
        Op::Cvtss2si,
        false,
        0xa09,
        0xa38a5a2ccc511bde3bde2c643d9f0aea,
        0xe9aab87348968bb3,
        Expect::Fault(0xa29),
    ),
    (
        Op::Cvtss2si,
        false,
        0xf834,
        0xce193df0117438c3dca5582c3f800001,
        0x2d251005622e71df,
        Expect::Ok(0x1, 0xf834),
    ),
    (
        Op::Cvtss2si,
        false,
        0x7aad,
        0x888e6bfd2c3e903c09856854d423612e,
        0xb1ff0a2f1d39225,
        Expect::Ok(0x80000000, 0x7aad),
    ),
    (
        Op::Cvtss2si,
        false,
        0xf541,
        0xa2efb894b0cba853ce83463f00400000,
        0x86d8e302561c9011,
        Expect::Ok(0x0, 0xf541),
    ),
    (
        Op::Cvtss2si,
        false,
        0x1b21,
        0xe6976bd0c82895a7a5ee856bb04b9e12,
        0xbac8800a41e5cf50,
        Expect::Ok(0x0, 0x1b21),
    ),
    (
        Op::Cvtss2si,
        false,
        0x6671,
        0x2b06e9336e9735063ec7acf1f7cce8e7,
        0x164a142084fb1d33,
        Expect::Fault(0x6671),
    ),
    (
        Op::Cvtss2si,
        false,
        0xaef2,
        0xe7f1ff494103767f3f34bba7313e0270,
        0x2c6e7c5bd9f37462,
        Expect::Fault(0xaef2),
    ),
    (
        Op::Cvtss2si,
        false,
        0xd7cc,
        0xe51cf7cec40c7eeeee1012b40009017d,
        0xd831df1ce2cee0cd,
        Expect::Ok(0x0, 0xd7cc),
    ),
    (
        Op::Cvtss2si,
        false,
        0x5afc,
        0xb1b6f54d5b2855142de9a1a4411d11c6,
        0x5ec55e9759ac4593,
        Expect::Ok(0xa, 0x5afc),
    ),
    (
        Op::Cvtss2si,
        false,
        0x93b8,
        0x1825ef8911d9198be3388263affe0cc4,
        0x2e972194bdb4f807,
        Expect::Ok(0x0, 0x93b8),
    ),
    (
        Op::Cvtss2si,
        false,
        0xa72a,
        0x32b63d45572870d98752f698200095a7,
        0xd8ed64127857e048,
        Expect::Fault(0xa72a),
    ),
    (
        Op::Cvtss2si,
        false,
        0x7928,
        0xe0191c30867a66c1d4ab080700400000,
        0x7c1b7cdfb26a11b3,
        Expect::Ok(0x0, 0x7928),
    ),
    (
        Op::Cvtss2si,
        false,
        0xf49e,
        0x8ee358351c2677de6540ef21dc8707b2,
        0x6d27c8b744ee882b,
        Expect::Ok(0x80000000, 0xf49f),
    ),
    (
        Op::Cvtss2si,
        false,
        0xd45b,
        0xa65ecf354c84d52f925884675c9eeb56,
        0xe06a2d26db6cdf53,
        Expect::Fault(0xd45b),
    ),
    (
        Op::Cvtss2si,
        false,
        0xa2bd,
        0xb9cc28ffe08e0e315fd2917e7fa00000,
        0x91c2ceea4df6f4bc,
        Expect::Ok(0x80000000, 0xa2bd),
    ),
    (
        Op::Cvtss2si,
        false,
        0x7f,
        0x6221ca2dd22bc0dfb66eb84a4b000000,
        0x6fbd202dc8403e4b,
        Expect::Ok(0x800000, 0x7f),
    ),
    (
        Op::Cvtss2si,
        false,
        0xe4cb,
        0x20fb9f98518fefbf08a210dabeffffff,
        0x1539ce1aa48fbad5,
        Expect::Fault(0xe4eb),
    ),
    (
        Op::Cvtss2si,
        false,
        0x9351,
        0x1f2083233655a35208738803d523bec1,
        0xdd8d3ab1689e8c9b,
        Expect::Fault(0x9351),
    ),
    (
        Op::Cvtss2si,
        false,
        0xaf70,
        0x142f2e5372e8039d22740a4cefffffe,
        0xbbdd481a3ac6506a,
        Expect::Ok(0x80000100, 0xaf70),
    ),
    (
        Op::Cvtss2si,
        false,
        0x9ce0,
        0xa3328e6c748c63c2d5ee20c4684800b,
        0x4f0cfae87712af76,
        Expect::Ok(0x4240, 0x9ce0),
    ),
    (
        Op::Cvtss2si,
        false,
        0x8477,
        0xb412e113cae128be94664a863f7fffff,
        0x7503d96b7a9461d6,
        Expect::Fault(0x8477),
    ),
    (
        Op::Cvtss2si,
        false,
        0xa3a,
        0xa56b1dea7c21afd41fd769ad8e0b358b,
        0x1013a2d0bc7217e9,
        Expect::Fault(0xa3a),
    ),
    (
        Op::Cvtss2si,
        false,
        0xc037,
        0x2b47ddac82e970a28031c1dcff88e07e,
        0xe671c359e46f509b,
        Expect::Fault(0xc037),
    ),
    (
        Op::Cvtss2si,
        true,
        0x29f3,
        0x47c1d3a19826edda931d355c0698e6ca,
        0x7d21cc4e815b85fb,
        Expect::Fault(0x29f3),
    ),
    (
        Op::Cvtss2si,
        true,
        0x5fc6,
        0xa8737b45d889bd020280a17e24f6b478,
        0xf88c468c55a740b4,
        Expect::Ok(0x1, 0x5fe6),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3b8c,
        0x249a2a86fa95d778fedb58f9ff92ab1c,
        0xc07e2d4532c3fda5,
        Expect::Ok(0x8000000000000000, 0x3b8d),
    ),
    (
        Op::Cvtss2si,
        true,
        0xcb80,
        0xd881ec0640b9380c2a837c17b0216788,
        0xcea844ee73d756ef,
        Expect::Fault(0xcba0),
    ),
    (
        Op::Cvtss2si,
        true,
        0x792,
        0x2d90e11d201f4aafb6a7a5d4f2fef72c,
        0x1ca1c48db5959380,
        Expect::Ok(0x8000000000000000, 0x793),
    ),
    (
        Op::Cvtss2si,
        true,
        0x68b3,
        0xe10735c90eaa1584899140bc35b1d5e6,
        0xfb386d330dacadbe,
        Expect::Fault(0x68b3),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3780,
        0x9a862f540a792834d4ddfa145f000000,
        0x763b2b22df0c65bd,
        Expect::Ok(0x8000000000000000, 0x3781),
    ),
    (
        Op::Cvtss2si,
        true,
        0xfaa5,
        0xae828369d7433eca8e410d444effffff,
        0x260a7910f44fac4d,
        Expect::Ok(0x7fffff80, 0xfaa5),
    ),
    (
        Op::Cvtss2si,
        true,
        0xb486,
        0x749b1e7143554e88cd032cbf0dc33a9e,
        0xe0b9d7dad315e601,
        Expect::Ok(0x0, 0xb4a6),
    ),
    (
        Op::Cvtss2si,
        true,
        0x6c2a,
        0xd8c98cbeb5e85c3867f0d5917effffff,
        0x7439f64a39f8d597,
        Expect::Fault(0x6c2b),
    ),
    (
        Op::Cvtss2si,
        true,
        0xf94a,
        0xd124762894442b7d608f5310ffc12345,
        0x6f98c212b8d4bd3e,
        Expect::Fault(0xf94b),
    ),
    (
        Op::Cvtss2si,
        true,
        0x88dc,
        0xe8d597e1ed2f81013b580a25cb000001,
        0x502d8edab3657a8e,
        Expect::Ok(0xffffffffff7fffff, 0x88dc),
    ),
    (
        Op::Cvtss2si,
        true,
        0x2844,
        0x46e33b0f698025120d200c393f7fffff,
        0xf0e04e39d440110d,
        Expect::Fault(0x2864),
    ),
    (
        Op::Cvtss2si,
        true,
        0xca62,
        0xfa0b065e2a97b0eb51ff88f134000000,
        0xa08df67f2e7a97b3,
        Expect::Fault(0xca62),
    ),
    (
        Op::Cvtss2si,
        true,
        0x749f,
        0x7ccdcca4a91adefc74ec9bba40000000,
        0x20c6c7bc583bde2c,
        Expect::Ok(0x2, 0x749f),
    ),
    (
        Op::Cvtss2si,
        true,
        0x393d,
        0x88c157cbb4b8cd07d9ff8f15003d5602,
        0x17048da3c5857604,
        Expect::Ok(0x0, 0x393d),
    ),
    (
        Op::Cvtss2si,
        true,
        0x2196,
        0xe78216d467d53329bed5f0a9beffffff,
        0x3c82d1651370c70c,
        Expect::Fault(0x21b6),
    ),
    (
        Op::Cvtss2si,
        true,
        0xa5f6,
        0xfd76e9a9fd5ad053cf1e60ca401f8f9f,
        0x2708bd0042f98138,
        Expect::Fault(0xa5f6),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3a97,
        0x82f1c0706eedbb73e6d1b3db00000000,
        0xf424f475df21f303,
        Expect::Ok(0x0, 0x3a97),
    ),
    (
        Op::Cvtss2si,
        true,
        0x86b5,
        0xb472ea1ffabe088e55f460669c2c4a3b,
        0xa0ffae50cd86a86d,
        Expect::Fault(0x86b5),
    ),
    (
        Op::Cvtss2si,
        true,
        0xd438,
        0x565c9abe7682f67eb146ea057fc00001,
        0x78929fae64192184,
        Expect::Fault(0xd439),
    ),
    (
        Op::Cvtss2si,
        true,
        0x47dc,
        0xd4e750e1c3bc923efa1ffc23ff800001,
        0xd87129bb7318cbff,
        Expect::Ok(0x8000000000000000, 0x47dd),
    ),
    (
        Op::Cvtss2si,
        true,
        0xc1ee,
        0x5237605fea4061c534ee256e00000001,
        0xc3c860d89831adf2,
        Expect::Ok(0x0, 0xc1ee),
    ),
    (
        Op::Cvtss2si,
        true,
        0x12b4,
        0xbaafc05d19ad63288eaec986003da2c9,
        0x576358dad5ae80c5,
        Expect::Ok(0x0, 0x12b4),
    ),
    (
        Op::Cvtss2si,
        true,
        0x95c3,
        0xd17e128623866d49d9b4606c8790c49e,
        0xe46a3e6a403f59a2,
        Expect::Ok(0x0, 0x95e3),
    ),
    (
        Op::Cvtss2si,
        true,
        0xacd3,
        0x22699d6ef8911013a92718b7ffa08fa,
        0xdb99275df4155fff,
        Expect::Ok(0x8000000000000000, 0xacd3),
    ),
    (
        Op::Cvtss2si,
        true,
        0x40c9,
        0x181b7110f2ec1bda4b896dd9d58f3fcc,
        0x1e52df3c487948f9,
        Expect::Ok(0xffffee1806800000, 0x40c9),
    ),
    (
        Op::Cvtss2si,
        true,
        0xf4c9,
        0xe9dd9e9d1c8f62d7deef149adf000001,
        0x34b620864349548c,
        Expect::Ok(0x8000000000000000, 0xf4c9),
    ),
    (
        Op::Cvtss2si,
        true,
        0x4455,
        0x4793a0e60cddf979b7cea05173ca0f3f,
        0xa1e7516501878736,
        Expect::Fault(0x4455),
    ),
    (
        Op::Cvtss2si,
        true,
        0x501e,
        0x2024ea1dfd7e9560927be6766d0b60ed,
        0x93f966ff777ac925,
        Expect::Fault(0x501f),
    ),
    (
        Op::Cvtss2si,
        true,
        0x2172,
        0x827e1cd993c6167dc7f1d5a74f8cba03,
        0x433118121db569df,
        Expect::Ok(0x119740600, 0x2172),
    ),
    (
        Op::Cvtss2si,
        true,
        0xe428,
        0x28b5dbf528dc193c91e7ef15ac01db82,
        0xba87b270bd9bbccc,
        Expect::Fault(0xe428),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3df3,
        0x31df78265043f2bb8fd7019ef364aa68,
        0x7547d6253314fa2e,
        Expect::Ok(0x8000000000000000, 0x3df3),
    ),
    (
        Op::Cvtss2si,
        true,
        0x799e,
        0x9b44a244f9ea3044a5fb102f0b550119,
        0x1a0a23728a2b2e78,
        Expect::Ok(0x0, 0x79be),
    ),
    (
        Op::Cvtss2si,
        true,
        0x5e80,
        0x17c15df9ea96ca823904bc6a40400000,
        0xe635ecba6169b3df,
        Expect::Ok(0x3, 0x5e80),
    ),
    (
        Op::Cvtss2si,
        true,
        0xe39d,
        0x3de8e07bc196c498f78d6ac1008658c4,
        0x27d1fe2867c81a4,
        Expect::Fault(0xe3bd),
    ),
    (
        Op::Cvtss2si,
        true,
        0x3b42,
        0xbc28bb7b47f0fea35e04e81d224d9742,
        0xd745d7ad0cdc749a,
        Expect::Ok(0x0, 0x3b62),
    ),
    (
        Op::Cvtss2si,
        true,
        0x8a59,
        0xd334bd5083b69d28b8d00bd0012a3d63,
        0x8a802a7c9b949b2d,
        Expect::Fault(0x8a79),
    ),
    (
        Op::Cvtss2si,
        true,
        0xcada,
        0x3b6860bda8b6b61df46eed38bbf96fae,
        0x4d49ceeae1e09a6f,
        Expect::Fault(0xcafa),
    ),
    (
        Op::Cvtss2si,
        true,
        0xb6a2,
        0xe187191004ae6b03ed709d35598808a4,
        0x6dc704cdc1599923,
        Expect::Ok(0x11011480000000, 0xb6a2),
    ),
    (
        Op::Cvtss2si,
        true,
        0x750,
        0xafc8977d62c8264e2d5769c1bfc00000,
        0xa7d926b335a319df,
        Expect::Fault(0x770),
    ),
    (
        Op::Cvtss2si,
        true,
        0xa17e,
        0x3b4b671d81ce985e1023f6fc800c0b36,
        0x49ab07eafcc4240b,
        Expect::Ok(0x0, 0xa17e),
    ),
    (
        Op::Cvtss2si,
        true,
        0x2103,
        0x5e21eef6295dd278192e96317e800000,
        0x7590e3bf07948346,
        Expect::Fault(0x2103),
    ),
    (
        Op::Cvtss2si,
        true,
        0xa324,
        0x8d18e440302523f12a528a4e807fffff,
        0x86fef80431ca4d31,
        Expect::Fault(0xa324),
    ),
    (
        Op::Cvtss2si,
        true,
        0x4694,
        0x5054c3f4f82123b7ed9ba6d840400000,
        0xdeceda7b173578c8,
        Expect::Ok(0x3, 0x4694),
    ),
    (
        Op::Cvtss2si,
        true,
        0x1749,
        0xc4da3899a83a95dcb5d404bf808b7927,
        0x16316c61900d2b55,
        Expect::Ok(0x0, 0x1769),
    ),
    (
        Op::Cvtss2si,
        true,
        0xbf56,
        0xef1345ed525383bb65b17d4cf4cb5c5d,
        0x5b4fa3afac4bc4b1,
        Expect::Fault(0xbf57),
    ),
    (
        Op::Cvtss2si,
        true,
        0x2a89,
        0xa07ce5cbc48e38f1ad087f5d74d94852,
        0x79ea82a292467539,
        Expect::Ok(0x8000000000000000, 0x2a89),
    ),
    (
        Op::Cvtss2si,
        true,
        0xf910,
        0xdbff83c58377bea8eda7bfed8b76612f,
        0x557139ed667994d5,
        Expect::Ok(0x0, 0xf930),
    ),
    (
        Op::Cvtss2si,
        true,
        0x9ad2,
        0x7f5bf42be772be6019daf03c3fc00000,
        0xfc6d6da0f4b3c54e,
        Expect::Ok(0x2, 0x9af2),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xaa53,
        0x1cd37c5c61a2a93e0008000000000000,
        0x95eaca2288d7a9b6,
        Expect::Ok(0x0, 0xaa53),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xd61f,
        0xf774ad97b03fdc14002a7efba922dea5,
        0x47e5827aba6885a8,
        Expect::Ok(0x0, 0xd63f),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xfc27,
        0x3aea61ae1561b074330000000000001,
        0x12c39626baecb8c8,
        Expect::Fault(0xfc27),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xbab6,
        0xb9ab624d8b2606791eb74d2eb952332a,
        0x7e6ed22271315061,
        Expect::Ok(0x0, 0xbab6),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x37c8,
        0xb6074d19bb70777bc1e0000000100000,
        0x5ed33c1358a6873e,
        Expect::Ok(0x80000000, 0x37e8),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xc09c,
        0x8d7ec9980cdafe8341dfffffffe00000,
        0xc0df2dd11578c423,
        Expect::Fault(0xc0bc),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x102d,
        0xd042620b4a3eab59bff0000000000000,
        0xd3ab9c29d925545b,
        Expect::Ok(0xffffffff, 0x102d),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x1dc0,
        0xf26e13ea84134ae1896f268203bbcbfb,
        0xa679376cb3b1c330,
        Expect::Ok(0x0, 0x1de0),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x2db9,
        0x312106319bdb25e7c084f908981332d3,
        0x5f38b0979a892469,
        Expect::Fault(0x2db9),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x5d80,
        0xe3748654b21411703ca62632450567f8,
        0x8026f7cc1f184fd7,
        Expect::Ok(0x0, 0x5da0),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x5883,
        0x70a4887cee55f97eb278ca656e9838b1,
        0x6222036067d59844,
        Expect::Ok(0x0, 0x58a3),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x3558,
        0xb4b4638693005e0e63501c7bc50f5f38,
        0x653d8999f6d0c9bf,
        Expect::Fault(0x3559),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xe0a1,
        0x3fbc021a4df63333cf39f12b4e305174,
        0xecbcffc8242c4b30,
        Expect::Ok(0x80000000, 0xe0a1),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x9b33,
        0x4d9953f40fd99c3cfff8000000000000,
        0x3fe06694a3753c05,
        Expect::Fault(0x9b33),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x307b,
        0x8fd089705d4aea683ca0000000000000,
        0x3ab008cd2ce62a16,
        Expect::Ok(0x0, 0x307b),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x862a,
        0xc91e785ef16c5cd77fffffffffffffff,
        0x28e83f71c8351c0c,
        Expect::Fault(0x862b),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x52e6,
        0xd94a7b12b58670b37ff8000000000001,
        0x58590d0ea1bdef6c,
        Expect::Ok(0x80000000, 0x52e7),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x3896,
        0xd4836488ed2d8b35ffefffffffffffff,
        0x1b993612e6a47f,
        Expect::Ok(0x80000000, 0x3897),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x4b9c,
        0xfe2f2f2345d917a37b67af364990f6bc,
        0x40e624eadc5c5527,
        Expect::Ok(0x80000000, 0x4b9d),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x240,
        0xb778a3b3d9c5aac5bff6f2187eec9fab,
        0xe5077f6103c83835,
        Expect::Fault(0x260),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x6c08,
        0x7ea57027638f52cd1c865a00af4287c6,
        0x2329efa49d43cd45,
        Expect::Fault(0x6c28),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xd71e,
        0x600c389ce6cd2a43433fffffffffffff,
        0xad05fd7727536e5c,
        Expect::Fault(0xd71f),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x35f4,
        0x6c9934b26075197f3ec4cec2fb15f79a,
        0x3ffba413a4d4e79c,
        Expect::Ok(0x0, 0x35f4),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x6be1,
        0x516b4fd0430bead8010d7491f7bb653,
        0x5c6054a2a90e3376,
        Expect::Fault(0x6be1),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x7bb1,
        0x2586e408cceb34c53ffe45a46500ea72,
        0xeaa0eaee98054a2,
        Expect::Ok(0x1, 0x7bb1),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x564c,
        0xca4890b9f332ae6fd969ed3b2e6a2772,
        0x8ea995d372cbb807,
        Expect::Fault(0x564d),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x1a00,
        0xbbba2535d958e77f4330000000000001,
        0x5868ba993cd4ad43,
        Expect::Fault(0x1a01),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x8917,
        0xbf9fb10c7ebeb4d641dfffffffe00000,
        0x44909a0f40e36616,
        Expect::Fault(0x8937),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x9158,
        0x64f404f397298268c1e0000000200000,
        0x7e5decff02a4c8cf,
        Expect::Fault(0x9159),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xc00a,
        0xd384ed605473134243d0000000000001,
        0x23d70ba3102744cc,
        Expect::Fault(0xc00b),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x6d96,
        0x2fa459010ceb2b23000fffffffffffff,
        0x8d3351c4bbac1a5d,
        Expect::Fault(0x6db6),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xa31f,
        0xc9d32895bae4dd51802ee9fb770f6094,
        0xa4434f08201e2251,
        Expect::Fault(0xa33f),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x4c8,
        0xdb492f7158fd21b88029bf733c70df7b,
        0xa0db18a49702563c,
        Expect::Fault(0x4e8),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x7f00,
        0x3ed1e899edd893d3fff0000000000001,
        0x6bc833ed91e20ff2,
        Expect::Fault(0x7f01),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xcc1e,
        0xb6fe4c105100b604800d95154c1b4153,
        0xe484a02401a198aa,
        Expect::Fault(0xcc3e),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xde0e,
        0xb5c9f4572f3777e64014000000000000,
        0xcb690fefb251bb20,
        Expect::Ok(0x5, 0xde0e),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xa279,
        0x718f445d8d961a2b3ca9e234fae205a6,
        0x793c89ba4e58bb8d,
        Expect::Fault(0xa279),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xb98c,
        0xed2e224756ba8eec3ca0000000000000,
        0x20ccf8571e9fa75d,
        Expect::Ok(0x0, 0xb9ac),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x9d75,
        0x171f11817e910b8241dfffffffc00000,
        0x32468a447ae92581,
        Expect::Ok(0x7fffffff, 0x9d75),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x2b5d,
        0x7f9458988f2f83001fffffffffffff,
        0x1eefd610b5c4ad1f,
        Expect::Fault(0x2b7d),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x3780,
        0xb6e6039c263b87613fefffffffffffff,
        0xb2dae538acb979c0,
        Expect::Ok(0x0, 0x37a0),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xe472,
        0x95467702eb6719c0f8e52bb732592353,
        0x84a891a388baab9d,
        Expect::Fault(0xe473),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xa180,
        0xcce5c5cdac55c321b089a91fae10d6ce,
        0xc6e81c68bed2a9a8,
        Expect::Fault(0xa1a0),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x3722,
        0x81bcddd5fd6cf40e2d409bf43307d852,
        0xff0ee7a237e8d5fb,
        Expect::Ok(0x0, 0x3722),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x2b3b,
        0xd4576b1ce76820037ff7ffffffffffff,
        0xde7905497ed521fb,
        Expect::Fault(0x2b3b),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x6afc,
        0x3b36606c7cab49da00033a84ee97a702,
        0xe5b2f752b3d68990,
        Expect::Ok(0x0, 0x6afc),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x71b0,
        0xee4d86220812e3ebc1e0000000100000,
        0x34c9e32447a119e4,
        Expect::Ok(0x80000000, 0x71b0),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x2d3e,
        0xe47c1cbac5ad032b41dfffffffe00000,
        0x65dbec15faf45af4,
        Expect::Fault(0x2d3e),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x9bd8,
        0xe847ef859a42c4fc000c90063cbda1b9,
        0x7a370574d2dffa3f,
        Expect::Ok(0x0, 0x9bd8),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x540c,
        0x4344647f0bd37df7ff0000000000000,
        0x55c6885656654279,
        Expect::Fault(0x540d),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x8749,
        0xd32c8726d44eb2560008000000000000,
        0xfab74e8e26d7daf0,
        Expect::Ok(0x0, 0x8749),
    ),
    (
        Op::Cvttsd2si,
        false,
        0xa589,
        0x726727d7bb85fdb14e40ac2aa252777d,
        0x55afa6553e5758a0,
        Expect::Ok(0x80000000, 0xa589),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x68b1,
        0xc23faa1a81d4ad138213933b52f9b883,
        0xf94cd09f0d16ae7c,
        Expect::Fault(0x68b1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xbfbb,
        0x5eb52a9788884c4dd3f009aa300b4a20,
        0x7c2046488ac83a66,
        Expect::Ok(0x8000000000000000, 0xbfbb),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xe771,
        0xd46fa678e9dfaa838226bccf31a07560,
        0xa7deb7251852bbb4,
        Expect::Fault(0xe771),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x1f09,
        0xdd3ff09038aa860c6046c352a5351351,
        0x55153ed59bd2bb0e,
        Expect::Fault(0x1f09),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x4611,
        0xde58a4a67389167bffd7c6e2ed64fac,
        0xfbba80731441b612,
        Expect::Fault(0x4631),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x4928,
        0x3186c43a7d365e06c1e0000000200000,
        0xf8f1ee3a2d229290,
        Expect::Ok(0xffffffff7fffffff, 0x4928),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6a7a,
        0xb0288adae4f77dc5fff8123456789abc,
        0x64337e5c6ae5a845,
        Expect::Fault(0x6a7b),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x233,
        0x6544d2310d2bf3000000000000000003,
        0x369606882db38650,
        Expect::Fault(0x233),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x56e7,
        0x262779611289c1f5433fffffffffffff,
        0xa599cd21f6b641f0,
        Expect::Ok(0x1fffffffffffff, 0x56e7),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6e81,
        0x3563b714294012033ff8000000000000,
        0xc9a375cf21d11b13,
        Expect::Fault(0x6ea1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xfd2a,
        0xfd69654c23c54b07ff4000000000000,
        0xcdef4b7a56cfdd8e,
        Expect::Fault(0xfd2b),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xdc6d,
        0x11114515c411d7fe4008000000000000,
        0x278334ea64d8b0de,
        Expect::Ok(0x3, 0xdc6d),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6f80,
        0x968ce1589a9ad626000fffffffffffff,
        0x90276080a925171c,
        Expect::Fault(0x6fa0),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xba01,
        0x6c3fc5732e87ee8e011ba165403cac04,
        0xed3cb1de83547e6c,
        Expect::Ok(0x0, 0xba21),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xdb64,
        0x1b7807bb665ce1ad43e0000000000001,
        0x36b7358acb68740f,
        Expect::Fault(0xdb65),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x932a,
        0x83cdb6db3bff03697fffffffffffffff,
        0x920ca56420c10ce3,
        Expect::Fault(0x932b),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x5c9d,
        0xfa50f9ad7c5133659b827d63c65d3104,
        0xdf8fc7868de4c37a,
        Expect::Ok(0x0, 0x5cbd),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x4e1d,
        0x5b79babf5b869c7c7fe749358c220cb3,
        0xfd4455cd30a35646,
        Expect::Fault(0x4e1d),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xb56e,
        0x7c88a22565b9d0048010000000000000,
        0xce99c474d8420abb,
        Expect::Ok(0x0, 0xb56e),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x65b2,
        0x1aafe92ec291c2416f401871a17609f8,
        0x33c378ec8dabab58,
        Expect::Ok(0x8000000000000000, 0x65b3),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xe664,
        0xbbafcac3b48a5b4d2cede7aead22c35b,
        0x3a33468f272cd926,
        Expect::Fault(0xe664),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xac76,
        0x2c730b96fa0d781a0000000000000003,
        0x1481ca4a455d9591,
        Expect::Ok(0x0, 0xac76),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xc2cb,
        0x77df9a5b5ed754df7ff7ffffffffffff,
        0x73824e9aecf24978,
        Expect::Ok(0x8000000000000000, 0xc2cb),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x3157,
        0xff1618956b8e2968fff89b74322187e2,
        0x7ef358946e969380,
        Expect::Fault(0x3157),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x5faa,
        0x9f128edaf2bff88aab269de704bc4a5d,
        0x6a5d61933d122883,
        Expect::Ok(0x0, 0x5faa),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x273a,
        0xa2dc2002600f0dd3fd5ed2b8c80e3f73,
        0xc8850826f6525867,
        Expect::Fault(0x273b),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x8d73,
        0x361ad2ebb774b3d943e0000000000001,
        0x12b98e0110658b86,
        Expect::Fault(0x8d73),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x4536,
        0x7cced189ec668433ff8000000000000,
        0xcb14d6bc91f741e8,
        Expect::Fault(0x4536),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xe63e,
        0x9318918119ff9da78d556d13041026e4,
        0x246d9d3878f70c55,
        Expect::Fault(0xe63e),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xfdf7,
        0x5e637e1124e52d39000fffffffffffff,
        0x6b6fccac461c64df,
        Expect::Ok(0x0, 0xfdf7),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x5c37,
        0x5e2c1a80478a2a290c8a0e03bf896b0b,
        0xe17f06c6567e88fb,
        Expect::Ok(0x0, 0x5c37),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x3cc7,
        0x4ca81e3bb4874a8ea26fe7753da486e7,
        0xc28d87ad63ecd7e4,
        Expect::Ok(0x0, 0x3ce7),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6624,
        0x43299c8ddf081074fff0000000000000,
        0x7dd21f8477efc3db,
        Expect::Fault(0x6625),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6b9c,
        0xae42cff046e978457ff0000000000000,
        0x91f29ffd2fe25bb5,
        Expect::Ok(0x8000000000000000, 0x6b9d),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xa116,
        0x32aa61a233b131de4005bb027c5f46ad,
        0x8a96e5099ff03c48,
        Expect::Fault(0xa136),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xd834,
        0x5666092c1be01281801f5edd3870b237,
        0x1c1658493156a76c,
        Expect::Ok(0x0, 0xd834),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xfbb1,
        0x5692d4a73809144a400d1688425eca2f,
        0xaf142c2f3e8bb8d1,
        Expect::Ok(0x3, 0xfbb1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x5ce1,
        0xcffeab03d1a4f1da0000000000000001,
        0xceba46e8e18e31f0,
        Expect::Ok(0x0, 0x5ce1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x6254,
        0xfe2b224df09bd9128010000000000000,
        0x2e1d9e7e03a45ba5,
        Expect::Fault(0x6274),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x17d2,
        0x9d2bbc4aea1903ffc1e0000000200000,
        0x8abf030a4d0a3209,
        Expect::Ok(0xffffffff7fffffff, 0x17d2),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x8382,
        0x4f2e1a604d78686d001fffffffffffff,
        0xd57eb961c9e21dea,
        Expect::Fault(0x83a2),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xd072,
        0x6f202d756d6320180000000000000001,
        0x2729c1cad0094eac,
        Expect::Ok(0x0, 0xd072),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xe9e1,
        0xf02dd32f036f6ae4c000000000000000,
        0x4694638cc9eac41d,
        Expect::Ok(0xfffffffffffffffe, 0xe9e1),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x7f40,
        0xcf3491de68d118ffc3e0000000000001,
        0xa10073e9661fcadd,
        Expect::Fault(0x7f41),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xfbf,
        0x7af06df759407519da2d0f3dcfa590e1,
        0x538cf846cf8e1944,
        Expect::Ok(0x8000000000000000, 0xfbf),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x589f,
        0x5659bf9fefff319d7fe0000000000000,
        0xdbae99412600ddd8,
        Expect::Ok(0x8000000000000000, 0x589f),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x935,
        0x43b33597ba470e930010000000000000,
        0xfd6ae14b4e0b03fc,
        Expect::Fault(0x935),
    ),
    (
        Op::Cvttsd2si,
        true,
        0xd116,
        0x9539ae7468c4dd2c8010000000000000,
        0xe962d5d04dcf91ee,
        Expect::Ok(0x0, 0xd136),
    ),
    (
        Op::Cvttss2si,
        false,
        0x75c9,
        0xcc989ee854395da55d8ae01740000000,
        0xfa386267cb0927ce,
        Expect::Ok(0x2, 0x75c9),
    ),
    (
        Op::Cvttss2si,
        false,
        0x547d,
        0x9c0e54122a9c7609516ad6aeff800001,
        0x1d63d7bd8d4563a8,
        Expect::Fault(0x547d),
    ),
    (
        Op::Cvttss2si,
        false,
        0x4c7f,
        0xccc03c85d5764650e39fff339e578fad,
        0x363d2c8bf3031db8,
        Expect::Fault(0x4c7f),
    ),
    (
        Op::Cvttss2si,
        false,
        0x3f58,
        0x1ed0fca2a41d156611f7458480000001,
        0x1e5c4161fa788cc0,
        Expect::Ok(0x0, 0x3f58),
    ),
    (
        Op::Cvttss2si,
        false,
        0x7632,
        0xf099e7cb89586c53fb1ef5a6566a71e7,
        0x7cf6a0778603e17a,
        Expect::Fault(0x7633),
    ),
    (
        Op::Cvttss2si,
        false,
        0x4b9f,
        0x276e83138ee46fc1ca24128a33800000,
        0x8ba56320793cab1b,
        Expect::Fault(0x4bbf),
    ),
    (
        Op::Cvttss2si,
        false,
        0xa076,
        0xecb3b563ffaf351646061af40b7a15ac,
        0x4bf199b3b0bcaf6,
        Expect::Fault(0xa076),
    ),
    (
        Op::Cvttss2si,
        false,
        0xcf52,
        0x95e8d1a64bbf74772e5cfaf8bf000000,
        0xe723cb66fa542b47,
        Expect::Fault(0xcf72),
    ),
    (
        Op::Cvttss2si,
        false,
        0xe4f2,
        0xa69adf39da33543a035b2e207eafdead,
        0x998af60d242a8719,
        Expect::Ok(0x80000000, 0xe4f3),
    ),
    (
        Op::Cvttss2si,
        false,
        0x804,
        0x972fb55c4e2b0ebfd7b688deff8edf61,
        0xbcc15b2280f34ec0,
        Expect::Fault(0x805),
    ),
    (
        Op::Cvttss2si,
        false,
        0xb94b,
        0x7fcfcae74a10548f6a86d0773f000000,
        0x782a1ba93a551e21,
        Expect::Ok(0x0, 0xb96b),
    ),
    (
        Op::Cvttss2si,
        false,
        0xfadf,
        0x8ce46abefa44beba0eeee7a2f48fd2b7,
        0x1fcbac8067052465,
        Expect::Ok(0x80000000, 0xfadf),
    ),
    (
        Op::Cvttss2si,
        false,
        0x9a6b,
        0x59e71f02d08475d0610e94cf3f000000,
        0xb20d53c6c0d4c0e5,
        Expect::Ok(0x0, 0x9a6b),
    ),
    (
        Op::Cvttss2si,
        false,
        0x8265,
        0x5ea92ce486d1c146a7d86656b26f6188,
        0x2bc7a88d9749acd9,
        Expect::Fault(0x8265),
    ),
    (
        Op::Cvttss2si,
        false,
        0x5d3,
        0x1409834e035b1714f1d3292cf47090a,
        0x48f1c03d5bfe429,
        Expect::Ok(0x80000000, 0x5d3),
    ),
    (
        Op::Cvttss2si,
        false,
        0x457a,
        0xf910978d668c401708922c0eb2cf73b4,
        0x34bdfa0a10a083f6,
        Expect::Fault(0x457a),
    ),
    (
        Op::Cvttss2si,
        false,
        0x4a82,
        0xa01140c6913cc9a7e2c2a44b40000000,
        0xbf0c86e0ecf47627,
        Expect::Ok(0x2, 0x4a82),
    ),
    (
        Op::Cvttss2si,
        false,
        0x36c1,
        0xab2063b375ec5fbb744a13e67181ca32,
        0x4a54f4325e8a3b39,
        Expect::Ok(0x80000000, 0x36c1),
    ),
    (
        Op::Cvttss2si,
        false,
        0x2a26,
        0xc0e36dc129a686ac2d4a6ca8fff00ecb,
        0x6f2bd22623828e9b,
        Expect::Fault(0x2a27),
    ),
    (
        Op::Cvttss2si,
        false,
        0xef5d,
        0x11de3850fb711bc66ef3981cbf9a8ea6,
        0xee47648a9c533e5f,
        Expect::Fault(0xef7d),
    ),
    (
        Op::Cvttss2si,
        false,
        0x4167,
        0x801911ad8633ac0f767acb7ae331a9e0,
        0xde0232c816e878bc,
        Expect::Fault(0x4167),
    ),
    (
        Op::Cvttss2si,
        false,
        0xb4d3,
        0x34951fa434b2db34ba3e3a7fc00001,
        0x7697426aed05c919,
        Expect::Ok(0x80000000, 0xb4d3),
    ),
    (
        Op::Cvttss2si,
        false,
        0x9da2,
        0xa5de453e3210eae99466eee37e800000,
        0x9dc2b564c9008b3,
        Expect::Ok(0x80000000, 0x9da3),
    ),
    (
        Op::Cvttss2si,
        false,
        0x1ec0,
        0x51f2787345c0c84a67ae4aa4190166f0,
        0x84a22161ec4a0533,
        Expect::Ok(0x0, 0x1ee0),
    ),
    (
        Op::Cvttss2si,
        false,
        0xfc03,
        0x8a314d39e20e1233d86352cc7f800001,
        0x411781f05f9f1e15,
        Expect::Fault(0xfc03),
    ),
    (
        Op::Cvttss2si,
        false,
        0xff1a,
        0x96e7c7f46e2d07e18997fc3bff5a0a85,
        0x44501126c636b39b,
        Expect::Fault(0xff1b),
    ),
    (
        Op::Cvttss2si,
        false,
        0xb6f2,
        0x6cba1d5438f4666de73b7e66ff800001,
        0x804fbc570aae0601,
        Expect::Ok(0x80000000, 0xb6f3),
    ),
    (
        Op::Cvttss2si,
        false,
        0x2862,
        0xc1fd327b09c51cdad46b2b94cf9b8f34,
        0xf79592cd50802301,
        Expect::Fault(0x2863),
    ),
    (
        Op::Cvttss2si,
        false,
        0x9142,
        0xe2931c69621ea318391c7fe80000000,
        0x91c075052323f9aa,
        Expect::Ok(0x0, 0x9142),
    ),
    (
        Op::Cvttss2si,
        false,
        0x478e,
        0x34cc2fd537b3766b765c04bc2a700f53,
        0x21bfb57870587422,
        Expect::Fault(0x47ae),
    ),
    (
        Op::Cvttss2si,
        false,
        0x951d,
        0x6a73026fb7f3bae29510df8a33800000,
        0xde21ccaff39b6e1b,
        Expect::Ok(0x0, 0x953d),
    ),
    (
        Op::Cvttss2si,
        false,
        0xe84d,
        0x46e1505c6c880a8c3db3efbb3160247c,
        0x4b061c160c55a909,
        Expect::Fault(0xe86d),
    ),
    (
        Op::Cvttss2si,
        false,
        0x8000,
        0x98b8eb667a9e5eb9cec4866bfb0c4f17,
        0xcbfb1771dc71593c,
        Expect::Fault(0x8001),
    ),
    (
        Op::Cvttss2si,
        false,
        0xe377,
        0x8a2e0997754a30ef7c9d88304189589f,
        0x41f942105473f56a,
        Expect::Fault(0xe377),
    ),
    (
        Op::Cvttss2si,
        false,
        0x6e4d,
        0xe8237a17a1696680c1935964ffc00000,
        0xa08933313091eb15,
        Expect::Fault(0x6e4d),
    ),
    (
        Op::Cvttss2si,
        false,
        0xec9,
        0x5c91aad04f1db1c7c3e267117f800001,
        0x5663b0f02bbad108,
        Expect::Ok(0x80000000, 0xec9),
    ),
    (
        Op::Cvttss2si,
        false,
        0x593,
        0x693334d8d613e7da286b137fef9b91aa,
        0x3c1f0b7cbee5ed9d,
        Expect::Ok(0x80000000, 0x593),
    ),
    (
        Op::Cvttss2si,
        true,
        0xef80,
        0x45b674e5f0662931aaa728df807fffff,
        0x1b9e60ba9b5020a9,
        Expect::Fault(0xefa0),
    ),
    (
        Op::Cvttss2si,
        true,
        0xdc8a,
        0x764f8bd97ed80896b051c5623f7fffff,
        0x3252d366630dc250,
        Expect::Ok(0x0, 0xdcaa),
    ),
    (
        Op::Cvttss2si,
        true,
        0x209e,
        0x86147d9990f74b29a236dc3f007fffff,
        0x58ce66d5f0e438bf,
        Expect::Fault(0x20be),
    ),
    (
        Op::Cvttss2si,
        true,
        0xc372,
        0xbfbfacd3ecb01cca5e36ddfbbf000000,
        0x1c57906f43ea5bfe,
        Expect::Fault(0xc372),
    ),
    (
        Op::Cvttss2si,
        true,
        0x61c9,
        0x91e3c566b7cc8e7665bbe5039362b623,
        0x43d9e7d662afa3c0,
        Expect::Fault(0x61e9),
    ),
    (
        Op::Cvttss2si,
        true,
        0xa6e,
        0xb5cd62edd39a48ecc19fcbb34b800001,
        0x51383789433756a9,
        Expect::Ok(0x1000002, 0xa6e),
    ),
    (
        Op::Cvttss2si,
        true,
        0xcc72,
        0xa16896fe27fcf5ce955ed584beffffff,
        0xfeba33348553a004,
        Expect::Fault(0xcc72),
    ),
    (
        Op::Cvttss2si,
        true,
        0xc6d5,
        0xc4ccc4d9f2be1c529110523affc12345,
        0x1c61303c061f7e3a,
        Expect::Ok(0x8000000000000000, 0xc6d5),
    ),
    (
        Op::Cvttss2si,
        true,
        0xbec1,
        0xa795a4c0146d982e1e7b01f300800001,
        0xaddd578a1a7df9f7,
        Expect::Ok(0x0, 0xbee1),
    ),
    (
        Op::Cvttss2si,
        true,
        0x1d2,
        0xda7d6af1d574fb640384a80a814355b4,
        0xe6e020284caea7ea,
        Expect::Fault(0x1f2),
    ),
    (
        Op::Cvttss2si,
        true,
        0x2928,
        0xeeff6806e342adf0c3b2a41ff56c0a0,
        0xfeba065efb3cf685,
        Expect::Fault(0x2929),
    ),
    (
        Op::Cvttss2si,
        true,
        0xc6ca,
        0x67a1d035c8be6bc2d5b5a06144b7d41d,
        0x78aacef0e8105f23,
        Expect::Fault(0xc6ea),
    ),
    (
        Op::Cvttss2si,
        true,
        0xb20d,
        0x4d909023d8aec608437651ca01000000,
        0xab70c9b7b2229a79,
        Expect::Ok(0x0, 0xb22d),
    ),
    (
        Op::Cvttss2si,
        true,
        0xf86,
        0x55c1c4518c8edd940fe99d7243266bd5,
        0x996499fa13f2d189,
        Expect::Fault(0xfa6),
    ),
    (
        Op::Cvttss2si,
        true,
        0x6e88,
        0xedd4807b2e845fc75eb6adca4b800001,
        0xf4afd2aa5ff21437,
        Expect::Ok(0x1000002, 0x6e88),
    ),
    (
        Op::Cvttss2si,
        true,
        0xdd68,
        0x5569477e2917a5c20012a455b026c3e5,
        0x99f158317e743ebc,
        Expect::Ok(0x0, 0xdd68),
    ),
    (
        Op::Cvttss2si,
        true,
        0x5eec,
        0x2e1bcc17ec23874bce8586d34f000001,
        0x29bbdb011133c75,
        Expect::Ok(0x80000100, 0x5eec),
    ),
    (
        Op::Cvttss2si,
        true,
        0x727c,
        0x4de51652f5d5c17283b338c4807fffff,
        0xf8bb29305401e7b1,
        Expect::Ok(0x0, 0x727c),
    ),
    (
        Op::Cvttss2si,
        true,
        0xed51,
        0xedc6d3d49dd9229ecc48cbb46260b4ae,
        0xf270553a4808d45b,
        Expect::Fault(0xed51),
    ),
    (
        Op::Cvttss2si,
        true,
        0x47e5,
        0x8af0ad2d12043b19580840eb7fbfffff,
        0x1d8f0c0f81d588fb,
        Expect::Ok(0x8000000000000000, 0x47e5),
    ),
    (
        Op::Cvttss2si,
        true,
        0xaca4,
        0x52a66640124ee62f9a25873f40000000,
        0xdc0d09b989fa4fd2,
        Expect::Ok(0x2, 0xaca4),
    ),
    (
        Op::Cvttss2si,
        true,
        0x5c26,
        0x38f81da5a8c564a80d409686c0000000,
        0xccde5fa2924cb60f,
        Expect::Ok(0xfffffffffffffffe, 0x5c26),
    ),
    (
        Op::Cvttss2si,
        true,
        0xb039,
        0x106fbd5970cc2924c3590ff63f800001,
        0x2213a0408b87812f,
        Expect::Ok(0x1, 0xb039),
    ),
    (
        Op::Cvttss2si,
        true,
        0x7b81,
        0x29d08fbc2fcd62674b95eb1052a15856,
        0xf5ff3cb83bbc2d4f,
        Expect::Ok(0x50ac2b0000, 0x7b81),
    ),
    (
        Op::Cvttss2si,
        true,
        0xb679,
        0x38c417fa9e6d3d3b0a3e162a7f800001,
        0x9fc8ffd8b1933621,
        Expect::Fault(0xb679),
    ),
    (
        Op::Cvttss2si,
        true,
        0x55e8,
        0xc973ff72faf3380c9b959493e2ad52d5,
        0x1208a7d029922014,
        Expect::Ok(0x8000000000000000, 0x55e9),
    ),
    (
        Op::Cvttss2si,
        true,
        0x7f7b,
        0x7f422ac132e733a8c13d68763e800000,
        0xb1057f9cb0edc19e,
        Expect::Ok(0x0, 0x7f7b),
    ),
    (
        Op::Cvttss2si,
        true,
        0x1f97,
        0x8dd0a11691dddbb42f49bd4980000000,
        0x62cceae8101f02eb,
        Expect::Ok(0x0, 0x1f97),
    ),
    (
        Op::Cvttss2si,
        true,
        0xa035,
        0xabf3af2a9a4d80ba6c32571da7fe5a71,
        0xfd7be56c5d092c8f,
        Expect::Fault(0xa035),
    ),
    (
        Op::Cvttss2si,
        true,
        0xf2c2,
        0x75073cbeec6068e02109c61c3fda4c28,
        0x5c4c9f2eb981061f,
        Expect::Ok(0x1, 0xf2e2),
    ),
    (
        Op::Cvttss2si,
        true,
        0x5b7b,
        0x99051be7a266470b1bbda4d63ffcb3ff,
        0x32eed126a64137d6,
        Expect::Ok(0x1, 0x5b7b),
    ),
    (
        Op::Cvttss2si,
        true,
        0xff70,
        0x15a0e7d93962f7d15c56d3d574d88517,
        0x9acea4ed941227d,
        Expect::Fault(0xff71),
    ),
    (
        Op::Cvttss2si,
        true,
        0x8a4,
        0x5c00685f3d613ad2c68d8f8e807fffff,
        0xda933b4d12dd6b65,
        Expect::Fault(0x8a4),
    ),
    (
        Op::Cvttss2si,
        true,
        0x3a66,
        0xe89f87ebeda583961e261e037fffffff,
        0x9bce95792cb5c7ad,
        Expect::Fault(0x3a67),
    ),
    (
        Op::Cvttss2si,
        true,
        0x3cd4,
        0x12a6051902a105d695c6e77a7f7fffff,
        0xb752b92e98a73c29,
        Expect::Ok(0x8000000000000000, 0x3cd5),
    ),
    (
        Op::Cvttss2si,
        true,
        0x7cf6,
        0x2d2f20edb0f9cea74aac09277f800001,
        0x41ef27a47d58eb91,
        Expect::Ok(0x8000000000000000, 0x7cf7),
    ),
    (
        Op::Cvttss2si,
        true,
        0x6649,
        0xeacef929bedab7352e4b81432c3af12c,
        0xfdca8444b3e267ef,
        Expect::Fault(0x6669),
    ),
    (
        Op::Cvttss2si,
        true,
        0x64e7,
        0x58872420766762cad8e357474b800001,
        0x77b4db0afde17bc2,
        Expect::Ok(0x1000002, 0x64e7),
    ),
    (
        Op::Cvttss2si,
        true,
        0xac8a,
        0x8afba023b6940d3995373d6400400000,
        0x5923a5f1cd2951e5,
        Expect::Fault(0xacaa),
    ),
    (
        Op::Cvttss2si,
        true,
        0xbf0,
        0xa18fc73c168b949aadf41d8c7e800000,
        0xc3cc0a4bc52054ed,
        Expect::Ok(0x8000000000000000, 0xbf1),
    ),
    (
        Op::Cvttss2si,
        true,
        0xafe5,
        0x9e6b3989c85239beef09a4f94b800001,
        0xba2ddf9c9210df78,
        Expect::Ok(0x1000002, 0xafe5),
    ),
    (
        Op::Cvttss2si,
        true,
        0xcf4f,
        0x61f6e5a3dfd01df2c081efe1807fffff,
        0xcf618ac5a9ba6c7a,
        Expect::Ok(0x0, 0xcf4f),
    ),
    (
        Op::Cvttss2si,
        true,
        0x2378,
        0xa68167afd6853b78e4619f4b5f000000,
        0xa3d8abcd375c9d41,
        Expect::Fault(0x2379),
    ),
    (
        Op::Cvttss2si,
        true,
        0x4bd0,
        0xd16e976440867f86d56aeabfffbfffff,
        0xb85413930dcb6d99,
        Expect::Ok(0x8000000000000000, 0x4bd1),
    ),
    (
        Op::Cvttss2si,
        true,
        0x5aa2,
        0x4de1232830d0a5aee5670c054de0cc48,
        0xde43bb74f19efb92,
        Expect::Ok(0x1c198900, 0x5aa2),
    ),
    (
        Op::Cvttss2si,
        true,
        0x9598,
        0x3f22988c2354bb15169ddee94b1ef905,
        0x9ab5c06a6b6473cb,
        Expect::Ok(0x9ef905, 0x9598),
    ),
    (
        Op::Cvttss2si,
        true,
        0xe69b,
        0x9837769860217e1b3bb506a4330754a5,
        0x81f4c08a1ee411f7,
        Expect::Fault(0xe6bb),
    ),
    (
        Op::Cvttss2si,
        true,
        0xa149,
        0xb4e050f573c31d06d746ad055fd4eeb3,
        0x34c6111e44a3a6f8,
        Expect::Fault(0xa149),
    ),
    (
        Op::Cvttss2si,
        true,
        0x1f40,
        0x21e8e3a8bdfbda9f8e0a8bb434000000,
        0x7392d0ec53bf63c4,
        Expect::Ok(0x0, 0x1f60),
    ),
    (
        Op::Cvttss2si,
        true,
        0xf0c4,
        0x4463da9bcb7a1886ba36e25e00400000,
        0x860a738bace54807,
        Expect::Ok(0x0, 0xf0c4),
    ),
    (
        Op::Cvttss2si,
        true,
        0x583d,
        0xf84bfcc13f7be86d976b94477f7fffff,
        0xdb54ed51ea16c751,
        Expect::Fault(0x583d),
    ),
    (
        Op::Cvttss2si,
        true,
        0x4f80,
        0xf4fda123652b415911aa9470f8d44ee1,
        0x367811d3e88ae7d7,
        Expect::Ok(0x8000000000000000, 0x4f81),
    ),
    (
        Op::Cvttss2si,
        true,
        0xe63a,
        0x6ed4de2bbd5986cdc96a6e6200ffffff,
        0x479dada415f22578,
        Expect::Fault(0xe63a),
    ),
    (
        Op::Cvttss2si,
        true,
        0x78ff,
        0x7f8d4828242febd6dc7c09754a7e3a9b,
        0x481e9b5a2fb977b3,
        Expect::Ok(0x3f8ea6, 0x78ff),
    ),
    (
        Op::Cvttss2si,
        true,
        0x89cb,
        0x5f435c9a46081c2f3dc03cdbb3875418,
        0xd4f4183c0bc36cff,
        Expect::Fault(0x89eb),
    ),
    (
        Op::Cvttss2si,
        true,
        0x9b5f,
        0x3f5192fa52e19bf964b43796a6e78d2a,
        0x589dadb38477d516,
        Expect::Ok(0x0, 0x9b7f),
    ),
    (
        Op::Cvttss2si,
        true,
        0x1df4,
        0x9b8934bf351c8a94d4f770a8c05b2239,
        0x56b3d7c0c41182ab,
        Expect::Ok(0xfffffffffffffffd, 0x1df4),
    ),
    (
        Op::Cvttss2si,
        true,
        0x9807,
        0x6ff433cc880918fa3393e068fa721eec,
        0xfe1b32ad83fb4050,
        Expect::Fault(0x9807),
    ),
    (
        Op::Cvttss2si,
        true,
        0x752,
        0xd8fdab38228231ad2d9b5cfc7fbfffff,
        0x8eeefde352ded1ae,
        Expect::Fault(0x753),
    ),
    (
        Op::Cvttss2si,
        true,
        0x4b82,
        0xa493a971a4d202f56818233240000000,
        0xc36a3901f281d2c2,
        Expect::Ok(0x2, 0x4b82),
    ),
    (
        Op::Cvttss2si,
        true,
        0x9f41,
        0x20eb8d6e4897a7bee1f357cd7f7fffff,
        0xe6cbe7321a81b154,
        Expect::Fault(0x9f41),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x1dc0,
        0x30c3bdd0c1abc70fbcb75d5a0cbfe7e2,
        0x836234190fd3882c,
        Expect::Ok(0x0, 0x1de0),
    ),
    (
        Op::Cvtsd2si,
        false,
        0xfd80,
        0xfa5aff441ab12b668000000000000000,
        0xca7fb50f20890a7d,
        Expect::Ok(0x0, 0xfd80),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x6fc0,
        0xde3e45996531939c3fe0000000000000,
        0xd279a9ffc84027ea,
        Expect::Fault(0x6fe0),
    ),
    (
        Op::Cvtsd2si,
        false,
        0x6000,
        0x947d4c6b434d3a0a7ff8000000000001,
        0x3736a73c545d25c1,
        Expect::Fault(0x6001),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x8000,
        0x3b66f63c193a7523ffefffffffffffff,
        0xb4828661dc0c7a1d,
        Expect::Fault(0x8001),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x1700,
        0x4e5ff69e90d57d307ff0000000000000,
        0x5ae6f18b3447bfc3,
        Expect::Fault(0x1701),
    ),
    (
        Op::Cvtsd2si,
        true,
        0x3d80,
        0x1a670f562455c31dc3e0000000000001,
        0xa7c74a675c17cc8,
        Expect::Ok(0x8000000000000000, 0x3d81),
    ),
    (
        Op::Cvtss2si,
        false,
        0x5e80,
        0x1b4aa683fdc609d179e6d3ce00ffffff,
        0xc79e73adf37ff0da,
        Expect::Ok(0x1, 0x5ea0),
    ),
    (
        Op::Cvtss2si,
        false,
        0x4f80,
        0xe712debbd1ed0125648a607bfc00000,
        0xab3c0df9c6efe058,
        Expect::Fault(0x4fa0),
    ),
    (
        Op::Cvtss2si,
        false,
        0x7b80,
        0x8d47208764011c2669d53345bf800000,
        0x87a408eef4cf0a7f,
        Expect::Ok(0xffffffff, 0x7b80),
    ),
    (
        Op::Cvtss2si,
        false,
        0x7b80,
        0xd2a0669a51f3da88a2458a0634000000,
        0xcf6d72a82635e10a,
        Expect::Ok(0x0, 0x7ba0),
    ),
    (
        Op::Cvtss2si,
        false,
        0x9f00,
        0xef2507be8f8fd5dfff37f0834f800000,
        0xf4b8ad8fea324feb,
        Expect::Fault(0x9f01),
    ),
    (
        Op::Cvtss2si,
        true,
        0x8f80,
        0x9765a8ac0b9a7781e39c84962b9259cd,
        0x9e808f59dfa2e2a3,
        Expect::Fault(0x8fa0),
    ),
    (
        Op::Cvtss2si,
        true,
        0xfc0,
        0x58c96eaf9a4e0068b8714c75f000000,
        0x4223b6f6fad77426,
        Expect::Ok(0x8000000000000000, 0xfc1),
    ),
    (
        Op::Cvtss2si,
        true,
        0xfe80,
        0x584cd37bb8ecc03750353d97f7fffff,
        0x8661e7ae12178fa5,
        Expect::Ok(0x8000000000000000, 0xfe81),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x9f00,
        0xd6ca77a55c159b66c3e0000000000000,
        0x99a8dc32c4bc57bf,
        Expect::Fault(0x9f01),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x7f40,
        0x247f8edddedf109c41e0000000000000,
        0x9add94209ab9a29,
        Expect::Fault(0x7f41),
    ),
    (
        Op::Cvttsd2si,
        false,
        0x80c0,
        0xc6f6330316b6ed6bc1e0000000000000,
        0xe231e835cfb8fbc3,
        Expect::Ok(0x80000000, 0x80c0),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x1f40,
        0x90597572e593f15e71a1beeea859f10e,
        0x815e296abcd9c6e5,
        Expect::Fault(0x1f41),
    ),
    (
        Op::Cvttsd2si,
        true,
        0x8000,
        0x655fb4d9a6eedea7c00b85be03d33045,
        0x8c3246263e82ac45,
        Expect::Fault(0x8020),
    ),
    (
        Op::Cvttss2si,
        false,
        0x6000,
        0x1dba422d8b90c96d605b55c97fa00000,
        0xbe76e8b5d9d5d2d5,
        Expect::Fault(0x6001),
    ),
    (
        Op::Cvttss2si,
        false,
        0x1f95,
        0xfcc11fd077aa3780c38ae3c23fc00000,
        0xfe1674970f4bc701,
        Expect::Ok(0x1, 0x1fb5),
    ),
    (
        Op::Cvttss2si,
        true,
        0x9780,
        0xf5ca0cad175ae44c7baa8a267e800000,
        0xb32ac50a840c0998,
        Expect::Ok(0x8000000000000000, 0x9781),
    ),
    (
        Op::Cvttss2si,
        true,
        0x2000,
        0xaf6cb9defe1afe44f5b2c65ec0200000,
        0x89a17deb8652d5f8,
        Expect::Fault(0x2020),
    ),
    (
        Op::Cvttss2si,
        true,
        0x6000,
        0x6657f20b33df3340a07b23714f000001,
        0x9846998dfc6d1731,
        Expect::Ok(0x80000100, 0x6000),
    ),
    (
        Op::Cvttss2si,
        true,
        0x1f95,
        0x6dcc3cc19919e6e206e5846c8065c342,
        0xe0085fa928498f0a,
        Expect::Ok(0x0, 0x1fb5),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x1fc0,
        0x1fffffffff95589153d576cac,
        0x2dd5332c1d21e99fcb45a0dbf5bb00ed,
        Expect::Ok(0xc19aa9dbac00000041ceabb656000000, 0x1fc0),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x1fbf,
        0x9134fef50ad671ba00000001c4ec4e46,
        0x81647d4dc4f18c41098d58db7c648276,
        Expect::Ok(0x3ff0000000000000c1cd89d8dd000000, 0x1fbf),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x1d80,
        0xffe320583de5da09001fffffffffffff,
        0x185711ac90b8bf5b94b95c6c65d04a94,
        Expect::Ok(0x8000000000000000, 0x1da1),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x5f00,
        0xc0040000000000004330000000000001,
        0x519af5380f239edaf8e120d3dce8a90d,
        Expect::Fault(0x5f01),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x5780,
        0x2000000000000041dfffffffc00000,
        0xd574970076dda58511cc48e6e5f7789d,
        Expect::Ok(0x17fffffff, 0x57a0),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x8f80,
        0x43300000000000017ff4000000000000,
        0x950db839c2f11fe74d510a9da43a6f98,
        Expect::Ok(0x8000000080000000, 0x8f81),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x8f80,
        0x7fd0000000000000bcb66fcfc78161d5,
        0xabf32c60c1346f7c8941bc31cdc6004c,
        Expect::Fault(0x8fa1),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xfc0,
        0x3fe0000000000000800fffffffffffff,
        0x6b7d1c0ab402d4326fda788b1611e042,
        Expect::Fault(0xfe0),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x7f40,
        0x800fffffffffffff0000000000000003,
        0xfa2d31437c719fcde52cec37715b2239,
        Expect::Ok(0x0, 0x7f40),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x6fc0,
        0x43e00000000000009ade9a45116e28d1,
        0x5c7dcd2e4f13778d7cdb5cebd830db94,
        Expect::Fault(0x6fe1),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x80c0,
        0xbcbf3ae15cd90563c1e0000000200000,
        0x32c277706f4d2ffef6d0168b4a101b72,
        Expect::Fault(0x80e1),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x1780,
        0x40080000000000004008000000000000,
        0xd369edae5ad3f65f13143025414a265e,
        Expect::Ok(0x300000003, 0x1780),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x380,
        0x3cb1445e028b9a19bca82fbf55e0c28f,
        0xaf276a3a111ebc743d88cdc69976b0b2,
        Expect::Fault(0x3a0),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x1a00,
        0x43400000000000003ff8000000000000,
        0x84e7f7b46bb972542bc10e84dee638df,
        Expect::Fault(0x1a01),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x5d80,
        0x7fe10000000000007ff0000000000001,
        0xe496e2e7269754702e73c5c7c5da9081,
        Expect::Ok(0x8000000080000000, 0x5d81),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x7f00,
        0x5deae059b25e8b824004000000000000,
        0xa3ac8d6182d36250b9df0ad38c1236e8,
        Expect::Fault(0x7f01),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x8f80,
        0x3fe0000000000000ffeba270523557db,
        0x7993129c45a453d71051ea19d31aa6d,
        Expect::Fault(0x8fa1),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xfe80,
        0xc3257e89fa8cef1bbcafdb8cbabffb15,
        0x4ba6d12c16ab23f2ff089e3ecc86ef6f,
        Expect::Ok(0x8000000000000000, 0xfea1),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x7ec0,
        0x8008648a4b8a8b5143dfffffffffffff,
        0x60838e2059abc1bf819b1f2dcb8f5569,
        Expect::Ok(0x80000000, 0x7ec1),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xfb80,
        0x3ff000000000000041dfffffffc00000,
        0x7867b6c2aa392cc6986ae59a820d5e0a,
        Expect::Ok(0x17fffffff, 0xfb80),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x1f95,
        0x3ff8000000000000800fffffffffffff,
        0x3d840b2b2e63f78cb6f0816ddff8798a,
        Expect::Ok(0x100000000, 0x1fb5),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x0,
        0x30000000000000001,
        0xe26d8252b64c958acdf0c9cf592798d9,
        Expect::Fault(0x20),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x3f80,
        0x400800000000000041dfffffffc00000,
        0x9c65014fcb6a67f866fd4f4b23a29f49,
        Expect::Ok(0x37fffffff, 0x3f80),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x0,
        0xbff00000000000000000000000000001,
        0x3fb78d9e34ef4c270749e6c3ae9ff49,
        Expect::Fault(0x20),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xffc0,
        0x3fe000000000000041dfffffffe00000,
        0x7c88443908109ab059e8689d88224146,
        Expect::Ok(0x7fffffff, 0xffe0),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x8d61,
        0x6f56a2a17798ebc1bb13e29825f8a3df,
        0x7d60c2b96992b4d0b51770f9490bc1cd,
        Expect::Ok(0xc1d13b075a00000041c2fc51ef800000, 0x8d61),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x26ca,
        0xa9966c8763ecd5c1917ba56d4573152e,
        0xa682bec41085ab5a8d82f687d4a09cc1,
        Expect::Ok(0xc1dba116a4c0000041d15cc54b800000, 0x26ca),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xf4da,
        0x206ace48dfdba64dbac89f091604f412,
        0xaac36788a3aeb233b8fd570ae88b4eb0,
        Expect::Ok(0xc1d14dd83dc0000041b604f412000000, 0xf4da),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x7228,
        0x3a1ce9997354ebd68b1058a2314ba793,
        0xbecc1ffd41201c2f8664ad1b5a53c15b,
        Expect::Ok(0xc1dd3be9d780000041c8a5d3c9800000, 0x7228),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xff3a,
        0x1361966ec46459a0d3ae106eac830019,
        0x6eb20388db0ed8eb4631a3fc728c1b64,
        Expect::Ok(0xc1c628f7c9000000c1d4df3ff9c00000, 0xff3a),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x9b9f,
        0xd228b337c3b8051698986450fdc44529,
        0x583f97f745eaf9ce90f58efab7b44867,
        Expect::Ok(0xc1d9d9e6ec000000c181ddd6b8000000, 0x9b9f),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xcfea,
        0x366b0adfafa64061f898a555b40d81e1,
        0x21cabaa56a0f3a9bcba992a4ba8af6e3,
        Expect::Ok(0xc19d9d6aac000000c1d2fc9f87c00000, 0xcfea),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x1e53,
        0x54e2f7a09c1cbef0909a7bf2e39dcadf,
        0x4807a1a0491f539d3088283918254ca3,
        Expect::Ok(0xc1dbd96103800000c1bc623521000000, 0x1e53),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xfc0e,
        0x31f1a826a7ff327d544079f795c2bc8d,
        0xd3c94375838ffcfdbb977f8124325b43,
        Expect::Ok(0x41d5101e7dc00000c1da8f50dcc00000, 0xfc0e),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x4a1f,
        0x9cb03d3a9d1e47ca91d99eb50227da61,
        0x22108eac38dd3a876310c5cbcecdbf23,
        Expect::Ok(0xc1db899852c0000041813ed308000000, 0x4a1f),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xa5c0,
        0x1d833ecd001e68ccd6a9434c065fe71,
        0x80da15afd934ddd2b2afc9930aa2eeeb,
        Expect::Ok(0xc1c94ab5e6000000c1cfcd00c7800000, 0xa5c0),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x7b72,
        0x7995ecf33b7452f828155b9ed8dfe7ea,
        0x3523cfeab4b21c93d9d02723cc3a500f,
        Expect::Ok(0x41c40aadcf000000c1c3900c0b000000, 0x7b72),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x2d4,
        0xe8817e4b57a0985595c43e43f7174e44,
        0x36bb26053b244ad21b054ab7f2dc2546,
        Expect::Ok(0xc1da8ef06f400000c1a1d16378000000, 0x2d4),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xb92e,
        0x6ac8ba8af7f0cfdc9e31dd790837e81b,
        0x60b5e3b0787371bce99145a99be6764b,
        Expect::Ok(0xc1d87388a1c0000041a06fd036000000, 0xb92e),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x5f03,
        0xfb28cd75e197388284ca0cd9ceae2a64,
        0xe05d73ae2affb9416bf608d4579c163,
        Expect::Ok(0xc1decd7cc9c00000c1c8a8eace000000, 0x5f03),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xad32,
        0xf35988a40f9d5d1dd840be6172c3e120,
        0xeb776acaa2cefe7576c068c1e65e6fd8,
        Expect::Ok(0xc1c3dfa0cf80000041dcb0f848000000, 0xad32),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xfe3c,
        0x8b140ef33a421df24d7bf66b4d2f6782,
        0x184167f392ec6f390e0ee76962f4c040,
        Expect::Ok(0x41d35efd9ac0000041d34bd9e0800000, 0xfe3c),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x8bc9,
        0x6476df4f9efb99bc46ebb1e0fb7e5bc6,
        0xd21fd5514d0bd2bdc554354f3b57f7a2,
        Expect::Ok(0x41d1baec78000000c1920690e8000000, 0x8bc9),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x5dea,
        0x3f15c217198db89ff3d34dab794ba5bb,
        0x427a40e51879d6c0f963530e272e4d49,
        Expect::Ok(0xc1a85964aa00000041de52e96ec00000, 0x5dea),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x64b4,
        0xafb65e778e4888cbf3ddf9dbbda783c0,
        0x11982a9e5dd4fb04fec0b88b17363ec3,
        Expect::Ok(0xc1a8440c4a000000c1d0961f10000000, 0x64b4),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xc4c3,
        0xca4495e2df396875d770d9c3d869481a,
        0xa3f422caeb8777d9c0e383892e7406ef,
        Expect::Ok(0xc1c447931e800000c1c3cb5bf3000000, 0xc4c3),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xe7a9,
        0x9b10af32bfb9cbe1f6bcebf0937d96f2,
        0xd39c6580a3427c113c7b112adafe988f,
        Expect::Ok(0xc1a2862820000000c1db209a43800000, 0xe7a9),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xdb98,
        0xe01b588e73ace1f4de88bde2953c6b21,
        0x263f53ea22e4393c624c8bfae45c5e5e,
        Expect::Ok(0xc1c0bba10f000000c1dab0e537c00000, 0xdb98),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xed8a,
        0xff42c03fa6a2ee6e62d5b066cd3553d6,
        0xfe1b2398004c04f547d719e616f3cdba,
        Expect::Ok(0x41d8b56c19800000c1c9655615000000, 0xed8a),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xf99e,
        0xfded284ec1e4fd589d973c995681e174,
        0x917b5531da461e09527fdb1c9b58ad9e,
        Expect::Ok(0xc1d89a30d9c0000041d5a0785d000000, 0xf99e),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x8347,
        0xa28daab0b6cd0f566845f50287f34189,
        0x25edb35aad286cb9b6d9fa57687a188e,
        Expect::Ok(0x41da117d40800000c1de032f9dc00000, 0x8347),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xf399,
        0x32752e8c781be6f3138e63ccf22bddfc,
        0x8b4a38f222be0f74c7983fdbc8c20ad1,
        Expect::Ok(0x41b38e63cc000000c1aba84408000000, 0xf399),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0x843,
        0x9f1e449ca63b73c72e1dbd213d45df03,
        0xd7d9c2299640ec156ca7ee7a8c99a5ea,
        Expect::Ok(0x41c70ede9080000041cea2ef81800000, 0x843),
    ),
    (
        Op::Cvtdq2pd,
        false,
        0xd75a,
        0x5be18f3ff26ba27fa5e665fcc013957f,
        0xc9f24248ccc191de1f8512dc2c854a2d,
        Expect::Ok(0xc1d6866681000000c1cff63540800000, 0xd75a),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x5769,
        0xadeb626f07a08839d9565d1ab8488b9a,
        0x1cc3638422674f38de85333d9bb43932,
        Expect::Fault(0x5769),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xc84c,
        0xae97617b831ef40065e1f55c55410284,
        0x2c5245e47ac0e993e29dc361849df8cc,
        Expect::Fault(0xc84d),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x7797,
        0xdc67c031b8f51188959a7377b078ea4b,
        0x262a06323e6949891ea9659edfcf0a65,
        Expect::Ok(0x8000000000000000, 0x77b7),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x51d3,
        0xd4bf43ff19693673d9170267b4a64a49,
        0x92a0ccd6d1edeba071c221047511a78d,
        Expect::Ok(0x8000000080000000, 0x51d3),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x9e9c,
        0xfa94029d6da974b73eb1dff84674bb80,
        0x94dab41730f7c5cf75af2323e16d3558,
        Expect::Ok(0x8000000000000000, 0x9ebd),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x916a,
        0xc54fdcf9da6d445e21bf7806676864f7,
        0x3ed14a6f8d651b09044dedd56c404416,
        Expect::Fault(0x916b),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xeec2,
        0x46d6234169eabddf9068d96e0030bb5b,
        0x1e25b6b035e4c02b2b2aa310ae3bada1,
        Expect::Fault(0xeee3),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xadb,
        0x5102067d35582e93b12c31417f66e30c,
        0x15e22293354cd8d395737dddb00841f0,
        Expect::Fault(0xafb),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x7ff4,
        0xff97b8e8fbff7bee9c1bb6859bacd8c0,
        0xc09577f2faad56ba64f2ad8388268b0,
        Expect::Ok(0x8000000000000000, 0x7ff5),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x6f06,
        0x5817c66fde5fa3739f740bf092744bfd,
        0x908e10e4352f023cf38db5bfdc4c76f7,
        Expect::Fault(0x6f07),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x8bbe,
        0xa8d6bb287810ecb53e1b58131802c7ba,
        0x3b6fc45c52bece8df3fca6db5b245aa2,
        Expect::Fault(0x8bbe),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x2703,
        0xd96c7ed4cb6c4da4ea0e0ea4ee6a2971,
        0x5e41ef084105fe6b6b4d9ccc6c25f25d,
        Expect::Fault(0x2703),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x7de4,
        0x807cba8d8fae4e8c029e280f14114aac,
        0xc7e65d4df299b295c09ebdd1c551419c,
        Expect::Ok(0x0, 0x7de4),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x3975,
        0x7fea79ca9ceb24405b27afe93cbb87,
        0x3d6f7472dc9a76c9710232af99dd805b,
        Expect::Ok(0x6c, 0x3975),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x9bf6,
        0xd8f92aef5a61245a8ad76794dbfcbccf,
        0xe162ad17180b0f15de46c784f05cc715,
        Expect::Ok(0x8000000000000000, 0x9bf7),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x8d59,
        0x4f21392bff6fba10ad5ad7ff45027234,
        0xc1a8b05d4010b258004389a010dc350f,
        Expect::Fault(0x8d59),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xe162,
        0x859115c6339cba0dcb479b6c798f02da,
        0x79fa452239b55b8ee0e7970af4fc5005,
        Expect::Fault(0xe163),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x6ef5,
        0x55d2c98444da923fcf58145bb6a8a722,
        0x7f543b610674cce348a480f7e40c1e21,
        Expect::Ok(0x8000000080000000, 0x6ef5),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xdfd6,
        0x69a710a722a58a63532aca6ae510108,
        0xefad584fb51c80dbeb5bab1ef1526bef,
        Expect::Ok(0x100000001, 0xdff6),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x3cc5,
        0x2220b7fd0d956dfd975d465c19f321c8,
        0xa36738c2e6300fa86554455a9a019374,
        Expect::Ok(0xffffffff, 0x3ce5),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x3236,
        0x4820bb637127526fa4b2d085762d47be,
        0xf47fdf5625c0f2d96ce456d5abfce60c,
        Expect::Fault(0x3237),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xb0d2,
        0x2c87828cfe59551c724fe807660d02fe,
        0x4e364540236c27442f21f70d959b15bf,
        Expect::Ok(0x80000000, 0xb0f3),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xc748,
        0xaa43dbbb4148bfa00a8a69412501b2b0,
        0xd21a4ec99571c63b2720bcff0e6845eb,
        Expect::Fault(0xc768),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0xc758,
        0x7d096ea0893505ce91eeffc294ac26a2,
        0x9b5daeae81df588b0a7818d28a4e475d,
        Expect::Fault(0xc759),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x450,
        0xe221b47564acadbe07d8d91179a00e67,
        0x8cacac4e9c6a9fa2e65ca745b90ad950,
        Expect::Fault(0x451),
    ),
    (
        Op::Cvtpd2dq,
        false,
        0x3f24,
        0xa6e9c948e74c9e116df8ffe92bf5b7eb,
        0xb23713136d2ba6751e4f662cb48b9bfc,
        Expect::Fault(0x3f25),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x3416,
        0xab81d01075e0e398f126294af7b3aaa8,
        0x5a3a7eeaaea5f94cc30262ce64f4d4d1,
        Expect::Fault(0x3417),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x7e8e,
        0x3c5080ee4160183cf87b51c90cae224a,
        0x1931f3e968a075e249ec8e7f2021b775,
        Expect::Ok(0x80000000, 0x7eaf),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xede9,
        0x14f8cd7f9de85ed6a8c9800132313283,
        0x20d349e590d6c0ef53025d31fc4cd6b,
        Expect::Fault(0xede9),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xb5,
        0xcf09cbe407937de7f04ae5b81a9902b9,
        0x12a283d9b46168e5c1ad0d13abab2bbb,
        Expect::Ok(0x8000000080000000, 0xb5),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x3587,
        0x87251d833b18fdb189163121fd2097f3,
        0xb95624fab3697ede1416a8c51eaeb25,
        Expect::Ok(0x0, 0x35a7),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xe946,
        0xdeb422b14b670c780e999ba92dd5a457,
        0x83e703cab1aada51c78ea0113e64337e,
        Expect::Fault(0xe947),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x225f,
        0xad53911c82011a65583065e71705d7e,
        0x825c0609d07d0a0671d89e19af9b90f2,
        Expect::Fault(0x225f),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x4ed5,
        0xa0c6ce56e55679fb3cdce5acd938b525,
        0x719f01f1946ce047bf6978163a06de3c,
        Expect::Fault(0x4ef5),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x847e,
        0x3acd36c9240d21be6babbcbd8c43e225,
        0x210d9c0eddc06d49ef5547d9803de1f2,
        Expect::Fault(0x847f),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xd32d,
        0x2cf1352d2258aa656c1bddb41bde5ce7,
        0x5d4a3fcc59ffa1f6cb891c141930718e,
        Expect::Fault(0xd32d),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x5297,
        0xa88636f7c4c2f9cf16c24eeebd02e335,
        0x1e48f6fa6da56214c70526664230819,
        Expect::Ok(0x0, 0x52b7),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xafda,
        0xea4a909bf30efebe65095a5e3bd30505,
        0x7e4106931ee8c3c8fc658d53db4e9410,
        Expect::Ok(0x8000000080000000, 0xafdb),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xa1a9,
        0x46ab9a87e4e79e378bcce957f4fc1c9c,
        0x4c87498e582a8325b8b4e72850c6422,
        Expect::Fault(0xa1a9),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x8ad3,
        0xbe7c3b5b6ce9dfca9d16491cd0379074,
        0xf9096765a84984832e6588eccaca2bdd,
        Expect::Fault(0x8af3),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x3273,
        0x36aaad0ae3a77b6cf3ba85217e492301,
        0x961da125a58b8ee6808891b1207360e1,
        Expect::Fault(0x3273),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xb7ad,
        0x92a43c4609b3a96e8fc21ec6d61f4a0b,
        0x91f28a83d5b36404e9790205a007d20e,
        Expect::Ok(0x0, 0xb7ad),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x57af,
        0xe8811e58299e6ef2bc97f924cb1d7cb7,
        0x320345e59b32c9073daf8ec9468a20ca,
        Expect::Ok(0x8000000000000000, 0x57af),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xc6e0,
        0xf73737197fb03ab03482a1a205833dc6,
        0x539d763611ab7dd95a524d03471a11b7,
        Expect::Fault(0xc6e1),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x80c0,
        0xd32df86861334222077e3bb6d96360cb,
        0xb028c3f842e0482b95b8976c6d8e39a4,
        Expect::Fault(0x80e1),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xaa52,
        0x41e5e9d64efddb64099f5c7de33fd139,
        0xb563cae1a1e4589869f36b169fc69094,
        Expect::Fault(0xaa53),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x1d54,
        0xf440892476b66d8f2cdf5f017a4ac56e,
        0xbcb2a287a5f6b6295dc4700971293de5,
        Expect::Fault(0x1d55),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xea11,
        0xc7de023756b3aa2b27ca7afa82473b55,
        0xaf8e3a46e5aad8bf3048469ddc279287,
        Expect::Fault(0xea11),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x3f85,
        0x65736efbc88f526acef19b45450c2532,
        0x49e303d9103e44b9dde8b297c2738d72,
        Expect::Ok(0x8000000080000000, 0x3f85),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x917a,
        0xa0620916f8a2599e7d98b8c1ad65d208,
        0xcb26021f5d52f556a99557615c5ec097,
        Expect::Fault(0x917b),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xbf47,
        0x97945c1715b521613c04752ebb08eec,
        0x35cc2c6eda4d9fc3316dab4c67e50a6f,
        Expect::Ok(0x0, 0xbf67),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0xf7b3,
        0x6d3711a27414f0449b432d965512162d,
        0xb3b0d4902a5e1a052d65f5533417061c,
        Expect::Ok(0x8000000000000000, 0xf7b3),
    ),
    (
        Op::Cvttpd2dq,
        false,
        0x1e0f,
        0xe7685358069991af0ace3049852bb31,
        0x457ebe38f3e7d36d4cb93d8e06e9c0e9,
        Expect::Fault(0x1e0f),
    ),
];

const GOLDEN_MMX: &[MmxRow] = &[
    (
        Mmx::Cvtpd2pi,
        0x1f80,
        6,
        0x7ff00000000000007fefffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0x1f81),
    ),
    (
        Mmx::Cvtpd2pi,
        0xf80,
        0,
        0xbfdfffffffffffff0000000000000001,
        0x0,
        Expect::Fault(0xfa0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x3f00,
        7,
        0x41dfffffffc00000,
        0x0,
        Expect::Ok(0x7fffffff, 0x3f00),
    ),
    (
        Mmx::Cvtpd2pi,
        0x2f80,
        0,
        0xfff0000000000000bcbd577588afaf01,
        0x0,
        Expect::Fault(0x2fa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x3f80,
        0,
        0x89f0a077d76240844000000000000000,
        0x0,
        Expect::Ok(0xffffffff00000002, 0x3fa0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x3f80,
        4,
        0x40140000000000004008000000000000,
        0x0,
        Expect::Ok(0x500000003, 0x3f80),
    ),
    (
        Mmx::Cvtpd2pi,
        0x3f80,
        1,
        0xc3dfffffffffffff3ff8000000000000,
        0x0,
        Expect::Ok(0x8000000000000001, 0x3fa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x5e80,
        6,
        0xc3245ca7a40d51f9c1dfffffffe00000,
        0x0,
        Expect::Ok(0x8000000080000001, 0x5ea1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x5780,
        2,
        0xd0f28d5070fd20010000000000001,
        0x0,
        Expect::Ok(0x100000001, 0x57a0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x7f00,
        3,
        0x3ff00000000000014330000000000001,
        0x0,
        Expect::Fault(0x7f01),
    ),
    (
        Mmx::Cvtpd2pi,
        0x7d80,
        5,
        0x7fdfffffffffffff4000000000000000,
        0x0,
        Expect::Ok(0x8000000000000002, 0x7d81),
    ),
    (
        Mmx::Cvtpd2pi,
        0x7d80,
        5,
        0x3fd00000000000007ff7ffffffffffff,
        0x0,
        Expect::Ok(0x80000000, 0x7da1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x7b80,
        0,
        0xbca0d49b4c16e87c802b2991812520cb,
        0x0,
        Expect::Ok(0x0, 0x7ba0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x5f80,
        2,
        0xa82aebc86ef16c01432a070f346c0c66,
        0x0,
        Expect::Ok(0x80000000, 0x5fa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x6f80,
        5,
        0xfffffffffffff7ff0000000000001,
        0x0,
        Expect::Fault(0x6fa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x9f00,
        1,
        0xc329485ca917337200209769a813a6f1,
        0x0,
        Expect::Fault(0x9f01),
    ),
    (
        Mmx::Cvtpd2pi,
        0x7f80,
        3,
        0x57dda23d4c82fa6ac3dfffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0x7f81),
    ),
    (
        Mmx::Cvtpd2pi,
        0x8f80,
        1,
        0xc1e00000001fffffbff5c91aef9a1f7f,
        0x0,
        Expect::Fault(0x8fa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0xfc0,
        3,
        0xc1e00000000000004008000000000000,
        0x0,
        Expect::Ok(0x8000000000000003, 0xfc0),
    ),
    (
        Mmx::Cvtpd2pi,
        0xfc0,
        6,
        0x3ff80000000000008000c02cde67c627,
        0x0,
        Expect::Fault(0xfe0),
    ),
    (
        Mmx::Cvtpd2pi,
        0xfe80,
        0,
        0xc1e00000000000004014000000000000,
        0x0,
        Expect::Ok(0x8000000000000005, 0xfe80),
    ),
    (
        Mmx::Cvtpd2pi,
        0x1fc0,
        0,
        0xbe6e39faec2f15bd7ff0000000000001,
        0x0,
        Expect::Ok(0x80000000, 0x1fe1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x1fc0,
        6,
        0x20c43df25db60c00e8caf789e0833,
        0x0,
        Expect::Ok(0xfffffffc, 0x1fe0),
    ),
    (
        Mmx::Cvtpd2pi,
        0xfb80,
        2,
        0x3fd00000000000000000000000000001,
        0x0,
        Expect::Ok(0x0, 0xfba0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x77c0,
        6,
        0x3fe00000000000007ffd28fc600b1847,
        0x0,
        Expect::Ok(0x80000000, 0x77e1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x6fc0,
        1,
        0xc00c57b6d3855f9f3ff0000000000000,
        0x0,
        Expect::Fault(0x6fe0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x2000,
        6,
        0x40000000000000007ff8000000000000,
        0x0,
        Expect::Fault(0x2001),
    ),
    (
        Mmx::Cvtpd2pi,
        0x4000,
        2,
        0xbff00000000000004000000000000000,
        0x0,
        Expect::Ok(0xffffffff00000002, 0x4000),
    ),
    (
        Mmx::Cvtpd2pi,
        0x4000,
        5,
        0x3fefffffffffffffc004000000000000,
        0x0,
        Expect::Fault(0x4020),
    ),
    (
        Mmx::Cvtpd2pi,
        0x9f80,
        2,
        0x6b19890cc6c98fc2c1e0000000200000,
        0x0,
        Expect::Ok(0x8000000080000000, 0x9f81),
    ),
    (
        Mmx::Cvtpd2pi,
        0x9f80,
        6,
        0xbfdfffffffffffff3ca6f7fd3b46e4da,
        0x0,
        Expect::Ok(0x0, 0x9fa0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x80c0,
        3,
        0x400000000000000000086a791e03f7dc,
        0x0,
        Expect::Ok(0x200000000, 0x80c0),
    ),
    (
        Mmx::Cvtpd2pi,
        0x1f95,
        4,
        0x1000000000000141e0000000000001,
        0x0,
        Expect::Ok(0x80000000, 0x1fb5),
    ),
    (
        Mmx::Cvtpd2pi,
        0x5faa,
        1,
        0xbfdfffffffffffff8000000000000000,
        0x0,
        Expect::Ok(0x0, 0x5faa),
    ),
    (
        Mmx::Cvtpd2pi,
        0x5faa,
        7,
        0x7ff00000000000010000000000000000,
        0x0,
        Expect::Ok(0x8000000000000000, 0x5fab),
    ),
    (
        Mmx::Cvtpd2pi,
        0x1d80,
        2,
        0xffefffffffffffff0008000000000000,
        0x0,
        Expect::Ok(0x8000000000000000, 0x1da1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x1b80,
        3,
        0x8000000000000000fff0000000000000,
        0x0,
        Expect::Ok(0x80000000, 0x1b81),
    ),
    (
        Mmx::Cvtpd2pi,
        0xffc0,
        6,
        0x8000000000000001bff0000000000000,
        0x0,
        Expect::Ok(0xffffffff, 0xffc0),
    ),
    (
        Mmx::Cvtpd2pi,
        0xffc0,
        2,
        0xffefffffffffffff8000000000000001,
        0x0,
        Expect::Ok(0x8000000000000000, 0xffc1),
    ),
    (
        Mmx::Cvtpd2pi,
        0xffc0,
        5,
        0x7ff8000000000000bca7996a7b29df34,
        0x0,
        Expect::Ok(0x8000000000000000, 0xffe1),
    ),
    (
        Mmx::Cvtpd2pi,
        0xf80,
        5,
        0xc1e00000001fffff400735ce1d8f7258,
        0x0,
        Expect::Fault(0xfa1),
    ),
    (
        Mmx::Cvtpd2pi,
        0x0,
        5,
        0xfff68d1576614b4043dfffffffffffff,
        0x0,
        Expect::Fault(0x1),
    ),
    (
        Mmx::Cvtpi2pd,
        0x1f80,
        1,
        0x12345678,
        0xcc9bdae22f4b707891782565ee598dbe,
        Expect::Ok(0x41b2345678000000, 0x1f80),
    ),
    (
        Mmx::Cvtpi2pd,
        0x5f00,
        7,
        0x1dcfa8fe01000001,
        0x9399668059e134111a7e43f1df6301ee,
        Expect::Ok(0x41bdcfa8fe0000004170000010000000, 0x5f00),
    ),
    (
        Mmx::Cvtpi2pd,
        0x9d80,
        2,
        0x300000001,
        0xaa78d39ef84e4633d6d8770875587daf,
        Expect::Ok(0x40080000000000003ff0000000000000, 0x9d80),
    ),
    (
        Mmx::Cvtpi2pd,
        0x1fc0,
        4,
        0xfffffffffe000001,
        0xc36c55d0381d72380ade8f871665ade2,
        Expect::Ok(0xbff0000000000000c17ffffff0000000, 0x1fc0),
    ),
    (
        Mmx::Cvtpi2pd,
        0xfb80,
        5,
        0x800000019a3069ad,
        0xac4131fbf7f62b45af0c070a6695073d,
        Expect::Ok(0xc1dfffffffc00000c1d973e594c00000, 0xfb80),
    ),
    (
        Mmx::Cvtpi2pd,
        0x40,
        4,
        0x168197b4e,
        0x63488bfcdc9073eb9185c516ddc800b1,
        Expect::Ok(0x3ff000000000000041da065ed3800000, 0x40),
    ),
    (
        Mmx::Cvtpi2pd,
        0x80c0,
        1,
        0xfffffffe80000000,
        0x3efac41e9138adb754bd3e23ef995df3,
        Expect::Ok(0xc000000000000000c1e0000000000000, 0x80c0),
    ),
    (
        Mmx::Cvtpi2pd,
        0x1fbf,
        0,
        0xffef82e95d85ecba,
        0xcedab5d74d3c57df7bb1d52348773627,
        Expect::Ok(0xc1307d170000000041d7617b2e800000, 0x1fbf),
    ),
    (
        Mmx::Cvtpi2pd,
        0x5faa,
        6,
        0xfffffe00,
        0xed8fcfc7a74bb3f7064a1c9eaa8e893,
        Expect::Ok(0xc080000000000000, 0x5faa),
    ),
    (
        Mmx::Cvtpi2pd,
        0xfbf,
        2,
        0x48ddc5c1fffffffe,
        0x77fb070eeb725a1d4132ad8de65a4118,
        Expect::Ok(0x41d2377170400000c000000000000000, 0xfbf),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1f80,
        3,
        0xfffffffffffffc00,
        0xe88083ad7e248552dba541d171700dd3,
        Expect::Ok(0xe88083ad7e248552bf800000c4800000, 0x1f80),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1f80,
        2,
        0xfffffe007469ec72,
        0xc5d816ff5069a341269f17e6a14ef1fa,
        Expect::Ok(0xc5d816ff5069a341c40000004ee8d3d9, 0x1fa0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1780,
        5,
        0x7799f38ffffffffe,
        0xe9363d752335d74337fe2510dcb36823,
        Expect::Ok(0xe9363d752335d7434eef33e7c0000000, 0x17a0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x2f80,
        4,
        0xae57ddf4ffffffff,
        0x6213a03c1193bdf639f36310401b85ac,
        Expect::Fault(0x2fa0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x5780,
        0,
        0x1fffffc01,
        0xf2a619162eb5e0b1eaeb4d58ca0f7136,
        Expect::Ok(0xf2a619162eb5e0b13f800000c47fc000, 0x5780),
    ),
    (
        Mmx::Cvtpi2ps,
        0x6f80,
        4,
        0xedcba98800000000,
        0xfcbc67b505d8de820914fd84e8e1bb1d,
        Expect::Fault(0x6fa0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x9780,
        0,
        0xfffffffffffffffe,
        0xa8db8be823a307e2e4811e5e35f37785,
        Expect::Ok(0xa8db8be823a307e2bf800000c0000000, 0x9780),
    ),
    (
        Mmx::Cvtpi2ps,
        0x7f40,
        2,
        0x100000000,
        0xd878ac48135d59b62740fc9f5286e0c9,
        Expect::Ok(0xd878ac48135d59b63f80000000000000, 0x7f40),
    ),
    (
        Mmx::Cvtpi2ps,
        0x7ec0,
        4,
        0x1000001fffffffe,
        0x2002b9462fadebec94c0f621753fb351,
        Expect::Ok(0x2002b9462fadebec4b800000c0000000, 0x7ee0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1fc0,
        0,
        0x8000000000000001,
        0x7b55b7b44bb20a0bc1904d3ea1f321ff,
        Expect::Ok(0x7b55b7b44bb20a0bcf0000003f800000, 0x1fc0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x6fc0,
        3,
        0x115b6cb17,
        0xdaa57fc97f14ab8db63ec62d333467a5,
        Expect::Fault(0x6fe0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x40,
        5,
        0xffffffff00000000,
        0x714e90cabaff2609efd937dee299c8d2,
        Expect::Ok(0x714e90cabaff2609bf80000000000000, 0x40),
    ),
    (
        Mmx::Cvtpi2ps,
        0x80c0,
        1,
        0x64922d8ea02221f6,
        0x60f3ea5410fb2eff296ff77c54d3c4ea,
        Expect::Fault(0x80e0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x80c0,
        5,
        0x1ffffffff,
        0x854a871ed2cd6af5b391311bfc9e5da1,
        Expect::Ok(0x854a871ed2cd6af53f800000bf800000, 0x80c0),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1fbf,
        2,
        0x93aae5a7bd99588,
        0x72d0d573113aaf37509a0a4bd514ddfd,
        Expect::Ok(0x72d0d573113aaf374d13aae64ef7b32b, 0x1fbf),
    ),
    (
        Mmx::Cvtpi2ps,
        0x1f95,
        5,
        0xfffffdff88441960,
        0xa45a2c4e5c11907712b3fd9615a5f0ad,
        Expect::Ok(0xa45a2c4e5c119077c4004000ceef77cd, 0x1fb5),
    ),
    (
        Mmx::Cvtpi2ps,
        0xfbf,
        3,
        0xffffffff713476f2,
        0x60d88b3a39bab4c76e460fdf93975e90,
        Expect::Fault(0xfbf),
    ),
    (
        Mmx::Cvtpi2ps,
        0x380,
        0,
        0xfffffdff01ffffff,
        0x7af5159a2d8df1ccdb756337d9cc7f6d,
        Expect::Fault(0x3a0),
    ),
    (
        Mmx::Cvtps2pi,
        0x1f80,
        3,
        0x501409c0f3716efdcf0000013fc00000,
        0x0,
        Expect::Ok(0x8000000000000002, 0x1fa1),
    ),
    (
        Mmx::Cvtps2pi,
        0x1b80,
        6,
        0x58c0c8f4c8f88bd8ffbfffff4b000000,
        0x0,
        Expect::Ok(0x8000000000800000, 0x1b81),
    ),
    (
        Mmx::Cvtps2pi,
        0x1780,
        5,
        0x1b5832b0d3405e77007fffff00800001,
        0x0,
        Expect::Ok(0x0, 0x17a0),
    ),
    (
        Mmx::Cvtps2pi,
        0xf80,
        5,
        0xb175928dcc09c41f7effffff00000003,
        0x0,
        Expect::Fault(0xfa1),
    ),
    (
        Mmx::Cvtps2pi,
        0x3d80,
        6,
        0x131e55b1e56713e4b7fffff4b800001,
        0x0,
        Expect::Ok(0xffffff01000002, 0x3d80),
    ),
    (
        Mmx::Cvtps2pi,
        0x3b80,
        0,
        0x801cacbfca8f53fb3e800000c009c384,
        0x0,
        Expect::Ok(0xfffffffd, 0x3ba0),
    ),
    (
        Mmx::Cvtps2pi,
        0x3f80,
        7,
        0x37c45064375a25b2400000007e800000,
        0x0,
        Expect::Ok(0x280000000, 0x3f81),
    ),
    (
        Mmx::Cvtps2pi,
        0x3f80,
        3,
        0xac2391ef8e695a42ff90cc8e80000001,
        0x0,
        Expect::Ok(0x80000000ffffffff, 0x3fa1),
    ),
    (
        Mmx::Cvtps2pi,
        0x5e80,
        2,
        0x82f88c1a8dddfcea40400000bf800000,
        0x0,
        Expect::Ok(0x3ffffffff, 0x5e80),
    ),
    (
        Mmx::Cvtps2pi,
        0x5d80,
        7,
        0x2000159ae770c32c00ad48cfffc00000,
        0x0,
        Expect::Ok(0x180000000, 0x5da1),
    ),
    (
        Mmx::Cvtps2pi,
        0x7e80,
        6,
        0xbaff9a3af92c32b04f0000004b7fffff,
        0x0,
        Expect::Ok(0x8000000000ffffff, 0x7e81),
    ),
    (
        Mmx::Cvtps2pi,
        0x5f80,
        6,
        0x18f506b80d0e0df70000000374fda4aa,
        0x0,
        Expect::Ok(0x180000000, 0x5fa1),
    ),
    (
        Mmx::Cvtps2pi,
        0x6f80,
        5,
        0x26234fec648bf82c7fa00000bf7fffff,
        0x0,
        Expect::Fault(0x6fa1),
    ),
    (
        Mmx::Cvtps2pi,
        0x9f00,
        5,
        0xccf66d3d5b0ebd025f0000017fc00000,
        0x0,
        Expect::Fault(0x9f01),
    ),
    (
        Mmx::Cvtps2pi,
        0x1f40,
        2,
        0x2445470f9800e95f7effffff80d4cfe5,
        0x0,
        Expect::Fault(0x1f41),
    ),
    (
        Mmx::Cvtps2pi,
        0x9d80,
        0,
        0xc29ed8a9e7f2231d7fa0000074ca9c46,
        0x0,
        Expect::Ok(0x8000000080000000, 0x9d81),
    ),
    (
        Mmx::Cvtps2pi,
        0x7f80,
        6,
        0x2aae6a788c0170bb4b0000007fffffff,
        0x0,
        Expect::Ok(0x80000080000000, 0x7f81),
    ),
    (
        Mmx::Cvtps2pi,
        0x7f80,
        4,
        0x78b772e72443d149bf00000000400000,
        0x0,
        Expect::Ok(0x0, 0x7fa0),
    ),
    (
        Mmx::Cvtps2pi,
        0x8f80,
        1,
        0xf8def5b5ec7329b93ff264b6beffffff,
        0x0,
        Expect::Fault(0x8fa0),
    ),
    (
        Mmx::Cvtps2pi,
        0x8f80,
        4,
        0x9921996b5d668b0800800000cfd10d99,
        0x0,
        Expect::Fault(0x8fa1),
    ),
    (
        Mmx::Cvtps2pi,
        0xff00,
        0,
        0x56303ce6207088fa5effffff40200000,
        0x0,
        Expect::Fault(0xff01),
    ),
    (
        Mmx::Cvtps2pi,
        0x7f40,
        3,
        0x81468eb5bb226174ff800001cf000001,
        0x0,
        Expect::Fault(0x7f41),
    ),
    (
        Mmx::Cvtps2pi,
        0x7ec0,
        0,
        0xaed374aeb45c22eb8000000180000000,
        0x0,
        Expect::Ok(0x0, 0x7ec0),
    ),
    (
        Mmx::Cvtps2pi,
        0xfd80,
        7,
        0x78d9dcdba06ac3bd7f7fffff807fffff,
        0x0,
        Expect::Ok(0x8000000000000000, 0xfda1),
    ),
    (
        Mmx::Cvtps2pi,
        0x1fc0,
        0,
        0xe611e7722c55daeaff80000040200000,
        0x0,
        Expect::Ok(0x8000000000000002, 0x1fe1),
    ),
    (
        Mmx::Cvtps2pi,
        0x1fc0,
        5,
        0x5f8f3428b49b3edb807fffff4efffffe,
        0x0,
        Expect::Ok(0x7fffff00, 0x1fc0),
    ),
    (
        Mmx::Cvtps2pi,
        0xfb80,
        1,
        0xecc301e01858adf88000000000d89ffb,
        0x0,
        Expect::Ok(0x0, 0xfba0),
    ),
    (
        Mmx::Cvtps2pi,
        0xf780,
        5,
        0x3dda88ba078d9eefcefffffe3f800000,
        0x0,
        Expect::Ok(0x8000010000000001, 0xf780),
    ),
    (
        Mmx::Cvtps2pi,
        0x4000,
        2,
        0x929749ba3ccfa4afbfcc8b3fc0000000,
        0x0,
        Expect::Fault(0x4020),
    ),
    (
        Mmx::Cvtps2pi,
        0x6000,
        3,
        0x6447de038a4399b1c03d429d3f800001,
        0x0,
        Expect::Fault(0x6020),
    ),
    (
        Mmx::Cvtps2pi,
        0x9f80,
        0,
        0xbba2fd4856306a5c0000000cefffffe,
        0x0,
        Expect::Ok(0xfffffffe80000100, 0x9f80),
    ),
    (
        Mmx::Cvtps2pi,
        0x80c0,
        4,
        0x35ab2a7a0c0e2adf3effffff7fffffff,
        0x0,
        Expect::Fault(0x80e1),
    ),
    (
        Mmx::Cvtps2pi,
        0x80c0,
        1,
        0x512ebe333408e95f0059740f00800000,
        0x0,
        Expect::Fault(0x80e0),
    ),
    (
        Mmx::Cvtps2pi,
        0x1f95,
        6,
        0x5883688b2eb37223ff800001816bdbba,
        0x0,
        Expect::Ok(0x8000000000000000, 0x1fb5),
    ),
    (
        Mmx::Cvtps2pi,
        0x5faa,
        3,
        0xb8702d1f74a7381e3fc00000ff19a783,
        0x0,
        Expect::Ok(0x280000000, 0x5fab),
    ),
    (
        Mmx::Cvtps2pi,
        0x5faa,
        2,
        0xfcd6f2be001c82fcc0200000c034adcc,
        0x0,
        Expect::Ok(0xfffffffefffffffe, 0x5faa),
    ),
    (
        Mmx::Cvtps2pi,
        0xfbf,
        0,
        0x3d31d44ee62e46e0807fffffc0200000,
        0x0,
        Expect::Fault(0xfbf),
    ),
    (
        Mmx::Cvtps2pi,
        0xffc0,
        2,
        0xccb57d1bcfbbf69ec02000007f800001,
        0x0,
        Expect::Ok(0xfffffffe80000000, 0xffe1),
    ),
    (
        Mmx::Cvtps2pi,
        0x1a00,
        7,
        0x8551b6354fbf4b0c000000040000000,
        0x0,
        Expect::Ok(0xfffffffe00000002, 0x1a00),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1f80,
        0,
        0x41e00000000000013ff0000000000000,
        0x0,
        Expect::Ok(0x8000000000000001, 0x1f81),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1f80,
        7,
        0xbca5f0ee84a8ce137fd0000000000000,
        0x0,
        Expect::Ok(0x80000000, 0x1fa1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1b80,
        3,
        0x3ca00000000000000008f83c82f93859,
        0x0,
        Expect::Ok(0x0, 0x1ba0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1780,
        1,
        0x3ff0000000000000ffd46b8a818ce0d6,
        0x0,
        Expect::Ok(0x180000000, 0x1781),
    ),
    (
        Mmx::Cvttpd2pi,
        0xf80,
        0,
        0xfffb4dd0a0b27d4dbffb4e36f8d008c8,
        0x0,
        Expect::Fault(0xfa1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x3f00,
        2,
        0x41dfffffffc000000000000000000000,
        0x0,
        Expect::Ok(0x7fffffff00000000, 0x3f00),
    ),
    (
        Mmx::Cvttpd2pi,
        0x3d80,
        5,
        0x3ff80000000000003f8b2891e89deb76,
        0x0,
        Expect::Ok(0x100000000, 0x3da0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x2f80,
        4,
        0x40080000000000008010000000000000,
        0x0,
        Expect::Fault(0x2fa0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x2f80,
        0,
        0x43e00000000000003fe0000000000000,
        0x0,
        Expect::Fault(0x2fa1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x3f80,
        7,
        0x7fff4ec98d7c2a3ac3dfffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0x3f81),
    ),
    (
        Mmx::Cvttpd2pi,
        0x3f80,
        6,
        0x41e0000000000001bca3a2b0b6e23c0d,
        0x0,
        Expect::Ok(0x8000000000000000, 0x3fa1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x3f80,
        5,
        0x34000000000000000,
        0x0,
        Expect::Ok(0x2, 0x3fa0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5d80,
        0,
        0x1433fffffffffffff,
        0x0,
        Expect::Ok(0x80000000, 0x5da1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5b80,
        5,
        0x4014000000000000c000000000000000,
        0x0,
        Expect::Ok(0x5fffffffe, 0x5b80),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5780,
        6,
        0x7fdfffffffffffff7fefffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0x5781),
    ),
    (
        Mmx::Cvttpd2pi,
        0x4f80,
        4,
        0x7ff40000000000003fd0000000000000,
        0x0,
        Expect::Fault(0x4fa1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x7f00,
        3,
        0x41dfffffffc000004008000000000000,
        0x0,
        Expect::Ok(0x7fffffff00000003, 0x7f00),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5f80,
        2,
        0xc0000000000000004008000000000000,
        0x0,
        Expect::Ok(0xfffffffe00000003, 0x5f80),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5f80,
        0,
        0xbcb507c6afe6d666802c8888689f1114,
        0x0,
        Expect::Ok(0x0, 0x5fa0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x5f80,
        0,
        0x43300000000000007fd0000000000000,
        0x0,
        Expect::Ok(0x8000000080000000, 0x5f81),
    ),
    (
        Mmx::Cvttpd2pi,
        0x9e80,
        6,
        0x40140000000000004000000000000000,
        0x0,
        Expect::Ok(0x500000002, 0x9e80),
    ),
    (
        Mmx::Cvttpd2pi,
        0x9b80,
        5,
        0x7fdfffffffffffff,
        0x0,
        Expect::Ok(0x80000000, 0x9b81),
    ),
    (
        Mmx::Cvttpd2pi,
        0x9780,
        3,
        0x1000000000000100082e1695b2a973,
        0x0,
        Expect::Ok(0x0, 0x97a0),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfc0,
        5,
        0x216b5fb72c216eb43e0000000000001,
        0x0,
        Expect::Fault(0xfe1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfc0,
        5,
        0xc3e000000000000141e0000000000001,
        0x0,
        Expect::Ok(0x8000000080000000, 0xfc1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfe80,
        1,
        0xc0000000000000003ff0000000000000,
        0x0,
        Expect::Ok(0xfffffffe00000001, 0xfe80),
    ),
    (
        Mmx::Cvttpd2pi,
        0x7ec0,
        4,
        0x7fdcf8dca7fc85923fdfffffffffffff,
        0x0,
        Expect::Ok(0x8000000000000000, 0x7ee1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfd80,
        0,
        0xbcb06d3f7793a72e43e0000000000000,
        0x0,
        Expect::Ok(0x80000000, 0xfda1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfd80,
        7,
        0xbffe5fdd5ec8a1e140008d3c8191b538,
        0x0,
        Expect::Ok(0xffffffff00000002, 0xfda0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1fc0,
        3,
        0x282bcd9cd5a5ae3ff8000000000000,
        0x0,
        Expect::Ok(0x1, 0x1fe0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1fc0,
        3,
        0xb5bd22b8306b4800fffffffffffff,
        0x0,
        Expect::Ok(0x0, 0x1fc0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x6fc0,
        7,
        0x41dfffffffc000008004b48d14b70aff,
        0x0,
        Expect::Ok(0x7fffffff00000000, 0x6fc0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x6fc0,
        2,
        0xbfdfffffffffffffc1e0000000000000,
        0x0,
        Expect::Fault(0x6fe0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x4000,
        6,
        0x800e0e47ab7b9c783fff44f35368bd9d,
        0x0,
        Expect::Fault(0x4020),
    ),
    (
        Mmx::Cvttpd2pi,
        0x9f80,
        2,
        0xfff8123456789abcfff0000000000001,
        0x0,
        Expect::Ok(0x8000000080000000, 0x9f81),
    ),
    (
        Mmx::Cvttpd2pi,
        0x40,
        7,
        0x7fda4ba8c7f250f900222a84af55c036,
        0x0,
        Expect::Fault(0x41),
    ),
    (
        Mmx::Cvttpd2pi,
        0x80c0,
        1,
        0x3ff8000000000000668bedfa87a031dc,
        0x0,
        Expect::Fault(0x80e1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x80c0,
        2,
        0x433fffffffffffffffefffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0x80c1),
    ),
    (
        Mmx::Cvttpd2pi,
        0x80c0,
        6,
        0xf45661b79924b4008000000000000,
        0x0,
        Expect::Ok(0x3, 0x80c0),
    ),
    (
        Mmx::Cvttpd2pi,
        0xfbf,
        0,
        0x7fe10000000000007fd0000000000000,
        0x0,
        Expect::Ok(0x8000000080000000, 0xfbf),
    ),
    (
        Mmx::Cvttpd2pi,
        0xffc0,
        5,
        0x7fffffffffffffff7fdfffffffffffff,
        0x0,
        Expect::Ok(0x8000000080000000, 0xffc1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xffc0,
        0,
        0xbfe0000000000000c3dfffffffffffff,
        0x0,
        Expect::Ok(0x80000000, 0xffe1),
    ),
    (
        Mmx::Cvttpd2pi,
        0xffc0,
        0,
        0x41dfffffffc000003cb67ce1c7a18581,
        0x0,
        Expect::Ok(0x7fffffff00000000, 0xffe0),
    ),
    (
        Mmx::Cvttpd2pi,
        0x1f00,
        7,
        0x7ffbd0c8cec1047841e0000000000000,
        0x0,
        Expect::Fault(0x1f01),
    ),
    (
        Mmx::Cvttpd2pi,
        0x380,
        4,
        0xbcb8859d1b39aba03ca0000000000000,
        0x0,
        Expect::Fault(0x3a0),
    ),
    (
        Mmx::Cvttps2pi,
        0x2f80,
        4,
        0xa13498af169b71ab008000017f000000,
        0x0,
        Expect::Fault(0x2fa1),
    ),
    (
        Mmx::Cvttps2pi,
        0x2f80,
        0,
        0x1b2cedca2fd0f93d7fa00000bf800000,
        0x0,
        Expect::Ok(0x80000000ffffffff, 0x2f81),
    ),
    (
        Mmx::Cvttps2pi,
        0x2f80,
        0,
        0xf9097800ad38f70dcf00000000000003,
        0x0,
        Expect::Fault(0x2fa0),
    ),
    (
        Mmx::Cvttps2pi,
        0x3f80,
        0,
        0xdb14dc0e2e59c3be0b14d04500800000,
        0x0,
        Expect::Ok(0x0, 0x3fa0),
    ),
    (
        Mmx::Cvttps2pi,
        0x3f80,
        7,
        0xa1a9a8311c3d3e174b7fffffff3c40a4,
        0x0,
        Expect::Ok(0xffffff80000000, 0x3f81),
    ),
    (
        Mmx::Cvttps2pi,
        0x5b80,
        2,
        0x64c1867062e7a43fb3dfaa00462ddf,
        0x0,
        Expect::Ok(0x100000000, 0x5ba0),
    ),
    (
        Mmx::Cvttps2pi,
        0x5b80,
        2,
        0xb3dff949dd756b0f4b8000014efffffe,
        0x0,
        Expect::Ok(0x10000027fffff00, 0x5b80),
    ),
    (
        Mmx::Cvttps2pi,
        0x7e80,
        3,
        0xc14af41bd1e70a57f000000ff800000,
        0x0,
        Expect::Ok(0x8000000080000000, 0x7e81),
    ),
    (
        Mmx::Cvttps2pi,
        0x7d80,
        5,
        0xe4df88944dfdd9c0007fffffbfc00000,
        0x0,
        Expect::Ok(0xffffffff, 0x7da0),
    ),
    (
        Mmx::Cvttps2pi,
        0x5f80,
        1,
        0xdf734b71377a9e217fa0000000000003,
        0x0,
        Expect::Ok(0x8000000000000000, 0x5fa1),
    ),
    (
        Mmx::Cvttps2pi,
        0x5f80,
        3,
        0x8bb39ca14a7955a07e8000004f853391,
        0x0,
        Expect::Ok(0x8000000080000000, 0x5f81),
    ),
    (
        Mmx::Cvttps2pi,
        0x5f80,
        4,
        0xf9b3d6081f1f61183f00000040000000,
        0x0,
        Expect::Ok(0x2, 0x5fa0),
    ),
    (
        Mmx::Cvttps2pi,
        0x6f80,
        2,
        0xf6489fc5edcd2ef13f800001bf000000,
        0x0,
        Expect::Fault(0x6fa0),
    ),
    (
        Mmx::Cvttps2pi,
        0x6f80,
        7,
        0xc390ba2f877b762500800001df000000,
        0x0,
        Expect::Fault(0x6fa1),
    ),
    (
        Mmx::Cvttps2pi,
        0x9d80,
        4,
        0xa8dcbe6669a32bf1df0000014f000000,
        0x0,
        Expect::Ok(0x8000000080000000, 0x9d81),
    ),
    (
        Mmx::Cvttps2pi,
        0x1bc0,
        7,
        0x4b82a4eb521fbc7b010000007f800001,
        0x0,
        Expect::Ok(0x80000000, 0x1be1),
    ),
    (
        Mmx::Cvttps2pi,
        0x1bc0,
        0,
        0xf96c33b6cbce92b5004000007fbfffff,
        0x0,
        Expect::Ok(0x80000000, 0x1bc1),
    ),
    (
        Mmx::Cvttps2pi,
        0x7f80,
        2,
        0xb3a58e802193a39bbeffffff40200000,
        0x0,
        Expect::Ok(0x2, 0x7fa0),
    ),
    (
        Mmx::Cvttps2pi,
        0x7f80,
        3,
        0xbd3a930800587d583f800000c0000000,
        0x0,
        Expect::Ok(0x1fffffffe, 0x7f80),
    ),
    (
        Mmx::Cvttps2pi,
        0x7f80,
        6,
        0x6cfdad6524f02b807fc000004b800001,
        0x0,
        Expect::Ok(0x8000000001000002, 0x7f81),
    ),
    (
        Mmx::Cvttps2pi,
        0xfc0,
        3,
        0x5e5ee41bab1f2239808055924f000001,
        0x0,
        Expect::Fault(0xfe1),
    ),
    (
        Mmx::Cvttps2pi,
        0xff00,
        4,
        0x2680207be7389d3f4b8000017fffffff,
        0x0,
        Expect::Fault(0xff01),
    ),
    (
        Mmx::Cvttps2pi,
        0x7f40,
        7,
        0x358b865257b0087fff800001ffc12345,
        0x0,
        Expect::Fault(0x7f41),
    ),
    (
        Mmx::Cvttps2pi,
        0x7dc0,
        1,
        0x8d973017c8d4acfc003934fc800f2c18,
        0x0,
        Expect::Ok(0x0, 0x7dc0),
    ),
    (
        Mmx::Cvttps2pi,
        0x7dc0,
        2,
        0xacc3ec97e36d3ce7effffff40a00000,
        0x0,
        Expect::Ok(0x8000000000000005, 0x7dc1),
    ),
    (
        Mmx::Cvttps2pi,
        0x1fc0,
        4,
        0x86576a2e1408ffdd017ee64f00400000,
        0x0,
        Expect::Ok(0x0, 0x1fe0),
    ),
    (
        Mmx::Cvttps2pi,
        0xf780,
        5,
        0xf224412b09be60f1f4c869e0804dda38,
        0x0,
        Expect::Ok(0x8000000000000000, 0xf7a1),
    ),
    (
        Mmx::Cvttps2pi,
        0x6fc0,
        7,
        0xe24ee1c53fbb60a10064678180800000,
        0x0,
        Expect::Fault(0x6fe0),
    ),
    (
        Mmx::Cvttps2pi,
        0x2000,
        4,
        0xd40846fb2ff87081ffc12345cf000001,
        0x0,
        Expect::Fault(0x2001),
    ),
    (
        Mmx::Cvttps2pi,
        0x2000,
        4,
        0xb7b4fac054d9080f4b8000004b000000,
        0x0,
        Expect::Ok(0x100000000800000, 0x2000),
    ),
    (
        Mmx::Cvttps2pi,
        0x9f80,
        2,
        0x464a8caf2b733043800000004b800001,
        0x0,
        Expect::Ok(0x1000002, 0x9f80),
    ),
    (
        Mmx::Cvttps2pi,
        0x9f80,
        5,
        0xc9515139ddc1f1d87e8000004b7fffff,
        0x0,
        Expect::Ok(0x8000000000ffffff, 0x9f81),
    ),
    (
        Mmx::Cvttps2pi,
        0x9f80,
        5,
        0x6508b28de1386fe97fbfffff0134809d,
        0x0,
        Expect::Ok(0x8000000000000000, 0x9fa1),
    ),
    (
        Mmx::Cvttps2pi,
        0x40,
        4,
        0x386da5f59655b44e00000001c0200000,
        0x0,
        Expect::Fault(0x60),
    ),
    (
        Mmx::Cvttps2pi,
        0x80c0,
        1,
        0xfd5cd59a833da578ff7fffff3f000000,
        0x0,
        Expect::Fault(0x80e1),
    ),
    (
        Mmx::Cvttps2pi,
        0x80c0,
        1,
        0x816461b2be0a928f00000000ffbfffff,
        0x0,
        Expect::Ok(0x80000000, 0x80c1),
    ),
    (
        Mmx::Cvttps2pi,
        0x5faa,
        6,
        0x7c0fc0e7f70772d540000000cb000001,
        0x0,
        Expect::Ok(0x2ff7fffff, 0x5faa),
    ),
    (
        Mmx::Cvttps2pi,
        0xfbf,
        4,
        0xa3cfe4989d0a8e24ff8eb76200ffffff,
        0x0,
        Expect::Fault(0xfbf),
    ),
    (
        Mmx::Cvttps2pi,
        0xffc0,
        7,
        0x15da6ceb47bb53197fbfffffa329733f,
        0x0,
        Expect::Ok(0x8000000000000000, 0xffe1),
    ),
    (
        Mmx::Cvttps2pi,
        0xffc0,
        4,
        0xd29998456813a689cefffffe004d1f0f,
        0x0,
        Expect::Ok(0x8000010000000000, 0xffc0),
    ),
    (
        Mmx::Cvttps2pi,
        0xffc0,
        7,
        0xcb7a867f51ce2dbb4b7fffff8b06b265,
        0x0,
        Expect::Ok(0xffffff00000000, 0xffe0),
    ),
    (
        Mmx::Cvttps2pi,
        0x1f00,
        3,
        0xae899285b7c94935bfc00000c0200000,
        0x0,
        Expect::Ok(0xfffffffffffffffe, 0x1f20),
    ),
    (
        Mmx::Cvttps2pi,
        0x380,
        6,
        0xd7cb7e6aecfe419340400000e6048dc6,
        0x0,
        Expect::Ok(0x380000000, 0x381),
    ),
    (
        Mmx::Cvttps2pi,
        0x1a00,
        5,
        0x5423bb9066ff551648bdc1ac7f800001,
        0x0,
        Expect::Fault(0x1a01),
    ),
    (
        Mmx::Cvttps2pi,
        0x1f00,
        6,
        0x4704c6266f478089cefffffe4efffffe,
        0x0,
        Expect::Ok(0x800001007fffff00, 0x1f00),
    ),
    (
        Mmx::Cvttps2pi,
        0x1e80,
        5,
        0x4ec51219ca135f930b78d7ccffbfffff,
        0x0,
        Expect::Ok(0x80000000, 0x1ea1),
    ),
];
