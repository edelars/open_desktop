//! S2-39 (CPL0 set): the SSE state of the FP member of the CPL0 profile set lives in
//! `LongModeState.sse` while the set runs (loaded from and stored back to its
//! `GuestFpState` cell at the entry and at every exit), so the general SSE core of S2-34, S2-37, S2-38,
//! S2-54 and S2-55 executes in the set exactly as in the bare run: same registers, MXCSR, RFLAGS, memory and
//! outcome, including the CR0.EM/TS and CR4.OSFXSR gates and the unmasked SIMD exceptions. The x87 half of the
//! cell is not touched by SSE forms, and FXSAVE64/FXRSTOR64 move the same XMM/MXCSR state.
//!
//! The expected values are the bare-run results of the same instruction on the same state (the bare run is the
//! accepted S2-37/S2-38/S2-54/S2-55 oracle-validated path); no value in this file is model output of the set itself.

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

fn check(name: &str, code: &[u8], case: &Case, controls: bool, expect: Option<Result_>) {
    if let Err(message) = compare(name, code, case, controls, expect) {
        panic!("{message}");
    }
}

/// Register forms and [RAX]-addressed memory forms by the plan slice that introduced them.
const FORMS: &[(&str, &[u8])] = &[
    // S2-34: raw 128-bit moves.
    ("S2-34 MOVAPS xmm1,[rax]", &[0x0f, 0x28, 0x08]),
    ("S2-34 MOVAPS [rax],xmm1", &[0x0f, 0x29, 0x08]),
    ("S2-34 MOVUPS xmm1,[rax]", &[0x0f, 0x10, 0x08]),
    ("S2-34 MOVDQU xmm1,[rax]", &[0xf3, 0x0f, 0x6f, 0x08]),
    ("S2-34 MOVDQA [rax],xmm1", &[0x66, 0x0f, 0x7f, 0x08]),
    // S2-37: arithmetic and conversions.
    ("S2-37 ADDPS xmm1,xmm2", &[0x0f, 0x58, 0xca]),
    ("S2-37 SUBPD xmm1,xmm2", &[0x66, 0x0f, 0x5c, 0xca]),
    ("S2-37 MULSD xmm1,xmm2", &[0xf2, 0x0f, 0x59, 0xca]),
    ("S2-37 DIVSS xmm1,xmm2", &[0xf3, 0x0f, 0x5e, 0xca]),
    ("S2-37 ADDPS xmm1,[rax]", &[0x0f, 0x58, 0x08]),
    ("S2-37 MULSD xmm1,[rax]", &[0xf2, 0x0f, 0x59, 0x08]),
    ("S2-37 SQRTPD xmm1,xmm2", &[0x66, 0x0f, 0x51, 0xca]),
    ("S2-37 MINPS xmm1,xmm2", &[0x0f, 0x5d, 0xca]),
    ("S2-37 RCPPS xmm1,xmm2", &[0x0f, 0x53, 0xca]),
    ("S2-37 CVTSI2SD xmm1,rax", &[0xf2, 0x48, 0x0f, 0x2a, 0xc8]),
    ("S2-37 CVTSI2SS xmm1,eax", &[0xf3, 0x0f, 0x2a, 0xc8]),
    ("S2-37 CVTTSD2SI rcx,xmm1", &[0xf2, 0x48, 0x0f, 0x2c, 0xc9]),
    ("S2-37 CVTSS2SI ecx,xmm2", &[0xf3, 0x0f, 0x2d, 0xca]),
    ("S2-37 CVTDQ2PD xmm1,xmm2", &[0xf3, 0x0f, 0xe6, 0xca]),
    ("S2-37 CVTPD2DQ xmm1,xmm2", &[0xf2, 0x0f, 0xe6, 0xca]),
    // S2-38: compares, logic, permutes, MOVMSK.
    ("S2-38 CMPPS xmm1,xmm2,2", &[0x0f, 0xc2, 0xca, 0x02]),
    ("S2-38 UCOMISD xmm1,xmm2", &[0x66, 0x0f, 0x2e, 0xca]),
    ("S2-38 COMISS xmm1,xmm2", &[0x0f, 0x2f, 0xca]),
    ("S2-38 ANDPS xmm1,xmm2", &[0x0f, 0x54, 0xca]),
    ("S2-38 XORPD xmm1,xmm2", &[0x66, 0x0f, 0x57, 0xca]),
    ("S2-38 SHUFPS xmm1,xmm2,0x1b", &[0x0f, 0xc6, 0xca, 0x1b]),
    ("S2-38 UNPCKLPS xmm1,xmm2", &[0x0f, 0x14, 0xca]),
    ("S2-38 UNPCKHPD xmm1,xmm2", &[0x66, 0x0f, 0x15, 0xca]),
    ("S2-38 MOVMSKPS eax,xmm2", &[0x0f, 0x50, 0xc2]),
    // S2-54: SSE3 and SSSE3.
    ("S2-54 HADDPS xmm1,xmm2", &[0xf2, 0x0f, 0x7c, 0xca]),
    ("S2-54 ADDSUBPD xmm1,xmm2", &[0x66, 0x0f, 0xd0, 0xca]),
    ("S2-54 MOVDDUP xmm1,xmm2", &[0xf2, 0x0f, 0x12, 0xca]),
    ("S2-54 MOVSHDUP xmm1,xmm2", &[0xf3, 0x0f, 0x16, 0xca]),
    ("S2-54 PSHUFB xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x00, 0xca]),
    (
        "S2-54 PALIGNR xmm1,xmm2,5",
        &[0x66, 0x0f, 0x3a, 0x0f, 0xca, 0x05],
    ),
    ("S2-54 PABSD xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x1e, 0xca]),
    ("S2-54 PHADDW xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x01, 0xca]),
    // S2-55: SSE4.1.
    ("S2-55 PMINSD xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x39, 0xca]),
    ("S2-55 PMULLD xmm1,[rax]", &[0x66, 0x0f, 0x38, 0x40, 0x08]),
    (
        "S2-55 PBLENDW xmm1,xmm2,0x5a",
        &[0x66, 0x0f, 0x3a, 0x0e, 0xca, 0x5a],
    ),
    ("S2-55 PMOVSXBW xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x20, 0xca]),
    (
        "S2-55 PEXTRD eax,xmm1,2",
        &[0x66, 0x0f, 0x3a, 0x16, 0xc8, 0x02],
    ),
    ("S2-55 PTEST xmm1,xmm2", &[0x66, 0x0f, 0x38, 0x17, 0xca]),
    (
        "S2-55 ROUNDPS xmm1,xmm2,9",
        &[0x66, 0x0f, 0x3a, 0x08, 0xca, 0x09],
    ),
    (
        "S2-55 DPPS xmm1,xmm2,0xff",
        &[0x66, 0x0f, 0x3a, 0x40, 0xca, 0xff],
    ),
];

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

fn failures_of(case: &Case, controls: bool, expect: Option<Result_>) -> Vec<String> {
    FORMS
        .iter()
        .filter_map(|(name, code)| compare(name, code, case, controls, expect.clone()).err())
        .collect()
}

#[test]
fn every_general_sse_form_runs_in_the_set_like_the_bare_run() {
    for controls in [false, true] {
        for ordinary_operands in [false, true] {
            let case = Case::new(ordinary_operands);
            let failures = failures_of(&case, controls, Some(Result_::Retired));
            assert!(
                failures.is_empty(),
                "controls {controls}, ordinary operands {ordinary_operands}: {failures:#?}"
            );
        }
    }
}

#[test]
fn a_program_of_every_form_in_one_run_matches_the_bare_sequence() {
    // The forms that write EAX/ECX go last so that the [RAX] operands of the others keep their address.
    let (late, early): (Vec<_>, Vec<_>) = FORMS.iter().partition(|(name, _)| {
        name.contains("eax,") || name.contains("ecx,") || name.contains("rcx,")
    });
    let code: Vec<u8> = early
        .iter()
        .chain(late.iter())
        .flat_map(|(_, code)| code.iter().copied())
        .collect();
    let count = FORMS.len() as u64;
    for controls in [false, true] {
        for ordinary_operands in [false, true] {
            let case = Case::new(ordinary_operands);
            if let Err(message) = compare_program(
                "all forms",
                &code,
                &case,
                controls,
                count,
                Some(Result_::Retired),
            ) {
                panic!("controls {controls}, ordinary operands {ordinary_operands}: {message}");
            }
        }
    }
}

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_before_any_address_check() {
    // (CR0, CR4, vector): EM and a clear OSFXSR are #UD, TS alone is #NM, and #UD wins over #NM.
    let gates = [
        (CR0_BASE | CR0_EM, CR4_FP, 6),
        (CR0_BASE | CR0_TS, CR4_FP, 7),
        (CR0_BASE | CR0_EM | CR0_TS, CR4_FP, 6),
        (CR0_BASE, CR4_BASE | CR4_OSXMMEXCPT, 6),
        (CR0_BASE | CR0_TS, CR4_BASE, 6),
    ];
    for (cr0, cr4, vector) in gates {
        // A misaligned and a non-canonical [RAX] must not matter: the gate comes first.
        for rax in [DATA, DATA + 1, 0x0001_0000_0000_0000] {
            let mut case = Case::new(true);
            case.cr0 = cr0;
            case.cr4 = cr4;
            case.rax = rax;
            let failures = failures_of(&case, true, Some(fault(vector)));
            assert!(
                failures.is_empty(),
                "cr0 {cr0:#x} cr4 {cr4:#x} rax {rax:#x}: {failures:#?}"
            );
            // Nothing changed: the registers and MXCSR of the cell are the initial ones.
            for (name, code) in FORMS {
                let (observed, before, after) = in_set(code, &case);
                assert_eq!(after, before, "{name}");
                assert_eq!(observed.rip, CODE, "{name}");
            }
        }
    }
}

#[test]
fn an_unmasked_exception_is_xm_or_ud_by_cr4_osxmmexcpt_and_keeps_the_destination() {
    // DIVPS xmm1,xmm2 with a zero divisor and the divide-by-zero mask clear.
    let code = [0x0f, 0x5e, 0xca];
    for (cr4, vector) in [(CR4_FP, 19), (CR4_BASE | CR4_OSFXSR, 6)] {
        let mut case = Case::new(true);
        case.cr4 = cr4;
        case.xmm[2] = [0; 16];
        case.mxcsr = MXCSR_MASKED & !(1 << 9);
        check("DIVPS unmasked", &code, &case, true, Some(fault(vector)));
        let (observed, before, after) = in_set(&code, &case);
        assert_eq!(observed.rip, CODE);
        assert_eq!(
            after.sse.xmm, before.sse.xmm,
            "the destination is unchanged"
        );
        // The fault leaves the detected flag in MXCSR (ZE, bit 2) in the cell.
        assert_eq!(after.sse.mxcsr, case.mxcsr | 1 << 2);
    }
    // The masked form sets the flag and retires.
    let mut case = Case::new(true);
    case.xmm[2] = [0; 16];
    check("DIVPS masked", &code, &case, true, Some(Result_::Retired));
    let (_, _, after) = in_set(&code, &case);
    assert_eq!(after.sse.mxcsr, MXCSR_MASKED | 1 << 2);
}

#[test]
fn reserved_mxcsr_bits_and_bad_operands_behave_like_the_bare_run() {
    for reserved in [1u32 << 16, 1 << 31, 0xffff_0000] {
        let mut case = Case::new(true);
        case.mxcsr |= reserved;
        for (name, code) in FORMS {
            check(name, code, &case, false, None);
        }
    }
    // Misaligned packed operands are #GP(0), a non-canonical address is #GP(0), an unmapped one is the backend's.
    for (rax, expect) in [
        (DATA + 1, Some(gp0())),
        (0x0001_0000_0000_0000, Some(gp0())),
        (0x10_0000, None),
    ] {
        let mut case = Case::new(true);
        case.rax = rax;
        for (name, code) in [
            ("MOVAPS load", &[0x0f, 0x28, 0x08][..]),
            ("MOVAPS store", &[0x0f, 0x29, 0x08]),
            ("ADDPS memory", &[0x0f, 0x58, 0x08]),
            ("PMULLD memory", &[0x66, 0x0f, 0x38, 0x40, 0x08]),
        ] {
            for controls in [false, true] {
                check(name, code, &case, controls, expect.clone());
            }
            let (observed, before, after) = in_set(code, &case);
            if rax == 0x10_0000 {
                assert!(matches!(observed.result, Result_::Memory(_)), "{name}");
            }
            assert_eq!(
                after, before,
                "{name}: a refused access changes no SSE or x87 state"
            );
        }
    }
}

const FXSAVE64_RAX: [u8; 5] = [0x48, 0x0f, 0xae, 0x40, 0x40];
const FXRSTOR64_RBX: [u8; 5] = [0x48, 0x0f, 0xae, 0x4b, 0x40];
const IMAGE_IN: u64 = DATA + 0x40;
const IMAGE_OUT: u64 = DATA + 0x240;

fn image_with(mxcsr: u32) -> [u8; 512] {
    let mut image = [0u8; 512];
    image[0..2].copy_from_slice(&0x027fu16.to_le_bytes());
    image[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    for register in 0..16 {
        image[160 + register * 16..176 + register * 16].copy_from_slice(&xmm_pattern(register + 3));
    }
    image
}

fn case_with_image() -> Case {
    let mut case = Case::new(false);
    case.rax = DATA + 0x200;
    case.rbx = DATA;
    case.memory = [0; 16];
    case.xmm = core::array::from_fn(|register| xmm_pattern(register + 0x30));
    case
}

fn run_image_program(
    code: &[u8],
    case: &Case,
    image: &[u8; 512],
    count: u64,
) -> (RunExit, LongModeState, Ram, GuestFpState, GuestFpState) {
    let mut state = initial_state(case);
    let mut ram = initial_memory(code, case);
    ram.put(IMAGE_IN, image);
    let cell = cell_for(case);
    let before = cell.borrow().clone();
    let (exit, _) = run_in_set(&mut state, &mut ram, &cell, count);
    let after = cell.borrow().clone();
    (exit, state, ram, before, after)
}

#[test]
fn fxrstor64_then_fxsave64_round_trips_the_xmm_and_mxcsr_state() {
    let image = image_with(0x1fa0);
    let case = case_with_image();
    let code = [FXRSTOR64_RBX, FXSAVE64_RAX].concat();
    let (exit, state, ram, before, after) = run_image_program(&code, &case, &image, 2);
    assert_eq!(exit, RunExit::InstructionBudgetExhausted);
    assert!(state.sse.is_none() && state.x87.is_none());
    // The restore replaced the SSE half of the cell from the image...
    assert_ne!(before.sse, after.sse);
    assert_eq!(after.sse.mxcsr, 0x1fa0);
    for register in 0..16 {
        assert_eq!(
            after.sse.xmm[register],
            xmm_pattern(register + 3),
            "xmm{register}"
        );
    }
    // ...and the save wrote the same fields back.
    let saved = &ram.bytes[IMAGE_OUT as usize..IMAGE_OUT as usize + 512];
    assert_eq!(saved[24..28], image[24..28]);
    assert_eq!(saved[160..416], image[160..416]);
    // The x87 half came from the image as well (FCW); FXRSTOR64 clears FCS and FDS (Intel SDM
    // Vol. 1 §8.1.8; S7-32 W4, before it the set kept them).
    assert_eq!(after.x87.control_word, 0x027f);
    assert_ne!(before.x87.instruction_selector, Some(0));
    assert_eq!(after.x87.instruction_selector, Some(0));
    assert_eq!(after.x87.data_selector, Some(0));
}

#[test]
fn an_sse_instruction_between_restore_and_save_sees_the_restored_state_and_is_saved() {
    let image = image_with(0x1fa0);
    let case = case_with_image();
    // Reference: the same ADDPS in the bare run on the restored registers and MXCSR.
    let mut restored = Case::new(false);
    restored.xmm = core::array::from_fn(|register| xmm_pattern(register + 3));
    restored.mxcsr = 0x1fa0;
    let reference = bare(&[0x0f, 0x58, 0xca], &restored, false);
    assert_eq!(reference.result, Result_::Retired);
    let code = [&FXRSTOR64_RBX[..], &[0x0f, 0x58, 0xca], &FXSAVE64_RAX[..]].concat();
    let (exit, _, ram, _, after) = run_image_program(&code, &case, &image, 3);
    assert_eq!(exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(after.sse.xmm, reference.xmm);
    assert_eq!(after.sse.mxcsr, reference.mxcsr);
    let saved = &ram.bytes[IMAGE_OUT as usize..IMAGE_OUT as usize + 512];
    assert_eq!(saved[24..28], reference.mxcsr.to_le_bytes());
    for register in 0..16 {
        assert_eq!(
            saved[160 + register * 16..176 + register * 16],
            reference.xmm[register],
            "xmm{register}"
        );
    }
}

#[test]
fn fxsave64_stores_the_live_sse_state_including_flags_set_by_the_previous_form() {
    // DIVPS xmm1,xmm2 with a zero divisor (masked): sets ZE; the image then carries MXCSR with ZE and the result.
    let mut case = Case::new(true);
    case.xmm[2] = [0; 16];
    case.rax = DATA;
    let reference = bare(&[0x0f, 0x5e, 0xca], &case, false);
    let code = [&[0x0f, 0x5e, 0xca][..], &FXSAVE64_RAX[..]].concat();
    let (observed, _, after) = in_set_program(&code, &case, 2);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.mxcsr, reference.mxcsr);
    assert_eq!(reference.mxcsr, MXCSR_MASKED | 1 << 2);
    let saved = &observed.data[0x40..0x40 + 512];
    assert_eq!(saved[24..28], reference.mxcsr.to_le_bytes());
    assert_eq!(saved[160 + 16..160 + 32], reference.xmm[1]);
    assert_eq!(saved[160 + 32..160 + 48], case.xmm[2]);
}

#[test]
fn fxrstor64_with_reserved_mxcsr_bits_is_gp_and_changes_no_state() {
    // Bits 31:16 are reserved; DAZ (bit 6) is in the target mask (ADR-0014).
    for reserved in [1u32 << 16, 1 << 31, 0x0001_ffc0] {
        let image = image_with(0x1f80 | reserved);
        let case = case_with_image();
        let code = FXRSTOR64_RBX;
        let (exit, state, _, before, after) = run_image_program(&code, &case, &image, 1);
        assert_eq!(
            exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            })
        );
        assert_eq!(after, before, "{reserved:#x}");
        assert!(state.sse.is_none());
    }
}

#[test]
fn x87_forms_of_the_member_keep_working_around_sse_forms() {
    // ADDPS; FNINIT; ADDPS: FNINIT resets the x87 half of the cell only, the SSE state carries on.
    let case = Case::new(true);
    let one = bare(&[0x0f, 0x58, 0xca], &case, false);
    let mut twice_case = case.clone();
    twice_case.xmm = one.xmm;
    twice_case.mxcsr = one.mxcsr;
    let two = bare(&[0x0f, 0x58, 0xca], &twice_case, false);
    let code = [0x0f, 0x58, 0xca, 0xdb, 0xe3, 0x0f, 0x58, 0xca];
    let (observed, before, after) = in_set_program(&code, &case, 3);
    assert_eq!(observed.result, Result_::Retired);
    assert_eq!(after.sse.xmm, two.xmm);
    assert_eq!(after.sse.mxcsr, two.mxcsr);
    assert_eq!(after.x87.control_word, 0x037f);
    assert_eq!(after.x87.status_word, 0);
    assert_eq!(after.x87.tag_word, 0xffff);
    assert_eq!(
        after.x87.registers, before.x87.registers,
        "FNINIT keeps the register payloads"
    );
}

#[test]
fn without_the_fp_member_the_set_keeps_refusing_every_sse_form() {
    for (name, code) in FORMS {
        let case = Case::new(true);
        let mut state = initial_state(&case);
        let mut ram = initial_memory(code, &case);
        let before = state.clone();
        let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&CpuidProfile::X8664V1Stage4A);
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(),
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
    }
}

/// S7-32 W2: MOVSD/MOVSS with any memory addressing in the CPL0 set (bootmgfw: `movsd 0x40(%rbx),
/// %xmm0`). SDM Vol. 2B MOVSD/MOVSS: the load writes the low 64 (32) bits and zeroes the rest of
/// the destination XMM register; the store writes exactly 8 (4) bytes; REX.W is ignored, REX.R
/// selects XMM8..15. Expected values by hand from the memory and register patterns below.
#[test]
fn s7_32_scalar_sse_moves_take_any_addressing_in_the_set() {
    let case = Case::new(false);
    let mut data = [0u8; 0x40];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = 0x80 + index as u8;
    }
    let run = |code: &[u8]| {
        let mut state = initial_state(&case);
        let mut ram = initial_memory(code, &case);
        ram.put(DATA, &data);
        let cell = cell_for(&case);
        let (exit, retired) = run_in_set(&mut state, &mut ram, &cell, 1);
        assert_eq!(
            (exit, retired),
            (RunExit::InstructionBudgetExhausted, 1),
            "{code:02x?}"
        );
        assert_eq!(state.rip, CODE + code.len() as u64, "{code:02x?}");
        let xmm = cell.borrow().sse.xmm;
        (xmm, ram.bytes[DATA as usize..DATA as usize + 0x40].to_vec())
    };
    // movsd 0x10(%rbx), %xmm2: bytes 0x90..0x97, upper half zero.
    let (xmm, _) = run(&[0xf2, 0x0f, 0x10, 0x53, 0x10]);
    assert_eq!(
        xmm[2],
        [
            0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    // movss 0x4(%rbx), %xmm9 (REX.R): bytes 0x84..0x87, the rest zero.
    let (xmm, _) = run(&[0xf3, 0x44, 0x0f, 0x10, 0x4b, 0x04]);
    assert_eq!(
        xmm[9],
        [0x84, 0x85, 0x86, 0x87, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    // movsd %xmm1, 0x20(%rbx) with REX.W (ignored): eight bytes of xmm1 at DATA+0x20, nothing
    // else changes.
    let (_, memory) = run(&[0xf2, 0x48, 0x0f, 0x11, 0x4b, 0x20]);
    let mut expected = data.to_vec();
    expected[0x20..0x28].copy_from_slice(&xmm_pattern(1)[..8]);
    assert_eq!(memory, expected);
    // movss %xmm3, 0x30(%rbx): four bytes of xmm3.
    let (_, memory) = run(&[0xf3, 0x0f, 0x11, 0x5b, 0x30]);
    let mut expected = data.to_vec();
    expected[0x30..0x34].copy_from_slice(&xmm_pattern(3)[..4]);
    assert_eq!(memory, expected);
}
