//! S2-55 slice B: the floating-point SSE4.1 forms (66-prefixed, no VEX): ROUNDPS/ROUNDPD/ROUNDSS/ROUNDSD
//! (66 0F 3A 08..0B), DPPS/DPPD (66 0F 3A 40/41), INSERTPS (66 0F 3A 21), EXTRACTPS (66 0F 3A 17),
//! BLENDPS/BLENDPD (66 0F 3A 0C/0D) and BLENDVPS/BLENDVPD (66 0F 38 14/15, implicit XMM0).
//!
//! Sources (Intel SDM edition 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200, text through pdftotext):
//! Vol. 2A BLENDPD pp. 3-78..3-79, BLENDPS pp. 3-80..3-81, BLENDVPD pp. 3-82..3-83, BLENDVPS pp. 3-84..3-85,
//! DPPD pp. 3-277..3-278, DPPS pp. 3-279..3-281, EXTRACTPS pp. 3-305..3-306, INSERTPS pp. 3-464..3-466; Vol. 2B
//! ROUNDPD pp. 4-583..4-585, ROUNDPS pp. 4-586..4-587, ROUNDSD pp. 4-588..4-589, ROUNDSS pp. 4-590..4-591
//! (Figure 1-23 and Table 1-13: the imm8 control byte, bit 3 = precision suppression, bit 2 = rounding source,
//! bits 1:0 = rounding mode); exception classes Vol. 2A Table 2-19 (Type 2: ROUNDPS/PD and DPPS/PD, aligned
//! m128, #XM), Table 2-20 (Type 3: ROUNDSS/SD, #XM), Table 2-21 (Type 4: BLEND*, aligned m128, no SIMD
//! exception), Table 2-22 (Type 5: INSERTPS and EXTRACTPS); Vol. 1 section 12.8.4 (the six SSE4.1 forms with
//! floating-point arithmetic and Table 12-1: ROUND* signal only invalid for a signaling NaN and precision,
//! never denormal, underflow or overflow; DPPS/DPPD resemble IEEE multiplies and adds with rounding at each
//! stage, NaN selection is implementation specific; the other SSE4.1 forms signal nothing).
//!
//! Where the SDM text does not fix an observable detail, the expected value is the one the physical x86 oracle
//! gave (Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42; the program loads MXCSR with LDMXCSR and runs the
//! instruction directly): ROUND* report an unmasked invalid operation alone (no precision flag next to it); the
//! dot products gather the pre-computation flags of all four multiplies, and of the two pair adds, before they
//! stop, report denormal operands of the intermediate sums, and take the low two destination elements from
//! (pair01 + pair23) with the pair sums formed as (p1 + p0) and (p3 + p2) and the high two from the swapped
//! sum, which only shows in which NaN wins. The GOLDEN table is hardware output, not model output
//! (work/stage8/s2_55b_oracle/: gen_cases.py, pick_golden.py, oracle.c; 203383 cases, all matched except the
//! forms the model refuses). The other tests use literal values computed by hand from the SDM operation.
//!
//! Not asserted here: the CPUID gate (S2-57), "#AC" of the Type 3 and Type 5 forms (an unaligned operand at CPL3
//! with RFLAGS.AC = 1 is Unsupported instead of guessed, as in S2-55 slice A), FS/GS overrides and the
//! mandatory-prefix sequences the hardware tolerates (they stay Unsupported), the JIT (refused, see od-jit),
//! CPL3 (refused, see cpl3_profile_set).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, Operation, Sse41FpKind as Kind, Sse41Operand};
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
const MXCSR_IE: u32 = 0x1;
const MXCSR_DE: u32 = 0x2;
const MXCSR_OE: u32 = 0x8;
const MXCSR_PE: u32 = 0x20;
const MXCSR_DAZ: u32 = 0x40;
const MXCSR_IM: u32 = 0x80;
const MXCSR_OM: u32 = 0x400;
const MXCSR_PM: u32 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;
const CR4_OSXMMEXCPT: u64 = 0x400;
const DATA: u64 = 0x8000;

/// Every form: kind, name, opcode bytes after the 66 prefix, has imm8, memory bytes, 16-byte aligned operand.
const KINDS: [(Kind, &str, [u8; 3], bool, usize, bool); 12] = [
    (Kind::Roundps, "roundps", [0x0f, 0x3a, 0x08], true, 16, true),
    (Kind::Roundpd, "roundpd", [0x0f, 0x3a, 0x09], true, 16, true),
    (Kind::Roundss, "roundss", [0x0f, 0x3a, 0x0a], true, 4, false),
    (Kind::Roundsd, "roundsd", [0x0f, 0x3a, 0x0b], true, 8, false),
    (Kind::Blendps, "blendps", [0x0f, 0x3a, 0x0c], true, 16, true),
    (Kind::Blendpd, "blendpd", [0x0f, 0x3a, 0x0d], true, 16, true),
    (
        Kind::Blendvps,
        "blendvps",
        [0x0f, 0x38, 0x14],
        false,
        16,
        true,
    ),
    (
        Kind::Blendvpd,
        "blendvpd",
        [0x0f, 0x38, 0x15],
        false,
        16,
        true,
    ),
    (
        Kind::Extractps,
        "extractps",
        [0x0f, 0x3a, 0x17],
        true,
        4,
        false,
    ),
    (
        Kind::Insertps,
        "insertps",
        [0x0f, 0x3a, 0x21],
        true,
        4,
        false,
    ),
    (Kind::Dpps, "dpps", [0x0f, 0x3a, 0x40], true, 16, true),
    (Kind::Dppd, "dppd", [0x0f, 0x3a, 0x41], true, 16, true),
];

fn spec(kind: Kind) -> &'static (Kind, &'static str, [u8; 3], bool, usize, bool) {
    KINDS.iter().find(|entry| entry.0 == kind).unwrap()
}

/// 66 [REX] 0F op /r [ib] with a register operand (for EXTRACTPS rm is a general register number).
fn reg_code(kind: Kind, reg: u8, rm: u8, immediate: u8) -> Vec<u8> {
    reg_code_rex(kind, reg, rm, immediate, false)
}

fn reg_code_rex(kind: Kind, reg: u8, rm: u8, immediate: u8, rex_w: bool) -> Vec<u8> {
    let (_, _, opcode, has_immediate, _, _) = *spec(kind);
    let mut code = vec![0x66];
    let rex = 0x40 | u8::from(rex_w) << 3 | u8::from(reg >= 8) << 2 | u8::from(rm >= 8);
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend(opcode);
    code.push(0xc0 | (reg & 7) << 3 | (rm & 7));
    if has_immediate {
        code.push(immediate);
    }
    code
}

/// 66 0F op /r [ib] with the memory operand [rax].
fn mem_code(kind: Kind, reg: u8, immediate: u8) -> Vec<u8> {
    let (_, _, opcode, has_immediate, _, _) = *spec(kind);
    let mut code = vec![0x66];
    if reg >= 8 {
        code.push(0x44);
    }
    code.extend(opcode);
    code.push((reg & 7) << 3);
    if has_immediate {
        code.push(immediate);
    }
    code
}

struct Ram {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below (an architectural fault of the backend,
    /// which the instruction must not turn into a partial transfer).
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

fn fault(vector: u8, error_code: Option<u32>) -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}

fn gp0() -> StepOutcome {
    fault(13, Some(0))
}

fn ss0() -> StepOutcome {
    fault(12, Some(0))
}

fn ud() -> StepOutcome {
    fault(6, None)
}

fn xm_exit() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 19,
        error_code: None,
        linear_address: None,
    })
}

fn ps(lanes: [u32; 4]) -> u128 {
    lanes.iter().enumerate().fold(0, |value, (index, lane)| {
        value | u128::from(*lane) << (32 * index)
    })
}

fn pd(lanes: [u64; 2]) -> u128 {
    u128::from(lanes[0]) | u128::from(lanes[1]) << 64
}

/// binary32 and binary64 encodings of literal values (only the encodings of the literals are used, no host
/// arithmetic).
fn s(value: f32) -> u32 {
    value.to_bits()
}

fn d(value: f64) -> u64 {
    value.to_bits()
}

/// What a vector of the GOLDEN table must produce: the destination register and the MXCSR afterwards, or a
/// SIMD floating-point exception (#XM) with the MXCSR the fault leaves behind (destination unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Ok(u128, u32),
    Fault(u32),
}

/// The outcome of one instruction under the FP-init profile pair with CR4.OSXMMEXCPT = 1.
struct Observed {
    exit: RunExit,
    before: LongModeState,
    after: LongModeState,
    ram: Ram,
    length: usize,
}

fn run_with(code: &[u8], cpu: LongModeState, setup: impl FnOnce(&mut Ram)) -> Observed {
    let before = cpu.clone();
    let mut ram = Ram::new(code);
    setup(&mut ram);
    let (result, after) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
    Observed {
        exit: result.exit,
        before,
        after,
        ram,
        length: code.len(),
    }
}

/// Register-form run: the given XMM registers, the MXCSR, CR4.OSXMMEXCPT = 1.
fn run_regs(code: &[u8], regs: &[(usize, u128)], mxcsr: u32) -> Observed {
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT);
    cpu.sse.as_mut().unwrap().mxcsr = mxcsr;
    for (index, value) in regs {
        set_xmm(&mut cpu, *index, *value);
    }
    run_with(code, cpu, |_| {})
}

/// Whole-state check of one run against an expectation: only the destination XMM, MXCSR and RIP may change
/// (a fault changes MXCSR only), and a fault is #XM.
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
            assert_eq!(observed.exit, xm_exit(), "{label}");
            expected.sse.as_mut().unwrap().mxcsr = mxcsr;
        }
    }
    assert_eq!(observed.after, expected, "{label}");
}

/// Runs a register form and asserts the destination (xmm1) and the MXCSR; the other state is unchanged.
fn assert_reg(code: &[u8], regs: &[(usize, u128)], mxcsr_in: u32, result: u128, mxcsr_out: u32) {
    let observed = run_regs(code, regs, mxcsr_in);
    check_observed(
        &observed,
        1,
        Expect::Ok(result, mxcsr_out),
        &format!("{code:02x?}"),
    );
}

/// Runs a register form that must raise #XM with the given MXCSR and leave the destination alone.
fn assert_xm(code: &[u8], regs: &[(usize, u128)], mxcsr_in: u32, mxcsr_out: u32) {
    let observed = run_regs(code, regs, mxcsr_in);
    check_observed(
        &observed,
        1,
        Expect::Fault(mxcsr_out),
        &format!("{code:02x?}"),
    );
}

/// Bare run (no CR0/CR4 in the state) of one instruction at CPL0 or CPL3.
fn exec(
    code: &[u8],
    privilege: PrivilegeLevel,
    prepare: impl FnOnce(&mut LongModeState, &mut Ram),
) -> (
    Result<StepOutcome, MemoryError>,
    LongModeState,
    LongModeState,
    Ram,
) {
    let mut cpu = cpu();
    let mut ram = Ram::new(code);
    prepare(&mut cpu, &mut ram);
    let before = cpu.clone();
    let outcome = step(&mut cpu, &mut ram, privilege);
    (outcome, before, cpu, ram)
}

fn rax_at(address: u64) -> impl FnOnce(&mut LongModeState, &mut Ram) {
    move |cpu, _| cpu.gpr[Register::Rax as usize] = address
}

// ---- golden vectors from the physical x86 oracle ----

/// Destination register number of a register-form encoding: [66] [REX] 0F map op modrm.
fn destination_of(code: &[u8]) -> usize {
    let mut index = 0;
    let mut rex = 0;
    while code[index] == 0x66 || (0x40..=0x4f).contains(&code[index]) {
        if code[index] != 0x66 {
            rex = code[index];
        }
        index += 1;
    }
    assert_eq!(code[index], 0x0f);
    let modrm = code[index + 3];
    usize::from((modrm >> 3) & 7 | (rex & 4) << 1)
}

#[test]
fn the_oracle_vectors_agree_for_round_dot_product_blend_and_insert_forms() {
    for (name, code, mxcsr_in, regs, expect) in GOLDEN {
        let observed = run_regs(code, regs, *mxcsr_in);
        check_observed(&observed, destination_of(code), *expect, name);
        assert!(
            observed.ram.reads.is_empty() && observed.ram.writes.is_empty(),
            "{name}"
        );
    }
}

// ---- decoding ----

#[test]
fn the_decoder_reports_kind_registers_operand_immediate_and_length() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // Every form, register operand, REX.R and REX.B extend the registers, the immediate is the last byte.
    for (kind, name, opcode, has_immediate, _, _) in KINDS {
        let code = if kind == Kind::Extractps {
            // r9d is the destination of EXTRACTPS xmm10 (reg), REX.W ignored.
            reg_code_rex(kind, 10, 9, 0x1e, true)
        } else {
            reg_code(kind, 10, 12, 0x1e)
        };
        let instruction = decode(&code);
        assert_eq!(usize::from(instruction.length), code.len(), "{name}");
        assert_eq!(instruction.opcode.byte, opcode[2], "{name}");
        let expected_operand = if kind == Kind::Extractps {
            Sse41Operand::Gpr(9)
        } else {
            Sse41Operand::Xmm(12)
        };
        assert!(
            matches!(
                instruction.operation,
                Operation::Sse41Fp { kind: k, xmm: 10, operand, immediate }
                    if k == kind && operand == expected_operand && immediate == if has_immediate { 0x1e } else { 0 }
            ),
            "{name}: {:?}",
            instruction.operation
        );
        // Memory operand [rax + rcx*4 + 0x10]: the immediate follows the displacement.
        let mut memory = vec![0x66];
        memory.extend(opcode);
        memory.extend([0x44, 0x88, 0x10]);
        if has_immediate {
            memory.push(0x7b);
        }
        let instruction = decode(&memory);
        assert_eq!(
            usize::from(instruction.length),
            memory.len(),
            "{name} memory"
        );
        assert!(
            matches!(
                instruction.operation,
                Operation::Sse41Fp { kind: k, xmm: 0, operand: Sse41Operand::Memory(_), immediate }
                    if k == kind && immediate == if has_immediate { 0x7b } else { 0 }
            ),
            "{name} memory: {:?}",
            instruction.operation
        );
    }
    // Kind helpers.
    for (kind, name, _, has_immediate, bytes, aligned) in KINDS {
        assert_eq!(kind.has_immediate(), has_immediate, "{name}");
        assert_eq!(kind.memory_bytes(), bytes, "{name}");
        assert_eq!(kind.requires_alignment(), aligned, "{name}");
    }
}

#[test]
fn lock_is_ud_for_every_form_and_prefix_order() {
    for (_, name, opcode, has_immediate, _, _) in KINDS {
        let tail: &[u8] = if has_immediate { &[0x00] } else { &[] };
        let codes: Vec<Vec<u8>> = vec![
            [&[0xf0, 0x66][..], &opcode, &[0x00], tail].concat(),
            [&[0x66, 0xf0][..], &opcode, &[0x00], tail].concat(),
            [&[0xf0, 0x66, 0x48][..], &opcode, &[0x10, 0x20][..], tail].concat(),
            [&[0xf0, 0x66][..], &opcode, &[0x05, 0, 0, 0, 0][..], tail].concat(),
            [&[0xf0, 0x66][..], &opcode, &[0xc1], tail].concat(),
            [&[0x66, 0xf0][..], &opcode, &[0xc1], tail].concat(),
            [&[0x66, 0x44, 0xf0][..], &opcode, &[0xc1], tail].concat(),
            [&[0xf0, 0x2e, 0x66][..], &opcode, &[0x00], tail].concat(),
        ];
        for code in codes {
            let (outcome, before, after, ram) = exec(&code, PrivilegeLevel::Ring0, |_, _| {});
            assert_eq!(outcome, Ok(ud()), "{name} {code:02x?}");
            assert_eq!(after, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            assert!(matches!(
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()),
                Err(long_decode::DecodeError::Invalid { .. })
            ));
        }
    }
}

#[test]
fn neighbouring_encodings_stay_unsupported_without_change() {
    let mut refused: Vec<(Vec<u8>, String)> = vec![];
    for (_, name, opcode, has_immediate, _, _) in KINDS {
        let tail: &[u8] = if has_immediate { &[0x04] } else { &[] };
        // Without the mandatory 66 prefix these opcodes are not SSE4.1: refused.
        for modrm in [0x00u8, 0xc1] {
            refused.push((
                [&opcode[..], &[modrm], tail].concat(),
                format!("{name} no 66 modrm {modrm:#x}"),
            ));
        }
        // F2 / F3 (with or without 66), repeated prefixes, FS and GS (no segment base in this operand model).
        for (prefix, what) in [
            (&[0xf2u8][..], "F2"),
            (&[0xf3], "F3"),
            (&[0xf2, 0x66], "F2 66"),
            (&[0x66, 0xf2], "66 F2"),
            (&[0xf3, 0x66], "F3 66"),
            (&[0x66, 0xf3], "66 F3"),
            (&[0xf2, 0xf3, 0x66], "F2 F3 66"),
            (&[0x66, 0x66], "duplicate 66"),
            (&[0x67, 0x67, 0x66], "duplicate 67"),
            (&[0x2e, 0x3e, 0x66], "two segment overrides"),
            (&[0x64, 0x66], "FS"),
            (&[0x66, 0x65], "GS"),
            (&[0x66, 0x40, 0x40], "duplicate REX"),
            (&[0xf0, 0xf0, 0x66], "duplicate LOCK"),
        ] {
            for modrm in [0x00u8, 0xc1] {
                refused.push((
                    [prefix, &opcode, &[modrm][..], tail].concat(),
                    format!("{name} {what} modrm {modrm:#x}"),
                ));
            }
        }
    }
    // Neighbouring opcodes of the same maps that are no SSE4.1 form of this slice.
    for opcode in [0x13u8, 0x1a, 0x1b, 0x43, 0x44, 0x4a] {
        refused.push((
            vec![0x66, 0x0f, 0x3a, opcode, 0x00, 0x01],
            format!("66 0F 3A {opcode:02x}"),
        ));
    }
    for opcode in [0x11u8, 0x12, 0x13, 0x16, 0x18, 0x19, 0x1a] {
        refused.push((
            vec![0x66, 0x0f, 0x38, opcode, 0x00],
            format!("66 0F 38 {opcode:02x}"),
        ));
    }
    for (code, name) in &refused {
        let (outcome, before, after, ram) = exec(code, PrivilegeLevel::Ring0, |_, _| {});
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{name}: {outcome:?}"
        );
        assert_eq!(after, before, "{name}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
    }
}

// ---- ROUNDSS / ROUNDSD / ROUNDPS / ROUNDPD ----

const ROUND_INPUTS: [f64; 10] = [2.5, 3.5, -2.5, -0.5, 0.5, 1.5, -1.5, 2.0, 0.3, -0.3];
/// SDM Table 1-13 applied by hand: nearest even, down, up, toward zero.
const ROUND_RESULTS: [[f64; 10]; 4] = [
    [2.0, 4.0, -2.0, -0.0, 0.0, 2.0, -2.0, 2.0, 0.0, -0.0],
    [2.0, 3.0, -3.0, -1.0, 0.0, 1.0, -2.0, 2.0, 0.0, -1.0],
    [3.0, 4.0, -2.0, -0.0, 1.0, 2.0, -1.0, 2.0, 1.0, -0.0],
    [2.0, 3.0, -2.0, -0.0, 0.0, 1.0, -1.0, 2.0, 0.0, -0.0],
];
const ROUND_INEXACT: [bool; 10] = [true, true, true, true, true, true, true, false, true, true];

#[test]
fn roundsd_and_roundss_round_in_the_four_modes_of_imm8_and_set_precision_unless_suppressed() {
    for (mode, expected) in ROUND_RESULTS.iter().enumerate() {
        for (index, input) in ROUND_INPUTS.iter().enumerate() {
            for suppress in [0u8, 8] {
                let imm = mode as u8 | suppress;
                let mxcsr_out = MXCSR
                    | if ROUND_INEXACT[index] && suppress == 0 {
                        MXCSR_PE
                    } else {
                        0
                    };
                // ROUNDSD: the high quadword of the destination is kept.
                assert_reg(
                    &reg_code(Kind::Roundsd, 1, 2, imm),
                    &[(1, pd([d(7.0), d(9.0)])), (2, pd([d(*input), d(5.0)]))],
                    MXCSR,
                    pd([d(expected[index]), d(9.0)]),
                    mxcsr_out,
                );
                // ROUNDSS: bits 127:32 of the destination are kept.
                assert_reg(
                    &reg_code(Kind::Roundss, 1, 2, imm),
                    &[
                        (1, ps([s(7.0), s(8.0), s(9.0), s(10.0)])),
                        (2, ps([s(*input as f32), s(5.0), s(6.0), s(7.0)])),
                    ],
                    MXCSR,
                    ps([s(expected[index] as f32), s(8.0), s(9.0), s(10.0)]),
                    mxcsr_out,
                );
            }
        }
    }
}

#[test]
fn imm8_bit_2_takes_the_rounding_mode_from_mxcsr_and_ignores_imm8_bits_1_0() {
    for rc in 0..4u32 {
        for low in 0..4u8 {
            for (index, input) in ROUND_INPUTS.iter().enumerate() {
                assert_reg(
                    &reg_code(Kind::Roundsd, 1, 2, 4 | low | 8),
                    &[(1, pd([0, 0])), (2, pd([d(*input), 0]))],
                    MXCSR | rc << 13,
                    pd([d(ROUND_RESULTS[rc as usize][index]), 0]),
                    MXCSR | rc << 13,
                );
            }
        }
    }
    // With bit 2 clear the MXCSR rounding mode is not looked at: imm8 = 1 (down) under MXCSR.RC = 3 (truncate).
    assert_reg(
        &reg_code(Kind::Roundsd, 1, 2, 9),
        &[(1, pd([0, 0])), (2, pd([d(-2.5), 0]))],
        MXCSR | 3 << 13,
        pd([d(-3.0), 0]),
        MXCSR | 3 << 13,
    );
    // The upper bits of imm8 (7:4) are ignored.
    assert_reg(
        &reg_code(Kind::Roundsd, 1, 2, 0xf1),
        &[(1, pd([0, 0])), (2, pd([d(-2.5), 0]))],
        MXCSR,
        pd([d(-3.0), 0]),
        MXCSR | MXCSR_PE,
    );
}

#[test]
fn roundps_and_roundpd_round_every_element_and_or_the_flags() {
    // ROUNDPS to nearest even: 2.5 -> 2, -1.5 -> -2, 0.5 -> 0, 100 stays.
    assert_reg(
        &reg_code(Kind::Roundps, 1, 2, 0),
        &[
            (1, ps([1, 2, 3, 4])),
            (2, ps([s(2.5), s(-1.5), s(0.5), s(100.0)])),
        ],
        MXCSR,
        ps([s(2.0), s(-2.0), s(0.0), s(100.0)]),
        MXCSR | MXCSR_PE,
    );
    // ROUNDPD up with the precision exception suppressed: 3.5 -> 4, -0.5 -> -0.
    assert_reg(
        &reg_code(Kind::Roundpd, 1, 2, 2 | 8),
        &[(1, pd([1, 2])), (2, pd([d(3.5), d(-0.5)]))],
        MXCSR,
        pd([d(4.0), d(-0.0)]),
        MXCSR,
    );
    // All elements exact: no flag.
    assert_reg(
        &reg_code(Kind::Roundps, 1, 2, 3),
        &[(1, 0), (2, ps([s(1.0), s(-2.0), s(0.0), s(1048576.0)]))],
        MXCSR,
        ps([s(1.0), s(-2.0), s(0.0), s(1048576.0)]),
        MXCSR,
    );
}

#[test]
fn round_leaves_infinities_zeros_quiet_nans_and_large_integers_alone_without_flags() {
    for value in [
        0u64,
        1 << 63,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x7ff8_0000_0000_0001,
        0xfff8_dead_beef_0000,
        0x4330_0000_0000_0000,
        0x4330_0000_0000_0001,
        0xc330_0000_0000_0001,
        0x7fef_ffff_ffff_ffff,
    ] {
        for imm in 0..4u8 {
            assert_reg(
                &reg_code(Kind::Roundsd, 1, 2, imm),
                &[(1, 0), (2, pd([value, 0]))],
                MXCSR,
                pd([value, 0]),
                MXCSR,
            );
        }
    }
    for value in [
        0x4b00_0000u32,
        0x4b00_0001,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0001,
        0x7f7f_ffff,
    ] {
        assert_reg(
            &reg_code(Kind::Roundss, 1, 2, 0),
            &[(1, 0), (2, ps([value, 0, 0, 0]))],
            MXCSR,
            ps([value, 0, 0, 0]),
            MXCSR,
        );
    }
}

#[test]
fn round_quiets_a_signaling_nan_and_sets_invalid_and_an_unmasked_invalid_raises_xm_alone() {
    let snan = 0x7ff0_0000_0000_0001u64;
    let neg_snan = 0xfff0_0000_0000_0001u64;
    assert_reg(
        &reg_code(Kind::Roundsd, 1, 2, 8),
        &[(1, 0), (2, pd([snan, 0]))],
        MXCSR,
        pd([0x7ff8_0000_0000_0001, 0]),
        MXCSR | MXCSR_IE,
    );
    assert_reg(
        &reg_code(Kind::Roundpd, 1, 2, 0),
        &[(1, 0), (2, pd([neg_snan, d(2.5)]))],
        MXCSR,
        pd([0xfff8_0000_0000_0001, d(2.0)]),
        MXCSR | MXCSR_IE | MXCSR_PE,
    );
    assert_reg(
        &reg_code(Kind::Roundss, 1, 2, 8),
        &[(1, 0), (2, ps([0x7fa0_0000, 0, 0, 0]))],
        MXCSR,
        ps([0x7fe0_0000, 0, 0, 0]),
        MXCSR | MXCSR_IE,
    );
    // Unmasked IM: #XM, the destination is not written and only IE is reported (the hardware does not add the
    // precision flag of the other element, nor evaluates it).
    assert_xm(
        &reg_code(Kind::Roundpd, 1, 2, 0),
        &[(1, pd([5, 6])), (2, pd([snan, d(2.5)]))],
        0,
        MXCSR_IE,
    );
    assert_xm(
        &reg_code(Kind::Roundsd, 1, 2, 8),
        &[(1, pd([5, 6])), (2, pd([snan, 0]))],
        MXCSR & !MXCSR_IM,
        (MXCSR & !MXCSR_IM) | MXCSR_IE,
    );
}

#[test]
fn an_unmasked_precision_exception_raises_xm_only_when_imm8_bit_3_is_clear() {
    let unmasked = MXCSR & !MXCSR_PM;
    // Inexact, bit 3 clear: #XM, PE set, destination unchanged.
    assert_xm(
        &reg_code(Kind::Roundsd, 1, 2, 0),
        &[(1, pd([5, 6])), (2, pd([d(2.5), 0]))],
        unmasked,
        unmasked | MXCSR_PE,
    );
    // Inexact, bit 3 set: the precision mask is ignored, nothing is signaled.
    assert_reg(
        &reg_code(Kind::Roundsd, 1, 2, 8),
        &[(1, pd([5, 6])), (2, pd([d(2.5), 0]))],
        unmasked,
        pd([d(2.0), 6]),
        unmasked,
    );
    // Exact: nothing to signal either way.
    assert_reg(
        &reg_code(Kind::Roundsd, 1, 2, 0),
        &[(1, pd([5, 6])), (2, pd([d(2.0), 0]))],
        unmasked,
        pd([d(2.0), 6]),
        unmasked,
    );
}

#[test]
fn round_handles_denormals_without_denormal_or_underflow_flags_and_daz_flushes_them() {
    let tiny = 1u64; // the smallest positive binary64 denormal
    let neg_tiny = (1u64 << 63) | 1;
    for (imm, input, expected, inexact) in [
        (0u8, tiny, 0u64, true),
        (1, tiny, 0, true),
        (2, tiny, d(1.0), true),
        (3, tiny, 0, true),
        (1, neg_tiny, d(-1.0), true),
        (0, neg_tiny, 1 << 63, true),
    ] {
        assert_reg(
            &reg_code(Kind::Roundsd, 1, 2, imm),
            &[(1, 0), (2, pd([input, 0]))],
            MXCSR,
            pd([expected, 0]),
            MXCSR | if inexact { MXCSR_PE } else { 0 },
        );
        // DAZ: the denormal is a zero of its sign before rounding: exact, no flag.
        assert_reg(
            &reg_code(Kind::Roundsd, 1, 2, imm),
            &[(1, 0), (2, pd([input, 0]))],
            MXCSR | MXCSR_DAZ,
            pd([input & (1 << 63), 0]),
            MXCSR | MXCSR_DAZ,
        );
    }
    // binary32 denormals the same way.
    assert_reg(
        &reg_code(Kind::Roundss, 1, 2, 2),
        &[(1, 0), (2, ps([1, 0, 0, 0]))],
        MXCSR,
        ps([s(1.0), 0, 0, 0]),
        MXCSR | MXCSR_PE,
    );
    assert_reg(
        &reg_code(Kind::Roundss, 1, 2, 2),
        &[(1, 0), (2, ps([0x8000_0001, 0, 0, 0]))],
        MXCSR | MXCSR_DAZ,
        ps([0x8000_0000, 0, 0, 0]),
        MXCSR | MXCSR_DAZ,
    );
}

// ---- DPPS / DPPD ----

#[test]
fn dpps_multiplies_the_selected_elements_adds_and_broadcasts_to_the_selected_elements() {
    let a = ps([s(1.0), s(2.0), s(3.0), s(4.0)]);
    let b = ps([s(5.0), s(6.0), s(7.0), s(8.0)]);
    for (imm, result) in [
        (0xffu8, [70.0f32, 70.0, 70.0, 70.0]),
        (0x31, [17.0, 0.0, 0.0, 0.0]),
        (0x7e, [0.0, 38.0, 38.0, 38.0]),
        (0xf1, [70.0, 0.0, 0.0, 0.0]),
        (0x1f, [5.0, 5.0, 5.0, 5.0]),
        (0x80, [0.0, 0.0, 0.0, 0.0]),
        (0xf0, [0.0, 0.0, 0.0, 0.0]),
        (0x4c, [0.0, 0.0, 21.0, 21.0]),
    ] {
        assert_reg(
            &reg_code(Kind::Dpps, 1, 2, imm),
            &[(1, a), (2, b)],
            MXCSR,
            ps(result.map(s)),
            MXCSR,
        );
    }
    // A single exact product plus zeros: no flag. (1/3)^2 + 1/3 is inexact in binary32: PE (hardware agrees).
    assert_reg(
        &reg_code(Kind::Dpps, 1, 2, 0x11),
        &[(1, ps([0x3eaa_aaab, 0, 0, 0])), (2, ps([s(1.0), 0, 0, 0]))],
        MXCSR,
        ps([0x3eaa_aaab, 0, 0, 0]),
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Dpps, 1, 2, 0x31),
        &[
            (1, ps([0x3eaa_aaab, 0x3eaa_aaab, 0, 0])),
            (2, ps([0x3eaa_aaab, s(1.0), 0, 0])),
        ],
        MXCSR,
        ps([0x3ee3_8e3a, 0, 0, 0]),
        MXCSR | MXCSR_PE,
    );
}

#[test]
fn dppd_multiplies_adds_and_broadcasts_for_the_two_elements() {
    let a = pd([d(1.5), d(2.5)]);
    let b = pd([d(2.0), d(4.0)]);
    for (imm, result) in [
        (0x33u8, [13.0f64, 13.0]),
        (0x31, [13.0, 0.0]),
        (0x32, [0.0, 13.0]),
        (0x13, [3.0, 3.0]),
        (0x12, [0.0, 3.0]),
        (0x23, [10.0, 10.0]),
        (0x30, [0.0, 0.0]),
        (0xf3, [13.0, 13.0]),
    ] {
        assert_reg(
            &reg_code(Kind::Dppd, 1, 2, imm),
            &[(1, a), (2, b)],
            MXCSR,
            pd(result.map(d)),
            MXCSR,
        );
    }
}

#[test]
fn an_unselected_product_is_plus_zero_and_signals_nothing_even_for_a_signaling_nan_or_a_denormal() {
    // imm8[7:4] = 0111: element 3 is not multiplied.
    let snan = 0x7fa0_0000u32;
    assert_reg(
        &reg_code(Kind::Dpps, 1, 2, 0x71),
        &[
            (1, ps([s(1.0), s(1.0), s(1.0), snan])),
            (2, ps([s(2.0), s(3.0), s(4.0), 0x0000_0001])),
        ],
        MXCSR,
        ps([s(9.0), 0, 0, 0]),
        MXCSR,
    );
    // A selected signaling NaN: IE, the result is the quiet NaN.
    assert_reg(
        &reg_code(Kind::Dpps, 1, 2, 0x11),
        &[(1, ps([snan, 0, 0, 0])), (2, ps([s(1.0), 0, 0, 0]))],
        MXCSR,
        ps([0x7fe0_0000, 0, 0, 0]),
        MXCSR | MXCSR_IE,
    );
    // Unmasked IM: #XM with IE only, destination unchanged.
    assert_xm(
        &reg_code(Kind::Dpps, 1, 2, 0x11),
        &[(1, ps([snan, 0, 0, 0])), (2, ps([s(1.0), 0, 0, 0]))],
        MXCSR & !MXCSR_IM,
        (MXCSR & !MXCSR_IM) | MXCSR_IE,
    );
}

#[test]
fn dpps_nan_selection_follows_the_hardware_low_pair_first_high_pair_swapped() {
    // Hardware (oracle case dm_dpps_0_7_0): products p0 = 0, p1 = QNaN(7fc00001, quieted from the signaling NaN
    // 7f800001), p2 = 00800001, p3 = QNaN ffc12345; pair sums (p1 + p0) = 7fc00001 and (p3 + p2) = ffc12345;
    // the low elements take (pair01 + pair23) = 7fc00001, the high ones (pair23 + pair01) = ffc12345.
    assert_reg(
        &reg_code(Kind::Dpps, 1, 2, 0xff),
        &[
            (1, ps([0, 0x807f_ffff, s(1.0), 0xffc1_2345])),
            (2, ps([s(0.5), 0x7f80_0001, 0x0080_0001, 0x4b7f_ffff])),
        ],
        MXCSR,
        ps([0x7fc0_0001, 0x7fc0_0001, 0xffc1_2345, 0xffc1_2345]),
        MXCSR | MXCSR_IE,
    );
    // Hardware (dm_dppd): DPPD adds (t0, t1) for element 0 and (t1, t0) for element 1.
    assert_reg(
        &reg_code(Kind::Dppd, 1, 2, 0x33),
        &[
            (1, pd([d(1.0), 0x7ff8_0000_0000_0002])),
            (2, pd([0x7ff8_0000_0000_0001, d(1.0)])),
        ],
        MXCSR,
        pd([0x7ff8_0000_0000_0001, 0x7ff8_0000_0000_0002]),
        MXCSR,
    );
}

#[test]
fn dpps_gathers_the_pre_computation_flags_of_all_multiplies_and_reports_them_alone_when_unmasked() {
    // p1 = 0.5 * sNaN (IE) and p2 = 1e20 * denormal (DE); with everything unmasked the hardware reports IE|DE
    // and no post-computation flag.
    assert_xm(
        &reg_code(Kind::Dpps, 1, 2, 0xff),
        &[
            (1, ps([0, s(0.5), 0x4b7f_ffff, 0x7f80_0000])),
            (2, ps([s(2.0), 0x7fbf_ffff, 0x8000_0001, 0x3f7f_ffff])),
        ],
        0,
        MXCSR_IE | MXCSR_DE,
    );
    // An unmasked overflow stops before the adds: OE (and PE: the unbounded result is inexact) only.
    let observed = run_regs(
        &reg_code(Kind::Dpps, 1, 2, 0xff),
        &[
            (1, ps([s(3.0e38), s(1.0), s(1.0), s(1.0)])),
            (2, ps([s(3.0e38), s(1.0), s(1.0), s(1.0)])),
        ],
        MXCSR & !MXCSR_OM,
    );
    assert_eq!(observed.exit, xm_exit());
    assert_eq!(mxcsr(&observed.after) & 0x3f, MXCSR_OE | MXCSR_PE);
    assert_eq!(xmm(&observed.after, 1), xmm(&observed.before, 1));
}

// ---- BLENDPS / BLENDPD / BLENDVPS / BLENDVPD ----

#[test]
fn blendps_and_blendpd_take_an_element_from_the_source_when_its_imm8_bit_is_set() {
    let a = ps([1, 2, 3, 4]);
    let b = ps([10, 20, 30, 40]);
    assert_reg(
        &reg_code(Kind::Blendps, 1, 2, 0b0101),
        &[(1, a), (2, b)],
        MXCSR,
        ps([10, 2, 30, 4]),
        MXCSR,
    );
    // imm8 bits 7:4 are ignored.
    assert_reg(
        &reg_code(Kind::Blendps, 1, 2, 0xf5),
        &[(1, a), (2, b)],
        MXCSR,
        ps([10, 2, 30, 4]),
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Blendps, 1, 2, 0xf0),
        &[(1, a), (2, b)],
        MXCSR,
        a,
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Blendps, 1, 2, 0x0f),
        &[(1, a), (2, b)],
        MXCSR,
        b,
        MXCSR,
    );
    let a = pd([1, 2]);
    let b = pd([10, 20]);
    assert_reg(
        &reg_code(Kind::Blendpd, 1, 2, 0b10),
        &[(1, a), (2, b)],
        MXCSR,
        pd([1, 20]),
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Blendpd, 1, 2, 0xfd),
        &[(1, a), (2, b)],
        MXCSR,
        pd([10, 2]),
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Blendpd, 1, 2, 0xfc),
        &[(1, a), (2, b)],
        MXCSR,
        a,
        MXCSR,
    );
}

#[test]
fn blendvps_and_blendvpd_select_by_the_top_bit_of_each_xmm0_element() {
    let a = ps([1, 2, 3, 4]);
    let b = ps([10, 20, 30, 40]);
    let mask = ps([0x8000_0000, 0x7fff_ffff, 0xffff_ffff, 0]);
    assert_reg(
        &reg_code(Kind::Blendvps, 1, 2, 0),
        &[(0, mask), (1, a), (2, b)],
        MXCSR,
        ps([10, 2, 30, 4]),
        MXCSR,
    );
    let a = pd([1, 2]);
    let b = pd([10, 20]);
    let mask = pd([0x7fff_ffff_ffff_ffff, 1 << 63]);
    assert_reg(
        &reg_code(Kind::Blendvpd, 1, 2, 0),
        &[(0, mask), (1, a), (2, b)],
        MXCSR,
        pd([1, 20]),
        MXCSR,
    );
    // The destination xmm0 is also the mask: BLENDVPS xmm0, xmm2 selects by its own old top bits.
    let observed = run_regs(
        &reg_code(Kind::Blendvps, 0, 2, 0),
        &[
            (0, ps([0x8000_0001, 5, 0xffff_ffff, 7])),
            (2, ps([100, 200, 300, 400])),
        ],
        MXCSR,
    );
    check_observed(
        &observed,
        0,
        Expect::Ok(ps([100, 5, 300, 7]), MXCSR),
        "blendvps xmm0",
    );
}

#[test]
fn the_blends_copy_signaling_nans_and_signal_nothing_with_every_exception_unmasked() {
    let snan = ps([0x7f80_0001, 0x7fa0_0000, 0xff80_0001, 0x0000_0001]);
    for code in [
        reg_code(Kind::Blendps, 1, 2, 0x0f),
        reg_code(Kind::Blendvps, 1, 2, 0),
    ] {
        assert_reg(
            &code,
            &[(0, ps([u32::MAX; 4])), (1, 0), (2, snan)],
            0,
            snan,
            0,
        );
    }
}

// ---- INSERTPS ----

#[test]
fn insertps_copies_one_element_of_a_register_source_to_the_selected_element_then_zeroes_by_the_mask()
 {
    let a = ps([1, 2, 3, 4]);
    let b = ps([10, 20, 30, 40]);
    // Count_S = 2 (element 2 of the source), Count_D = 1, ZMask = 0b1000.
    assert_reg(
        &reg_code(Kind::Insertps, 1, 2, 2 << 6 | 1 << 4 | 0b1000),
        &[(1, a), (2, b)],
        MXCSR,
        ps([1, 30, 3, 0]),
        MXCSR,
    );
    // Count_S = 3 -> element 0 of the destination, no mask.
    assert_reg(
        &reg_code(Kind::Insertps, 1, 2, 3 << 6),
        &[(1, a), (2, b)],
        MXCSR,
        ps([40, 2, 3, 4]),
        MXCSR,
    );
    // The mask also zeroes the inserted element.
    assert_reg(
        &reg_code(Kind::Insertps, 1, 2, 1 << 4 | 0b0010),
        &[(1, a), (2, b)],
        MXCSR,
        ps([1, 0, 3, 4]),
        MXCSR,
    );
    assert_reg(
        &reg_code(Kind::Insertps, 1, 2, 0x0f),
        &[(1, a), (2, b)],
        MXCSR,
        0,
        MXCSR,
    );
    // Source and destination are the same register: element 3 -> element 0.
    let observed = run_regs(&reg_code(Kind::Insertps, 1, 1, 3 << 6), &[(1, a)], MXCSR);
    check_observed(
        &observed,
        1,
        Expect::Ok(ps([4, 2, 3, 4]), MXCSR),
        "insertps xmm1, xmm1",
    );
    // A signaling NaN is copied unchanged, no flag, no fault even with every exception unmasked.
    assert_reg(
        &reg_code(Kind::Insertps, 1, 2, 3 << 6 | 2 << 4),
        &[(1, a), (2, ps([0, 0, 0, 0x7f80_0001]))],
        0,
        ps([1, 2, 0x7f80_0001, 4]),
        0,
    );
}

#[test]
fn insertps_with_a_memory_source_ignores_count_s_reads_four_bytes_and_needs_no_alignment() {
    let a = ps([1, 2, 3, 4]);
    let (outcome, before, after, ram) = exec(
        &mem_code(Kind::Insertps, 1, 3 << 6 | 2 << 4 | 0b0001),
        PrivilegeLevel::Ring0,
        |cpu, ram| {
            cpu.gpr[Register::Rax as usize] = DATA + 1;
            set_xmm(cpu, 1, a);
            ram.put(DATA + 1, &[0x11, 0x22, 0x33, 0x44, 0x55]);
        },
    );
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    let mut expected = before.clone();
    set_xmm(&mut expected, 1, ps([0, 2, 0x4433_2211, 4]));
    expected.rip += 6;
    assert_eq!(after, expected);
    assert_eq!(ram.reads, vec![(DATA + 1, 4)]);
}

// ---- EXTRACTPS ----

#[test]
fn extractps_zero_extends_the_selected_element_into_a_64_bit_register_whatever_rex_w() {
    let source = ps([0x1111_1111, 0x8222_2222, 0x8333_3333, 0x4444_4444]);
    for (imm, expected) in [
        (0u8, 0x1111_1111u64),
        (1, 0x8222_2222),
        (2, 0x8333_3333),
        (3, 0x4444_4444),
        (0xfe, 0x8333_3333),
        (0xfc, 0x1111_1111),
    ] {
        for rex_w in [false, true] {
            // EXTRACTPS rcx, xmm9, imm8 (REX.R for the source) and r9 as the destination (REX.B).
            for (rm, register) in [(1u8, Register::Rcx), (9, Register::R9)] {
                let code = reg_code_rex(Kind::Extractps, 9, rm, imm, rex_w);
                let observed = run_regs(&code, &[(9, source)], 0);
                let mut expected_state = observed.before.clone();
                expected_state.gpr[register as usize] = expected;
                expected_state.rip += code.len() as u64;
                assert_eq!(observed.exit, RunExit::InstructionBudgetExhausted);
                assert_eq!(observed.after, expected_state, "{code:02x?}");
            }
        }
    }
}

#[test]
fn extractps_to_memory_writes_exactly_four_bytes_with_no_alignment_requirement() {
    let source = ps([1, 0xa1a2_a3a4, 3, 4]);
    let (outcome, before, after, ram) = exec(
        &mem_code(Kind::Extractps, 1, 1),
        PrivilegeLevel::Ring0,
        |cpu, _| {
            cpu.gpr[Register::Rax as usize] = DATA + 3;
            set_xmm(cpu, 1, source);
        },
    );
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    let mut expected = before.clone();
    expected.rip += 6;
    assert_eq!(after, expected);
    assert_eq!(ram.writes, vec![(DATA + 3, 4)]);
    assert_eq!(
        &ram.bytes[DATA as usize + 2..DATA as usize + 8],
        &[SENTINEL, 0xa4, 0xa3, 0xa2, 0xa1, SENTINEL]
    );
    assert!(ram.reads.is_empty());
}

#[test]
fn extractps_to_memory_faults_leave_the_state_and_memory_untouched() {
    let code = mem_code(Kind::Extractps, 1, 2);
    // A backend #PF on the store.
    let (outcome, before, after, ram) = exec(&code, PrivilegeLevel::Ring0, |cpu, ram| {
        cpu.gpr[Register::Rax as usize] = DATA + 0xffe;
        ram.fault_page = Some(DATA + 0x1000);
    });
    assert!(matches!(
        outcome,
        Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
    ));
    assert_eq!(after, before);
    assert!(
        ram.bytes
            .iter()
            .skip(DATA as usize)
            .take(0x1100)
            .all(|byte| *byte == SENTINEL)
    );
    // A non-canonical address is #GP(0) before any memory access.
    let (outcome, before, after, ram) = exec(&code, PrivilegeLevel::Ring0, |cpu, _| {
        cpu.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
    });
    assert_eq!(outcome, Ok(gp0()));
    assert_eq!(after, before);
    assert!(ram.writes.is_empty());
    // An RBP base is #SS(0).
    let rbp_code = [0x66, 0x0f, 0x3a, 0x17, 0x4d, 0x00, 0x02];
    let (outcome, before, after, ram) = exec(&rbp_code, PrivilegeLevel::Ring0, |cpu, _| {
        cpu.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000;
    });
    assert_eq!(outcome, Ok(ss0()));
    assert_eq!(after, before);
    assert!(ram.writes.is_empty());
}

// ---- memory operand rules ----

#[test]
fn a_16_byte_memory_operand_that_is_not_16_byte_aligned_is_gp0_for_types_2_and_4_only() {
    for (kind, name, _, _, bytes, aligned) in KINDS {
        if kind == Kind::Extractps {
            continue;
        }
        for offset in [1u64, 4, 8, 15] {
            let (outcome, before, after, ram) = exec(
                &mem_code(kind, 1, 3),
                PrivilegeLevel::Ring0,
                rax_at(DATA + offset),
            );
            if aligned {
                assert_eq!(outcome, Ok(gp0()), "{name} +{offset}");
                assert_eq!(after, before, "{name}");
                assert!(ram.reads.is_empty(), "{name}");
            } else {
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} +{offset}");
                assert_eq!(ram.reads, vec![(DATA + offset, bytes)], "{name}");
            }
        }
        // Aligned operands work and read exactly the operand width.
        let (outcome, _, _, ram) = exec(
            &mem_code(kind, 1, 3),
            PrivilegeLevel::Ring0,
            rax_at(DATA + 0x10),
        );
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
        assert_eq!(ram.reads, vec![(DATA + 0x10, bytes)], "{name}");
    }
}

#[test]
fn a_memory_source_gives_the_same_result_as_the_register_source() {
    let value = [0x3fc0_0000u32, 0x4040_0000, 0xc0a0_0000, 0x7f80_0000];
    for (kind, name, _, has_immediate, bytes, _) in KINDS {
        if kind == Kind::Extractps {
            continue;
        }
        let imm = if has_immediate { 0x40 } else { 0 };
        let register = run_regs(
            &reg_code(kind, 1, 2, imm),
            &[
                (0, ps([0x8000_0000, 0, 0xffff_ffff, 0])),
                (1, ps([1, 2, 3, 4])),
                (2, ps(value)),
            ],
            MXCSR,
        );
        let mut cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT);
        set_xmm(&mut cpu, 0, ps([0x8000_0000, 0, 0xffff_ffff, 0]));
        set_xmm(&mut cpu, 1, ps([1, 2, 3, 4]));
        cpu.gpr[Register::Rax as usize] = DATA + 0x20;
        let memory = run_with(&mem_code(kind, 1, imm), cpu, |ram| {
            let mut raw = [0u8; 16];
            raw[..].copy_from_slice(&ps(value).to_le_bytes());
            ram.put(DATA + 0x20, &raw[..bytes]);
        });
        assert_eq!(memory.exit, RunExit::InstructionBudgetExhausted, "{name}");
        if kind == Kind::Insertps {
            // The memory form has no Count_S: the same as Count_S = 0 of the register form.
            continue;
        }
        assert_eq!(xmm(&memory.after, 1), xmm(&register.after, 1), "{name}");
        assert_eq!(mxcsr(&memory.after), mxcsr(&register.after), "{name}");
    }
}

#[test]
fn memory_faults_and_bad_addresses_leave_the_state_untouched() {
    for (kind, name, _, _, bytes, aligned) in KINDS {
        if kind == Kind::Extractps {
            continue;
        }
        let code = mem_code(kind, 1, 3);
        // A non-canonical address is #GP(0) and wins over the alignment check.
        let (outcome, before, after, ram) =
            exec(&code, PrivilegeLevel::Ring0, rax_at(0x0000_8000_0000_0008));
        assert_eq!(outcome, Ok(gp0()), "{name}");
        assert_eq!(after, before, "{name}");
        assert!(ram.reads.is_empty(), "{name}");
        // An RBP base is #SS(0): xmm1, [rbp+0] (modrm 01 001 101, disp8 0).
        let mut rbp = code.clone();
        rbp[4] = 0x4d;
        rbp.insert(5, 0x00);
        let (outcome, before, after, _) = exec(&rbp, PrivilegeLevel::Ring0, |cpu, _| {
            cpu.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000;
        });
        assert_eq!(outcome, Ok(ss0()), "{name}");
        assert_eq!(after, before, "{name}");
        // The first byte is canonical but the last one is not: #GP(0) before memory (unaligned forms).
        if !aligned {
            let start = 0x0000_7fff_ffff_ffff - (bytes as u64 - 2);
            let (outcome, before, after, ram) = exec(&code, PrivilegeLevel::Ring0, rax_at(start));
            assert_eq!(outcome, Ok(gp0()), "{name}");
            assert_eq!(after, before, "{name}");
            assert!(ram.reads.is_empty(), "{name}");
        }
        // A backend #PF on the read leaves everything alone.
        let (outcome, before, after, _) = exec(&code, PrivilegeLevel::Ring0, |cpu, ram| {
            cpu.gpr[Register::Rax as usize] = DATA + 0x1000;
            ram.fault_page = Some(DATA + 0x1000);
        });
        assert!(
            matches!(
                outcome,
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
            ),
            "{name}: {outcome:?}"
        );
        assert_eq!(after, before, "{name}");
    }
}

#[test]
fn at_cpl3_with_the_alignment_check_armed_aligned_operands_run_and_unaligned_type_3_and_5_are_alignment_check()
 {
    for (kind, name, _, _, bytes, aligned) in KINDS {
        let code = mem_code(kind, 1, 3);
        let armed = |address: u64| {
            move |cpu: &mut LongModeState, _: &mut Ram| {
                cpu.rflags |= 1 << 18;
                cpu.gpr[Register::Rax as usize] = address;
            }
        };
        let (outcome, _, _, _) = exec(&code, PrivilegeLevel::Ring3, armed(DATA + 0x40));
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} aligned");
        if !aligned {
            let (outcome, before, after, ram) =
                exec(&code, PrivilegeLevel::Ring3, armed(DATA + 0x41));
            // ADR-0023 C1 (decision 4): #AC(0) for the scalar (Type 3/5) operands.
            assert_eq!(
                outcome,
                Ok(StepOutcome::Fault(od_isa::ArchitecturalFault {
                    vector: 17,
                    error_code: Some(0),
                    linear_address: None
                })),
                "{name} unaligned {bytes}"
            );
            assert_eq!(after, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        } else {
            let (outcome, _, _, _) = exec(&code, PrivilegeLevel::Ring3, armed(DATA + 0x48));
            assert_eq!(outcome, Ok(gp0()), "{name}");
        }
    }
}

// ---- control registers, MXCSR and the SSE state ----

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_before_any_address_or_memory_check() {
    for (kind, name, _, has_immediate, _, _) in KINDS {
        let imm = if has_immediate { 3 } else { 0 };
        let codes = [reg_code(kind, 1, 2, imm), mem_code(kind, 1, imm)];
        for code in codes {
            for (cr0, cr4, expected) in [
                (
                    CR0_BASE | 1 << 2,
                    CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 6,
                        error_code: None,
                        linear_address: None,
                    }),
                ),
                (
                    CR0_BASE,
                    CR4_PAE | CR4_OSXMMEXCPT,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 6,
                        error_code: None,
                        linear_address: None,
                    }),
                ),
                (
                    CR0_BASE | 1 << 3,
                    CR4_PAE | CR4_OSFXSR | CR4_OSXMMEXCPT,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 7,
                        error_code: None,
                        linear_address: None,
                    }),
                ),
                // EM and TS together: #UD wins.
                (
                    CR0_BASE | 1 << 2 | 1 << 3,
                    CR4_PAE | CR4_OSFXSR,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 6,
                        error_code: None,
                        linear_address: None,
                    }),
                ),
            ] {
                let mut cpu = fp_init_cpu(cr0, cr4);
                // A non-canonical address would be #GP(0) if the gate did not come first.
                cpu.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
                let observed = run_with(&code, cpu, |_| {});
                assert_eq!(
                    observed.exit, expected,
                    "{name} {code:02x?} cr0 {cr0:#x} cr4 {cr4:#x}"
                );
                assert_eq!(observed.after, observed.before, "{name}");
                assert!(observed.ram.reads.is_empty() && observed.ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn an_unmasked_exception_raises_ud_without_cr4_osxmmexcpt_and_is_refused_without_cr4() {
    // CR4.OSXMMEXCPT = 0: #UD (vector 6), the flags are still set in MXCSR, the destination is unchanged.
    let code = reg_code(Kind::Roundsd, 1, 2, 0);
    let mut cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR);
    cpu.sse.as_mut().unwrap().mxcsr = MXCSR & !MXCSR_PM;
    set_xmm(&mut cpu, 2, pd([d(2.5), 0]));
    let observed = run_with(&code, cpu, |_| {});
    assert_eq!(
        observed.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 6,
            error_code: None,
            linear_address: None
        })
    );
    let mut expected = observed.before.clone();
    expected.sse.as_mut().unwrap().mxcsr = (MXCSR & !MXCSR_PM) | MXCSR_PE;
    assert_eq!(observed.after, expected);
    // Without CR4 in the state the vector is not known: Unsupported with nothing changed.
    let (outcome, before, after, _) = exec(&code, PrivilegeLevel::Ring0, |cpu, _| {
        cpu.sse.as_mut().unwrap().mxcsr = MXCSR & !MXCSR_PM;
        set_xmm(cpu, 2, pd([d(2.5), 0]));
    });
    assert_eq!(outcome, Ok(StepOutcome::Unsupported { opcode: 0x0b }));
    assert_eq!(after, before);
}

#[test]
fn reserved_mxcsr_bits_refuse_the_arithmetic_forms_only() {
    let reserved = MXCSR | 0x1_0000;
    for (kind, name, _, has_immediate, _, _) in KINDS {
        let code = reg_code(kind, 1, 2, if has_immediate { 3 } else { 0 });
        let (outcome, before, after, _) = exec(&code, PrivilegeLevel::Ring0, |cpu, _| {
            cpu.sse.as_mut().unwrap().mxcsr = reserved;
        });
        let arithmetic = matches!(
            kind,
            Kind::Roundps | Kind::Roundpd | Kind::Roundss | Kind::Roundsd | Kind::Dpps | Kind::Dppd
        );
        if arithmetic {
            assert!(
                matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
                "{name}"
            );
            assert_eq!(after, before);
        } else {
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
            assert_eq!(mxcsr(&after), reserved, "{name}");
        }
    }
}

#[test]
fn without_sse_state_every_form_stops_as_unsupported_and_x87_state_is_not_needed() {
    for (kind, name, opcode, has_immediate, _, _) in KINDS {
        let imm = if has_immediate { 3 } else { 0 };
        for code in [reg_code(kind, 1, 2, imm), mem_code(kind, 1, imm)] {
            let (outcome, before, after, ram) = exec(&code, PrivilegeLevel::Ring0, |cpu, _| {
                cpu.sse = None;
                cpu.gpr[Register::Rax as usize] = DATA;
            });
            assert_eq!(
                outcome,
                Ok(StepOutcome::Unsupported { opcode: opcode[2] }),
                "{name}"
            );
            assert_eq!(after, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            let (outcome, _, _, _) = exec(&code, PrivilegeLevel::Ring0, |cpu, _| {
                cpu.x87 = None;
                cpu.gpr[Register::Rax as usize] = DATA;
            });
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
        }
    }
}

#[test]
fn only_the_destination_changes_and_flags_and_general_registers_are_untouched() {
    // Every register pair of the register forms: the destination register (or the general register of
    // EXTRACTPS), RIP and, for the arithmetic forms, MXCSR change; nothing else does.
    for (kind, name, _, has_immediate, _, _) in KINDS {
        for destination in 0..16u8 {
            for source in [0u8, 5, 15] {
                let code = if kind == Kind::Extractps {
                    reg_code(kind, source, destination, 1)
                } else {
                    reg_code(
                        kind,
                        destination,
                        source,
                        if has_immediate { 0x5a } else { 0 },
                    )
                };
                let observed = run_regs(&code, &[], MXCSR);
                assert_eq!(
                    observed.exit,
                    RunExit::InstructionBudgetExhausted,
                    "{name} {destination} {source}"
                );
                let (before, after) = (&observed.before, &observed.after);
                assert_eq!(after.rflags, before.rflags, "{name}");
                assert_eq!(after.x87, before.x87, "{name}");
                for index in 0..16 {
                    let changes_gpr = kind == Kind::Extractps && index == usize::from(destination);
                    if !changes_gpr {
                        assert_eq!(after.gpr[index], before.gpr[index], "{name} gpr {index}");
                    }
                    let changes_xmm = kind != Kind::Extractps && index == usize::from(destination);
                    if !changes_xmm {
                        assert_eq!(xmm(after, index), xmm(before, index), "{name} xmm {index}");
                    }
                }
                assert_eq!(after.rip, before.rip + code.len() as u64);
            }
        }
    }
}

/// Hardware vectors of the S2-55 slice B oracle: name, instruction bytes, MXCSR before, the XMM registers that
/// the case sets (the others keep the test pattern), and the destination plus MXCSR afterwards or the #XM MXCSR.
#[rustfmt::skip]
const GOLDEN: &[(&str, &[u8], u32, &[(usize, u128)], Expect)] = &[
// 216 hardware vectors (S2-55 slice B oracle, work/stage8/s2_55b_oracle/pick_golden.py)
    ("bi_blendpd_144_1", &[0x66, 0x0f, 0x3a, 0x0d, 0xf6, 0x90], 0x1f00, &[(6, 0x3ff8000000000000ae83fee817bd9130)], Expect::Ok(0x3ff8000000000000ae83fee817bd9130, 0x1f00)),
    ("bi_blendpd_164_1", &[0x66, 0x44, 0x0f, 0x3a, 0x0d, 0xd2, 0xa4], 0x1bc0, &[(2, 0x3fe00000000000003fd0000000000000), (10, 0x2ab198f1359846a43ff8000000000000)], Expect::Ok(0x2ab198f1359846a43ff8000000000000, 0x1bc0)),
    ("bi_blendpd_200_1", &[0x66, 0x45, 0x0f, 0x3a, 0x0d, 0xdf, 0xc8], 0x7dc0, &[(11, 0xbff00000000000004330000000000000), (15, 0x00200000000000003cbde5f98f3280b1)], Expect::Ok(0xbff00000000000004330000000000000, 0x7dc0)),
    ("bi_blendpd_213_2", &[0x66, 0x45, 0x0f, 0x3a, 0x0d, 0xdc, 0xd5], 0x17c0, &[(11, 0xbca65f689045ae22c330000000000001), (12, 0xbfefffffffffffff3ff8000000000000)], Expect::Ok(0xbca65f689045ae223ff8000000000000, 0x17c0)),
    ("bi_blendpd_253_2", &[0x66, 0x44, 0x0f, 0x3a, 0x0d, 0xde, 0xfd], 0x8f80, &[(6, 0xfff7ffffffffffffbca57ea14f62ac7e), (11, 0xbfe00000000000000006e78830dcc5a4)], Expect::Ok(0xbfe0000000000000bca57ea14f62ac7e, 0x8f80)),
    ("bi_blendpd_62_3", &[0x66, 0x44, 0x0f, 0x3a, 0x0d, 0xe1, 0x3e], 0x9e80, &[(1, 0x00000000000000007ff8000000000001), (12, 0x7fe00000000000004008000000000000)], Expect::Ok(0x00000000000000004008000000000000, 0x9e80)),
    ("bi_blendps_126_0", &[0x66, 0x45, 0x0f, 0x3a, 0x0c, 0xd5, 0x7e], 0x1f00, &[(10, 0x0000000180f5b83e0000000000000003), (13, 0x3014a4643007fbf5cb7c330c4feb77ca)], Expect::Ok(0x3014a4643007fbf5cb7c330c00000003, 0x1f00)),
    ("bi_blendps_130_2", &[0x66, 0x44, 0x0f, 0x3a, 0x0c, 0xdf, 0x82], 0x40, &[(7, 0x4c14879a00361822005759f734000000), (11, 0x3043aa06a6bd05ca804026d374eeefa8)], Expect::Ok(0x3043aa06a6bd05ca005759f774eeefa8, 0x40)),
    ("bi_blendps_133_2", &[0x66, 0x45, 0x0f, 0x3a, 0x0c, 0xc9, 0x85], 0x5faa, &[(9, 0x7f800000ff7fffff0ee2b2f97fbfffff)], Expect::Ok(0x7f800000ff7fffff0ee2b2f97fbfffff, 0x5faa)),
    ("bi_blendps_144_3", &[0x66, 0x45, 0x0f, 0x3a, 0x0c, 0xf2, 0x90], 0x1f95, &[(10, 0xcff3dd55c2f6e97980e50907ff7fffff), (14, 0x3f000000bf7fffff3f7fffff7f000000)], Expect::Ok(0x3f000000bf7fffff3f7fffff7f000000, 0x1f95)),
    ("bi_blendps_167_1", &[0x66, 0x41, 0x0f, 0x3a, 0x0c, 0xdb, 0xa7], 0x3f80, &[(3, 0xdebf31cc007fffff815f848700800000), (11, 0x3f7fffff800000010000000100812d24)], Expect::Ok(0xdebf31cc800000010000000100812d24, 0x3f80)),
    ("bi_blendps_18_3", &[0x66, 0x45, 0x0f, 0x3a, 0x0c, 0xc6, 0x12], 0x6fc0, &[(8, 0x3fc00000c0200000bfc00000ffc0d6e9), (14, 0x7fbfffff3fc0000080000001cb47eca8)], Expect::Ok(0x3fc00000c020000080000001ffc0d6e9, 0x6fc0)),
    ("bi_blendps_197_3", &[0x66, 0x41, 0x0f, 0x3a, 0x0c, 0xd8, 0xc5], 0x7ec0, &[(3, 0x8046d8947e8000007fd94ef94b000000), (8, 0xcadbdad97fa0000000800001ff800000)], Expect::Ok(0x8046d8947fa000007fd94ef9ff800000, 0x7ec0)),
    ("bi_blendps_205_1", &[0x66, 0x45, 0x0f, 0x3a, 0x0c, 0xed, 0xcd], 0x7f80, &[(13, 0x808000007e800000814b45143fc00000)], Expect::Ok(0x808000007e800000814b45143fc00000, 0x7f80)),
    ("bi_blendps_49_3", &[0x66, 0x44, 0x0f, 0x3a, 0x0c, 0xd1, 0x31], 0xfbf, &[(1, 0xff800001bf8000003f80000000000000), (10, 0xc00000004b800000007fffff8155f1b5)], Expect::Ok(0xc00000004b800000007fffff00000000, 0xfbf)),
    ("bi_blendps_72_3", &[0x66, 0x41, 0x0f, 0x3a, 0x0c, 0xdd, 0x48], 0x6000, &[(3, 0x7fa0000033800000ffc00000bf7fffff), (13, 0x000000017e8000004040000000000001)], Expect::Ok(0x0000000133800000ffc00000bf7fffff, 0x6000)),
    ("bi_blendps_99_3", &[0x66, 0x44, 0x0f, 0x3a, 0x0c, 0xda, 0x63], 0x3d80, &[(2, 0x3043db04be8000003effffffc0000000), (11, 0xbe800000f4e0b7310000000140000000)], Expect::Ok(0xbe800000f4e0b7313effffffc0000000, 0x3d80)),
    ("bi_insertps_101_3", &[0x66, 0x45, 0x0f, 0x3a, 0x21, 0xca, 0x65], 0x9f80, &[(9, 0x7fc00001800cfd0dc2f6e9793e800000), (10, 0x00493a25cbfa0377bfc000004b800000)], Expect::Ok(0x7fc0000100000000c2f6e97900000000, 0x9f80)),
    ("bi_insertps_150_3", &[0x66, 0x45, 0x0f, 0x3a, 0x21, 0xc2, 0x96], 0x1f00, &[(8, 0xbf000000808000007effffff7f000000), (10, 0x300fa3c04bade44e008000017f000000)], Expect::Ok(0xbf00000000000000000000007f000000, 0x1f00)),
    ("bi_insertps_178_1", &[0x66, 0x41, 0x0f, 0x3a, 0x21, 0xda, 0xb2], 0x5f00, &[(3, 0x400000003f7fffff340000003fc00000), (10, 0x3fa69915812b9660b03bad6b7f344cdf)], Expect::Ok(0x812b96603f7fffff000000003fc00000, 0x5f00)),
    ("bi_insertps_178_3", &[0x66, 0x41, 0x0f, 0x3a, 0x21, 0xf7, 0xb2], 0x5f00, &[(6, 0x007fffff4ac99990ff2e0e38f49d2cbe), (15, 0x4b8000007f7fffffcb27cd4b00800000)], Expect::Ok(0x7f7fffff4ac9999000000000f49d2cbe, 0x5f00)),
    ("bi_insertps_190_0", &[0x66, 0x44, 0x0f, 0x3a, 0x21, 0xe7, 0xbe], 0x77c0, &[(7, 0x4ac43bcabf000000cb0000013f800000), (12, 0xc0000000bf000000ff7fffff40400000)], Expect::Ok(0x00000000000000000000000040400000, 0x77c0)),
    ("bi_insertps_245_2", &[0x66, 0x44, 0x0f, 0x3a, 0x21, 0xd9, 0xf5], 0xffc0, &[(1, 0x40000000800000003f7fffff4b68f563), (11, 0xbe80000040400000ff00fd2d4b000001)], Expect::Ok(0x4000000000000000ff00fd2d00000000, 0xffc0)),
    ("bv_blendvpd_x0_x6_2364", &[0x66, 0x0f, 0x38, 0x15, 0xc6], 0x2f80, &[(0, 0xffffffffffffffffffffffffffffffff), (6, 0x4330000000000000c0093ac359d06cd6)], Expect::Ok(0x4330000000000000c0093ac359d06cd6, 0x2f80)),
    ("bv_blendvpd_x10_x15_430", &[0x66, 0x45, 0x0f, 0x38, 0x15, 0xd7], 0x1f80, &[(0, 0xb3d4cc9de73a06650000000000000000), (10, 0x00090935528864514330000000000001), (15, 0x434759133b456ddcbca21c09ea58cd31)], Expect::Ok(0x434759133b456ddc4330000000000001, 0x1f80)),
    ("bv_blendvpd_x12_x8_451", &[0x66, 0x45, 0x0f, 0x38, 0x15, 0xe0], 0x6000, &[(0, 0xffffffffffffffff0000000000000000), (8, 0x0010000000000001ffefffffffffffff), (12, 0x40040000000000007ffad17a7ac7fba1)], Expect::Ok(0x00100000000000017ffad17a7ac7fba1, 0x6000)),
    ("bv_blendvpd_x14_x4_495", &[0x66, 0x44, 0x0f, 0x38, 0x15, 0xf4], 0x17c0, &[(0, 0x80000000000000003ea290c962cc6d20), (4, 0xc32c708a83d4ef540022d03bdac4fb33), (14, 0x80000000000000000000000000000001)], Expect::Ok(0xc32c708a83d4ef540000000000000001, 0x17c0)),
    ("bv_blendvpd_x15_x5_2483", &[0x66, 0x44, 0x0f, 0x38, 0x15, 0xfd], 0x1a00, &[(0, 0x0aae4170ba0dc5bb7fffffffffffffff), (5, 0x001629c2cc5fd70bbff0000000000000), (15, 0x800d3d70ef83382cbcb53316c1605c6d)], Expect::Ok(0x800d3d70ef83382cbcb53316c1605c6d, 0x1a00)),
    ("bv_blendvpd_x1_x15_1079", &[0x66, 0x41, 0x0f, 0x38, 0x15, 0xcf], 0x7e80, &[(0, 0x14f89f5bd0dbaa2b7fffffffffffffff), (1, 0xc0000000000000007fd0000000000000), (15, 0x7ffd24fea815236abffa7d82b2342f3f)], Expect::Ok(0xc0000000000000007fd0000000000000, 0x7e80)),
    ("bv_blendvpd_x1_x1_58", &[0x66, 0x0f, 0x38, 0x15, 0xc9], 0xf80, &[(0, 0x00000000000000000000000000000000), (1, 0xbff00000000000008000000000000001)], Expect::Ok(0xbff00000000000008000000000000001, 0xf80)),
    ("bv_blendvpd_x3_x2_2424", &[0x66, 0x0f, 0x38, 0x15, 0xda], 0x1ec0, &[(0, 0xffffffffffffffffffffffffffffffff), (2, 0x7fdffffffffffffffff8123456789abc), (3, 0x7ff7ffffffffffff7ff0000000000000)], Expect::Ok(0x7fdffffffffffffffff8123456789abc, 0x1ec0)),
    ("bv_blendvpd_x7_x14_862", &[0x66, 0x41, 0x0f, 0x38, 0x15, 0xfe], 0x40, &[(0, 0xc45fcb1d534326000000000000000000), (7, 0x0000000000000000000b84dae4137b1e), (14, 0xbcbde4770f54bf5fbff0000000000000)], Expect::Ok(0xbcbde4770f54bf5f000b84dae4137b1e, 0x40)),
    ("bv_blendvps_x11_x8_1046", &[0x66, 0x45, 0x0f, 0x38, 0x14, 0xd8], 0x8000, &[(0, 0x254785047fffffffffffffffffffffff), (8, 0x7f800000c03eb980c02000003f400000), (11, 0xc0086a8ecb000001400000004aee0365)], Expect::Ok(0xc0086a8ecb000001c02000003f400000, 0x8000)),
    ("bv_blendvps_x12_x3_1523", &[0x66, 0x44, 0x0f, 0x38, 0x14, 0xe3], 0x3f80, &[(0, 0x800000008000000000000000417eaf50), (3, 0xbf0000004073ada880772b8f00400000), (12, 0x0039bca2a2ed73853f7fffff7e800000)], Expect::Ok(0xbf0000004073ada83f7fffff7e800000, 0x3f80)),
    ("bv_blendvps_x14_x0_81", &[0x66, 0x44, 0x0f, 0x38, 0x14, 0xf0], 0x7f40, &[(0, 0xbf000000f4d7077434000000ad993173), (14, 0x3fc000003f8000004b000001cb000001)], Expect::Ok(0xbf000000f4d707744b000001ad993173, 0x7f40)),
    ("bv_blendvps_x15_x13_1939", &[0x66, 0x45, 0x0f, 0x38, 0x14, 0xfd], 0x7bc0, &[(0, 0x8000000028b53b0d5bb0186e7fffffff), (13, 0x7f800000400000000b08d58abf000000), (15, 0x01000000ff1781c97fbfffff3fb37d04)], Expect::Ok(0x7f800000ff1781c97fbfffff3fb37d04, 0x7bc0)),
    ("bv_blendvps_x1_x8_614", &[0x66, 0x41, 0x0f, 0x38, 0x14, 0xc8], 0x2000, &[(0, 0x7fffffff7fffffff80000000e365685e), (1, 0x4ac827b9bf000000ffc123457fffffff), (8, 0xff62c6308000000040785efa4c60de34)], Expect::Ok(0x4ac827b9bf00000040785efa4c60de34, 0x2000)),
    ("bv_blendvps_x3_x0_786", &[0x66, 0x0f, 0x38, 0x14, 0xd8], 0x1b80, &[(0, 0x010000004a8421f001254c094ffbcbfa), (3, 0x3effffff007fffffffc00000bf7fffff)], Expect::Ok(0x3effffff007fffffffc00000bf7fffff, 0x1b80)),
    ("bv_blendvps_x4_x5_1144", &[0x66, 0x0f, 0x38, 0x14, 0xe5], 0x6000, &[(0, 0x9f207790ec4ffe917fffffff00000000), (4, 0x402000004040000001000000ffd14f03), (5, 0x7f800001ff4891ab00ffffffb07cce62)], Expect::Ok(0x7f800001ff4891ab01000000ffd14f03, 0x6000)),
    ("bv_blendvps_x7_x12_798", &[0x66, 0x41, 0x0f, 0x38, 0x14, 0xfc], 0x17c0, &[(0, 0x80000000dba25fde7fffffffffffffff), (7, 0xc0000000807fffff3fc00000ff800000), (12, 0x800000013fc000007effffff7fc00000)], Expect::Ok(0x800000013fc000003fc000007fc00000, 0x17c0)),
    ("dm_dppd_11_2_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x13], 0x0, &[(3, 0x0010000000000001fff0000000000000), (4, 0x7ff00000000000003ff0000000000000)], Expect::Ok(0xfff0000000000000fff0000000000000, 0x0)),
    ("dm_dppd_11_30_0", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0xff], 0x1f80, &[(3, 0xbff0000000000000fff0000000000000), (4, 0x3fe80000000000000010000000000001)], Expect::Ok(0xfff0000000000000fff0000000000000, 0x1f80)),
    ("dm_dppd_1_51_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x33], 0x0, &[(3, 0xc0040000000000008000000000000000), (4, 0x3fe00000000000003fdfffffffffffff)], Expect::Ok(0xbff4000000000000bff4000000000000, 0x0)),
    ("dm_dppd_29_23_2", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x13], 0x9fc0, &[(3, 0x00000000000000018010000000000000), (4, 0x001fffffffffffff8000000000000001)], Expect::Ok(0x00000000000000000000000000000000, 0x9fc0)),
    ("dm_dppd_29_35_2", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x31], 0x9fc0, &[(3, 0x3fd00000000000008010000000000000), (4, 0x80100000000000003fd0000000000000)], Expect::Ok(0x00000000000000008000000000000000, 0x9ff0)),
    ("dm_dppd_2_47_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x32], 0x0, &[(3, 0x40000000000000003ff0000000000000), (4, 0x7fe0000000000000bff8000000000000)], Expect::Fault(0x8)),
    ("dm_dppd_2_55_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0xff], 0x0, &[(3, 0x40140000000000003ff0000000000000), (4, 0x433fffffffffffff4325555555555555)], Expect::Fault(0x20)),
    ("dm_dppd_34_7_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x32], 0x0, &[(3, 0x40000000000000007fd0000000000000), (4, 0x40040000000000003fe0000000000000)], Expect::Fault(0x20)),
    ("dm_dppd_35_10_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x33], 0x0, &[(3, 0x00000000000000013fd0000000000000), (4, 0x40080000000000007ff0000000000000)], Expect::Fault(0x2)),
    ("dm_dppd_36_42_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x32], 0x0, &[(3, 0xfff8123456789abc3cb0000000000000), (4, 0x3ff00000000000010020000000000000)], Expect::Fault(0x10)),
    ("dm_dppd_37_27_0", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x31], 0x1f80, &[(3, 0x40000000000000003ca0000000000000), (4, 0x40000000000000000000000000000003)], Expect::Ok(0x00000000000000004010000000000000, 0x1fb2)),
    ("dm_dppd_47_21_1", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x31], 0x7f80, &[(3, 0x3ca0000000000000bff8000000000000), (4, 0x8000000000000001fff7ffffffffffff)], Expect::Ok(0x0000000000000000ffffffffffffffff, 0x7fb3)),
    ("dm_dppd_48_35_2", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x33], 0x9fc0, &[(3, 0x0000000000000001c004000000000000), (4, 0x80000000000000013fd0000000000000)], Expect::Ok(0xbfe4000000000000bfe4000000000000, 0x9fc0)),
    ("dm_dppd_57_21_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x33], 0x0, &[(3, 0x00000000000000033fe8000000000000), (4, 0x8000000000000001fff7ffffffffffff)], Expect::Fault(0x3)),
    ("dm_dppd_58_1_0", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x32], 0x1f80, &[(3, 0x3ff0000000000000c05edd2f1a9fbe77), (4, 0x3ca00000000000008000000000000000)], Expect::Ok(0x3ca00000000000000000000000000000, 0x1f80)),
    ("dm_dppd_58_55_3", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x13], 0x0, &[(3, 0x0000000000000001c05edd2f1a9fbe77), (4, 0x3fe00000000000004325555555555555)], Expect::Fault(0x20)),
    ("dm_dppd_6_31_1", &[0x66, 0x0f, 0x3a, 0x41, 0xdc, 0x33], 0x7f80, &[(3, 0x3fe80000000000004008000000000000), (4, 0xc0040000000000007fefffffffffffff)], Expect::Ok(0x7feffffffffffffe7feffffffffffffe, 0x7fa8)),
    ("dm_dpps_11_51_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x32], 0x9fc0, &[(3, 0x4b00000033800000c2f6e979ff800000), (4, 0xbf0000007fbfffff340000003effffff)], Expect::Ok(0x0000000000000000ff80000000000000, 0x9fc0)),
    ("dm_dpps_12_44_1", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x71], 0x7f80, &[(3, 0x4b00000040000000800000017fc00000), (4, 0x4b0000017fc000017fffffff40a00000)], Expect::Ok(0x0000000000000000000000007fffffff, 0x7f80)),
    ("dm_dpps_13_43_0", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x71], 0x1f80, &[(3, 0xc00000004b7fffff7fa00000ffc00000), (4, 0xffc123453f400000ff80000100ffffff)], Expect::Ok(0x0000000000000000000000007fe00000, 0x1fa1)),
    ("dm_dpps_13_57_0", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x31], 0x1f80, &[(3, 0x0000000140a00000be800000ffc00000), (4, 0x7fbfffff7fffffffffbfffff3f400000)], Expect::Ok(0x000000000000000000000000ffffffff, 0x1f81)),
    ("dm_dpps_14_57_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0xf1], 0x0, &[(3, 0x80000001402000007fa000007fc00001), (4, 0x7f8000004b0000003f8000013f400000)], Expect::Fault(0x3)),
    ("dm_dpps_15_10_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x13], 0x0, &[(3, 0x34000000800000007fa000007fffffff), (4, 0xbf0000004b000000400000007f800000)], Expect::Ok(0x00000000000000007fffffff7fffffff, 0x0)),
    ("dm_dpps_18_55_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x31], 0x9fc0, &[(3, 0x338000007f00000033800000ff800001), (4, 0xbf0000007f8000017fa000004aaaaaab)], Expect::Ok(0x0000000000000000000000007fe00000, 0x9fc1)),
    ("dm_dpps_21_50_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x13], 0x0, &[(3, 0x007fffff4afffffebf800000ffbfffff), (4, 0x00400000cb0000013f800000bf7fffff)], Expect::Fault(0x1)),
    ("dm_dpps_26_22_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x31], 0x9fc0, &[(3, 0xcb000001007fffff4b7fffff00400000), (4, 0x3fc00000000000010040000000000001)], Expect::Ok(0x00000000000000000000000000000000, 0x9fc0)),
    ("dm_dpps_2_37_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x71], 0x0, &[(3, 0x004000003effffff338000003f800000), (4, 0x007fffff4aaaaaab0040000033800000)], Expect::Fault(0x2)),
    ("dm_dpps_2_52_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x32], 0x0, &[(3, 0x8000000080000000bf7fffff3f800000), (4, 0x338000007f0000003f8000004b000001)], Expect::Fault(0x20)),
    ("dm_dpps_31_11_1", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x13], 0x7f80, &[(3, 0x3e8000007f8000017e8000007f7fffff), (4, 0xbe8000008080000040000000ff800000)], Expect::Ok(0x0000000000000000ff800000ff800000, 0x7f80)),
    ("dm_dpps_31_38_1", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x31], 0x7f80, &[(3, 0x3effffffc2f6e9794b0000017f7fffff), (4, 0x3f0000003effffff008000004b000000)], Expect::Ok(0x0000000000000000000000007f7fffff, 0x7fa8)),
    ("dm_dpps_37_38_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x71], 0x9fc0, &[(3, 0x0000000100800001bf80000033800000), (4, 0xc020000000000000000000004b000000)], Expect::Ok(0x0000000000000000000000003f000000, 0x9fc0)),
    ("dm_dpps_38_31_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0xff], 0x0, &[(3, 0x3f8000017fa0000000ffffff4b000000), (4, 0x00ffffff7fa000004b8000007f7fffff)], Expect::Fault(0x1)),
    ("dm_dpps_40_18_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x32], 0x9fc0, &[(3, 0x4b800000c02000004b0000014b800000), (4, 0x7effffff80800000c0200000ff800001)], Expect::Ok(0x0000000000000000ffc0000100000000, 0x9fe1)),
    ("dm_dpps_42_28_2", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0xf1], 0x9fc0, &[(3, 0x807fffffff8000017fa0000001000000), (4, 0x00ffffff4b0000017f80000100800000)], Expect::Ok(0x0000000000000000000000007fe00000, 0x9ff1)),
    ("dm_dpps_53_47_1", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x13], 0x7f80, &[(3, 0x7fc00000807fffff3f400000cb000001), (4, 0x340000003f0000007f800000bfc00000)], Expect::Ok(0x00000000000000004b4000014b400001, 0x7fa0)),
    ("dm_dpps_57_9_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0xf1], 0x0, &[(3, 0x7effffff7fc000007fc000013f400000), (4, 0xffbfffff80000000ffc000003f7fffff)], Expect::Fault(0x1)),
    ("dm_dpps_58_47_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x33], 0x0, &[(3, 0x00400000402000004afffffec2f6e979), (4, 0x34000000c2f6e9793f400000bfc00000)], Expect::Fault(0x20)),
    ("dm_dpps_5_13_3", &[0x66, 0x0f, 0x3a, 0x40, 0xdc, 0x32], 0x0, &[(3, 0x4b800000bf000000c0200000c0000000), (4, 0x8080000080000001ff800000ffc00000)], Expect::Ok(0x0000000000000000ffc0000000000000, 0x0)),
    ("dp_dppd_174_1", &[0x66, 0x44, 0x0f, 0x3a, 0x41, 0xc2, 0xae], 0x3f80, &[(2, 0xc0073bd88e306578bff1be3c234a2840), (8, 0x00100000000000008008f9d8727ccb51)], Expect::Ok(0x80273bd88e3065780000000000000000, 0x3f80)),
    ("dp_dpps_107_0", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xed, 0x6b], 0x1f80, &[(13, 0xff6fa3b77f7c18d940290a564aa9ec40)], Expect::Ok(0x7f800000000000007f8000007f800000, 0x1fa8)),
    ("dp_dpps_122_5", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xe9, 0x7a], 0xfbf, &[(9, 0x3f800000c0000000b04f657000800000), (13, 0x0b2a6d96bff803a2ff4a3f62cab115fc)], Expect::Fault(0xfbf)),
    ("dp_dpps_12_4", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xf5, 0x0c], 0x6f80, &[(13, 0xcb6cf8a14aaaaaab7fffffff3effffff), (14, 0x808000007f800001c7618f644afffffe)], Expect::Ok(0x00000000000000000000000000000000, 0x6f80)),
    ("dpr_dppd_1184", &[0x66, 0x0f, 0x3a, 0x41, 0xd8, 0xae], 0xa95c, &[(0, 0xc32f82309e15e1628011ffe9c28a4240), (3, 0x3ca7e7139dade7ca8004149874e8d8f9)], Expect::Fault(0xa97c)),
    ("dpr_dppd_1305", &[0x66, 0x44, 0x0f, 0x3a, 0x41, 0xe2, 0xff], 0x1f95, &[(2, 0x80258e760353a035bffe53cbecbc8e21), (12, 0x0004597fc681fd803cbe11a69605d332)], Expect::Ok(0xbccc7f499326b6a2bccc7f499326b6a2, 0x1fb7)),
    ("dpr_dppd_1583", &[0x66, 0x45, 0x0f, 0x3a, 0x41, 0xf7, 0xef], 0xc1a0, &[(14, 0x001446bdd8870f9d3caa905f24af1c9e), (15, 0x7fda97639be6dbe9c33e7f80a5969f38)], Expect::Fault(0xc1a0)),
    ("dpr_dppd_1679", &[0x66, 0x44, 0x0f, 0x3a, 0x41, 0xe7, 0x96], 0x9f80, &[(7, 0x800011338d3ac271000402076774fcda), (12, 0x0012f3b31f0048e706b42d1916d3fde4)], Expect::Ok(0x00000000000000000000000000000000, 0x9fb2)),
    ("dpr_dppd_2117", &[0x66, 0x0f, 0x3a, 0x41, 0xf8, 0x65], 0x28b1, &[(0, 0x433ba9292aa0ce5c3cac75a7680c9bf5), (7, 0xc33cb1a331ad49f9000a6ed662961e36)], Expect::Fault(0x28b1)),
    ("dpr_dppd_3145", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xfc, 0xc7], 0x3780, &[(7, 0xbca79b5cb8994992c000a02c44790964), (12, 0xfff06f2e941f6034bcb77c775786e034)], Expect::Ok(0x00000000000000000000000000000000, 0x3780)),
    ("dpr_dppd_3370", &[0x66, 0x45, 0x0f, 0x3a, 0x41, 0xe3, 0x3f], 0x563e, &[(11, 0xc002a7f361b5fcdd000618c37ba62586), (12, 0x7fd3f9a980a6e06980054e6e49be39dd)], Expect::Fault(0x563e)),
    ("dpr_dppd_447", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xc6, 0xff], 0x2678, &[(0, 0xbca5c9b49e29b14c00060c0c9c1fcc13), (14, 0xc00a79405b34f079c344fc7f0feeff32)], Expect::Fault(0x2678)),
    ("dpr_dppd_4533", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xf7, 0xff], 0x9780, &[(6, 0xbcafe5fab5fd7bb1434335c8c2108b61), (15, 0xfffc6e9bf2bea5453ca8d649272beb99)], Expect::Ok(0xfffc6e9bf2bea545fffc6e9bf2bea545, 0x97a0)),
    ("dpr_dppd_4554", &[0x66, 0x45, 0x0f, 0x3a, 0x41, 0xcf, 0xff], 0x40e7, &[(9, 0x434262d87a0eae09ffd04327b5e191b3), (15, 0x43456738572868783ff4120e85c6aa8e)], Expect::Fault(0x40e7)),
    ("dpr_dppd_5047", &[0x66, 0x0f, 0x3a, 0x41, 0xe0, 0x33], 0x8b7, &[(0, 0xc0088c123e16a2030015718183844cbc), (4, 0xbcbe6f8f20e3524abff4bd3e77a57552)], Expect::Fault(0x8b7)),
    ("dpr_dppd_5391", &[0x66, 0x0f, 0x3a, 0x41, 0xca, 0xff], 0x1d80, &[(1, 0x433bc8deb647d8d20002fb7a855b2008), (2, 0x433f42113ff1ac94bcbb0319b0769ab5)], Expect::Ok(0x468b23f506292c2d468b23f506292c2d, 0x1db2)),
    ("dpr_dppd_5663", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xca, 0xd4], 0x9139, &[(1, 0xc3310e73c75d2b98c325d17f4f970cda), (10, 0x3ca083f374a222ab7fd33c2c897bb5d1)], Expect::Fault(0x9139)),
    ("dpr_dppd_5843", &[0x66, 0x44, 0x0f, 0x3a, 0x41, 0xf2, 0x38], 0x6f80, &[(2, 0x7fd891dc0a209e72400a990bba584144), (14, 0x3ca3a17cad264b93001c0aa8d10e8c41)], Expect::Fault(0x6fa0)),
    ("dpr_dppd_6148", &[0x66, 0x44, 0x0f, 0x3a, 0x41, 0xc4, 0x64], 0x9780, &[(4, 0x87396498db4d32257fea475b03811e9b), (8, 0xc329c5dd8ea8390b800c79058dabced5)], Expect::Ok(0x00000000000000000000000000000000, 0x97a0)),
    ("dpr_dppd_6250", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xd6, 0x9e], 0x7ec0, &[(2, 0x00081474226a42d1c34f4df9368f4e3c), (14, 0xbffd9291212b4bbf40093e371f9a5730)], Expect::Ok(0xc368b1c7b26886830000000000000000, 0x7ee0)),
    ("dpr_dppd_6510", &[0x66, 0x45, 0x0f, 0x3a, 0x41, 0xcd, 0xff], 0x5e80, &[(9, 0x0010fdb340fdb128c3274c018774f475), (13, 0x00038eee7ae41a7e3ff4e07b38cf658e)], Expect::Fault(0x5e82)),
    ("dpr_dppd_6644", &[0x66, 0x0f, 0x3a, 0x41, 0xd4, 0x5a], 0x56f1, &[(2, 0x433552ef0d9b27450029b83420c93c61), (4, 0xc3263690b6816fa3bcba581c4df0174d)], Expect::Fault(0x56f1)),
    ("dpr_dppd_7161", &[0x66, 0x45, 0x0f, 0x3a, 0x41, 0xfd, 0xff], 0xa1f4, &[(13, 0x433581af19b4b8acffd28089b80605f9), (15, 0x0021b36e9202f55cffffe6a1fc8895ed)], Expect::Fault(0xa1f4)),
    ("dpr_dppd_7474", &[0x66, 0x0f, 0x3a, 0x41, 0xfb, 0x7e], 0xe77e, &[(3, 0xbffcaf710a6cc6b0ffe5d626cf8748f7), (7, 0x80286edad9573de60012969aa77bb097)], Expect::Fault(0xe77e)),
    ("dpr_dppd_7526", &[0x66, 0x0f, 0x3a, 0x41, 0xf6, 0xd9], 0x8f80, &[(6, 0x00119685434d9e42c34339a821cdbf21)], Expect::Fault(0x8fa0)),
    ("dpr_dppd_7693", &[0x66, 0x41, 0x0f, 0x3a, 0x41, 0xca, 0x17], 0xa929, &[(1, 0x3cbb7f3f273a87047ff445349dc2c8e8), (10, 0xbca67a0283bde5ec801cb56c3062bcba)], Expect::Fault(0xa929)),
    ("dpr_dpps_1163", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xf6, 0x4d], 0x7e80, &[(6, 0x8105b174cc0702ac4b7482ce3f89f502), (14, 0x8019fc30cad71fc00b380f370b10867d)], Expect::Ok(0x5762e7fa5762e7fa000000005762e7fa, 0x7ea0)),
    ("dpr_dpps_1250", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xc6, 0x6e], 0xe78, &[(6, 0x8b4243697f014b5ebfeff9694fe8cd28), (8, 0xbfe46790b002af62f4b13a644c194d5d)], Expect::Fault(0xe78)),
    ("dpr_dpps_1357", &[0x66, 0x0f, 0x3a, 0x40, 0xe4, 0x0b], 0x1780, &[(4, 0x3f88af5fb065ba683fceb7bf4c0c880f)], Expect::Ok(0x00000000000000000000000000000000, 0x1780)),
    ("dpr_dpps_1724", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xee, 0x53], 0x17c0, &[(6, 0xb06f5487f4d48bf04bc453024028b15d), (13, 0x8b76df0b0118b23174838ffc498cd647)], Expect::Ok(0x00000000000000004a399c644a399c64, 0x17e0)),
    ("dpr_dpps_2262", &[0x66, 0x0f, 0x3a, 0x40, 0xe3, 0xff], 0xfb80, &[(3, 0xcb7893d70b7b66f10b753117cbc6267a), (4, 0xff4e6445f4c81b118ee2b82174e31a07)], Expect::Fault(0xfbb8)),
    ("dpr_dpps_2669", &[0x66, 0x0f, 0x3a, 0x40, 0xcd, 0x59], 0xadf4, &[(1, 0x4aa9e12d4b186ff0ca84320e00918d7f), (5, 0x00f1ab4d81598694008ab0e9ff08f457)], Expect::Fault(0xadf4)),
    ("dpr_dpps_2670", &[0x66, 0x0f, 0x3a, 0x40, 0xec, 0xff], 0x5faa, &[(4, 0xcc4e0f40307a6339803cc3dacfa5ae8a), (5, 0x7f74b687c008d77d61591696c05778b2)], Expect::Ok(0xff7ffffeff7ffffeff7ffffeff7ffffe, 0x5faa)),
    ("dpr_dpps_3108", &[0x66, 0x0f, 0x3a, 0x40, 0xdb, 0xff], 0x94a1, &[(3, 0x807471b440532ad9405a2370cadbd6ff)], Expect::Fault(0x94a3)),
    ("dpr_dpps_3577", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xc4, 0x00], 0x1f00, &[(4, 0xcab4b8f83f96004b4b9bcc824fa07c43), (8, 0x800517a8bfc42a7880510656801930d1)], Expect::Ok(0x00000000000000000000000000000000, 0x1f00)),
    ("dpr_dpps_4008", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xd3, 0xff], 0x353, &[(2, 0x7f86c8cf0b605007403abf64ff0f8ee0), (11, 0xc048eea44b002e44fffbfa90caea8495)], Expect::Fault(0x353)),
    ("dpr_dpps_5588", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xf7, 0x31], 0xecee, &[(6, 0x00f3847600766c314f8dd3c70b40ec45), (15, 0xf4dd91670b7d8a2180c0f564bfd5b0de)], Expect::Fault(0xecee)),
    ("dpr_dpps_5594", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xd2, 0xc6], 0x928e, &[(10, 0x80567e018097d0c1f4fe4fbc74a96e41)], Expect::Fault(0x928e)),
    ("dpr_dpps_6009", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xe8, 0xff], 0xf780, &[(8, 0x006302324aadeb2a74f41f7bcc366181), (13, 0xb0047eff3fbbaaa94aa619b300f32db5)], Expect::Fault(0xf7ba)),
    ("dpr_dpps_6029", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xeb, 0x2a], 0x12b6, &[(11, 0xcfd3f7a0ff8f93b3ca969b10f498bd18), (13, 0xcb215c85b04450a2800f2ad8ff0c0a82)], Expect::Fault(0x12b6)),
    ("dpr_dpps_6065", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xdc, 0x9f], 0x38f9, &[(11, 0xb0617e4700c973444fb57cf5bf84157c), (12, 0x805336864003e3ffbf926def4b2ab4ec)], Expect::Ok(0xcb30273bcb30273bcb30273bcb30273b, 0x38f9)),
    ("dpr_dpps_6259", &[0x66, 0x0f, 0x3a, 0x40, 0xca, 0xd4], 0xde8f, &[(1, 0x802d1d1c2ac7e4067fb965be81614585), (2, 0x407495cb4b0e2ca5c044ad65b06a3de0)], Expect::Fault(0xde8f)),
    ("dpr_dpps_6318", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xf7, 0xff], 0xfc0, &[(7, 0xf65e970d005dfa71caaf639c4aa5c17b), (14, 0x80b6d1a9cb846652010f6a1bcab81fec)], Expect::Fault(0xfe0)),
    ("dpr_dpps_6342", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xee, 0xff], 0x5419, &[(5, 0x4b5ebb5bb060fa4b7fbfc8da3fc4eec0), (14, 0xcb41dee2cb55bf46b0487160c0795bc5)], Expect::Fault(0x5419)),
    ("dpr_dpps_6909", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xec, 0xff], 0x8cbe, &[(5, 0x005ac0bcc0750926c0641c0a5b5aea01), (12, 0xb031bb0bff0cb5b4a006dfd88036bc59)], Expect::Fault(0x8cbe)),
    ("dpr_dpps_6992", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xff, 0xb3], 0xfbf, &[(7, 0xcffbc9f880f6fd89b1766c5fcfbca83f), (15, 0x00e9bcf73017102c807c10a64b508a3d)], Expect::Fault(0xfbf)),
    ("dpr_dpps_7046", &[0x66, 0x0f, 0x3a, 0x40, 0xdf, 0x4c], 0x7e80, &[(3, 0xfeb3188770899e483fc866bccc4bd22b), (7, 0x4c4165030173f8064f9295708b2fba89)], Expect::Ok(0x32832692328326920000000000000000, 0x7ea0)),
    ("dpr_dpps_7141", &[0x66, 0x44, 0x0f, 0x3a, 0x40, 0xe9, 0x2b], 0xff20, &[(1, 0x3023059a014893aacbe8396fb059bf44), (13, 0x7480fc2ff4f33004cbb6124bcb276a1f)], Expect::Ok(0x5825296c000000005825296c5825296c, 0xff20)),
    ("dpr_dpps_853", &[0x66, 0x45, 0x0f, 0x3a, 0x40, 0xc2, 0xda], 0x1f80, &[(8, 0x7fb54f9a8b5e1d108b582bdc816680f9), (10, 0x40145968cfc6545cf4f1c2f9c005242f)], Expect::Ok(0x7ff54f9a000000007ff54f9a00000000, 0x1fa1)),
    ("dpr_dpps_870", &[0x66, 0x41, 0x0f, 0x3a, 0x40, 0xd6, 0xff], 0xf933, &[(2, 0xf4ae193fcb31d2cff4f08e09cfbb7a76), (14, 0x00bd8b6cbfe7a49bb062d8cb00fcc5de)], Expect::Ok(0x65d5291965d5291965d5291965d52919, 0xf933)),
    ("rp_roundpd_12_12", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xd1, 0xee], 0x1780, &[(9, 0xffd4ca9c82a3cba63cb0000000000000), (10, 0x8000000000000001800fffffffffffff)], Expect::Ok(0xffd4ca9c82a3cba60000000000000000, 0x1780)),
    ("rp_roundpd_1_116", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xe3, 0x02], 0x3f80, &[(11, 0xbfe00000000000007bd9ac25f7e02bc1), (12, 0x800b40eec33cbfe23cad96d408c89487)], Expect::Ok(0x80000000000000007bd9ac25f7e02bc1, 0x3fa0)),
    ("rp_roundpd_1_60", &[0x66, 0x44, 0x0f, 0x3a, 0x09, 0xe9, 0x58], 0x3f80, &[(1, 0x55af9ee814d8e1748000000000000000), (13, 0x43400000000000003fdfffffffffffff)], Expect::Ok(0x55af9ee814d8e1748000000000000000, 0x3f80)),
    ("rp_roundpd_28_77", &[0x66, 0x44, 0x0f, 0x3a, 0x09, 0xec, 0x06], 0x7d80, &[(4, 0x4014000000000000fff0000000000001), (13, 0xfff00000000000008000000000000000)], Expect::Ok(0x4014000000000000fff8000000000001, 0x7d81)),
    ("rp_roundpd_40_67", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xcf, 0x0b], 0x9780, &[(9, 0x3fe80000000000000000000000000003), (15, 0xbcbd47280000c47e800196352f475c08)], Expect::Ok(0x80000000000000008000000000000000, 0x9780)),
    ("rp_roundpd_42_110", &[0x66, 0x0f, 0x3a, 0x09, 0xca, 0x00], 0x8f80, &[(1, 0x3cb00000000000008000000000000001), (2, 0xfff00000000000010020000000000000)], Expect::Fault(0x8fa1)),
    ("rp_roundpd_49_40", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xf9, 0xbf], 0x7dc0, &[(9, 0x7fe00000000000007fe63965c9d908b2), (15, 0x0020000000000000c3397a805dec21bd)], Expect::Ok(0x7fe00000000000007fe63965c9d908b2, 0x7dc0)),
    ("rp_roundpd_57_80", &[0x66, 0x0f, 0x3a, 0x09, 0xc1, 0xd1], 0x4000, &[(0, 0x3cb0000000000000800748a87e200693), (1, 0x3ffcae06479af8eb3fd0000000000000)], Expect::Fault(0x4020)),
    ("rp_roundpd_59_75", &[0x66, 0x0f, 0x3a, 0x09, 0xfd, 0x05], 0x8000, &[(5, 0x3ff35ef2c6c30a1b4340000000000000), (7, 0x000e1c112e82901b3cbfdd89db99569d)], Expect::Fault(0x8020)),
    ("rp_roundpd_61_108", &[0x66, 0x0f, 0x3a, 0x09, 0xcd, 0xc0], 0x80c0, &[(1, 0x7fffffffffffffff0008000000000000), (5, 0xbcbc6b1edad61ca94014000000000000)], Expect::Fault(0x80e0)),
    ("rp_roundpd_63_96", &[0x66, 0x44, 0x0f, 0x3a, 0x09, 0xde, 0xd2], 0x1f95, &[(6, 0x00100000000000007ff7ffffffffffff), (11, 0x43400000000000003cbbb7ea435e9f13)], Expect::Ok(0x3ff00000000000007fffffffffffffff, 0x1fb5)),
    ("rp_roundpd_65_120", &[0x66, 0x41, 0x0f, 0x3a, 0x09, 0xe1, 0x96], 0xfbf, &[(4, 0x7fd0000000000000800fffffffffffff), (9, 0x400ba1883702effd8000000000000000)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_128", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xe4, 0x51], 0xfbf, &[(12, 0x0020000000000000000ddcc1cae65dbb)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_13", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xc8, 0x04], 0xfbf, &[(8, 0x3fd00000000000000010000000000001), (9, 0xc330000000000001c330000000000001)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_27", &[0x66, 0x0f, 0x3a, 0x09, 0xdb, 0x04], 0xfbf, &[(3, 0xc325ad5de128198f8010000000000000)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_58", &[0x66, 0x44, 0x0f, 0x3a, 0x09, 0xe7, 0x07], 0xfbf, &[(7, 0x432ffffffffffffe0000000000000003), (12, 0x4330000000000000432cbba8120b40f5)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_61", &[0x66, 0x41, 0x0f, 0x3a, 0x09, 0xc6, 0x03], 0xfbf, &[(0, 0x7ff8000000000000bff646fe002dd4ae), (14, 0x3cb32327176a318e7fd0000000000000)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_78", &[0x66, 0x0f, 0x3a, 0x09, 0xf1, 0x04], 0xfbf, &[(1, 0xfff80000000000003ff0000000000001), (6, 0x40140000000000003ff8000000000000)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_94", &[0x66, 0x0f, 0x3a, 0x09, 0xc9, 0x03], 0xfbf, &[(1, 0x3fe8000000000000001fffffffffffff)], Expect::Fault(0xfbf)),
    ("rp_roundpd_65_95", &[0x66, 0x0f, 0x3a, 0x09, 0xe5, 0x04], 0xfbf, &[(4, 0xffde20bcf7419c3fbff8000000000000), (5, 0x3cb0000000000000c330000000000001)], Expect::Fault(0xfbf)),
    ("rp_roundpd_6_37", &[0x66, 0x41, 0x0f, 0x3a, 0x09, 0xd4, 0x07], 0xffc0, &[(2, 0x3ff0000000000001800a2de44496bf94), (12, 0xffefffffffffffffbff1b98f03f602b8)], Expect::Ok(0xffefffffffffffffbff0000000000000, 0xffe0)),
    ("rp_roundpd_71_147", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xd6, 0x06], 0x1f00, &[(10, 0xbfefffffffffffff3ff0000000000001), (14, 0x7ff0000000000000ffd3c741510e2a77)], Expect::Ok(0x7ff0000000000000ffd3c741510e2a77, 0x1f00)),
    ("rp_roundpd_74_70", &[0x66, 0x45, 0x0f, 0x3a, 0x09, 0xe0, 0x08], 0x1a00, &[(8, 0xbfd0000000000000fff7ffffffffffff), (12, 0x7ff5281e4f1556ff0000000000000000)], Expect::Fault(0x1a01)),
    ("rp_roundps_14_19", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xe4, 0x05], 0x3f00, &[(4, 0x4af9959ebf800000004000003f000000), (12, 0x7e80000080a58e2e4027a797cb000001)], Expect::Ok(0x7e800000bf80000040000000cb000001, 0x3f20)),
    ("rp_roundps_19_14", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xd4, 0x03], 0x2f80, &[(2, 0xffc12345c0000000a76b82413fc00000), (12, 0xff1711db3effffffbf0000000b62fd5f)], Expect::Fault(0x2fa0)),
    ("rp_roundps_1_11", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xea, 0x0c], 0x3f80, &[(10, 0x008000017f00000040400000407bf303), (13, 0x7f80000174dd0049000000033fc00000)], Expect::Ok(0x000000007f0000004040000040400000, 0x3f80)),
    ("rp_roundps_23_133", &[0x66, 0x0f, 0x3a, 0x08, 0xce, 0x07], 0x5b80, &[(1, 0x7effffff808000004aaaaaabcbbaf603), (6, 0xff7fffff807fffff7fc000007effffff)], Expect::Ok(0xff7fffff800000007fc000007effffff, 0x5ba0)),
    ("rp_roundps_31_142", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xc6, 0x03], 0x6f80, &[(6, 0x3069d050814686094aaaaaabb00df945), (8, 0x6b9d74514aaaaaabbf7fffff303b64a6)], Expect::Fault(0x6fa0)),
    ("rp_roundps_33_13", &[0x66, 0x0f, 0x3a, 0x08, 0xf0, 0x0c], 0x1f40, &[(0, 0x402000004b800000b5bf0125603698ee), (6, 0x01648d9e40a000007f8000003f400000)], Expect::Ok(0x400000004b80000080000000603698ee, 0x1f40)),
    ("rp_roundps_43_11", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xe0, 0x00], 0xfc0, &[(8, 0x80f3ed970b2646ed3fb549a43f800000), (12, 0xcbd224f0010000004feef4c67fffffff)], Expect::Fault(0xfe0)),
    ("rp_roundps_44_79", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xd6, 0x06], 0xff00, &[(10, 0x80eddb8034000000bf7fffff80800000), (14, 0x402000004ba550018000000140000000)], Expect::Ok(0x400000004ba550018000000040000000, 0xff20)),
    ("rp_roundps_44_9", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xf3, 0x03], 0xff00, &[(3, 0x3e800000010000007fa000007f800001), (14, 0x0b505f6b3f7fffffcc68b34f0b55c46c)], Expect::Fault(0xff01)),
    ("rp_roundps_4_144", &[0x66, 0x0f, 0x3a, 0x08, 0xf6, 0x75], 0x1fc0, &[(6, 0x3fe20d45004000007fc00001cb000001)], Expect::Ok(0x40000000000000007fc00001cb000001, 0x1fe0)),
    ("rp_roundps_50_131", &[0x66, 0x0f, 0x3a, 0x08, 0xeb, 0x01], 0xfb80, &[(3, 0x7f800000be800000ff800001007fffff), (5, 0x30512c35305a7ea63fc00000f4cee2ad)], Expect::Ok(0x7f800000bf800000ffc0000100000000, 0xfba1)),
    ("rp_roundps_55_35", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xd3, 0x09], 0x6fc0, &[(10, 0x4aaaaaab40a000000051d103ffbfffff), (11, 0x8080000001784b5b7f1caa2e00ffffff)], Expect::Ok(0xbf800000000000007f1caa2e00000000, 0x6fc0)),
    ("rp_roundps_55_71", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xc0, 0x04], 0x6fc0, &[(0, 0xa99f4324008000014afffffecac13f87), (8, 0x4bbd6883cb000001cb70be8c7f800000)], Expect::Fault(0x6fe0)),
    ("rp_roundps_55_95", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xd6, 0x0a], 0x6fc0, &[(10, 0x3f7fffff404d1df5ff7fffff4b800000), (14, 0x7fc00001c0200000009cb28180800000)], Expect::Ok(0x7fc00001c00000003f80000080000000, 0x6fc0)),
    ("rp_roundps_56_94", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xfc, 0x05], 0x2000, &[(4, 0x40400000004000003f4000008139b461), (15, 0x00000001b02ac2c3008a07ad00400000)], Expect::Fault(0x2020)),
    ("rp_roundps_59_73", &[0x66, 0x0f, 0x3a, 0x08, 0xe4, 0x09], 0x8000, &[(4, 0x338000004b083bf57fbfffff012e5cea)], Expect::Fault(0x8001)),
    ("rp_roundps_60_68", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xe2, 0x3c], 0x40, &[(4, 0x3f7fffff7f000000c0200000cad0e4d2), (10, 0xcb0000017fa000003380000000000003)], Expect::Fault(0x41)),
    ("rp_roundps_65_118", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xe8, 0x07], 0xfbf, &[(8, 0x7f000000699b07fc7effffff00ffffff), (13, 0x8b004adfff62748e4c609d3140200000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_132", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xd9, 0x93], 0xfbf, &[(9, 0xcfdb422b3fc0000080b729824b800000), (11, 0x80800000bf7ffffff49306c84aaaaaab)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_154", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xe2, 0x01], 0xfbf, &[(10, 0x3effffff40a0000001000000c2f6e979), (12, 0xcbe99a803ffb4821400000007f000000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_155", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xe0, 0x07], 0xfbf, &[(8, 0xbe800000cc325657bf9e594733800000), (12, 0xbe8000003f7fffff4c6fcb28b076a5e2)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_27", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xce, 0x07], 0xfbf, &[(6, 0x3fb428a73f0000007effffffbf800000), (9, 0xbf000000808000004ad256694afffffe)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_28", &[0x66, 0x0f, 0x3a, 0x08, 0xc8, 0x36], 0xfbf, &[(0, 0xcb9b12d30b7f40de007fffff4f8dbf05), (1, 0x7e800000ca86850bbf8000003fc00000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_30", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xe4, 0x02], 0xfbf, &[(4, 0xb07ff4347f8000017f8000014aeba89e), (12, 0xbf0000003fc000003e8000003e800000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_55", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xef, 0x00], 0xfbf, &[(7, 0x4b000000ffc12345402000008b7c1015), (13, 0xc2f6e9794fd2e1fbbfb06c4f4b000000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_57", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xc7, 0x03], 0xfbf, &[(0, 0x7f000000ffaaa072ff8000008045fe03), (15, 0xc0200000ffc3af7d7f80000033800000)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_65", &[0x66, 0x0f, 0x3a, 0x08, 0xff, 0x03], 0xfbf, &[(7, 0x80800000ffc1234580000001bf7fffff)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_82", &[0x66, 0x44, 0x0f, 0x3a, 0x08, 0xf7, 0x02], 0xfbf, &[(7, 0x2ec9833f01053d064b7fffff3e800000), (14, 0xca8d3ace4c4fcf73c0424a40807fffff)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_95", &[0x66, 0x0f, 0x3a, 0x08, 0xd5, 0x03], 0xfbf, &[(2, 0x4b800000007fffff8114de94cae1366f), (5, 0xf4d5dbe8bf0000003f0000004b000001)], Expect::Fault(0xfbf)),
    ("rp_roundps_65_97", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xf5, 0x06], 0xfbf, &[(6, 0x7fc00000cf8046c0ffbfffffc0042058), (13, 0x0b67e10cffc69e954b0000003effffff)], Expect::Fault(0xfbf)),
    ("rp_roundps_66_9", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xff, 0x09], 0x1e80, &[(15, 0x807593114b7fffffffbfffff7f800001)], Expect::Ok(0xbf8000004b7fffffffffffff7fc00001, 0x1e81)),
    ("rp_roundps_71_17", &[0x66, 0x0f, 0x3a, 0x08, 0xc1, 0x0e], 0x1f00, &[(0, 0x4fed3631f4f9fe9a7fa2679f4b000000), (1, 0x3f40000000400000ff800001c02eb0e9)], Expect::Fault(0x1f01)),
    ("rp_roundps_72_72", &[0x66, 0x45, 0x0f, 0x3a, 0x08, 0xd2, 0x5c], 0x1700, &[(10, 0x7f7fffff016d4a40bf0000007e800000)], Expect::Ok(0x7f7fffff00000000800000007e800000, 0x1700)),
    ("rp_roundps_73_117", &[0x66, 0x41, 0x0f, 0x3a, 0x08, 0xf2, 0x01], 0x380, &[(6, 0xc0200000bf800000000000004c0e4353), (10, 0x4afffffe3f7fffff80f092114b000001)], Expect::Fault(0x3a0)),
    ("rp_roundps_8_95", &[0x66, 0x0f, 0x3a, 0x08, 0xe1, 0x09], 0x1f00, &[(1, 0xffc12345b05d8259ffc000008b1ace01), (4, 0x004000004fc4239abfafaf7fff800000)], Expect::Ok(0xffc12345bf800000ffc00000bf800000, 0x1f00)),
    ("rs_roundsd_20_11_3", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x0b], 0x7f80, &[(1, 0xed3f9ea83c485dab6efc512da93099e7), (2, 0x376d05bf640629a57ff7ffffffffffff)], Expect::Ok(0xed3f9ea83c485dab7fffffffffffffff, 0x7f81)),
    ("rs_roundsd_21_1_9", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x01], 0x9fc0, &[(1, 0x9c3b4b3e09c653c6b23f50de80334d6f), (2, 0x4e70db1adb15d299fff7ffffffffffff)], Expect::Ok(0x9c3b4b3e09c653c6ffffffffffffffff, 0x9fc1)),
    ("rs_roundsd_25_7_1", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x07], 0x3f80, &[(1, 0xaf24f881eccd1d28b87462bb6dd8d644), (2, 0x693be5d76ea8fbf9800fffffffffffff)], Expect::Ok(0xaf24f881eccd1d28bff0000000000000, 0x3fa0)),
    ("rs_roundsd_48_9_0", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x09], 0x1f80, &[(1, 0xe78602dc733e0e4a9ea0b3b5c9ec5400), (2, 0xcf0a77e4d2f71f29c004000000000000)], Expect::Ok(0xe78602dc733e0e4ac008000000000000, 0x1f80)),
    ("rs_roundsd_49_3_0", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x03], 0x1f80, &[(1, 0xbe1750c2f7a16f7a7d5a206f0775117d), (2, 0x2838e7dbd6236615bfe0000000000000)], Expect::Ok(0xbe1750c2f7a16f7a8000000000000000, 0x1fa0)),
    ("rs_roundsd_49_5_6", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x65], 0xf80, &[(1, 0xdf117b168d693c2d585443271c9ab4c9), (2, 0xcb474c865a61d783bfe0000000000000)], Expect::Fault(0xfa0)),
    ("rs_roundsd_50_1_6", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x01], 0xf80, &[(1, 0xf21be11fdb599b9ce2fa0683cb81e9cb), (2, 0x45becce24b3217b9bfefffffffffffff)], Expect::Fault(0xfa0)),
    ("rs_roundsd_52_11_8", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x0b], 0x2f00, &[(1, 0x63c562c2e589c75dc7cba9a1aae0cec4), (2, 0x08f794191c105a8a4330000000000001)], Expect::Ok(0x63c562c2e589c75d4330000000000001, 0x2f00)),
    ("rs_roundsd_55_5_7", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x85], 0x6fc0, &[(1, 0x308778fe909a723b05c134e76eb6eb12), (2, 0x89047f07bee9d7684325555555555555)], Expect::Fault(0x6fe0)),
    ("rs_roundsd_56_2_7", &[0x66, 0x0f, 0x3a, 0x0b, 0xca, 0x02], 0x6fc0, &[(1, 0x31b657169477ec76c986be3f4460262d), (2, 0x5e33ee94347fc787bfd0000000000000)], Expect::Fault(0x6fe0)),
    ("rs_roundss_10_4_9", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0xf4], 0x9fc0, &[(1, 0x1ff51af85f5f54309f2ee8ea135846c9), (2, 0xe56c7f066556d0729d3fa1e07f800000)], Expect::Ok(0x1ff51af85f5f54309f2ee8ea7f800000, 0x9fc0)),
    ("rs_roundss_14_3_8", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x03], 0x2f00, &[(1, 0xbd0fd1884bacb491fe0a082c30e1a554), (2, 0x25baef9cbb39e354eb0396827fc00001)], Expect::Ok(0xbd0fd1884bacb491fe0a082c7fc00001, 0x2f00)),
    ("rs_roundss_20_14_1", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x0e], 0x3f80, &[(1, 0xc992718016431a66ef44f28d61eae681), (2, 0x2f55f4ebcbd8c0aaf9d137c37fbfffff)], Expect::Ok(0xc992718016431a66ef44f28d7fffffff, 0x3f81)),
    ("rs_roundss_23_5_6", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x05], 0xf80, &[(1, 0x2d768ee4e254ccd44669ccd6f90127ab), (2, 0xc6c9f33eba714b3b8e97a1a380000001)], Expect::Fault(0xfa0)),
    ("rs_roundss_28_0_3", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x20], 0x7f80, &[(1, 0xbf6298bdb9e7df6c307f683fa0c02080), (2, 0xbf63950a32b7797e08486cab00800000)], Expect::Ok(0xbf6298bdb9e7df6c307f683f00000000, 0x7fa0)),
    ("rs_roundss_32_15_8", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0xaf], 0x2f00, &[(1, 0xd22810e02a8d67a041437688bd473291), (2, 0xbbae782ce4c33429c168b1a8ff7fffff)], Expect::Ok(0xd22810e02a8d67a041437688ff7fffff, 0x2f00)),
    ("rs_roundss_32_2_6", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x02], 0xf80, &[(1, 0x76a4a08d9a7654ce738f50e82226407a), (2, 0xd1c86a5da5d4f05cc754369eff7fffff)], Expect::Ok(0x76a4a08d9a7654ce738f50e8ff7fffff, 0xf80)),
    ("rs_roundss_36_5_9", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0xe5], 0x9fc0, &[(1, 0xdbefecb7ece4518d66e21b5f01515a7f), (2, 0x67b58b10804e8911caf6e05a34000000)], Expect::Ok(0xdbefecb7ece4518d66e21b5f00000000, 0x9fe0)),
    ("rs_roundss_36_7_0", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x07], 0x1f80, &[(1, 0x9d1600206caa57f2482bbe08271aad6c), (2, 0x94b13dd5230ef04d3cd5a7b334000000)], Expect::Ok(0x9d1600206caa57f2482bbe0800000000, 0x1fa0)),
    ("rs_roundss_44_6_1", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x06], 0x3f80, &[(1, 0x5729efc1db4648db84065ce396606c66), (2, 0x10b04a870ceb057694fa009f40a00000)], Expect::Ok(0x5729efc1db4648db84065ce340a00000, 0x3f80)),
    ("rs_roundss_46_11_9", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0xdb], 0x9fc0, &[(1, 0xbcb6138e0fe6c4d3bcf109867f15ca3e), (2, 0x45e004d3bf0b9baf79da2ffc40200000)], Expect::Ok(0xbcb6138e0fe6c4d3bcf1098640000000, 0x9fc0)),
    ("rs_roundss_46_13_2", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x0d], 0x5f80, &[(1, 0x2509610dc9bf497c307c7795801cb90e), (2, 0xeb9cc91a1c16ad24bc9f33f240200000)], Expect::Ok(0x2509610dc9bf497c307c779540400000, 0x5f80)),
    ("rs_roundss_48_3_8", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x03], 0x2f00, &[(1, 0xebbb2e1b4265da69d7325c04ed90a05e), (2, 0xe205539a97ba9c855f02e4f1c0200000)], Expect::Fault(0x2f20)),
    ("rs_roundss_48_6_2", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x06], 0x5f80, &[(1, 0x5d11d9a48cea245cd73387aea7cae6d6), (2, 0xf0c7708411210d07d52c751bc0200000)], Expect::Ok(0x5d11d9a48cea245cd73387aec0000000, 0x5fa0)),
    ("rs_roundss_52_15_6", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x7f], 0xf80, &[(1, 0x71e36131ba9896c826fc262121847ad3), (2, 0x8ed6426f57ebfe156bde24de4b000001)], Expect::Ok(0x71e36131ba9896c826fc26214b000001, 0xf80)),
    ("rs_roundss_52_1_8", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x01], 0x2f00, &[(1, 0xc58186aaec99b05ff59cb3f53d2cdb4f), (2, 0xc5bbb6d0a929b4c11225b0a64b000001)], Expect::Ok(0xc58186aaec99b05ff59cb3f54b000001, 0x2f00)),
    ("rs_roundss_57_6_4", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0xe6], 0x1fc0, &[(1, 0xd29f9d20e84f9dc91b3cc137574bdc6a), (2, 0x774876986d28d0d2e73cacc13f400000)], Expect::Ok(0xd29f9d20e84f9dc91b3cc1373f800000, 0x1fe0)),
    ("rs_roundss_58_5_6", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x05], 0xf80, &[(1, 0x98b303d03a1b8d12a6cc556087ef1891), (2, 0xc5dafd137d3f10c56e4f0a3cc2f6e979)], Expect::Fault(0xfa0)),
    ("rs_roundss_7_4_3", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x04], 0x7f80, &[(1, 0xe8389d84d339a571ebb1567fc049b611), (2, 0x23c3f7c6f2b69759cccc73333f000000)], Expect::Ok(0xe8389d84d339a571ebb1567f00000000, 0x7fa0)),
    ("rs_roundss_9_2_2", &[0x66, 0x0f, 0x3a, 0x0a, 0xca, 0x92], 0x5f80, &[(1, 0xcfef443a9ad60dffb35a8bf09789609f), (2, 0x8a3850278faa9484f623215c3f7fffff)], Expect::Ok(0xcfef443a9ad60dffb35a8bf03f800000, 0x5fa0)),
];
