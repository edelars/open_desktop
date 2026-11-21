//! S2-37 (remainder of the general SSE floating-point core): CVTPS2PD (0F 5A), CVTPD2PS (66 0F 5A), CVTSS2SD
//! (F3 0F 5A), CVTSD2SS (F2 0F 5A), RCPPS (0F 53), RCPSS (F3 0F 53), RSQRTPS (0F 52) and RSQRTSS (F3 0F 52), with any
//! XMM or memory source.
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A CVTPD2PS pp. 3-219..3-222, CVTPS2PD pp. 3-228..3-230, CVTSD2SS pp. 3-234..3-235, CVTSS2SD pp. 3-240..3-241
//! (SIMD exceptions: CVTPS2PD and CVTSS2SD invalid and denormal; CVTPD2PS and CVTSD2SS invalid, precision, underflow,
//! overflow, denormal; CVTPD2PS zeroes the high quadword; CVTSS2SD and CVTSD2SS keep the bits above the element);
//! Vol. 2B RCPPS pp. 4-538..4-539, RCPSS pp. 4-540..4-541, RSQRTPS pp. 4-594..4-595, RSQRTSS pp. 4-596..4-597
//! (relative error at most 1.5 * 2^-12, not affected by MXCSR.RC, a denormal source is a zero of the same sign, tiny
//! results are flushed to zero, an SNaN becomes a QNaN, no SIMD floating-point exceptions); exception classes Vol. 2A
//! Table 2-19 (Type 2: CVTPD2PS needs a 16-byte aligned memory operand), Table 2-20 (Type 3: CVTPS2PD m64, CVTSS2SD m32,
//! CVTSD2SS m64), Table 2-21 (Type 4: RCPPS and RSQRTPS, aligned) and Table 2-22 (Type 5: RCPSS and RSQRTSS); Vol. 1
//! sections 4.8.3.5 (SSE NaN rules), 4.9.1/4.9.2, 10.2.3 and 11.5.2/11.5.3 (MXCSR, SIMD floating-point exceptions).
//!
//! Where the SDM text does not fix an observable detail, the expected value is the one the physical x86 oracle gave
//! (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; MXCSR loaded with LDMXCSR per case). The table GOLDEN below is hardware
//! results (selected from 89 988 cases in work/stage8/s2_37_cvt_oracle/: gen_cases.py, oracle.c, pick_golden.py), not model
//! output. For CVTPD2PS/CVTSD2SS the hardware also settles what the SDM leaves open: a quiet NaN keeps the top 23 bits of its
//! payload, a denormal source is a denormal operand (not under DAZ), and an unmasked overflow reports PE only when rounding
//! with an unbounded exponent is inexact. RCPPS/RCPSS/RSQRTPS/RSQRTSS values are approximations: the model returns the
//! deterministic reference of binary_float (ISSUE-025), which differs from the hardware table in the low bits, so the
//! GOLDEN rows of these forms are the ones where hardware and reference coincide (specials, signs, NaNs, zeros, infinities,
//! subnormals) and the normal values are checked against the SDM error bound only.
//!
//! Not asserted here: the CPUID gate (S2-57), the alignment-check exception of the scalar forms (an unaligned scalar
//! operand at CPL3 with RFLAGS.AC = 1 is Unsupported), FS/GS overrides and the prefix sequences the hardware tolerates
//! (Unsupported), the JIT (refused, see od-jit), CPL3 (refused, see cpl3_profile_set).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, DecodeError, Operation, Sse3Source, SseFpMiscKind, SseFpShape};
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
/// All six exceptions masked, round to nearest, no DAZ, no FTZ, no flag set.
const MXCSR: u32 = 0x1f80;
const IE: u32 = 1;
const DE: u32 = 2;
const OE: u32 = 8;
const UE: u32 = 0x10;
const PE: u32 = 0x20;
const DAZ: u32 = 0x40;
const FTZ: u32 = 0x8000;
const RC_DOWN: u32 = 1 << 13;
const RC_UP: u32 = 2 << 13;
const RC_ZERO: u32 = 3 << 13;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;
const CR4_ALL: u64 = CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT;

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

/// A vector of four binary32 lanes, lane 0 lowest.
fn s4(lanes: [u32; 4]) -> u128 {
    lanes
        .iter()
        .enumerate()
        .fold(0, |acc, (i, lane)| acc | u128::from(*lane) << (32 * i))
}

fn d2(lanes: [u64; 2]) -> u128 {
    u128::from(lanes[0]) | u128::from(lanes[1]) << 64
}

// ---- hardware vectors ----

enum Expect {
    /// The destination register afterwards and MXCSR.
    Ok(u128, u32),
    /// A SIMD floating-point exception (#XM) with the MXCSR the fault leaves behind.
    Fault(u32),
}

/// One hardware vector: name, instruction bytes (register form), MXCSR before, (register, value) inputs, outcome.
type Row = (
    &'static str,
    &'static [u8],
    u32,
    &'static [(u8, u128)],
    Expect,
);

/// The destination register of a register-form instruction: the ModRM.reg field plus REX.R.
fn destination_of(code: &[u8]) -> usize {
    let mut at = 0;
    let mut rex = 0;
    while matches!(code[at], 0x66 | 0xf2 | 0xf3 | 0x40..=0x4f) {
        if code[at] & 0xf0 == 0x40 {
            rex = code[at];
        }
        at += 1;
    }
    usize::from(code[at + 2] >> 3 & 7) | usize::from(rex & 4) << 1
}

fn run_golden(row: &Row) {
    let (name, code, mxcsr, inputs, expect) = row;
    let d = destination_of(code);
    let build = |cr0: Option<u64>| {
        let mut cpu = match cr0 {
            Some(cr0) => fp_init_cpu(cr0, CR4_ALL),
            None => cpu(),
        };
        cpu.sse.as_mut().unwrap().mxcsr = *mxcsr;
        for (index, value) in *inputs {
            set_xmm(&mut cpu, usize::from(*index), *value);
        }
        cpu
    };
    let expected_of = |before: &LongModeState| {
        let mut expected = before.clone();
        match expect {
            Expect::Ok(value, mx) => {
                set_xmm(&mut expected, d, *value);
                expected.sse.as_mut().unwrap().mxcsr = *mx;
                expected.rip += code.len() as u64;
            }
            Expect::Fault(mx) => expected.sse.as_mut().unwrap().mxcsr = *mx,
        }
        expected
    };
    // The FP-init profile pair with CR4.OSXMMEXCPT = 1 also runs the faulting vectors (#XM).
    let before = build(Some(CR0_BASE));
    let mut ram = Ram::new(code);
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
            step(&mut bare, &mut Ram::new(code)),
            Ok(StepOutcome::Advanced),
            "{name} (bare)"
        );
        assert_eq!(bare, expected_of(&before), "{name} (bare)");
    }
}

#[test]
fn hardware_vectors_register_forms() {
    assert!(GOLDEN.len() > 600);
    for row in GOLDEN {
        run_golden(row);
    }
}

// ---- encodings ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Cvtps2pd,
    Cvtpd2ps,
    Cvtss2sd,
    Cvtsd2ss,
    Rcpps,
    Rcpss,
    Rsqrtps,
    Rsqrtss,
}

const ALL: [Op; 8] = [
    Op::Cvtps2pd,
    Op::Cvtpd2ps,
    Op::Cvtss2sd,
    Op::Cvtsd2ss,
    Op::Rcpps,
    Op::Rcpss,
    Op::Rsqrtps,
    Op::Rsqrtss,
];

fn prefix(op: Op) -> &'static [u8] {
    match op {
        Op::Cvtpd2ps => &[0x66],
        Op::Cvtss2sd | Op::Rcpss | Op::Rsqrtss => &[0xf3],
        Op::Cvtsd2ss => &[0xf2],
        Op::Cvtps2pd | Op::Rcpps | Op::Rsqrtps => &[],
    }
}

fn opcode(op: Op) -> u8 {
    match op {
        Op::Cvtps2pd | Op::Cvtpd2ps | Op::Cvtss2sd | Op::Cvtsd2ss => 0x5a,
        Op::Rcpps | Op::Rcpss => 0x53,
        Op::Rsqrtps | Op::Rsqrtss => 0x52,
    }
}

/// The memory source: its size in bytes and whether it must be 16-byte aligned (Types 2 and 4: #GP(0)).
fn memory_of(op: Op) -> (usize, bool) {
    match op {
        Op::Cvtps2pd | Op::Cvtsd2ss => (8, false),
        Op::Cvtss2sd | Op::Rcpss | Op::Rsqrtss => (4, false),
        Op::Cvtpd2ps | Op::Rcpps | Op::Rsqrtps => (16, true),
    }
}

/// A source vector that gives a defined, exactly representable result for every instruction.
fn test_value(op: Op) -> u128 {
    match op {
        Op::Cvtps2pd => s4([0x4040_0000, 0xbf80_0000, 0, 0]),
        Op::Cvtpd2ps => d2([0x4008_0000_0000_0000, 0xbff0_0000_0000_0000]),
        Op::Cvtss2sd => s4([0x40a0_0000, 0, 0, 0]),
        Op::Cvtsd2ss => d2([0x4010_0000_0000_0000, 0]),
        _ => s4([0x4080_0000, 0x3e80_0000, 0x4120_0000, 0x42c8_0000]),
    }
}

/// [prefix] [REX] 0F op modrm with a register source. REX.W is ignored by the instruction; with it the narrow profile
/// decoder refuses the encoding and the general decoder handles it.
fn reg_code(op: Op, dst: u8, src: u8, rex_w: bool) -> Vec<u8> {
    let mut code = prefix(op).to_vec();
    let rex = 0x40 | u8::from(rex_w) << 3 | u8::from(dst >= 8) << 2 | u8::from(src >= 8);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(op), 0xc0 | (dst & 7) << 3 | (src & 7)]);
    code
}

/// The same instruction with the memory source given by the ModRM, SIB and displacement bytes.
fn mem_code(op: Op, dst: u8, address: &[u8], rex_b: bool) -> Vec<u8> {
    let mut code = prefix(op).to_vec();
    let rex = 0x40 | u8::from(dst >= 8) << 2 | u8::from(rex_b);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend([0x0f, opcode(op)]);
    code.push(address[0] | (dst & 7) << 3);
    code.extend(&address[1..]);
    code
}

struct Obs {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    length: usize,
}

/// One instruction under the FP-init profile pair: xmm1 = a, xmm2 = b, MXCSR as given.
fn run_code(code: &[u8], (a, b): (u128, u128), mx: u32) -> Obs {
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_ALL);
    cpu.sse.as_mut().unwrap().mxcsr = mx;
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

/// Whole-state check of one register-form instruction: either only xmm1, MXCSR and RIP changed (RFLAGS and every
/// other register stay), or #XM with only MXCSR changed.
fn check(op: Op, ab: (u128, u128), mx: u32, expect: Expect, label: &str) {
    let observed = run_code(&reg_code(op, 1, 2, false), ab, mx);
    let mut expected = observed.before.clone();
    match expect {
        Expect::Ok(value, mx_out) => {
            assert_eq!(
                observed.exit,
                RunExit::InstructionBudgetExhausted,
                "{label}: {:?}",
                observed.exit
            );
            set_xmm(&mut expected, 1, value);
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

/// The scalar results keep the destination bits above the element.
fn keep32(low: u32) -> u128 {
    HI & !0xffff_ffff | u128::from(low)
}

fn keep64(low: u64) -> u128 {
    HI & !0xffff_ffff_ffff_ffff | u128::from(low)
}

// ---- CVTPS2PD ----

#[test]
fn cvtps2pd_widens_the_two_low_lanes_exactly() {
    // 1.5, -2.5; the upper two source lanes play no part.
    check(
        Op::Cvtps2pd,
        (HI, s4([0x3fc0_0000, 0xc020_0000, 0x1234_5678, 0x9abc_def0])),
        MXCSR,
        Expect::Ok(d2([0x3ff8_0000_0000_0000, 0xc004_0000_0000_0000]), MXCSR),
        "1.5 and -2.5",
    );
    // Zero, infinity and the largest finite value keep their sign.
    check(
        Op::Cvtps2pd,
        (HI, s4([0x8000_0000, 0x7f80_0000, 0, 0])),
        MXCSR,
        Expect::Ok(d2([0x8000_0000_0000_0000, 0x7ff0_0000_0000_0000]), MXCSR),
        "-0 and +inf",
    );
    check(
        Op::Cvtps2pd,
        (HI, s4([0xff7f_ffff, 0xff80_0000, 0, 0])),
        MXCSR,
        Expect::Ok(d2([0xc7ef_ffff_e000_0000, 0xfff0_0000_0000_0000]), MXCSR),
        "-max and -inf",
    );
}

#[test]
fn cvtps2pd_nans_and_denormals() {
    // A signaling NaN is quieted (bit 51) and invalid; a quiet NaN keeps its payload shifted left by 29.
    check(
        Op::Cvtps2pd,
        (HI, s4([0x7fa0_0000, 0x3f80_0000, 0, 0])),
        MXCSR,
        Expect::Ok(
            d2([0x7ffc_0000_0000_0000, 0x3ff0_0000_0000_0000]),
            MXCSR | IE,
        ),
        "SNaN",
    );
    check(
        Op::Cvtps2pd,
        (HI, s4([0xffc0_0001, 0x7fc0_0001, 0, 0])),
        MXCSR,
        Expect::Ok(d2([0xfff8_0000_2000_0000, 0x7ff8_0000_2000_0000]), MXCSR),
        "QNaN payloads",
    );
    // An unmasked invalid operation: #XM, the destination stays, IE is set.
    check(
        Op::Cvtps2pd,
        (HI, s4([0x7fa0_0000, 0x3f80_0000, 0, 0])),
        0x1f00,
        Expect::Fault(0x1f01),
        "unmasked IE",
    );
    // A denormal source is a denormal operand and widens to a normal binary64; DAZ makes it a zero instead.
    let denormals = s4([0x0000_0001, 0x8040_0000, 0, 0]);
    check(
        Op::Cvtps2pd,
        (HI, denormals),
        MXCSR,
        Expect::Ok(
            d2([0x36a0_0000_0000_0000, 0xb800_0000_0000_0000]),
            MXCSR | DE,
        ),
        "denormals",
    );
    check(
        Op::Cvtps2pd,
        (HI, denormals),
        MXCSR | DAZ,
        Expect::Ok(d2([0, 0x8000_0000_0000_0000]), MXCSR | DAZ),
        "denormals under DAZ",
    );
    check(
        Op::Cvtps2pd,
        (HI, denormals),
        0x1e80,
        Expect::Fault(0x1e80 | DE),
        "unmasked DE",
    );
    // The invalid operation has priority over the denormal operand of the other lane.
    check(
        Op::Cvtps2pd,
        (HI, s4([0x7fa0_0000, 1, 0, 0])),
        0x1f00,
        Expect::Fault(0x1f00 | IE | DE),
        "IE and DE together",
    );
}

// ---- CVTPD2PS ----

#[test]
fn cvtpd2ps_narrows_and_zeroes_the_high_half() {
    check(
        Op::Cvtpd2ps,
        (HI, d2([0x3ff0_0000_0000_0000, 0xc004_0000_0000_0000])),
        MXCSR,
        Expect::Ok(s4([0x3f80_0000, 0xc020_0000, 0, 0]), MXCSR),
        "1.0 and -2.5",
    );
    check(
        Op::Cvtpd2ps,
        (HI, d2([0x7ff0_0000_0000_0000, 0x8000_0000_0000_0000])),
        MXCSR,
        Expect::Ok(s4([0x7f80_0000, 0x8000_0000, 0, 0]), MXCSR),
        "+inf and -0",
    );
}

#[test]
fn cvtpd2ps_rounds_by_rc_and_reports_precision() {
    // 1 + 2^-28 is not a binary32 value: the four modes give 1.0 or the next value up.
    let up = d2([0x3ff0_0000_0100_0000, 0xbff0_0000_0100_0000]);
    for (mx, low, high) in [
        (MXCSR, 0x3f80_0000, 0xbf80_0000),
        (MXCSR | RC_DOWN, 0x3f80_0000, 0xbf80_0001),
        (MXCSR | RC_UP, 0x3f80_0001, 0xbf80_0000),
        (MXCSR | RC_ZERO, 0x3f80_0000, 0xbf80_0000),
    ] {
        check(
            Op::Cvtpd2ps,
            (HI, up),
            mx,
            Expect::Ok(s4([low, high, 0, 0]), mx | PE),
            &format!("rounding {mx:#x}"),
        );
    }
    // Unmasked precision: the instruction faults after the masked response, so only PE is reported.
    check(
        Op::Cvtpd2ps,
        (HI, up),
        0x1f80 & !0x1000,
        Expect::Fault(0x0f80 | PE),
        "unmasked PE",
    );
}

#[test]
fn cvtpd2ps_overflow() {
    let big = d2([0x47f0_0000_0000_0000, 0xc7f0_0000_0000_0000]); // +2^128, -2^128
    for (mx, low, high) in [
        (MXCSR, 0x7f80_0000, 0xff80_0000),
        (MXCSR | RC_DOWN, 0x7f7f_ffff, 0xff80_0000),
        (MXCSR | RC_UP, 0x7f80_0000, 0xff7f_ffff),
        (MXCSR | RC_ZERO, 0x7f7f_ffff, 0xff7f_ffff),
    ] {
        check(
            Op::Cvtpd2ps,
            (HI, big),
            mx,
            Expect::Ok(s4([low, high, 0, 0]), mx | OE | PE),
            &format!("overflow {mx:#x}"),
        );
    }
    // Unmasked overflow: 2^128 is exact with an unbounded exponent, so only OE is reported; a value that is
    // inexact there (2^128 * (1 + 2^-24), a tie that rounds to even) reports PE too (hardware behavior).
    let exact = d2([0x47f0_0000_0000_0000, 0]);
    check(
        Op::Cvtpd2ps,
        (HI, exact),
        0x1b80,
        Expect::Fault(0x1b80 | OE),
        "unmasked OE, exact",
    );
    let inexact = d2([0x47f0_0000_1000_0000, 0]);
    check(
        Op::Cvtpd2ps,
        (HI, inexact),
        0x1b80,
        Expect::Fault(0x1b80 | OE | PE),
        "unmasked OE, inexact",
    );
}

#[test]
fn cvtpd2ps_underflow_ftz_nans_and_denormals() {
    // 2^-127 is a binary32 denormal and exact: no flag when masked; FTZ flushes it to a zero of the same sign
    // with UE and PE.
    let tiny = d2([0x3800_0000_0000_0000, 0xb800_0000_0000_0000]);
    check(
        Op::Cvtpd2ps,
        (HI, tiny),
        MXCSR,
        Expect::Ok(s4([0x0040_0000, 0x8040_0000, 0, 0]), MXCSR),
        "exact denormal result",
    );
    check(
        Op::Cvtpd2ps,
        (HI, tiny),
        MXCSR | FTZ,
        Expect::Ok(s4([0, 0x8000_0000, 0, 0]), MXCSR | FTZ | UE | PE),
        "FTZ",
    );
    // Unmasked underflow is reported for a tiny result even when it is exact.
    check(
        Op::Cvtpd2ps,
        (HI, tiny),
        0x1780,
        Expect::Fault(0x1780 | UE),
        "unmasked UE",
    );
    // 2^-150 is half of the smallest denormal: it rounds to zero (to the smallest denormal upward), UE and PE.
    let half = d2([0x3690_0000_0000_0000, 0]);
    check(
        Op::Cvtpd2ps,
        (HI, half),
        MXCSR,
        Expect::Ok(s4([0, 0, 0, 0]), MXCSR | UE | PE),
        "ties to even to zero",
    );
    check(
        Op::Cvtpd2ps,
        (HI, half),
        MXCSR | RC_UP,
        Expect::Ok(s4([1, 0, 0, 0]), MXCSR | RC_UP | UE | PE),
        "rounded up to the smallest denormal",
    );
    // NaNs: a quiet NaN keeps the top 23 payload bits, a signaling NaN is quieted and invalid.
    check(
        Op::Cvtpd2ps,
        (HI, d2([0xfff8_0000_2000_0000, 0x7ff0_0000_0000_0001])),
        MXCSR,
        Expect::Ok(s4([0xffc0_0001, 0x7fc0_0000, 0, 0]), MXCSR | IE),
        "NaNs",
    );
    // A denormal binary64 source is a denormal operand and rounds to zero (tiny and inexact); DAZ zeroes it first.
    let denormal = d2([1, 0x8000_0000_0000_0001]);
    check(
        Op::Cvtpd2ps,
        (HI, denormal),
        MXCSR,
        Expect::Ok(s4([0, 0x8000_0000, 0, 0]), MXCSR | DE | UE | PE),
        "denormal source",
    );
    check(
        Op::Cvtpd2ps,
        (HI, denormal),
        MXCSR | DAZ,
        Expect::Ok(s4([0, 0x8000_0000, 0, 0]), MXCSR | DAZ),
        "denormal source under DAZ",
    );
}

// ---- CVTSS2SD and CVTSD2SS ----

#[test]
fn scalar_conversions_keep_the_bits_above_the_element() {
    check(
        Op::Cvtss2sd,
        (HI, s4([0x3fc0_0000, 0x1234_5678, 0, 0])),
        MXCSR,
        Expect::Ok(HI & !0xffff_ffff_ffff_ffff | 0x3ff8_0000_0000_0000, MXCSR),
        "cvtss2sd",
    );
    check(
        Op::Cvtss2sd,
        (HI, s4([0x7fa0_0000, 0, 0, 0])),
        MXCSR,
        Expect::Ok(keep64(0x7ffc_0000_0000_0000), MXCSR | IE),
        "cvtss2sd SNaN",
    );
    check(
        Op::Cvtss2sd,
        (HI, s4([1, 0, 0, 0])),
        MXCSR,
        Expect::Ok(keep64(0x36a0_0000_0000_0000), MXCSR | DE),
        "cvtss2sd denormal",
    );
    check(
        Op::Cvtsd2ss,
        (HI, d2([0xc004_0000_0000_0000, 0x3ff0_0000_0000_0000])),
        MXCSR,
        Expect::Ok(keep32(0xc020_0000), MXCSR),
        "cvtsd2ss",
    );
    check(
        Op::Cvtsd2ss,
        (HI, d2([0x47f0_0000_0000_0000, 0])),
        MXCSR | RC_ZERO,
        Expect::Ok(keep32(0x7f7f_ffff), MXCSR | RC_ZERO | OE | PE),
        "cvtsd2ss overflow toward zero",
    );
    check(
        Op::Cvtsd2ss,
        (HI, d2([0x3ff0_0000_0100_0000, 0])),
        0x1f80 & !0x1000,
        Expect::Fault(0x0f80 | PE),
        "cvtsd2ss unmasked PE",
    );
}

// ---- RCPPS, RCPSS, RSQRTPS, RSQRTSS ----

/// The deterministic cases of the SDM text: a zero or a denormal gives an infinity of the same sign, an infinity
/// gives a zero (RSQRT of -inf and of any negative number: the default NaN), an SNaN is quieted, a QNaN returned.
fn special(op: Op, bits: u32) -> u32 {
    let reciprocal = matches!(op, Op::Rcpps | Op::Rcpss);
    let sign = bits & 0x8000_0000;
    match bits & 0x7fff_ffff {
        0 | 1..=0x007f_ffff => sign | 0x7f80_0000,
        0x7f80_0000 if reciprocal => sign,
        0x7f80_0000 => {
            if sign != 0 {
                0xffc0_0000
            } else {
                0
            }
        }
        _ if bits & 0x7fff_ffff > 0x7f80_0000 => bits | 0x0040_0000,
        _ => unreachable!("only specials"),
    }
}

#[test]
fn reciprocal_specials_ignore_mxcsr_and_raise_nothing() {
    let specials = [
        0x0000_0000u32,
        0x8000_0000,
        0x0000_0001,
        0x8000_0001,
        0x007f_ffff,
        0x807f_ffff,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0001,
        0xffc0_0000,
        0x7fa0_0000,
        0xffbf_ffff,
    ];
    for op in [Op::Rcpps, Op::Rcpss, Op::Rsqrtps, Op::Rsqrtss] {
        for mx in [
            MXCSR,
            0x0000,
            MXCSR | RC_ZERO,
            MXCSR | DAZ | FTZ,
            MXCSR | 0x3f,
            0x0040 | RC_UP,
        ] {
            for chunk in specials.chunks(4) {
                let source = s4([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let expected = if matches!(op, Op::Rcpps | Op::Rsqrtps) {
                    s4(core::array::from_fn(|i| special(op, chunk[i])))
                } else {
                    keep32(special(op, chunk[0]))
                };
                check(
                    op,
                    (HI, source),
                    mx,
                    Expect::Ok(expected, mx),
                    &format!("{op:?} {mx:#x} {chunk:x?}"),
                );
            }
        }
    }
}

/// Runs one register-form instruction that must complete and returns xmm1 and MXCSR.
fn exec(op: Op, ab: (u128, u128), mx: u32) -> (u128, u32) {
    let observed = run_code(&reg_code(op, 1, 2, false), ab, mx);
    assert_eq!(
        observed.exit,
        RunExit::InstructionBudgetExhausted,
        "{op:?} {ab:x?} {mx:#x}"
    );
    (
        xmm(&observed.after, 1),
        observed.after.sse.as_ref().unwrap().mxcsr,
    )
}

/// The SDM bound: |relative error| <= 1.5 * 2^-12 (a small margin for the host arithmetic of the check).
fn within_bound(result: u32, exact: f64) -> bool {
    let value = f64::from(f32::from_bits(result));
    ((value - exact) / exact).abs() <= 1.5 / 4096.0 + 1e-9
}

#[test]
fn rsqrt_of_a_negative_number_is_the_default_nan_and_positive_lanes_meet_the_bound() {
    for bits in [0xbf80_0000u32, 0xc2c8_0000, 0xff7f_ffff, 0x8080_0000] {
        let (dst, mx) = exec(
            Op::Rsqrtps,
            (HI, s4([bits, 0x3f80_0000, bits, 0x4080_0000])),
            0x0000,
        );
        assert_eq!(mx, 0x0000);
        let lanes: [u32; 4] = core::array::from_fn(|i| (dst >> (32 * i)) as u32);
        assert_eq!(
            (lanes[0], lanes[2]),
            (0xffc0_0000, 0xffc0_0000),
            "{bits:#x}"
        );
        assert!(within_bound(lanes[1], 1.0), "{:#x}", lanes[1]);
        assert!(within_bound(lanes[3], 0.5), "{:#x}", lanes[3]);
    }
}

#[test]
fn reciprocal_normals_meet_the_sdm_error_bound() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..4000 {
        // Normal values over the exponent range where no result is tiny or infinite (the tiny boundary is
        // ISSUE-025): both signs for RCP, positive only for RSQRT.
        let operands: [u32; 4] = core::array::from_fn(|_| {
            let exponent = 2 + (next() % 250) as u32;
            (next() as u32 & 0x8000_0000) | exponent << 23 | (next() as u32 & 0x007f_ffff)
        });
        let positive = operands.map(|bits| bits & 0x7fff_ffff);
        let mx = [MXCSR, MXCSR | RC_ZERO, 0x0000, MXCSR | DAZ | FTZ][round % 4];
        let (rcp, mx_out) = exec(Op::Rcpps, (HI, s4(operands)), mx);
        assert_eq!(mx_out, mx);
        let (rsqrt, mx_out) = exec(Op::Rsqrtps, (HI, s4(positive)), mx);
        assert_eq!(mx_out, mx);
        for lane in 0..4 {
            let x = f64::from(f32::from_bits(operands[lane]));
            let r = (rcp >> (32 * lane)) as u32;
            assert!(
                within_bound(r, 1.0 / x),
                "rcp {:#x} -> {r:#x}",
                operands[lane]
            );
            let y = f64::from(f32::from_bits(positive[lane]));
            let r = (rsqrt >> (32 * lane)) as u32;
            assert!(
                within_bound(r, 1.0 / y.sqrt()),
                "rsqrt {:#x} -> {r:#x}",
                positive[lane]
            );
        }
        // The scalar forms compute element 0 and keep the rest of the destination.
        let (rcpss, _) = exec(Op::Rcpss, (HI, s4(operands)), mx);
        assert_eq!(rcpss >> 32, HI >> 32);
        assert_eq!(rcpss as u32, rcp as u32);
        let (rsqrtss, _) = exec(Op::Rsqrtss, (HI, s4(positive)), mx);
        assert_eq!(rsqrtss >> 32, HI >> 32);
        assert_eq!(rsqrtss as u32, rsqrt as u32);
    }
}

// ---- gates ----

#[test]
fn cr0_cr4_gates_and_lock() {
    for op in ALL {
        let code = reg_code(op, 1, 2, false);
        for (cr0, cr4, vector) in [
            (CR0_BASE | 1 << 2, CR4_ALL, 6u8),       // CR0.EM
            (CR0_BASE | 1 << 3, CR4_ALL, 7),         // CR0.TS: #NM
            (CR0_BASE, CR4_PAE | CR4_OSXMMEXCPT, 6), // CR4.OSFXSR = 0
        ] {
            let mut cpu = fp_init_cpu(cr0, cr4);
            set_xmm(&mut cpu, 2, test_value(op));
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
        // LOCK is #UD.
        let mut locked = vec![0xf0];
        locked.extend(&code);
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

#[test]
fn unmasked_exception_without_cr4_is_unsupported_and_changes_nothing() {
    // The bare run has no CR4 to pick #XM or #UD from: the faulting CVTPS2PD stays Unsupported.
    let mut cpu = cpu();
    cpu.sse.as_mut().unwrap().mxcsr = 0x1f00;
    set_xmm(&mut cpu, 2, s4([0x7fa0_0000, 0, 0, 0]));
    let before = cpu.clone();
    let code = reg_code(Op::Cvtps2pd, 1, 2, false);
    assert_eq!(
        step(&mut cpu, &mut Ram::new(&code)),
        Ok(StepOutcome::Unsupported { opcode: 0x5a })
    );
    assert_eq!(cpu, before);
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

#[test]
fn memory_source_equals_register_source() {
    for op in ALL {
        let source = test_value(op);
        let (width, _) = memory_of(op);
        let first = s4([0x4000_0000, 0x4040_0000, 0x3f80_0000, 0xc000_0000]);
        let from_register = run_code(&reg_code(op, 1, 2, false), (first, source), MXCSR);
        assert_eq!(
            from_register.exit,
            RunExit::InstructionBudgetExhausted,
            "{op:?}"
        );
        for (name, address, rex_b, registers, effective) in address_forms(0) {
            let code = mem_code(op, 1, &address, rex_b);
            let mut cpu = cpu();
            set_xmm(&mut cpu, 1, first);
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let mut ram = Ram::new(&code);
            ram.put(effective, &source.to_le_bytes()[..width]);
            // The bytes after the operand differ from the register source's upper lanes: they must not matter.
            ram.put(effective + width as u64, &[0xa5; 16]);
            let mut expected = cpu.clone();
            set_xmm(&mut expected, 1, xmm(&from_register.after, 1));
            expected.rip += code.len() as u64;
            assert_eq!(
                step(&mut cpu, &mut ram),
                Ok(StepOutcome::Advanced),
                "{op:?} {name}"
            );
            assert_eq!(cpu, expected, "{op:?} {name}");
            assert_eq!(ram.reads, vec![(effective, width)], "{op:?} {name}");
            assert!(ram.writes.is_empty());
        }
    }
}

#[test]
fn memory_alignment_faults_and_unaligned_scalars() {
    for op in ALL {
        let (width, aligned) = memory_of(op);
        for delta in [1u64, 4, 8, 12] {
            let (name, address, rex_b, registers, effective) = address_forms(delta).remove(1);
            let code = mem_code(op, 1, &address, rex_b);
            let mut cpu = cpu();
            for (register, value) in registers {
                cpu.gpr[register as usize] = value;
            }
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            ram.put(effective, &test_value(op).to_le_bytes());
            let outcome = step(&mut cpu, &mut ram);
            if aligned {
                // Types 2 and 4: a 128-bit memory operand must be 16-byte aligned: #GP(0), no read.
                assert_eq!(outcome, Ok(fault_of(13, Some(0))), "{op:?} {name} +{delta}");
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            } else {
                // Types 3 and 5: no alignment requirement (CVTPS2PD reads 8 bytes, not 16).
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{op:?} {name} +{delta}");
                assert_eq!(ram.reads, vec![(effective, width)], "{op:?} +{delta}");
            }
        }
    }
}

#[test]
fn memory_faults_are_not_partial_and_noncanonical_is_gp() {
    for op in ALL {
        let (_, address, rex_b, _, _) = address_forms(0).remove(1);
        let code = mem_code(op, 1, &address, rex_b);
        // A page fault of the backend at the source: nothing changes.
        let mut cpu = cpu();
        cpu.gpr[od_isa::Register::Rbx as usize] = DATA + 0x1000;
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
        cpu.gpr[od_isa::Register::Rbx as usize] = 0x0000_8000_0000_0000;
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
fn cvtps2pd_reads_only_eight_bytes_at_the_end_of_a_page() {
    // The 8-byte source ends at the page boundary: a 16-byte read would touch the next page.
    let code = mem_code(Op::Cvtps2pd, 1, &[0x03], false);
    let mut cpu = cpu();
    cpu.gpr[od_isa::Register::Rbx as usize] = DATA + 0x1000 - 8;
    let mut ram = Ram::new(&code);
    ram.fault_page = Some(DATA + 0x1000);
    ram.put(
        DATA + 0x1000 - 8,
        &s4([0x3f80_0000, 0x4000_0000, 0, 0]).to_le_bytes()[..8],
    );
    assert_eq!(step(&mut cpu, &mut ram), Ok(StepOutcome::Advanced));
    assert_eq!(
        xmm(&cpu, 1),
        d2([0x3ff0_0000_0000_0000, 0x4000_0000_0000_0000])
    );
    assert_eq!(ram.reads, vec![(DATA + 0x1000 - 8, 8)]);
}

// ---- decoding ----

fn decode(bytes: &[u8]) -> Result<long_decode::Instruction, DecodeError> {
    let mut ram = Ram::new(bytes);
    long_decode::decode(&mut ram, CODE, AccessContext::default())
}

#[test]
fn decoder_produces_the_general_operations() {
    use SseFpMiscKind::{Cvtpd2ps, Cvtps2pd, Cvtsd2ss, Cvtss2sd, Rcp, Rsqrt};
    // Memory forms other than [rax] with xmm1 are not narrow profile forms: the general decoder handles them.
    let memory: [(&[u8], SseFpMiscKind, SseFpShape, u8); 8] = [
        (&[0x0f, 0x5a, 0x0b], Cvtps2pd, PS, 1),
        (&[0x66, 0x0f, 0x5a, 0x0b], Cvtpd2ps, PD, 1),
        (&[0xf3, 0x0f, 0x5a, 0x0b], Cvtss2sd, SS, 1),
        (&[0xf2, 0x0f, 0x5a, 0x13], Cvtsd2ss, SD, 2),
        (&[0x0f, 0x53, 0x0b], Rcp, PS, 1),
        (&[0xf3, 0x0f, 0x53, 0x0b], Rcp, SS, 1),
        (&[0x0f, 0x52, 0x0b], Rsqrt, PS, 1),
        (&[0xf3, 0x44, 0x0f, 0x52, 0x0b], Rsqrt, SS, 9),
    ];
    for (bytes, kind, shape, destination) in memory {
        let instruction = decode(bytes).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
        let Operation::SseFpMisc {
            kind: k,
            shape: s,
            destination: d,
            source: Sse3Source::Memory(_),
            immediate: 0,
        } = instruction.operation
        else {
            panic!("{bytes:02x?}: {:?}", instruction.operation);
        };
        assert_eq!((k, s, d), (kind, shape, destination), "{bytes:02x?}");
    }
    // With REX.W (ignored by the instruction) the register forms are general operations too; REX.R/B extend the registers.
    for (bytes, kind, shape) in [
        (&[0x4d, 0x0f, 0x5a, 0xca][..], Cvtps2pd, PS),
        (&[0x66, 0x4d, 0x0f, 0x5a, 0xca], Cvtpd2ps, PD),
        (&[0xf3, 0x4d, 0x0f, 0x5a, 0xca], Cvtss2sd, SS),
        (&[0xf2, 0x4d, 0x0f, 0x5a, 0xca], Cvtsd2ss, SD),
        (&[0x4d, 0x0f, 0x53, 0xca], Rcp, PS),
        (&[0xf3, 0x4d, 0x0f, 0x53, 0xca], Rcp, SS),
        (&[0x4d, 0x0f, 0x52, 0xca], Rsqrt, PS),
        (&[0xf3, 0x4d, 0x0f, 0x52, 0xca], Rsqrt, SS),
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
    // The size and alignment rule are properties of the instruction, not of the prefix shape alone.
    assert_eq!(SseFpMiscKind::Cvtps2pd.source_access(PS), (8, false));
    assert_eq!(SseFpMiscKind::Cvtpd2ps.source_access(PD), (16, true));
    assert_eq!(SseFpMiscKind::Cvtss2sd.source_access(SS), (4, false));
    assert_eq!(SseFpMiscKind::Cvtsd2ss.source_access(SD), (8, false));
    assert_eq!(SseFpMiscKind::Rcp.source_access(PS), (16, true));
    assert_eq!(SseFpMiscKind::Rcp.source_access(SS), (4, false));
    assert_eq!(SseFpMiscKind::Rsqrt.source_access(PS), (16, true));
    assert_eq!(SseFpMiscKind::Rsqrt.source_access(SS), (4, false));
}

#[test]
fn decoder_refuses_what_it_does_not_model_and_locked_forms_are_invalid() {
    // Unsupported: 66 together with F2/F3, a repeated prefix, FS/GS, F2 on RCP/RSQRT, 66 on them.
    for bytes in [
        &[0x66, 0xf2, 0x0f, 0x5a, 0x0b][..],
        &[0x66, 0xf3, 0x0f, 0x5a, 0x0b],
        &[0xf2, 0xf2, 0x0f, 0x5a, 0x0b],
        &[0x66, 0x66, 0x0f, 0x5a, 0x0b],
        &[0x64, 0x0f, 0x5a, 0x0b],
        &[0x65, 0xf3, 0x0f, 0x53, 0x0b],
        &[0xf2, 0x0f, 0x53, 0x0b],
        &[0xf2, 0x0f, 0x52, 0x0b],
        &[0x66, 0x0f, 0x53, 0x0b],
        &[0x66, 0x0f, 0x52, 0x0b],
    ] {
        assert!(
            matches!(decode(bytes), Err(DecodeError::Unsupported { .. })),
            "{bytes:02x?}: {:?}",
            decode(bytes).map(|i| i.operation)
        );
    }
    for bytes in [
        &[0xf0, 0x0f, 0x5a, 0x0b][..],
        &[0xf0, 0x66, 0x0f, 0x5a, 0x0b],
        &[0xf0, 0xf3, 0x0f, 0x5a, 0x0b],
        &[0xf0, 0xf2, 0x0f, 0x5a, 0x0b],
        &[0xf0, 0x0f, 0x53, 0x0b],
        &[0xf0, 0xf3, 0x0f, 0x52, 0x0b],
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
        0x7fc0_0001,
        0x7fa0_0000,
        1,
        0x007f_ffff,
        0x4f00_0000,
        0xcf00_0000,
        0x4b80_0001,
        0x3fc0_0000,
        0x7e80_0000,
    ];
    for op in ALL {
        for round in 0..300 {
            let mut vector = || {
                s4(core::array::from_fn(|_| {
                    pool[(next() % pool.len() as u64) as usize]
                })) | u128::from(next()) << 64 & if round % 3 == 0 { u128::MAX } else { 0 }
            };
            let (a, b) = (vector(), vector());
            let mx = [MXCSR, MXCSR | DAZ, MXCSR & !0x80, 0, MXCSR | 3 << 13][round % 5];
            let narrow = run_code(&reg_code(op, 1, 2, false), (a, b), mx);
            let general = run_code(&reg_code(op, 1, 2, true), (a, b), mx);
            assert_eq!(narrow.exit, general.exit, "{op:?} {a:#x} {b:#x}");
            assert_eq!(xmm(&narrow.after, 1), xmm(&general.after, 1), "{op:?}");
            assert_eq!(
                narrow.after.sse.as_ref().unwrap().mxcsr,
                general.after.sse.as_ref().unwrap().mxcsr,
                "{op:?}"
            );
            assert_eq!(narrow.after.rflags, general.after.rflags, "{op:?}");
        }
    }
}

// ---- host arithmetic ----

#[test]
fn widening_and_narrowing_match_the_host_on_random_operands() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..4000 {
        // CVTSS2SD and CVTPS2PD are exact: the host cast gives the same bits for every non-NaN value, denormals
        // included (they set DE, checked separately above).
        let x = f32::from_bits(next() as u32);
        let y = f32::from_bits(next() as u32);
        if !x.is_nan() && !y.is_nan() {
            let (dst, _) = exec(
                Op::Cvtps2pd,
                (HI, s4([x.to_bits(), y.to_bits(), 0, 0])),
                MXCSR,
            );
            assert_eq!(
                dst,
                d2([f64::from(x).to_bits(), f64::from(y).to_bits()]),
                "{x:e} {y:e}"
            );
        }
        // CVTSD2SS and CVTPD2PS round to nearest even by default like the host cast. The flags: overflow
        // (the result is infinite from a finite source) and precision (the conversion back differs).
        let a = f64::from_bits(next());
        let b =
            f64::from_bits(next() & 0x7fff_ffff_ffff_ffff | (next() & 0x0010_0000_0000_0000) << 1);
        if !a.is_nan() && !b.is_nan() {
            let (dst, mx) = exec(
                Op::Cvtpd2ps,
                (HI, d2([a.to_bits(), b.to_bits()])),
                MXCSR | DAZ,
            );
            // Under DAZ a denormal source is a zero: compare with the flushed values.
            let flush = |v: f64| {
                if v.is_subnormal() {
                    0.0f64.copysign(v)
                } else {
                    v
                }
            };
            let (fa, fb) = (flush(a), flush(b));
            assert_eq!(
                dst,
                s4([(fa as f32).to_bits(), (fb as f32).to_bits(), 0, 0]),
                "{a:e} {b:e}"
            );
            let overflow = (fa.is_finite() && (fa as f32).is_infinite())
                || (fb.is_finite() && (fb as f32).is_infinite());
            assert_eq!(mx & OE != 0, overflow, "{a:e} {b:e}");
            let (sa, _) = exec(Op::Cvtsd2ss, (HI, d2([a.to_bits(), 0])), MXCSR | DAZ);
            assert_eq!(sa, keep32((flush(a) as f32).to_bits()), "{a:e}");
        }
    }
}

// ---- hardware vectors ----

const GOLDEN: &[Row] = &[
    (
        "rg_cvtpd2ps_0_16",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe5],
        0x3d80,
        &[
            (5, 0x43300000000000000000000000000000),
            (12, 0x65dad5ae75dd13c36355fa7c117aed60),
        ],
        Expect::Ok(0x00000000000000005980000000000000, 0x3d80),
    ),
    (
        "rg_cvtpd2ps_10_22",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc7],
        0x5d80,
        &[
            (0, 0xf11449009f80d8d80b56218b8a82048e),
            (15, 0x00200000000000007ff0000000000000),
        ],
        Expect::Ok(0x0000000000000000000000017f800000, 0x5db0),
    ),
    (
        "rg_cvtpd2ps_10_32",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc5],
        0x9f00,
        &[
            (0, 0x17f67edbe73f5ed7fd8080d1621e3fd3),
            (13, 0xbfe00000000000007ff0000000000000),
        ],
        Expect::Ok(0x0000000000000000bf0000007f800000, 0x9f00),
    ),
    (
        "rg_cvtpd2ps_10_69",
        &[0x66, 0x45, 0x0f, 0x5a, 0xce],
        0x1780,
        &[
            (9, 0x585027c46bb3922bdf23f0e45372c1d4),
            (14, 0x3ff00000000000007ff0000000000000),
        ],
        Expect::Ok(0x00000000000000003f8000007f800000, 0x1780),
    ),
    (
        "rg_cvtpd2ps_11_29",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc3],
        0x7b80,
        &[
            (0, 0x1ce9f4bae259b17aa2f4a568c5553220),
            (11, 0xc000000000000000fff0000000000000),
        ],
        Expect::Ok(0x0000000000000000c0000000ff800000, 0x7b80),
    ),
    (
        "rg_cvtpd2ps_12_14",
        &[0x66, 0x45, 0x0f, 0x5a, 0xf3],
        0x3f00,
        &[
            (11, 0x40080000000000007ff8000000000000),
            (14, 0x8a23a100f55be0f8aa5a200ac48972e0),
        ],
        Expect::Ok(0x0000000000000000404000007fc00000, 0x3f00),
    ),
    (
        "rg_cvtpd2ps_12_48",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd8],
        0xfd80,
        &[
            (0, 0x3ca00000000000007ff8000000000000),
            (11, 0x0dc24bde7ad713090dca5136b3dddd2b),
        ],
        Expect::Ok(0x0000000000000000250000007fc00000, 0xfd80),
    ),
    (
        "rg_cvtpd2ps_13_72",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd9],
        0x1700,
        &[
            (1, 0x4008000000000000fff8000000000000),
            (11, 0xf5bfa489dabeb328b154400e20fecc51),
        ],
        Expect::Ok(0x000000000000000040400000ffc00000, 0x1700),
    ),
    (
        "rg_cvtpd2ps_14_52",
        &[0x66, 0x0f, 0x5a, 0xd8],
        0xf780,
        &[
            (0, 0x3fe00000000000007ff8000000000001),
            (3, 0xc084876d7b8b688fb2e1366e69f9bbaa),
        ],
        Expect::Ok(0x00000000000000003f0000007fc00000, 0xf780),
    ),
    (
        "rg_cvtpd2ps_15_23",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd4],
        0x5b80,
        &[
            (4, 0x3ff00000000000007fffffffffffffff),
            (10, 0xe1f7b93dab9ea3ea096460b641cac350),
        ],
        Expect::Ok(0x00000000000000003f8000007fffffff, 0x5b80),
    ),
    (
        "rg_cvtpd2ps_15_36",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe6],
        0x9d80,
        &[
            (12, 0xc62c09f60b5a1e25fd95055bbb12c3e8),
            (14, 0x7ff00000000000017fffffffffffffff),
        ],
        Expect::Ok(0x00000000000000007fc000007fffffff, 0x9d81),
    ),
    (
        "rg_cvtpd2ps_17_29",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd7],
        0x7b80,
        &[
            (2, 0x4e353f56aa5874f372084d5ddce112e0),
            (15, 0x80000000000000007ff0000000000001),
        ],
        Expect::Ok(0x0000000000000000800000007fc00000, 0x7b81),
    ),
    (
        "rg_cvtpd2ps_17_57",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd0],
        0x4000,
        &[
            (0, 0x43400000000000007ff0000000000001),
            (10, 0xb6868a564312651f5a9de1edf9df6ca5),
        ],
        Expect::Fault(0x4001),
    ),
    (
        "rg_cvtpd2ps_18_60",
        &[0x66, 0x45, 0x0f, 0x5a, 0xfb],
        0x40,
        &[
            (11, 0x7ffffffffffffffffff0000000000001),
            (15, 0xfa4f15305a90e622ba786eb0b9316734),
        ],
        Expect::Fault(0x41),
    ),
    (
        "rg_cvtpd2ps_1_42",
        &[0x66, 0x44, 0x0f, 0x5a, 0xfa],
        0x8f80,
        &[
            (2, 0x7fffffffffffffff8000000000000000),
            (15, 0x9d48395e24679727f4e64b0eda952fd9),
        ],
        Expect::Ok(0x00000000000000007fffffff80000000, 0x8f80),
    ),
    (
        "rg_cvtpd2ps_1_45",
        &[0x66, 0x41, 0x0f, 0x5a, 0xfc],
        0x7f40,
        &[
            (7, 0x16594ae33e846b4e37578031e2b464fd),
            (12, 0x3ff00000000000008000000000000000),
        ],
        Expect::Ok(0x00000000000000003f80000080000000, 0x7f40),
    ),
    (
        "rg_cvtpd2ps_1_49",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc3],
        0x7dc0,
        &[
            (8, 0x66b9517b355471175ad00899772893b6),
            (11, 0x3ff80000000000008000000000000000),
        ],
        Expect::Ok(0x00000000000000003fc0000080000000, 0x7dc0),
    ),
    (
        "rg_cvtpd2ps_1_50",
        &[0x66, 0x45, 0x0f, 0x5a, 0xf7],
        0xfb80,
        &[
            (14, 0x578afe359636118fc957c89d9f563c57),
            (15, 0x3fe80000000000008000000000000000),
        ],
        Expect::Ok(0x00000000000000003f40000080000000, 0xfb80),
    ),
    (
        "rg_cvtpd2ps_1_55",
        &[0x66, 0x0f, 0x5a, 0xd3],
        0x6fc0,
        &[
            (2, 0x557a821dac58857a24f64664a7073fa8),
            (3, 0xfff8123456789abc8000000000000000),
        ],
        Expect::Ok(0x0000000000000000ffc091a280000000, 0x6fc0),
    ),
    (
        "rg_cvtpd2ps_21_5",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe2],
        0x9f80,
        &[
            (2, 0x0010000000000000fff7ffffffffffff),
            (12, 0x47c026dd6662f38a6877c5960e43b84c),
        ],
        Expect::Ok(0x000000000000000000000000ffffffff, 0x9fb1),
    ),
    (
        "rg_cvtpd2ps_22_61",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe8],
        0x80c0,
        &[
            (8, 0x7ff00000000000000000000000000001),
            (13, 0x0b3c300dd75cac210166e260a9325efd),
        ],
        Expect::Ok(0x00000000000000007f80000000000000, 0x80c0),
    ),
    (
        "rg_cvtpd2ps_24_10",
        &[0x66, 0x41, 0x0f, 0x5a, 0xf7],
        0x1d80,
        &[
            (6, 0xae8555f78a8100e37b6a26987e8c2c8b),
            (15, 0x3fd0000000000000000fffffffffffff),
        ],
        Expect::Ok(0x00000000000000003e80000000000000, 0x1db2),
    ),
    (
        "rg_cvtpd2ps_25_60",
        &[0x66, 0x0f, 0x5a, 0xcb],
        0x40,
        &[
            (1, 0x51a968e256411f64a47865772bd2a5f5),
            (3, 0xc004000000000000800fffffffffffff),
        ],
        Expect::Ok(0x0000000000000000c020000080000000, 0x40),
    ),
    (
        "rg_cvtpd2ps_25_7",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd2],
        0x0,
        &[
            (2, 0x3cb0000000000000800fffffffffffff),
            (10, 0x5a20822c9255aec181663405c3154795),
        ],
        Expect::Fault(0x2),
    ),
    (
        "rg_cvtpd2ps_27_17",
        &[0x66, 0x41, 0x0f, 0x5a, 0xdf],
        0x3b80,
        &[
            (3, 0xc7484cf383ea6a5b26ab834aa24b206a),
            (15, 0xfff8123456789abc0000000000000003),
        ],
        Expect::Ok(0x0000000000000000ffc091a200000000, 0x3bb2),
    ),
    (
        "rg_cvtpd2ps_28_65",
        &[0x66, 0x41, 0x0f, 0x5a, 0xca],
        0xfbf,
        &[
            (1, 0xf75c84820e053f12f25eecf55e168efb),
            (10, 0x7ff00000000000000010000000000000),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rg_cvtpd2ps_29_44",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc9],
        0xff00,
        &[
            (1, 0x3fd00000000000008010000000000000),
            (9, 0xe40848c6589b10fb5e7cfd76e226c1f9),
        ],
        Expect::Ok(0x00000000000000003e80000080000000, 0xff30),
    ),
    (
        "rg_cvtpd2ps_2_46",
        &[0x66, 0x0f, 0x5a, 0xde],
        0xfe80,
        &[
            (3, 0x56bc3d631815ed67df539479e823ef1e),
            (6, 0x43400000000000003ff0000000000000),
        ],
        Expect::Ok(0x00000000000000005a0000003f800000, 0xfe80),
    ),
    (
        "rg_cvtpd2ps_32_56",
        &[0x66, 0x0f, 0x5a, 0xe1],
        0x2000,
        &[
            (1, 0x7ff8000000000001ffefffffffffffff),
            (4, 0x1bc8f6966a9a8dccf10c847b6f1ac76a),
        ],
        Expect::Fault(0x2028),
    ),
    (
        "rg_cvtpd2ps_33_64",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc1],
        0x5faa,
        &[
            (1, 0xc0040000000000007fe0000000000000),
            (8, 0x9908f1cf2e33cc0e691b2fc88679eef7),
        ],
        Expect::Ok(0x0000000000000000c02000007f800000, 0x5faa),
    ),
    (
        "rg_cvtpd2ps_36_39",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd0],
        0x1bc0,
        &[
            (2, 0xbc5fb3eee5c1134884e65eff2f53f621),
            (8, 0x3fd00000000000003cb0000000000000),
        ],
        Expect::Ok(0x00000000000000003e80000025800000, 0x1bc0),
    ),
    (
        "rg_cvtpd2ps_36_45",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc1],
        0x7f40,
        &[
            (0, 0xe394bbf88ea948a65606fcfb8dcf4eb3),
            (9, 0x800fffffffffffff3cb0000000000000),
        ],
        Expect::Ok(0x00000000000000008000000025800000, 0x7f40),
    ),
    (
        "rg_cvtpd2ps_37_36",
        &[0x66, 0x0f, 0x5a, 0xcb],
        0x9d80,
        &[
            (1, 0xa4b2e78dc0df0f0f34c5ca721de3ff13),
            (3, 0x43255555555555553ca0000000000000),
        ],
        Expect::Ok(0x0000000000000000592aaaab25000000, 0x9da0),
    ),
    (
        "rg_cvtpd2ps_38_28",
        &[0x66, 0x0f, 0x5a, 0xf2],
        0x7d80,
        &[
            (2, 0x3cb00000000000004330000000000000),
            (6, 0x2e893a0eaec252fcf7cfe4d196184e7b),
        ],
        Expect::Ok(0x00000000000000002580000059800000, 0x7d80),
    ),
    (
        "rg_cvtpd2ps_38_32",
        &[0x66, 0x0f, 0x5a, 0xc0],
        0x9f00,
        &[(0, 0x40000000000000004330000000000000)],
        Expect::Ok(0x00000000000000004000000059800000, 0x9f00),
    ),
    (
        "rg_cvtpd2ps_42_16",
        &[0x66, 0x0f, 0x5a, 0xf5],
        0x3d80,
        &[
            (5, 0x3ca00000000000000020000000000000),
            (6, 0x236b0b47ee208f31f1e724309855a36c),
        ],
        Expect::Ok(0x00000000000000002500000000000000, 0x3db0),
    ),
    (
        "rg_cvtpd2ps_44_3",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc4],
        0x7f80,
        &[
            (8, 0xcc016769137d62be79eeface84a59c23),
            (12, 0x7ff7ffffffffffff4014000000000000),
        ],
        Expect::Ok(0x00000000000000007fffffff40a00000, 0x7f81),
    ),
    (
        "rg_cvtpd2ps_45_74",
        &[0x66, 0x0f, 0x5a, 0xef],
        0x1a00,
        &[
            (5, 0x40de174d3a7e6abfe7e8b9fb18cf4a3e),
            (7, 0x43300000000000003ff8000000000000),
        ],
        Expect::Ok(0x0000000000000000598000003fc00000, 0x1a00),
    ),
    (
        "rg_cvtpd2ps_46_15",
        &[0x66, 0x44, 0x0f, 0x5a, 0xca],
        0x3e80,
        &[
            (2, 0x80000000000000004004000000000000),
            (9, 0x9b6127ffbcde070ca89a8921a9f64a3a),
        ],
        Expect::Ok(0x00000000000000008000000040200000, 0x3e80),
    ),
    (
        "rg_cvtpd2ps_49_20",
        &[0x66, 0x44, 0x0f, 0x5a, 0xce],
        0x5f00,
        &[
            (6, 0x3cb0000000000000bfe0000000000000),
            (9, 0xad15b659dc321cf5c2cdfd170d0273a4),
        ],
        Expect::Ok(0x000000000000000025800000bf000000, 0x5f00),
    ),
    (
        "rg_cvtpd2ps_4_29",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc5],
        0x7b80,
        &[
            (5, 0x7ff80000000000014000000000000000),
            (8, 0x24f0aef020ef30b4ac1d009cab85c71a),
        ],
        Expect::Ok(0x00000000000000007fc0000040000000, 0x7b80),
    ),
    (
        "rg_cvtpd2ps_50_42",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd1],
        0x8f80,
        &[
            (1, 0x8000000000000000bfefffffffffffff),
            (10, 0xfaec20b6faf907555021fb6e1213996e),
        ],
        Expect::Fault(0x8fa0),
    ),
    (
        "rg_cvtpd2ps_54_44",
        &[0x66, 0x0f, 0x5a, 0xf5],
        0xff00,
        &[
            (5, 0xbfd0000000000000432ffffffffffffe),
            (6, 0x6a5143a748fd06b7f8bc2c3f4559e103),
        ],
        Expect::Ok(0x0000000000000000be800000597fffff, 0xff20),
    ),
    (
        "rg_cvtpd2ps_55_53",
        &[0x66, 0x44, 0x0f, 0x5a, 0xf1],
        0x77c0,
        &[
            (1, 0xbff00000000000004325555555555555),
            (14, 0xfc6ccbb5dd981e9590dc610a4bed2cda),
        ],
        Expect::Ok(0x0000000000000000bf800000592aaaaa, 0x77e0),
    ),
    (
        "rg_cvtpd2ps_7_26",
        &[0x66, 0x0f, 0x5a, 0xe6],
        0x7f00,
        &[
            (4, 0x7eb73cc38276b71da3f72cfd36a52d57),
            (6, 0x40000000000000003fe0000000000000),
        ],
        Expect::Ok(0x0000000000000000400000003f000000, 0x7f00),
    ),
    (
        "rg_cvtpd2ps_8_10",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe3],
        0x1d80,
        &[
            (11, 0x00000000000000003ff0000000000001),
            (12, 0xbee6ef861ff06026cc29de01ff331f0d),
        ],
        Expect::Ok(0x0000000000000000000000003f800000, 0x1da0),
    ),
    (
        "rg_cvtpd2ps_9_31",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc9],
        0x6f80,
        &[
            (1, 0x3ca00000000000003fefffffffffffff),
            (9, 0x696d148c4f8bd3cb645b236e6128d35d),
        ],
        Expect::Fault(0x6fa0),
    ),
    (
        "rg_cvtpd2ps_9_50",
        &[0x66, 0x45, 0x0f, 0x5a, 0xec],
        0xfb80,
        &[
            (12, 0x3cb00000000000003fefffffffffffff),
            (13, 0xfb304fcc3a63d76878a2b6bd77fbe12c),
        ],
        Expect::Ok(0x0000000000000000258000003f7fffff, 0xfba0),
    ),
    (
        "rg_cvtps2pd_12_58",
        &[0x41, 0x0f, 0x5a, 0xd7],
        0x6000,
        &[
            (2, 0x7ef3a69a1beb0e4d806ef024cf349840),
            (15, 0x4b00000040a000003f8000007fc00000),
        ],
        Expect::Ok(0x3ff00000000000007ff8000000000000, 0x6000),
    ),
    (
        "rg_cvtps2pd_12_7",
        &[0x45, 0x0f, 0x5a, 0xf0],
        0x0,
        &[
            (8, 0x00000003cb000001807fffff7fc00000),
            (14, 0x7b2bbec81bad8fc9ff2500f56da554e4),
        ],
        Expect::Fault(0x2),
    ),
    (
        "rg_cvtps2pd_15_56",
        &[0x41, 0x0f, 0x5a, 0xf1],
        0x2000,
        &[
            (6, 0xdc80253e8d71a65278dbad71a5445ad4),
            (9, 0x00ffffffc02000004b7fffff7fffffff),
        ],
        Expect::Ok(0x416fffffe00000007fffffffe0000000, 0x2000),
    ),
    (
        "rg_cvtps2pd_16_25",
        &[0x0f, 0x5a, 0xca],
        0x4f80,
        &[
            (1, 0x9ad4d8c75acb560bd887b6f7eb434068),
            (2, 0x3f7fffffc2f6e979c2f6e979ffc12345),
        ],
        Expect::Ok(0xc05edd2f20000000fff82468a0000000, 0x4f80),
    ),
    (
        "rg_cvtps2pd_17_28",
        &[0x41, 0x0f, 0x5a, 0xdd],
        0x7d80,
        &[
            (3, 0x3d2e3e4f4df728bd5b2cd52d5df478cf),
            (13, 0x4b80000000000003000000037f800001),
        ],
        Expect::Ok(0x36b80000000000007ff8000020000000, 0x7d83),
    ),
    (
        "rg_cvtps2pd_18_45",
        &[0x44, 0x0f, 0x5a, 0xe6],
        0x7f40,
        &[
            (6, 0x010000007fbfffff7e800000ff800001),
            (12, 0xbacade2b2347f01b497cf6e42a17fb11),
        ],
        Expect::Fault(0x7f41),
    ),
    (
        "rg_cvtps2pd_18_58",
        &[0x45, 0x0f, 0x5a, 0xef],
        0x6000,
        &[
            (13, 0xe3ad8f704d30bda46b73a09e3dd60288),
            (15, 0x3f8000003f000000ff800001ff800001),
        ],
        Expect::Fault(0x6001),
    ),
    (
        "rg_cvtps2pd_18_72",
        &[0x44, 0x0f, 0x5a, 0xe0],
        0x1700,
        &[
            (0, 0xbf7fffff7f7fffff3f7fffffff800001),
            (12, 0x1442ac55609fa659629d613159391220),
        ],
        Expect::Fault(0x1701),
    ),
    (
        "rg_cvtps2pd_19_14",
        &[0x0f, 0x5a, 0xf1],
        0x3f00,
        &[
            (1, 0x7f8000007f8000003f8000017fa00000),
            (6, 0x55023af673b76316635a3359624663e8),
        ],
        Expect::Fault(0x3f01),
    ),
    (
        "rg_cvtps2pd_19_16",
        &[0x0f, 0x5a, 0xe5],
        0x3d80,
        &[
            (4, 0x4561ebd092c8c427b5a0ad07ccf39846),
            (5, 0x340000007fffffff007fffff7fa00000),
        ],
        Expect::Ok(0x380fffffc00000007ffc000000000000, 0x3d83),
    ),
    (
        "rg_cvtps2pd_19_33",
        &[0x44, 0x0f, 0x5a, 0xf8],
        0x1f40,
        &[
            (0, 0x7f7fffff3f0000004afffffe7fa00000),
            (15, 0xa9ead3be1942ceae58afcecf4eee116a),
        ],
        Expect::Fault(0x1f41),
    ),
    (
        "rg_cvtps2pd_19_45",
        &[0x0f, 0x5a, 0xd6],
        0x7f40,
        &[
            (2, 0x3c0abbbb580afc8ee8d1a08b9b5ed11f),
            (6, 0x7f800001c00000007e8000007fa00000),
        ],
        Expect::Fault(0x7f41),
    ),
    (
        "rg_cvtps2pd_20_37",
        &[0x41, 0x0f, 0x5a, 0xd6],
        0x1dc0,
        &[
            (2, 0x1a5c29b2f076dea91b0b615a0377fb91),
            (14, 0xbe80000000000000ff8000007fbfffff),
        ],
        Expect::Ok(0xfff00000000000007fffffffe0000000, 0x1dc1),
    ),
    (
        "rg_cvtps2pd_21_22",
        &[0x41, 0x0f, 0x5a, 0xf6],
        0x5d80,
        &[
            (6, 0x59fb5cc8a42d973fe395c172c97ed433),
            (14, 0xc00000004b7fffffc0200000ffbfffff),
        ],
        Expect::Ok(0xc004000000000000ffffffffe0000000, 0x5d81),
    ),
    (
        "rg_cvtps2pd_21_26",
        &[0x45, 0x0f, 0x5a, 0xde],
        0x7f00,
        &[
            (11, 0xed1968e2f96557febf565fcabad943fa),
            (14, 0x807fffffbe8000007f000000ffbfffff),
        ],
        Expect::Fault(0x7f01),
    ),
    (
        "rg_cvtps2pd_21_33",
        &[0x44, 0x0f, 0x5a, 0xe0],
        0x1f40,
        &[
            (0, 0x7e8000000000000100800000ffbfffff),
            (12, 0x144a79126b1de4fba9c50b2be3072b48),
        ],
        Expect::Fault(0x1f41),
    ),
    (
        "rg_cvtps2pd_22_32",
        &[0x41, 0x0f, 0x5a, 0xf2],
        0x9f00,
        &[
            (6, 0x41d59c71952d319c68ac854ea3e625db),
            (10, 0x00ffffffcb000001ffc0000000000001),
        ],
        Expect::Ok(0xfff800000000000036a0000000000000, 0x9f02),
    ),
    (
        "rg_cvtps2pd_23_32",
        &[0x41, 0x0f, 0x5a, 0xe6],
        0x9f00,
        &[
            (4, 0x96773e0f8daebbdf72795789f7bd8f3e),
            (14, 0x80000000008000007f80000080000001),
        ],
        Expect::Ok(0x7ff0000000000000b6a0000000000000, 0x9f02),
    ),
    (
        "rg_cvtps2pd_25_28",
        &[0x44, 0x0f, 0x5a, 0xda],
        0x7d80,
        &[
            (2, 0xbf7fffff807fffff4afffffe807fffff),
            (11, 0xd27e25c67b699e7695da567a59d23dd8),
        ],
        Expect::Ok(0x415fffffc0000000b80fffffc0000000, 0x7d82),
    ),
    (
        "rg_cvtps2pd_27_25",
        &[0x0f, 0x5a, 0xf7],
        0x4f80,
        &[
            (6, 0x8badf133f471601d4c91de4f2fa99098),
            (7, 0x008000014b7fffff3f7fffff00000003),
        ],
        Expect::Ok(0x3fefffffe000000036b8000000000000, 0x4f82),
    ),
    (
        "rg_cvtps2pd_27_69",
        &[0x0f, 0x5a, 0xc4],
        0x1780,
        &[
            (0, 0xa1cf869890e5e0dac531aeef77575a81),
            (4, 0x808000004b0000017fa0000000000003),
        ],
        Expect::Ok(0x7ffc00000000000036b8000000000000, 0x1783),
    ),
    (
        "rg_cvtps2pd_29_12",
        &[0x44, 0x0f, 0x5a, 0xf5],
        0x1780,
        &[
            (5, 0x00000001ffbfffff7fbfffff80800000),
            (14, 0x3c01de1a8c568efc700053c12f41e063),
        ],
        Expect::Ok(0x7fffffffe0000000b810000000000000, 0x1781),
    ),
    (
        "rg_cvtps2pd_29_9",
        &[0x41, 0x0f, 0x5a, 0xd4],
        0x1e80,
        &[
            (2, 0x2e78ee3162fd303c55be0e56b28e036b),
            (12, 0x7f7fffff000000000000000180800000),
        ],
        Expect::Fault(0x1e82),
    ),
    (
        "rg_cvtps2pd_2_49",
        &[0x41, 0x0f, 0x5a, 0xda],
        0x7dc0,
        &[
            (3, 0x8f0e64659b83b0f1736c7e3ac73b5063),
            (10, 0x7fc0000100ffffff402000003f800000),
        ],
        Expect::Ok(0x40040000000000003ff0000000000000, 0x7dc0),
    ),
    (
        "rg_cvtps2pd_34_29",
        &[0x41, 0x0f, 0x5a, 0xcb],
        0x7b80,
        &[
            (1, 0x9a01c6ec3ab8e8bba59d1415fd6378e9),
            (11, 0xbf000000bf000000808000007e800000),
        ],
        Expect::Ok(0xb81000000000000047d0000000000000, 0x7b80),
    ),
    (
        "rg_cvtps2pd_35_59",
        &[0x45, 0x0f, 0x5a, 0xfc],
        0x8000,
        &[
            (12, 0x007fffff3f000000807fffff3e800000),
            (15, 0x8d2c57bbb7f196cb3081ad22b173eb04),
        ],
        Expect::Fault(0x8002),
    ),
    (
        "rg_cvtps2pd_36_21",
        &[0x0f, 0x5a, 0xf4],
        0x5e80,
        &[
            (4, 0x7f800001ff8000000040000034000000),
            (6, 0x9fe90c699a7c3f1e405c78ae398ec52f),
        ],
        Expect::Fault(0x5e82),
    ),
    (
        "rg_cvtps2pd_36_28",
        &[0x41, 0x0f, 0x5a, 0xd5],
        0x7d80,
        &[
            (2, 0x74e3168c87d5aacfcffa34f45ba6700e),
            (13, 0x402000004b7fffff7fa0000034000000),
        ],
        Expect::Ok(0x7ffc0000000000003e80000000000000, 0x7d81),
    ),
    (
        "rg_cvtps2pd_36_32",
        &[0x45, 0x0f, 0x5a, 0xd5],
        0x9f00,
        &[
            (10, 0xe68d46a0eeef7d33482fff03e429d78c),
            (13, 0x33800000bf8000007f80000134000000),
        ],
        Expect::Fault(0x9f01),
    ),
    (
        "rg_cvtps2pd_36_41",
        &[0x41, 0x0f, 0x5a, 0xee],
        0x17c0,
        &[
            (5, 0x7df8ab50726155bff6cc8167e6a1003c),
            (14, 0x3fc0000000ffffff7f80000034000000),
        ],
        Expect::Ok(0x7ff00000000000003e80000000000000, 0x17c0),
    ),
    (
        "rg_cvtps2pd_36_59",
        &[0x41, 0x0f, 0x5a, 0xe9],
        0x8000,
        &[
            (5, 0x47b80e4450fb9f11ced1f1e19c54b4bd),
            (9, 0x7f7fffff400000003fc0000034000000),
        ],
        Expect::Ok(0x3ff80000000000003e80000000000000, 0x8000),
    ),
    (
        "rg_cvtps2pd_38_17",
        &[0x44, 0x0f, 0x5a, 0xe2],
        0x3b80,
        &[
            (2, 0x7effffff00ffffffbe8000004b000000),
            (12, 0x478d159065ca6219038be48cda2d4437),
        ],
        Expect::Ok(0xbfd00000000000004160000000000000, 0x3b80),
    ),
    (
        "rg_cvtps2pd_38_8",
        &[0x44, 0x0f, 0x5a, 0xd8],
        0x1f00,
        &[
            (0, 0x0080000040a00000000000034b000000),
            (11, 0x1f34bc4db5fd6522ea43e45c16d6eb4e),
        ],
        Expect::Ok(0x36b80000000000004160000000000000, 0x1f02),
    ),
    (
        "rg_cvtps2pd_39_56",
        &[0x44, 0x0f, 0x5a, 0xe0],
        0x2000,
        &[
            (0, 0xffc1234500000000807fffff4b7fffff),
            (12, 0x85c92b99e3004311b713b4ac327b9bab),
        ],
        Expect::Fault(0x2002),
    ),
    (
        "rg_cvtps2pd_41_38",
        &[0x45, 0x0f, 0x5a, 0xdb],
        0x9b80,
        &[(11, 0xff8000007f800001000000007effffff)],
        Expect::Ok(0x000000000000000047dfffffe0000000, 0x9b80),
    ),
    (
        "rg_cvtps2pd_44_55",
        &[0x45, 0x0f, 0x5a, 0xc1],
        0x6fc0,
        &[
            (8, 0xedd6399d4038299c85e5798ba61c66a1),
            (9, 0x80000001808000007f80000140a00000),
        ],
        Expect::Ok(0x7ff80000200000004014000000000000, 0x6fc1),
    ),
    (
        "rg_cvtps2pd_45_18",
        &[0x41, 0x0f, 0x5a, 0xfd],
        0x3780,
        &[
            (7, 0x9fd1016d21d3383c6a2b6097340693a4),
            (13, 0x008000004aaaaaab3f4000003fc00000),
        ],
        Expect::Ok(0x3fe80000000000003ff8000000000000, 0x3780),
    ),
    (
        "rg_cvtps2pd_45_52",
        &[0x41, 0x0f, 0x5a, 0xcf],
        0xf780,
        &[
            (1, 0x0dd42f18747d9531a1aeed98d1047c49),
            (15, 0xbe8000003f7fffff000000003fc00000),
        ],
        Expect::Ok(0x00000000000000003ff8000000000000, 0xf780),
    ),
    (
        "rg_cvtps2pd_47_6",
        &[0x0f, 0x5a, 0xd7],
        0xffc0,
        &[
            (2, 0x6e592c4e86cf3c1b3b3165ccac54c76d),
            (7, 0x7fc000017fa000007fbfffffbfc00000),
        ],
        Expect::Ok(0x7fffffffe0000000bff8000000000000, 0xffc1),
    ),
    (
        "rg_cvtps2pd_48_44",
        &[0x44, 0x0f, 0x5a, 0xdb],
        0xff00,
        &[
            (3, 0x3f0000007f7fffff007fffffc0200000),
            (11, 0x4b92923604eb1787f0a47c68bc19b388),
        ],
        Expect::Ok(0x380fffffc0000000c004000000000000, 0xff02),
    ),
    (
        "rg_cvtps2pd_48_50",
        &[0x45, 0x0f, 0x5a, 0xdf],
        0xfb80,
        &[
            (11, 0x8b4e570663416283693b0e23a27212af),
            (15, 0x4b000000ff800000bfc00000c0200000),
        ],
        Expect::Ok(0xbff8000000000000c004000000000000, 0xfb80),
    ),
    (
        "rg_cvtps2pd_49_44",
        &[0x45, 0x0f, 0x5a, 0xd5],
        0xff00,
        &[
            (10, 0x93d644255dcf902ec28c503001323097),
            (13, 0xc0200000bf0000007fbfffffbf000000),
        ],
        Expect::Fault(0xff01),
    ),
    (
        "rg_cvtps2pd_50_63",
        &[0x45, 0x0f, 0x5a, 0xc4],
        0x1f95,
        &[
            (8, 0x9b798fef0cbd5acdca8c7808658f263d),
            (12, 0x3fc000007fc0000100800000bf7fffff),
        ],
        Expect::Ok(0x3810000000000000bfefffffe0000000, 0x1f95),
    ),
    (
        "rg_cvtps2pd_55_34",
        &[0x45, 0x0f, 0x5a, 0xf4],
        0x9e80,
        &[
            (12, 0x7f80000140200000000000034aaaaaab),
            (14, 0x0f919d05e049128bd7559846ce642b0e),
        ],
        Expect::Fault(0x9e82),
    ),
    (
        "rg_cvtps2pd_55_50",
        &[0x0f, 0x5a, 0xf1],
        0xfb80,
        &[
            (1, 0x800000017effffff007fffff4aaaaaab),
            (6, 0xe0049a9e607ea08b631e0898509f20c3),
        ],
        Expect::Ok(0x380fffffc00000004155555560000000, 0xfb82),
    ),
    (
        "rg_cvtps2pd_56_59",
        &[0x45, 0x0f, 0x5a, 0xeb],
        0x8000,
        &[
            (11, 0x3e8000007fbfffff00800001be800000),
            (13, 0xd77415cd3b312f58caa6b501c3fc098f),
        ],
        Expect::Ok(0x3810000020000000bfd0000000000000, 0x8000),
    ),
    (
        "rg_cvtps2pd_58_42",
        &[0x44, 0x0f, 0x5a, 0xe1],
        0x8f80,
        &[
            (1, 0x80000001bf800000007fffffc2f6e979),
            (12, 0xc2224190a56a25205bce22eb0b04f5fa),
        ],
        Expect::Ok(0x380fffffc0000000c05edd2f20000000, 0x8f82),
    ),
    (
        "rg_cvtps2pd_58_61",
        &[0x0f, 0x5a, 0xfc],
        0x80c0,
        &[
            (4, 0xcb000001ffc000007fc00000c2f6e979),
            (7, 0x1ae22c49a8db52890f878208f1ae59b8),
        ],
        Expect::Ok(0x7ff8000000000000c05edd2f20000000, 0x80c0),
    ),
    (
        "rg_cvtps2pd_9_54",
        &[0x0f, 0x5a, 0xe0],
        0xef80,
        &[
            (0, 0xffc1234540400000000000013f7fffff),
            (4, 0x0f73c3ca05a5148cc338c5671425aab4),
        ],
        Expect::Ok(0x36a00000000000003fefffffe0000000, 0xef82),
    ),
    (
        "rg_cvtsd2ss_0_71",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xf3],
        0x1f00,
        &[
            (6, 0x956d476a32eac5fbd7a6a9a10e2b1323),
            (11, 0xaef377a9112a9c1e0000000000000000),
        ],
        Expect::Ok(0x956d476a32eac5fbd7a6a9a100000000, 0x1f00),
    ),
    (
        "rg_cvtsd2ss_10_19",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc4],
        0x2f80,
        &[
            (4, 0x1ac62dc276c89c497ff0000000000000),
            (8, 0x35042e19899e95ad962c6d30e81e9aa6),
        ],
        Expect::Ok(0x35042e19899e95ad962c6d307f800000, 0x2f80),
    ),
    (
        "rg_cvtsd2ss_11_22",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf3],
        0x5d80,
        &[
            (11, 0x3c819a0d58cae91efff0000000000000),
            (14, 0x0450e8e6f8e83577480f6abcd3df6979),
        ],
        Expect::Ok(0x0450e8e6f8e83577480f6abcff800000, 0x5d80),
    ),
    (
        "rg_cvtsd2ss_11_40",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc2],
        0x9780,
        &[
            (8, 0xcd40b1cf4a3b5065fa66826c0af8c589),
            (10, 0x2589ca5f6796d944fff0000000000000),
        ],
        Expect::Ok(0xcd40b1cf4a3b5065fa66826cff800000, 0x9780),
    ),
    (
        "rg_cvtsd2ss_12_16",
        &[0xf2, 0x0f, 0x5a, 0xd7],
        0x3d80,
        &[
            (2, 0x8db950d675d146cad04e8e10b05fc1f5),
            (7, 0x3248632b6b184a467ff8000000000000),
        ],
        Expect::Ok(0x8db950d675d146cad04e8e107fc00000, 0x3d80),
    ),
    (
        "rg_cvtsd2ss_12_30",
        &[0xf2, 0x0f, 0x5a, 0xca],
        0x7780,
        &[
            (1, 0xf54508dd0018ead617688e17762d7927),
            (2, 0xbfafb41b54c323947ff8000000000000),
        ],
        Expect::Ok(0xf54508dd0018ead617688e177fc00000, 0x7780),
    ),
    (
        "rg_cvtsd2ss_12_58",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd0],
        0x6000,
        &[
            (2, 0x42bc170055515e10ee09cca4965f6562),
            (8, 0x085e3c12c69729e27ff8000000000000),
        ],
        Expect::Ok(0x42bc170055515e10ee09cca47fc00000, 0x6000),
    ),
    (
        "rg_cvtsd2ss_13_4",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd6],
        0x1fc0,
        &[
            (10, 0x5ec59e8424dbae298f0d2ec3744cf369),
            (14, 0x0ac4c090323c487ffff8000000000000),
        ],
        Expect::Ok(0x5ec59e8424dbae298f0d2ec3ffc00000, 0x1fc0),
    ),
    (
        "rg_cvtsd2ss_13_48",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xfe],
        0xfd80,
        &[
            (7, 0x6ca9efc2795f6d38a34e9bb28e3719f3),
            (14, 0x413d917a7c6b447efff8000000000000),
        ],
        Expect::Ok(0x6ca9efc2795f6d38a34e9bb2ffc00000, 0xfd80),
    ),
    (
        "rg_cvtsd2ss_15_40",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd2],
        0x9780,
        &[
            (2, 0xf6f8cb46253b99c17fffffffffffffff),
            (10, 0x97af444b8cc118ce3a1353f29dd29fec),
        ],
        Expect::Ok(0x97af444b8cc118ce3a1353f27fffffff, 0x9780),
    ),
    (
        "rg_cvtsd2ss_18_53",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xcc],
        0x77c0,
        &[
            (9, 0x5e6ea8de0e415857626766068e48000f),
            (12, 0xbc94044a684e06e3fff0000000000001),
        ],
        Expect::Ok(0x5e6ea8de0e41585762676606ffc00000, 0x77c1),
    ),
    (
        "rg_cvtsd2ss_19_4",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe4],
        0x1fc0,
        &[
            (4, 0x26e512b5c916ff7b7ff4000000000000),
            (12, 0xcd5e948bab724933137a50c7ba1bbc20),
        ],
        Expect::Ok(0xcd5e948bab724933137a50c77fe00000, 0x1fc1),
    ),
    (
        "rg_cvtsd2ss_20_51",
        &[0xf2, 0x0f, 0x5a, 0xcb],
        0x7bc0,
        &[
            (1, 0xa9d9b00b7ea7fb0752027601bd38c65c),
            (3, 0x710bcc2ede30bca27ff7ffffffffffff),
        ],
        Expect::Ok(0xa9d9b00b7ea7fb07520276017fffffff, 0x7bc1),
    ),
    (
        "rg_cvtsd2ss_20_56",
        &[0xf2, 0x0f, 0x5a, 0xcf],
        0x2000,
        &[
            (1, 0x17c863d870f4dc211b0e4aa90ab68275),
            (7, 0xa55373db6c97b36d7ff7ffffffffffff),
        ],
        Expect::Fault(0x2001),
    ),
    (
        "rg_cvtsd2ss_21_26",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd1],
        0x7f00,
        &[
            (2, 0xca007d876232fe988ba80fde4202d03b),
            (9, 0x72c011b6103bae3bfff7ffffffffffff),
        ],
        Expect::Fault(0x7f01),
    ),
    (
        "rg_cvtsd2ss_22_34",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc5],
        0x9e80,
        &[
            (8, 0x33812ab6e1d517f615dce7cecaadae77),
            (13, 0x74b718ec700d423d0000000000000001),
        ],
        Expect::Fault(0x9e82),
    ),
    (
        "rg_cvtsd2ss_24_40",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc5],
        0x9780,
        &[
            (8, 0xc6df6e2df93c520f649b49dbba5df72c),
            (13, 0x2a862afe08beafac000fffffffffffff),
        ],
        Expect::Fault(0x97b2),
    ),
    (
        "rg_cvtsd2ss_24_5",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xf7],
        0x9f80,
        &[
            (6, 0xfb3d2076c5bf0d18520a06e17afbaf2f),
            (15, 0xe3b46318f5fc29fe000fffffffffffff),
        ],
        Expect::Ok(0xfb3d2076c5bf0d18520a06e100000000, 0x9fb2),
    ),
    (
        "rg_cvtsd2ss_24_55",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf3],
        0x6fc0,
        &[
            (11, 0x3436accce84fe131000fffffffffffff),
            (14, 0x89fac3fb0e2f1bf47f0d5adc62f2c826),
        ],
        Expect::Ok(0x89fac3fb0e2f1bf47f0d5adc00000000, 0x6fc0),
    ),
    (
        "rg_cvtsd2ss_25_43",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd9],
        0xfc0,
        &[
            (1, 0x670fdc928bbd5c5e800fffffffffffff),
            (11, 0x3b433da7560f967cc464e78a6696e9df),
        ],
        Expect::Ok(0x3b433da7560f967cc464e78a80000000, 0xfc0),
    ),
    (
        "rg_cvtsd2ss_27_47",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe2],
        0x7ec0,
        &[
            (4, 0xa970b30d5e619567201bc3903434fb7b),
            (10, 0x4d5d9a9423f1ec080000000000000003),
        ],
        Expect::Ok(0xa970b30d5e619567201bc39000000000, 0x7ec0),
    ),
    (
        "rg_cvtsd2ss_28_69",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc4],
        0x1780,
        &[
            (0, 0x86cc529f21d22a90f247bc9e9efa1c54),
            (12, 0x579b2abd050d0d0c0010000000000000),
        ],
        Expect::Fault(0x1790),
    ),
    (
        "rg_cvtsd2ss_2_59",
        &[0xf2, 0x0f, 0x5a, 0xee],
        0x8000,
        &[
            (5, 0x2c4707b18c4665d5a5531313473283f7),
            (6, 0x8a68bb8d012656f43ff0000000000000),
        ],
        Expect::Ok(0x2c4707b18c4665d5a55313133f800000, 0x8000),
    ),
    (
        "rg_cvtsd2ss_30_53",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xee],
        0x77c0,
        &[
            (5, 0x031ad96de57e0424e45612a277d92511),
            (14, 0x53c191cbb8c785e60010000000000001),
        ],
        Expect::Fault(0x77f0),
    ),
    (
        "rg_cvtsd2ss_31_30",
        &[0xf2, 0x0f, 0x5a, 0xd1],
        0x7780,
        &[
            (1, 0x4495c42e9e6ac50a7fefffffffffffff),
            (2, 0x665a12ff1ef38629ea8d22020e80068b),
        ],
        Expect::Ok(0x665a12ff1ef38629ea8d22027f7fffff, 0x77a8),
    ),
    (
        "rg_cvtsd2ss_31_57",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf6],
        0x4000,
        &[(14, 0x47cc007c7e1c11e37fefffffffffffff)],
        Expect::Fault(0x4028),
    ),
    (
        "rg_cvtsd2ss_32_70",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xed],
        0xf80,
        &[(13, 0xe7cf74e4cabea0feffefffffffffffff)],
        Expect::Fault(0xfa8),
    ),
    (
        "rg_cvtsd2ss_35_3",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf4],
        0x7f80,
        &[
            (4, 0xb7535822d69b81843fd0000000000000),
            (14, 0x85143666a467f3c69b46aa72910ca59e),
        ],
        Expect::Ok(0x85143666a467f3c69b46aa723e800000, 0x7f80),
    ),
    (
        "rg_cvtsd2ss_36_50",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd0],
        0xfb80,
        &[
            (0, 0x4c7f5aad88ac2aa83cb0000000000000),
            (10, 0x9b6d6d2c074f3673f051ee0cf27ffb69),
        ],
        Expect::Ok(0x9b6d6d2c074f3673f051ee0c25800000, 0xfb80),
    ),
    (
        "rg_cvtsd2ss_38_10",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf7],
        0x1d80,
        &[
            (14, 0x6fbf6d829f9e90d9a4038cb77b0f2fc0),
            (15, 0xb851d29df486aebd4330000000000000),
        ],
        Expect::Ok(0x6fbf6d829f9e90d9a4038cb759800000, 0x1d80),
    ),
    (
        "rg_cvtsd2ss_38_17",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe9],
        0x3b80,
        &[
            (9, 0x5cbd4d1d667bfcbb4330000000000000),
            (13, 0x8e546b0d6b7acf73c5165cddb0bc05a5),
        ],
        Expect::Ok(0x8e546b0d6b7acf73c5165cdd59800000, 0x3b80),
    ),
    (
        "rg_cvtsd2ss_39_24",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xec],
        0x5780,
        &[
            (5, 0x76a260062bbc16c05d55d44fdea9a150),
            (12, 0x7e8cbf67854e3874433fffffffffffff),
        ],
        Expect::Ok(0x76a260062bbc16c05d55d44f5a000000, 0x57a0),
    ),
    (
        "rg_cvtsd2ss_40_21",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xde],
        0x5e80,
        &[
            (6, 0xa45b3318771b64004340000000000000),
            (11, 0x738b28edd4ee442be0733c52d4bd69ab),
        ],
        Expect::Ok(0x738b28edd4ee442be0733c525a000000, 0x5e80),
    ),
    (
        "rg_cvtsd2ss_43_52",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe9],
        0xf780,
        &[
            (1, 0xd8843a5174c26225001fffffffffffff),
            (13, 0x3811443b4a48f0ecb21b145e6e70bf6a),
        ],
        Expect::Fault(0xf7b0),
    ),
    (
        "rg_cvtsd2ss_50_15",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xcd],
        0x3e80,
        &[
            (5, 0x7578ac7ac3512976bfefffffffffffff),
            (9, 0x78b26a93985b80b4dbfe5324afbf70f2),
        ],
        Expect::Ok(0x78b26a93985b80b4dbfe5324bf800000, 0x3ea0),
    ),
    (
        "rg_cvtsd2ss_50_31",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xdc],
        0x6f80,
        &[
            (3, 0xaa69b258eef672d97bda25166abcf06b),
            (12, 0x49d90087398bcd84bfefffffffffffff),
        ],
        Expect::Fault(0x6fa0),
    ),
    (
        "rg_cvtsd2ss_50_37",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe0],
        0x1dc0,
        &[
            (4, 0xed32b89cb956d090e9d10983f55f9c72),
            (8, 0x83f467c25e6d2a00bfefffffffffffff),
        ],
        Expect::Ok(0xed32b89cb956d090e9d10983bf800000, 0x1de0),
    ),
    (
        "rg_cvtsd2ss_50_46",
        &[0xf2, 0x0f, 0x5a, 0xd4],
        0xfe80,
        &[
            (2, 0x5e9e4e23b7f67192e548ea075530078c),
            (4, 0x31965279b6046fcabfefffffffffffff),
        ],
        Expect::Ok(0x5e9e4e23b7f67192e548ea07bf7fffff, 0xfea0),
    ),
    (
        "rg_cvtsd2ss_51_55",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xff],
        0x6fc0,
        &[
            (7, 0x631ae82edc33267faaed1de06c0e8a89),
            (15, 0xc1afce52a28091053fdfffffffffffff),
        ],
        Expect::Fault(0x6fe0),
    ),
    (
        "rg_cvtsd2ss_52_54",
        &[0xf2, 0x0f, 0x5a, 0xf8],
        0xef80,
        &[
            (0, 0x302ec9cbbaa7e1894330000000000001),
            (7, 0xe4d1ddb8f2bc1894b866938e2b510476),
        ],
        Expect::Fault(0xefa0),
    ),
    (
        "rg_cvtsd2ss_54_55",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xfa],
        0x6fc0,
        &[
            (10, 0x5e7ceff051061e36432ffffffffffffe),
            (15, 0xe2d88fcf7d80d57af8e2554a6340f1a9),
        ],
        Expect::Fault(0x6fe0),
    ),
    (
        "rg_cvtsd2ss_54_62",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd5],
        0x1fbf,
        &[
            (5, 0x51605b9feb10baee432ffffffffffffe),
            (10, 0x412f03ed16edf1840670de12980073de),
        ],
        Expect::Ok(0x412f03ed16edf1840670de1259800000, 0x1fbf),
    ),
    (
        "rg_cvtsd2ss_54_65",
        &[0xf2, 0x0f, 0x5a, 0xc6],
        0xfbf,
        &[
            (0, 0x7d7ef298467a78c3a1d377ac5f5ae247),
            (6, 0x64c2be8b47f2ad81432ffffffffffffe),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rg_cvtsd2ss_54_7",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd9],
        0x0,
        &[
            (1, 0x86575facaa0fec70432ffffffffffffe),
            (11, 0x55222673842abc98c8242d720dd105d9),
        ],
        Expect::Fault(0x20),
    ),
    (
        "rg_cvtsd2ss_56_44",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xf1],
        0xff00,
        &[
            (6, 0xe5593181e5563c03dd2f8a10b4784934),
            (9, 0xda6621f0a256ec9cbfd0000000000000),
        ],
        Expect::Ok(0xe5593181e5563c03dd2f8a10be800000, 0xff00),
    ),
    (
        "rg_cvtsd2ss_58_29",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xf5],
        0x7b80,
        &[
            (6, 0x61b5dcb5ebf0081290e2914ac8b2cba6),
            (13, 0x888dd390955304eac05edd2f1a9fbe77),
        ],
        Expect::Ok(0x61b5dcb5ebf0081290e2914ac2f6e978, 0x7ba0),
    ),
    (
        "rg_cvtsd2ss_5_3",
        &[0xf2, 0x0f, 0x5a, 0xd2],
        0x7f80,
        &[(2, 0xe3e2cb79cc0a047cc000000000000000)],
        Expect::Ok(0xe3e2cb79cc0a047cc0000000c0000000, 0x7f80),
    ),
    (
        "rg_cvtsd2ss_8_59",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xfe],
        0x8000,
        &[
            (6, 0x5b315c14077d09233ff0000000000001),
            (15, 0x978e3095638a316de6ae1b9ab0683901),
        ],
        Expect::Fault(0x8020),
    ),
    (
        "rg_cvtss2sd_10_1",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xed],
        0x3f80,
        &[(13, 0x58fcd14addb44a0f5eb5fd7c7f800000)],
        Expect::Ok(0x58fcd14addb44a0f7ff0000000000000, 0x3f80),
    ),
    (
        "rg_cvtss2sd_10_53",
        &[0xf3, 0x0f, 0x5a, 0xd9],
        0x77c0,
        &[
            (1, 0xe693e9c4e495301346f7741b7f800000),
            (3, 0x3921a433fb8b4d242182afe044c5e9cf),
        ],
        Expect::Ok(0x3921a433fb8b4d247ff0000000000000, 0x77c0),
    ),
    (
        "rg_cvtss2sd_11_56",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf9],
        0x2000,
        &[
            (9, 0x4a796887db330abf2a75beb9ff800000),
            (15, 0x85519ba3ececbb0aa58270ab6fd3d2a4),
        ],
        Expect::Ok(0x85519ba3ececbb0afff0000000000000, 0x2000),
    ),
    (
        "rg_cvtss2sd_13_57",
        &[0xf3, 0x0f, 0x5a, 0xc5],
        0x4000,
        &[
            (0, 0x01a3a8d6c4b8d31b9b1769c8fa800bbf),
            (5, 0xd872dd29548f6726be900419ffc00000),
        ],
        Expect::Ok(0x01a3a8d6c4b8d31bfff8000000000000, 0x4000),
    ),
    (
        "rg_cvtss2sd_15_5",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc2],
        0x9f80,
        &[
            (8, 0x4ab289e2cae4e2eb517bd617bd1c7e67),
            (10, 0xf2f9e90cb0cb8fe34468bd037fffffff),
        ],
        Expect::Ok(0x4ab289e2cae4e2eb7fffffffe0000000, 0x9f80),
    ),
    (
        "rg_cvtss2sd_15_58",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf8],
        0x6000,
        &[
            (8, 0x07bd98a787f3cec2a8bb16997fffffff),
            (15, 0x029f2d656999514781e850b081e0b35b),
        ],
        Expect::Ok(0x029f2d65699951477fffffffe0000000, 0x6000),
    ),
    (
        "rg_cvtss2sd_15_71",
        &[0xf3, 0x0f, 0x5a, 0xe7],
        0x1f00,
        &[
            (4, 0xf0403db9e12704d5af594a896e74aea7),
            (7, 0x3ba543199f0239c7eb961f987fffffff),
        ],
        Expect::Ok(0xf0403db9e12704d57fffffffe0000000, 0x1f00),
    ),
    (
        "rg_cvtss2sd_16_72",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc5],
        0x1700,
        &[
            (5, 0x1d1c36172f4cb0494ae562b2ffc12345),
            (8, 0x8d28d27179542d1f0cbece39805cc07a),
        ],
        Expect::Ok(0x8d28d27179542d1ffff82468a0000000, 0x1700),
    ),
    (
        "rg_cvtss2sd_17_13",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xfb],
        0xf80,
        &[
            (7, 0x7a06387849108b9c0d22c7fe86fd7199),
            (11, 0x9b7c4b86f3d3d8d6d92c467f7f800001),
        ],
        Expect::Ok(0x7a06387849108b9c7ff8000020000000, 0xf81),
    ),
    (
        "rg_cvtss2sd_17_37",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe7],
        0x1dc0,
        &[
            (4, 0xcfd4f51b10e83602cda492d3002c4e82),
            (15, 0xb0eb08da2110339b2f7d9b517f800001),
        ],
        Expect::Ok(0xcfd4f51b10e836027ff8000020000000, 0x1dc1),
    ),
    (
        "rg_cvtss2sd_18_16",
        &[0xf3, 0x0f, 0x5a, 0xd5],
        0x3d80,
        &[
            (2, 0xd9df6b8a252eb9be234c3f7ef4755035),
            (5, 0xb8a4e22291584187da2b0258ff800001),
        ],
        Expect::Ok(0xd9df6b8a252eb9befff8000020000000, 0x3d81),
    ),
    (
        "rg_cvtss2sd_18_25",
        &[0xf3, 0x0f, 0x5a, 0xe7],
        0x4f80,
        &[
            (4, 0xbe6044baa01d12151db73c225b234da0),
            (7, 0xfe14f75c16397cc863068033ff800001),
        ],
        Expect::Ok(0xbe6044baa01d1215fff8000020000000, 0x4f81),
    ),
    (
        "rg_cvtss2sd_18_32",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xdd],
        0x9f00,
        &[
            (11, 0xf7cf91c2ceed0c056b84a37424d658b0),
            (13, 0x4df17cfe1701ff5b899f775cff800001),
        ],
        Expect::Fault(0x9f01),
    ),
    (
        "rg_cvtss2sd_18_44",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd1],
        0xff00,
        &[
            (1, 0xd04da0e47fa705211decfabcff800001),
            (10, 0x4076baecf878e165b20a77408c6477bf),
        ],
        Expect::Fault(0xff01),
    ),
    (
        "rg_cvtss2sd_18_72",
        &[0xf3, 0x0f, 0x5a, 0xda],
        0x1700,
        &[
            (2, 0x46f523e03c0072141b1df84cff800001),
            (3, 0x17f930480e0f6afbb5f52cdb0045871f),
        ],
        Expect::Fault(0x1701),
    ),
    (
        "rg_cvtss2sd_19_26",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe0],
        0x7f00,
        &[
            (4, 0xf7011166b583f38f721703cc5308ce7e),
            (8, 0xd24b19f87099c0c83cfc28fd7fa00000),
        ],
        Expect::Fault(0x7f01),
    ),
    (
        "rg_cvtss2sd_19_5",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd1],
        0x9f80,
        &[
            (9, 0x996829d5145bd7ab61e1035d7fa00000),
            (10, 0x9ff83a9f1ed5d811889475d4944f13a8),
        ],
        Expect::Ok(0x9ff83a9f1ed5d8117ffc000000000000, 0x9f81),
    ),
    (
        "rg_cvtss2sd_19_54",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xdc],
        0xef80,
        &[
            (4, 0xfc4a7266ac50547e6e6ae1ca7fa00000),
            (11, 0xb7537520825f83b2143c8580a4b97d46),
        ],
        Expect::Ok(0xb7537520825f83b27ffc000000000000, 0xef81),
    ),
    (
        "rg_cvtss2sd_19_56",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xe9],
        0x2000,
        &[
            (1, 0xae9b6441e6c470380f9d2d677fa00000),
            (13, 0xed473929810090c038bc6d515a3f13e4),
        ],
        Expect::Fault(0x2001),
    ),
    (
        "rg_cvtss2sd_19_8",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf7],
        0x1f00,
        &[
            (14, 0x32d6dbb428a0102e7ade3f8d54329c7f),
            (15, 0x123a8dcf1364254d5daeedd37fa00000),
        ],
        Expect::Fault(0x1f01),
    ),
    (
        "rg_cvtss2sd_20_14",
        &[0xf3, 0x0f, 0x5a, 0xde],
        0x3f00,
        &[
            (3, 0x743867e3b42aeed794b379d7d1928c3e),
            (6, 0x81e9b2c457ea0eb0a4eac6ed7fbfffff),
        ],
        Expect::Fault(0x3f01),
    ),
    (
        "rg_cvtss2sd_20_27",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xec],
        0x7e80,
        &[
            (4, 0xbbfc55118672c336d71d5b6f7fbfffff),
            (13, 0x5df96342cefe934dcd6584f0cd5c3918),
        ],
        Expect::Ok(0x5df96342cefe934d7fffffffe0000000, 0x7e81),
    ),
    (
        "rg_cvtss2sd_20_3",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xfe],
        0x7f80,
        &[
            (6, 0x1ac599fa3802a8c7a71d582e7fbfffff),
            (15, 0x47b7359657a033a0e08d926c2c78bcc5),
        ],
        Expect::Ok(0x47b7359657a033a07fffffffe0000000, 0x7f81),
    ),
    (
        "rg_cvtss2sd_20_37",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xd9],
        0x1dc0,
        &[
            (3, 0x58b79644213909b35c84fc32d191d37f),
            (9, 0x30406421e89b732302800f027fbfffff),
        ],
        Expect::Ok(0x58b79644213909b37fffffffe0000000, 0x1dc1),
    ),
    (
        "rg_cvtss2sd_20_6",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xf3],
        0xffc0,
        &[
            (3, 0x436337e7f9150ab4402a8bad7fbfffff),
            (14, 0x8c06c0d7c43f534e7b97603f18353c35),
        ],
        Expect::Ok(0x8c06c0d7c43f534e7fffffffe0000000, 0xffc1),
    ),
    (
        "rg_cvtss2sd_20_60",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xd8],
        0x40,
        &[
            (3, 0x9c5d8e8d2e8638f17a6e4fb143f0268d),
            (8, 0xa05b229d1753f266e79a0a737fbfffff),
        ],
        Expect::Fault(0x41),
    ),
    (
        "rg_cvtss2sd_21_13",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc4],
        0xf80,
        &[
            (4, 0x9c38c50d79546df0c6df3f3bffbfffff),
            (8, 0xe6b5efead3b5c556c09700929eb0a223),
        ],
        Expect::Ok(0xe6b5efead3b5c556ffffffffe0000000, 0xf81),
    ),
    (
        "rg_cvtss2sd_21_20",
        &[0xf3, 0x0f, 0x5a, 0xc8],
        0x5f00,
        &[
            (0, 0xcc9518e438568a6f0750e6a6ffbfffff),
            (1, 0x2b3c0b7425cfb1de9f672952f31ac09b),
        ],
        Expect::Fault(0x5f01),
    ),
    (
        "rg_cvtss2sd_21_32",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xee],
        0x9f00,
        &[
            (5, 0x6e739eb184ccf57a97ea3a252075f5a6),
            (14, 0xa95dae869306f6ee63efd1cdffbfffff),
        ],
        Expect::Fault(0x9f01),
    ),
    (
        "rg_cvtss2sd_21_45",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xfd],
        0x7f40,
        &[
            (5, 0xa61c0e3352fc5ad49c5e18e4ffbfffff),
            (15, 0xa470066614f2b6a23369ce0d04a5586f),
        ],
        Expect::Fault(0x7f41),
    ),
    (
        "rg_cvtss2sd_21_57",
        &[0xf3, 0x0f, 0x5a, 0xe7],
        0x4000,
        &[
            (4, 0xb2c82610e3ddf8a769259795d91e1e07),
            (7, 0xfac3000ef75296c4731c6749ffbfffff),
        ],
        Expect::Fault(0x4001),
    ),
    (
        "rg_cvtss2sd_22_0",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc9],
        0x1f80,
        &[
            (1, 0x3ebf77126a2c9f495ac1cfdd00000001),
            (9, 0x1c8f62c135f617aabb1e94344167aac0),
        ],
        Expect::Ok(0x1c8f62c135f617aa36a0000000000000, 0x1f82),
    ),
    (
        "rg_cvtss2sd_22_27",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xca],
        0x7e80,
        &[
            (2, 0xf6000aa977188403a686f9e500000001),
            (9, 0x86cabf9a24a98677c23e2d82b459e59c),
        ],
        Expect::Fault(0x7e82),
    ),
    (
        "rg_cvtss2sd_22_46",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xdd],
        0xfe80,
        &[
            (5, 0x6c75114e7eb118849c1ce48b00000001),
            (11, 0xd9799422c5a81be05db50f0efd476d0d),
        ],
        Expect::Fault(0xfe82),
    ),
    (
        "rg_cvtss2sd_22_67",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc0],
        0x1d80,
        &[
            (0, 0x77c56207a88572bfb77d31c800000001),
            (8, 0xde4e186d817ad078c4d63b757afd54d0),
        ],
        Expect::Ok(0xde4e186d817ad07836a0000000000000, 0x1d82),
    ),
    (
        "rg_cvtss2sd_23_14",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xcd],
        0x3f00,
        &[
            (9, 0x6832ddf0b532f1d19e26a72777cc836b),
            (13, 0x4efa168f943460f5132f608380000001),
        ],
        Expect::Ok(0x6832ddf0b532f1d1b6a0000000000000, 0x3f02),
    ),
    (
        "rg_cvtss2sd_23_56",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc5],
        0x2000,
        &[
            (8, 0x11663b6ecac3457365f3808231c2a3b1),
            (13, 0x7c6958e068004ed5922b8f3f80000001),
        ],
        Expect::Fault(0x2002),
    ),
    (
        "rg_cvtss2sd_24_58",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xcf],
        0x6000,
        &[
            (7, 0x06322f6e7891c43ec6379948007fffff),
            (9, 0xd248bb2c116edaad345e7d6240eb9e21),
        ],
        Expect::Fault(0x6002),
    ),
    (
        "rg_cvtss2sd_25_15",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe3],
        0x3e80,
        &[
            (4, 0x6de23076b3532caf2208757a5d906929),
            (11, 0x2ce7b9ecfa9f57a6d8aa0a75807fffff),
        ],
        Expect::Fault(0x3e82),
    ),
    (
        "rg_cvtss2sd_26_44",
        &[0xf3, 0x0f, 0x5a, 0xf5],
        0xff00,
        &[
            (5, 0x39eea6129a444d87f30cb0de00400000),
            (6, 0x7f6044077e5e8e6ad267420a9a658d65),
        ],
        Expect::Ok(0x7f6044077e5e8e6a3800000000000000, 0xff02),
    ),
    (
        "rg_cvtss2sd_26_54",
        &[0xf3, 0x0f, 0x5a, 0xdc],
        0xef80,
        &[
            (3, 0xd319d87c60c6401ef04969db0c70d330),
            (4, 0x0c362229290229126c29df9400400000),
        ],
        Expect::Ok(0xd319d87c60c6401e3800000000000000, 0xef82),
    ),
    (
        "rg_cvtss2sd_26_59",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xd6],
        0x8000,
        &[
            (2, 0x7cff48b6706214f9926c74ffcb6f6fa6),
            (14, 0xefc013792df5d0c953c0264a00400000),
        ],
        Expect::Fault(0x8002),
    ),
    (
        "rg_cvtss2sd_28_56",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xfd],
        0x2000,
        &[
            (7, 0x3d57a32b6c3d3ceefdad4f4656d5c8cd),
            (13, 0x3f6f24d8da0f55630cd0aef000800000),
        ],
        Expect::Ok(0x3d57a32b6c3d3cee3810000000000000, 0x2000),
    ),
    (
        "rg_cvtss2sd_35_60",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xcd],
        0x40,
        &[
            (9, 0xe4dd776a7f874fb68e2cc5db1f639f62),
            (13, 0x8ffb40fff2bd49d35479beaa3e800000),
        ],
        Expect::Ok(0xe4dd776a7f874fb63fd0000000000000, 0x40),
    ),
    (
        "rg_cvtss2sd_40_8",
        &[0xf3, 0x0f, 0x5a, 0xd1],
        0x1f00,
        &[
            (1, 0x59445d76f0700def6e345d354b800000),
            (2, 0x80cfa6b594e59e856a79e89ba0dfb17e),
        ],
        Expect::Ok(0x80cfa6b594e59e854170000000000000, 0x1f00),
    ),
    (
        "rg_cvtss2sd_44_58",
        &[0xf3, 0x0f, 0x5a, 0xce],
        0x6000,
        &[
            (1, 0x84110fa940aebc3be4f641a82772b5d4),
            (6, 0xdeb06ce229dbd93c201a4ff240a00000),
        ],
        Expect::Ok(0x84110fa940aebc3b4014000000000000, 0x6000),
    ),
    (
        "rg_cvtss2sd_56_24",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc4],
        0x5780,
        &[
            (4, 0x1831b6b0c36f45e99628c469be800000),
            (8, 0x70b61a2eff8a78a04bf97aa1d18dc35d),
        ],
        Expect::Ok(0x70b61a2eff8a78a0bfd0000000000000, 0x5780),
    ),
    (
        "rg_rcpps_15_5",
        &[0x45, 0x0f, 0x53, 0xfd],
        0x9f80,
        &[
            (13, 0xff7fffff7fa000007f7fffff7fffffff),
            (15, 0x05133545022cbd93ac1b3aabba390d54),
        ],
        Expect::Ok(0x800000007fe00000000000007fffffff, 0x9f80),
    ),
    (
        "rg_rcpps_17_8",
        &[0x45, 0x0f, 0x53, 0xf0],
        0x1f00,
        &[
            (8, 0xff7fffff800000017effffff7f800001),
            (14, 0x7a9bd0eeb75866d58843fcea3fc3b915),
        ],
        Expect::Ok(0x80000000ff800000000000007fc00001, 0x1f00),
    ),
    (
        "rg_rcpps_25_14",
        &[0x0f, 0x53, 0xd3],
        0x3f00,
        &[
            (2, 0x8776cdc9cffb43b2ca636ed7d36fd38a),
            (3, 0x4aaaaaab7f0000007f000000807fffff),
        ],
        Expect::Ok(0x344000000000000000000000ff800000, 0x3f00),
    ),
    (
        "rg_rcpss_1_15",
        &[0xf3, 0x0f, 0x53, 0xcb],
        0x3e80,
        &[
            (1, 0xbbbe53a813cb7bbcfebc0e1cf541af79),
            (3, 0x836d4ecedbc7999d3c405ad680000000),
        ],
        Expect::Ok(0xbbbe53a813cb7bbcfebc0e1cff800000, 0x3e80),
    ),
    (
        "rg_rcpss_20_14",
        &[0xf3, 0x0f, 0x53, 0xec],
        0x3f00,
        &[
            (4, 0x3f9ea1d56b54c4962daa78817fbfffff),
            (5, 0x24586f7e5ba2f4a0a561d15bd9e20531),
        ],
        Expect::Ok(0x24586f7e5ba2f4a0a561d15b7fffffff, 0x3f00),
    ),
    (
        "rg_rcpss_23_9",
        &[0xf3, 0x45, 0x0f, 0x53, 0xd6],
        0x1e80,
        &[
            (10, 0xe58d76186cde032fd7f354c1f6f60cea),
            (14, 0xe9b80be5fff7c36b49c2532e80000001),
        ],
        Expect::Ok(0xe58d76186cde032fd7f354c1ff800000, 0x1e80),
    ),
    (
        "rg_rsqrtss_14_8",
        &[0xf3, 0x44, 0x0f, 0x52, 0xe4],
        0x1f00,
        &[
            (4, 0x075d4bccd8a13d36206e82e77fc00001),
            (12, 0x588c50792875dfea56808ff09ade91a9),
        ],
        Expect::Ok(0x588c50792875dfea56808ff07fc00001, 0x1f00),
    ),
    (
        "rr_cvtpd2ps_1028",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd3],
        0x2e00,
        &[
            (2, 0xb8b8f4364b188e25bafe2af609858325),
            (11, 0x4804816bd0000000c00c6776fe7b8765),
        ],
        Expect::Fault(0x2e28),
    ),
    (
        "rr_cvtpd2ps_1055",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe7],
        0xb6fb,
        &[
            (7, 0xfff9aef230687fbab68637f010000000),
            (12, 0x68f44872c2ffa0c04ca363f2af4bedde),
        ],
        Expect::Fault(0xb6fb),
    ),
    (
        "rr_cvtpd2ps_1143",
        &[0x66, 0x45, 0x0f, 0x5a, 0xd7],
        0xa45d,
        &[
            (10, 0xa1ab2fea24d640fa6969260a3fedeb9f),
            (15, 0x7ff00000180000003800000010000001),
        ],
        Expect::Fault(0xa45d),
    ),
    (
        "rr_cvtpd2ps_1196",
        &[0x66, 0x41, 0x0f, 0x5a, 0xde],
        0x4000,
        &[
            (3, 0xdd8d6d8b812d707e97b3a4c9ec73d249),
            (14, 0xc0004baf3562f4c83ca6c7c2e072fb5e),
        ],
        Expect::Fault(0x4020),
    ),
    (
        "rr_cvtpd2ps_1208",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc3],
        0xd6dd,
        &[
            (0, 0xd01586a191df4033de95b61a2dbd669f),
            (11, 0x7fde3ae84315978f433c999de2ffc734),
        ],
        Expect::Ok(0x00000000000000007f80000059e4ccf0, 0xd6fd),
    ),
    (
        "rr_cvtpd2ps_1230",
        &[0x66, 0x41, 0x0f, 0x5a, 0xef],
        0xdf03,
        &[
            (5, 0xa6964302c55dcaf60bda1f197bdfc7cf),
            (15, 0xc00fffffffffffff3688000000000000),
        ],
        Expect::Ok(0x0000000000000000c07fffff00000000, 0xdf33),
    ),
    (
        "rr_cvtpd2ps_1268",
        &[0x66, 0x45, 0x0f, 0x5a, 0xd7],
        0xda37,
        &[
            (10, 0xedea92f7018def92af1009b531c6ab97),
            (15, 0x00020915692e5c76fff6e8bf004845cc),
        ],
        Expect::Fault(0xda37),
    ),
    (
        "rr_cvtpd2ps_1281",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc3],
        0xe2fd,
        &[
            (8, 0x27d23291ad1639920de3b32de9f3a879),
            (11, 0x3ca1e167c1096c9c80000e670b2c5bd2),
        ],
        Expect::Fault(0xe2fd),
    ),
    (
        "rr_cvtpd2ps_1287",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc9],
        0xec3c,
        &[(9, 0xc0074b641380424f7fe2ca7d897c2ce9)],
        Expect::Fault(0xec3c),
    ),
    (
        "rr_cvtpd2ps_1301",
        &[0x66, 0x41, 0x0f, 0x5a, 0xde],
        0x4c37,
        &[
            (3, 0x5b265baf09b56bc120a652df980d0320),
            (14, 0xb68fffffffffffffb810000000000000),
        ],
        Expect::Fault(0x4c37),
    ),
    (
        "rr_cvtpd2ps_1347",
        &[0x66, 0x0f, 0x5a, 0xfc],
        0x20fd,
        &[
            (4, 0x37f000001fffffff3690000010000000),
            (7, 0x64dba0243abc0ec6199ed67ae7bcbab2),
        ],
        Expect::Fault(0x20fd),
    ),
    (
        "rr_cvtpd2ps_1385",
        &[0x66, 0x45, 0x0f, 0x5a, 0xd9],
        0x2000,
        &[
            (9, 0x400536b1c002eb26ffdbb39d16094ad6),
            (11, 0x9559da8433530724ff369b5663d2892a),
        ],
        Expect::Fault(0x2028),
    ),
    (
        "rr_cvtpd2ps_1414",
        &[0x66, 0x45, 0x0f, 0x5a, 0xed],
        0x77c0,
        &[(13, 0x800d33efabf2967b5402f4dace93a9db)],
        Expect::Ok(0x0000000000000000800000007f7fffff, 0x77e8),
    ),
    (
        "rr_cvtpd2ps_145",
        &[0x66, 0x41, 0x0f, 0x5a, 0xe9],
        0xd3ba,
        &[
            (5, 0xdc59fe8482702f6f1f53298f039e533b),
            (9, 0x3ff9947bb9cb8b1bd2e28009d2704977),
        ],
        Expect::Fault(0xd3ba),
    ),
    (
        "rr_cvtpd2ps_1468",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc5],
        0x7131,
        &[
            (8, 0x86ad3606681e86a8498fe153ee8e718c),
            (13, 0x3690000018000000c7efffffffffffff),
        ],
        Expect::Fault(0x7131),
    ),
    (
        "rr_cvtpd2ps_1505",
        &[0x66, 0x0f, 0x5a, 0xd2],
        0xccfa,
        &[(2, 0xb698000000000000c800000010000001)],
        Expect::Fault(0xccfa),
    ),
    (
        "rr_cvtpd2ps_1520",
        &[0x66, 0x44, 0x0f, 0x5a, 0xdd],
        0x1bc0,
        &[
            (5, 0xc000000018000000bff0000000000001),
            (11, 0x0cfade8490c8c3b925e1952a4be1f0b1),
        ],
        Expect::Ok(0x0000000000000000c0000001bf800000, 0x1be0),
    ),
    (
        "rr_cvtpd2ps_1556",
        &[0x66, 0x0f, 0x5a, 0xcb],
        0xd97f,
        &[
            (1, 0x77238406d9e2a7b08da56ce842f919c0),
            (3, 0xc3452b3f1b88bef0ffe930f58341f99b),
        ],
        Expect::Fault(0xd97f),
    ),
    (
        "rr_cvtpd2ps_164",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd7],
        0xbc01,
        &[
            (2, 0x728164b54e5d56e36d1ce0f681579ca6),
            (15, 0xffd4a6059f97c31dc3355c82feaa08c3),
        ],
        Expect::Ok(0x0000000000000000ff800000d9aae418, 0xbc29),
    ),
    (
        "rr_cvtpd2ps_1691",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd1],
        0xa87e,
        &[
            (1, 0xbff000000fffffff3ff941fc8fffffff),
            (10, 0x8f01a2be53d41b106c211dc5ed42acb5),
        ],
        Expect::Fault(0xa87e),
    ),
    (
        "rr_cvtpd2ps_1824",
        &[0x66, 0x44, 0x0f, 0x5a, 0xce],
        0xba52,
        &[
            (6, 0xb81fe023263554303800000010000000),
            (9, 0x61cea40bd5f5a84f41b323c1540ec0ae),
        ],
        Expect::Ok(0x000000000000000080ff011a00000000, 0xba72),
    ),
    (
        "rr_cvtpd2ps_1842",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc1],
        0xae98,
        &[
            (8, 0xdc0647a33e471c7910f85f39284b4da4),
            (9, 0xc7e000000fffffffb8230f8c86c04338),
        ],
        Expect::Fault(0xaeb8),
    ),
    (
        "rr_cvtpd2ps_1851",
        &[0x66, 0x41, 0x0f, 0x5a, 0xdd],
        0xe4ba,
        &[
            (3, 0xe8592dc9f394817f68e8afe6594d174b),
            (13, 0xc004158ba775d19e3ff33e16b7370811),
        ],
        Expect::Fault(0xe4ba),
    ),
    (
        "rr_cvtpd2ps_191",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe4],
        0x5f80,
        &[(12, 0x0906611465bfcf633cb853c9989bf16d)],
        Expect::Ok(0x00000000000000000000000125c29e4d, 0x5fb0),
    ),
    (
        "rr_cvtpd2ps_2026",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd5],
        0xb9db,
        &[
            (5, 0xffd7d27757b2b63b433bc10b7e6de0f3),
            (10, 0x9f9e580aa767a7391d406ae3db881f92),
        ],
        Expect::Fault(0xb9fb),
    ),
    (
        "rr_cvtpd2ps_2087",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd6],
        0xbea2,
        &[
            (6, 0x4800000010000001c7f8000000000000),
            (10, 0x0b811c4e3e5e614c06a393debcc6f057),
        ],
        Expect::Ok(0x00000000000000007f7fffffff800000, 0xbeaa),
    ),
    (
        "rr_cvtpd2ps_2096",
        &[0x66, 0x44, 0x0f, 0x5a, 0xed],
        0x6000,
        &[
            (5, 0x433a2fc8f408353a80298a6a528df63f),
            (13, 0xb89a0521caa7ae1cde986ecb179530a4),
        ],
        Expect::Fault(0x6030),
    ),
    (
        "rr_cvtpd2ps_2108",
        &[0x66, 0x44, 0x0f, 0x5a, 0xf7],
        0x80c0,
        &[
            (7, 0x0006a1e987471f02800e4e361c092e29),
            (14, 0x1aa5569941afc703b80a577fd48ea074),
        ],
        Expect::Ok(0x00000000000000000000000080000000, 0x80c0),
    ),
    (
        "rr_cvtpd2ps_2135",
        &[0x66, 0x44, 0x0f, 0x5a, 0xfe],
        0xbd3c,
        &[
            (6, 0x434cb031e895e198c3425ee7ff5d1e54),
            (15, 0x1b68eaebdfd5c4cd4bca500e7f011009),
        ],
        Expect::Ok(0x00000000000000005a65818fda12f740, 0xbd3c),
    ),
    (
        "rr_cvtpd2ps_2176",
        &[0x66, 0x44, 0x0f, 0x5a, 0xed],
        0x2e2c,
        &[
            (5, 0x381000000fffffff3808000000000000),
            (13, 0x2d55e523b159196029ce211277aa68dd),
        ],
        Expect::Fault(0x2e2c),
    ),
    (
        "rr_cvtpd2ps_2226",
        &[0x66, 0x44, 0x0f, 0x5a, 0xf9],
        0x36f5,
        &[
            (1, 0xc7f000001000000137f0000000000001),
            (15, 0x5a645156c1834771b54b24d127d1b683),
        ],
        Expect::Fault(0x36fd),
    ),
    (
        "rr_cvtpd2ps_233",
        &[0x66, 0x0f, 0x5a, 0xe9],
        0x9e80,
        &[
            (1, 0x37f00000000000003803ca43b2d047c8),
            (5, 0x2cb0b7d5a87701d142b60c86c4848555),
        ],
        Expect::Ok(0x00000000000000000000000000000000, 0x9eb0),
    ),
    (
        "rr_cvtpd2ps_2332",
        &[0x66, 0x41, 0x0f, 0x5a, 0xf1],
        0x8e1a,
        &[
            (6, 0x2f22ebbe9a28a2059704af14f45d6f3d),
            (9, 0x80003d709f335eb0368fffffffffffff),
        ],
        Expect::Fault(0x8e1a),
    ),
    (
        "rr_cvtpd2ps_2336",
        &[0x66, 0x45, 0x0f, 0x5a, 0xca],
        0x5d80,
        &[
            (9, 0xc08dec5f9423e235056ed72ef77286fa),
            (10, 0xb6a000000000000047ffffffffffffff),
        ],
        Expect::Ok(0x0000000000000000800000017f800000, 0x5da8),
    ),
    (
        "rr_cvtpd2ps_2339",
        &[0x66, 0x45, 0x0f, 0x5a, 0xec],
        0xc9bb,
        &[
            (12, 0x3cb4b1d8b12857a6ffe8ad293c25caf3),
            (13, 0xfd25b14d54fb8a8497670170d4d128cc),
        ],
        Expect::Fault(0xc9bb),
    ),
    (
        "rr_cvtpd2ps_2607",
        &[0x66, 0x45, 0x0f, 0x5a, 0xde],
        0x17c0,
        &[
            (11, 0x7101a343a177397fdc2612c91666b7b2),
            (14, 0x38100000100000013820000000000001),
        ],
        Expect::Ok(0x00000000000000000080000101000000, 0x17e0),
    ),
    (
        "rr_cvtpd2ps_2627",
        &[0x66, 0x0f, 0x5a, 0xf3],
        0x26d,
        &[
            (3, 0xbff1401c6f1825a243422960a9b9881f),
            (6, 0xa73b720acf61c3d6adcb4ac0812c3374),
        ],
        Expect::Fault(0x26d),
    ),
    (
        "rr_cvtpd2ps_2658",
        &[0x66, 0x0f, 0x5a, 0xcf],
        0xac3c,
        &[
            (1, 0x784417fe3546fdee92d1b14ab737372f),
            (7, 0xc320cf67d4eec7ad001131aebc20cc82),
        ],
        Expect::Fault(0xac3c),
    ),
    (
        "rr_cvtpd2ps_2787",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc7],
        0xfbf,
        &[
            (7, 0x80000000000000013ff000001fffffff),
            (8, 0x4f405cc70c9936e09676f0069fdd7793),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rr_cvtpd2ps_2860",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd9],
        0x77c0,
        &[
            (1, 0xb7f000000fffffff3800000010000001),
            (11, 0x7c2b0e05a2fd3a94924d16081b145d8e),
        ],
        Expect::Fault(0x77f0),
    ),
    (
        "rr_cvtpd2ps_2887",
        &[0x66, 0x41, 0x0f, 0x5a, 0xdb],
        0x1780,
        &[
            (3, 0xa665c619439ab6d6a33eabaf65898bfd),
            (11, 0x368000001fffffffb800000000000000),
        ],
        Expect::Fault(0x17b0),
    ),
    (
        "rr_cvtpd2ps_2911",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc5],
        0xfbf,
        &[
            (0, 0xb356a69bb6a463e4faae8b1e87425fc8),
            (13, 0x62d27863fe3aef69fff6ef497cc85d33),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rr_cvtpd2ps_2954",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe3],
        0x4f4b,
        &[
            (11, 0x43208d927f4b84d5fffc58ba8d9321ef),
            (12, 0x68413e01ad2538087dc1fca747dddbb0),
        ],
        Expect::Fault(0x4f6b),
    ),
    (
        "rr_cvtpd2ps_3014",
        &[0x66, 0x0f, 0x5a, 0xf4],
        0x77c0,
        &[
            (4, 0x36a000001fffffffc002e9b9cae8e4f8),
            (6, 0xd92fb58fb025a4436408c9a921324697),
        ],
        Expect::Fault(0x77f0),
    ),
    (
        "rr_cvtpd2ps_306",
        &[0x66, 0x0f, 0x5a, 0xd3],
        0xc860,
        &[
            (2, 0xa3598df305514c170d0df56a1fdb020b),
            (3, 0x7feaf6f0569eb23f8028e836daa8126d),
        ],
        Expect::Fault(0xc878),
    ),
    (
        "rr_cvtpd2ps_3066",
        &[0x66, 0x45, 0x0f, 0x5a, 0xec],
        0xcd1c,
        &[
            (12, 0x3693cd034800ea2bb68fffffffffffff),
            (13, 0xdd6d7d30a5f0892fbf1281ade9ea18a8),
        ],
        Expect::Fault(0xcd3c),
    ),
    (
        "rr_cvtpd2ps_3119",
        &[0x66, 0x44, 0x0f, 0x5a, 0xcd],
        0xd7ee,
        &[
            (5, 0x3ff000001fffffff480000000fffffff),
            (9, 0xf7bb141e9353a8e3b0b8f90195d147d0),
        ],
        Expect::Ok(0x00000000000000003f8000017f800000, 0xd7ee),
    ),
    (
        "rr_cvtpd2ps_3289",
        &[0x66, 0x45, 0x0f, 0x5a, 0xca],
        0xb421,
        &[
            (9, 0x7059032d40eef1c8cd8e02d768c1d7f2),
            (10, 0x369000001800000047e0000010000000),
        ],
        Expect::Fault(0xb431),
    ),
    (
        "rr_cvtpd2ps_3318",
        &[0x66, 0x0f, 0x5a, 0xed],
        0x3a76,
        &[(5, 0xfffc4f9afd4b239bc000797d5b03d870)],
        Expect::Ok(0x0000000000000000ffe27cd7c003cbeb, 0x3a76),
    ),
    (
        "rr_cvtpd2ps_3369",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe9],
        0x3b80,
        &[
            (1, 0x3684cd1effffffff3680000000000000),
            (13, 0x82c1c55e0eb5e0f89524024f1948119e),
        ],
        Expect::Ok(0x00000000000000000000000000000000, 0x3bb0),
    ),
    (
        "rr_cvtpd2ps_339",
        &[0x66, 0x0f, 0x5a, 0xca],
        0x58df,
        &[
            (1, 0x9ed624500453ed32b52854b69bb8ef97),
            (2, 0x4346af93a8a414c8af5a83ec62cd0285),
        ],
        Expect::Ok(0x00000000000000005a357c9e80000000, 0x58ff),
    ),
    (
        "rr_cvtpd2ps_3411",
        &[0x66, 0x45, 0x0f, 0x5a, 0xfd],
        0xb5eb,
        &[
            (13, 0xc7f0000018000000800000001fffffff),
            (15, 0xac4f7f2ae586111af9bf6c9df5cc5584),
        ],
        Expect::Ok(0x0000000000000000ff80000080000000, 0xb5eb),
    ),
    (
        "rr_cvtpd2ps_3475",
        &[0x66, 0x45, 0x0f, 0x5a, 0xce],
        0xb933,
        &[
            (9, 0x1bb278ef3efecad474f65b049d7b190b),
            (14, 0xfffce0e5d00000017ff000001fffffff),
        ],
        Expect::Fault(0xb933),
    ),
    (
        "rr_cvtpd2ps_348",
        &[0x66, 0x0f, 0x5a, 0xe3],
        0xdf21,
        &[
            (3, 0x002521090adcb22ffffc23896e9a66c7),
            (4, 0x17192e5df2fb77866463df2c646c5d73),
        ],
        Expect::Ok(0x000000000000000000000000ffe11c4b, 0xdf31),
    ),
    (
        "rr_cvtpd2ps_3483",
        &[0x66, 0x0f, 0x5a, 0xdd],
        0xceb2,
        &[
            (3, 0xe4e0b2cacd50dd2322902a68abc127bd),
            (5, 0x433c4c0d638ddca9c340794c605058c5),
        ],
        Expect::Fault(0xceb2),
    ),
    (
        "rr_cvtpd2ps_3567",
        &[0x66, 0x0f, 0x5a, 0xdf],
        0xef80,
        &[
            (3, 0x362b4c49928b4a42aab6278dbd4a25a1),
            (7, 0x47f80000000000004800000010000001),
        ],
        Expect::Fault(0xefa8),
    ),
    (
        "rr_cvtpd2ps_3718",
        &[0x66, 0x44, 0x0f, 0x5a, 0xfe],
        0x2000,
        &[
            (6, 0xb6a8000000000000c7e0000018000000),
            (15, 0x4d773dbdcb2524a2aa5690066042d91c),
        ],
        Expect::Fault(0x2030),
    ),
    (
        "rr_cvtpd2ps_3719",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc3],
        0x263b,
        &[
            (3, 0xb81df6dbe4c99427c7e0000018000000),
            (8, 0x0650043b8b6ee87bb081005ff5a42eae),
        ],
        Expect::Fault(0x263b),
    ),
    (
        "rr_cvtpd2ps_3758",
        &[0x66, 0x41, 0x0f, 0x5a, 0xed],
        0xf03d,
        &[
            (5, 0xae6dba715f99dccf6da89aa7f73bd38e),
            (13, 0x7fe34066570058ea7fd01ae6aab85fa2),
        ],
        Expect::Fault(0xf03d),
    ),
    (
        "rr_cvtpd2ps_3773",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe2],
        0xbfa5,
        &[
            (10, 0xb82fffffffffffffc800000018000000),
            (12, 0x7de49ff167c1b61c34e909caa3388b82),
        ],
        Expect::Ok(0x000000000000000081800000ff800000, 0xbfad),
    ),
    (
        "rr_cvtpd2ps_3927",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc5],
        0x7dc0,
        &[
            (5, 0x3cbc410793772da67ff6a0f35c91589b),
            (8, 0x92859c6e99121d3e913393717ce8b94e),
        ],
        Expect::Ok(0x000000000000000025e2083c7ff5079a, 0x7de1),
    ),
    (
        "rr_cvtpd2ps_3969",
        &[0x66, 0x0f, 0x5a, 0xf2],
        0xd48d,
        &[
            (2, 0xb80fffffffffffff4000000010000001),
            (6, 0x5e780fcdedb908c1ffce56fb19bd2e5c),
        ],
        Expect::Fault(0xd4bd),
    ),
    (
        "rr_cvtpd2ps_4",
        &[0x66, 0x44, 0x0f, 0x5a, 0xeb],
        0x9780,
        &[
            (3, 0x381000000fffffffb7fe30add0000000),
            (13, 0xa5e6354c215b31b16f76dbb1c670e726),
        ],
        Expect::Fault(0x97b0),
    ),
    (
        "rr_cvtpd2ps_40",
        &[0x66, 0x0f, 0x5a, 0xfa],
        0x2e07,
        &[
            (2, 0x00084b21c373e721ffd18a17ef7971a5),
            (7, 0x07fdf7f86ce261c91301b531d3142c36),
        ],
        Expect::Fault(0x2e07),
    ),
    (
        "rr_cvtpd2ps_4057",
        &[0x66, 0x0f, 0x5a, 0xfc],
        0x135b,
        &[
            (4, 0x3690000000000000000fffffffffffff),
            (7, 0x90b3ba08dbba70b30711f7d11b180d04),
        ],
        Expect::Fault(0x135b),
    ),
    (
        "rr_cvtpd2ps_4085",
        &[0x66, 0x41, 0x0f, 0x5a, 0xf8],
        0x436e,
        &[
            (7, 0xf2b76a001f978a902486184f3b6dc2a8),
            (8, 0xc3229cd0e477662fbffec86084458237),
        ],
        Expect::Fault(0x436e),
    ),
    (
        "rr_cvtpd2ps_4095",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd5],
        0x59cb,
        &[
            (5, 0x7ff89c8374b920713cbd5c62620a90c5),
            (10, 0xbd8163dc5328dbc51a20e03d84115b2f),
        ],
        Expect::Ok(0x00000000000000007fc4e41b25eae314, 0x59eb),
    ),
    (
        "rr_cvtpd2ps_4142",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc7],
        0x5e86,
        &[
            (0, 0x040420e5030f7153646b28aa5e905079),
            (15, 0x800000000fffffff3812551909dc9e34),
        ],
        Expect::Fault(0x5e86),
    ),
    (
        "rr_cvtpd2ps_4155",
        &[0x66, 0x45, 0x0f, 0x5a, 0xf8],
        0x8237,
        &[
            (8, 0xc33b69199da532a0802e538152ee3b5b),
            (15, 0xf82c30525a780af7f08bd45ddae0cb78),
        ],
        Expect::Fault(0x8237),
    ),
    (
        "rr_cvtpd2ps_4165",
        &[0x66, 0x44, 0x0f, 0x5a, 0xf9],
        0x57fc,
        &[
            (1, 0x3cbd84b5598cfee5ffdec6ba885fa211),
            (15, 0xb2f6c4b02d3de9e5f5dd676a7845e58f),
        ],
        Expect::Ok(0x000000000000000025ec25abff7fffff, 0x57fc),
    ),
    (
        "rr_cvtpd2ps_4305",
        &[0x66, 0x41, 0x0f, 0x5a, 0xff],
        0xa01f,
        &[
            (7, 0x4edb6e1a02a3a59c9dbe9722aacb9dad),
            (15, 0x480000001ffffffffff8bab310000000),
        ],
        Expect::Fault(0xa03f),
    ),
    (
        "rr_cvtpd2ps_4312",
        &[0x66, 0x45, 0x0f, 0x5a, 0xcf],
        0xfbf,
        &[
            (9, 0x82e0f0ffaa67d043b35d0dedf282e4f8),
            (15, 0x3ff8000000000000bff0000010000000),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rr_cvtpd2ps_4316",
        &[0x66, 0x44, 0x0f, 0x5a, 0xdf],
        0x8a3c,
        &[
            (7, 0xc80b83fdf64109c1382fffffffffffff),
            (11, 0xf841f9a33b76879a6c23fcc3b6a21451),
        ],
        Expect::Fault(0x8a3c),
    ),
    (
        "rr_cvtpd2ps_435",
        &[0x66, 0x45, 0x0f, 0x5a, 0xf0],
        0x8c08,
        &[
            (8, 0xc7e00000100000017fffffffffffffff),
            (14, 0x661273d809ea048b06f2bee09c91230a),
        ],
        Expect::Fault(0x8c28),
    ),
    (
        "rr_cvtpd2ps_4536",
        &[0x66, 0x0f, 0x5a, 0xc5],
        0x9d80,
        &[
            (0, 0x85bd9c8186696f623da400668d5f2abf),
            (5, 0x3828000000000000b818000000000000),
        ],
        Expect::Ok(0x00000000000000000140000080c00000, 0x9d80),
    ),
    (
        "rr_cvtpd2ps_4546",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe1],
        0x3f20,
        &[
            (1, 0x802448060dc404bd433ed03170e05d40),
            (12, 0x550465c7539db1cc78187ec42568c771),
        ],
        Expect::Ok(0x00000000000000008000000159f6818b, 0x3f30),
    ),
    (
        "rr_cvtpd2ps_4587",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd2],
        0xbb01,
        &[
            (2, 0xbf8a3990b93426a738733b741fc73ddd),
            (10, 0xc0009fa699938f3a002eb33b1b752f7b),
        ],
        Expect::Ok(0x0000000000000000c004fd3500000000, 0xbb31),
    ),
    (
        "rr_cvtpd2ps_4613",
        &[0x66, 0x41, 0x0f, 0x5a, 0xe2],
        0xbf36,
        &[
            (4, 0x52906f8f78b366e60b5325ce0391b9fc),
            (10, 0x7ff690980f64c853ffee4873c600c3a3),
        ],
        Expect::Fault(0xbf37),
    ),
    (
        "rr_cvtpd2ps_4627",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc8],
        0x5431,
        &[
            (0, 0x368fffffffffffffb688000000000000),
            (9, 0xe2df522e4e455ab94d942cdee2678aac),
        ],
        Expect::Fault(0x5431),
    ),
    (
        "rr_cvtpd2ps_4644",
        &[0x66, 0x41, 0x0f, 0x5a, 0xf8],
        0x1f95,
        &[
            (7, 0xf80db1d2bffb201f7cb77fb93aff8904),
            (8, 0x47f00000000000014008000000000000),
        ],
        Expect::Ok(0x00000000000000007f80000040400000, 0x1fbd),
    ),
    (
        "rr_cvtpd2ps_4676",
        &[0x66, 0x0f, 0x5a, 0xea],
        0x32fb,
        &[
            (2, 0x38200000100000017ff0000010000001),
            (5, 0x20f1412f275a22755aa0f8804ab31725),
        ],
        Expect::Ok(0x0000000000000000010000007fc00000, 0x32fb),
    ),
    (
        "rr_cvtpd2ps_4731",
        &[0x66, 0x45, 0x0f, 0x5a, 0xfc],
        0xbf3d,
        &[
            (12, 0xfff000000fffffff7ff000001fffffff),
            (15, 0x7405caab61a3cc335e6ccc8bd741c30e),
        ],
        Expect::Fault(0xbf3d),
    ),
    (
        "rr_cvtpd2ps_479",
        &[0x66, 0x0f, 0x5a, 0xf0],
        0xddbe,
        &[
            (0, 0x4000000018000000b81fffffffffffff),
            (6, 0xb07dda14334f16a7b9b5a230bcaa5b43),
        ],
        Expect::Ok(0x00000000000000004000000180ffffff, 0xddbe),
    ),
    (
        "rr_cvtpd2ps_4796",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc4],
        0xb130,
        &[
            (0, 0x1b304dfc28a8eb04de01fbd839250bf5),
            (12, 0x7ffbf56478c492c6bcb077e5be2ccca5),
        ],
        Expect::Ok(0x00000000000000007fdfab23a583bf2e, 0xb130),
    ),
    (
        "rr_cvtpd2ps_4833",
        &[0x66, 0x45, 0x0f, 0x5a, 0xcc],
        0xa84e,
        &[
            (9, 0x9cc40758f8d90211ee8446939b019b36),
            (12, 0xc7e000001fffffffb800000010000000),
        ],
        Expect::Fault(0xa87e),
    ),
    (
        "rr_cvtpd2ps_496",
        &[0x66, 0x0f, 0x5a, 0xff],
        0x5b80,
        &[(7, 0x3810000010000000bff0000000000001)],
        Expect::Ok(0x000000000000000000800001bf800000, 0x5ba0),
    ),
    (
        "rr_cvtpd2ps_5016",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd0],
        0xdd97,
        &[
            (0, 0x3ff31158afffffffb808000000000000),
            (10, 0xd48910b404514d0174408854e1ce4121),
        ],
        Expect::Ok(0x00000000000000003f988ac680000000, 0xddb7),
    ),
    (
        "rr_cvtpd2ps_5064",
        &[0x66, 0x44, 0x0f, 0x5a, 0xec],
        0xb4c1,
        &[
            (4, 0x7fdcbc2d2337f4d43cbdccab7d746a34),
            (13, 0x5735509e8992590bb60cd5ab32bef28e),
        ],
        Expect::Ok(0x00000000000000007f7fffff25ee655b, 0xb4e9),
    ),
    (
        "rr_cvtpd2ps_5077",
        &[0x66, 0x0f, 0x5a, 0xd5],
        0x693c,
        &[
            (2, 0x1f23bddf4bec18a5a784f245627541e9),
            (5, 0xb69000000fffffffc7f0000000000001),
        ],
        Expect::Fault(0x693c),
    ),
    (
        "rr_cvtpd2ps_515",
        &[0x66, 0x45, 0x0f, 0x5a, 0xda],
        0x7dc0,
        &[
            (10, 0x4343cdf772fc05fbffdfc36a54132289),
            (11, 0xed95053e503d71878bca437f99a52f50),
        ],
        Expect::Ok(0x00000000000000005a1e6fbbff7fffff, 0x7de8),
    ),
    (
        "rr_cvtpd2ps_532",
        &[0x66, 0x0f, 0x5a, 0xea],
        0x6a7f,
        &[
            (2, 0x800b134012a4da6a7fde7d9413b9df69),
            (5, 0xbbd8e49ad89389176c8365d41f9a9d97),
        ],
        Expect::Fault(0x6a7f),
    ),
    (
        "rr_cvtpd2ps_5354",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc8],
        0xdfe9,
        &[
            (0, 0x7fdb4f345e2b78827fe01a33f6e45a9d),
            (9, 0x0b84b1f520ec66e0faeca5518f17dd0b),
        ],
        Expect::Ok(0x00000000000000007f8000007f800000, 0xdfe9),
    ),
    (
        "rr_cvtpd2ps_5497",
        &[0x66, 0x44, 0x0f, 0x5a, 0xcc],
        0x6be8,
        &[
            (4, 0x800fdc46daf547427fe4209b668ddb28),
            (9, 0xed03c9e7b5418477376f2c6fcb0402f7),
        ],
        Expect::Fault(0x6be8),
    ),
    (
        "rr_cvtpd2ps_5599",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc1],
        0x40,
        &[
            (0, 0x65efb41ed88f06223f55000d66d26e1f),
            (9, 0xb7f00000180000007ff0000000000001),
        ],
        Expect::Fault(0x41),
    ),
    (
        "rr_cvtpd2ps_5602",
        &[0x66, 0x44, 0x0f, 0x5a, 0xf8],
        0xc10c,
        &[
            (0, 0x400b8b35b8d0df253cbae5b723f67b09),
            (15, 0x2097499277233ad83408267d45bf5035),
        ],
        Expect::Fault(0xc12c),
    ),
    (
        "rr_cvtpd2ps_5633",
        &[0x66, 0x45, 0x0f, 0x5a, 0xd8],
        0xd831,
        &[
            (8, 0xb680000018000000b6a0000010000001),
            (11, 0xbd0a3efabb76e0d7d226c0b990d6bc7d),
        ],
        Expect::Ok(0x00000000000000008000000080000000, 0xd831),
    ),
    (
        "rr_cvtpd2ps_5705",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc1],
        0xe3f0,
        &[
            (8, 0xefa9bbf305ecdb8d2fae3e25d8e16a01),
            (9, 0x4347be83a57b21be433c663c8958db9a),
        ],
        Expect::Fault(0xe3f0),
    ),
    (
        "rr_cvtpd2ps_5736",
        &[0x66, 0x41, 0x0f, 0x5a, 0xda],
        0x61bf,
        &[
            (3, 0xb2b24c409067dc198da8a0df37d133c9),
            (10, 0x37f67a0c8d237bc5fff0000010000000),
        ],
        Expect::Fault(0x61bf),
    ),
    (
        "rr_cvtpd2ps_5788",
        &[0x66, 0x45, 0x0f, 0x5a, 0xed],
        0x77c0,
        &[(13, 0x3690000000000000380fffffffffffff)],
        Expect::Fault(0x77f0),
    ),
    (
        "rr_cvtpd2ps_5868",
        &[0x66, 0x0f, 0x5a, 0xf5],
        0x1fbf,
        &[
            (5, 0xc7e156a9dfffffff3810000010000000),
            (6, 0x830936618bf6b64fb19f3d4770ad9b02),
        ],
        Expect::Ok(0x0000000000000000ff0ab54f00800000, 0x1fbf),
    ),
    (
        "rr_cvtpd2ps_5879",
        &[0x66, 0x0f, 0x5a, 0xc5],
        0x8da2,
        &[
            (0, 0x4e11ca5438dfadd721f01a02b49dd021),
            (5, 0x40000000100000013810000000000001),
        ],
        Expect::Fault(0x8da2),
    ),
    (
        "rr_cvtpd2ps_5886",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe4],
        0xb8b0,
        &[
            (4, 0xb810000018000000381dbf0090000000),
            (12, 0xe32d9370ac7d9a282ea332cb7038060b),
        ],
        Expect::Ok(0x00000000000000008080000100edf804, 0xb8b0),
    ),
    (
        "rr_cvtpd2ps_6014",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc4],
        0x86d8,
        &[
            (0, 0x1efe2a299c637bfdc507235511c46ef0),
            (12, 0x802875bf8a11ee90434752f0c372da8b),
        ],
        Expect::Fault(0x86f8),
    ),
    (
        "rr_cvtpd2ps_6026",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc6],
        0x8000,
        &[
            (8, 0xf93a9bd3651289248342a7aa30b62650),
            (14, 0x3ff98588315a498fb680000010000001),
        ],
        Expect::Fault(0x8030),
    ),
    (
        "rr_cvtpd2ps_6035",
        &[0x66, 0x41, 0x0f, 0x5a, 0xef],
        0x464e,
        &[
            (5, 0x6fda4fe58cc8851e7bd0f6ecd8fd2173),
            (15, 0xc32d8338b2b9a1b1166443735590b990),
        ],
        Expect::Fault(0x467e),
    ),
    (
        "rr_cvtpd2ps_6059",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe6],
        0x4f80,
        &[
            (12, 0x9010a6200e0fa103223e87408ee88509),
            (14, 0xb6a0000018000000c0046c000e47a00f),
        ],
        Expect::Fault(0x4fb0),
    ),
    (
        "rr_cvtpd2ps_6108",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc6],
        0x7d80,
        &[
            (0, 0xb8a855287212ad20185328b145382e14),
            (14, 0x368708914fffffff3810000018000000),
        ],
        Expect::Ok(0x00000000000000000000000000800000, 0x7db0),
    ),
    (
        "rr_cvtpd2ps_6114",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd6],
        0xb2eb,
        &[
            (2, 0x5b40e459efc567decb51d01a37cc9ca3),
            (14, 0x4324fd814ea4f65fbcbd86bf1ac44b2e),
        ],
        Expect::Ok(0x00000000000000005927ec0aa5ec35f9, 0xb2eb),
    ),
    (
        "rr_cvtpd2ps_6207",
        &[0x66, 0x0f, 0x5a, 0xd8],
        0x4c9f,
        &[
            (0, 0xc321ae86b81cfc7f80022954aab85adf),
            (3, 0x806efe466471b35ab768b08528ed3d08),
        ],
        Expect::Fault(0x4c9f),
    ),
    (
        "rr_cvtpd2ps_6226",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd3],
        0x4000,
        &[
            (3, 0x3680000000000001c00fffffffffffff),
            (10, 0x5a3772a694f6a14c6e2048de30f78382),
        ],
        Expect::Fault(0x4030),
    ),
    (
        "rr_cvtpd2ps_6236",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc0],
        0x9fb,
        &[(8, 0x8009e79521925c830011522de0e5840c)],
        Expect::Fault(0x9fb),
    ),
    (
        "rr_cvtpd2ps_6237",
        &[0x66, 0x44, 0x0f, 0x5a, 0xe9],
        0xd41e,
        &[
            (1, 0x47e8b1d6647cb4f147e786a15f6e23dd),
            (13, 0xae64cee4c99df260009f5e6841074807),
        ],
        Expect::Ok(0x00000000000000007f458eb47f3c350b, 0xd43e),
    ),
    (
        "rr_cvtpd2ps_6257",
        &[0x66, 0x44, 0x0f, 0x5a, 0xfd],
        0x6f80,
        &[
            (5, 0xffddb26d1e9529333ffe93a22aa7165c),
            (15, 0x6e6ab5741e85ad32c613096f7c7ea372),
        ],
        Expect::Fault(0x6fa8),
    ),
    (
        "rr_cvtpd2ps_6261",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc9],
        0xb026,
        &[
            (1, 0x1c72f49aff0cc61c68d89557a4f148b4),
            (9, 0x8009608d3877d02ac3291b257e218791),
        ],
        Expect::Fault(0xb026),
    ),
    (
        "rr_cvtpd2ps_6274",
        &[0x66, 0x0f, 0x5a, 0xfc],
        0x3cb9,
        &[
            (4, 0xffeb30d3d4d2f772c00af822c08ee4df),
            (7, 0x947c4bc60a1abe89679f96c3bc240581),
        ],
        Expect::Ok(0x0000000000000000ff800000c057c117, 0x3cb9),
    ),
    (
        "rr_cvtpd2ps_6346",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc5],
        0x1e9,
        &[
            (5, 0x3ca1a4c51dd2170afffd647978a563a2),
            (8, 0x6e09361102eb0c1cfe5799745418cd4a),
        ],
        Expect::Fault(0x1e9),
    ),
    (
        "rr_cvtpd2ps_6371",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe6],
        0x37cc,
        &[
            (12, 0x0a87ec27ffd95b8b5e9a2e46d62e49d8),
            (14, 0xc80fffffffffffffc7e000001fffffff),
        ],
        Expect::Ok(0x0000000000000000ff800000ff000001, 0x37ec),
    ),
    (
        "rr_cvtpd2ps_6416",
        &[0x66, 0x41, 0x0f, 0x5a, 0xe9],
        0x2e54,
        &[
            (5, 0x57412345b1a566ab98eae4e0ee40822e),
            (9, 0x0011a292afbc8e93fff78d854b67d008),
        ],
        Expect::Fault(0x2e55),
    ),
    (
        "rr_cvtpd2ps_642",
        &[0x66, 0x44, 0x0f, 0x5a, 0xcc],
        0x53ec,
        &[
            (4, 0x41dbc06f1296ab063cbcbfd85564e8cf),
            (9, 0xd1500c8094f2d1055689d8494a951dbd),
        ],
        Expect::Ok(0x00000000000000004ede037925e5fec3, 0x53ec),
    ),
    (
        "rr_cvtpd2ps_6430",
        &[0x66, 0x45, 0x0f, 0x5a, 0xd6],
        0x3a5f,
        &[
            (10, 0xe843bbb79e68f7180857d4b24186a475),
            (14, 0xc3309d5485ccb333bcbf99bb7a6f65af),
        ],
        Expect::Ok(0x0000000000000000d984eaa5a5fccddc, 0x3a7f),
    ),
    (
        "rr_cvtpd2ps_6431",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd9],
        0x28ba,
        &[
            (1, 0x8000f5d8b7e2ba0d7ffc6e73ce2e0c22),
            (11, 0x2e378a46da80e004ddd1e8e1e3fd059f),
        ],
        Expect::Fault(0x28ba),
    ),
    (
        "rr_cvtpd2ps_6454",
        &[0x66, 0x44, 0x0f, 0x5a, 0xd6],
        0xde2b,
        &[
            (6, 0x4800000018000000fff8000000000000),
            (10, 0xf5cb710088f746ea93d47647a4d1f3ee),
        ],
        Expect::Ok(0x00000000000000007f800000ffc00000, 0xde2b),
    ),
    (
        "rr_cvtpd2ps_6510",
        &[0x66, 0x44, 0x0f, 0x5a, 0xed],
        0xef80,
        &[
            (5, 0x800144ecaf97f4037fd6d5b7deb7f7d1),
            (13, 0x9b0bab2c70d27ee25f909c9330399d7c),
        ],
        Expect::Fault(0xefba),
    ),
    (
        "rr_cvtpd2ps_6571",
        &[0x66, 0x0f, 0x5a, 0xec],
        0x7f40,
        &[
            (4, 0xb828000000000000fff000000fffffff),
            (5, 0x59be903af12af07da8a9a07146404b40),
        ],
        Expect::Fault(0x7f41),
    ),
    (
        "rr_cvtpd2ps_6575",
        &[0x66, 0x41, 0x0f, 0x5a, 0xf7],
        0x380,
        &[
            (6, 0x1c36d341c1534cdb736e89c6bda85c15),
            (15, 0xb6900000000000007ff9fb429463210f),
        ],
        Expect::Fault(0x390),
    ),
    (
        "rr_cvtpd2ps_6580",
        &[0x66, 0x41, 0x0f, 0x5a, 0xc3],
        0x7d80,
        &[
            (0, 0x017d548210de447a8b626f6ef342e499),
            (11, 0xbcb907faf5cfdbc900085a5e3200d94b),
        ],
        Expect::Ok(0x0000000000000000a5c83fd700000000, 0x7db2),
    ),
    (
        "rr_cvtpd2ps_6582",
        &[0x66, 0x0f, 0x5a, 0xfc],
        0x80c0,
        &[
            (4, 0x380000001fffffffc800000000000001),
            (7, 0x4733d2b63ffe75551607cb237ea5466a),
        ],
        Expect::Fault(0x80f8),
    ),
    (
        "rr_cvtpd2ps_6609",
        &[0x66, 0x45, 0x0f, 0x5a, 0xcc],
        0xf090,
        &[
            (9, 0x16a019b2227da639deb86c6bec5d1d6c),
            (12, 0x37ffffffffffffff3ff0000010000000),
        ],
        Expect::Fault(0xf0b0),
    ),
    (
        "rr_cvtpd2ps_6665",
        &[0x66, 0x0f, 0x5a, 0xe6],
        0x1780,
        &[
            (4, 0x24517d22ddb1f7f2e29396973ba02674),
            (6, 0xb68000000fffffffb80f9a4510000000),
        ],
        Expect::Fault(0x17b0),
    ),
    (
        "rr_cvtpd2ps_6694",
        &[0x66, 0x45, 0x0f, 0x5a, 0xfc],
        0x1ec0,
        &[
            (12, 0xbff000000fffffffffffffffffffffff),
            (15, 0x2b4c16bdf7400f8f77e5949510fdcab0),
        ],
        Expect::Ok(0x0000000000000000bf800000ffffffff, 0x1ee0),
    ),
    (
        "rr_cvtpd2ps_6739",
        &[0x66, 0x41, 0x0f, 0x5a, 0xdc],
        0xe92a,
        &[
            (3, 0x98f619ff3d32c356d37a2596c64956b6),
            (12, 0xb820000010000000480000000fffffff),
        ],
        Expect::Fault(0xe92a),
    ),
    (
        "rr_cvtpd2ps_686",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc6],
        0x55d4,
        &[
            (8, 0x477e52db7aa6df38835e9a55c6ea2021),
            (14, 0x3cbccd8ebc60db13758bee21aac39c00),
        ],
        Expect::Ok(0x000000000000000025e66c767f800000, 0x55fc),
    ),
    (
        "rr_cvtpd2ps_6879",
        &[0x66, 0x41, 0x0f, 0x5a, 0xfd],
        0x5faa,
        &[
            (7, 0x8a603518690c4e43e4413c173c2de369),
            (13, 0xc323e3378fbd02654344158c5ecef8db),
        ],
        Expect::Ok(0x0000000000000000d91f19bc5a20ac63, 0x5faa),
    ),
    (
        "rr_cvtpd2ps_69",
        &[0x66, 0x0f, 0x5a, 0xf2],
        0x4873,
        &[
            (2, 0x400fd5869053c751bca4c08845554616),
            (6, 0xffd6522d338139f3becb3d827e9c9bc2),
        ],
        Expect::Fault(0x4873),
    ),
    (
        "rr_cvtpd2ps_6911",
        &[0x66, 0x45, 0x0f, 0x5a, 0xe0],
        0xdb36,
        &[
            (8, 0xb69000000000000036a8ca5c8fffffff),
            (12, 0x4723c09285eaf44256cae0dd1c92a276),
        ],
        Expect::Ok(0x00000000000000008000000000000000, 0xdb36),
    ),
    (
        "rr_cvtpd2ps_693",
        &[0x66, 0x0f, 0x5a, 0xe6],
        0x1bc0,
        &[
            (4, 0x5b265d3ffcc12c4ce049438819616525),
            (6, 0xbcadc46e929ca3957ff80cb52e4b380a),
        ],
        Expect::Ok(0x0000000000000000a56e23757fc065a9, 0x1be0),
    ),
    (
        "rr_cvtpd2ps_6942",
        &[0x66, 0x45, 0x0f, 0x5a, 0xc4],
        0xef80,
        &[
            (8, 0xefa04597a233bc12d20695cf484f5d56),
            (12, 0x721d5a1eb1b4dc86c003b58771bc8c5e),
        ],
        Expect::Fault(0xefa8),
    ),
    (
        "rr_cvtpd2ps_6972",
        &[0x66, 0x41, 0x0f, 0x5a, 0xd2],
        0xc51d,
        &[
            (2, 0x79981bc6c223b9efdcde7ffb576b29cd),
            (10, 0xbff6ccc9efffffffc7e8000000000000),
        ],
        Expect::Fault(0xc53d),
    ),
    (
        "rr_cvtpd2ps_6984",
        &[0x66, 0x45, 0x0f, 0x5a, 0xed],
        0x1b80,
        &[(13, 0xfff0000000000000b690000000000001)],
        Expect::Ok(0x0000000000000000ff80000080000001, 0x1bb0),
    ),
    (
        "rr_cvtpd2ps_738",
        &[0x66, 0x44, 0x0f, 0x5a, 0xc6],
        0xfd80,
        &[
            (6, 0x30fd55bf23ddff59400dda2ae6cbb283),
            (8, 0xecce57e171f781ba77a1da1df629b566),
        ],
        Expect::Ok(0x000000000000000000000000406ed157, 0xfdb0),
    ),
    (
        "rr_cvtpd2ps_788",
        &[0x66, 0x41, 0x0f, 0x5a, 0xfb],
        0x53e1,
        &[
            (7, 0xd7dc8c1268f60e46431f6c1257e8a00c),
            (11, 0xc323ef593bebab5a20e9c454aac7acdb),
        ],
        Expect::Fault(0x53f1),
    ),
    (
        "rr_cvtpd2ps_915",
        &[0x66, 0x0f, 0x5a, 0xd8],
        0x7332,
        &[
            (0, 0x8024b4404acb426543229497eaccad38),
            (3, 0x19c57e9554a5b987d40a316230da42c1),
        ],
        Expect::Fault(0x7332),
    ),
    (
        "rr_cvtpd2ps_94",
        &[0x66, 0x41, 0x0f, 0x5a, 0xe3],
        0x6d84,
        &[
            (4, 0xbda71f55dc173322d60af0201560210c),
            (11, 0x0003db45bcb48a34c0074d51c88291e4),
        ],
        Expect::Fault(0x6db6),
    ),
    (
        "rr_cvtps2pd_102",
        &[0x0f, 0x5a, 0xd8],
        0x1f95,
        &[
            (0, 0x000b7f274bce8cd70007fdf90b6ae1e2),
            (3, 0x3fe30a05ec717ae76918b4f54fbceb0f),
        ],
        Expect::Ok(0x37cff7e400000000396d5c3c40000000, 0x1f97),
    ),
    (
        "rr_cvtps2pd_1035",
        &[0x41, 0x0f, 0x5a, 0xfb],
        0x5e80,
        &[
            (7, 0x731042e2c8061b367a2ad6220de6e50d),
            (11, 0xffa6affacb3c7d80ff8e5399cc09b208),
        ],
        Expect::Ok(0xfff9ca7320000000c181364100000000, 0x5e81),
    ),
    (
        "rr_cvtps2pd_1080",
        &[0x0f, 0x5a, 0xec],
        0xd635,
        &[
            (4, 0xb00071823ffcfe4f803d372674da3eed),
            (5, 0x033847bf24464e6f27720de1b38c2c94),
        ],
        Expect::Fault(0xd637),
    ),
    (
        "rr_cvtps2pd_1115",
        &[0x44, 0x0f, 0x5a, 0xf0],
        0xa8aa,
        &[
            (0, 0xff52a93aff25e1d780335bc5cc220a9d),
            (14, 0x06b4376c9a83ad7c11f8f4adbeb695d8),
        ],
        Expect::Fault(0xa8aa),
    ),
    (
        "rr_cvtps2pd_1122",
        &[0x45, 0x0f, 0x5a, 0xea],
        0x3a1d,
        &[
            (10, 0x003e744a0b4d795ccc49f9e8ff9edd2d),
            (13, 0x1c7cd8aa8225e0ecdf0b8a2de5cdc6bc),
        ],
        Expect::Fault(0x3a1d),
    ),
    (
        "rr_cvtps2pd_1255",
        &[0x0f, 0x5a, 0xc7],
        0xfe80,
        &[
            (0, 0x1e045edd138856e129d04f00f6ffc680),
            (7, 0x80cd04bc7fc6873d4074872b4012f360),
        ],
        Expect::Ok(0x400e90e56000000040025e6c00000000, 0xfe80),
    ),
    (
        "rr_cvtps2pd_1290",
        &[0x44, 0x0f, 0x5a, 0xf4],
        0x728a,
        &[
            (4, 0xcbacadabffe7b6e8b05ea1ff0007e940),
            (14, 0xa1db75c93841e26b3e3549f293ac4f4b),
        ],
        Expect::Fault(0x728a),
    ),
    (
        "rr_cvtps2pd_1358",
        &[0x44, 0x0f, 0x5a, 0xe3],
        0x2000,
        &[
            (3, 0x0055d0574b5c1847bf99f5e8004ea2a6),
            (12, 0x9ae87792779608fc381bb9a469e537d0),
        ],
        Expect::Fault(0x2002),
    ),
    (
        "rr_cvtps2pd_1461",
        &[0x41, 0x0f, 0x5a, 0xd9],
        0x6000,
        &[
            (3, 0xeb40d660c6f96a5631e31e442af7f4e5),
            (9, 0x3f8bcdac803d9162eabce8f200755289),
        ],
        Expect::Fault(0x6002),
    ),
    (
        "rr_cvtps2pd_1504",
        &[0x0f, 0x5a, 0xed],
        0xbf72,
        &[(5, 0x7aa1c3d301a0520c0017fa3c4b6d9279)],
        Expect::Ok(0x0000000000000000416db24f20000000, 0xbf72),
    ),
    (
        "rr_cvtps2pd_1520",
        &[0x41, 0x0f, 0x5a, 0xca],
        0x5675,
        &[
            (1, 0x7767c1d7d8688c9cd268687087e52b85),
            (10, 0xb04d2e90806872588b6bb2c04bc75a34),
        ],
        Expect::Ok(0xb96d7658000000004178eb4680000000, 0x5675),
    ),
    (
        "rr_cvtps2pd_1538",
        &[0x44, 0x0f, 0x5a, 0xdf],
        0xff00,
        &[
            (7, 0xc3b88989f4a4a66b7fba523bffdf23b6),
            (11, 0xc2cdab5d8a38c5bb6f659b04b6c99a7f),
        ],
        Expect::Fault(0xff01),
    ),
    (
        "rr_cvtps2pd_1565",
        &[0x45, 0x0f, 0x5a, 0xef],
        0xb024,
        &[
            (13, 0xda7f2ac7f24d7f05aec2529ebe7aaf7c),
            (15, 0x803640f24b7ec5130149c981c03ec707),
        ],
        Expect::Ok(0x3829393020000000c007d8e0e0000000, 0xb024),
    ),
    (
        "rr_cvtps2pd_1613",
        &[0x41, 0x0f, 0x5a, 0xcc],
        0x4ca3,
        &[
            (1, 0x20d82ef965c2effc2da2c83716222411),
            (12, 0x7f91ab2b8b1c53fb0b262b2e0016f6c8),
        ],
        Expect::Fault(0x4ca3),
    ),
    (
        "rr_cvtps2pd_1644",
        &[0x41, 0x0f, 0x5a, 0xf9],
        0xd807,
        &[
            (7, 0xb852fb9ee685595e8cb215fd7f4a0ec0),
            (9, 0xcb86ef1ccabbd55a003282b8cc7bf039),
        ],
        Expect::Fault(0xd807),
    ),
    (
        "rr_cvtps2pd_1662",
        &[0x41, 0x0f, 0x5a, 0xd4],
        0xb4a1,
        &[
            (2, 0x8935c45d638dc37f995fc754cd18387e),
            (12, 0x000ce7e9802f753900561f89c05cad61),
        ],
        Expect::Fault(0xb4a3),
    ),
    (
        "rr_cvtps2pd_169",
        &[0x44, 0x0f, 0x5a, 0xdc],
        0x2f80,
        &[
            (4, 0x00c70c11cbb426db8047c7934f8edf12),
            (11, 0x3e4638e583bd660d9d99da630303fc89),
        ],
        Expect::Ok(0xb801f1e4c000000041f1dbe240000000, 0x2f82),
    ),
    (
        "rr_cvtps2pd_1727",
        &[0x41, 0x0f, 0x5a, 0xee],
        0xa42f,
        &[
            (5, 0xbd74e27243dfd1d57cef3bbca16c3f86),
            (14, 0x4bd290cefffbed5a00c06402806e098d),
        ],
        Expect::Fault(0xa42f),
    ),
    (
        "rr_cvtps2pd_179",
        &[0x44, 0x0f, 0x5a, 0xc9],
        0x1a00,
        &[
            (1, 0x00f009274aa3e204003d3d764b2c8680),
            (9, 0x9103b6e232c28854ff70d099fe911e49),
        ],
        Expect::Fault(0x1a02),
    ),
    (
        "rr_cvtps2pd_1856",
        &[0x45, 0x0f, 0x5a, 0xda],
        0xb8d0,
        &[
            (10, 0x4f978002ffbd82af0074713a4ff96d05),
            (11, 0x6df865c9649e3d26dba55310f3ddd1e6),
        ],
        Expect::Ok(0x000000000000000041ff2da0a0000000, 0xb8d0),
    ),
    (
        "rr_cvtps2pd_1912",
        &[0x44, 0x0f, 0x5a, 0xd8],
        0x4e34,
        &[
            (0, 0x00ccbdfa3036bdc1007e8458f4c5203f),
            (11, 0xe4f0531270f553950a5d0617b2309b40),
        ],
        Expect::Fault(0x4e36),
    ),
    (
        "rr_cvtps2pd_1941",
        &[0x45, 0x0f, 0x5a, 0xe9],
        0x503b,
        &[
            (9, 0x74a9b2353021879530429726ff955a79),
            (13, 0x630d90610aa8046a8a53e1ab3829c76d),
        ],
        Expect::Fault(0x503b),
    ),
    (
        "rr_cvtps2pd_1944",
        &[0x44, 0x0f, 0x5a, 0xc4],
        0x6000,
        &[
            (4, 0x304ffe7f8146c594b0524378806b63d0),
            (8, 0xba064a26d68486dbb2f3906114a77cda),
        ],
        Expect::Fault(0x6002),
    ),
    (
        "rr_cvtps2pd_1955",
        &[0x41, 0x0f, 0x5a, 0xdd],
        0x846c,
        &[
            (3, 0x296cda8f639da2ef7b9e1477b82ee472),
            (13, 0xcb432cf04a96399500d1659bff488452),
        ],
        Expect::Ok(0x381a2cb360000000c7e9108a40000000, 0x846c),
    ),
    (
        "rr_cvtps2pd_2065",
        &[0x0f, 0x5a, 0xed],
        0xbca9,
        &[(5, 0x7ffdb2ebbfc17af28174b51c802d49c6)],
        Expect::Fault(0xbcab),
    ),
    (
        "rr_cvtps2pd_2124",
        &[0x41, 0x0f, 0x5a, 0xde],
        0x801f,
        &[
            (3, 0x3e924ead7e923f0cd57151d23f755ab0),
            (14, 0x4c2ec0cb8b2070da7fbc2d73cb88b015),
        ],
        Expect::Fault(0x801f),
    ),
    (
        "rr_cvtps2pd_2165",
        &[0x41, 0x0f, 0x5a, 0xd8],
        0xa6ea,
        &[
            (3, 0xbcb65f824aed1401bf0994d3c326e921),
            (8, 0x8025876a0c85f0c400d9fdb180d95e39),
        ],
        Expect::Ok(0x381b3fb620000000b81b2bc720000000, 0xa6ea),
    ),
    (
        "rr_cvtps2pd_2201",
        &[0x44, 0x0f, 0x5a, 0xec],
        0xd491,
        &[
            (4, 0xc00764b9bfa62bee803a11d2bfff9c86),
            (13, 0xa79fab014ad1a126bd84611c3e3b8c58),
        ],
        Expect::Fault(0xd493),
    ),
    (
        "rr_cvtps2pd_2215",
        &[0x41, 0x0f, 0x5a, 0xe4],
        0x4f80,
        &[
            (4, 0xa1ad92260d3291dcf18cdd11f58cbed7),
            (12, 0x4c2c93630b7caf750b5926ee80570f9f),
        ],
        Expect::Ok(0x396b24ddc0000000b805c3e7c0000000, 0x4f82),
    ),
    (
        "rr_cvtps2pd_2342",
        &[0x41, 0x0f, 0x5a, 0xfb],
        0x9f00,
        &[
            (7, 0x6fb197ec679fdaa1e465ed306bd36b11),
            (11, 0xcbba542d8b3724b84056862e4b7979d7),
        ],
        Expect::Ok(0x400ad0c5c0000000416f2f3ae0000000, 0x9f00),
    ),
    (
        "rr_cvtps2pd_236",
        &[0x44, 0x0f, 0x5a, 0xc1],
        0x1700,
        &[
            (1, 0xcbe8ec1e8b769dd7ff21e7dc8151667a),
            (8, 0x5632d39db062d5ec991f43370e6ab208),
        ],
        Expect::Ok(0xc7e43cfb80000000b82a2ccf40000000, 0x1700),
    ),
    (
        "rr_cvtps2pd_2396",
        &[0x44, 0x0f, 0x5a, 0xd5],
        0xc7ae,
        &[
            (5, 0x7ffbf3c5cfed29fccb8a2021748422d6),
            (10, 0xf6bfdfa6f2595421377da67d146c948f),
        ],
        Expect::Ok(0xc1714404200000004690845ac0000000, 0xc7ae),
    ),
    (
        "rr_cvtps2pd_2439",
        &[0x0f, 0x5a, 0xc9],
        0x7d80,
        &[(1, 0x4aa9b63e00ea545080347d5aff5faa6d)],
        Expect::Ok(0xb7fa3ead00000000c7ebf54da0000000, 0x7d82),
    ),
    (
        "rr_cvtps2pd_2450",
        &[0x45, 0x0f, 0x5a, 0xe0],
        0x5e80,
        &[
            (8, 0x8b6e95fa4adf81eaf4aeaa497f685a9b),
            (12, 0x04e0e35ed4a86dbe94249115e351c0f4),
        ],
        Expect::Ok(0xc695d5492000000047ed0b5360000000, 0x5e80),
    ),
    (
        "rr_cvtps2pd_2531",
        &[0x41, 0x0f, 0x5a, 0xe1],
        0xae50,
        &[
            (4, 0xcefa737ce6bcced6303080437d3b8c19),
            (9, 0xe825cb3074e3822480d09a787f8fc827),
        ],
        Expect::Fault(0xae51),
    ),
    (
        "rr_cvtps2pd_2534",
        &[0x0f, 0x5a, 0xc4],
        0x7bc0,
        &[
            (0, 0xa77c706d3e4dc88402df8d482bd3d8c7),
            (4, 0xffd2df9274d79a894b46a725ff57d212),
        ],
        Expect::Ok(0x4168d4e4a0000000c7eafa4240000000, 0x7bc0),
    ),
    (
        "rr_cvtps2pd_2591",
        &[0x41, 0x0f, 0x5a, 0xf9],
        0xc114,
        &[
            (7, 0x136ea0f8ec19048aecd4df3e90d69611),
            (9, 0x74c20b200132407b801d859a802d04fa),
        ],
        Expect::Ok(0xb7ed859a00000000b7f6827d00000000, 0xc116),
    ),
    (
        "rr_cvtps2pd_2618",
        &[0x0f, 0x5a, 0xed],
        0xfe80,
        &[(5, 0x4c7d93e680f2f037b03c9b8100678ada)],
        Expect::Fault(0xfe82),
    ),
    (
        "rr_cvtps2pd_2642",
        &[0x45, 0x0f, 0x5a, 0xfc],
        0xe496,
        &[
            (12, 0xffa6b6ca406f390dcb7dc727004834f3),
            (15, 0xc2efdd9aee3807167149c09c686973d2),
        ],
        Expect::Fault(0xe496),
    ),
    (
        "rr_cvtps2pd_2699",
        &[0x45, 0x0f, 0x5a, 0xc3],
        0xff00,
        &[
            (8, 0x0c5c93ad448c7e1bc85d49dec41a6f3a),
            (11, 0xffee21833fa7b818bf8e72c10067b749),
        ],
        Expect::Ok(0xbff1ce58200000003809edd240000000, 0xff02),
    ),
    (
        "rr_cvtps2pd_2702",
        &[0x0f, 0x5a, 0xd0],
        0xa608,
        &[
            (0, 0x4fe56633810d69154fa748a78038d5f9),
            (2, 0xce1a72398176d927f965d15db9222463),
        ],
        Expect::Fault(0xa60a),
    ),
    (
        "rr_cvtps2pd_2712",
        &[0x44, 0x0f, 0x5a, 0xdf],
        0xb018,
        &[
            (7, 0x4fc02d2fcc757670ffe85a7f7fab2672),
            (11, 0x2bb6635d0095fb231e5e80deb8e965f4),
        ],
        Expect::Fault(0xb019),
    ),
    (
        "rr_cvtps2pd_2732",
        &[0x44, 0x0f, 0x5a, 0xc7],
        0xcb30,
        &[
            (7, 0x8056d7f24ab61eaf74b912a4ffa763a6),
            (8, 0xd7f747eecdc619e2879c88415be17cd6),
        ],
        Expect::Fault(0xcb31),
    ),
    (
        "rr_cvtps2pd_2829",
        &[0x41, 0x0f, 0x5a, 0xd7],
        0x24ec,
        &[
            (2, 0xf2069b53210460b313dafc8d04a6070f),
            (15, 0x304408dd4c517b82ffb0625bcb8e812f),
        ],
        Expect::Ok(0xfffe0c4b60000000c171d025e0000000, 0x24ed),
    ),
    (
        "rr_cvtps2pd_2917",
        &[0x44, 0x0f, 0x5a, 0xc8],
        0xf62d,
        &[
            (0, 0xcc5200c57f663f48ff97659b8b66760d),
            (9, 0x034b7bb8da28b023431cef9c0cfdd2d3),
        ],
        Expect::Fault(0xf62d),
    ),
    (
        "rr_cvtps2pd_294",
        &[0x45, 0x0f, 0x5a, 0xd6],
        0x1fbf,
        &[
            (10, 0xcb97a8eaf6a034ef8014ce0ac4ad03f6),
            (14, 0x8b6226ec7fd629498109da01ffe5aa70),
        ],
        Expect::Ok(0xb8213b4020000000fffcb54e00000000, 0x1fbf),
    ),
    (
        "rr_cvtps2pd_2993",
        &[0x44, 0x0f, 0x5a, 0xe6],
        0x80a0,
        &[
            (6, 0x7f0c34ddcc65a367cb92b5700056c183),
            (12, 0x3bf424de63d1ee2fab16c657f882c674),
        ],
        Expect::Fault(0x80a2),
    ),
    (
        "rr_cvtps2pd_305",
        &[0x44, 0x0f, 0x5a, 0xf3],
        0xa9a7,
        &[
            (3, 0x401cafc1806b77564031ee5eb04f2b5f),
            (14, 0xa87a09baeddd3f29ab0e42e04a0e3750),
        ],
        Expect::Ok(0x40063dcbc0000000be09e56be0000000, 0xa9a7),
    ),
    (
        "rr_cvtps2pd_3076",
        &[0x44, 0x0f, 0x5a, 0xd3],
        0x9a3a,
        &[
            (3, 0x4c40ab024ab6fafc003f2d18f4d22c9e),
            (10, 0xf3dd48cfdbbc4f45fd43411ec329c638),
        ],
        Expect::Fault(0x9a3a),
    ),
    (
        "rr_cvtps2pd_3125",
        &[0x0f, 0x5a, 0xd3],
        0x960f,
        &[
            (2, 0xfe6f911609e7f760c2ee5f5c9154ca86),
            (3, 0x4afe458f8128283400376110c02e097b),
        ],
        Expect::Fault(0x960f),
    ),
    (
        "rr_cvtps2pd_3279",
        &[0x0f, 0x5a, 0xc8],
        0x7d80,
        &[
            (0, 0xcc646df2014f4e994b7fea7a010a70a3),
            (1, 0x0f3ae53892c5144d76b7ce5c8fa63982),
        ],
        Expect::Ok(0x416ffd4f4000000038214e1460000000, 0x7d80),
    ),
    (
        "rr_cvtps2pd_3297",
        &[0x45, 0x0f, 0x5a, 0xc1],
        0x3e12,
        &[
            (8, 0x695c32bbf45a18855b71e1b7fa2f1ab4),
            (9, 0x8029deb8ff34ef694fa9b0bb0062b691),
        ],
        Expect::Fault(0x3e12),
    ),
    (
        "rr_cvtps2pd_3337",
        &[0x41, 0x0f, 0x5a, 0xdf],
        0x76b7,
        &[
            (3, 0x2bdc6da181a1a37eaccbd9a04e32b2df),
            (15, 0xcc7e1251ffd7cb500044951580799a5e),
        ],
        Expect::Fault(0x76b7),
    ),
    (
        "rr_cvtps2pd_3415",
        &[0x41, 0x0f, 0x5a, 0xce],
        0x3f00,
        &[
            (1, 0x8725e8c4e6732cf06dd008ca7da21377),
            (14, 0x810780b68058386c7fd47c8980107bd1),
        ],
        Expect::Ok(0x7ffa8f9120000000b7e07bd100000000, 0x3f02),
    ),
    (
        "rr_cvtps2pd_3478",
        &[0x41, 0x0f, 0x5a, 0xd0],
        0xcf84,
        &[
            (2, 0x1f998fafbde87b6f59c81f97c4599d94),
            (8, 0x0b15af91ff72fd084bc72fba7fac4bb2),
        ],
        Expect::Ok(0x4178e5f7400000007ffd897640000000, 0xcf85),
    ),
    (
        "rr_cvtps2pd_3531",
        &[0x41, 0x0f, 0x5a, 0xf1],
        0xd77e,
        &[
            (6, 0x8b536a06f95c9de50d6b65a6464d1a06),
            (9, 0x8055d020806d13dccb88a2c34fde55e6),
        ],
        Expect::Ok(0xc17114586000000041fbcabcc0000000, 0xd77e),
    ),
    (
        "rr_cvtps2pd_362",
        &[0x41, 0x0f, 0x5a, 0xfb],
        0xbf28,
        &[
            (7, 0x0224ab3498be9eed4292542f10ec3235),
            (11, 0x0075a2a98044e335805a8b764bc82ec5),
        ],
        Expect::Ok(0xb806a2dd80000000417905d8a0000000, 0xbf2a),
    ),
    (
        "rr_cvtps2pd_3694",
        &[0x45, 0x0f, 0x5a, 0xfa],
        0xdb89,
        &[
            (10, 0x7fe4efd1f4a1683c4a901904af1e8660),
            (15, 0xc33887b70d1f2797ac830a654dd3cda9),
        ],
        Expect::Ok(0x4152032080000000bde3d0cc00000000, 0xdb89),
    ),
    (
        "rr_cvtps2pd_3702",
        &[0x44, 0x0f, 0x5a, 0xf5],
        0x2ee0,
        &[
            (5, 0x7feb826440a22e95817a06d4f4b0155a),
            (14, 0x9023ee35dc60f8475a639928765db14b),
        ],
        Expect::Ok(0xb82f40da80000000c69602ab40000000, 0x2ee0),
    ),
    (
        "rr_cvtps2pd_3754",
        &[0x41, 0x0f, 0x5a, 0xf3],
        0x2d17,
        &[
            (6, 0x08e6e946db4b2ee0207e3d1c88fd7231),
            (11, 0x63c46cc04aca9c517f63e47f7fb42329),
        ],
        Expect::Fault(0x2d17),
    ),
    (
        "rr_cvtps2pd_3859",
        &[0x0f, 0x5a, 0xd0],
        0xcdb8,
        &[
            (0, 0x01777d13ff9be6ad4ba548df000aa001),
            (2, 0x53778190e701e8dcaed63370e19794fb),
        ],
        Expect::Ok(0x4174a91be000000037d5400200000000, 0xcdba),
    ),
    (
        "rr_cvtps2pd_3874",
        &[0x44, 0x0f, 0x5a, 0xc3],
        0x4000,
        &[
            (3, 0xcba315fd014b802c0053b0b7ffc57037),
            (8, 0x1249cbe25c17d11eacd5ce3a3aed47ff),
        ],
        Expect::Fault(0x4002),
    ),
    (
        "rr_cvtps2pd_4066",
        &[0x41, 0x0f, 0x5a, 0xd8],
        0xd890,
        &[
            (3, 0xcfc5796518e3dcc33fa2315bfff68bdd),
            (8, 0xff72d7e28b4b0afd801e02e97fb53b94),
        ],
        Expect::Fault(0xd893),
    ),
    (
        "rr_cvtps2pd_4146",
        &[0x0f, 0x5a, 0xf9],
        0x1971,
        &[
            (1, 0x74825f498161bec0815204ba7fa2326f),
            (7, 0x89f8904c3fcc3ec5803ee972b872950b),
        ],
        Expect::Fault(0x1971),
    ),
    (
        "rr_cvtps2pd_4242",
        &[0x44, 0x0f, 0x5a, 0xf0],
        0x2f80,
        &[
            (0, 0xff14135532f6dcc38b5bc48f8004adfc),
            (14, 0xe84538bc8668673ae816d4fac5b60886),
        ],
        Expect::Ok(0xb96b7891e0000000b7c2b7f000000000, 0x2f82),
    ),
    (
        "rr_cvtps2pd_4281",
        &[0x41, 0x0f, 0x5a, 0xfa],
        0x6ebf,
        &[
            (7, 0x330d273a1a7424dd4994a63a6b051a06),
            (10, 0x809b9728ca9915508b5adbb3805e28b1),
        ],
        Expect::Fault(0x6ebf),
    ),
    (
        "rr_cvtps2pd_4328",
        &[0x41, 0x0f, 0x5a, 0xe3],
        0xad84,
        &[
            (4, 0xd531bd998cc1d4cc935afb9b476dc3ca),
            (11, 0x406e4bbb8b10b2174c4130de803265d8),
        ],
        Expect::Ok(0x4188261bc0000000b7f932ec00000000, 0xad86),
    ),
    (
        "rr_cvtps2pd_4332",
        &[0x44, 0x0f, 0x5a, 0xe8],
        0x17c0,
        &[
            (0, 0xcf822148000042a33fa8f55b4b41e599),
            (13, 0x3ee18d50a061ceac7ba919f6bd25aff0),
        ],
        Expect::Ok(0x3ff51eab6000000041683cb320000000, 0x17c0),
    ),
    (
        "rr_cvtps2pd_4460",
        &[0x45, 0x0f, 0x5a, 0xf4],
        0x56dc,
        &[
            (12, 0xcf8c105f4a9819a17f9cbdcf80933dec),
            (14, 0xc4e6a05591e555da0123d14b62ba6d8d),
        ],
        Expect::Ok(0x7ffb97b9e0000000b81267bd80000000, 0x56dd),
    ),
    (
        "rr_cvtps2pd_4610",
        &[0x0f, 0x5a, 0xe3],
        0x1780,
        &[
            (3, 0xf4beae7acff464dfff65cad3c0088660),
            (4, 0xa437ec388fe24d8ef7fc5ddd2d62ee27),
        ],
        Expect::Ok(0xc7ecb95a60000000c00110cc00000000, 0x1780),
    ),
    (
        "rr_cvtps2pd_4761",
        &[0x41, 0x0f, 0x5a, 0xee],
        0x701e,
        &[
            (5, 0x5342f8511d371fef0d88c299c12f20b7),
            (14, 0x4ab722a14aaf291f001d60e100b2fe47),
        ],
        Expect::Fault(0x701e),
    ),
    (
        "rr_cvtps2pd_4955",
        &[0x41, 0x0f, 0x5a, 0xc0],
        0x5f80,
        &[
            (0, 0x2c6181fa1871bc0721b50723084770e0),
            (8, 0x7fc1e73880652d343fc41a16bfd38596),
        ],
        Expect::Ok(0x3ff88342c0000000bffa70b2c0000000, 0x5f80),
    ),
    (
        "rr_cvtps2pd_497",
        &[0x45, 0x0f, 0x5a, 0xe8],
        0xe21f,
        &[
            (8, 0xcc577cd200522ee98003384722808461),
            (13, 0x18aeb719ef52e9f518a54fb77bf03501),
        ],
        Expect::Fault(0xe21f),
    ),
    (
        "rr_cvtps2pd_5087",
        &[0x0f, 0x5a, 0xf2],
        0x2f4d,
        &[
            (2, 0x3a7a06dd303237cfff8a757ed762fe00),
            (6, 0xed4828f998a9814860a945cfdecbfbb0),
        ],
        Expect::Fault(0x2f4d),
    ),
    (
        "rr_cvtps2pd_5119",
        &[0x0f, 0x5a, 0xc5],
        0x9b80,
        &[
            (0, 0x0df3562740c48f653713b927d198b25d),
            (5, 0x807ffae90b7c60068b1fe1e1805e6b33),
        ],
        Expect::Ok(0xb963fc3c20000000b8079accc0000000, 0x9b82),
    ),
    (
        "rr_cvtps2pd_518",
        &[0x45, 0x0f, 0x5a, 0xc9],
        0xb63c,
        &[(9, 0x74db796c7fce169c0b2fd4ca013340d9)],
        Expect::Ok(0x3965fa99400000003826681b20000000, 0xb63c),
    ),
    (
        "rr_cvtps2pd_5247",
        &[0x44, 0x0f, 0x5a, 0xe7],
        0x9b53,
        &[
            (7, 0xcad976364aa0571d811efbe0ffadefdd),
            (12, 0x6b069a8703f56b175305f3686f12fd80),
        ],
        Expect::Fault(0x9b53),
    ),
    (
        "rr_cvtps2pd_5262",
        &[0x44, 0x0f, 0x5a, 0xe1],
        0x4cab,
        &[
            (1, 0xcb6b456e3047ab420067c1ffcc1c906e),
            (12, 0xb50961ea3b3723bdea7073bd3a5ca67c),
        ],
        Expect::Fault(0x4cab),
    ),
    (
        "rr_cvtps2pd_5285",
        &[0x45, 0x0f, 0x5a, 0xca],
        0xcbbc,
        &[
            (9, 0xbf18452222eee651a3d6e2726294fdcb),
            (10, 0xb02a3fdc4f9f76404ba3ef64ff180c0a),
        ],
        Expect::Ok(0x41747dec80000000c7e3018140000000, 0xcbbc),
    ),
    (
        "rr_cvtps2pd_538",
        &[0x41, 0x0f, 0x5a, 0xca],
        0xae4e,
        &[
            (1, 0x78ba84921fdaa5cd79a69b68506a55b3),
            (10, 0xb0134d087f280ce6cb4f785bff9b7375),
        ],
        Expect::Fault(0xae4f),
    ),
    (
        "rr_cvtps2pd_5421",
        &[0x45, 0x0f, 0x5a, 0xe2],
        0xb3aa,
        &[
            (10, 0x81194dde8064ab0eff86b9664c2415a2),
            (12, 0xc6709cb0fe36db9bf4cb98986bd63f5e),
        ],
        Expect::Ok(0xfff8d72cc0000000418482b440000000, 0xb3ab),
    ),
    (
        "rr_cvtps2pd_5520",
        &[0x0f, 0x5a, 0xd7],
        0xea16,
        &[
            (2, 0xeb9c80d83c32f1302649283a15c5d819),
            (7, 0x8b6d2b204b55400b800e2c42cfb126ad),
        ],
        Expect::Fault(0xea16),
    ),
    (
        "rr_cvtps2pd_5606",
        &[0x45, 0x0f, 0x5a, 0xe6],
        0x77c0,
        &[
            (12, 0x940d30592b0706ab5fc762ad5db5c7d0),
            (14, 0x0139ceff400b94177f53100ccbd9d033),
        ],
        Expect::Ok(0x47ea620180000000c17b3a0660000000, 0x77c0),
    ),
    (
        "rr_cvtps2pd_5634",
        &[0x45, 0x0f, 0x5a, 0xf2],
        0x1c17,
        &[
            (10, 0x4acdf6360070a596801fe52acc0c27d5),
            (14, 0x7fba2ef2ffce4887299f1c8826b165ef),
        ],
        Expect::Fault(0x1c17),
    ),
    (
        "rr_cvtps2pd_57",
        &[0x45, 0x0f, 0x5a, 0xcd],
        0x465a,
        &[
            (9, 0x0494928082d3bdd45ff2f163d8678311),
            (13, 0xcb1caeeafff18153403c434ccc7de1f3),
        ],
        Expect::Ok(0x4007886980000000c18fbc3e60000000, 0x465a),
    ),
    (
        "rr_cvtps2pd_5739",
        &[0x44, 0x0f, 0x5a, 0xd8],
        0x1dc0,
        &[
            (0, 0x7f01e9ad4bd2ddcccbd9115a7fa253d8),
            (11, 0xbff4325d3ecfe61e345afcb5782f11a6),
        ],
        Expect::Ok(0xc17b222b400000007ffc4a7b00000000, 0x1dc1),
    ),
    (
        "rr_cvtps2pd_5750",
        &[0x45, 0x0f, 0x5a, 0xf0],
        0x84b6,
        &[
            (8, 0xffe5409800074eb6801d45634fe81629),
            (14, 0xd4b6bbba855cbac6379cff7799782737),
        ],
        Expect::Fault(0x84b6),
    ),
    (
        "rr_cvtps2pd_5841",
        &[0x41, 0x0f, 0x5a, 0xea],
        0xe04f,
        &[
            (5, 0xfd5cb906f729c914ebfa3e15648333d0),
            (10, 0x40270136cfea6143816e6437ff990c27),
        ],
        Expect::Fault(0xe04f),
    ),
    (
        "rr_cvtps2pd_5858",
        &[0x41, 0x0f, 0x5a, 0xda],
        0xd505,
        &[
            (3, 0x7a55aebca6fa438fa71e7eb315732cc0),
            (10, 0x806df7d45ec28a82748be3197f95afdc),
        ],
        Expect::Fault(0xd505),
    ),
    (
        "rr_cvtps2pd_5864",
        &[0x44, 0x0f, 0x5a, 0xc2],
        0x268a,
        &[
            (2, 0xf4dbb57eff5308627fc615b94ab09b91),
            (8, 0x98bb68f2ea1d802c588798f94787132c),
        ],
        Expect::Ok(0x7ff8c2b7200000004156137220000000, 0x268a),
    ),
    (
        "rr_cvtps2pd_5970",
        &[0x0f, 0x5a, 0xc5],
        0x4abb,
        &[
            (0, 0xbcb555f84198d8baea9de707abe3c58a),
            (5, 0x001fba324ba9dfc6cab3909a803a4d84),
        ],
        Expect::Fault(0x4abb),
    ),
    (
        "rr_cvtps2pd_6030",
        &[0x45, 0x0f, 0x5a, 0xd2],
        0x5de8,
        &[(10, 0xcc5ba4e1805835537fa894f274cd6a40)],
        Expect::Ok(0x7ffd129e400000004699ad4800000000, 0x5de9),
    ),
    (
        "rr_cvtps2pd_6051",
        &[0x45, 0x0f, 0x5a, 0xda],
        0xfe80,
        &[
            (10, 0x4b8ac954302701e80b0d8d1800621c49),
            (11, 0xa65b8b5a57152d270cad44fcfc953851),
        ],
        Expect::Fault(0xfe82),
    ),
    (
        "rr_cvtps2pd_6131",
        &[0x41, 0x0f, 0x5a, 0xe7],
        0xef80,
        &[
            (4, 0x09a7e81d5a88fb5eda9d9c8f2a619ccf),
            (15, 0x006be7ca8b1420bbcb84d31d009845fa),
        ],
        Expect::Ok(0xc1709a63a0000000381308bf40000000, 0xef80),
    ),
    (
        "rr_cvtps2pd_6413",
        &[0x44, 0x0f, 0x5a, 0xfe],
        0x7e6d,
        &[
            (6, 0x4b6096e4c04c7272ffabe5fccb737b35),
            (15, 0x76074a3c6dfda5aef9c47922157b1a67),
        ],
        Expect::Fault(0x7e6d),
    ),
    (
        "rr_cvtps2pd_6498",
        &[0x0f, 0x5a, 0xfa],
        0x5780,
        &[
            (2, 0x401791258b7403be4b51c8a23065166d),
            (7, 0x634ae99d60f6a142ac78ba05732bdc1f),
        ],
        Expect::Ok(0x416a3914400000003e0ca2cda0000000, 0x5780),
    ),
    (
        "rr_cvtps2pd_6532",
        &[0x41, 0x0f, 0x5a, 0xd8],
        0x5e80,
        &[
            (3, 0x5967da248c305d311eccf4bf800aca26),
            (8, 0x4ae90d4200638740cbf749e6003213b1),
        ],
        Expect::Fault(0x5e82),
    ),
    (
        "rr_cvtps2pd_6557",
        &[0x45, 0x0f, 0x5a, 0xe0],
        0x2000,
        &[
            (8, 0xca6c0fab2b94320bcc24c12b00018335),
            (12, 0xda8634f38277ed76a693fb114cc454e7),
        ],
        Expect::Fault(0x2002),
    ),
    (
        "rr_cvtps2pd_6861",
        &[0x0f, 0x5a, 0xfe],
        0xa102,
        &[
            (6, 0x3fdc9bb74b253bbc8b7de50dffeafe06),
            (7, 0x64566732b02430d2ba536f280173bf09),
        ],
        Expect::Ok(0xb96fbca1a0000000fffd5fc0c0000000, 0xa102),
    ),
    (
        "rr_cvtps2pd_6927",
        &[0x44, 0x0f, 0x5a, 0xce],
        0xc09a,
        &[
            (6, 0x4ab1531fcb000ffd404aa0af007bd641),
            (9, 0x40cda712a0ae480d5af44b351acc1e61),
        ],
        Expect::Fault(0xc09a),
    ),
    (
        "rr_cvtps2pd_6995",
        &[0x41, 0x0f, 0x5a, 0xcc],
        0xdea8,
        &[
            (1, 0x3dbc0d8c87a754bf1ba42c86a5ab132b),
            (12, 0xb05ef8fdcfe667024c66721c74e44d6b),
        ],
        Expect::Ok(0x418cce4380000000469c89ad60000000, 0xdea8),
    ),
    (
        "rr_cvtps2pd_783",
        &[0x44, 0x0f, 0x5a, 0xe8],
        0xccca,
        &[
            (0, 0xffe003e4f4d9c151bb85ceeb0055a607),
            (13, 0x89818097136f514c902ac05f610db2ef),
        ],
        Expect::Ok(0xbf70b9dd600000000000000000000000, 0xccca),
    ),
    (
        "rr_cvtps2pd_802",
        &[0x45, 0x0f, 0x5a, 0xc9],
        0x7b80,
        &[(9, 0xffebe28900234b680b3e2dc7012e4ff3)],
        Expect::Ok(0x3967c5b8e00000003825c9fe60000000, 0x7b80),
    ),
    (
        "rr_cvtps2pd_831",
        &[0x41, 0x0f, 0x5a, 0xe9],
        0xb2b6,
        &[
            (5, 0xf2138fc247a851e68b3e1eb79ea8bfc3),
            (9, 0x8012235fc0075a8f004d0164cfc8dd17),
        ],
        Expect::Fault(0xb2b6),
    ),
    (
        "rr_cvtps2pd_93",
        &[0x44, 0x0f, 0x5a, 0xfd],
        0x7f40,
        &[
            (5, 0xa5bff9068166e81629a53d71cbf50d09),
            (15, 0x47c08e34fcbce73897bb967e8052b4da),
        ],
        Expect::Ok(0x3d34a7ae20000000c17ea1a120000000, 0x7f40),
    ),
    (
        "rr_cvtsd2ss_1048",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd4],
        0x4f80,
        &[
            (10, 0xf61448d0066cf6c96e1510a2ef63e259),
            (12, 0x8b988420df639c3147e0000010000001),
        ],
        Expect::Fault(0x4fa0),
    ),
    (
        "rr_cvtsd2ss_1092",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf4],
        0x6936,
        &[
            (12, 0x4fb6ce0a548f250d3820000000000001),
            (14, 0x91c3407b0fbff20cd1c27e7e1cba729b),
        ],
        Expect::Fault(0x6936),
    ),
    (
        "rr_cvtsd2ss_1095",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xce],
        0xf780,
        &[
            (1, 0x3419c534e4eb08dabffd35d174c067d6),
            (14, 0x2d1777bca4c3be3a001131ca74abe857),
        ],
        Expect::Fault(0xf7b0),
    ),
    (
        "rr_cvtsd2ss_1134",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe3],
        0xadb9,
        &[
            (3, 0xb47708ac336668840028ff54fb6db202),
            (12, 0x13e94e761f0ed906d0e69ef2bc1de9f9),
        ],
        Expect::Fault(0xadb9),
    ),
    (
        "rr_cvtsd2ss_1182",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xed],
        0x7780,
        &[
            (5, 0x1322d936c7f937a8ae5e99c277610a2c),
            (13, 0x3fb81a6e5ba4dd00800e7880703fb1d5),
        ],
        Expect::Fault(0x77b2),
    ),
    (
        "rr_cvtsd2ss_1273",
        &[0xf2, 0x0f, 0x5a, 0xf5],
        0x46be,
        &[
            (5, 0xdc88aa1cd40d4402b68fffffffffffff),
            (6, 0x3c6a2198a23e36152445a2ea017a0b7f),
        ],
        Expect::Fault(0x46be),
    ),
    (
        "rr_cvtsd2ss_1447",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe1],
        0xfc0,
        &[
            (9, 0x103d449fbdbd81af8018b8d6828a6254),
            (12, 0xb028939182d26a4a1784dd311051ad01),
        ],
        Expect::Fault(0xff0),
    ),
    (
        "rr_cvtsd2ss_1566",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe5],
        0x34f7,
        &[
            (4, 0x84103f1f8ad974f5b1119fb905682a30),
            (13, 0xe461556324232ee87ff0000018000000),
        ],
        Expect::Ok(0x84103f1f8ad974f5b1119fb97fc00000, 0x34f7),
    ),
    (
        "rr_cvtsd2ss_1597",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xfe],
        0x5c7c,
        &[
            (14, 0x48e22754ab6230143ffdfae2b3f6f936),
            (15, 0x9c501da1ae86aab133171567f7ad9d56),
        ],
        Expect::Ok(0x9c501da1ae86aab1331715673fefd716, 0x5c7c),
    ),
    (
        "rr_cvtsd2ss_1610",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd1],
        0x4000,
        &[
            (1, 0x8701c6e21e2e0784ffde766d16abd23e),
            (10, 0x23b3852cde0334f03de006281139cd6b),
        ],
        Expect::Fault(0x4028),
    ),
    (
        "rr_cvtsd2ss_1613",
        &[0xf2, 0x0f, 0x5a, 0xce],
        0x1d80,
        &[
            (1, 0x763ea5116426fc50e5f240ec392885b0),
            (6, 0xcf5069fffaaa93697ff0000000000000),
        ],
        Expect::Ok(0x763ea5116426fc50e5f240ec7f800000, 0x1d80),
    ),
    (
        "rr_cvtsd2ss_1631",
        &[0xf2, 0x0f, 0x5a, 0xcd],
        0xdc01,
        &[
            (1, 0x7afda4e6ce95e0d0465a30b29f3d0211),
            (5, 0x650ef3926d6f73ffffd8c67697734cc8),
        ],
        Expect::Ok(0x7afda4e6ce95e0d0465a30b2ff7fffff, 0xdc29),
    ),
    (
        "rr_cvtsd2ss_1658",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf6],
        0x3f80,
        &[(14, 0x50fa3ef30d29b5bd47e0000000000000)],
        Expect::Ok(0x50fa3ef30d29b5bd47e000007f000000, 0x3f80),
    ),
    (
        "rr_cvtsd2ss_1721",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd4],
        0xa2c1,
        &[
            (10, 0x497feadfd295271cc15059a2a0cefce6),
            (12, 0x95fb762fc2450450382000000fffffff),
        ],
        Expect::Fault(0xa2e1),
    ),
    (
        "rr_cvtsd2ss_1739",
        &[0xf2, 0x0f, 0x5a, 0xd1],
        0xacf1,
        &[
            (1, 0xb3e836b9f7e7dab0b7f16088d8d2fc21),
            (2, 0x2cb4f5cb40653b15bd2637cdbf147af1),
        ],
        Expect::Fault(0xacf1),
    ),
    (
        "rr_cvtsd2ss_1774",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xfb],
        0xca06,
        &[
            (7, 0x5d17f3f1ee5db2042b065a0427fa8015),
            (11, 0xfca8fb47d237bc6e7ff0000018000000),
        ],
        Expect::Fault(0xca07),
    ),
    (
        "rr_cvtsd2ss_1809",
        &[0xf2, 0x0f, 0x5a, 0xf3],
        0x1b80,
        &[
            (3, 0xba158f74e13ac3f847e0000000000001),
            (6, 0x93670bbb548371e52bf4fa4523a7a665),
        ],
        Expect::Ok(0x93670bbb548371e52bf4fa457f000000, 0x1ba0),
    ),
    (
        "rr_cvtsd2ss_1919",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf8],
        0xfbf,
        &[
            (0, 0xe3e4fe8b7468ca62c34bb0d3b700d327),
            (15, 0x7c2a4270eeda3e0ee89f9d124d4cd8b4),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rr_cvtsd2ss_1923",
        &[0xf2, 0x0f, 0x5a, 0xce],
        0x26ba,
        &[
            (1, 0x5a134677d1799066558ed259b5edd122),
            (6, 0x8dada561965c1a847feba975a532eaf2),
        ],
        Expect::Fault(0x26ba),
    ),
    (
        "rr_cvtsd2ss_198",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc3],
        0x1700,
        &[
            (8, 0x367f71023b37037092517e227d72cb99),
            (11, 0xb3364b6eac46e4664006ac06dac46124),
        ],
        Expect::Ok(0x367f71023b37037092517e2240356037, 0x1720),
    ),
    (
        "rr_cvtsd2ss_2004",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xef],
        0x4926,
        &[
            (7, 0x63779fe3b8a4b48d3cb1e0df0c0eec32),
            (13, 0x7768071b337eb2cab354ec7c29f28264),
        ],
        Expect::Fault(0x4926),
    ),
    (
        "rr_cvtsd2ss_2022",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xcd],
        0xb47b,
        &[
            (5, 0xf706d644a35f1cb73800000010000001),
            (9, 0x74f0b85b1b59542215c068d2986b8aa2),
        ],
        Expect::Fault(0xb47b),
    ),
    (
        "rr_cvtsd2ss_204",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xce],
        0xc942,
        &[
            (1, 0x0293d4ecb79f05e8b167c92d9091554e),
            (14, 0x7c6a03fe47556cbebff772ee85f2bfe7),
        ],
        Expect::Fault(0xc962),
    ),
    (
        "rr_cvtsd2ss_2041",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe0],
        0xaf7b,
        &[
            (0, 0xcf023ea5c50b9bc03ff23ca3ff873087),
            (12, 0x496ba9f56bfdb1fa14f25c527f40cacc),
        ],
        Expect::Fault(0xaf7b),
    ),
    (
        "rr_cvtsd2ss_2053",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xed],
        0x3780,
        &[
            (5, 0x5af41bbb8d1dbad3802e144cbea7f665),
            (13, 0xc1839b9f90d81597a2c1f49d221db6fd),
        ],
        Expect::Fault(0x37b0),
    ),
    (
        "rr_cvtsd2ss_2111",
        &[0xf2, 0x0f, 0x5a, 0xf7],
        0xffc0,
        &[
            (6, 0x3f88e3cab20699a633f240840bdaf4bf),
            (7, 0x00fa98c47b3cb73d7ffbe03403f41ff5),
        ],
        Expect::Ok(0x3f88e3cab20699a633f240847fdf01a0, 0xffc0),
    ),
    (
        "rr_cvtsd2ss_2188",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd0],
        0x93fc,
        &[
            (0, 0xe9dbb2137535754bb6afffffffffffff),
            (10, 0x2a09f1b789c8dd1f60046cfab3cf7da1),
        ],
        Expect::Fault(0x93fc),
    ),
    (
        "rr_cvtsd2ss_2230",
        &[0xf2, 0x0f, 0x5a, 0xf8],
        0xf780,
        &[
            (0, 0x63b628c2b1546fc67ff8000000000000),
            (7, 0xcc61544f327cde04937c355ab211ba4c),
        ],
        Expect::Ok(0xcc61544f327cde04937c355a7fc00000, 0xf780),
    ),
    (
        "rr_cvtsd2ss_2315",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd9],
        0x7bb9,
        &[
            (3, 0x595e9c98a8e38144a011d67931eca600),
            (9, 0xee4014ba5be3a9df7fd36d490c4ed03f),
        ],
        Expect::Fault(0x7bb9),
    ),
    (
        "rr_cvtsd2ss_232",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd8],
        0xc521,
        &[
            (8, 0xb78db5aee382d646c00b9e292a19911b),
            (11, 0x6ba4cf2586e73ae6582fce2c4a334a0d),
        ],
        Expect::Fault(0xc521),
    ),
    (
        "rr_cvtsd2ss_2347",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xcf],
        0x0,
        &[
            (1, 0x45f07290310dd29618680d2c8933a3a7),
            (15, 0x872078456b4e9edd36a000000fffffff),
        ],
        Expect::Fault(0x30),
    ),
    (
        "rr_cvtsd2ss_2367",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe3],
        0x5e8a,
        &[
            (4, 0xfbeaf5f7dc71fc91d260edf9b9931beb),
            (11, 0xb25b3a6436f84b27bcb49a1166ea26ca),
        ],
        Expect::Ok(0xfbeaf5f7dc71fc91d260edf9a5a4d08b, 0x5eaa),
    ),
    (
        "rr_cvtsd2ss_238",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc1],
        0xfc0,
        &[
            (1, 0x1af1e274167b6072380fffffffffffff),
            (8, 0x9cf739d4ffa151bd2fd117fc88df4bc4),
        ],
        Expect::Fault(0xfe0),
    ),
    (
        "rr_cvtsd2ss_2424",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc9],
        0xc689,
        &[
            (1, 0xea02c52ec7996b653ff0000000000000),
            (9, 0x7f5f0fdd2bdce18e355bef380c481538),
        ],
        Expect::Ok(0x7f5f0fdd2bdce18e355bef383f800000, 0xc689),
    ),
    (
        "rr_cvtsd2ss_2428",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc3],
        0x346b,
        &[
            (0, 0x11b98741fdbfc923399cdfe2b3dd71a2),
            (11, 0xf871445755f9d207fff8000000000000),
        ],
        Expect::Ok(0x11b98741fdbfc923399cdfe2ffc00000, 0x346b),
    ),
    (
        "rr_cvtsd2ss_2489",
        &[0xf2, 0x0f, 0x5a, 0xea],
        0x45d,
        &[
            (2, 0xc23608c09fda21fb0008ed152d5989cf),
            (5, 0x307123170b80320bfa80f4e03148aa92),
        ],
        Expect::Ok(0x307123170b80320bfa80f4e000000000, 0x45d),
    ),
    (
        "rr_cvtsd2ss_2492",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd5],
        0xca03,
        &[
            (10, 0x4c9a2d18746034f69a3b6b780955ace7),
            (13, 0x04e04a32f4879f3cc343b562f41cb9b3),
        ],
        Expect::Fault(0xca23),
    ),
    (
        "rr_cvtsd2ss_2506",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xcb],
        0xd0a7,
        &[
            (3, 0x7d032a5aa9ee0d81c34491fea8c73412),
            (9, 0x4327705afa64ba9f6730472a690456a1),
        ],
        Expect::Ok(0x4327705afa64ba9f6730472ada248ff5, 0xd0a7),
    ),
    (
        "rr_cvtsd2ss_2571",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xda],
        0x7ec0,
        &[
            (3, 0x45a866eaf30155f3fe3a3e0de9965741),
            (10, 0xaca956bfee2af08c3828000000000000),
        ],
        Expect::Ok(0x45a866eaf30155f3fe3a3e0d01400000, 0x7ec0),
    ),
    (
        "rr_cvtsd2ss_2629",
        &[0xf2, 0x0f, 0x5a, 0xe2],
        0xb79b,
        &[
            (2, 0x26c10069c05997b27ffca45f9be6589b),
            (4, 0x51107d27891766051de1069e6c869a6f),
        ],
        Expect::Ok(0x51107d27891766051de1069e7fe522fc, 0xb79b),
    ),
    (
        "rr_cvtsd2ss_2635",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xff],
        0xdb36,
        &[
            (7, 0xbbc96bff5eed2e3036afffffffffffff),
            (15, 0xbb01397abeed5ca24772300f1c105725),
        ],
        Expect::Ok(0xbb01397abeed5ca24772300f00000000, 0xdb36),
    ),
    (
        "rr_cvtsd2ss_265",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf1],
        0x20bf,
        &[
            (9, 0x72fac8b191120dd7801df7bfa94a57d5),
            (14, 0x92b57009de120e11690030fc0680e6c0),
        ],
        Expect::Fault(0x20bf),
    ),
    (
        "rr_cvtsd2ss_2652",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc1],
        0xda6d,
        &[
            (8, 0x9297c4aafd697433d31356b3c1106dea),
            (9, 0x46eb195faace56c2800797483c7f4b04),
        ],
        Expect::Ok(0x9297c4aafd697433d31356b380000000, 0xda6d),
    ),
    (
        "rr_cvtsd2ss_2678",
        &[0xf2, 0x0f, 0x5a, 0xe5],
        0x4a20,
        &[
            (4, 0x5069bda790977f508d1f0d958b6e2c11),
            (5, 0x460ddcead94b0aab3812e6cda1d09b1c),
        ],
        Expect::Fault(0x4a20),
    ),
    (
        "rr_cvtsd2ss_2804",
        &[0xf2, 0x0f, 0x5a, 0xd8],
        0xad6a,
        &[
            (0, 0x1bf7be1a6220dd34ffde6304e7b7189a),
            (3, 0xffbb3e9a2413e1fd2f93d375fbe752ec),
        ],
        Expect::Fault(0xad6a),
    ),
    (
        "rr_cvtsd2ss_2864",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf6],
        0xdea0,
        &[
            (6, 0xcf8b589e819a68d10000000000000001),
            (14, 0xe4c6a8b4503ce7df7fccc768b9b721da),
        ],
        Expect::Fault(0xdea2),
    ),
    (
        "rr_cvtsd2ss_3003",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xfb],
        0xdf49,
        &[
            (3, 0xa584ee53542f853136ab8171a3267759),
            (15, 0x1c652aaf11defe76daea32971c89e91b),
        ],
        Expect::Ok(0x1c652aaf11defe76daea329700000000, 0xdf79),
    ),
    (
        "rr_cvtsd2ss_3048",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xdd],
        0xe624,
        &[
            (5, 0x01d34e1c1218b2dd7fd3720cdfcf8bf6),
            (11, 0xa7aeb2512efbcfc4308baf24db1b0f24),
        ],
        Expect::Fault(0xe62c),
    ),
    (
        "rr_cvtsd2ss_3077",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe0],
        0xb9e3,
        &[
            (0, 0xf9e59450ee4a087b368e0c037fffffff),
            (12, 0x5a6aa0dd17191609e40d3c31d7c035db),
        ],
        Expect::Ok(0x5a6aa0dd17191609e40d3c3100000000, 0xb9f3),
    ),
    (
        "rr_cvtsd2ss_3150",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd4],
        0x1f40,
        &[
            (4, 0xfbcec5af7c7d9be18009fa9ed1689eb9),
            (10, 0xf067ee5c18f6da81f4c1a16ef762d538),
        ],
        Expect::Ok(0xf067ee5c18f6da81f4c1a16e80000000, 0x1f40),
    ),
    (
        "rr_cvtsd2ss_3163",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xec],
        0xba10,
        &[
            (5, 0x962f3ad1fc8260f7df8b18b0fb219eb1),
            (12, 0x7bb2702fe75108463ff0000010000000),
        ],
        Expect::Ok(0x962f3ad1fc8260f7df8b18b03f800000, 0xba30),
    ),
    (
        "rr_cvtsd2ss_3233",
        &[0xf2, 0x0f, 0x5a, 0xd9],
        0x9b80,
        &[
            (1, 0x76c3bf51568640293800000018000000),
            (3, 0x9a2135f5a9f4834aaacc00d9fd3d8333),
        ],
        Expect::Ok(0x9a2135f5a9f4834aaacc00d900000000, 0x9bb0),
    ),
    (
        "rr_cvtsd2ss_332",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe6],
        0xa13b,
        &[
            (6, 0x076827c7e81928754342082ab037f198),
            (12, 0x32fbf6246afe48b45c74750ada5bab4a),
        ],
        Expect::Fault(0xa13b),
    ),
    (
        "rr_cvtsd2ss_3357",
        &[0xf2, 0x0f, 0x5a, 0xf3],
        0x7ec0,
        &[
            (3, 0x6303e1f52564db3cbca0670db211ee9a),
            (6, 0xf5a58b74b01c969a671b465289b42592),
        ],
        Expect::Ok(0xf5a58b74b01c969a671b4652a503386d, 0x7ee0),
    ),
    (
        "rr_cvtsd2ss_3394",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf4],
        0x6138,
        &[
            (4, 0xfd06fb177b51a1b7c34917d5e6819f0e),
            (14, 0xda5dbf337fb47447fb519b214bd9db86),
        ],
        Expect::Fault(0x6138),
    ),
    (
        "rr_cvtsd2ss_3397",
        &[0xf2, 0x0f, 0x5a, 0xd5],
        0x5f3,
        &[
            (2, 0x96338c1c50240cbdf02d6e247ede9f75),
            (5, 0xa3d0094645313cf7c7e0000000000001),
        ],
        Expect::Fault(0x5f3),
    ),
    (
        "rr_cvtsd2ss_3424",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc5],
        0xa8d4,
        &[
            (8, 0x26c17951f3d81d9abd84fa3266cf3c52),
            (13, 0xc9f2862f0b259e4efff4b2c85fffffff),
        ],
        Expect::Ok(0x26c17951f3d81d9abd84fa32ffe59642, 0xa8d5),
    ),
    (
        "rr_cvtsd2ss_3427",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc3],
        0x8000,
        &[
            (8, 0x14250042d7d319fe810b7c49be6aa35e),
            (11, 0x14bc866a35eab220b68fffffffffffff),
        ],
        Expect::Fault(0x8030),
    ),
    (
        "rr_cvtsd2ss_3485",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xfd],
        0xac15,
        &[
            (13, 0xefcc6b1f5867f037c330da2a2510be38),
            (15, 0xcdacee0e5b83eb99bb762df435b37096),
        ],
        Expect::Fault(0xac35),
    ),
    (
        "rr_cvtsd2ss_3551",
        &[0xf2, 0x0f, 0x5a, 0xe9],
        0xbed8,
        &[
            (1, 0x0d201912f3dabb94b7f0000000000001),
            (5, 0xa8afcda1a1bc20db1a8ffe3bbdd1e2f3),
        ],
        Expect::Ok(0xa8afcda1a1bc20db1a8ffe3b80000000, 0xbef8),
    ),
    (
        "rr_cvtsd2ss_3572",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd7],
        0xb70f,
        &[
            (7, 0x1187564fe802bd2047f2e13b2597fa77),
            (10, 0x420f03652af9f2cafae461f91d141211),
        ],
        Expect::Ok(0x420f03652af9f2cafae461f97f7fffff, 0xb72f),
    ),
    (
        "rr_cvtsd2ss_3634",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf1],
        0xdd1b,
        &[
            (1, 0x5a666ebc55abbe1643279e236806e390),
            (14, 0x157a29f47150d43f3881e6cfec150963),
        ],
        Expect::Ok(0x157a29f47150d43f3881e6cf593cf11c, 0xdd3b),
    ),
    (
        "rr_cvtsd2ss_3645",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe1],
        0x5b80,
        &[
            (9, 0x40c48bd4b410d2717ff0000000000000),
            (12, 0x1164d222e41101b3e60783464786170e),
        ],
        Expect::Ok(0x1164d222e41101b3e60783467f800000, 0x5b80),
    ),
    (
        "rr_cvtsd2ss_366",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd5],
        0x3780,
        &[
            (2, 0xaaea2318e3c92fbe444d4242a8f287b5),
            (13, 0x94a78297d8d01fc3b800000010000001),
        ],
        Expect::Fault(0x37b0),
    ),
    (
        "rr_cvtsd2ss_3728",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe0],
        0x65f5,
        &[
            (0, 0xf43c8a1be237cd34b69fffffffffffff),
            (12, 0x925f05810e07f06e4d0a978067a702a7),
        ],
        Expect::Fault(0x65f5),
    ),
    (
        "rr_cvtsd2ss_3729",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc3],
        0x378,
        &[
            (0, 0x7283630fb94eba023fc8ee4c7bd42baf),
            (11, 0x82a416f85e3201f736a0000000000001),
        ],
        Expect::Fault(0x378),
    ),
    (
        "rr_cvtsd2ss_3730",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xdf],
        0xd35a,
        &[
            (7, 0x1cb99f6da5a782dc368fffffffffffff),
            (11, 0x305a8da31e97c01b469a524b73ddc6f6),
        ],
        Expect::Fault(0xd37a),
    ),
    (
        "rr_cvtsd2ss_3771",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xfd],
        0x39d4,
        &[
            (7, 0x54968a198d40516a22ba3f10f22667e9),
            (13, 0x09ce6a45e535902b400056ea851b19ea),
        ],
        Expect::Ok(0x54968a198d40516a22ba3f104002b754, 0x39f4),
    ),
    (
        "rr_cvtsd2ss_3814",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc4],
        0x2000,
        &[
            (8, 0xd3e4db20a2d4765af6d45555b71e9351),
            (12, 0xc66c4467b7dead8f480fffffffffffff),
        ],
        Expect::Fault(0x2028),
    ),
    (
        "rr_cvtsd2ss_3917",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd4],
        0xfe80,
        &[
            (2, 0x1c4b5af776232ffda4c8a193674239ff),
            (12, 0xc58c37ea6466b3a436a000000fffffff),
        ],
        Expect::Ok(0x1c4b5af776232ffda4c8a19300000000, 0xfeb0),
    ),
    (
        "rr_cvtsd2ss_3923",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe8],
        0xb834,
        &[
            (8, 0xea6047e245da07923680000018000000),
            (13, 0xd620c0f99380eb31f199f9041fa13952),
        ],
        Expect::Ok(0xd620c0f99380eb31f199f90400000000, 0xb834),
    ),
    (
        "rr_cvtsd2ss_3932",
        &[0xf2, 0x0f, 0x5a, 0xef],
        0x5f00,
        &[
            (5, 0x75a7977c07bdee7da8e73be5acaa0a98),
            (7, 0xc05cfa93dc05329d47efffffffffffff),
        ],
        Expect::Ok(0x75a7977c07bdee7da8e73be57f800000, 0x5f28),
    ),
    (
        "rr_cvtsd2ss_4014",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc8],
        0x2a4d,
        &[
            (1, 0x4002f97cb55d252b9c07400efc931651),
            (8, 0x20c694302af2f9bcb68e294e9079a5a9),
        ],
        Expect::Fault(0x2a7d),
    ),
    (
        "rr_cvtsd2ss_4039",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd5],
        0xbfac,
        &[
            (10, 0xfcde80ee5b836d7d668acefe06e58d93),
            (13, 0x85ae1f40e8a23ad07ff2f19c12a36fd1),
        ],
        Expect::Ok(0xfcde80ee5b836d7d668acefe7fd78ce0, 0xbfad),
    ),
    (
        "rr_cvtsd2ss_4070",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd7],
        0xfc0,
        &[
            (10, 0xcfe997b2f09e807cdf1d710c5c0e4628),
            (15, 0xaf3e0d236965b32cb82086a990000000),
        ],
        Expect::Fault(0xfe0),
    ),
    (
        "rr_cvtsd2ss_4102",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xeb],
        0xa3c4,
        &[
            (3, 0xe3c93be5543efcd5ffe5b5a64562cc4e),
            (13, 0x2c9e50b2069b89f21f508d458731ac43),
        ],
        Expect::Fault(0xa3ec),
    ),
    (
        "rr_cvtsd2ss_4135",
        &[0xf2, 0x0f, 0x5a, 0xd4],
        0x40f6,
        &[
            (2, 0xbe93c56bac58c68cc251d7980bf3afaf),
            (4, 0xb97f1bb725b252a237f8000000000000),
        ],
        Expect::Fault(0x40f6),
    ),
    (
        "rr_cvtsd2ss_4172",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xd5],
        0xfe80,
        &[
            (10, 0x1c89e5af38aa176b1d263082d802d27b),
            (13, 0x84b9add10c86802efff000000fffffff),
        ],
        Expect::Ok(0x1c89e5af38aa176b1d263082ffc00000, 0xfe81),
    ),
    (
        "rr_cvtsd2ss_4300",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc8],
        0xdb0f,
        &[
            (0, 0x0f71837eea62ca5436a0000000000001),
            (9, 0x94be5661d87ad1ddf08c49386ba0b2f9),
        ],
        Expect::Ok(0x94be5661d87ad1ddf08c493800000000, 0xdb3f),
    ),
    (
        "rr_cvtsd2ss_4306",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xec],
        0x6773,
        &[
            (5, 0x61a7c96ca73583ee4d97083a0e34647a),
            (12, 0x0ec18358355507da00280505a0a39012),
        ],
        Expect::Fault(0x6773),
    ),
    (
        "rr_cvtsd2ss_434",
        &[0xf2, 0x0f, 0x5a, 0xe0],
        0x1780,
        &[
            (0, 0xf6f554ca6275d305c80fffffffffffff),
            (4, 0xbad6202d060b20061f7ca34cc3babc90),
        ],
        Expect::Ok(0xbad6202d060b20061f7ca34cff800000, 0x17a8),
    ),
    (
        "rr_cvtsd2ss_4366",
        &[0xf2, 0x0f, 0x5a, 0xe1],
        0xf16f,
        &[
            (1, 0x495f76cf32ac5e3a7fe94b9b46001b9c),
            (4, 0xb251652ea7d5cfb3e9fb682c743017d9),
        ],
        Expect::Fault(0xf16f),
    ),
    (
        "rr_cvtsd2ss_4375",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc6],
        0x8a29,
        &[
            (0, 0x4707e9e53ba751825227a14863e684dd),
            (14, 0x1fa1530046b03b5e47efffffffffffff),
        ],
        Expect::Fault(0x8a29),
    ),
    (
        "rr_cvtsd2ss_4408",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe1],
        0x90a9,
        &[
            (1, 0xd579f1499e70540ac8047a195fffffff),
            (12, 0xee0dbf338b8fdeb6e33a3c6590469a34),
        ],
        Expect::Fault(0x90a9),
    ),
    (
        "rr_cvtsd2ss_4436",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf5],
        0x85b2,
        &[
            (5, 0x8844f235b4b88ded380fffffffffffff),
            (14, 0xf9e61c189f377c8e7762816d52000e5c),
        ],
        Expect::Fault(0x85b2),
    ),
    (
        "rr_cvtsd2ss_4509",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc2],
        0xfcdc,
        &[
            (8, 0x00578a952d66b9e01ac0e7aa65150826),
            (10, 0xec83bbfc9991bdfc8a10d535ec381523),
        ],
        Expect::Ok(0x00578a952d66b9e01ac0e7aa80000000, 0xfcfc),
    ),
    (
        "rr_cvtsd2ss_4597",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf3],
        0xf35c,
        &[
            (11, 0xa3e29972056a079b4808000000000000),
            (14, 0xe0b6af9286b1362551dee28e1116b5d5),
        ],
        Expect::Fault(0xf35c),
    ),
    (
        "rr_cvtsd2ss_4688",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xff],
        0x225,
        &[
            (7, 0xbb9d13d54d9d2944d469505f76463647),
            (15, 0xdde4aa97f27a637f7ff2488f81c456b0),
        ],
        Expect::Fault(0x225),
    ),
    (
        "rr_cvtsd2ss_471",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe6],
        0x80c0,
        &[
            (6, 0xdc0c14b7f1daad2100132d8feb0f5057),
            (12, 0x7faadc4ec145408ec797df3af9b46a75),
        ],
        Expect::Fault(0x80f0),
    ),
    (
        "rr_cvtsd2ss_4737",
        &[0xf2, 0x0f, 0x5a, 0xc2],
        0x3a6b,
        &[
            (0, 0x109e7f3e8afe0c61abdc165f31ffcfee),
            (2, 0x4ee3375f0c889d4d8002a8410d78a498),
        ],
        Expect::Ok(0x109e7f3e8afe0c61abdc165f80000000, 0x3a6b),
    ),
    (
        "rr_cvtsd2ss_4756",
        &[0xf2, 0x0f, 0x5a, 0xfa],
        0x1780,
        &[
            (2, 0x28b9e68e4a8ab376c00fffffffffffff),
            (7, 0x6f68f8e321b9f25216ca657bbaa28742),
        ],
        Expect::Ok(0x6f68f8e321b9f25216ca657bc0800000, 0x17a0),
    ),
    (
        "rr_cvtsd2ss_4764",
        &[0xf2, 0x0f, 0x5a, 0xc3],
        0xae89,
        &[
            (0, 0x33f98ebfb99070c11dd02287b329ea1d),
            (3, 0x7fcd4bf9e7e6bd1bbff2a2c515dbe728),
        ],
        Expect::Fault(0xaea9),
    ),
    (
        "rr_cvtsd2ss_4854",
        &[0xf2, 0x0f, 0x5a, 0xce],
        0xcbf5,
        &[
            (1, 0x1aac39606baed7a3f069d420e663481b),
            (6, 0xe766872c29100a05b800000018000000),
        ],
        Expect::Fault(0xcbf5),
    ),
    (
        "rr_cvtsd2ss_4881",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd8],
        0x5b80,
        &[
            (0, 0x5eaf7711ab5fb7e847f0000000000001),
            (11, 0x3f519bd31fa58d12e528b9c3922abd57),
        ],
        Expect::Fault(0x5ba8),
    ),
    (
        "rr_cvtsd2ss_4935",
        &[0xf2, 0x0f, 0x5a, 0xf4],
        0xda61,
        &[
            (4, 0xcca5d607d2b9498a79d599f73e996eec),
            (6, 0xd731335d8e7405aa558c01126c740e45),
        ],
        Expect::Fault(0xda69),
    ),
    (
        "rr_cvtsd2ss_4950",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe1],
        0x9d80,
        &[
            (4, 0x50738cd33916ae07449a72e5b0bc2051),
            (9, 0xbd05c8ad1775170bbff25abf7fffffff),
        ],
        Expect::Ok(0x50738cd33916ae07449a72e5bf92d5fc, 0x9da0),
    ),
    (
        "rr_cvtsd2ss_5057",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xd5],
        0x6000,
        &[
            (2, 0xed40f297dfd25681410b63ef0f228f41),
            (13, 0xc9b60e2791ecbb88e1ac4f5027df3c26),
        ],
        Expect::Fault(0x6028),
    ),
    (
        "rr_cvtsd2ss_5077",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc6],
        0xe4b6,
        &[
            (0, 0x0c036e53809121376a92a5ad73e063cd),
            (14, 0x2647607f6ceb10d68000000018000000),
        ],
        Expect::Fault(0xe4b6),
    ),
    (
        "rr_cvtsd2ss_5112",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf5],
        0x77c0,
        &[
            (13, 0x18460e07d17247a536a0000010000001),
            (14, 0x71940ac3382aed0207d944d6f7a5e202),
        ],
        Expect::Fault(0x77f0),
    ),
    (
        "rr_cvtsd2ss_5144",
        &[0xf2, 0x0f, 0x5a, 0xdd],
        0xd9f7,
        &[
            (3, 0x6aef4bd9a831b46e806f95d24b75dc1e),
            (5, 0xb2ecdec16efa36a07ff0000010000000),
        ],
        Expect::Ok(0x6aef4bd9a831b46e806f95d27fc00000, 0xd9f7),
    ),
    (
        "rr_cvtsd2ss_5212",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd2],
        0x3f00,
        &[
            (2, 0x2db1834ceefee087bca2ce0ba853214d),
            (10, 0x670b2f002e8c63b18e122df618ff9b14),
        ],
        Expect::Ok(0x670b2f002e8c63b18e122df6a516705e, 0x3f20),
    ),
    (
        "rr_cvtsd2ss_5214",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe5],
        0xa78d,
        &[
            (5, 0x765f34ad3b70d2860000000018000000),
            (12, 0x61ca9fb9b47f04f37312203e3bff1cff),
        ],
        Expect::Fault(0xa79f),
    ),
    (
        "rr_cvtsd2ss_5216",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc9],
        0xde04,
        &[
            (1, 0xcb35a38ea05ab09b380fffffffffffff),
            (9, 0x020f6118d2ae2a285ddd37656f94e194),
        ],
        Expect::Ok(0x020f6118d2ae2a285ddd376500800000, 0xde24),
    ),
    (
        "rr_cvtsd2ss_526",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc5],
        0x351,
        &[
            (5, 0xc0d9ecc3fc157ddf7ff000000fffffff),
            (8, 0x95e0e92e4026b7f62040994b3d7d9345),
        ],
        Expect::Fault(0x351),
    ),
    (
        "rr_cvtsd2ss_5285",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xc3],
        0x7d80,
        &[
            (8, 0x46317a3176608d46e01956f61bfec9e2),
            (11, 0x99df8e371c26aeca7fdbecf245e94a66),
        ],
        Expect::Ok(0x46317a3176608d46e01956f67f7fffff, 0x7da8),
    ),
    (
        "rr_cvtsd2ss_5341",
        &[0xf2, 0x0f, 0x5a, 0xe8],
        0xc820,
        &[
            (0, 0xf7eb24e1164c3defc32e31bc3cb34cb4),
            (5, 0x42911ae86db923d052f978c011f42823),
        ],
        Expect::Fault(0xc820),
    ),
    (
        "rr_cvtsd2ss_5385",
        &[0xf2, 0x0f, 0x5a, 0xee],
        0xc17a,
        &[
            (5, 0x56f1e74d16c013310d44fcc3de6d5ddd),
            (6, 0x27c3adddffbbba9748037437db191499),
        ],
        Expect::Fault(0xc17a),
    ),
    (
        "rr_cvtsd2ss_5397",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe0],
        0xd2b3,
        &[
            (8, 0x3ed9459f387d97e17ff0000018000000),
            (12, 0x639af45b6327ee60463a3c544a1babf2),
        ],
        Expect::Ok(0x639af45b6327ee60463a3c547fc00000, 0xd2b3),
    ),
    (
        "rr_cvtsd2ss_5425",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe3],
        0xfbf,
        &[
            (11, 0xe058f17045a9b4bd002b89f7991e1ea3),
            (12, 0x6aa6830bf604cac6beaceb3447ccb82b),
        ],
        Expect::Fault(0xfbf),
    ),
    (
        "rr_cvtsd2ss_5441",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xd9],
        0x1dc0,
        &[
            (1, 0x5b9f46f89d680e5ab7f0000010000001),
            (11, 0xd6ac1e79ea823f5f3a06b38ef4d1630e),
        ],
        Expect::Ok(0xd6ac1e79ea823f5f3a06b38e80200000, 0x1df0),
    ),
    (
        "rr_cvtsd2ss_5558",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xed],
        0xd6b8,
        &[
            (5, 0x3b3105fbd3daeeba800fd4c0f13cb178),
            (13, 0xe7f5092be894379a2ae822568d52eaab),
        ],
        Expect::Fault(0xd6ba),
    ),
    (
        "rr_cvtsd2ss_5560",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xc7],
        0xf1b6,
        &[
            (0, 0x05c8a488be520b83f90c2e596b1d4d38),
            (15, 0x0126d0391029f119b7ffffffffffffff),
        ],
        Expect::Fault(0xf1b6),
    ),
    (
        "rr_cvtsd2ss_5562",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xcb],
        0xd5c2,
        &[
            (3, 0xbc6cdcd52552333d434f3e1d87958ef3),
            (9, 0x29914bfdaa8714286c8aab16c97498f5),
        ],
        Expect::Ok(0x29914bfdaa8714286c8aab165a79f0ed, 0xd5e2),
    ),
    (
        "rr_cvtsd2ss_5688",
        &[0xf2, 0x0f, 0x5a, 0xed],
        0x9e80,
        &[(5, 0x7731f81ede2f1bdb4335e87a1facf912)],
        Expect::Ok(0x7731f81ede2f1bdb4335e87a59af43d1, 0x9ea0),
    ),
    (
        "rr_cvtsd2ss_5811",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc5],
        0x7b80,
        &[
            (5, 0x2e4a1eb297d383a1c34d594e71dc1c3a),
            (8, 0xc0674ddf2ca1b8311533eb81eeef9717),
        ],
        Expect::Ok(0xc0674ddf2ca1b8311533eb81da6aca73, 0x7ba0),
    ),
    (
        "rr_cvtsd2ss_5835",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xdb],
        0xfb80,
        &[(11, 0x84df9311cfa8691937f0000018000000)],
        Expect::Ok(0x84df9311cfa8691937f0000000000000, 0xfbb0),
    ),
    (
        "rr_cvtsd2ss_5878",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xdf],
        0x84af,
        &[
            (11, 0x8dd3cef70f37d3fd445c5379a8f9035e),
            (15, 0xbfedab824d5115ab3828000000000000),
        ],
        Expect::Ok(0x8dd3cef70f37d3fd445c537901400000, 0x84af),
    ),
    (
        "rr_cvtsd2ss_5912",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xcb],
        0x2053,
        &[
            (9, 0x890d75423bce055747b90269a8b0c500),
            (11, 0xb866812e90da842a37ffffffffffffff),
        ],
        Expect::Fault(0x2073),
    ),
    (
        "rr_cvtsd2ss_5927",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xe4],
        0x3f80,
        &[
            (4, 0xe3792092d757b60cfffb28dcb01de88e),
            (12, 0xac279f24baef8d8ab7f0000010000001),
        ],
        Expect::Ok(0xe3792092d757b60cfffb28dc80200001, 0x3fb0),
    ),
    (
        "rr_cvtsd2ss_6195",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xda],
        0xcb5c,
        &[
            (10, 0x86ff28b13c3cb98affffffffffffffff),
            (11, 0x8215a4e993791af2c1ac3890498ded30),
        ],
        Expect::Ok(0x8215a4e993791af2c1ac3890ffffffff, 0xcb5c),
    ),
    (
        "rr_cvtsd2ss_6216",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf5],
        0x445d,
        &[
            (5, 0x6b78dc263272fa218026604b6c15f71a),
            (14, 0x7f824ca90f16de6df63e29f3dfdfb9c4),
        ],
        Expect::Fault(0x447d),
    ),
    (
        "rr_cvtsd2ss_6317",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xef],
        0x6eaf,
        &[
            (5, 0xe9ed6fc1997f7b8aedab81c76d9c0160),
            (15, 0xb27650be7dbdf6a3c0052dee26d2df6a),
        ],
        Expect::Fault(0x6eaf),
    ),
    (
        "rr_cvtsd2ss_6416",
        &[0xf2, 0x0f, 0x5a, 0xc0],
        0x48e8,
        &[(0, 0x4235721a629f6b65b82fffffffffffff)],
        Expect::Fault(0x48e8),
    ),
    (
        "rr_cvtsd2ss_6438",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xe9],
        0xe7bb,
        &[
            (9, 0x0e17b28bb20ce2824327036376596feb),
            (13, 0x352f799261774e0eb885c45e28adaa89),
        ],
        Expect::Fault(0xe7bb),
    ),
    (
        "rr_cvtsd2ss_6467",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xf7],
        0xd451,
        &[
            (14, 0xedb80359f117dcfebdce9efcbf830a23),
            (15, 0x5be8c8c820be9ee0c7ffffffffffffff),
        ],
        Expect::Ok(0xedb80359f117dcfebdce9efcff7fffff, 0xd479),
    ),
    (
        "rr_cvtsd2ss_647",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xfe],
        0xdec5,
        &[
            (14, 0xe2a6ba8dac54daa7368fffffffffffff),
            (15, 0x3100fd66783b1f7c634f27fa6d8f68cd),
        ],
        Expect::Ok(0x3100fd66783b1f7c634f27fa00000000, 0xdef5),
    ),
    (
        "rr_cvtsd2ss_6569",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xe7],
        0x3f80,
        &[
            (7, 0x72fbd09628923f083690000000000001),
            (12, 0xfc734ea1f23a2ca86ea8f8b2645050f4),
        ],
        Expect::Ok(0xfc734ea1f23a2ca86ea8f8b200000000, 0x3fb0),
    ),
    (
        "rr_cvtsd2ss_6583",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xca],
        0x17c0,
        &[
            (9, 0x893b1193bba79939b187c926b3e2a023),
            (10, 0x165342bacb28b316c7e000001fffffff),
        ],
        Expect::Ok(0x893b1193bba79939b187c926ff000001, 0x17e0),
    ),
    (
        "rr_cvtsd2ss_670",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xed],
        0x8721,
        &[
            (5, 0x0c429e933d4fccd53ca3bd203efb4701),
            (13, 0x1eb78d9bb1fcc135583624231ae84fa9),
        ],
        Expect::Fault(0x8721),
    ),
    (
        "rr_cvtsd2ss_6704",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xf2],
        0xe206,
        &[
            (2, 0x874dde0b84ac3225000fffffffffffff),
            (14, 0xd3ed1be5a3da9d6fff619dd2291a390c),
        ],
        Expect::Fault(0xe206),
    ),
    (
        "rr_cvtsd2ss_6854",
        &[0xf2, 0x45, 0x0f, 0x5a, 0xef],
        0x24bc,
        &[
            (13, 0x29bb483f3030a27944a53e9061226984),
            (15, 0xa5929f838616e779b680000010000001),
        ],
        Expect::Fault(0x24bc),
    ),
    (
        "rr_cvtsd2ss_6916",
        &[0xf2, 0x0f, 0x5a, 0xe9],
        0x5b80,
        &[
            (1, 0x020e180eb4e00ab58000000000000000),
            (5, 0x36d8e53472dc0c8996b592334c92370c),
        ],
        Expect::Ok(0x36d8e53472dc0c8996b5923380000000, 0x5b80),
    ),
    (
        "rr_cvtsd2ss_796",
        &[0xf2, 0x0f, 0x5a, 0xfa],
        0xb883,
        &[
            (2, 0x032fd2fba43a894b4008000000000000),
            (7, 0x82f2eceba41523379b3d972d07bffe2f),
        ],
        Expect::Ok(0x82f2eceba41523379b3d972d40400000, 0xb883),
    ),
    (
        "rr_cvtsd2ss_830",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xef],
        0x5f00,
        &[
            (5, 0x19949441ac0fb5a5e625739657728896),
            (15, 0xba2a8cbf2a71eb813ffbffbe8892418e),
        ],
        Expect::Ok(0x19949441ac0fb5a5e62573963fdffdf5, 0x5f20),
    ),
    (
        "rr_cvtsd2ss_856",
        &[0xf2, 0x41, 0x0f, 0x5a, 0xdb],
        0x2ed4,
        &[
            (3, 0x2b55a117ce9c6baf395fb222443fc254),
            (11, 0x790c8345e2bca716000000001fffffff),
        ],
        Expect::Ok(0x2b55a117ce9c6baf395fb22200000000, 0x2ed4),
    ),
    (
        "rr_cvtsd2ss_87",
        &[0xf2, 0x44, 0x0f, 0x5a, 0xc0],
        0xbcbf,
        &[
            (0, 0xceed7828727c6c6936afffffffffffff),
            (8, 0xbbca1c8a4a3545fcf44e23f8b1d92a73),
        ],
        Expect::Ok(0xbbca1c8a4a3545fcf44e23f800000000, 0xbcbf),
    ),
    (
        "rr_cvtss2sd_108",
        &[0xf3, 0x0f, 0x5a, 0xfc],
        0xfd80,
        &[
            (4, 0x2b144d850e89c0d8041a6e547f8bbe0e),
            (7, 0x8e1b3301e529d0f7ab387137b74b3313),
        ],
        Expect::Ok(0x8e1b3301e529d0f77ff977c1c0000000, 0xfd81),
    ),
    (
        "rr_cvtss2sd_1125",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd1],
        0x1a00,
        &[
            (1, 0x510a7966262bfcce64a8b9dd80048d00),
            (10, 0x53036548cfd7c404fe36710344e0e72d),
        ],
        Expect::Fault(0x1a02),
    ),
    (
        "rr_cvtss2sd_131",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd2],
        0xef80,
        &[(10, 0xe20022ddf659118144d7a16ccb448d43)],
        Expect::Ok(0xe20022ddf6591181c16891a860000000, 0xef80),
    ),
    (
        "rr_cvtss2sd_1359",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc1],
        0xbc3a,
        &[
            (8, 0x9819fc62a94c720dc50fbb787478927f),
            (9, 0x1b734e1897cbaa718231dd2c0072428c),
        ],
        Expect::Fault(0xbc3a),
    ),
    (
        "rr_cvtss2sd_1407",
        &[0xf3, 0x0f, 0x5a, 0xc1],
        0x98b6,
        &[
            (0, 0xa5ae5fae16a2e489aeeeb7217761a216),
            (1, 0xd07a05bd8da024cd76921e0700512eff),
        ],
        Expect::Fault(0x98b6),
    ),
    (
        "rr_cvtss2sd_1462",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xf6],
        0xea97,
        &[
            (6, 0xf2bb0a39695980c93d41cbbe47704251),
            (14, 0x7051d6c6e7ac520ffa36ab3c00592940),
        ],
        Expect::Fault(0xea97),
    ),
    (
        "rr_cvtss2sd_1541",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc7],
        0x6b6,
        &[
            (0, 0x1016abab44dbbaff4431d3d541e659c9),
            (15, 0x1b071e571b3c7813708b966174df8b14),
        ],
        Expect::Ok(0x1016abab44dbbaff469bf16280000000, 0x6b6),
    ),
    (
        "rr_cvtss2sd_1711",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc7],
        0xd9a4,
        &[
            (0, 0x4d02ee3b77a16670e317fb2c6d87ecc6),
            (15, 0x5c3f966c64e555f1c0aab12900112af1),
        ],
        Expect::Ok(0x4d02ee3b77a1667037e12af100000000, 0xd9a6),
    ),
    (
        "rr_cvtss2sd_1842",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xe9],
        0xc193,
        &[
            (1, 0x7d0c92c75b3fc6098f9d92fc004caaa8),
            (13, 0x18ff76530b5cd63c100ec2ec4217a330),
        ],
        Expect::Ok(0x18ff76530b5cd63c38032aaa00000000, 0xc193),
    ),
    (
        "rr_cvtss2sd_1843",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xcc],
        0xc838,
        &[
            (1, 0x89586759fa64daea56526713f2328060),
            (12, 0x62f903418d815a9bc6687e7c7fa098d0),
        ],
        Expect::Fault(0xc839),
    ),
    (
        "rr_cvtss2sd_1860",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd4],
        0x4a1f,
        &[
            (4, 0x3bac4162647f790de55580d37f97bed3),
            (10, 0xd6708bc231e950927570f3f8e70c397e),
        ],
        Expect::Fault(0x4a1f),
    ),
    (
        "rr_cvtss2sd_1986",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe9],
        0x3bdc,
        &[
            (5, 0xdf334da7b9474845c687e7aeb5275e8d),
            (9, 0x423bf85c776847e5e13f6bb18050232f),
        ],
        Expect::Ok(0xdf334da7b94748458000000000000000, 0x3bdc),
    ),
    (
        "rr_cvtss2sd_1993",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd5],
        0xa9a1,
        &[
            (10, 0xf92b6de4061d42f8e614d65c5f39860b),
            (13, 0x129d48217fae6cf5688881c600fc2d7b),
        ],
        Expect::Ok(0xf92b6de4061d42f8381f85af60000000, 0xa9a1),
    ),
    (
        "rr_cvtss2sd_20",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc2],
        0xb71d,
        &[
            (8, 0xd275835d2a87a52d815ac7cb5628d1e1),
            (10, 0x41a84bfd5cce54e276af6e1c803eceb0),
        ],
        Expect::Ok(0xd275835d2a87a52db7ff675800000000, 0xb71f),
    ),
    (
        "rr_cvtss2sd_2154",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xfc],
        0x77c0,
        &[
            (12, 0x4e13499f681644757bd883da7fcafd9c),
            (15, 0x946a12063126ecaec1e6ea04897c80de),
        ],
        Expect::Ok(0x946a12063126ecae7ff95fb380000000, 0x77c0),
    ),
    (
        "rr_cvtss2sd_2159",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf3],
        0x7ec0,
        &[
            (11, 0x7daab6ed8f7c16b15fa72f5c805a19e6),
            (14, 0x177ab4746907b0587a38ed154b5f9635),
        ],
        Expect::Ok(0x177ab4746907b0588000000000000000, 0x7ec0),
    ),
    (
        "rr_cvtss2sd_2206",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xfe],
        0x9a38,
        &[
            (14, 0xa41b01c328f0b24d856e17fbff104842),
            (15, 0x139f12ce38682a82508fa3645a823de7),
        ],
        Expect::Ok(0x139f12ce38682a82c7e2090840000000, 0x9a38),
    ),
    (
        "rr_cvtss2sd_2275",
        &[0xf3, 0x0f, 0x5a, 0xd7],
        0xc428,
        &[
            (2, 0x882e8e97d6589aded9324e3a7829af31),
            (7, 0xf14766ffcc7b9f9c5c9c3b7aff8b2f50),
        ],
        Expect::Fault(0xc429),
    ),
    (
        "rr_cvtss2sd_2626",
        &[0xf3, 0x0f, 0x5a, 0xc6],
        0x7bc0,
        &[
            (0, 0x99f6683fb570a44cd22ff9173f4660a1),
            (6, 0xd6aa202a38b7517f10f917eb803bd4f3),
        ],
        Expect::Ok(0x99f6683fb570a44c8000000000000000, 0x7bc0),
    ),
    (
        "rr_cvtss2sd_2669",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xfa],
        0xf036,
        &[
            (2, 0x6986218861a0a02ec1bf355b806e4607),
            (15, 0xb7e6665c192a72b826f4ee5ee53ba1ae),
        ],
        Expect::Fault(0xf036),
    ),
    (
        "rr_cvtss2sd_2701",
        &[0xf3, 0x0f, 0x5a, 0xdd],
        0x40,
        &[
            (3, 0xa289b388276bd4d016a49318c750c96a),
            (5, 0xb440072f1ef6dc807e78b860fff201ea),
        ],
        Expect::Ok(0xa289b388276bd4d0fffe403d40000000, 0x40),
    ),
    (
        "rr_cvtss2sd_272",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xfe],
        0x1f40,
        &[
            (7, 0x96aab480ebfd031e12a1470b6b15c9ef),
            (14, 0xb99769680ed80c879e62ff99ff880aef),
        ],
        Expect::Fault(0x1f41),
    ),
    (
        "rr_cvtss2sd_2829",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd1],
        0x1a00,
        &[
            (1, 0x5cd8fde2f10e0f7e2126b697806cd54b),
            (10, 0x65ab39569db90ed69289ef91775e2874),
        ],
        Expect::Fault(0x1a02),
    ),
    (
        "rr_cvtss2sd_2850",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xd1],
        0xfe80,
        &[
            (2, 0x971d7a868d5096ba148426d3f6c5db07),
            (9, 0x419c20271b97b4c4bb6f891f00364c3c),
        ],
        Expect::Fault(0xfe82),
    ),
    (
        "rr_cvtss2sd_3102",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd5],
        0x6a0f,
        &[
            (5, 0x6dadbb243b44fbedf4fcae13800d8784),
            (10, 0x9b8f2f897429766bc82ca7556e01bcac),
        ],
        Expect::Fault(0x6a0f),
    ),
    (
        "rr_cvtss2sd_3157",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xf1],
        0x6000,
        &[
            (1, 0x402df7db32fd82d4d486f50ff4f63614),
            (14, 0x1eeda72f7fdfef0eba2d06b262672ff0),
        ],
        Expect::Ok(0x1eeda72f7fdfef0ec69ec6c280000000, 0x6000),
    ),
    (
        "rr_cvtss2sd_3333",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xfd],
        0xad72,
        &[
            (7, 0x9b3bacf70064c755a3869d347ff84133),
            (13, 0x66d3982a75027ebd7b1eb4ab001ea509),
        ],
        Expect::Ok(0x9b3bacf70064c7550000000000000000, 0xad72),
    ),
    (
        "rr_cvtss2sd_3344",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xdf],
        0x9d80,
        &[
            (11, 0xffe3ed69ce67981d1942a32fe6f9c48c),
            (15, 0x38e223141ff7f6795db4b7eccb98182f),
        ],
        Expect::Ok(0xffe3ed69ce67981dc1730305e0000000, 0x9d80),
    ),
    (
        "rr_cvtss2sd_3380",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xeb],
        0x59c2,
        &[
            (11, 0x0e0a095c9eb6db7c503cbe037f808f48),
            (13, 0x59f3a01f0b17a456fe61f4e9e4870bda),
        ],
        Expect::Ok(0x59f3a01f0b17a4567ff811e900000000, 0x59c3),
    ),
    (
        "rr_cvtss2sd_3550",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xd3],
        0x7571,
        &[
            (3, 0x8f9635801cb0256ac96b4d807fa54969),
            (10, 0x75ad28c6948f82d1022770236a51bfbf),
        ],
        Expect::Fault(0x7571),
    ),
    (
        "rr_cvtss2sd_3645",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xef],
        0x66b7,
        &[
            (7, 0x7fa4afa56c81448a9ca1b78f00239cbb),
            (13, 0x5a06381016d45093dd064132670e7d29),
        ],
        Expect::Fault(0x66b7),
    ),
    (
        "rr_cvtss2sd_3673",
        &[0xf3, 0x0f, 0x5a, 0xe7],
        0x1f40,
        &[
            (4, 0xa2de670306dcf3fd284aef763607aab8),
            (7, 0x39b8631e118ef9deb8b75a583f9f3312),
        ],
        Expect::Ok(0xa2de670306dcf3fd3ff3e66240000000, 0x1f40),
    ),
    (
        "rr_cvtss2sd_37",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf6],
        0x4ddd,
        &[(14, 0x0f07c1f1de51d119fff593e17f705eca)],
        Expect::Ok(0x0f07c1f1de51d11947ee0bd940000000, 0x4ddd),
    ),
    (
        "rr_cvtss2sd_3745",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xdd],
        0xfd80,
        &[
            (11, 0x7905851f83670209eb11edb65d1b7b90),
            (13, 0xad64bc7c9add8f6b3a5028e17fd6b46b),
        ],
        Expect::Ok(0x7905851f836702097ffad68d60000000, 0xfd80),
    ),
    (
        "rr_cvtss2sd_3796",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc5],
        0x6f80,
        &[
            (0, 0x14ee7accb0a0124a7e25fb62d7c90685),
            (13, 0x778f1b0cfe6980102f21b7a9805595bf),
        ],
        Expect::Ok(0x14ee7accb0a0124ab805656fc0000000, 0x6f82),
    ),
    (
        "rr_cvtss2sd_3846",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc1],
        0x4964,
        &[
            (0, 0x9980a5f8d3a35e6043d2887cf3014955),
            (9, 0x6ce001cb4359b8537a321e4ccbbc4953),
        ],
        Expect::Ok(0x9980a5f8d3a35e60c177892a60000000, 0x4964),
    ),
    (
        "rr_cvtss2sd_3875",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc6],
        0x9e80,
        &[
            (0, 0xe18e8db8f70dd1dfcdb591801dc8637f),
            (14, 0x037d43ea32681e85d080332880636cd8),
        ],
        Expect::Fault(0x9e82),
    ),
    (
        "rr_cvtss2sd_3927",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd2],
        0x8f80,
        &[(10, 0x848c0d3e6bcdb417ffeff688003646b9)],
        Expect::Ok(0x848c0d3e6bcdb41737fb235c80000000, 0x8f82),
    ),
    (
        "rr_cvtss2sd_3986",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe7],
        0x5780,
        &[
            (4, 0x25c7f4fbb3eb4163a4a79ee7fb23568b),
            (15, 0x459a8e6d21adb1212c8e5c028046235d),
        ],
        Expect::Ok(0x25c7f4fbb3eb4163b80188d740000000, 0x5782),
    ),
    (
        "rr_cvtss2sd_3991",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xcc],
        0xa4f6,
        &[
            (4, 0xfd2582548ed4e0ed5df75f5e400c5cf5),
            (9, 0x226fc04bd94180b819cf32d16a2a7173),
        ],
        Expect::Ok(0x226fc04bd94180b840018b9ea0000000, 0xa4f6),
    ),
    (
        "rr_cvtss2sd_4082",
        &[0xf3, 0x0f, 0x5a, 0xdb],
        0x3f00,
        &[(3, 0x2886fd898f3f0b8219ba52c8803ff87e)],
        Expect::Ok(0x2886fd898f3f0b82b7fffc3f00000000, 0x3f02),
    ),
    (
        "rr_cvtss2sd_4116",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xe3],
        0x4000,
        &[
            (3, 0xf3898f2d52745662619bbd960031f80a),
            (12, 0x442790e7dafe1e17b292d7a509debcd4),
        ],
        Expect::Fault(0x4002),
    ),
    (
        "rr_cvtss2sd_4143",
        &[0xf3, 0x0f, 0x5a, 0xe9],
        0xcb54,
        &[
            (1, 0x6115851b13a1be20440435857fb9998b),
            (5, 0x9bf754df8cc42020560d64be5434c9d1),
        ],
        Expect::Fault(0xcb55),
    ),
    (
        "rr_cvtss2sd_433",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc0],
        0x1c1b,
        &[
            (0, 0x780ccfe0eadd21aa4ccd9396b56b26c4),
            (8, 0x88715300f8646bc2c9c4422e000fe0f1),
        ],
        Expect::Fault(0x1c1b),
    ),
    (
        "rr_cvtss2sd_4331",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc9],
        0x49e,
        &[
            (1, 0xa17f0e729dcbd4c378f29471ccb58596),
            (9, 0xc3dcc7b0894abc6a53a1850d80006016),
        ],
        Expect::Fault(0x49e),
    ),
    (
        "rr_cvtss2sd_4364",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xe7],
        0x6000,
        &[
            (7, 0x4c12b38610c78d6000e9f5e97fb3f523),
            (12, 0xb5717325ee2971dbe7d4425db05f3dc5),
        ],
        Expect::Fault(0x6001),
    ),
    (
        "rr_cvtss2sd_4437",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xdb],
        0x3780,
        &[
            (3, 0x7ac04598f09d7e814af056511e82cd38),
            (11, 0x8a07306feff7490b64b521577fb32ca7),
        ],
        Expect::Ok(0x7ac04598f09d7e817ffe6594e0000000, 0x3781),
    ),
    (
        "rr_cvtss2sd_4642",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xf3],
        0x4000,
        &[
            (11, 0xc73c26268bb5e42f0184bf6800144122),
            (14, 0xaf40e24406be32cd2406d6ffa1c6e814),
        ],
        Expect::Fault(0x4002),
    ),
    (
        "rr_cvtss2sd_4665",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xd2],
        0x420f,
        &[
            (2, 0x9c22d73cb1c38bb525ae2ff50d047782),
            (10, 0xd1864b57a19e85bf5ec74631ffb32580),
        ],
        Expect::Fault(0x420f),
    ),
    (
        "rr_cvtss2sd_4798",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xfa],
        0x80c0,
        &[
            (7, 0xc62cfa03df7c64c14e120e3f78a3bb5c),
            (10, 0x600842cd9e064ab915e0c083c012662c),
        ],
        Expect::Ok(0xc62cfa03df7c64c1c0024cc580000000, 0x80c0),
    ),
    (
        "rr_cvtss2sd_4834",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xfd],
        0x7bc0,
        &[
            (13, 0xaf469122b2b84c887dae07a8ffa11d6a),
            (15, 0x66cdf8bc0674bed4b5feacd795bfd1df),
        ],
        Expect::Ok(0x66cdf8bc0674bed4fffc23ad40000000, 0x7bc1),
    ),
    (
        "rr_cvtss2sd_5041",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xcd],
        0x8f80,
        &[
            (9, 0x5d23c3344c3ca59fd8d0b24899cfef26),
            (13, 0x3ee6da08f900610f811dcb07804e9a8a),
        ],
        Expect::Ok(0x5d23c3344c3ca59fb803a6a280000000, 0x8f82),
    ),
    (
        "rr_cvtss2sd_5074",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xdc],
        0x80a2,
        &[
            (3, 0xe740b16f18f1b95ae7eee049a95d10e8),
            (12, 0x2781209fa8cf61461f08e8be004d530a),
        ],
        Expect::Fault(0x80a2),
    ),
    (
        "rr_cvtss2sd_5162",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe8],
        0x7959,
        &[
            (5, 0xf358c30ba3f57e32750f6225effef8c6),
            (8, 0x52e7fe94426eb230c96af765ff9c79c2),
        ],
        Expect::Fault(0x7959),
    ),
    (
        "rr_cvtss2sd_5171",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xdc],
        0x53cf,
        &[
            (4, 0x87f6ec9334b7fe4218ce97a3ffa45b04),
            (11, 0xda8eee993a8cd4543543074715d7d0fb),
        ],
        Expect::Ok(0xda8eee993a8cd454fffc8b6080000000, 0x53cf),
    ),
    (
        "rr_cvtss2sd_5207",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc8],
        0xa838,
        &[
            (8, 0x1288338a53fddc54f3fd8bfcbf8a6dd7),
            (9, 0x2b6969817eb57952c8b7b1c5deb915b2),
        ],
        Expect::Ok(0x2b6969817eb57952bff14dbae0000000, 0xa838),
    ),
    (
        "rr_cvtss2sd_5229",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xc3],
        0xb961,
        &[
            (8, 0x56b3795ba8d3ebca596d95c9eb3a2a1d),
            (11, 0x32fd1e1c02199806f6137f0e7fba1118),
        ],
        Expect::Fault(0xb961),
    ),
    (
        "rr_cvtss2sd_5248",
        &[0xf3, 0x0f, 0x5a, 0xc7],
        0x5dc6,
        &[
            (0, 0xa2cbe6fbfa5a703c33e7cbc7ebc12786),
            (7, 0xe2f6bb5200fac94329c166db4bb65726),
        ],
        Expect::Ok(0xa2cbe6fbfa5a703c4176cae4c0000000, 0x5dc6),
    ),
    (
        "rr_cvtss2sd_5348",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xce],
        0x5f00,
        &[
            (1, 0x22219d91835c7bdbdd5faa42c4407022),
            (14, 0xe0f794f69b0f92e844635a8f006819b5),
        ],
        Expect::Ok(0x22219d91835c7bdb380a066d40000000, 0x5f02),
    ),
    (
        "rr_cvtss2sd_5376",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xdb],
        0x5772,
        &[
            (3, 0x750913b4c61caf7e17d2c2b1450564d5),
            (11, 0x141b5cd5e8cdccba0d1170367f9a4908),
        ],
        Expect::Fault(0x5773),
    ),
    (
        "rr_cvtss2sd_5489",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd7],
        0xdcb5,
        &[
            (10, 0x2a2f31b2579ee3713161b5700d7ad0fe),
            (15, 0x8c4991c67fa00898f3cea6e78b62e9f1),
        ],
        Expect::Ok(0x2a2f31b2579ee371b96c5d3e20000000, 0xdcb5),
    ),
    (
        "rr_cvtss2sd_5859",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xf3],
        0x7d80,
        &[
            (6, 0xecfa7ba41451dca317f64864ded3fa91),
            (11, 0xafcccd2d09753474fb8c9d8180be87a7),
        ],
        Expect::Ok(0xecfa7ba41451dca3b817d0f4e0000000, 0x7d80),
    ),
    (
        "rr_cvtss2sd_5863",
        &[0xf3, 0x0f, 0x5a, 0xcb],
        0xa303,
        &[
            (1, 0x4aca40963c509e14702888834ed822d4),
            (3, 0x76250c93b6fc15d313db75478b5ae5d4),
        ],
        Expect::Ok(0x4aca40963c509e14b96b5cba80000000, 0xa303),
    ),
    (
        "rr_cvtss2sd_5895",
        &[0xf3, 0x0f, 0x5a, 0xde],
        0x3fe3,
        &[
            (3, 0x7fef00d7f4b765a51504d0451122b482),
            (6, 0x325a4a807b87757781bee95d3048c85e),
        ],
        Expect::Ok(0x7fef00d7f4b765a53e09190bc0000000, 0x3fe3),
    ),
    (
        "rr_cvtss2sd_6043",
        &[0xf3, 0x0f, 0x5a, 0xfb],
        0x9780,
        &[
            (3, 0xd96366b43b7582816e64de8e000bf1dd),
            (7, 0xd0a4ddd073e54841ad50315d5040054e),
        ],
        Expect::Ok(0xd0a4ddd073e5484137d7e3ba00000000, 0x9782),
    ),
    (
        "rr_cvtss2sd_6066",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xdf],
        0x22f8,
        &[
            (11, 0xff87b3b435e6f782d79fa46f52ea9f1a),
            (15, 0x3ab0a46e51c2b1fca5ac0b4d7fb1a8bb),
        ],
        Expect::Ok(0xff87b3b435e6f7827ffe351760000000, 0x22f9),
    ),
    (
        "rr_cvtss2sd_6075",
        &[0xf3, 0x0f, 0x5a, 0xe4],
        0x5f00,
        &[(4, 0xf2dad8a71dee95a7393d086bb018ccf4)],
        Expect::Ok(0xf2dad8a71dee95a7be03199e80000000, 0x5f00),
    ),
    (
        "rr_cvtss2sd_6114",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xfb],
        0x8000,
        &[
            (11, 0x0e9b71ae02554400b8a4b77780b76305),
            (15, 0xf6078b4d06c05f8db26e1bd5d4718453),
        ],
        Expect::Ok(0xf6078b4d06c05f8db816ec60a0000000, 0x8000),
    ),
    (
        "rr_cvtss2sd_615",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xdc],
        0xfd80,
        &[
            (3, 0xb4c6ae09f9e85c92606baa31ad913250),
            (12, 0xfa2cd7a2f0f864d2fb6006d3cab9aee4),
        ],
        Expect::Ok(0xb4c6ae09f9e85c92c15735dc80000000, 0xfd80),
    ),
    (
        "rr_cvtss2sd_6184",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc7],
        0x3b80,
        &[
            (7, 0x8e0b2f3fad3bb9837a5ab497cac18e54),
            (8, 0xf00587ad287b4927b636022c3b6a5d84),
        ],
        Expect::Ok(0xf00587ad287b4927c15831ca80000000, 0x3b80),
    ),
    (
        "rr_cvtss2sd_6372",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xc4],
        0xd4af,
        &[
            (0, 0xab3634a31389087b4b1e2772e6ea00e2),
            (12, 0x42044af4dbe85370c0c21ae5ff917c6b),
        ],
        Expect::Ok(0xab3634a31389087bfffa2f8d60000000, 0xd4af),
    ),
    (
        "rr_cvtss2sd_6383",
        &[0xf3, 0x0f, 0x5a, 0xd7],
        0x4022,
        &[
            (2, 0xbd2d45c7df72af003357db89dc7c28b1),
            (7, 0x572c962a927e8d2fab5bc6a60049f204),
        ],
        Expect::Fault(0x4022),
    ),
    (
        "rr_cvtss2sd_6427",
        &[0xf3, 0x0f, 0x5a, 0xf5],
        0xff00,
        &[
            (5, 0x72bbda8194f5e09e1cfa01e37fae3e1e),
            (6, 0xa7cacfb7aad709b8defd68572b6fef61),
        ],
        Expect::Fault(0xff01),
    ),
    (
        "rr_cvtss2sd_658",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd6],
        0x7f00,
        &[
            (10, 0xf54b13f17e87e974d2d277863fded758),
            (14, 0x1fc2f78d575acb015e15bb1b006fe8d7),
        ],
        Expect::Ok(0xf54b13f17e87e974380bfa35c0000000, 0x7f02),
    ),
    (
        "rr_cvtss2sd_667",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xe2],
        0x5faa,
        &[
            (4, 0x630aec73e0995a14929cd7a2554c9e8a),
            (10, 0xc50bec38631adc48ef4a214fffa9fce1),
        ],
        Expect::Ok(0x630aec73e0995a14fffd3f9c20000000, 0x5fab),
    ),
    (
        "rr_cvtss2sd_6683",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xea],
        0xc3f4,
        &[
            (2, 0x4df0b0a76b6d610894988ed2ff8f624e),
            (13, 0x3ca667e36db14afe2ea6d0c82fc2e11c),
        ],
        Expect::Ok(0x3ca667e36db14afefff9ec49c0000000, 0xc3f5),
    ),
    (
        "rr_cvtss2sd_6870",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xda],
        0xce05,
        &[
            (2, 0x4efa6094cf4242f78898ae80004f7ea9),
            (11, 0xed079a34be8ed94c46383d0fd2365d2a),
        ],
        Expect::Fault(0xce07),
    ),
    (
        "rr_cvtss2sd_6907",
        &[0xf3, 0x45, 0x0f, 0x5a, 0xd4],
        0x7f40,
        &[
            (10, 0x5018bb09fb2cd2410e3beaa6b1eca115),
            (12, 0x29968a72e0ccb09c7bd87aecff9fcb86),
        ],
        Expect::Fault(0x7f41),
    ),
    (
        "rr_cvtss2sd_6912",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xcd],
        0xc138,
        &[
            (5, 0xd3877cfb91f2f5c655f8a4b3c0573791),
            (9, 0x5e61cbe0caa3a61e14bfb111f8803134),
        ],
        Expect::Ok(0x5e61cbe0caa3a61ec00ae6f220000000, 0xc138),
    ),
    (
        "rr_cvtss2sd_6967",
        &[0xf3, 0x0f, 0x5a, 0xe1],
        0xd230,
        &[
            (1, 0x87c35c443d0fdc5f59be28af005bc6dd),
            (4, 0xf36dcd596209ced1e8177921e8d73e2d),
        ],
        Expect::Fault(0xd232),
    ),
    (
        "rr_cvtss2sd_6990",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xdd],
        0x5faa,
        &[
            (3, 0x6cd46842ac9bee96e9ec935c963ca6f4),
            (13, 0x60cd2be5035f68a33f268e4ec076adec),
        ],
        Expect::Ok(0x6cd46842ac9bee96c00ed5bd80000000, 0x5faa),
    ),
    (
        "rr_cvtss2sd_765",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xca],
        0xa12c,
        &[
            (1, 0xce27b689285ce72c59fc889bd823324a),
            (10, 0x47b390b95a86f93c249a666affa94a7e),
        ],
        Expect::Fault(0xa12d),
    ),
    (
        "rr_cvtss2sd_887",
        &[0xf3, 0x0f, 0x5a, 0xea],
        0x7483,
        &[
            (2, 0xb63937c8da9c92493862c98f007e3874),
            (5, 0x8427f73a67ab15506bccd1b7c55bce0f),
        ],
        Expect::Fault(0x7483),
    ),
    (
        "rr_cvtss2sd_899",
        &[0xf3, 0x44, 0x0f, 0x5a, 0xc8],
        0xac36,
        &[
            (0, 0x6eb70163687423ab27f8934d80524f75),
            (9, 0x33571011be62d6d93000cbe1a6388aea),
        ],
        Expect::Fault(0xac36),
    ),
    (
        "rr_cvtss2sd_979",
        &[0xf3, 0x41, 0x0f, 0x5a, 0xf5],
        0xfd1b,
        &[
            (6, 0x69ac201915dc6e441da1ddf49c339ca2),
            (13, 0x18cd5da1b65109bd6be201b17fdcc158),
        ],
        Expect::Ok(0x69ac201915dc6e447ffb982b00000000, 0xfd1b),
    ),
    (
        "rr_rcpps_148",
        &[0x0f, 0x53, 0xe1],
        0x380,
        &[
            (1, 0x004cea27ffddc4f300507844802b3a4d),
            (4, 0x59d6ac360aadc909979324cacdd15db6),
        ],
        Expect::Ok(0x7f800000ffddc4f37f800000ff800000, 0x380),
    ),
    (
        "rr_rcpps_2656",
        &[0x45, 0x0f, 0x53, 0xc0],
        0x9b80,
        &[(8, 0x006af6307f5db93d006a3584ff661aec)],
        Expect::Ok(0x7f800000000000007f80000080000000, 0x9b80),
    ),
    (
        "rr_rcpps_3902",
        &[0x0f, 0x53, 0xc5],
        0xef80,
        &[
            (0, 0xd050c052b1662967f7386aa5a32daf16),
            (5, 0x0002f4cf00407e328013efc8005124a1),
        ],
        Expect::Ok(0x7f8000007f800000ff8000007f800000, 0xef80),
    ),
    (
        "rr_rcpss_2524",
        &[0xf3, 0x44, 0x0f, 0x53, 0xf6],
        0x5b80,
        &[
            (6, 0x8058141d3f3cc11d92e3af68ff042bff),
            (14, 0x8d3d16b0bf98c741ded694f37d51b967),
        ],
        Expect::Ok(0x8d3d16b0bf98c741ded694f380000000, 0x5b80),
    ),
    (
        "rr_rcpss_273",
        &[0xf3, 0x45, 0x0f, 0x53, 0xc2],
        0x1f80,
        &[
            (8, 0xb8bd20e13dd4d3a8d2eaf7ce48b15998),
            (10, 0xef35a1c215d9d70fbfcdcb2dffe28b13),
        ],
        Expect::Ok(0xb8bd20e13dd4d3a8d2eaf7ceffe28b13, 0x1f80),
    ),
    (
        "rr_rcpss_3329",
        &[0xf3, 0x45, 0x0f, 0x53, 0xec],
        0x1780,
        &[
            (12, 0xba21932b0bb06a8dec29eb537f2f760d),
            (13, 0xb8fcd79175bfb8b7974ca41bf031d079),
        ],
        Expect::Ok(0xb8fcd79175bfb8b7974ca41b00000000, 0x1780),
    ),
    (
        "rr_rsqrtps_1814",
        &[0x41, 0x0f, 0x52, 0xc5],
        0x1a00,
        &[
            (0, 0x63d603510e7ad5e59e9b2af4337a67c9),
            (13, 0xbfc0673aff847eeccbfb2a83f7ee0e28),
        ],
        Expect::Ok(0xffc00000ffc47eecffc00000ffc00000, 0x1a00),
    ),
    (
        "rr_rsqrtps_27",
        &[0x0f, 0x52, 0xcc],
        0xfd80,
        &[
            (1, 0x5d089d10c3044bdd6419eee370aacfef),
            (4, 0x0070f474ffac68d7815aed5080e4434f),
        ],
        Expect::Ok(0x7f800000ffec68d7ffc00000ffc00000, 0xfd80),
    ),
    (
        "rr_rsqrtps_3075",
        &[0x41, 0x0f, 0x52, 0xf8],
        0x1b80,
        &[
            (7, 0xe8215a10e189e62a10a6735fd5f7f33f),
            (8, 0xf495c88680832943cc15b9f481118807),
        ],
        Expect::Ok(0xffc00000ffc00000ffc00000ffc00000, 0x1b80),
    ),
    (
        "rr_rsqrtps_3418",
        &[0x45, 0x0f, 0x52, 0xd8],
        0x1d80,
        &[
            (8, 0xff8cd61eca9e1523ff0ddd78ffa5fe19),
            (11, 0x21b69bdcba3f49dc483c278c33877e60),
        ],
        Expect::Ok(0xffccd61effc00000ffc00000ffe5fe19, 0x1d80),
    ),
    (
        "rr_rsqrtps_463",
        &[0x44, 0x0f, 0x52, 0xfe],
        0x0,
        &[
            (6, 0x805b86bb81249c5acb56323980db5aa2),
            (15, 0x5554f496a4b2d24805527fd036f5c3b5),
        ],
        Expect::Ok(0xff800000ffc00000ffc00000ffc00000, 0x0),
    ),
    (
        "rr_rsqrtps_600",
        &[0x44, 0x0f, 0x52, 0xe3],
        0x7780,
        &[
            (3, 0xcc4095fe806a65c8ca9c0ad1ff1af685),
            (12, 0xd713260d589bc94802eefe11731f0356),
        ],
        Expect::Ok(0xffc00000ff800000ffc00000ffc00000, 0x7780),
    ),
    (
        "rr_rsqrtss_1191",
        &[0xf3, 0x44, 0x0f, 0x52, 0xd1],
        0x1f80,
        &[
            (1, 0x1fe94b95e21ffecd81dc4e31802ce19b),
            (10, 0x3f8c417bc8539c216098fb713215bdce),
        ],
        Expect::Ok(0x3f8c417bc8539c216098fb71ff800000, 0x1f80),
    ),
    (
        "rr_rsqrtss_1389",
        &[0xf3, 0x0f, 0x52, 0xf2],
        0x1dc0,
        &[
            (2, 0x5cdcc2f2eec4ad9c46a8c149ffca4317),
            (6, 0xc99821d9212d65354c9ea20f47706fc8),
        ],
        Expect::Ok(0xc99821d9212d65354c9ea20fffca4317, 0x1dc0),
    ),
    (
        "rr_rsqrtss_3169",
        &[0xf3, 0x41, 0x0f, 0x52, 0xfe],
        0x1f95,
        &[
            (7, 0x221592bb5d7d2c67e9587b936a7c98ae),
            (14, 0x604aad780f18fe2d1a5b84f7abdf761a),
        ],
        Expect::Ok(0x221592bb5d7d2c67e9587b93ffc00000, 0x1f95),
    ),
    (
        "rr_rsqrtss_589",
        &[0xf3, 0x45, 0x0f, 0x52, 0xe0],
        0x4000,
        &[
            (8, 0x7d5833bc1667e9ffae019a048049245d),
            (12, 0xd32cb57a72eae09e188769aefcfc8e81),
        ],
        Expect::Ok(0xd32cb57a72eae09e188769aeff800000, 0x4000),
    ),
    (
        "rr_rsqrtss_810",
        &[0xf3, 0x44, 0x0f, 0x52, 0xfd],
        0x4f80,
        &[
            (5, 0xc3523b0599800dd5bafb7724ffe30f55),
            (15, 0x986a1e1a259230792ba3f5b862729554),
        ],
        Expect::Ok(0x986a1e1a259230792ba3f5b8ffe30f55, 0x4f80),
    ),
];
