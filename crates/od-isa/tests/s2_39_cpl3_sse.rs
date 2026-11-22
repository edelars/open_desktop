//! S2-39 (CPL3 set): the general SSE/SSE2 forms (S2-34, S2-37, S2-38, S2-54, S2-55 and the S2-39 slices: raw 128-bit
//! moves, MOVSS/MOVSD, LDMXCSR/STMXCSR, the integer SSE2 forms, MOVNT*, MASKMOVDQU) execute at CPL3 of the profile set
//! with the FP member, on `LongModeState.sse`, with the results of the bare run and of the CPL0 set (ADR-0015 item 9).
//!
//! The references are the plain `step` (the S2-34/S2-37/S2-38/S2-54/S2-55 results validated against the physical
//! i5-3210M tables) and the CPL0 set (S2-39m/n/q/r, validated by the physical oracle); no value in this file is model
//! output of the CPL3 set itself, except where a test says that it compares the CPL3 set with the CPL0 set. The CPL3 rules
//! checked here: CR0.EM/TS and CR4.OSFXSR gates before any operand access, #XM/#UD, canonical addresses (#GP, #SS for an
//! RSP/RBP base), the page privilege of the access (a supervisor-only page is a #PF with U/S = 1), the alignment #GP of
//! the aligned forms, and the transfer of #AC (a misaligned operand with RFLAGS.AC = 1 and CR0.AM = 1 is `Unsupported`,
//! nothing changed). MMX and the forms that need the MMX registers stay refused at CPL3.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig,
    RunExit, StepOutcome, run_with_cpl0_profile_set, run_with_cpl3_profile_set,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CodeSegmentDescriptorCache,
    CpuActivity, DataSegmentDescriptorCache, GuestMemory, InstructionMode, LongModeState,
    MemoryError, PrivilegeLevel, StiShadow, cpuid::CpuidProfile, page_fault_error_code,
};
use std::cell::RefCell;

const CODE: u64 = 0x1000;
const DATA: u64 = 0x2000;
const SUPERVISOR_PAGE: u64 = 0x3000;
const RAM: usize = 0x4000;
const SENTINEL: u8 = 0xa5;
const CR0_BASE: u64 = 0x8005_0033;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_AM: u64 = 1 << 18;
const CR4_BASE: u64 = 0x20;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_FP: u64 = CR4_BASE | CR4_OSFXSR | CR4_OSXMMEXCPT;
const MXCSR_MASKED: u32 = 0x1f80;
const RFLAGS_BASE: u64 = 2 | (1 << 9) | 0x85;
const RFLAGS_AC: u64 = 1 << 18;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Cpl0,
    Cpl3,
}

/// Memory of the cases. Pages are 4 KiB; `supervisor` pages fault for an access whose page privilege is Ring3.
struct Ram {
    bytes: Vec<u8>,
    supervisor: Vec<u64>,
    reads: usize,
    writes: usize,
}

impl Ram {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![SENTINEL; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            supervisor: vec![SUPERVISOR_PAGE >> 12],
            reads: 0,
            writes: 0,
        }
    }

    fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
    }

    fn check(
        &self,
        address: u64,
        length: usize,
        access: AccessKind,
        context: AccessContext,
    ) -> Result<usize, MemoryError> {
        let start = usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM))
            .ok_or(MemoryError::Unmapped { address, access })?;
        if context.page_privilege() == PrivilegeLevel::Ring3 {
            for page in address >> 12..=(address + length as u64 - 1) >> 12 {
                if self.supervisor.contains(&page) {
                    return Err(MemoryError::Architectural(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(page_fault_error_code(
                            access, context, true, false, false,
                        )),
                        linear_address: Some(address.max(page << 12)),
                    }));
                }
            }
        }
        Ok(start)
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        let start = self.check(address, 1, AccessKind::Fetch, context)?;
        Ok(self.bytes[start])
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        out: &mut [u8],
    ) -> Result<(), MemoryError> {
        let start = self.check(address, out.len(), AccessKind::Read, context)?;
        self.reads += 1;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        input: &[u8],
    ) -> Result<(), MemoryError> {
        let start = self.check(address, input.len(), AccessKind::Write, context)?;
        self.writes += 1;
        self.bytes[start..start + input.len()].copy_from_slice(input);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn context(level: Level) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: match level {
            Level::Cpl0 => PrivilegeLevel::Ring0,
            Level::Cpl3 => PrivilegeLevel::Ring3,
        },
        origin: AccessOrigin::Explicit,
    }
}

fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|byte| ((index * 16 + byte) as u8).wrapping_mul(0x1d) ^ 0x3c)
}

fn ordinary(index: usize) -> [u8; 16] {
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
    rflags: u64,
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdi: u64,
    rbp: u64,
    memory: [u8; 16],
    /// Sixteen bytes at DATA + 16: a valid MXCSR value for the LDMXCSR forms at [RAX+0x10].
    tail: [u8; 16],
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
        let mut tail = [0x77; 16];
        tail[..4].copy_from_slice(&0x1fa0u32.to_le_bytes());
        Self {
            xmm: core::array::from_fn(pick),
            mxcsr: MXCSR_MASKED,
            cr0: CR0_BASE,
            cr4: CR4_FP,
            rflags: RFLAGS_BASE,
            rax: DATA,
            rbx: DATA,
            rcx: 0x1234_5678_9abc_def0,
            rdi: DATA + 0x80,
            rbp: DATA + 0x100,
            memory: pick(3),
            tail,
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
    executed: u64,
    rip: u64,
    rflags: u64,
    gpr: [u64; 16],
    xmm: [[u8; 16]; 16],
    mxcsr: u32,
    data: Vec<u8>,
}

fn initial_state(case: &Case, level: Level) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: case.rflags,
        cr0: Some(case.cr0),
        cr4: Some(case.cr4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    };
    if level == Level::Cpl3 {
        state.cs_selector = Some(0x33);
        state.cs_cache = Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xfb,
            flags: 0x2,
        });
        state.ss_selector = Some(0x2b);
        state.ss_cache = Some(DataSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xf3,
            flags: 0xc,
        });
    }
    for (index, slot) in state.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    state.gpr[0] = case.rax;
    state.gpr[1] = case.rcx;
    state.gpr[3] = case.rbx;
    state.gpr[5] = case.rbp;
    state.gpr[7] = case.rdi;
    state
}

fn initial_memory(code: &[u8], case: &Case) -> Ram {
    let mut ram = Ram::new(code);
    ram.put(DATA, &case.memory);
    ram.put(DATA + 16, &case.tail);
    ram
}

fn result_of(exit: RunExit) -> Result_ {
    match exit {
        RunExit::InstructionBudgetExhausted => Result_::Retired,
        RunExit::Fault(fault) => Result_::Fault(fault),
        RunExit::Unsupported { opcode } => Result_::Unsupported(opcode),
        RunExit::MemoryError(error) => Result_::Memory(error),
        other => panic!("unexpected exit {other:?}"),
    }
}

/// The bare run: `step` over a state with SSE and x87 state and no control registers (the general S2-34/37/38/54/55 core
/// without gates), in the context of `level` (the executors refuse a misaligned operand at Ring3 with AC set).
fn bare(code: &[u8], case: &Case, level: Level, count: u64) -> Observed {
    let mut state = initial_state(case, level);
    state.cs_selector = None;
    state.cs_cache = None;
    state.ss_selector = None;
    state.ss_cache = None;
    state.sse = Some(SseState {
        xmm: case.xmm,
        mxcsr: case.mxcsr,
    });
    state.x87 = Some(X87State::fixture_seed());
    state.cr0 = None;
    state.cr4 = None;
    let mut ram = initial_memory(code, case);
    let mut exit = RunExit::InstructionBudgetExhausted;
    let mut executed = 0;
    for _ in 0..count {
        exit = match execute::step(&mut state, &mut ram, context(level)) {
            Ok(StepOutcome::Advanced) => {
                executed += 1;
                continue;
            }
            Ok(StepOutcome::Fault(fault)) => RunExit::Fault(fault),
            Ok(StepOutcome::Unsupported { opcode }) => RunExit::Unsupported { opcode },
            Ok(other) => panic!("unexpected bare outcome {other:?}"),
            Err(error) => RunExit::MemoryError(error),
        };
        break;
    }
    let sse = state.sse.expect("the bare state keeps its SSE state");
    Observed {
        result: result_of(exit),
        executed,
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
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
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

fn run_set(
    level: Level,
    state: &mut LongModeState,
    ram: &mut Ram,
    cell: &RefCell<GuestFpState>,
    budget: u64,
) -> (RunExit, u64) {
    let config = RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: context(level),
    };
    let result = match level {
        Level::Cpl0 => run_with_cpl0_profile_set(
            state,
            ram,
            config,
            od_isa::INTEGER_FLAGS,
            &set(cell),
            None,
            || false,
        ),
        Level::Cpl3 => run_with_cpl3_profile_set(
            state,
            ram,
            config,
            od_isa::INTEGER_FLAGS,
            &set(cell),
            None,
            || false,
        ),
    }
    .expect("the case is admitted by the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    (exit, result.instructions_executed)
}

/// Runs `count` instructions in the set at `level`; returns what was observed and the cell before and after.
fn in_set(
    level: Level,
    code: &[u8],
    case: &Case,
    count: u64,
    change: impl FnOnce(&mut LongModeState),
) -> (Observed, GuestFpState, GuestFpState) {
    let mut state = initial_state(case, level);
    change(&mut state);
    let mut ram = initial_memory(code, case);
    let cell = cell_for(case);
    let before = cell.borrow().clone();
    let (exit, executed) = run_set(level, &mut state, &mut ram, &cell, count);
    assert!(
        state.sse.is_none() && state.x87.is_none(),
        "the set hands the state back without FP"
    );
    let after = cell.borrow().clone();
    (
        Observed {
            result: result_of(exit),
            executed,
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

fn fault(vector: u8) -> Result_ {
    Result_::Fault(ArchitecturalFault {
        vector,
        error_code: None,
        linear_address: None,
    })
}

fn fault_code(vector: u8, code: u32) -> Result_ {
    Result_::Fault(ArchitecturalFault {
        vector,
        error_code: Some(code),
        linear_address: None,
    })
}

fn gp0() -> Result_ {
    fault_code(13, 0)
}

/// Register forms and [RAX]-addressed memory forms by the plan slice that introduced them (the S2-34/37/38/54/55 forms
/// of `s2_39_cpl0_sse` and the S2-39n/q/r forms of the set: raw moves, MOVSS/MOVSD, LDMXCSR/STMXCSR, integer SSE2).
const FORMS: &[(&str, &[u8])] = &[
    // S2-34: raw 128-bit moves.
    ("MOVAPS xmm1,[rax]", &[0x0f, 0x28, 0x08]),
    ("MOVAPS [rax],xmm1", &[0x0f, 0x29, 0x08]),
    ("MOVAPD xmm1,[rax]", &[0x66, 0x0f, 0x28, 0x08]),
    ("MOVAPD [rax],xmm1", &[0x66, 0x0f, 0x29, 0x08]),
    ("MOVUPS xmm1,[rax]", &[0x0f, 0x10, 0x08]),
    ("MOVUPS [rax],xmm1", &[0x0f, 0x11, 0x08]),
    ("MOVUPD xmm1,[rax]", &[0x66, 0x0f, 0x10, 0x08]),
    ("MOVUPD [rax],xmm1", &[0x66, 0x0f, 0x11, 0x08]),
    ("MOVDQA xmm1,[rax]", &[0x66, 0x0f, 0x6f, 0x08]),
    ("MOVDQA [rax],xmm1", &[0x66, 0x0f, 0x7f, 0x08]),
    ("MOVDQU xmm1,[rax]", &[0xf3, 0x0f, 0x6f, 0x08]),
    ("MOVDQU [rax],xmm1", &[0xf3, 0x0f, 0x7f, 0x08]),
    ("MOVAPS xmm1,xmm2", &[0x0f, 0x28, 0xca]),
    ("MOVAPS xmm2,xmm1 (store direction)", &[0x0f, 0x29, 0xca]),
    ("MOVAPD xmm1,xmm2", &[0x66, 0x0f, 0x28, 0xca]),
    ("MOVUPS xmm1,xmm2", &[0x0f, 0x10, 0xca]),
    ("MOVUPD xmm1,xmm2", &[0x66, 0x0f, 0x10, 0xca]),
    ("MOVDQA xmm1,xmm2", &[0x66, 0x0f, 0x6f, 0xca]),
    (
        "MOVDQA xmm2,xmm1 (store direction)",
        &[0x66, 0x0f, 0x7f, 0xca],
    ),
    ("MOVDQU xmm1,xmm2", &[0xf3, 0x0f, 0x6f, 0xca]),
    (
        "MOVDQU xmm2,xmm1 (store direction)",
        &[0xf3, 0x0f, 0x7f, 0xca],
    ),
    // S2-39n: MOVSS/MOVSD, LDMXCSR/STMXCSR.
    ("MOVSS xmm1,xmm2", &[0xf3, 0x0f, 0x10, 0xca]),
    (
        "MOVSS xmm2,xmm1 (store direction)",
        &[0xf3, 0x0f, 0x11, 0xca],
    ),
    ("MOVSD xmm1,xmm2", &[0xf2, 0x0f, 0x10, 0xca]),
    (
        "MOVSD xmm2,xmm1 (store direction)",
        &[0xf2, 0x0f, 0x11, 0xca],
    ),
    ("MOVSS xmm1,[rax]", &[0xf3, 0x0f, 0x10, 0x08]),
    ("MOVSS [rax],xmm1", &[0xf3, 0x0f, 0x11, 0x08]),
    ("MOVSD xmm1,[rax]", &[0xf2, 0x0f, 0x10, 0x08]),
    ("MOVSD [rax],xmm1", &[0xf2, 0x0f, 0x11, 0x08]),
    (
        "MOVSD xmm1,[rbp-24] (decoder form)",
        &[0xf2, 0x0f, 0x10, 0x4d, 0xe8],
    ),
    ("LDMXCSR [rax+0x10]", &[0x0f, 0xae, 0x50, 0x10]),
    ("STMXCSR [rax]", &[0x0f, 0xae, 0x18]),
    // S2-37: arithmetic and conversions.
    ("ADDPS xmm1,xmm2", &[0x0f, 0x58, 0xca]),
    ("SUBPD xmm1,xmm2", &[0x66, 0x0f, 0x5c, 0xca]),
    ("MULSD xmm1,xmm2", &[0xf2, 0x0f, 0x59, 0xca]),
    ("DIVSS xmm1,xmm2", &[0xf3, 0x0f, 0x5e, 0xca]),
    ("ADDSD xmm1,xmm2", &[0xf2, 0x0f, 0x58, 0xca]),
    ("ADDPS xmm1,[rax]", &[0x0f, 0x58, 0x08]),
    ("MULSD xmm1,[rax]", &[0xf2, 0x0f, 0x59, 0x08]),
    ("SQRTPD xmm1,xmm2", &[0x66, 0x0f, 0x51, 0xca]),
    ("MINPS xmm1,xmm2", &[0x0f, 0x5d, 0xca]),
    ("MAXSD xmm1,xmm2", &[0xf2, 0x0f, 0x5f, 0xca]),
    ("RCPPS xmm1,xmm2", &[0x0f, 0x53, 0xca]),
    ("CVTSI2SD xmm1,rax", &[0xf2, 0x48, 0x0f, 0x2a, 0xc8]),
    ("CVTSI2SS xmm1,eax", &[0xf3, 0x0f, 0x2a, 0xc8]),
    ("CVTTSD2SI rcx,xmm1", &[0xf2, 0x48, 0x0f, 0x2c, 0xc9]),
    ("CVTSS2SI ecx,xmm2", &[0xf3, 0x0f, 0x2d, 0xca]),
    ("CVTDQ2PD xmm1,xmm2", &[0xf3, 0x0f, 0xe6, 0xca]),
    ("CVTPD2DQ xmm1,xmm2", &[0xf2, 0x0f, 0xe6, 0xca]),
    // S2-38: compares, logic, permutes, MOVMSK.
    ("CMPPS xmm1,xmm2,2", &[0x0f, 0xc2, 0xca, 0x02]),
    ("UCOMISD xmm1,xmm2", &[0x66, 0x0f, 0x2e, 0xca]),
    ("COMISS xmm1,xmm2", &[0x0f, 0x2f, 0xca]),
    ("ANDPS xmm1,xmm2", &[0x0f, 0x54, 0xca]),
    ("ANDPS xmm1,[rax]", &[0x0f, 0x54, 0x08]),
    ("XORPD xmm1,xmm2", &[0x66, 0x0f, 0x57, 0xca]),
    ("XORPS xmm1,xmm2", &[0x0f, 0x57, 0xca]),
    ("SHUFPS xmm1,xmm2,0x1b", &[0x0f, 0xc6, 0xca, 0x1b]),
    ("UNPCKLPS xmm1,xmm2", &[0x0f, 0x14, 0xca]),
    ("UNPCKHPD xmm1,xmm2", &[0x66, 0x0f, 0x15, 0xca]),
    ("MOVMSKPS eax,xmm2", &[0x0f, 0x50, 0xc2]),
    // S2-54: SSE3 and SSSE3.
    ("HADDPS xmm1,xmm2", &[0xf2, 0x0f, 0x7c, 0xca]),
    ("ADDSUBPD xmm1,xmm2", &[0x66, 0x0f, 0xd0, 0xca]),
    ("MOVDDUP xmm1,xmm2", &[0xf2, 0x0f, 0x12, 0xca]),
    ("MOVSHDUP xmm1,xmm2", &[0xf3, 0x0f, 0x16, 0xca]),
    ("PSHUFB xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x00, 0xca]),
    ("PALIGNR xmm1,xmm2,5", &[0x66, 0x0f, 0x3a, 0x0f, 0xca, 0x05]),
    ("PABSD xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x1e, 0xca]),
    ("PHADDW xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x01, 0xca]),
    // S2-55: SSE4.1.
    ("PMINSD xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x39, 0xca]),
    ("PMULLD xmm1,[rax]", &[0x66, 0x0f, 0x38, 0x40, 0x08]),
    (
        "PBLENDW xmm1,xmm2,0x5a",
        &[0x66, 0x0f, 0x3a, 0x0e, 0xca, 0x5a],
    ),
    ("PMOVSXBW xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x20, 0xca]),
    ("PEXTRD eax,xmm1,2", &[0x66, 0x0f, 0x3a, 0x16, 0xc8, 0x02]),
    ("PTEST xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x17, 0xca]),
    ("ROUNDPS xmm1,xmm2,9", &[0x66, 0x0f, 0x3a, 0x08, 0xca, 0x09]),
    ("DPPS xmm1,xmm2,0xff", &[0x66, 0x0f, 0x3a, 0x40, 0xca, 0xff]),
    // S2-39q: integer SSE2 (register, memory and shift forms).
    ("PADDB xmm1,xmm2", &[0x66, 0x0f, 0xfc, 0xca]),
    ("PADDB xmm1,[rax]", &[0x66, 0x0f, 0xfc, 0x08]),
    ("PSUBUSW xmm1,xmm2", &[0x66, 0x0f, 0xd9, 0xca]),
    ("PMULHW xmm1,xmm2", &[0x66, 0x0f, 0xe5, 0xca]),
    ("PMADDWD xmm1,[rax]", &[0x66, 0x0f, 0xf5, 0x08]),
    ("PMULUDQ xmm1,xmm2", &[0x66, 0x0f, 0xf4, 0xca]),
    ("PSADBW xmm1,xmm2", &[0x66, 0x0f, 0xf6, 0xca]),
    ("PAVGB xmm1,xmm2", &[0x66, 0x0f, 0xe0, 0xca]),
    ("PMAXSW xmm1,xmm2", &[0x66, 0x0f, 0xee, 0xca]),
    ("PUNPCKHQDQ xmm1,xmm2", &[0x66, 0x0f, 0x6d, 0xca]),
    ("PACKSSDW xmm1,[rax]", &[0x66, 0x0f, 0x6b, 0x08]),
    ("PAND xmm1,xmm2", &[0x66, 0x0f, 0xdb, 0xca]),
    ("PCMPEQB xmm1,[rax]", &[0x66, 0x0f, 0x74, 0x08]),
    ("PCMPGTD xmm1,xmm2", &[0x66, 0x0f, 0x66, 0xca]),
    ("PSLLW xmm1,xmm2", &[0x66, 0x0f, 0xf1, 0xca]),
    ("PSRAD xmm1,[rax]", &[0x66, 0x0f, 0xe2, 0x08]),
    ("PSLLQ xmm1,3", &[0x66, 0x0f, 0x73, 0xf1, 0x03]),
    ("PSRLDQ xmm1,5", &[0x66, 0x0f, 0x73, 0xd9, 0x05]),
    ("PSHUFD xmm1,xmm2,0x1b", &[0x66, 0x0f, 0x70, 0xca, 0x1b]),
    ("PSHUFHW xmm1,xmm2,0x4e", &[0xf3, 0x0f, 0x70, 0xca, 0x4e]),
    ("PSHUFLW xmm1,[rax],0x39", &[0xf2, 0x0f, 0x70, 0x08, 0x39]),
    ("MOVD xmm1,eax", &[0x66, 0x0f, 0x6e, 0xc8]),
    ("MOVQ xmm1,rax", &[0x66, 0x48, 0x0f, 0x6e, 0xc8]),
    ("MOVD ecx,xmm1", &[0x66, 0x0f, 0x7e, 0xc9]),
    ("MOVQ rcx,xmm1", &[0x66, 0x48, 0x0f, 0x7e, 0xc9]),
    ("MOVQ xmm1,xmm2", &[0xf3, 0x0f, 0x7e, 0xca]),
    ("MOVQ xmm1,[rax]", &[0xf3, 0x0f, 0x7e, 0x08]),
    ("MOVQ [rax],xmm1", &[0x66, 0x0f, 0xd6, 0x08]),
    ("MOVLPS xmm1,[rax]", &[0x0f, 0x12, 0x08]),
    ("MOVLPS [rax],xmm1", &[0x0f, 0x13, 0x08]),
    ("MOVLPD xmm1,[rax]", &[0x66, 0x0f, 0x12, 0x08]),
    ("MOVHPS xmm1,[rax]", &[0x0f, 0x16, 0x08]),
    ("MOVHPD [rax],xmm1", &[0x66, 0x0f, 0x17, 0x08]),
    ("MOVHLPS xmm1,xmm2", &[0x0f, 0x12, 0xca]),
    ("MOVLHPS xmm1,xmm2", &[0x0f, 0x16, 0xca]),
    ("PEXTRW ecx,xmm2,3", &[0x66, 0x0f, 0xc5, 0xca, 0x03]),
    ("PINSRW xmm1,eax,2", &[0x66, 0x0f, 0xc4, 0xc8, 0x02]),
    ("PINSRW xmm1,[rax],2", &[0x66, 0x0f, 0xc4, 0x08, 0x02]),
    ("PMOVMSKB eax,xmm2", &[0x66, 0x0f, 0xd7, 0xc2]),
    // S2-39r: non-temporal stores and MASKMOVDQU (implicit [RDI]).
    ("MOVNTDQ [rax],xmm1", &[0x66, 0x0f, 0xe7, 0x08]),
    ("MOVNTPS [rax],xmm1", &[0x0f, 0x2b, 0x08]),
    ("MOVNTPD [rax],xmm1", &[0x66, 0x0f, 0x2b, 0x08]),
    ("MASKMOVDQU xmm1,xmm2", &[0x66, 0x0f, 0xf7, 0xca]),
    // Forms with a handler of the former CPL3 profile (S6-22) and their addressing neighbours.
    ("MOVD xmm1,[rax]", &[0x66, 0x0f, 0x6e, 0x08]),
    ("PMINUB xmm1,[rax+0x10]", &[0x66, 0x0f, 0xda, 0x48, 0x10]),
    ("PCMPEQB xmm1,[rax+0x10]", &[0x66, 0x0f, 0x74, 0x48, 0x10]),
    ("PCMPEQB xmm1,[rbx]", &[0x66, 0x0f, 0x74, 0x0b]),
    (
        "PINSRW xmm1,[rax+8],1",
        &[0x66, 0x0f, 0xc4, 0x48, 0x08, 0x01],
    ),
    ("MOVD xmm1,[eax] (67)", &[0x67, 0x66, 0x0f, 0x6e, 0x08]),
    ("PADDB xmm1,[eax] (67)", &[0x67, 0x66, 0x0f, 0xfc, 0x08]),
];

/// Names of the FORMS that write memory (the stores).
fn stores_memory(name: &str) -> bool {
    name.contains("],xmm")
        || name.starts_with("MOVNT")
        || name.starts_with("STMXCSR")
        || name.starts_with("MASKMOVDQU")
}

fn compare(name: &str, code: &[u8], case: &Case, expect: Option<Result_>) -> Result<(), String> {
    let reference = bare(code, case, Level::Cpl3, 1);
    let (cpl3, before, after) = in_set(Level::Cpl3, code, case, 1, |_| {});
    let (cpl0, _, _) = in_set(Level::Cpl0, code, case, 1, |_| {});
    // The plain `step` does not decode some of the forms (the S2-39n forms, the handler forms of the former CPL3 profile):
    // it answers `Unsupported` and is no reference for them; the CPL0 set is.
    if !matches!(reference.result, Result_::Unsupported(_)) {
        if cpl3.result != reference.result {
            return Err(format!(
                "{name}: CPL3 outcome {:?}, bare {:?}",
                cpl3.result, reference.result
            ));
        }
        if cpl3 != reference {
            return Err(format!(
                "{name}: the CPL3 set differs from the bare run in registers or memory"
            ));
        }
    }
    if cpl3 != cpl0 {
        return Err(format!(
            "{name}: the CPL3 set differs from the CPL0 set (CPL3 {cpl3:?}, CPL0 {cpl0:?})"
        ));
    }
    if after.x87 != before.x87 {
        return Err(format!("{name}: an SSE form changed the x87 state"));
    }
    match expect {
        Some(expect) if cpl3.result != expect => Err(format!(
            "{name}: the result {:?} is not the intended {expect:?}",
            cpl3.result
        )),
        _ => Ok(()),
    }
}

fn failures_of(case: &Case, expect: Option<Result_>) -> Vec<String> {
    FORMS
        .iter()
        .filter_map(|(name, code)| compare(name, code, case, expect.clone()).err())
        .collect()
}

#[test]
fn every_general_sse_form_retires_at_cpl3_like_the_bare_run_and_the_cpl0_set() {
    for ordinary_operands in [false, true] {
        let case = Case::new(ordinary_operands);
        let failures = failures_of(&case, Some(Result_::Retired));
        assert!(
            failures.is_empty(),
            "ordinary operands {ordinary_operands}: {failures:#?}"
        );
    }
}

#[test]
fn the_forms_change_only_what_the_bare_run_changes_and_store_what_it_stores() {
    // One whole-state comparison per form on the pattern operands: registers, MXCSR, flags, memory and RIP are those of
    // the bare run (checked in `compare`); here: a store form changes memory, a load form does not.
    let case = Case::new(false);
    let mut stores = 0;
    for (name, code) in FORMS {
        let (observed, _, _) = in_set(Level::Cpl3, code, &case, 1, |_| {});
        assert_eq!(observed.result, Result_::Retired, "{name}");
        assert_eq!(observed.rip, CODE + code.len() as u64, "{name}");
        assert_eq!(observed.executed, 1, "{name}");
        let initial = initial_memory(code, &case);
        let same_memory =
            observed.data == initial.bytes[DATA as usize..DATA as usize + 0x500].to_vec();
        if stores_memory(name) && !name.starts_with("MASKMOVDQU") {
            stores += 1;
            assert!(!same_memory, "{name} wrote nothing");
        } else if !stores_memory(name) {
            assert!(same_memory, "{name} changed memory");
        }
    }
    assert!(stores >= 15, "stores {stores}");
}

#[test]
fn a_program_of_every_form_in_one_cpl3_run_matches_the_bare_sequence_and_the_cpl0_run() {
    // The forms that write EAX/ECX/RCX go last so that the [RAX] operands of the others keep their address; LDMXCSR
    // and the stores keep their [RAX] too.
    let (late, early): (Vec<_>, Vec<_>) = FORMS.iter().partition(|(name, _)| {
        name.contains("eax,")
            || name.contains("ecx,")
            || name.contains("rcx,")
            || name.contains("rax,")
    });
    let code: Vec<u8> = early
        .iter()
        .chain(late.iter())
        .flat_map(|(_, code)| code.iter().copied())
        .collect();
    let count = FORMS.len() as u64;
    for ordinary_operands in [false, true] {
        let case = Case::new(ordinary_operands);
        // The plain `step` stops at the first form it does not decode (the S2-39n forms), so the CPL0 set is the
        // reference of the whole program; every form is compared with the bare run one by one in the tests above.
        let (cpl3, _, _) = in_set(Level::Cpl3, &code, &case, count, |_| {});
        let (cpl0, _, _) = in_set(Level::Cpl0, &code, &case, count, |_| {});
        assert_eq!(cpl3, cpl0, "ordinary operands {ordinary_operands}");
        assert_eq!(
            cpl3.result,
            Result_::Retired,
            "ordinary operands {ordinary_operands}"
        );
        assert_eq!(
            cpl3.executed, count,
            "ordinary operands {ordinary_operands}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Sweep of the SSE opcode space: the CPL3 set against the CPL0 set (which S2-39n/q/r checked against the physical oracle).

#[derive(Clone)]
struct SweepCase {
    name: String,
    code: Vec<u8>,
    prefix: Option<u8>,
    map: u8,
    opcode: u8,
    memory: bool,
    rex_w: bool,
    rax: u64,
    ac: bool,
}

fn needs_imm(map: u8, opcode: u8) -> bool {
    map == 2 || (map == 0 && matches!(opcode, 0x70..=0x73 | 0xc2 | 0xc4..=0xc6))
}

/// Forms of the MMX class: they use the x87 data registers, which stay in the cell at CPL3, so the CPL3 set refuses them.
fn mmx_class(prefix: Option<u8>, map: u8, opcode: u8) -> bool {
    match (prefix, map) {
        (None, 0) => {
            matches!(opcode, 0x2a | 0x2c | 0x2d | 0x60..=0x77 | 0x7e | 0x7f | 0xc4 | 0xc5 | 0xd1..=0xfe)
        }
        (None, _) => true,
        (Some(0x66), 0) => matches!(opcode, 0x2a | 0x2c | 0x2d),
        (Some(0xf2 | 0xf3), 0) => opcode == 0xd6,
        _ => false,
    }
}

fn sweep_cases() -> Vec<SweepCase> {
    let mut cases = Vec::new();
    for prefix in [None, Some(0x66u8), Some(0xf2), Some(0xf3)] {
        for map in 0..3u8 {
            let opcodes: Vec<u8> = if map == 0 {
                (0x10..=0x17u8)
                    .chain(0x28..=0x2f)
                    .chain(0x50..=0x7f)
                    .chain(0xc2..=0xc6)
                    .chain(0xd0..=0xff)
                    .chain([0xae])
                    .collect()
            } else {
                (0..=0xffu8).collect()
            };
            for opcode in opcodes {
                let mut escape = vec![0x0f];
                match map {
                    1 => escape.push(0x38),
                    2 => escape.push(0x3a),
                    _ => {}
                }
                let imm: Vec<u8> = if needs_imm(map, opcode) {
                    vec![0x03]
                } else {
                    vec![]
                };
                let build = |rex: Option<u8>, modrm: &[u8]| -> Vec<u8> {
                    let mut code: Vec<u8> = prefix.into_iter().collect();
                    code.extend(rex);
                    code.extend(&escape);
                    code.push(opcode);
                    code.extend(modrm);
                    code.extend(&imm);
                    code
                };
                let tag = |kind: &str| {
                    format!(
                        "{}{}{opcode:02x}-{kind}",
                        prefix.map(|p| format!("{p:02x}-")).unwrap_or_default(),
                        ["0f-", "0f38-", "0f3a-"][usize::from(map)]
                    )
                };
                let push = |cases: &mut Vec<SweepCase>,
                            kind: &str,
                            code: Vec<u8>,
                            memory: bool,
                            rax: u64,
                            ac: bool| {
                    cases.push(SweepCase {
                        name: format!("{}-a{:x}-{}", tag(kind), rax - DATA, u8::from(ac)),
                        code,
                        prefix,
                        map,
                        opcode,
                        memory,
                        rex_w: kind.ends_with('w'),
                        rax,
                        ac,
                    });
                };
                if map == 0 && opcode == 0xae {
                    for modrm in [0x10u8, 0x18] {
                        for (rax, ac) in [
                            (DATA, false),
                            (DATA + 1, false),
                            (DATA + 1, true),
                            (DATA + 4, true),
                        ] {
                            push(
                                &mut cases,
                                &format!("m{modrm:02x}"),
                                build(None, &[modrm]),
                                true,
                                rax,
                                ac,
                            );
                        }
                    }
                    continue;
                }
                // Register forms (the group opcodes 71..73 take every /digit).
                let register_modrms: Vec<u8> = if map == 0 && (0x71..=0x73).contains(&opcode) {
                    (0..8).map(|digit| 0xc0 | digit << 3 | 2).collect()
                } else {
                    vec![0xca, 0xc1]
                };
                for modrm in register_modrms {
                    push(
                        &mut cases,
                        &format!("r{modrm:02x}"),
                        build(None, &[modrm]),
                        false,
                        DATA,
                        false,
                    );
                }
                push(
                    &mut cases,
                    "rrex",
                    build(Some(0x45), &[0xca]),
                    false,
                    DATA,
                    false,
                );
                push(
                    &mut cases,
                    "rw",
                    build(Some(0x48), &[0xca]),
                    false,
                    DATA,
                    false,
                );
                push(
                    &mut cases,
                    "rw1",
                    build(Some(0x48), &[0xc1]),
                    false,
                    DATA,
                    false,
                );
                // Memory forms: [RAX] (aligned, odd, 8-aligned) with and without RFLAGS.AC; REX.W as well.
                for (rax, ac) in [
                    (DATA, false),
                    (DATA + 1, false),
                    (DATA + 8, false),
                    (DATA, true),
                    (DATA + 1, true),
                    (DATA + 2, true),
                    (DATA + 4, true),
                    (DATA + 8, true),
                ] {
                    push(&mut cases, "m", build(None, &[0x08]), true, rax, ac);
                    push(&mut cases, "mw", build(Some(0x48), &[0x08]), true, rax, ac);
                }
            }
        }
    }
    cases
}

#[derive(Default)]
struct SweepTotals {
    cases: usize,
    equal: usize,
    equal_retired: usize,
    refused_ac: usize,
    refused_mmx: usize,
    refused_crc32: usize,
    /// (prefix, map, opcode, REX.W, offset from a 16-byte boundary) of the memory forms that retired with AC set at
    /// an operand misaligned for a 16-byte access.
    misaligned_ac_retired: std::collections::BTreeSet<(Option<u8>, u8, u8, bool, u64)>,
}

/// Forms whose memory operand is one byte (PEXTRB, PINSRB) or one word (PINSRW, PEXTRW, PMOVSXBQ/PMOVZXBQ): the
/// forms that may retire at an odd (byte) or 2 mod 4 (word) address with AC set, because they have no misalignment.
fn byte_form(prefix: Option<u8>, map: u8, opcode: u8) -> bool {
    prefix == Some(0x66) && map == 2 && matches!(opcode, 0x14 | 0x20)
}

fn word_form(prefix: Option<u8>, map: u8, opcode: u8) -> bool {
    prefix == Some(0x66)
        && ((map == 0 && opcode == 0xc4)
            || (map == 1 && matches!(opcode, 0x22 | 0x32))
            || (map == 2 && opcode == 0x15))
}

/// Forms with an eight-byte memory operand (the rest of the retirements are of at most four bytes): none may retire
/// at an operand that is 4 mod 8 with AC set.
fn eight_byte_form(prefix: Option<u8>, map: u8, opcode: u8, rex_w: bool) -> bool {
    match (prefix, map, opcode) {
        (Some(0xf2), 0, 0x10 | 0x11 | 0x12 | 0x2c | 0x2d | 0x51..=0x5f | 0xc2 | 0xe6) => true,
        (Some(0x66), 2, 0x0b) => true,
        (None | Some(0x66), 0, 0x12 | 0x13 | 0x16 | 0x17) => true,
        (None, 0, 0x5a) | (Some(0x66), 0, 0x2e | 0x2f | 0xd6) | (Some(0xf3), 0, 0x7e | 0xe6) => {
            true
        }
        (Some(0x66), 1, 0x20 | 0x23 | 0x25 | 0x30 | 0x33 | 0x35) => true,
        (Some(0x66), 0, 0x6e | 0x7e)
        | (Some(0x66), 2, 0x16 | 0x22)
        | (Some(0xf2 | 0xf3), 0, 0x2a) => rex_w,
        _ => false,
    }
}

/// Whether the memory operand of the form at `offset` from a 16-byte boundary is misaligned for the size of the
/// operand (the check of the `#AC` transfer): never for a byte form, at an odd address for a word form, at a
/// non-multiple of eight for an eight-byte form, at a non-multiple of sixteen for the unaligned 16-byte moves, and
/// at a non-multiple of four for the other forms (at most four bytes).
fn misaligned_for_its_size(
    prefix: Option<u8>,
    map: u8,
    opcode: u8,
    rex_w: bool,
    offset: u64,
) -> bool {
    if byte_form(prefix, map, opcode) {
        false
    } else if word_form(prefix, map, opcode) {
        offset % 2 != 0
    } else if eight_byte_form(prefix, map, opcode, rex_w) {
        offset % 8 != 0
    } else if sixteen_byte_form(prefix, map, opcode) {
        offset % 16 != 0
    } else {
        offset % 4 != 0
    }
}

/// Forms with a 16-byte memory operand that the hardware accepts at a misaligned address (no `#GP`).
fn sixteen_byte_form(prefix: Option<u8>, map: u8, opcode: u8) -> bool {
    match (prefix, map, opcode) {
        // MOVUPS, MOVUPD, MOVDQU, LDDQU and the string compares PCMPESTRM/PCMPESTRI/PCMPISTRM/PCMPISTRI.
        (None | Some(0x66), 0, 0x10 | 0x11)
        | (Some(0xf3), 0, 0x6f | 0x7f)
        | (Some(0xf2), 0, 0xf0) => true,
        (Some(0x66), 2, 0x60..=0x63) => true,
        _ => false,
    }
}

#[test]
fn the_sse_opcode_space_at_cpl3_equals_the_cpl0_set_except_the_ac_transfer_and_the_mmx_forms() {
    let mut totals = SweepTotals::default();
    let mut failures = Vec::new();
    for sweep in sweep_cases() {
        let mut case = Case::new(totals.cases % 2 == 0);
        case.rax = sweep.rax;
        case.rbx = sweep.rax;
        if sweep.ac {
            case.rflags |= RFLAGS_AC;
        }
        totals.cases += 1;
        let (cpl3, before, after) = in_set(Level::Cpl3, &sweep.code, &case, 1, |_| {});
        let (cpl0, _, _) = in_set(Level::Cpl0, &sweep.code, &case, 1, |_| {});
        if after.x87 != before.x87 {
            failures.push(format!(
                "{}: the CPL3 run changed the x87 state",
                sweep.name
            ));
        }
        if cpl3 == cpl0 {
            totals.equal += 1;
            if cpl3.result == Result_::Retired {
                totals.equal_retired += 1;
                if sweep.ac && sweep.memory && sweep.rax & 15 != 0 {
                    totals.misaligned_ac_retired.insert((
                        sweep.prefix,
                        sweep.map,
                        sweep.opcode,
                        sweep.rex_w,
                        sweep.rax - DATA,
                    ));
                }
            }
            continue;
        }
        match (&cpl3.result, &cpl0.result) {
            (Result_::Unsupported(opcode), cpl0_result)
                if !matches!(cpl0_result, Result_::Unsupported(_)) =>
            {
                // A refusal of CPL3 only: the state is the initial one, nothing was retired or written.
                let untouched = cpl3.executed == 0
                    && cpl3.rip == CODE
                    && after == before
                    && cpl3.rflags == case.rflags
                    && cpl3.data
                        == initial_memory(&sweep.code, &case).bytes
                            [DATA as usize..DATA as usize + 0x500];
                if !untouched {
                    failures.push(format!("{}: a refusal changed the state", sweep.name));
                } else if sweep.ac
                    && sweep.memory
                    && misaligned_for_its_size(
                        sweep.prefix,
                        sweep.map,
                        sweep.opcode,
                        sweep.rex_w,
                        sweep.rax - DATA,
                    )
                {
                    totals.refused_ac += 1;
                } else if mmx_class(sweep.prefix, sweep.map, sweep.opcode) {
                    totals.refused_mmx += 1;
                } else if sweep.prefix == Some(0xf2)
                    && sweep.map == 1
                    && matches!(sweep.opcode, 0xf0 | 0xf1)
                {
                    // CRC32 is a general-register form of S2-56, in the CPL0 set at Ring0 only (not an SSE form).
                    totals.refused_crc32 += 1;
                } else {
                    failures.push(format!(
                        "{}: CPL3 refuses (opcode {opcode:#x}) what the CPL0 set runs: {cpl0_result:?}",
                        sweep.name
                    ));
                }
            }
            _ => failures.push(format!(
                "{}: CPL3 {:?} / CPL0 {:?}",
                sweep.name, cpl3.result, cpl0.result
            )),
        }
    }
    eprintln!(
        "sweep: {} cases, {} equal ({} retired), {} AC refusals, {} MMX refusals, {} CRC32 refusals, {} failures; {} misaligned AC retirements (aligned to their size)",
        totals.cases,
        totals.equal,
        totals.equal_retired,
        totals.refused_ac,
        totals.refused_mmx,
        totals.refused_crc32,
        failures.len(),
        totals.misaligned_ac_retired.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures, first {:#?}",
        failures.len(),
        &failures[..failures.len().min(30)]
    );
    // No form retires at a misaligned operand with AC set unless the operand of the form is aligned to its size: a
    // byte form anywhere, a word form at an even address, the other forms (at most four bytes) at a multiple of four.
    for (prefix, map, opcode, rex_w, offset) in &totals.misaligned_ac_retired {
        let (prefix, map, opcode, rex_w, offset) = (*prefix, *map, *opcode, *rex_w, *offset);
        let allowed = !misaligned_for_its_size(prefix, map, opcode, rex_w, offset);
        assert!(
            allowed,
            "retired at +{offset} with AC: {prefix:02x?} map {map} {opcode:#x} rex.w {rex_w}"
        );
    }
    assert!(totals.cases > 40_000, "cases {}", totals.cases);
    assert!(
        totals.equal_retired > 2_000,
        "retired {}",
        totals.equal_retired
    );
    assert!(totals.refused_ac > 100, "AC refusals {}", totals.refused_ac);
    assert!(
        totals.refused_mmx > 100,
        "MMX refusals {}",
        totals.refused_mmx
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Rules of CPL3: gates, SIMD exceptions, MXCSR, addresses, page privilege, #AC transfer, refusals.

fn assert_refused_unchanged(
    name: &str,
    observed: &Observed,
    before: &GuestFpState,
    after: &GuestFpState,
    case: &Case,
) {
    assert_eq!(observed.executed, 0, "{name}");
    assert_eq!(observed.rip, CODE, "{name}");
    assert_eq!(observed.rflags, case.rflags, "{name}");
    assert_eq!(after, before, "{name}: the cell changed");
}

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_at_cpl3_before_any_address_check() {
    // (CR0, CR4, vector): EM and a clear OSFXSR are #UD, TS alone is #NM, and #UD wins over #NM.
    let gates = [
        (CR0_BASE | CR0_EM, CR4_FP, 6),
        (CR0_BASE | CR0_TS, CR4_FP, 7),
        (CR0_BASE | CR0_EM | CR0_TS, CR4_FP, 6),
        (CR0_BASE, CR4_BASE | CR4_OSXMMEXCPT, 6),
        (CR0_BASE | CR0_TS, CR4_BASE, 6),
    ];
    for (cr0, cr4, vector) in gates {
        // A misaligned and a non-canonical [RAX], and AC set, must not matter: the gate comes first.
        for (rax, ac) in [
            (DATA, false),
            (DATA + 1, false),
            (DATA + 1, true),
            (0x0001_0000_0000_0000, false),
        ] {
            let mut case = Case::new(true);
            case.cr0 = cr0;
            case.cr4 = cr4;
            case.rax = rax;
            if ac {
                case.rflags |= RFLAGS_AC;
            }
            for (name, code) in FORMS {
                let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
                let (cpl0, _, _) = in_set(Level::Cpl0, code, &case, 1, |_| {});
                assert_eq!(
                    cpl3.result,
                    fault(vector),
                    "{name} cr0 {cr0:#x} cr4 {cr4:#x} rax {rax:#x}"
                );
                assert_eq!(cpl3, cpl0, "{name}");
                assert_refused_unchanged(name, &cpl3, &before, &after, &case);
            }
        }
    }
}

#[test]
fn an_unmasked_exception_at_cpl3_is_xm_or_ud_by_cr4_osxmmexcpt_and_keeps_the_destination() {
    // DIVPS xmm1,xmm2 with a zero divisor and the divide-by-zero mask clear.
    let code = [0x0f, 0x5e, 0xca];
    for (cr4, vector) in [(CR4_FP, 19), (CR4_BASE | CR4_OSFXSR, 6)] {
        let mut case = Case::new(true);
        case.cr4 = cr4;
        case.xmm[2] = [0; 16];
        case.mxcsr = MXCSR_MASKED & !(1 << 9);
        let (cpl3, before, after) = in_set(Level::Cpl3, &code, &case, 1, |_| {});
        let (cpl0, _, _) = in_set(Level::Cpl0, &code, &case, 1, |_| {});
        assert_eq!(cpl3.result, fault(vector));
        assert_eq!(cpl3, cpl0);
        assert_eq!(cpl3.rip, CODE);
        assert_eq!(
            after.sse.xmm, before.sse.xmm,
            "the destination is unchanged"
        );
        // The fault leaves the detected flag in MXCSR (ZE, bit 2) in the cell.
        assert_eq!(after.sse.mxcsr, case.mxcsr | 1 << 2);
    }
    // Masked: the flag is set and the form retires, as in the bare run.
    let mut case = Case::new(true);
    case.xmm[2] = [0; 16];
    let reference = bare(&code, &case, Level::Cpl3, 1);
    let (cpl3, _, after) = in_set(Level::Cpl3, &code, &case, 1, |_| {});
    assert_eq!(cpl3.result, Result_::Retired);
    assert_eq!(cpl3, reference);
    assert_eq!(after.sse.mxcsr, MXCSR_MASKED | 1 << 2);
}

#[test]
fn ldmxcsr_and_stmxcsr_work_at_cpl3_with_the_target_mask() {
    let ldmxcsr = [0x0f, 0xae, 0x50, 0x10];
    let stmxcsr = [0x0f, 0xae, 0x18];
    // Every valid value (bits 31:16 clear; DAZ, bit 6, is in the target mask, ADR-0014).
    for value in [
        0x1f80u32,
        0x1fa0,
        0x0000,
        0x1f40,
        0x1fc0,
        0xffff,
        0x9fe0 & 0xffff,
        0x007f,
    ] {
        let mut case = Case::new(true);
        case.tail[..4].copy_from_slice(&value.to_le_bytes());
        let (cpl0, _, _) = in_set(Level::Cpl0, &ldmxcsr, &case, 1, |_| {});
        let (cpl3, _, after) = in_set(Level::Cpl3, &ldmxcsr, &case, 1, |_| {});
        assert_eq!(cpl3.result, Result_::Retired, "{value:#x}");
        assert_eq!(cpl3, cpl0, "{value:#x}");
        assert_eq!(after.sse.mxcsr, value, "{value:#x}");
        assert_eq!(
            after.sse.xmm, case.xmm,
            "{value:#x}: no XMM register changes"
        );
        assert_eq!(cpl3.rflags, RFLAGS_BASE, "{value:#x}");
    }
    // A reserved bit (31:16) is #GP(0) and the value is not loaded.
    for value in [1u32 << 16, 1 << 31, 0xffff_ffff, 0x0001_1f80] {
        let mut case = Case::new(true);
        case.tail[..4].copy_from_slice(&value.to_le_bytes());
        let (cpl3, before, after) = in_set(Level::Cpl3, &ldmxcsr, &case, 1, |_| {});
        assert_eq!(cpl3.result, gp0(), "{value:#x}");
        assert_refused_unchanged("LDMXCSR", &cpl3, &before, &after, &case);
    }
    // STMXCSR stores the live value, flags included, and changes nothing else.
    let mut case = Case::new(true);
    case.mxcsr = 0x1f80 | 0x25;
    let (cpl3, before, after) = in_set(Level::Cpl3, &stmxcsr, &case, 1, |_| {});
    assert_eq!(cpl3.result, Result_::Retired);
    assert_eq!(cpl3.data[..4], (0x1f80u32 | 0x25).to_le_bytes());
    assert_eq!(after, before);
    // The loaded value governs the next form: DIVPS by zero with ZM clear after an LDMXCSR is #XM.
    let mut case = Case::new(true);
    case.xmm[2] = [0; 16];
    case.tail[..4].copy_from_slice(&(MXCSR_MASKED & !(1 << 9)).to_le_bytes());
    let code = [&ldmxcsr[..], &[0x0f, 0x5e, 0xca]].concat();
    let (cpl3, _, after) = in_set(Level::Cpl3, &code, &case, 2, |_| {});
    assert_eq!(cpl3.result, fault(19));
    assert_eq!(cpl3.executed, 1);
    assert_eq!(after.sse.mxcsr, (MXCSR_MASKED & !(1 << 9)) | 1 << 2);
}

#[test]
fn addresses_at_cpl3_follow_the_cpl3_rules_canonical_stack_alignment_and_the_backend() {
    let ss0 = Result_::Fault(ArchitecturalFault {
        vector: 12,
        error_code: Some(0),
        linear_address: None,
    });
    let non_canonical = 0x0001_0000_0000_0000u64;
    // (name, code, register, value, expected)
    let rbp_load = [0x0f, 0x58, 0x45, 0x00]; // ADDPS xmm0,[rbp+0]
    let rsp_load = [0x0f, 0x58, 0x04, 0x24]; // ADDPS xmm0,[rsp]
    let rax_load = [0x0f, 0x58, 0x08];
    let rax_move = [0x0f, 0x28, 0x08]; // MOVAPS xmm1,[rax]
    for (name, code, expected_noncanonical) in [
        ("[rbp]", &rbp_load[..], ss0.clone()),
        ("[rsp]", &rsp_load[..], ss0),
        ("[rax]", &rax_load[..], gp0()),
        ("[rax] move", &rax_move[..], gp0()),
    ] {
        let mut case = Case::new(true);
        case.rax = non_canonical;
        case.rbp = non_canonical;
        let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |state| {
            state.gpr[4] = non_canonical;
        });
        let (cpl0, _, _) = in_set(Level::Cpl0, code, &case, 1, |state| {
            state.gpr[4] = non_canonical;
        });
        assert_eq!(cpl3.result, expected_noncanonical, "{name}");
        assert_eq!(cpl3, cpl0, "{name}");
        assert_refused_unchanged(name, &cpl3, &before, &after, &case);
    }
    // A misaligned packed operand of an aligned form is #GP(0) whatever AC is; an unmapped one is the backend's.
    for ac in [false, true] {
        for (name, code) in [
            ("MOVAPS load", &[0x0f, 0x28, 0x08][..]),
            ("MOVAPS store", &[0x0f, 0x29, 0x08]),
            ("ADDPS memory", &[0x0f, 0x58, 0x08]),
            ("PMULLD memory", &[0x66, 0x0f, 0x38, 0x40, 0x08]),
            ("PADDB memory", &[0x66, 0x0f, 0xfc, 0x08]),
            ("MOVNTDQ", &[0x66, 0x0f, 0xe7, 0x08]),
        ] {
            let mut case = Case::new(true);
            case.rax = DATA + 1;
            if ac {
                case.rflags |= RFLAGS_AC;
            }
            let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
            assert_eq!(cpl3.result, gp0(), "{name} ac {ac}");
            assert_refused_unchanged(name, &cpl3, &before, &after, &case);
            case.rax = 0x10_0000;
            let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
            assert!(
                matches!(cpl3.result, Result_::Memory(_)),
                "{name}: {:?}",
                cpl3.result
            );
            assert_refused_unchanged(name, &cpl3, &before, &after, &case);
        }
    }
}

#[test]
fn a_supervisor_page_is_a_page_fault_with_the_user_bit_at_cpl3_and_readable_at_cpl0() {
    // (name, code, write?)
    let forms: &[(&str, &[u8], bool)] = &[
        ("MOVAPS load", &[0x0f, 0x28, 0x08], false),
        ("MOVAPS store", &[0x0f, 0x29, 0x08], true),
        ("MOVUPS load", &[0x0f, 0x10, 0x08], false),
        ("MOVSD load", &[0xf2, 0x0f, 0x10, 0x08], false),
        ("MOVSD store", &[0xf2, 0x0f, 0x11, 0x08], true),
        ("MOVSS load", &[0xf3, 0x0f, 0x10, 0x08], false),
        ("ADDPS memory", &[0x0f, 0x58, 0x08], false),
        ("ADDSD memory", &[0xf2, 0x0f, 0x58, 0x08], false),
        ("PADDB memory", &[0x66, 0x0f, 0xfc, 0x08], false),
        ("PMULLD memory", &[0x66, 0x0f, 0x38, 0x40, 0x08], false),
        ("MOVQ load", &[0xf3, 0x0f, 0x7e, 0x08], false),
        ("MOVQ store", &[0x66, 0x0f, 0xd6, 0x08], true),
        ("MOVLPS load", &[0x0f, 0x12, 0x08], false),
        ("MOVHPD store", &[0x66, 0x0f, 0x17, 0x08], true),
        ("PINSRW memory", &[0x66, 0x0f, 0xc4, 0x08, 0x01], false),
        ("PCMPEQB memory", &[0x66, 0x0f, 0x74, 0x08], false),
        ("LDMXCSR", &[0x0f, 0xae, 0x10], false),
        ("STMXCSR", &[0x0f, 0xae, 0x18], true),
        ("MOVNTDQ", &[0x66, 0x0f, 0xe7, 0x08], true),
        ("PMOVZXBW memory", &[0x66, 0x0f, 0x38, 0x30, 0x08], false),
    ];
    for (name, code, write) in forms {
        let mut case = Case::new(true);
        case.rax = SUPERVISOR_PAGE;
        let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
        let expected = Result_::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(1 | if *write { 2 } else { 0 } | 4),
            linear_address: Some(SUPERVISOR_PAGE),
        });
        assert_eq!(cpl3.result, expected, "{name}");
        assert_refused_unchanged(name, &cpl3, &before, &after, &case);
        // CPL0 may use the page: no fault (the form retires or stops on its own rules, never on the page).
        let (cpl0, _, _) = in_set(Level::Cpl0, code, &case, 1, |_| {});
        assert!(
            !matches!(
                cpl0.result,
                Result_::Fault(ArchitecturalFault { vector: 14, .. })
            ),
            "{name}: {:?}",
            cpl0.result
        );
    }
    // An access that crosses into the page faults at the page, with no partial effect.
    let mut case = Case::new(true);
    case.rax = SUPERVISOR_PAGE - 4;
    let (cpl3, before, after) = in_set(Level::Cpl3, &[0xf2, 0x0f, 0x11, 0x08], &case, 1, |_| {});
    assert_eq!(
        cpl3.result,
        Result_::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(SUPERVISOR_PAGE),
        })
    );
    assert_refused_unchanged("MOVSD store across", &cpl3, &before, &after, &case);
    assert_eq!(
        cpl3.data,
        initial_memory(&[0xf2, 0x0f, 0x11, 0x08], &case).bytes
            [DATA as usize..DATA as usize + 0x500]
    );
}

#[test]
fn a_misaligned_operand_with_ac_at_cpl3_is_a_refusal_and_the_other_cases_run_like_cpl0() {
    // (name, code, size of the memory operand in bytes: the alignment #AC is checked against)
    let forms: &[(&str, &[u8], u64)] = &[
        ("MOVUPS load", &[0x0f, 0x10, 0x08], 16),
        ("MOVUPS store", &[0x0f, 0x11, 0x08], 16),
        ("MOVUPD load", &[0x66, 0x0f, 0x10, 0x08], 16),
        ("MOVDQU load", &[0xf3, 0x0f, 0x6f, 0x08], 16),
        ("MOVDQU store", &[0xf3, 0x0f, 0x7f, 0x08], 16),
        ("MOVSS load", &[0xf3, 0x0f, 0x10, 0x08], 4),
        ("MOVSS store", &[0xf3, 0x0f, 0x11, 0x08], 4),
        ("MOVSD load", &[0xf2, 0x0f, 0x10, 0x08], 8),
        ("MOVSD store", &[0xf2, 0x0f, 0x11, 0x08], 8),
        ("MOVLPS load", &[0x0f, 0x12, 0x08], 8),
        ("MOVLPS store", &[0x0f, 0x13, 0x08], 8),
        ("MOVHPD load", &[0x66, 0x0f, 0x16, 0x08], 8),
        ("MOVQ load", &[0xf3, 0x0f, 0x7e, 0x08], 8),
        ("MOVQ store", &[0x66, 0x0f, 0xd6, 0x08], 8),
        ("MOVD load", &[0x66, 0x0f, 0x6e, 0x08], 4),
        ("MOVD store", &[0x66, 0x0f, 0x7e, 0x08], 4),
        ("PINSRW memory", &[0x66, 0x0f, 0xc4, 0x08, 0x01], 2),
        ("ADDSS memory", &[0xf3, 0x0f, 0x58, 0x08], 4),
        ("MULSD memory", &[0xf2, 0x0f, 0x59, 0x08], 8),
        ("SQRTSD memory", &[0xf2, 0x0f, 0x51, 0x08], 8),
        ("COMISS memory", &[0x0f, 0x2f, 0x08], 4),
        ("UCOMISD memory", &[0x66, 0x0f, 0x2e, 0x08], 8),
        ("CVTSI2SD memory", &[0xf2, 0x0f, 0x2a, 0x08], 4),
        ("CVTSI2SS memory", &[0xf3, 0x0f, 0x2a, 0x08], 4),
        ("CVTSD2SI memory", &[0xf2, 0x0f, 0x2d, 0x08], 8),
        ("CVTTSS2SI memory", &[0xf3, 0x0f, 0x2c, 0x08], 4),
        ("PMOVZXBW memory", &[0x66, 0x0f, 0x38, 0x30, 0x08], 8),
        ("PMOVSXDQ memory", &[0x66, 0x0f, 0x38, 0x25, 0x08], 8),
        ("LDMXCSR", &[0x0f, 0xae, 0x10], 4),
        ("STMXCSR", &[0x0f, 0xae, 0x18], 4),
    ];
    for (name, code, size) in forms {
        // Offsets from a 16-byte boundary: an operand misaligned for its own size is refused, one aligned to its size
        // (even when not to 16 bytes) is no #AC and runs.
        for offset in [1u64, 2, 3, 4, 5, 6, 7, 8, 12] {
            let mut case = Case::new(true);
            // The 16 bytes at DATA + 0x10 hold a valid MXCSR value first (the LDMXCSR cases).
            case.rax = DATA + 0x10 + offset;
            // MXCSR for LDMXCSR: a valid value at the misaligned address.
            case.rflags |= RFLAGS_AC;
            let (ac, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
            let (cpl0_ac, _, _) = in_set(Level::Cpl0, code, &case, 1, |_| {});
            if offset % size != 0 {
                assert!(
                    matches!(ac.result, Result_::Unsupported(_)),
                    "{name} offset {offset}: a misaligned operand with AC at CPL3 must not retire: {:?}",
                    ac.result
                );
                assert_refused_unchanged(name, &ac, &before, &after, &case);
                assert_eq!(
                    ac.data,
                    initial_memory(code, &case).bytes[DATA as usize..DATA as usize + 0x500]
                );
            } else {
                assert_eq!(ac, cpl0_ac, "{name} offset {offset}: equal to the CPL0 set");
                // LDMXCSR reads whatever the data holds at the offset (a reserved bit set is a #GP(0) as at CPL0);
                // every other form retires.
                if *name != "LDMXCSR" {
                    assert_eq!(
                        ac.result,
                        Result_::Retired,
                        "{name} offset {offset}: an operand aligned to its size with AC at CPL3 runs"
                    );
                } else {
                    assert!(
                        !matches!(ac.result, Result_::Unsupported(_)),
                        "{name} offset {offset}"
                    );
                }
            }
            // CR0.AM = 0: no alignment check, the form runs as at CPL0 (and as in the bare run).
            let mut no_am = case.clone();
            no_am.cr0 &= !CR0_AM;
            let (cpl3, _, _) = in_set(Level::Cpl3, code, &no_am, 1, |_| {});
            let (cpl0, _, _) = in_set(Level::Cpl0, code, &no_am, 1, |_| {});
            assert_eq!(cpl3, cpl0, "{name} offset {offset} AM = 0");
            assert!(
                !matches!(cpl3.result, Result_::Unsupported(_)),
                "{name} offset {offset} AM = 0: {:?}",
                cpl3.result
            );
            // The CPL0 set has no #AC at all, and an aligned operand with AC runs at CPL3.
            assert_eq!(cpl0_ac, cpl0, "{name} offset {offset}: CPL0 ignores AC");
            let mut aligned = case.clone();
            aligned.rax = DATA + 0x10;
            let (aligned3, _, _) = in_set(Level::Cpl3, code, &aligned, 1, |_| {});
            let mut aligned_off = aligned.clone();
            aligned_off.rflags = RFLAGS_BASE;
            let (aligned_no_ac, _, _) = in_set(Level::Cpl3, code, &aligned_off, 1, |_| {});
            assert_eq!(aligned3.result, Result_::Retired, "{name} aligned");
            let normalized = |mut observed: Observed| {
                observed.rflags &= !RFLAGS_AC;
                observed
            };
            assert_eq!(
                normalized(aligned3),
                normalized(aligned_no_ac),
                "{name} aligned"
            );
        }
    }
}

#[test]
fn mmx_and_the_forms_that_need_the_mmx_registers_stay_refused_at_cpl3() {
    let forms: &[(&str, &[u8], u8)] = &[
        ("PADDW mm0,mm1", &[0x0f, 0xfd, 0xc1], 0xfd),
        ("PXOR mm0,[rax]", &[0x0f, 0xef, 0x00], 0xef),
        ("PSLLQ mm0,4", &[0x0f, 0x73, 0xf0, 0x04], 0x73),
        ("PSHUFW mm0,mm1,0x1b", &[0x0f, 0x70, 0xc1, 0x1b], 0x70),
        ("MOVD mm0,eax", &[0x0f, 0x6e, 0xc0], 0x6e),
        ("MOVQ mm0,[rax]", &[0x0f, 0x6f, 0x00], 0x6f),
        ("MOVQ [rax],mm0", &[0x0f, 0x7f, 0x00], 0x7f),
        ("EMMS", &[0x0f, 0x77], 0x77),
        ("PSHUFB mm0,mm1", &[0x0f, 0x38, 0x00, 0xc1], 0x00),
        ("PALIGNR mm0,mm1,4", &[0x0f, 0x3a, 0x0f, 0xc1, 0x04], 0x0f),
        ("CVTPI2PS xmm1,mm2", &[0x0f, 0x2a, 0xca], 0x2a),
        ("CVTPI2PD xmm1,mm2", &[0x66, 0x0f, 0x2a, 0xca], 0x2a),
        ("CVTPS2PI mm1,xmm2", &[0x0f, 0x2d, 0xca], 0x2d),
        ("CVTTPD2PI mm1,xmm2", &[0x66, 0x0f, 0x2c, 0xca], 0x2c),
        ("MOVDQ2Q mm1,xmm2", &[0xf2, 0x0f, 0xd6, 0xca], 0xd6),
        ("MOVQ2DQ xmm1,mm2", &[0xf3, 0x0f, 0xd6, 0xca], 0xd6),
        ("MOVNTQ [rax],mm0", &[0x0f, 0xe7, 0x00], 0xe7),
        ("MASKMOVQ mm0,mm1", &[0x0f, 0xf7, 0xc1], 0xf7),
        ("PMOVMSKB eax,mm1", &[0x0f, 0xd7, 0xc1], 0xd7),
        ("PEXTRW eax,mm1,2", &[0x0f, 0xc5, 0xc1, 0x02], 0xc5),
    ];
    for (name, code, opcode) in forms {
        let case = Case::new(true);
        let (cpl3, before, after) = in_set(Level::Cpl3, code, &case, 1, |_| {});
        assert_eq!(cpl3.result, Result_::Unsupported(*opcode), "{name}");
        assert_refused_unchanged(name, &cpl3, &before, &after, &case);
        assert_eq!(
            cpl3.data,
            initial_memory(code, &case).bytes[DATA as usize..DATA as usize + 0x500]
        );
    }
}

#[test]
fn fs_and_gs_overrides_on_the_memory_forms_stay_refused_at_cpl3() {
    for segment in [0x64u8, 0x65] {
        for (name, body) in [
            ("MOVSS load", &[0xf3u8, 0x0f, 0x10, 0x08][..]),
            ("MOVAPS load", &[0x0f, 0x28, 0x08]),
            ("ADDPS memory", &[0x0f, 0x58, 0x08]),
            ("PADDB memory", &[0x66, 0x0f, 0xfc, 0x08]),
            ("LDMXCSR", &[0x0f, 0xae, 0x10]),
        ] {
            // The override goes first (before the mandatory prefix), as a compiler emits it.
            let mut code = vec![segment];
            code.extend_from_slice(body);
            let case = Case::new(true);
            let (cpl3, before, after) = in_set(Level::Cpl3, &code, &case, 1, |_| {});
            assert!(
                matches!(cpl3.result, Result_::Unsupported(_)),
                "{name} {segment:#x}: {:?}",
                cpl3.result
            );
            assert_refused_unchanged(name, &cpl3, &before, &after, &case);
        }
    }
}

#[test]
fn lock_on_an_sse_form_is_ud_at_cpl3() {
    for (name, code) in [
        ("LOCK ADDPS", &[0xf0, 0x0f, 0x58, 0xca][..]),
        ("LOCK PADDB", &[0xf0, 0x66, 0x0f, 0xfc, 0xca]),
        ("LOCK MOVAPS", &[0xf0, 0x0f, 0x28, 0x08]),
    ] {
        let case = Case::new(true);
        let (cpl3, _, _) = in_set(Level::Cpl3, code, &case, 1, |_| {});
        let (cpl0, _, _) = in_set(Level::Cpl0, code, &case, 1, |_| {});
        assert_eq!(cpl3.result, fault(6), "{name}");
        assert_eq!(cpl3, cpl0, "{name}");
    }
}

#[test]
fn without_the_fp_member_the_cpl3_entry_keeps_refusing_every_sse_form() {
    for (name, code) in FORMS {
        let case = Case::new(true);
        let mut state = initial_state(&case, Level::Cpl3);
        let mut ram = initial_memory(code, &case);
        let before = state.clone();
        let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&CpuidProfile::X8664V1Stage4A);
        let result = run_with_cpl3_profile_set(
            &mut state,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(Level::Cpl3),
            },
            od_isa::INTEGER_FLAGS,
            &plain,
            None,
            || false,
        )
        .expect("admitted");
        assert!(
            matches!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported { .. })
            ),
            "{name}: {:?}",
            result.exit
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(ram.reads + ram.writes, 0, "{name}");
    }
}

#[test]
fn sse_forms_between_ordinary_cpl3_instructions_and_the_budget_per_instruction() {
    // mov eax, 0x2000 ; movaps xmm1,[rax] ; addps xmm1,xmm2 ; movaps [rax+0x40],xmm1 (as MOVUPS) ; nop
    let code = [
        0xb8, 0x00, 0x20, 0x00, 0x00, // mov eax, 0x2000
        0x0f, 0x28, 0x08, // movaps xmm1,[rax]
        0x0f, 0x58, 0xca, // addps xmm1,xmm2
        0x0f, 0x11, 0x48, 0x40, // movups [rax+0x40],xmm1
        0x0f, 0xae, 0x58, 0x60, // stmxcsr [rax+0x60]
    ];
    let case = Case::new(true);
    for budget in 1..=5u64 {
        let (cpl3, _, after) = in_set(Level::Cpl3, &code, &case, budget, |_| {});
        let (cpl0, _, _) = in_set(Level::Cpl0, &code, &case, budget, |_| {});
        assert_eq!(cpl3.result, Result_::Retired, "budget {budget}");
        assert_eq!(cpl3.executed, budget, "budget {budget}");
        assert_eq!(cpl3, cpl0, "budget {budget}");
        // The plain `step` has no STMXCSR: it is the reference for the first four instructions.
        if budget <= 4 {
            let partial = bare(&code, &case, Level::Cpl3, budget);
            assert_eq!(cpl3, partial, "budget {budget}");
            assert_eq!(after.sse.xmm, partial.xmm, "budget {budget}");
        }
    }
    let (cpl3, _, _) = in_set(Level::Cpl3, &code, &case, 5, |_| {});
    assert_eq!(cpl3.data[0x60..0x64], MXCSR_MASKED.to_le_bytes());
}
