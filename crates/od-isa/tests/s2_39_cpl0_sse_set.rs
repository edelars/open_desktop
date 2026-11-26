//! S2-39n (CPL0 set): the SSE forms that S2-39m left refused in the FP member of the CPL0 profile set, now on
//! `LongModeState.sse`: the register-register forms of the raw 128-bit moves (MOVUPS, MOVUPD, MOVAPS, MOVAPD,
//! MOVDQA load; `0F 10/11/28/29`, `66 0F 6F`), MOVSS and MOVSD in every form the decoder returns, LDMXCSR and
//! STMXCSR (`0F AE /2`, `/3`, m32, any addressing form), the integer SSE2 forms on XMM registers (PSHUFD, PUNPCK*,
//! PACK*, PAND/PANDN/POR/PXOR, PADD*/PSUB*, PCMPEQ*/PCMPGT*, PMOVMSKB, the shift-immediate groups, ...) and the
//! forms that only the CPL3 profile executed (MOVD/MOVQ, PSHUFLW, PINSRW m16, MOVLPD/MOVHPD/MOVHPS loads, PMINUB and
//! PCMPEQB with a memory operand). CR0.EM/TS and CR4.OSFXSR gate all of them (`#UD`, `#NM`) before any operand
//! access.
//!
//! The reference of every form that the plain `step` retires is the plain `step` on the same state (the
//! accepted, oracle-validated path of S2-34/S2-37/S2-38); LDMXCSR/STMXCSR and the CPL3-handler forms are asserted
//! against values computed in the test from the Intel SDM definitions (LDMXCSR/STMXCSR also against the physical
//! oracle, see the S2-39n report). No value in this file is model output of the set itself.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr4Profile, GuestFpState,
    RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, PrivilegeLevel, StiShadow, cpuid::CpuidProfile,
};
use std::cell::RefCell;

const CODE: u64 = 0x1000;
const DATA: u64 = 0x2000;
const RAM: usize = 0x4000;
const SENTINEL: u8 = 0xa5;
const CR0_BASE: u64 = 0x8001_0033;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR4_BASE: u64 = 0x20;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_FP: u64 = CR4_BASE | CR4_OSFXSR | CR4_OSXMMEXCPT;
const MXCSR_MASKED: u32 = 0x1f80;

struct Ram {
    bytes: Vec<u8>,
}

impl Ram {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![SENTINEL; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self { bytes }
    }

    fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
    }

    fn range(&self, address: u64, length: usize, access: AccessKind) -> Result<usize, MemoryError> {
        usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM))
            .ok_or(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let start = self.range(address, 1, AccessKind::Fetch)?;
        Ok(self.bytes[start])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let start = self.range(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, input: &[u8]) -> Result<(), MemoryError> {
        let start = self.range(address, input.len(), AccessKind::Write)?;
        self.bytes[start..start + input.len()].copy_from_slice(input);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

/// Distinct bytes in every register (NaNs, denormals and ordinary values mixed); register 1 and 2 are replaced by
/// `floats` when a case wants ordinary operands.
fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|byte| ((index * 16 + byte) as u8).wrapping_mul(0x1d) ^ 0x3c)
}

fn ordinary(index: usize) -> [u8; 16] {
    // Two doubles or four singles of ordinary magnitude, different per register.
    let singles: [f32; 4] = match index {
        1 => [1.5, 2.5, -3.25, 100.0],
        2 => [0.5, 4.0, 8.0, -1.0],
        _ => [index as f32 + 0.25, -(index as f32), 7.0, 0.125],
    };
    let mut out = [0u8; 16];
    for (lane, value) in singles.iter().enumerate() {
        out[lane * 4..lane * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

#[derive(Clone)]
struct Case {
    xmm: [[u8; 16]; 16],
    mxcsr: u32,
    cr0: u64,
    cr4: u64,
    rax: u64,
    rbx: u64,
    rcx: u64,
    memory: [u8; 16],
}

impl Case {
    fn new(ordinary_operands: bool) -> Self {
        let pick = |index| {
            if ordinary_operands {
                ordinary(index)
            } else {
                xmm_pattern(index)
            }
        };
        Self {
            xmm: core::array::from_fn(pick),
            mxcsr: MXCSR_MASKED,
            cr0: CR0_BASE,
            cr4: CR4_FP,
            rax: DATA,
            rbx: DATA,
            rcx: 0x1234_5678_9abc_def0,
            memory: pick(3),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Result_ {
    Retired,
    Fault(ArchitecturalFault),
    Unsupported(u8),
    Memory(MemoryError),
}

#[derive(Clone, Debug, PartialEq)]
struct Observed {
    result: Result_,
    rip: u64,
    rflags: u64,
    gpr: [u64; 16],
    xmm: [[u8; 16]; 16],
    mxcsr: u32,
    data: Vec<u8>,
}

fn initial_state(case: &Case) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2 | (1 << 9) | 0x85,
        cr0: Some(case.cr0),
        cr4: Some(case.cr4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    };
    for (index, slot) in state.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    state.gpr[0] = case.rax;
    state.gpr[1] = case.rcx;
    state.gpr[3] = case.rbx;
    state
}

fn initial_memory(code: &[u8], case: &Case) -> Ram {
    let mut ram = Ram::new(code);
    ram.put(DATA, &case.memory);
    ram
}

/// The bare run. `controls = false`: `step` on a state without control registers (the general S2-34/S2-37/S2-38/S2-54/
/// S2-55 core, no gates). `controls = true`: the FP-init CR0/CR4 profile pair of S2-37 (live CR0.EM/TS and CR4.OSFXSR
/// gates, #XM or #UD by CR4.OSXMMEXCPT) over a state that carries x87 and SSE.
fn bare(code: &[u8], case: &Case, controls: bool) -> Observed {
    bare_program(code, case, controls, 1)
}

/// `count` instructions of `code` in the bare run (the first non-retiring instruction ends it).
fn bare_program(code: &[u8], case: &Case, controls: bool, count: u64) -> Observed {
    let mut state = initial_state(case);
    state.sse = Some(SseState {
        xmm: case.xmm,
        mxcsr: case.mxcsr,
    });
    state.x87 = Some(X87State::fixture_seed());
    let mut ram = initial_memory(code, case);
    let exit = if controls {
        execute::run_with_cr4_profile(
            &mut state,
            &mut ram,
            RunConfig {
                instruction_budget: count,
                stop_rip: None,
                context: context(),
            },
            Cr4Profile::FpInitLong64V1,
            || false,
        )
        .exit
    } else {
        state.cr0 = None;
        state.cr4 = None;
        let mut exit = RunExit::InstructionBudgetExhausted;
        for _ in 0..count {
            exit = match execute::step(&mut state, &mut ram, context()) {
                Ok(StepOutcome::Advanced) => continue,
                Ok(StepOutcome::Fault(fault)) => RunExit::Fault(fault),
                Ok(StepOutcome::Unsupported { opcode }) => RunExit::Unsupported { opcode },
                Ok(other) => panic!("unexpected bare outcome {other:?}"),
                Err(error) => RunExit::MemoryError(error),
            };
            break;
        }
        exit
    };
    let result = match exit {
        RunExit::InstructionBudgetExhausted => Result_::Retired,
        RunExit::Fault(fault) => Result_::Fault(fault),
        RunExit::Unsupported { opcode } => Result_::Unsupported(opcode),
        RunExit::MemoryError(error) => Result_::Memory(error),
        other => panic!("unexpected bare exit {other:?}"),
    };
    let sse = state.sse.expect("the bare state keeps its SSE state");
    Observed {
        result,
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        xmm: sse.xmm,
        mxcsr: sse.mxcsr,
        data: ram.bytes[DATA as usize..DATA as usize + 0x500].to_vec(),
    }
}

fn set(cell: &RefCell<GuestFpState>) -> Cpl0ProfileSet<'_> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4AExt2)
        .with_fp_state(cell)
}

fn cell_for(case: &Case) -> RefCell<GuestFpState> {
    let mut x87 = X87State::fixture_seed();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    RefCell::new(GuestFpState {
        x87,
        sse: SseState {
            xmm: case.xmm,
            mxcsr: case.mxcsr,
        },
    })
}

fn run_in_set(
    state: &mut LongModeState,
    ram: &mut Ram,
    cell: &RefCell<GuestFpState>,
    budget: u64,
) -> (RunExit, u64) {
    let result = run_with_cpl0_profile_set(
        state,
        ram,
        RunConfig {
            instruction_budget: budget,
            stop_rip: None,
            context: context(),
        },
        od_isa::INTEGER_FLAGS,
        &set(cell),
        None,
        || false,
    )
    .expect("the case is admitted by the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    (exit, result.instructions_executed)
}

/// The same instruction in the CPL0 set with the FP member: the SSE state starts in and returns to the cell.
fn in_set(code: &[u8], case: &Case) -> (Observed, GuestFpState, GuestFpState) {
    in_set_program(code, case, 1)
}

fn in_set_program(code: &[u8], case: &Case, count: u64) -> (Observed, GuestFpState, GuestFpState) {
    let mut state = initial_state(case);
    let mut ram = initial_memory(code, case);
    let cell = cell_for(case);
    let before = cell.borrow().clone();
    let (exit, executed) = run_in_set(&mut state, &mut ram, &cell, count);
    let result = match exit {
        RunExit::InstructionBudgetExhausted => {
            assert_eq!(executed, count);
            Result_::Retired
        }
        RunExit::Fault(fault) => Result_::Fault(fault),
        RunExit::Unsupported { opcode } => Result_::Unsupported(opcode),
        RunExit::MemoryError(error) => Result_::Memory(error),
        other => panic!("unexpected set exit {other:?}"),
    };
    assert!(
        state.sse.is_none() && state.x87.is_none(),
        "the set hands the state back without FP"
    );
    let after = cell.borrow().clone();
    (
        Observed {
            result,
            rip: state.rip,
            rflags: state.rflags,
            gpr: state.gpr,
            xmm: after.sse.xmm,
            mxcsr: after.sse.mxcsr,
            data: ram.bytes[DATA as usize..DATA as usize + 0x500].to_vec(),
        },
        before,
        after,
    )
}

/// `Err` names the first difference between the set and the bare reference.
fn compare(
    name: &str,
    code: &[u8],
    case: &Case,
    controls: bool,
    expect: Option<Result_>,
) -> Result<(), String> {
    compare_program(name, code, case, controls, 1, expect)
}

fn compare_program(
    name: &str,
    code: &[u8],
    case: &Case,
    controls: bool,
    count: u64,
    expect: Option<Result_>,
) -> Result<(), String> {
    let reference = bare_program(code, case, controls, count);
    let (observed, before, after) = in_set_program(code, case, count);
    if observed.result != reference.result {
        return Err(format!(
            "{name}: outcome {:?}, bare {:?}",
            observed.result, reference.result
        ));
    }
    if observed != reference {
        return Err(format!(
            "{name}: the set differs from the bare run in registers or memory"
        ));
    }
    if after.x87 != before.x87 {
        return Err(format!("{name}: an SSE form changed the x87 state"));
    }
    match expect {
        Some(expect) if reference.result != expect => Err(format!(
            "{name}: the bare reference {:?} is not the intended {expect:?}",
            reference.result
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// S2-39n: the register moves, the integer SSE2 forms, MOVSS/MOVSD, LDMXCSR/STMXCSR and the CPL3-handler forms in
// the CPL0 set. The reference of every form that the plain step retires is the plain step; the gates are asserted
// directly (the FP-init CR0/CR4 pair refuses these forms).
// ---------------------------------------------------------------------------------------------------------------

/// `[prefix] [REX] 0F opcode modrm [imm8]` with `reg` in ModRM.reg and `rm` in ModRM.rm (register direct); `ext`
/// replaces the reg field for the `71/72/73 /n` shift-immediate groups (the register is then `rm`).
fn encode(prefix: &[u8], opcode: u8, ext: Option<u8>, reg: u8, rm: u8, imm: Option<u8>) -> Vec<u8> {
    let reg_field = ext.unwrap_or(reg);
    let rex = 0x40 | if ext.is_none() { (reg >> 3) << 2 } else { 0 } | (rm >> 3);
    let mut code = prefix.to_vec();
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend_from_slice(&[0x0f, opcode, 0xc0 | (reg_field & 7) << 3 | (rm & 7)]);
    code.extend(imm);
    code
}

/// (name, mandatory prefix, opcode after 0F, group extension, imm8). The register moves and MOVSS/MOVSD first.
const REGISTER_FORMS: &[(&str, &[u8], u8, Option<u8>, Option<u8>)] = &[
    ("MOVUPS load", &[], 0x10, None, None),
    ("MOVUPS store", &[], 0x11, None, None),
    ("MOVAPS load", &[], 0x28, None, None),
    ("MOVAPS store", &[], 0x29, None, None),
    ("MOVUPD load", &[0x66], 0x10, None, None),
    ("MOVUPD store", &[0x66], 0x11, None, None),
    ("MOVAPD load", &[0x66], 0x28, None, None),
    ("MOVAPD store", &[0x66], 0x29, None, None),
    ("MOVDQA load", &[0x66], 0x6f, None, None),
    ("MOVSS load", &[0xf3], 0x10, None, None),
    ("MOVSS store", &[0xf3], 0x11, None, None),
    ("MOVSD load", &[0xf2], 0x10, None, None),
    ("MOVSD store", &[0xf2], 0x11, None, None),
    ("PUNPCKLBW", &[0x66], 0x60, None, None),
    ("PUNPCKLWD", &[0x66], 0x61, None, None),
    ("PUNPCKLDQ", &[0x66], 0x62, None, None),
    ("PACKSSWB", &[0x66], 0x63, None, None),
    ("PCMPGTB", &[0x66], 0x64, None, None),
    ("PCMPGTW", &[0x66], 0x65, None, None),
    ("PCMPGTD", &[0x66], 0x66, None, None),
    ("PACKUSWB", &[0x66], 0x67, None, None),
    ("PUNPCKHBW", &[0x66], 0x68, None, None),
    ("PUNPCKHWD", &[0x66], 0x69, None, None),
    ("PUNPCKHDQ", &[0x66], 0x6a, None, None),
    ("PACKSSDW", &[0x66], 0x6b, None, None),
    ("PUNPCKLQDQ", &[0x66], 0x6c, None, None),
    ("PUNPCKHQDQ", &[0x66], 0x6d, None, None),
    ("PSHUFD 0x1b", &[0x66], 0x70, None, Some(0x1b)),
    ("PSHUFD 0xe4", &[0x66], 0x70, None, Some(0xe4)),
    ("PSHUFD 0x00", &[0x66], 0x70, None, Some(0x00)),
    ("PSRLW imm 3", &[0x66], 0x71, Some(2), Some(3)),
    ("PSRAW imm 4", &[0x66], 0x71, Some(4), Some(4)),
    ("PSLLW imm 5", &[0x66], 0x71, Some(6), Some(5)),
    ("PSLLW imm 16", &[0x66], 0x71, Some(6), Some(16)),
    ("PSRLD imm 7", &[0x66], 0x72, Some(2), Some(7)),
    ("PSRAD imm 8", &[0x66], 0x72, Some(4), Some(8)),
    ("PSLLD imm 9", &[0x66], 0x72, Some(6), Some(9)),
    ("PSRLQ imm 11", &[0x66], 0x73, Some(2), Some(11)),
    ("PSRLDQ imm 5", &[0x66], 0x73, Some(3), Some(5)),
    ("PSLLQ imm 13", &[0x66], 0x73, Some(6), Some(13)),
    ("PSLLDQ imm 7", &[0x66], 0x73, Some(7), Some(7)),
    ("PSLLDQ imm 16", &[0x66], 0x73, Some(7), Some(16)),
    ("PCMPEQB", &[0x66], 0x74, None, None),
    ("PCMPEQW", &[0x66], 0x75, None, None),
    ("PCMPEQD", &[0x66], 0x76, None, None),
    ("PMOVMSKB", &[0x66], 0xd7, None, None),
    ("PADDQ", &[0x66], 0xd4, None, None),
    ("PMULLW", &[0x66], 0xd5, None, None),
    ("PSUBUSB", &[0x66], 0xd8, None, None),
    ("PSUBUSW", &[0x66], 0xd9, None, None),
    ("PMINUB", &[0x66], 0xda, None, None),
    ("PAND", &[0x66], 0xdb, None, None),
    ("PADDUSB", &[0x66], 0xdc, None, None),
    ("PADDUSW", &[0x66], 0xdd, None, None),
    ("PMAXUB", &[0x66], 0xde, None, None),
    ("PANDN", &[0x66], 0xdf, None, None),
    ("PAVGB", &[0x66], 0xe0, None, None),
    ("PAVGW", &[0x66], 0xe3, None, None),
    ("PMULHUW", &[0x66], 0xe4, None, None),
    ("PSUBSB", &[0x66], 0xe8, None, None),
    ("PSUBSW", &[0x66], 0xe9, None, None),
    ("PMINSW", &[0x66], 0xea, None, None),
    ("POR", &[0x66], 0xeb, None, None),
    ("PADDSB", &[0x66], 0xec, None, None),
    ("PADDSW", &[0x66], 0xed, None, None),
    ("PMAXSW", &[0x66], 0xee, None, None),
    ("PXOR", &[0x66], 0xef, None, None),
    ("PMULUDQ", &[0x66], 0xf4, None, None),
    ("PMADDWD", &[0x66], 0xf5, None, None),
    ("PSADBW", &[0x66], 0xf6, None, None),
    ("PSUBB", &[0x66], 0xf8, None, None),
    ("PSUBW", &[0x66], 0xf9, None, None),
    ("PSUBD", &[0x66], 0xfa, None, None),
    ("PSUBQ", &[0x66], 0xfb, None, None),
    ("PADDB", &[0x66], 0xfc, None, None),
    ("PADDW", &[0x66], 0xfd, None, None),
    ("PADDD", &[0x66], 0xfe, None, None),
];

const REGISTER_PAIRS: &[(u8, u8)] = &[
    (1, 2),
    (0, 0),
    (3, 3),
    (8, 9),
    (15, 0),
    (7, 12),
    (9, 9),
    (2, 1),
];

/// The reference outcome of the plain step for a register form, with the case adjusted so that PCMPEQ* sees equal
/// and unequal lanes.
fn register_cases() -> Vec<Case> {
    let mut cases = vec![Case::new(false), Case::new(true)];
    let mut equal = Case::new(false);
    equal.xmm[2] = equal.xmm[1];
    equal.xmm[9] = equal.xmm[8];
    cases.push(equal);
    let mut ones = Case::new(false);
    ones.xmm[1] = [0xff; 16];
    ones.xmm[2] = [0x80; 16];
    ones.xmm[9] = [0x7f; 16];
    cases.push(ones);
    cases
}

#[test]
fn register_moves_and_integer_sse2_forms_run_in_the_set_like_the_plain_step() {
    let mut retired = 0usize;
    let mut refused = Vec::new();
    let mut failures = Vec::new();
    for (name, prefix, opcode, ext, imm) in REGISTER_FORMS {
        for (reg, rm) in REGISTER_PAIRS {
            // PMOVMSKB writes a general register (ModRM.reg), the others XMM registers.
            let code = encode(prefix, *opcode, *ext, *reg, *rm, *imm);
            for case in register_cases() {
                let reference = bare(&code, &case, false);
                match reference.result {
                    Result_::Retired => retired += 1,
                    _ => refused.push(format!("{name} {code:02x?}")),
                }
                if let Err(message) = compare(name, &code, &case, false, None) {
                    failures.push(format!("{code:02x?}: {message}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    refused.sort();
    refused.dedup();
    assert_eq!(
        refused,
        Vec::<String>::new(),
        "forms the plain step does not retire"
    );
    // 75 forms x 8 register pairs x 4 operand patterns.
    assert_eq!(retired, REGISTER_FORMS.len() * REGISTER_PAIRS.len() * 4);
}

fn fault(vector: u8) -> Result_ {
    Result_::Fault(ArchitecturalFault {
        vector,
        error_code: None,
        linear_address: None,
    })
}

fn gp0() -> Result_ {
    Result_::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

/// `in_set_program` with a change of the initial CPU state (a base register, say).
fn in_set_with(
    code: &[u8],
    case: &Case,
    count: u64,
    change: impl FnOnce(&mut LongModeState),
) -> (Observed, GuestFpState, GuestFpState) {
    let mut state = initial_state(case);
    change(&mut state);
    let mut ram = initial_memory(code, case);
    let cell = cell_for(case);
    let before = cell.borrow().clone();
    let (exit, executed) = run_in_set(&mut state, &mut ram, &cell, count);
    let result = match exit {
        RunExit::InstructionBudgetExhausted => {
            assert_eq!(executed, count);
            Result_::Retired
        }
        RunExit::Fault(fault) => Result_::Fault(fault),
        RunExit::Unsupported { opcode } => Result_::Unsupported(opcode),
        RunExit::MemoryError(error) => Result_::Memory(error),
        other => panic!("unexpected set exit {other:?}"),
    };
    assert!(state.sse.is_none() && state.x87.is_none());
    let after = cell.borrow().clone();
    (
        Observed {
            result,
            rip: state.rip,
            rflags: state.rflags,
            gpr: state.gpr,
            xmm: after.sse.xmm,
            mxcsr: after.sse.mxcsr,
            data: ram.bytes[DATA as usize..DATA as usize + 0x500].to_vec(),
        },
        before,
        after,
    )
}

/// Everything but the named XMM register, the data bytes and the registers in `rip`/`gpr` is the initial state.
fn expect_unchanged_except(
    observed: &Observed,
    before: &GuestFpState,
    case: &Case,
    code_len: usize,
) {
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.rip, CODE + code_len as u64);
    assert_eq!(observed.rflags, 2 | (1 << 9) | 0x85, "no flag changes");
    assert_eq!(observed.mxcsr, case.mxcsr);
    assert_eq!(observed.mxcsr, before.sse.mxcsr);
}

#[test]
fn movss_and_movsd_memory_forms_load_zero_extend_and_store_exact_widths() {
    let case = Case::new(false);
    let memory = case.memory;
    // (name, code, wide, store, xmm register)
    let forms: &[(&str, &[u8], bool, bool, usize)] = &[
        (
            "MOVSS xmm1,[rax]",
            &[0xf3, 0x0f, 0x10, 0x08],
            false,
            false,
            1,
        ),
        (
            "MOVSS xmm9,[rax]",
            &[0xf3, 0x44, 0x0f, 0x10, 0x08],
            false,
            false,
            9,
        ),
        (
            "MOVSS [rax],xmm1",
            &[0xf3, 0x0f, 0x11, 0x08],
            false,
            true,
            1,
        ),
        (
            "MOVSD xmm1,[rax]",
            &[0xf2, 0x0f, 0x10, 0x08],
            true,
            false,
            1,
        ),
        (
            "MOVSD xmm9,[rax]",
            &[0xf2, 0x44, 0x0f, 0x10, 0x08],
            true,
            false,
            9,
        ),
        ("MOVSD [rax],xmm1", &[0xf2, 0x0f, 0x11, 0x08], true, true, 1),
    ];
    for (name, code, wide, store, register) in forms {
        let width = if *wide { 8 } else { 4 };
        let (observed, before, after) = in_set(code, &case);
        // [rax] is the encoding the plain step retires: it is the reference.
        let reference = bare(code, &case, false);
        assert_eq!(reference.result, Result_::Retired, "{name}");
        assert_eq!(observed, reference, "{name}");
        expect_unchanged_except(&observed, &before, &case, code.len());
        assert_eq!(after.x87, before.x87, "{name}");
        let mut xmm = case.xmm;
        let mut data = vec![SENTINEL; 0x500];
        data[..16].copy_from_slice(&memory);
        if *store {
            data[..width].copy_from_slice(&case.xmm[*register][..width]);
        } else {
            let mut value = [0u8; 16];
            value[..width].copy_from_slice(&memory[..width]);
            xmm[*register] = value;
        }
        assert_eq!(after.sse.xmm, xmm, "{name}");
        assert_eq!(observed.data, data, "{name}");
    }
}

#[test]
fn movsd_rbp_and_rip_relative_forms_the_decoder_returns_run_in_the_set() {
    let case = Case::new(false);
    let memory = case.memory;
    // MOVSD xmm1,[rbp-24]; RBP = DATA + 24.
    let load_rbp = [0xf2, 0x0f, 0x10, 0x4d, 0xe8];
    let (observed, _, after) = in_set_with(&load_rbp, &case, 1, |state| state.gpr[5] = DATA + 24);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.rip, CODE + 5);
    let mut expect = [0u8; 16];
    expect[..8].copy_from_slice(&memory[..8]);
    assert_eq!(after.sse.xmm[1], expect);
    // MOVSD [rbp-24],xmm1.
    let store_rbp = [0xf2, 0x0f, 0x11, 0x4d, 0xe8];
    let (observed, _, _) = in_set_with(&store_rbp, &case, 1, |state| state.gpr[5] = DATA + 24);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.data[..8], case.xmm[1][..8]);
    assert_eq!(
        observed.data[8..16],
        memory[8..16],
        "only the low qword is stored"
    );
    // MOVSD xmm1,[rip+disp32] (a load form the decoder returns; there is no RIP-relative store form).
    let disp = (DATA as i64 - (CODE as i64 + 8)) as i32;
    let mut code = vec![0xf2, 0x0f, 0x10, 0x0d];
    code.extend_from_slice(&disp.to_le_bytes());
    let (observed, _, after) = in_set(&code, &case);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.xmm[1][..8], memory[..8]);
    assert_eq!(after.sse.xmm[1][8..], [0u8; 8]);
    // S7-32 W2: the other addressing forms of MOVSS/MOVSD memory operands decode through the scalar
    // fallback and run in the set (SDM Vol. 2B MOVSD/MOVSS): a load takes the low 64 (32) bits and
    // zeroes the rest of the register, a store writes 8 (4) bytes. RAX = RBX = DATA.
    let memory = case.memory;
    for (name, code, load) in [
        (
            "MOVSD xmm1,[rax+8]",
            &[0xf2u8, 0x0f, 0x10, 0x48, 0x08][..],
            Some(8),
        ),
        ("MOVSD [rax+8],xmm1", &[0xf2, 0x0f, 0x11, 0x48, 0x08], None),
        (
            "MOVSS xmm1,[rax+8]",
            &[0xf3, 0x0f, 0x10, 0x48, 0x08],
            Some(4),
        ),
        ("MOVSS [rax+8],xmm1", &[0xf3, 0x0f, 0x11, 0x48, 0x08], None),
        ("MOVSS xmm1,[rbx]", &[0xf3, 0x0f, 0x10, 0x0b], Some(4)),
    ] {
        let (observed, before, after) = in_set(code, &case);
        assert_eq!(observed.result, Result_::Retired, "{name}");
        assert_eq!(observed.rip, CODE + code.len() as u64, "{name}");
        let offset = if code[3] == 0x0b { 0 } else { 8 };
        match load {
            Some(width) => {
                let mut expected = [0u8; 16];
                expected[..width].copy_from_slice(&memory[offset..offset + width]);
                assert_eq!(after.sse.xmm[1], expected, "{name}");
            }
            None => {
                let width = if code[0] == 0xf2 { 8 } else { 4 };
                assert_eq!(
                    &observed.data[offset..offset + width],
                    &before.sse.xmm[1][..width],
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn movss_movsd_register_forms_merge_the_low_lanes_and_keep_the_rest() {
    let case = Case::new(false);
    // MOVSS xmm1,xmm2 keeps xmm1[4..16]; MOVSD keeps xmm1[8..16]; the store direction (11) writes the rm register.
    for (name, code, width, dst, src) in [
        (
            "MOVSS xmm1,xmm2",
            [0xf3u8, 0x0f, 0x10, 0xca],
            4usize,
            1usize,
            2usize,
        ),
        ("MOVSD xmm1,xmm2", [0xf2, 0x0f, 0x10, 0xca], 8, 1, 2),
        ("MOVSS xmm2,xmm1 (11)", [0xf3, 0x0f, 0x11, 0xca], 4, 2, 1),
        ("MOVSD xmm2,xmm1 (11)", [0xf2, 0x0f, 0x11, 0xca], 8, 2, 1),
    ] {
        let (observed, _, after) = in_set(&code, &case);
        assert_eq!(observed.result, Result_::Retired, "{name}");
        let mut xmm = case.xmm;
        xmm[dst][..width].copy_from_slice(&case.xmm[src][..width]);
        assert_eq!(after.sse.xmm, xmm, "{name}");
        assert_eq!(observed, bare(&code, &case, false), "{name}");
    }
}

/// Forms of the new slice that touch memory or not, with their instruction bytes ([RAX]-addressed).
const GATED_FORMS: &[(&str, &[u8])] = &[
    ("MOVAPS xmm1,xmm2", &[0x0f, 0x28, 0xca]),
    ("MOVUPD xmm2,xmm1 (11)", &[0x66, 0x0f, 0x11, 0xca]),
    ("MOVDQA xmm1,xmm2", &[0x66, 0x0f, 0x6f, 0xca]),
    ("MOVSS xmm1,xmm2", &[0xf3, 0x0f, 0x10, 0xca]),
    ("MOVSD xmm1,[rax]", &[0xf2, 0x0f, 0x10, 0x08]),
    ("MOVSS [rax],xmm1", &[0xf3, 0x0f, 0x11, 0x08]),
    ("PXOR xmm1,xmm2", &[0x66, 0x0f, 0xef, 0xca]),
    ("PADDD xmm1,xmm2", &[0x66, 0x0f, 0xfe, 0xca]),
    ("PSHUFD xmm1,xmm2,0x1b", &[0x66, 0x0f, 0x70, 0xca, 0x1b]),
    ("PSLLW xmm1,3", &[0x66, 0x0f, 0x71, 0xf1, 0x03]),
    ("PMOVMSKB eax,xmm2", &[0x66, 0x0f, 0xd7, 0xc2]),
    ("LDMXCSR [rax]", &[0x0f, 0xae, 0x10]),
    ("STMXCSR [rax]", &[0x0f, 0xae, 0x18]),
    ("MOVD xmm1,ecx", &[0x66, 0x0f, 0x6e, 0xc9]),
    ("PSHUFLW xmm1,xmm2,0x1b", &[0xf2, 0x0f, 0x70, 0xca, 0x1b]),
    ("MOVLPD xmm1,[rax]", &[0x66, 0x0f, 0x12, 0x08]),
    ("PCMPEQB xmm1,[rax]", &[0x66, 0x0f, 0x74, 0x08]),
    ("PINSRW xmm1,[rax],5", &[0x66, 0x0f, 0xc4, 0x08, 0x05]),
    ("PMINUB xmm0,[rax+0x10]", &[0x66, 0x0f, 0xda, 0x40, 0x10]),
];

#[test]
fn the_new_forms_are_gated_by_cr0_em_ts_and_cr4_osfxsr_before_any_operand_access() {
    // (CR0, CR4, vector): EM and a clear OSFXSR are #UD, TS alone is #NM, and #UD wins over #NM.
    let gates = [
        (CR0_BASE | CR0_EM, CR4_FP, 6),
        (CR0_BASE | CR0_TS, CR4_FP, 7),
        (CR0_BASE | CR0_EM | CR0_TS, CR4_FP, 6),
        (CR0_BASE, CR4_BASE | CR4_OSXMMEXCPT, 6),
        (CR0_BASE | CR0_TS, CR4_BASE, 6),
    ];
    for (cr0, cr4, vector) in gates {
        // A misaligned, a non-canonical and an unmapped [RAX] must not matter: the gate comes first.
        for rax in [DATA, DATA + 1, 0x0001_0000_0000_0000, 0x10_0000] {
            let mut case = Case::new(true);
            case.cr0 = cr0;
            case.cr4 = cr4;
            case.rax = rax;
            for (name, code) in GATED_FORMS {
                let (observed, before, after) = in_set(code, &case);
                // S7-31i (main priority): LDMXCSR is `Unsupported` for CR0.EM = 1 or CR4.OSFXSR = 0 (where the
                // other forms are #UD); CR0.TS alone is #NM for it too.
                let expected = if *name == "LDMXCSR [rax]" && vector == 6 {
                    Result_::Unsupported(0xae)
                } else {
                    fault(vector)
                };
                assert_eq!(
                    observed.result, expected,
                    "{name}: cr0 {cr0:#x} cr4 {cr4:#x} rax {rax:#x}"
                );
                assert_eq!(
                    after, before,
                    "{name}: a gated form changes no SSE or x87 state"
                );
                assert_eq!(observed.rip, CODE, "{name}");
                assert_eq!(observed.mxcsr, case.mxcsr, "{name}");
            }
        }
    }
}

#[test]
fn memory_operand_errors_of_the_new_memory_forms_change_nothing() {
    for (name, code) in [
        ("MOVSS xmm1,[rax]", &[0xf3, 0x0f, 0x10, 0x08][..]),
        ("MOVSD xmm1,[rax]", &[0xf2, 0x0f, 0x10, 0x08]),
        ("LDMXCSR [rax]", &[0x0f, 0xae, 0x10]),
        ("MOVLPD xmm1,[rax]", &[0x66, 0x0f, 0x12, 0x08]),
        ("PINSRW xmm1,[rax],5", &[0x66, 0x0f, 0xc4, 0x08, 0x05]),
        ("PMINUB xmm0,[rax+0x10]", &[0x66, 0x0f, 0xda, 0x40, 0x10]),
    ] {
        // A non-canonical address is #GP(0); an unmapped one is the backend's error.
        let mut case = Case::new(true);
        case.rax = 0x0001_0000_0000_0000;
        let (observed, before, after) = in_set(code, &case);
        assert_eq!(observed.result, gp0(), "{name}");
        assert_eq!((after, observed.rip), (before, CODE), "{name}");
        case.rax = 0x10_0000;
        let (observed, before, after) = in_set(code, &case);
        assert!(matches!(observed.result, Result_::Memory(_)), "{name}");
        assert_eq!((after, observed.rip), (before, CODE), "{name}");
    }
    // A store to an unmapped address is the backend's error too, and MXCSR stays.
    let mut case = Case::new(true);
    case.rax = 0x10_0000;
    for code in [
        &[0xf3u8, 0x0f, 0x11, 0x08][..],
        &[0xf2, 0x0f, 0x11, 0x08],
        &[0x0f, 0xae, 0x18],
    ] {
        let (observed, before, after) = in_set(code, &case);
        assert!(matches!(observed.result, Result_::Memory(_)), "{code:02x?}");
        assert_eq!(after, before);
    }
}

/// A case whose first dword of data is `value` (the source of LDMXCSR) and whose MXCSR is `mxcsr`.
fn mxcsr_case(value: u32, mxcsr: u32) -> Case {
    let mut case = Case::new(false);
    case.memory[..4].copy_from_slice(&value.to_le_bytes());
    case.mxcsr = mxcsr;
    case
}

const FLAGS: u64 = 2 | (1 << 9) | 0x85;

fn assert_untouched_except_mxcsr(
    name: &str,
    observed: &Observed,
    before: &GuestFpState,
    case: &Case,
) {
    assert_eq!(observed.rflags, FLAGS, "{name}: no flag changes");
    assert_eq!(
        observed.gpr,
        initial_state(case).gpr,
        "{name}: no register changes"
    );
    assert_eq!(observed.xmm, before.sse.xmm, "{name}: no XMM changes");
    let mut data = vec![SENTINEL; 0x500];
    data[..16].copy_from_slice(&case.memory);
    assert_eq!(
        observed.data, data,
        "{name}: the memory operand is only read"
    );
}

#[test]
fn ldmxcsr_loads_every_valid_value_and_changes_nothing_else() {
    for value in [
        0x1f80u32,
        0x0000,
        0x1fa5,
        0xffbf,
        0x9fbf,
        0x003f,
        0x6000,
        0x8000,
        0x1f80 | 0x3f,
    ] {
        for old in [0x1f80u32, 0x1fa5, 0x0000] {
            let case = mxcsr_case(value, old);
            let code = [0x0f, 0xae, 0x10];
            let (observed, before, after) = in_set(&code, &case);
            let name = format!("LDMXCSR {value:#x} over {old:#x}");
            assert_eq!(observed.result, Result_::Retired, "{name}");
            assert_eq!(observed.rip, CODE + 3, "{name}");
            assert_eq!(after.sse.mxcsr, value, "{name}");
            assert_eq!(after.x87, before.x87, "{name}");
            assert_untouched_except_mxcsr(&name, &observed, &before, &case);
        }
    }
}

#[test]
fn ldmxcsr_with_a_reserved_bit_is_gp0_and_changes_nothing() {
    // TARGET_FXSAVE_MXCSR_MASK = 0xffbf (S7-31i, "main priority" rule of merge 56c60c9; ADR-0014 superseded):
    // bits 31:16 and DAZ (bit 6) are reserved. The i5-3210M accepts DAZ (S2-39p, KNOWN_ISSUES ISSUE-680), the
    // model refuses it by the owner's decision.
    for value in [
        0x0040u32,
        0x1fc0,
        0x9fc0,
        0xffff,
        0x8040,
        0x1f40,
        0x0001_1f80,
        0x8000_1f80,
        0xffff_ffff,
        0x0001_ffff,
        0x0002_0000,
        0x1f80 | (1 << 20),
    ] {
        let case = mxcsr_case(value, 0x1f80);
        let (observed, before, after) = in_set(&[0x0f, 0xae, 0x10], &case);
        assert_eq!(observed.result, gp0(), "{value:#x}");
        assert_eq!(observed.rip, CODE, "{value:#x}");
        assert_eq!(after, before, "{value:#x}");
        assert_untouched_except_mxcsr(&format!("{value:#x}"), &observed, &before, &case);
    }
}

#[test]
fn ldmxcsr_and_stmxcsr_take_every_addressing_form_with_an_unaligned_operand() {
    // 0x1fa5 and values of the target mask 0xffbf (the DAZ values of the physical oracle are #GP here, see the
    // reserved-bit test and ISSUE-680).
    for value in [0x1fa5u32, 0x003f, 0x9fbf, 0xffbf, 0x6000, 0x1f80] {
        let mut case = mxcsr_case(value, 0x1f80);
        // The operand is unaligned at DATA + 1 (a 16-byte memory image holds the dword at offset 1).
        case.memory = [0; 16];
        case.memory[1..5].copy_from_slice(&value.to_le_bytes());
        let forms: &[(&str, &[u8], fn(&mut LongModeState))] = &[
            ("[rax+1]", &[0x0f, 0xae, 0x50, 0x01], |_| {}),
            ("[r8+1]", &[0x41, 0x0f, 0xae, 0x50, 0x01], |s| {
                s.gpr[8] = DATA
            }),
            ("[rax+rcx*2+1]", &[0x0f, 0xae, 0x54, 0x48, 0x01], |s| {
                s.gpr[0] = DATA - 8;
                s.gpr[1] = 4;
            }),
            (
                "[rbp+0x101]",
                &[0x0f, 0xae, 0x95, 0x01, 0x01, 0x00, 0x00],
                |s| s.gpr[5] = DATA - 0x100,
            ),
            ("[eax+1] 67", &[0x67, 0x0f, 0xae, 0x50, 0x01], |_| {}),
            ("ds:[rax+1]", &[0x3e, 0x0f, 0xae, 0x50, 0x01], |_| {}),
        ];
        for (name, code, change) in forms {
            let (observed, before, after) = in_set_with(code, &case, 1, |state| change(state));
            // S7-31i (main priority): the `LdmxcsrMemory` executor refuses a REX, a 67 and a segment prefix
            // (`Unsupported`, nothing changes); only the unprefixed forms with a 64-bit address run.
            if matches!(*name, "[r8+1]" | "[eax+1] 67" | "ds:[rax+1]") {
                assert_eq!(observed.result, Result_::Unsupported(0xae), "{name}");
                assert_eq!(observed.rip, CODE, "{name}");
                assert_eq!(after, before, "{name}");
                continue;
            }
            assert_eq!(observed.result, Result_::Retired, "{name}");
            assert_eq!(observed.rip, CODE + code.len() as u64, "{name}");
            assert_eq!(after.sse.mxcsr, value, "{name}");
            assert_eq!(observed.xmm, before.sse.xmm, "{name}");
        }
        // [RIP+disp32] to DATA + 1.
        let disp = (DATA as i64 + 1 - (CODE as i64 + 7)) as i32;
        let mut code = vec![0x0f, 0xae, 0x15];
        code.extend_from_slice(&disp.to_le_bytes());
        let (observed, _, after) = in_set(&code, &case);
        assert_eq!(observed.result, Result_::Retired);
        assert_eq!(after.sse.mxcsr, value);
    }
    // STMXCSR [rax+1] writes exactly four bytes there.
    let case = mxcsr_case(0, 0x1f85);
    let (observed, before, after) = in_set(&[0x0f, 0xae, 0x58, 0x01], &case);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(
        after, before,
        "STMXCSR changes neither MXCSR nor the registers"
    );
    assert_eq!(observed.rflags, FLAGS);
    assert_eq!(observed.gpr, initial_state(&case).gpr);
    assert_eq!(observed.data[1..5], 0x1f85u32.to_le_bytes());
    assert_eq!(observed.data[..1], case.memory[..1]);
    assert_eq!(observed.data[5..16], case.memory[5..16], "four bytes only");
    assert_eq!(observed.data[16], SENTINEL);
}

#[test]
fn ldmxcsr_daz_makes_the_next_sse_form_read_a_denormal_source_as_zero() {
    // LDMXCSR [rax], then ADDSS xmm1,xmm2 with xmm1.low = 0x0000_0001 (the smallest binary32 denormal) and
    // xmm2.low = 1.0. DAZ = 1: the denormal is +0, the sum 1.0 is exact and no flag is set (SDM Vol. 1
    // §10.2.3.4). DAZ = 0: DE (denormal operand) and PE (1.0 + 2^-149 rounds to 1.0) are set. The i5-3210M
    // gives 0x3f80_0000 with MXCSR 0x1fc0 and 0x1fa2 for the two loads (ADR-0014). By the "main priority" rule
    // (merge 56c60c9, mask 0xffbf) the DAZ load is `#GP(0)` at the LDMXCSR and nothing changes (ISSUE-680).
    {
        let mut case = mxcsr_case(0x1fc0, 0x1f80);
        case.xmm[1][..4].copy_from_slice(&1u32.to_le_bytes());
        case.xmm[2][..4].copy_from_slice(&0x3f80_0000u32.to_le_bytes());
        let code = [&[0x0f, 0xae, 0x10][..], &[0xf3, 0x0f, 0x58, 0xca][..]].concat();
        let (observed, before, after) = in_set_program(&code, &case, 2);
        assert_eq!(observed.result, gp0());
        assert_eq!(observed.rip, CODE);
        assert_eq!(after, before);
    }
    for (loaded, expected_mxcsr) in [(0x1f80, 0x1fa2u32)] {
        let mut case = mxcsr_case(loaded, 0x1f80);
        case.xmm[1][..4].copy_from_slice(&1u32.to_le_bytes());
        case.xmm[2][..4].copy_from_slice(&0x3f80_0000u32.to_le_bytes());
        let code = [&[0x0f, 0xae, 0x10][..], &[0xf3, 0x0f, 0x58, 0xca][..]].concat();
        let (observed, _, after) = in_set_program(&code, &case, 2);
        assert_eq!(observed.result, Result_::Retired, "{loaded:#x}");
        assert_eq!(observed.rip, CODE + 7, "{loaded:#x}");
        assert_eq!(after.sse.mxcsr, expected_mxcsr, "{loaded:#x}");
        assert_eq!(
            after.sse.xmm[1][..4],
            0x3f80_0000u32.to_le_bytes(),
            "{loaded:#x}"
        );
        assert_eq!(after.sse.xmm[1][4..], case.xmm[1][4..], "{loaded:#x}");
    }
}

#[test]
fn stmxcsr_stores_the_live_value_including_flags_of_the_previous_form_and_the_load_changes_the_next_form()
 {
    // DIVPS with a zero divisor sets ZE (masked); STMXCSR then stores 0x1f84.
    let mut case = Case::new(true);
    case.xmm[2] = [0; 16];
    let code = [&[0x0f, 0x5e, 0xca][..], &[0x0f, 0xae, 0x18][..]].concat();
    let (observed, _, after) = in_set_program(&code, &case, 2);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.data[..4], (MXCSR_MASKED | 1 << 2).to_le_bytes());
    assert_eq!(after.sse.mxcsr, MXCSR_MASKED | 1 << 2);
    // LDMXCSR with the divide-by-zero mask clear makes the same DIVPS #XM (CR4.OSXMMEXCPT = 1) at its own RIP.
    let mut case = mxcsr_case(MXCSR_MASKED & !(1 << 9), MXCSR_MASKED);
    case.cr4 = CR4_FP;
    case.xmm[2] = [0; 16];
    let code = [&[0x0f, 0xae, 0x10][..], &[0x0f, 0x5e, 0xca][..]].concat();
    let (observed, before, after) = in_set_program(&code, &case, 2);
    assert_eq!(observed.result, fault(19));
    assert_eq!(observed.rip, CODE + 3);
    assert_eq!(
        after.sse.mxcsr,
        (MXCSR_MASKED & !(1 << 9)) | 1 << 2,
        "ZE is detected"
    );
    assert_eq!(
        after.sse.xmm, before.sse.xmm,
        "the destination of the faulting form is unchanged"
    );
    // The same pair with OSXMMEXCPT clear is #UD.
    case.cr4 = CR4_BASE | CR4_OSFXSR;
    let (observed, _, _) = in_set_program(&code, &case, 2);
    assert_eq!(observed.result, fault(6));
    // LDMXCSR with the 0x1f80 restores the masks: the same DIVPS retires.
    let mut case = mxcsr_case(MXCSR_MASKED, MXCSR_MASKED & !(1 << 9));
    case.xmm[2] = [0; 16];
    let (observed, _, after) = in_set_program(&code, &case, 2);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.mxcsr, MXCSR_MASKED | 1 << 2);
}

#[test]
fn movss_movsd_memory_forms_with_an_fs_or_gs_override_are_refused() {
    let case = Case::new(false);
    for (name, code, opcode) in [
        (
            "FS: MOVSS xmm1,[rax]",
            &[0x64u8, 0xf3, 0x0f, 0x10, 0x08][..],
            0x10,
        ),
        (
            "GS: MOVSD xmm1,[rax]",
            &[0x65, 0xf2, 0x0f, 0x10, 0x08],
            0x10,
        ),
        (
            "FS: MOVSS [rax],xmm1",
            &[0x64, 0xf3, 0x0f, 0x11, 0x08],
            0x11,
        ),
        (
            "GS: MOVSD [rax],xmm1",
            &[0x65, 0xf2, 0x0f, 0x11, 0x08],
            0x11,
        ),
    ] {
        let (observed, before, after) = in_set(code, &case);
        assert_eq!(observed.result, Result_::Unsupported(opcode), "{name}");
        assert_eq!((after, observed.rip), (before, CODE), "{name}");
    }
}

#[test]
fn ldmxcsr_stmxcsr_forms_outside_the_decoded_set_keep_their_opcode_refusal() {
    let case = mxcsr_case(MXCSR_MASKED, 0x1fa5);
    for (name, code) in [
        ("66 LDMXCSR", &[0x66, 0x0f, 0xae, 0x10][..]),
        ("REX.W LDMXCSR", &[0x48, 0x0f, 0xae, 0x10]),
        ("REX.W STMXCSR", &[0x48, 0x0f, 0xae, 0x18]),
        ("F3 LDMXCSR", &[0xf3, 0x0f, 0xae, 0x10]),
        ("F2 STMXCSR", &[0xf2, 0x0f, 0xae, 0x18]),
        ("LDMXCSR mod 11", &[0x0f, 0xae, 0xd0]),
        ("STMXCSR mod 11", &[0x0f, 0xae, 0xd8]),
        ("FS: LDMXCSR", &[0x64, 0x0f, 0xae, 0x10]),
        ("GS: STMXCSR", &[0x65, 0x0f, 0xae, 0x18]),
    ] {
        let (observed, before, after) = in_set(code, &case);
        assert_eq!(observed.result, Result_::Unsupported(0xae), "{name}");
        assert_eq!(after, before, "{name}");
        assert_eq!(observed.rip, CODE, "{name}");
    }
}

fn with_regs(
    case: &Case,
    code: &[u8],
    regs: &[(usize, u64)],
) -> (Observed, GuestFpState, GuestFpState) {
    in_set_with(code, case, 1, |state| {
        for (index, value) in regs {
            state.gpr[*index] = *value;
        }
    })
}

#[test]
fn forms_with_a_handler_of_the_cpl3_profile_only_run_in_the_set_with_the_same_results() {
    let case = Case::new(false);
    let memory = case.memory;
    let initial = initial_state(&case);
    let mut expected_data = vec![SENTINEL; 0x500];
    expected_data[..16].copy_from_slice(&memory);
    let quad = |bytes: &[u8]| -> [u8; 16] {
        let mut value = [0u8; 16];
        value[..bytes.len()].copy_from_slice(bytes);
        value
    };
    // MOVD xmm1,edx: zero-extended to 128 bits.
    let (observed, _, after) = with_regs(&case, &[0x66, 0x0f, 0x6e, 0xca], &[]);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(
        after.sse.xmm[1],
        quad(&(initial.gpr[2] as u32).to_le_bytes())
    );
    assert_eq!(observed.rip, CODE + 4);
    // MOVQ xmm1,rax (REX.W).
    let (observed, _, after) = with_regs(&case, &[0x66, 0x48, 0x0f, 0x6e, 0xc8], &[]);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.xmm[1], quad(&DATA.to_le_bytes()));
    // MOVQ xmm0,[rsi] (F3 0F 7E): low qword loaded, high qword zero.
    let (observed, _, after) = with_regs(&case, &[0xf3, 0x0f, 0x7e, 0x06], &[(6, DATA)]);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.xmm[0], quad(&memory[..8]));
    // MOVQ [rdi],xmm1 (66 0F D6): the low qword is stored.
    let (observed, _, after) = with_regs(&case, &[0x66, 0x0f, 0xd6, 0x0f], &[(7, DATA + 8)]);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.data[8..16], case.xmm[1][..8]);
    assert_eq!(observed.data[..8], memory[..8]);
    assert_eq!(after.sse.xmm, case.xmm);
    // PSHUFLW xmm0,xmm1,0x1b: the low words of the source in reverse order, the high qword of the source.
    let (observed, _, after) = with_regs(&case, &[0xf2, 0x0f, 0x70, 0xc1, 0x1b], &[]);
    assert_eq!(observed.result, Result_::Retired);
    let source = case.xmm[1];
    let mut expect = source;
    for lane in 0..4 {
        let from = 3 - lane;
        expect[lane * 2..lane * 2 + 2].copy_from_slice(&source[from * 2..from * 2 + 2]);
    }
    assert_eq!(after.sse.xmm[0], expect);
    // MOVLPD xmm1,[rdi] at an unaligned address replaces the low qword only.
    let mut case2 = case.clone();
    case2.memory = core::array::from_fn(|i| 0x40 + i as u8);
    let (observed, _, after) = with_regs(&case2, &[0x66, 0x0f, 0x12, 0x0f], &[(7, DATA + 1)]);
    assert_eq!(observed.result, Result_::Retired);
    let mut expect = case2.xmm[1];
    expect[..8].copy_from_slice(&[0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]);
    assert_eq!(after.sse.xmm[1], expect);
    // MOVLPD xmm9,[r8] reaches xmm9 and r8.
    let (observed, _, after) = with_regs(&case2, &[0x66, 0x45, 0x0f, 0x12, 0x08], &[(8, DATA)]);
    assert_eq!(observed.result, Result_::Retired);
    let mut expect = case2.xmm[9];
    expect[..8].copy_from_slice(&case2.memory[..8]);
    assert_eq!(after.sse.xmm[9], expect);
    // MOVHPS and MOVHPD xmm1,[rdi]: the high qword.
    for code in [&[0x0f, 0x16, 0x0fu8][..], &[0x66, 0x0f, 0x16, 0x0f]] {
        let (observed, _, after) = with_regs(&case2, code, &[(7, DATA)]);
        assert_eq!(observed.result, Result_::Retired, "{code:02x?}");
        let mut expect = case2.xmm[1];
        expect[8..].copy_from_slice(&case2.memory[..8]);
        assert_eq!(after.sse.xmm[1], expect, "{code:02x?}");
    }
    // PCMPEQB xmm1,[rdi] (aligned): 0xff where the bytes are equal.
    let mut case3 = case.clone();
    case3.xmm[1] = case3.memory;
    case3.xmm[1][3] ^= 1;
    case3.xmm[1][15] ^= 0x80;
    let (observed, _, after) = with_regs(&case3, &[0x66, 0x0f, 0x74, 0x0f], &[(7, DATA)]);
    assert_eq!(observed.result, Result_::Retired);
    let expect: [u8; 16] = core::array::from_fn(|i| if i == 3 || i == 15 { 0 } else { 0xff });
    assert_eq!(after.sse.xmm[1], expect);
    // ... and the same form at an unaligned address is #GP(0) with nothing changed.
    let (observed, before, after) = with_regs(&case3, &[0x66, 0x0f, 0x74, 0x0f], &[(7, DATA + 4)]);
    assert_eq!(observed.result, gp0());
    assert_eq!((after, observed.rip), (before, CODE));
    // PINSRW xmm1,[rax],imm8 (66 0F C4 /r m16, imm8): the word at an unaligned address replaces lane imm8 & 7 only.
    // imm8 = 0x0d selects lane 5, so the test also covers that the lane number is taken modulo 8.
    let (observed, _, after) = with_regs(&case2, &[0x66, 0x0f, 0xc4, 0x08, 0x0d], &[(0, DATA + 1)]);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(observed.rip, CODE + 5);
    let mut expect = case2.xmm[1];
    expect[10..12].copy_from_slice(&[0x41, 0x42]);
    assert_eq!(after.sse.xmm[1], expect);
    assert_ne!(expect, case2.xmm[1], "the word must change the lane");
    // ... lane 0 at an aligned address, the destination in ModRM.reg (xmm9 with REX.R).
    let (observed, _, after) = with_regs(&case2, &[0x66, 0x44, 0x0f, 0xc4, 0x08, 0x00], &[]);
    assert_eq!(observed.result, Result_::Retired);
    let mut expect = case2.xmm[9];
    expect[..2].copy_from_slice(&[0x40, 0x41]);
    assert_eq!(after.sse.xmm[9], expect);
    // PMINUB xmm0,[rax+disp8] (66 0F DA /r m128; the decoder returns xmm0 and [RAX+disp8] with disp8 in
    // 0x10, 0x20, 0x30, 0x50, 0x60, 0x70): bytewise unsigned minimum with an aligned operand.
    let mut case4 = case.clone();
    case4.xmm[0] = core::array::from_fn(|i| 0x80u8.wrapping_add((i as u8).wrapping_mul(0x13)));
    case4.memory = core::array::from_fn(|i| 0xa0u8.wrapping_sub((i as u8).wrapping_mul(0x0b)));
    let mut data4 = expected_data.clone();
    data4[..16].copy_from_slice(&case4.memory);
    for disp in [0x10u8, 0x70] {
        let pminub = [0x66, 0x0f, 0xda, 0x40, disp];
        let base = DATA - u64::from(disp);
        let (observed, _, after) = with_regs(&case4, &pminub, &[(0, base)]);
        assert_eq!(observed.result, Result_::Retired, "disp {disp:#x}");
        assert_eq!(observed.rip, CODE + 5);
        let expect: [u8; 16] = core::array::from_fn(|i| case4.xmm[0][i].min(case4.memory[i]));
        assert_ne!(expect, case4.xmm[0], "the operand must lower some byte");
        assert_ne!(expect, case4.memory, "the register must lower some byte");
        assert_eq!(after.sse.xmm[0], expect, "disp {disp:#x}");
        assert_eq!(after.sse.xmm[1], case4.xmm[1], "no other register changes");
        assert_eq!(observed.data, data4, "a load changes no memory");
        // ... the same form at an unaligned address is #GP(0) with nothing changed.
        let (observed, before, after) = with_regs(&case4, &pminub, &[(0, base + 4)]);
        assert_eq!(observed.result, gp0(), "disp {disp:#x}");
        assert_eq!((after, observed.rip), (before, CODE));
    }
    // The data of every form above except the store is unchanged.
    let (observed, _, _) = with_regs(&case, &[0x66, 0x0f, 0x6e, 0xca], &[]);
    assert_eq!(observed.data, expected_data);
}

#[test]
fn a_program_of_the_new_forms_matches_the_plain_step_sequence() {
    // Register forms only (the plain step retires them all); each reads the result of the one before.
    let code: Vec<u8> = [
        vec![0x66, 0x0f, 0xef, 0xca],       // PXOR xmm1,xmm2
        vec![0x66, 0x0f, 0xfe, 0xd1],       // PADDD xmm2,xmm1
        vec![0x66, 0x0f, 0x70, 0xda, 0x1b], // PSHUFD xmm3,xmm2,0x1b
        vec![0x66, 0x0f, 0x60, 0xc3],       // PUNPCKLBW xmm0,xmm3
        vec![0x0f, 0x28, 0xe0],             // MOVAPS xmm4,xmm0
        vec![0xf3, 0x0f, 0x10, 0xe2],       // MOVSS xmm4,xmm2
        vec![0x66, 0x0f, 0x74, 0xc4],       // PCMPEQB xmm0,xmm4
        vec![0x66, 0x0f, 0xd7, 0xc0],       // PMOVMSKB eax,xmm0
        vec![0x66, 0x0f, 0x73, 0xfc, 0x03], // PSLLDQ xmm4,3
        vec![0x66, 0x0f, 0x71, 0xd3, 0x02], // PSRLW xmm3,2
        vec![0xf2, 0x0f, 0x10, 0xeb],       // MOVSD xmm5,xmm3
        vec![0x66, 0x0f, 0xdb, 0xe5],       // PAND xmm4,xmm5
    ]
    .concat();
    for ordinary in [false, true] {
        let case = Case::new(ordinary);
        if let Err(message) =
            compare_program("program", &code, &case, false, 12, Some(Result_::Retired))
        {
            panic!("{message}");
        }
    }
}
