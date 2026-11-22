//! S2-39 (integer SSE2 remainder, CPL0 set): the integer SSE2 instructions on `state.sse` that S2-39n left refused: the
//! packed integer arithmetic, logic, compare, pack/unpack and shift forms with an XMM or 16-byte memory source in any
//! addressing form (PADDB/W/D/Q, PSUBB/W/D/Q, PAND/PANDN/POR/PXOR, PCMPEQ*/PCMPGT*, PADDS*/PADDUS*/PSUBS*/PSUBUS*,
//! PMULLW/PMULHW/PMULHUW/PMADDWD/PMULUDQ/PSADBW, PAVG*/PMAX*/PMIN*, PUNPCK*, PACK*), the shifts by an XMM or memory
//! count (PSLL/PSRL/PSRA, `66 0F D1..D3, E1, E2, F1..F3`), PSHUFD/PSHUFHW/PSHUFLW, MOVD/MOVQ between XMM and general
//! registers or memory, MOVQ xmm/m64, xmm and xmm, xmm/m64, MOVLPS/MOVLPD/MOVHPS/MOVHPD, MOVHLPS/MOVLHPS, PEXTRW,
//! PINSRW and the register forms of MOVDQU/MOVDQA. CR0.EM/TS and CR4.OSFXSR gate all of them (`#UD`, `#NM`) before any
//! operand access; a packed 16-byte memory operand must be 16-byte aligned (`#GP(0)`).
//!
//! Every expected value is computed in this file from the Intel SDM definitions (the `reference` functions share no
//! code with `src/sse2_int.rs`), or taken from the plain `step` for forms its narrow profile already executes. No
//! value is model output of the new code. The physical oracle (Core i5-3210M) checked the same forms byte for byte; see
//! the S2-39 integer SSE2 report in `docs/reports/STAGE_2_REPORT.md`.

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

// ---------------------------------------------------------------------------------------------------------------
// Reference model of the integer SSE2 instructions, written from the Intel SDM definitions on 128-bit lanes held as
// wide integers (it shares no code with src/sse2_int.rs).
// ---------------------------------------------------------------------------------------------------------------

fn lanes(v: &[u8; 16], width: usize) -> Vec<u128> {
    v.chunks(width)
        .map(|chunk| {
            chunk
                .iter()
                .rev()
                .fold(0u128, |acc, byte| acc << 8 | u128::from(*byte))
        })
        .collect()
}

fn signed(x: u128, width: usize) -> i128 {
    let shift = 128 - 8 * width as u32;
    ((x << shift) as i128) >> shift
}

fn build(width: usize, values: &[i128]) -> [u8; 16] {
    assert_eq!(values.len() * width, 16);
    let mut out = [0u8; 16];
    for (index, value) in values.iter().enumerate() {
        out[index * width..(index + 1) * width]
            .copy_from_slice(&(*value as u128).to_le_bytes()[..width]);
    }
    out
}

fn binary(width: usize, d: &[u8; 16], s: &[u8; 16], f: impl Fn(u128, u128) -> i128) -> [u8; 16] {
    let values: Vec<i128> = lanes(d, width)
        .iter()
        .zip(lanes(s, width))
        .map(|(a, b)| f(*a, b))
        .collect();
    build(width, &values)
}

fn unpack(width: usize, d: &[u8; 16], s: &[u8; 16], high: bool) -> [u8; 16] {
    let (dl, sl) = (lanes(d, width), lanes(s, width));
    let half = dl.len() / 2;
    let base = if high { half } else { 0 };
    let mut values = Vec::new();
    for index in 0..half {
        values.push(dl[base + index] as i128);
        values.push(sl[base + index] as i128);
    }
    build(width, &values)
}

/// The result of `66 0F opcode` for the destination `d` and the source `s` (the packed forms without imm8).
fn reference(opcode: u8, d: &[u8; 16], s: &[u8; 16]) -> [u8; 16] {
    let count = lanes(s, 8)[0];
    match opcode {
        0xfc => binary(1, d, s, |a, b| (a + b) as i128),
        0xfd => binary(2, d, s, |a, b| (a + b) as i128),
        0xfe => binary(4, d, s, |a, b| (a + b) as i128),
        0xd4 => binary(8, d, s, |a, b| a.wrapping_add(b) as i128),
        0xf8 => binary(1, d, s, |a, b| a as i128 - b as i128),
        0xf9 => binary(2, d, s, |a, b| a as i128 - b as i128),
        0xfa => binary(4, d, s, |a, b| a as i128 - b as i128),
        0xfb => binary(8, d, s, |a, b| a as i128 - b as i128),
        0xec => binary(1, d, s, |a, b| {
            (signed(a, 1) + signed(b, 1)).clamp(-128, 127)
        }),
        0xed => binary(2, d, s, |a, b| {
            (signed(a, 2) + signed(b, 2)).clamp(-32768, 32767)
        }),
        0xe8 => binary(1, d, s, |a, b| {
            (signed(a, 1) - signed(b, 1)).clamp(-128, 127)
        }),
        0xe9 => binary(2, d, s, |a, b| {
            (signed(a, 2) - signed(b, 2)).clamp(-32768, 32767)
        }),
        0xdc => binary(1, d, s, |a, b| (a + b).min(0xff) as i128),
        0xdd => binary(2, d, s, |a, b| (a + b).min(0xffff) as i128),
        0xd8 => binary(1, d, s, |a, b| (a as i128 - b as i128).max(0)),
        0xd9 => binary(2, d, s, |a, b| (a as i128 - b as i128).max(0)),
        0xdb => binary(1, d, s, |a, b| (a & b) as i128),
        0xdf => binary(1, d, s, |a, b| (!a & b) as i128),
        0xeb => binary(1, d, s, |a, b| (a | b) as i128),
        0xef => binary(1, d, s, |a, b| (a ^ b) as i128),
        0x74 => binary(1, d, s, |a, b| if a == b { -1 } else { 0 }),
        0x75 => binary(2, d, s, |a, b| if a == b { -1 } else { 0 }),
        0x76 => binary(4, d, s, |a, b| if a == b { -1 } else { 0 }),
        0x64 => binary(
            1,
            d,
            s,
            |a, b| if signed(a, 1) > signed(b, 1) { -1 } else { 0 },
        ),
        0x65 => binary(
            2,
            d,
            s,
            |a, b| if signed(a, 2) > signed(b, 2) { -1 } else { 0 },
        ),
        0x66 => binary(
            4,
            d,
            s,
            |a, b| if signed(a, 4) > signed(b, 4) { -1 } else { 0 },
        ),
        0xd5 => binary(2, d, s, |a, b| signed(a, 2) * signed(b, 2)),
        0xe5 => binary(2, d, s, |a, b| (signed(a, 2) * signed(b, 2)) >> 16),
        0xe4 => binary(2, d, s, |a, b| ((a * b) >> 16) as i128),
        0xe0 => binary(1, d, s, |a, b| ((a + b + 1) >> 1) as i128),
        0xe3 => binary(2, d, s, |a, b| ((a + b + 1) >> 1) as i128),
        0xde => binary(1, d, s, |a, b| a.max(b) as i128),
        0xda => binary(1, d, s, |a, b| a.min(b) as i128),
        0xee => binary(2, d, s, |a, b| signed(a, 2).max(signed(b, 2))),
        0xea => binary(2, d, s, |a, b| signed(a, 2).min(signed(b, 2))),
        0xf5 => {
            let (dl, sl) = (lanes(d, 2), lanes(s, 2));
            let values: Vec<i128> = (0..4)
                .map(|i| {
                    signed(dl[2 * i], 2) * signed(sl[2 * i], 2)
                        + signed(dl[2 * i + 1], 2) * signed(sl[2 * i + 1], 2)
                })
                .collect();
            build(4, &values)
        }
        0xf4 => {
            let (dl, sl) = (lanes(d, 4), lanes(s, 4));
            build(8, &[(dl[0] * sl[0]) as i128, (dl[2] * sl[2]) as i128])
        }
        0xf6 => {
            let sum = |half: usize| -> i128 {
                (0..8)
                    .map(|k| {
                        (i32::from(d[8 * half + k]) - i32::from(s[8 * half + k])).abs() as i128
                    })
                    .sum()
            };
            build(8, &[sum(0), sum(1)])
        }
        0x60 => unpack(1, d, s, false),
        0x68 => unpack(1, d, s, true),
        0x61 => unpack(2, d, s, false),
        0x69 => unpack(2, d, s, true),
        0x62 => unpack(4, d, s, false),
        0x6a => unpack(4, d, s, true),
        0x6c => unpack(8, d, s, false),
        0x6d => unpack(8, d, s, true),
        0x63 | 0x67 => {
            let (lo, hi) = if opcode == 0x63 {
                (-128, 127)
            } else {
                (0, 255)
            };
            let values: Vec<i128> = lanes(d, 2)
                .into_iter()
                .chain(lanes(s, 2))
                .map(|w| signed(w, 2).clamp(lo, hi))
                .collect();
            build(1, &values)
        }
        0x6b => {
            let values: Vec<i128> = lanes(d, 4)
                .into_iter()
                .chain(lanes(s, 4))
                .map(|w| signed(w, 4).clamp(-32768, 32767))
                .collect();
            build(2, &values)
        }
        0xd1 | 0xd2 | 0xd3 => {
            let (width, bits) = match opcode {
                0xd1 => (2, 16),
                0xd2 => (4, 32),
                _ => (8, 64),
            };
            binary(width, d, s, |a, _| {
                if count >= bits {
                    0
                } else {
                    (a >> count) as i128
                }
            })
        }
        0xf1 | 0xf2 | 0xf3 => {
            let (width, bits) = match opcode {
                0xf1 => (2, 16),
                0xf2 => (4, 32),
                _ => (8, 64),
            };
            binary(width, d, s, |a, _| {
                if count >= bits {
                    0
                } else {
                    (a << count) as i128
                }
            })
        }
        0xe1 | 0xe2 => {
            let (width, bits) = if opcode == 0xe1 { (2, 16u128) } else { (4, 32) };
            binary(width, d, s, |a, _| signed(a, width) >> count.min(bits - 1))
        }
        _ => panic!("no reference for {opcode:#x}"),
    }
}

/// `PSHUFD` (`66`), `PSHUFHW` (`F3`) and `PSHUFLW` (`F2`).
fn reference_shuffle(prefix: u8, s: &[u8; 16], imm: u8) -> [u8; 16] {
    let pick = |index: usize| usize::from(imm >> (2 * index)) & 3;
    match prefix {
        0x66 => {
            let sl = lanes(s, 4);
            build(4, &(0..4).map(|i| sl[pick(i)] as i128).collect::<Vec<_>>())
        }
        0xf2 => {
            let sl = lanes(s, 2);
            build(
                2,
                &(0..8)
                    .map(|i| (if i < 4 { sl[pick(i)] } else { sl[i] }) as i128)
                    .collect::<Vec<_>>(),
            )
        }
        0xf3 => {
            let sl = lanes(s, 2);
            build(
                2,
                &(0..8)
                    .map(|i| (if i >= 4 { sl[4 + pick(i - 4)] } else { sl[i] }) as i128)
                    .collect::<Vec<_>>(),
            )
        }
        _ => panic!("not a shuffle prefix"),
    }
}

/// Packed forms of `66 0F opcode /r` with an XMM or 16-byte memory source and no immediate.
const PACKED: &[(&str, u8)] = &[
    ("PADDB", 0xfc),
    ("PADDW", 0xfd),
    ("PADDD", 0xfe),
    ("PADDQ", 0xd4),
    ("PSUBB", 0xf8),
    ("PSUBW", 0xf9),
    ("PSUBD", 0xfa),
    ("PSUBQ", 0xfb),
    ("PADDSB", 0xec),
    ("PADDSW", 0xed),
    ("PADDUSB", 0xdc),
    ("PADDUSW", 0xdd),
    ("PSUBSB", 0xe8),
    ("PSUBSW", 0xe9),
    ("PSUBUSB", 0xd8),
    ("PSUBUSW", 0xd9),
    ("PAND", 0xdb),
    ("PANDN", 0xdf),
    ("POR", 0xeb),
    ("PXOR", 0xef),
    ("PCMPEQB", 0x74),
    ("PCMPEQW", 0x75),
    ("PCMPEQD", 0x76),
    ("PCMPGTB", 0x64),
    ("PCMPGTW", 0x65),
    ("PCMPGTD", 0x66),
    ("PMULLW", 0xd5),
    ("PMULHW", 0xe5),
    ("PMULHUW", 0xe4),
    ("PMADDWD", 0xf5),
    ("PMULUDQ", 0xf4),
    ("PSADBW", 0xf6),
    ("PAVGB", 0xe0),
    ("PAVGW", 0xe3),
    ("PMAXUB", 0xde),
    ("PMINUB", 0xda),
    ("PMAXSW", 0xee),
    ("PMINSW", 0xea),
    ("PUNPCKLBW", 0x60),
    ("PUNPCKLWD", 0x61),
    ("PUNPCKLDQ", 0x62),
    ("PUNPCKLQDQ", 0x6c),
    ("PUNPCKHBW", 0x68),
    ("PUNPCKHWD", 0x69),
    ("PUNPCKHDQ", 0x6a),
    ("PUNPCKHQDQ", 0x6d),
    ("PACKSSWB", 0x63),
    ("PACKUSWB", 0x67),
    ("PACKSSDW", 0x6b),
    ("PSLLW", 0xf1),
    ("PSLLD", 0xf2),
    ("PSLLQ", 0xf3),
    ("PSRLW", 0xd1),
    ("PSRLD", 0xd2),
    ("PSRLQ", 0xd3),
    ("PSRAW", 0xe1),
    ("PSRAD", 0xe2),
];

const SHUFFLES: &[(&str, u8)] = &[("PSHUFD", 0x66), ("PSHUFHW", 0xf3), ("PSHUFLW", 0xf2)];

const EXTREME_BYTES: [u8; 16] = [
    0x00, 0x01, 0x7f, 0x80, 0x81, 0xff, 0x02, 0xfe, 0x40, 0xc0, 0x7e, 0x82, 0x55, 0xaa, 0x10, 0xf0,
];

fn from_words(words: [u16; 8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (index, word) in words.iter().enumerate() {
        out[2 * index..2 * index + 2].copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn from_dwords(dwords: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (index, dword) in dwords.iter().enumerate() {
        out[4 * index..4 * index + 4].copy_from_slice(&dword.to_le_bytes());
    }
    out
}

/// (destination, source) operand pairs: pseudo-random, byte/word/dword extremes, equal operands, all ones.
fn operand_pairs() -> Vec<([u8; 16], [u8; 16])> {
    let mut rot3 = EXTREME_BYTES;
    rot3.rotate_left(3);
    let mut rot5 = EXTREME_BYTES;
    rot5.rotate_left(5);
    let words = from_words([
        0x0000, 0x7fff, 0x8000, 0xffff, 0x0001, 0x8001, 0x4000, 0xc000,
    ]);
    let mut words_rot = words;
    words_rot.rotate_left(2);
    let dwords = from_dwords([0, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff]);
    let mut dwords_rot = dwords;
    dwords_rot.rotate_left(4);
    vec![
        (xmm_pattern(1), xmm_pattern(2)),
        (xmm_pattern(8), xmm_pattern(9)),
        (EXTREME_BYTES, rot3),
        (EXTREME_BYTES, rot5),
        (words, words_rot),
        (dwords, dwords_rot),
        (EXTREME_BYTES, EXTREME_BYTES),
        ([0xff; 16], [0x80; 16]),
        ([0x7f; 16], [0x7f; 16]),
        ([0; 16], EXTREME_BYTES),
    ]
}

/// The state the case starts in with the code length of the retired instruction accounted for: the reference every
/// expectation below modifies.
fn expected(case: &Case, code: &[u8]) -> Observed {
    let state = initial_state(case);
    let mut data = vec![SENTINEL; 0x500];
    data[..16].copy_from_slice(&case.memory);
    Observed {
        result: Result_::Retired,
        rip: CODE + code.len() as u64,
        rflags: state.rflags,
        gpr: state.gpr,
        xmm: case.xmm,
        mxcsr: case.mxcsr,
        data,
    }
}

/// The state after a form that did not retire: nothing changed, the result is `result`.
fn unchanged(case: &Case, code: &[u8], result: Result_) -> Observed {
    Observed {
        result,
        rip: CODE,
        ..expected(case, code)
    }
}

/// A register-direct encoding `[prefix] [REX] 0F opcode modrm [imm8]` with `reg` in ModRM.reg and `rm` in ModRM.rm.
fn reg_code(prefix: &[u8], opcode: u8, reg: u8, rm: u8, imm: Option<u8>) -> Vec<u8> {
    let rex = 0x40 | ((reg >> 3) << 2) | (rm >> 3);
    let mut code = prefix.to_vec();
    if rex != 0x40 {
        code.push(rex);
    }
    code.extend_from_slice(&[0x0f, opcode, 0xc0 | (reg & 7) << 3 | (rm & 7)]);
    code.extend(imm);
    code
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Rax,
    RaxDisp8,
    RipRelative,
    Sib,
    Address32,
}

const MODES: [Mode; 5] = [
    Mode::Rax,
    Mode::RaxDisp8,
    Mode::RipRelative,
    Mode::Sib,
    Mode::Address32,
];

/// A memory form whose operand is the 16 bytes at DATA, with the registers it needs (rax, rbx, rcx).
fn mem_code(
    prefix: &[u8],
    opcode: u8,
    reg: u8,
    mode: Mode,
    imm: Option<u8>,
) -> (Vec<u8>, impl Fn(&mut LongModeState) + use<>) {
    let mut code = Vec::new();
    if matches!(mode, Mode::Address32) {
        code.push(0x67);
    }
    code.extend_from_slice(prefix);
    if reg >= 8 {
        code.push(0x44);
    }
    code.extend_from_slice(&[0x0f, opcode]);
    let field = (reg & 7) << 3;
    let (mut rax, mut rbx, rcx) = (DATA, DATA, 4u64);
    match mode {
        Mode::Rax => code.push(field),
        Mode::RaxDisp8 => {
            code.extend_from_slice(&[0x40 | field, 0x10]);
            rax = DATA - 0x10;
        }
        Mode::RipRelative => {
            let length = code.len() + 1 + 4 + usize::from(imm.is_some());
            code.push(0x05 | field);
            code.extend_from_slice(
                &((DATA as i64 - (CODE as i64 + length as i64)) as i32).to_le_bytes(),
            );
        }
        Mode::Sib => {
            code.extend_from_slice(&[0x44 | field, 0x8b, 0x20]);
            rbx = DATA - 0x20 - 16;
        }
        Mode::Address32 => {
            code.push(field);
            rax = 0xdead_beef_0000_0000 | DATA;
        }
    }
    code.extend(imm);
    (code, move |state: &mut LongModeState| {
        state.gpr[0] = rax;
        state.gpr[3] = rbx;
        state.gpr[1] = rcx;
    })
}

fn case_with(
    dest_register: usize,
    dest: [u8; 16],
    source_register: Option<usize>,
    source: [u8; 16],
) -> Case {
    let mut case = Case::new(false);
    case.xmm[dest_register] = dest;
    if let Some(register) = source_register {
        case.xmm[register] = source;
    }
    case.memory = source;
    case
}

#[test]
fn every_packed_register_form_equals_the_reference() {
    let mut forms = 0;
    for (name, opcode) in PACKED {
        for (dst, src) in [(1u8, 2u8), (3, 3), (9, 10), (0, 15)] {
            for (d, s) in operand_pairs() {
                let code = reg_code(&[0x66], *opcode, dst, src, None);
                // With one register for both operands the source value is the destination value.
                let d = if dst == src { s } else { d };
                let case = case_with(usize::from(dst), d, Some(usize::from(src)), s);
                let (observed, before, after) = in_set(&code, &case);
                let mut want = expected(&case, &code);
                want.xmm[usize::from(dst)] = reference(*opcode, &d, &s);
                assert_eq!(observed, want, "{name} xmm{dst},xmm{src}");
                assert_eq!(after.x87, before.x87, "{name}");
                forms += 1;
            }
        }
    }
    assert_eq!(forms, 57 * 4 * 10);
}

#[test]
fn every_packed_memory_form_equals_the_reference_in_every_addressing_form() {
    let mut forms = 0;
    for (name, opcode) in PACKED {
        for (index, (d, s)) in operand_pairs().into_iter().enumerate() {
            for mode in MODES {
                let reg = if index % 3 == 0 { 9 } else { 1 };
                let (code, setup) = mem_code(&[0x66], *opcode, reg, mode, None);
                let case = case_with(usize::from(reg), d, None, s);
                let (observed, before, after) = in_set_with(&code, &case, 1, setup);
                let mut want = expected(&case, &code);
                want.xmm[usize::from(reg)] = reference(*opcode, &d, &s);
                // The base registers are test inputs; compare the rest of the state.
                let mut seen = observed.clone();
                seen.gpr = want.gpr;
                assert_eq!(seen, want, "{name} {mode:?} xmm{reg}");
                assert_eq!(after.x87, before.x87, "{name}");
                forms += 1;
            }
        }
    }
    assert_eq!(forms, 57 * 10 * 5);
}

/// The plain step is the oracle-validated reference for the register forms it already retires (the narrow profile);
/// the memory form in the set must equal it with the same bytes in a source register.
#[test]
fn memory_forms_equal_the_plain_step_register_form_where_the_plain_step_has_one() {
    // PMULHW and the shifts by an XMM count have no narrow register form: the plain step executes them with the code of
    // this slice, so they are not an independent reference.
    const NO_NARROW_FORM: [u8; 9] = [0xe5, 0xd1, 0xd2, 0xd3, 0xe1, 0xe2, 0xf1, 0xf2, 0xf3];
    let mut compared = 0;
    for (name, opcode) in PACKED {
        if NO_NARROW_FORM.contains(opcode) {
            continue;
        }
        let reg_form = reg_code(&[0x66], *opcode, 1, 2, None);
        for (d, s) in operand_pairs() {
            let reg_case = case_with(1, d, Some(2), s);
            let reference = bare(&reg_form, &reg_case, false);
            assert_eq!(reference.result, Result_::Retired, "{name}");
            let (code, setup) = mem_code(&[0x66], *opcode, 1, Mode::Rax, None);
            let (observed, _, _) = in_set_with(&code, &case_with(1, d, None, s), 1, setup);
            assert_eq!(observed.result, Result_::Retired, "{name}");
            assert_eq!(observed.xmm[1], reference.xmm[1], "{name}");
            assert_eq!(observed.rflags, reference.rflags, "{name}");
            assert_eq!(observed.mxcsr, reference.mxcsr, "{name}");
            compared += 1;
        }
    }
    assert_eq!(compared, (57 - 9) * 10);
}

#[test]
fn shifts_by_an_xmm_count_follow_the_count_in_the_low_quadword() {
    let counts: [u64; 18] = [
        0,
        1,
        2,
        7,
        8,
        15,
        16,
        17,
        31,
        32,
        33,
        63,
        64,
        65,
        127,
        255,
        0x1_0000_0000,
        u64::MAX,
    ];
    let shifts: [u8; 8] = [0xd1, 0xd2, 0xd3, 0xe1, 0xe2, 0xf1, 0xf2, 0xf3];
    for opcode in shifts {
        for count in counts {
            for d in [
                xmm_pattern(1),
                EXTREME_BYTES,
                [0x80; 16],
                from_words([0x8001, 0x7fff, 0xffff, 1, 0x8000, 2, 3, 0xfffe]),
            ] {
                let mut s = [0xa5u8; 16];
                s[..8].copy_from_slice(&count.to_le_bytes());
                let want_value = reference(opcode, &d, &s);
                // Register form.
                let code = reg_code(&[0x66], opcode, 1, 2, None);
                let case = case_with(1, d, Some(2), s);
                let (observed, _, _) = in_set(&code, &case);
                let mut want = expected(&case, &code);
                want.xmm[1] = want_value;
                assert_eq!(observed, want, "{opcode:#x} count {count:#x} register form");
                // Memory form: the whole m128 is loaded, the high quadword is ignored.
                let (code, setup) = mem_code(&[0x66], opcode, 1, Mode::Rax, None);
                let case = case_with(1, d, None, s);
                let (observed, _, _) = in_set_with(&code, &case, 1, setup);
                let mut want = expected(&case, &code);
                want.xmm[1] = want_value;
                let mut seen = observed.clone();
                seen.gpr = want.gpr;
                assert_eq!(seen, want, "{opcode:#x} count {count:#x} memory form");
            }
        }
    }
}

#[test]
fn packed_forms_give_the_values_of_the_sdm_definitions_for_hand_computed_operands() {
    // (opcode, destination, source, expected destination), words/bytes written lane 0 first.
    let b = |bytes: [u8; 16]| bytes;
    let case = |opcode: u8, d: [u8; 16], s: [u8; 16], want: [u8; 16]| {
        let code = reg_code(&[0x66], opcode, 1, 2, None);
        let case = case_with(1, d, Some(2), s);
        let (observed, _, _) = in_set(&code, &case);
        assert_eq!(observed.xmm[1], want, "{opcode:#x}");
        let (code, setup) = mem_code(&[0x66], opcode, 1, Mode::Rax, None);
        let (observed, _, _) = in_set_with(&code, &case_with(1, d, None, s), 1, setup);
        assert_eq!(observed.xmm[1], want, "{opcode:#x} memory");
    };
    let rep = |byte: u8| [byte; 16];
    // PADDB wraps: 0xff + 0x01 = 0x00, 0x7f + 0x7f = 0xfe.
    case(
        0xfc,
        b([0xff, 0x7f, 0x80, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([1, 0x7f, 0x80, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([0, 0xfe, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PADDSB saturates: 0x7f + 1 = 0x7f, 0x80 + 0xff = 0x80 (-128 + -1), 100 + 100 = 127.
    case(
        0xec,
        b([0x7f, 0x80, 100, 0xf0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([1, 0xff, 100, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([0x7f, 0x80, 0x7f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PSUBUSB clamps at zero: 0 - 1 = 0, 0xff - 0x0f = 0xf0.
    case(
        0xd8,
        b([0, 0xff, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([1, 0x0f, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([0, 0xf0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PMULHW: 0x8000 * 0x8000 = 0x4000_0000 -> 0x4000; -1 * 1 = -1 -> 0xffff; PMULHUW: 0xffff * 0xffff -> 0xfffe.
    case(
        0xe5,
        from_words([0x8000, 0xffff, 0x7fff, 2, 0, 0, 0, 0]),
        from_words([0x8000, 1, 0x7fff, 3, 0, 0, 0, 0]),
        from_words([0x4000, 0xffff, 0x3fff, 0, 0, 0, 0, 0]),
    );
    case(
        0xe4,
        from_words([0xffff, 0x8000, 2, 0, 0, 0, 0, 0]),
        from_words([0xffff, 0x8000, 3, 5, 0, 0, 0, 0]),
        from_words([0xfffe, 0x4000, 0, 0, 0, 0, 0, 0]),
    );
    // PMADDWD: 0x8000*0x8000 + 0x8000*0x8000 = 0x8000_0000 (wraps), 1*2 + 3*4 = 14.
    case(
        0xf5,
        from_words([0x8000, 0x8000, 1, 3, 0, 0, 0, 0]),
        from_words([0x8000, 0x8000, 2, 4, 0, 0, 0, 0]),
        from_dwords([0x8000_0000, 14, 0, 0]),
    );
    // PACKSSWB: 0x7fff -> 0x7f, 0x8000 -> 0x80, 0x0100 -> 0x7f, 5 -> 5; the source supplies the high eight bytes.
    case(
        0x63,
        from_words([0x7fff, 0x8000, 0x0100, 5, 0, 0, 0, 0]),
        from_words([0xff80, 0xff7f, 1, 0, 0, 0, 0, 0]),
        b([
            0x7f, 0x80, 0x7f, 5, 0, 0, 0, 0, 0x80, 0x80, 1, 0, 0, 0, 0, 0,
        ]),
    );
    // PACKUSWB: negative -> 0, 0x0100 -> 0xff.
    case(
        0x67,
        from_words([0xffff, 0x0100, 0x00ff, 7, 0, 0, 0, 0]),
        from_words([0x8000, 0x7fff, 0, 0, 0, 0, 0, 0]),
        b([0, 0xff, 0xff, 7, 0, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0, 0]),
    );
    // PUNPCKLBW interleaves the low bytes of the destination (even) and the source (odd).
    case(
        0x60,
        b([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        b([
            101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
        ]),
        b([
            1, 101, 2, 102, 3, 103, 4, 104, 5, 105, 6, 106, 7, 107, 8, 108,
        ]),
    );
    case(
        0x6d,
        b([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        b([
            101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
        ]),
        b([
            9, 10, 11, 12, 13, 14, 15, 16, 109, 110, 111, 112, 113, 114, 115, 116,
        ]),
    );
    // PSADBW: |1-5| + |2-2| + ... summed into the low word of each quadword.
    case(
        0xf6,
        b([1, 2, 3, 4, 5, 6, 7, 8, 0xff, 0, 0xff, 0, 0, 0, 0, 0]),
        b([5, 2, 0, 4, 9, 6, 7, 1, 0, 0xff, 0, 0, 0, 0, 0, 0]),
        from_words([4 + 3 + 4 + 7, 0, 0, 0, 255 + 255 + 255, 0, 0, 0]),
    );
    // PMULUDQ multiplies dwords 0 and 2 into the two quadwords.
    case(
        0xf4,
        from_dwords([0xffff_ffff, 9, 3, 9]),
        from_dwords([0xffff_ffff, 9, 5, 9]),
        b([1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 15, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PAVGB rounds up: (1+2+1)>>1 = 2, (0xff+0xff+1)>>1 = 0xff.
    case(
        0xe0,
        b([1, 0xff, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([2, 0xff, 1, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([2, 0xff, 1, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PCMPGTB is signed: 0x01 > 0xff (-1) is true, 0x80 > 0x7f is false.
    case(
        0x64,
        b([1, 0x80, 0x7f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([0xff, 0x7f, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        b([0xff, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // PANDN is (not destination) and source.
    case(0xdf, rep(0xf0), rep(0x3c), rep(0x0c));
    // PSLLW by 4, PSRAW by 20 (sign fill), PSRLQ by 63, PSLLQ by 64 (clears).
    let count = |c: u64| {
        let mut s = [0u8; 16];
        s[..8].copy_from_slice(&c.to_le_bytes());
        s
    };
    case(
        0xf1,
        from_words([1, 0x8000, 0x7fff, 0xffff, 0, 0, 0, 0]),
        count(4),
        from_words([0x10, 0, 0xfff0, 0xfff0, 0, 0, 0, 0]),
    );
    case(
        0xe1,
        from_words([0x8000, 0x7fff, 0xffff, 1, 0, 0, 0, 0]),
        count(20),
        from_words([0xffff, 0, 0xffff, 0, 0, 0, 0, 0]),
    );
    case(
        0xd3,
        from_dwords([0xffff_ffff, 0xffff_ffff, 0x8000_0000, 0x8000_0000]),
        count(63),
        from_dwords([1, 0, 1, 0]),
    );
    case(0xf3, rep(0xff), count(64), rep(0));
    case(
        0xe2,
        from_dwords([0x8000_0000, 0x7fff_ffff, 0xffff_fff0, 5]),
        count(31),
        from_dwords([0xffff_ffff, 0, 0xffff_ffff, 0]),
    );
}

#[test]
fn shuffles_with_every_immediate_equal_the_reference_in_register_and_memory_form() {
    let source = xmm_pattern(5);
    for (name, prefix) in SHUFFLES {
        for imm in 0..=255u8 {
            let want_value = reference_shuffle(*prefix, &source, imm);
            // Register form: PSHUFx xmm1, xmm2, imm8; the destination does not take part.
            let code = reg_code(&[*prefix], 0x70, 1, 2, Some(imm));
            let case = case_with(1, xmm_pattern(1), Some(2), source);
            let (observed, _, _) = in_set(&code, &case);
            let mut want = expected(&case, &code);
            want.xmm[1] = want_value;
            assert_eq!(observed, want, "{name} {imm:#x} register form");
            // Memory form, `[rax]` and RIP-relative (the displacement is relative to the end of the imm8).
            for mode in [Mode::Rax, Mode::RipRelative] {
                let (code, setup) = mem_code(&[*prefix], 0x70, 9, mode, Some(imm));
                let case = case_with(9, xmm_pattern(9), None, source);
                let (observed, _, _) = in_set_with(&code, &case, 1, setup);
                let mut want = expected(&case, &code);
                want.xmm[9] = want_value;
                let mut seen = observed.clone();
                seen.gpr = want.gpr;
                assert_eq!(seen, want, "{name} {imm:#x} {mode:?}");
            }
        }
    }
}

#[test]
fn a_misaligned_packed_memory_operand_is_a_general_protection_fault_before_the_access() {
    for (name, opcode) in PACKED
        .iter()
        .map(|(n, o)| (*n, *o))
        .chain([("PSHUFD", 0x70)])
    {
        for delta in [1u64, 2, 3, 4, 7, 8, 15] {
            let prefix: &[u8] = &[0x66];
            let imm = (opcode == 0x70).then_some(0x1b);
            let (code, _) = mem_code(prefix, opcode, 1, Mode::Rax, imm);
            let case = case_with(1, xmm_pattern(1), None, xmm_pattern(2));
            let (observed, before, after) =
                in_set_with(&code, &case, 1, |state| state.gpr[0] = DATA + delta);
            let mut want = unchanged(&case, &code, gp0());
            want.gpr[0] = DATA + delta;
            assert_eq!(observed, want, "{name} +{delta}");
            assert_eq!(after, before, "{name}");
        }
    }
}

#[test]
fn a_packed_operand_at_the_end_of_memory_reports_the_backend_error_and_changes_nothing() {
    for (name, opcode) in [("PADDB", 0xfcu8), ("PSLLQ", 0xf3), ("PMULHW", 0xe5)] {
        let (code, _) = mem_code(&[0x66], opcode, 1, Mode::Rax, None);
        let case = case_with(1, xmm_pattern(1), None, xmm_pattern(2));
        // Aligned, but outside the RAM of the test.
        let (observed, before, after) =
            in_set_with(&code, &case, 1, |state| state.gpr[0] = RAM as u64);
        let mut want = unchanged(
            &case,
            &code,
            Result_::Memory(MemoryError::Unmapped {
                address: RAM as u64,
                access: AccessKind::Read,
            }),
        );
        want.gpr[0] = RAM as u64;
        assert_eq!(observed, want, "{name}");
        assert_eq!(after, before, "{name}");
        // A non-canonical base is #GP(0), an RBP base #SS(0).
        let (observed, _, _) = in_set_with(&code, &case, 1, |state| state.gpr[0] = 1 << 47);
        assert_eq!(observed.result, gp0(), "{name}");
        let rbp_form = [0x66, 0x0f, opcode, 0x4d, 0x00];
        let (observed, _, _) = in_set_with(&rbp_form, &case, 1, |state| state.gpr[5] = 1 << 47);
        assert_eq!(
            observed.result,
            Result_::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None
            }),
            "{name}"
        );
    }
}

#[test]
fn the_alignment_fault_comes_before_the_backend_error_and_the_gates_before_both() {
    let (code, _) = mem_code(&[0x66], 0xfc, 1, Mode::Rax, None);
    // Misaligned and outside the RAM: #GP(0) first.
    let case = case_with(1, xmm_pattern(1), None, xmm_pattern(2));
    let (observed, _, _) = in_set_with(&code, &case, 1, |state| state.gpr[0] = RAM as u64 + 1);
    assert_eq!(observed.result, gp0());
    // Gates before any address check (a non-canonical, misaligned base): #UD wins over #NM.
    for (cr0, cr4, vector) in [
        (CR0_BASE | CR0_EM, CR4_FP, 6),
        (CR0_BASE | CR0_TS, CR4_FP, 7),
        (CR0_BASE | CR0_EM | CR0_TS, CR4_FP, 6),
        (CR0_BASE, CR4_FP & !CR4_OSFXSR, 6),
        (CR0_BASE | CR0_TS, CR4_FP & !CR4_OSFXSR, 6),
    ] {
        let mut case = case_with(1, xmm_pattern(1), None, xmm_pattern(2));
        case.cr0 = cr0;
        case.cr4 = cr4;
        let (observed, before, after) =
            in_set_with(&code, &case, 1, |state| state.gpr[0] = (1 << 47) + 1);
        let mut want = unchanged(&case, &code, fault(vector));
        want.gpr[0] = (1 << 47) + 1;
        assert_eq!(observed, want, "cr0 {cr0:#x} cr4 {cr4:#x}");
        assert_eq!(after, before);
    }
}

#[test]
fn every_form_of_the_slice_is_gated_by_cr0_em_cr0_ts_and_cr4_osfxsr() {
    // One encoding of each form class; the operand (when memory) is aligned and valid, so only the gate decides.
    let forms: &[(&str, &[u8])] = &[
        ("PADDB xmm1,xmm2", &[0x66, 0x0f, 0xfc, 0xca]),
        ("PADDB xmm1,[rax]", &[0x66, 0x0f, 0xfc, 0x08]),
        ("PMULHW xmm1,xmm2", &[0x66, 0x0f, 0xe5, 0xca]),
        ("PSLLW xmm1,xmm2", &[0x66, 0x0f, 0xf1, 0xca]),
        ("PSHUFD xmm1,xmm2,1", &[0x66, 0x0f, 0x70, 0xca, 0x01]),
        ("PSHUFHW xmm1,[rax],1", &[0xf3, 0x0f, 0x70, 0x08, 0x01]),
        ("PSHUFLW xmm1,xmm2,1", &[0xf2, 0x0f, 0x70, 0xca, 0x01]),
        ("MOVQ xmm1,xmm2", &[0xf3, 0x0f, 0x7e, 0xca]),
        ("MOVQ [rax],xmm1", &[0x66, 0x0f, 0xd6, 0x08]),
        ("MOVQ xmm1,xmm2 (D6)", &[0x66, 0x0f, 0xd6, 0xd1]),
        ("MOVD ecx,xmm1", &[0x66, 0x0f, 0x7e, 0xc9]),
        ("MOVQ [rax],xmm1 (7E)", &[0x66, 0x48, 0x0f, 0x7e, 0x08]),
        ("MOVLPS xmm1,[rax]", &[0x0f, 0x12, 0x08]),
        ("MOVLPD [rax],xmm1", &[0x66, 0x0f, 0x13, 0x08]),
        ("MOVHPD [rax],xmm1", &[0x66, 0x0f, 0x17, 0x08]),
        ("MOVHLPS xmm1,xmm2", &[0x0f, 0x12, 0xca]),
        ("MOVLHPS xmm1,xmm2", &[0x0f, 0x16, 0xca]),
        ("PEXTRW ecx,xmm2,3", &[0x66, 0x0f, 0xc5, 0xca, 0x03]),
        ("PINSRW xmm2,ecx,3", &[0x66, 0x0f, 0xc4, 0xd1, 0x03]),
        ("MOVDQU xmm1,xmm2", &[0xf3, 0x0f, 0x6f, 0xca]),
        ("MOVDQU xmm2,xmm1", &[0xf3, 0x0f, 0x7f, 0xca]),
        ("MOVDQA xmm2,xmm1", &[0x66, 0x0f, 0x7f, 0xca]),
    ];
    for (name, code) in forms {
        for (cr0, cr4, vector) in [
            (CR0_BASE | CR0_EM, CR4_FP, Some(6)),
            (CR0_BASE | CR0_TS, CR4_FP, Some(7)),
            (CR0_BASE | CR0_EM | CR0_TS, CR4_FP, Some(6)),
            (CR0_BASE, CR4_FP & !CR4_OSFXSR, Some(6)),
            (CR0_BASE | CR0_TS, CR4_FP & !CR4_OSFXSR, Some(6)),
            (CR0_BASE, CR4_FP, None),
            // CR4.OSXMMEXCPT has no part in these forms (no SIMD floating-point exception exists for them).
            (CR0_BASE, CR4_FP & !CR4_OSXMMEXCPT, None),
        ] {
            let mut case = Case::new(false);
            case.cr0 = cr0;
            case.cr4 = cr4;
            let (observed, before, after) = in_set(code, &case);
            match vector {
                Some(vector) => {
                    assert_eq!(
                        observed,
                        unchanged(&case, code, fault(vector)),
                        "{name} cr0 {cr0:#x} cr4 {cr4:#x}"
                    );
                    assert_eq!(after, before, "{name}");
                }
                None => assert_eq!(
                    observed.result,
                    Result_::Retired,
                    "{name} cr0 {cr0:#x} cr4 {cr4:#x}"
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// MOVD/MOVQ, MOVLP*/MOVHP*, MOVHLPS/MOVLHPS, PEXTRW, PINSRW, MOVDQU/MOVDQA register forms.
// ---------------------------------------------------------------------------------------------------------------

fn lo(value: [u8; 16]) -> [u8; 8] {
    value[..8].try_into().unwrap()
}

fn hi(value: [u8; 16]) -> [u8; 8] {
    value[8..].try_into().unwrap()
}

fn join(low: [u8; 8], high: [u8; 8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&low);
    out[8..].copy_from_slice(&high);
    out
}

#[test]
fn movd_and_movq_from_an_xmm_register_to_a_general_register_or_memory() {
    let case = Case::new(false);
    let low = lo(case.xmm[1]);
    // MOVD ecx, xmm1: the low dword, zero-extended into the 64-bit register.
    let code = [0x66, 0x0f, 0x7e, 0xc9];
    let (observed, before, after) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.gpr[1] = u64::from(u32::from_le_bytes(low[..4].try_into().unwrap()));
    assert_eq!(observed, want);
    assert_eq!(after.x87, before.x87);
    // MOVQ rcx, xmm1 (REX.W): the low qword.
    let code = [0x66, 0x48, 0x0f, 0x7e, 0xc9];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.gpr[1] = u64::from_le_bytes(low);
    assert_eq!(observed, want);
    // MOVQ r8, xmm9 (REX.W, REX.R, REX.B).
    let code = [0x66, 0x4d, 0x0f, 0x7e, 0xc8];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.gpr[8] = u64::from_le_bytes(lo(case.xmm[9]));
    assert_eq!(observed, want);
    // MOVD r15d, xmm10 (REX.R, REX.B): the upper half of r15 is cleared.
    let code = [0x66, 0x45, 0x0f, 0x7e, 0xd7];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.gpr[15] = u64::from(u32::from_le_bytes(
        lo(case.xmm[10])[..4].try_into().unwrap(),
    ));
    assert_eq!(observed, want);
    // MOVD [rax], xmm1 and MOVQ [rax], xmm1 write 4 and 8 bytes; the rest of the memory is untouched.
    for (code, width) in [
        (vec![0x66, 0x0f, 0x7e, 0x08], 4),
        (vec![0x66, 0x48, 0x0f, 0x7e, 0x08], 8),
        (vec![0x66, 0x0f, 0xd6, 0x08], 8),
    ] {
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.data[..width].copy_from_slice(&low[..width]);
        assert_eq!(observed, want, "{code:02x?}");
    }
    // The store forms accept any addressing form and any (unaligned) address: no alignment rule.
    for mode in MODES {
        let (code, setup) = mem_code(&[0x66, 0x48], 0x7e, 1, mode, None);
        // mem_code places REX.W inside the prefix slice: [66 48] 0F 7E.
        let (observed, _, _) = in_set_with(&code, &case, 1, setup);
        let mut want = expected(&case, &code);
        want.data[..8].copy_from_slice(&low);
        let mut seen = observed.clone();
        seen.gpr = want.gpr;
        assert_eq!(seen, want, "{mode:?}");
    }
    let (observed, _, _) = in_set_with(&[0x66, 0x48, 0x0f, 0x7e, 0x08], &case, 1, |state| {
        state.gpr[0] = DATA + 0x21
    });
    let mut want = expected(&case, &[0x66, 0x48, 0x0f, 0x7e, 0x08]);
    want.data[0x21..0x29].copy_from_slice(&low);
    want.gpr[0] = DATA + 0x21;
    assert_eq!(observed, want);
}

#[test]
fn movd_and_movq_from_a_general_register_or_memory_into_an_xmm_register() {
    let case = Case::new(false);
    let rcx = case.rcx.to_le_bytes();
    // MOVD xmm0, ecx: the low dword, the rest of the register cleared.
    let code = [0x66, 0x0f, 0x6e, 0xc1];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[0] = join([rcx[0], rcx[1], rcx[2], rcx[3], 0, 0, 0, 0], [0; 8]);
    assert_eq!(observed, want);
    // MOVQ xmm0, rcx, MOVQ xmm9, rcx and MOVQ xmm0, r9 (REX.W + R/B).
    for (code, register, source) in [
        (vec![0x66, 0x48, 0x0f, 0x6e, 0xc1], 0usize, 1usize),
        (vec![0x66, 0x4c, 0x0f, 0x6e, 0xc9], 9, 1),
        (vec![0x66, 0x49, 0x0f, 0x6e, 0xc1], 0, 9),
    ] {
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[register] = join(initial_state(&case).gpr[source].to_le_bytes(), [0; 8]);
        assert_eq!(observed, want, "{code:02x?}");
    }
    // Memory sources: MOVD reads 4 bytes, MOVQ 8, with no alignment rule.
    for (code, width) in [
        (vec![0x66, 0x0f, 0x6e, 0x08], 4),
        (vec![0x66, 0x48, 0x0f, 0x6e, 0x08], 8),
        (vec![0xf3, 0x0f, 0x7e, 0x08], 8),
    ] {
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        let mut value = [0u8; 16];
        value[..width].copy_from_slice(&case.memory[..width]);
        want.xmm[1] = value;
        assert_eq!(observed, want, "{code:02x?}");
        let (observed, _, _) = in_set_with(&code, &case, 1, |state| state.gpr[0] = DATA + 3);
        let mut want = expected(&case, &code);
        let mut value = [0u8; 16];
        value[..width].copy_from_slice(
            &[
                case.memory[3],
                case.memory[4],
                case.memory[5],
                case.memory[6],
                case.memory[7],
                case.memory[8],
                case.memory[9],
                case.memory[10],
            ][..width],
        );
        want.xmm[1] = value;
        want.gpr[0] = DATA + 3;
        assert_eq!(observed, want, "{code:02x?} unaligned");
    }
}

#[test]
fn movq_between_xmm_registers_clears_the_high_quadword() {
    let case = Case::new(false);
    // F3 0F 7E: MOVQ xmm1, xmm2 (xmm2 is ModRM.rm); 66 0F D6: MOVQ xmm1, xmm2 with xmm1 in ModRM.rm.
    for (code, destination, source) in [
        (vec![0xf3, 0x0f, 0x7e, 0xca], 1usize, 2usize),
        (vec![0xf3, 0x45, 0x0f, 0x7e, 0xca], 9, 10),
        (vec![0x66, 0x0f, 0xd6, 0xd1], 1, 2),
        (vec![0x66, 0x41, 0x0f, 0xd6, 0xd1], 9, 2),
        (vec![0x66, 0x0f, 0xd6, 0xca], 2, 1),
    ] {
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[destination] = join(lo(case.xmm[source]), [0; 8]);
        assert_eq!(observed, want, "{code:02x?}");
    }
}

#[test]
fn movlps_movlpd_movhps_movhpd_load_and_store_one_quadword() {
    let case = Case::new(false);
    let memory = lo(case.memory);
    for prefix in [&[][..], &[0x66][..]] {
        let with = |opcode: u8, tail: &[u8]| {
            let mut code = prefix.to_vec();
            code.extend_from_slice(&[0x0f, opcode]);
            code.extend_from_slice(tail);
            code
        };
        // Loads: the other quadword of the destination is kept.
        let code = with(0x12, &[0x08]);
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[1] = join(memory, hi(case.xmm[1]));
        assert_eq!(observed, want, "MOVLP{{S,D}} load {prefix:02x?}");
        let code = with(0x16, &[0x08]);
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[1] = join(lo(case.xmm[1]), memory);
        assert_eq!(observed, want, "MOVHP{{S,D}} load {prefix:02x?}");
        // Stores: one quadword of the source, the rest of the memory untouched.
        let code = with(0x13, &[0x08]);
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.data[..8].copy_from_slice(&lo(case.xmm[1]));
        assert_eq!(observed, want, "MOVLP{{S,D}} store {prefix:02x?}");
        let code = with(0x17, &[0x08]);
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.data[..8].copy_from_slice(&hi(case.xmm[1]));
        assert_eq!(observed, want, "MOVHP{{S,D}} store {prefix:02x?}");
        // REX.R register, unaligned address, RIP-relative operand.
        let mut with_rex = prefix.to_vec();
        with_rex.extend_from_slice(&[0x44, 0x0f, 0x16, 0x08]);
        let (observed, _, _) = in_set_with(&with_rex, &case, 1, |state| state.gpr[0] = DATA + 5);
        let mut want = expected(&case, &with_rex);
        let unaligned: [u8; 8] = case.memory[5..13].try_into().unwrap();
        want.xmm[9] = join(lo(case.xmm[9]), unaligned);
        want.gpr[0] = DATA + 5;
        assert_eq!(observed, want, "MOVHP{{S,D}} xmm9 unaligned {prefix:02x?}");
        let (rip_code, setup) = mem_code(prefix, 0x13, 1, Mode::RipRelative, None);
        let (observed, _, _) = in_set_with(&rip_code, &case, 1, setup);
        let mut want = expected(&case, &rip_code);
        want.data[..8].copy_from_slice(&lo(case.xmm[1]));
        let mut seen = observed.clone();
        seen.gpr = want.gpr;
        assert_eq!(seen, want, "MOVLP{{S,D}} RIP-relative {prefix:02x?}");
    }
}

#[test]
fn movhlps_and_movlhps_copy_one_quadword_between_registers() {
    let case = Case::new(false);
    // MOVHLPS xmm1, xmm2: low quadword of xmm1 := high quadword of xmm2, high quadword kept.
    let code = [0x0f, 0x12, 0xca];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[1] = join(hi(case.xmm[2]), hi(case.xmm[1]));
    assert_eq!(observed, want);
    // MOVLHPS xmm1, xmm2: high quadword of xmm1 := low quadword of xmm2, low quadword kept.
    let code = [0x0f, 0x16, 0xca];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[1] = join(lo(case.xmm[1]), lo(case.xmm[2]));
    assert_eq!(observed, want);
    // REX.R and REX.B registers; the same register for both operands.
    let code = [0x45, 0x0f, 0x12, 0xca];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[9] = join(hi(case.xmm[10]), hi(case.xmm[9]));
    assert_eq!(observed, want);
    let code = [0x0f, 0x16, 0xc9];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[1] = join(lo(case.xmm[1]), lo(case.xmm[1]));
    assert_eq!(observed, want);
}

#[test]
fn pextrw_selects_the_word_by_the_low_three_bits_of_imm8_and_zero_extends() {
    let case = Case::new(false);
    for imm in 0..=255u8 {
        let lane = usize::from(imm & 7);
        let word = u16::from_le_bytes([case.xmm[2][2 * lane], case.xmm[2][2 * lane + 1]]);
        // PEXTRW ecx, xmm2, imm8: the whole 64-bit register takes the zero-extended word.
        let code = [0x66, 0x0f, 0xc5, 0xca, imm];
        let (observed, before, after) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.gpr[1] = u64::from(word);
        assert_eq!(observed, want, "imm {imm:#x}");
        assert_eq!(after.x87, before.x87);
    }
    // REX.R destination (r8) and REX.B source (xmm10); REX.W changes nothing.
    for (code, destination, source) in [
        (vec![0x66, 0x44, 0x0f, 0xc5, 0xc2, 0x07], 8usize, 2usize),
        (vec![0x66, 0x41, 0x0f, 0xc5, 0xca, 0x00], 1, 10),
        (vec![0x66, 0x4d, 0x0f, 0xc5, 0xc2, 0x0b], 8, 10),
    ] {
        let imm = *code.last().unwrap();
        let lane = usize::from(imm & 7);
        let word = u16::from_le_bytes([case.xmm[source][2 * lane], case.xmm[source][2 * lane + 1]]);
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.gpr[destination] = u64::from(word);
        assert_eq!(observed, want, "{code:02x?}");
    }
}

#[test]
fn pinsrw_replaces_one_word_from_a_general_register_or_memory() {
    let case = Case::new(false);
    for imm in 0..=255u8 {
        let lane = usize::from(imm & 7);
        // PINSRW xmm2, ecx, imm8: the low 16 bits of ecx.
        let code = [0x66, 0x0f, 0xc4, 0xd1, imm];
        let (observed, _, _) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[2][2 * lane..2 * lane + 2].copy_from_slice(&(case.rcx as u16).to_le_bytes());
        assert_eq!(observed, want, "imm {imm:#x} register form");
        // PINSRW xmm2, [rax], imm8: two bytes of memory, any alignment.
        let code = [0x66, 0x0f, 0xc4, 0x10, imm];
        let (observed, _, _) = in_set_with(&code, &case, 1, |state| state.gpr[0] = DATA + 1);
        let mut want = expected(&case, &code);
        want.xmm[2][2 * lane..2 * lane + 2].copy_from_slice(&case.memory[1..3]);
        want.gpr[0] = DATA + 1;
        assert_eq!(observed, want, "imm {imm:#x} memory form");
    }
    // REX.R destination (xmm9) and REX.B source (r9).
    let code = [0x66, 0x45, 0x0f, 0xc4, 0xc9, 0x06];
    let (observed, _, _) = in_set(&code, &case);
    let mut want = expected(&case, &code);
    want.xmm[9][12..14].copy_from_slice(&(initial_state(&case).gpr[9] as u16).to_le_bytes());
    assert_eq!(observed, want);
}

#[test]
fn movdqu_and_movdqa_register_forms_copy_the_whole_register() {
    let case = Case::new(false);
    for (code, destination, source) in [
        (vec![0xf3, 0x0f, 0x6f, 0xca], 1usize, 2usize),
        (vec![0xf3, 0x45, 0x0f, 0x6f, 0xca], 9, 10),
        (vec![0xf3, 0x0f, 0x7f, 0xca], 2, 1),
        (vec![0x66, 0x0f, 0x7f, 0xca], 2, 1),
        (vec![0x66, 0x41, 0x0f, 0x7f, 0xca], 10, 1),
        (vec![0x66, 0x44, 0x0f, 0x7f, 0xc2], 2, 8),
    ] {
        let (observed, before, after) = in_set(&code, &case);
        let mut want = expected(&case, &code);
        want.xmm[destination] = case.xmm[source];
        assert_eq!(observed, want, "{code:02x?}");
        assert_eq!(after.x87, before.x87);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Refusals, #UD, the member, the plain step and a program.
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn encodings_the_sdm_leaves_undefined_are_invalid_opcode_faults_before_any_gate_or_access() {
    let forms: &[(&str, &[u8])] = &[
        ("LOCK PADDB xmm1,xmm2", &[0xf0, 0x66, 0x0f, 0xfc, 0xca]),
        ("LOCK PADDB xmm1,[rax]", &[0xf0, 0x66, 0x0f, 0xfc, 0x08]),
        ("66 LOCK PSLLW xmm1,[rax]", &[0x66, 0xf0, 0x0f, 0xf1, 0x08]),
        (
            "LOCK PSHUFD xmm1,[rax],0",
            &[0xf0, 0x66, 0x0f, 0x70, 0x08, 0x00],
        ),
        ("LOCK MOVD ecx,xmm1", &[0xf0, 0x66, 0x0f, 0x7e, 0xc9]),
        ("LOCK MOVQ xmm1,xmm2", &[0xf0, 0xf3, 0x0f, 0x7e, 0xca]),
        ("LOCK MOVLPS xmm1,[rax]", &[0xf0, 0x0f, 0x12, 0x08]),
        (
            "LOCK PEXTRW ecx,xmm2,3",
            &[0xf0, 0x66, 0x0f, 0xc5, 0xca, 0x03],
        ),
        (
            "PEXTRW with a memory operand",
            &[0x66, 0x0f, 0xc5, 0x08, 0x00],
        ),
        (
            "MOVLPS m64,xmm with a register operand",
            &[0x0f, 0x13, 0xca],
        ),
        (
            "MOVHPS m64,xmm with a register operand",
            &[0x0f, 0x17, 0xca],
        ),
        (
            "MOVLPD m64,xmm with a register operand",
            &[0x66, 0x0f, 0x13, 0xca],
        ),
        (
            "MOVHPD m64,xmm with a register operand",
            &[0x66, 0x0f, 0x17, 0xca],
        ),
        (
            "MOVLPD xmm,m64 with a register operand",
            &[0x66, 0x0f, 0x12, 0xca],
        ),
        (
            "MOVHPD xmm,m64 with a register operand",
            &[0x66, 0x0f, 0x16, 0xca],
        ),
    ];
    for (name, code) in forms {
        // Gates and a misaligned non-canonical address would otherwise fire: #UD of the encoding comes first.
        let mut case = Case::new(false);
        case.cr0 = CR0_BASE | CR0_TS;
        let (observed, before, after) =
            in_set_with(code, &case, 1, |state| state.gpr[0] = (1 << 47) + 1);
        let mut want = unchanged(&case, code, fault(6));
        want.gpr[0] = (1 << 47) + 1;
        assert_eq!(observed, want, "{name}");
        assert_eq!(after, before, "{name}");
    }
}

#[test]
fn encodings_outside_the_slice_stay_refused_without_a_change() {
    let forms: &[(&str, &[u8], u8)] = &[
        ("FS override", &[0x64, 0x66, 0x0f, 0xfc, 0x08], 0xfc),
        ("GS override", &[0x66, 0x65, 0x0f, 0xfc, 0x08], 0xfc),
        (
            "FS override PSHUFD",
            &[0x64, 0x66, 0x0f, 0x70, 0x08, 0x00],
            0x70,
        ),
        (
            "FS override MOVQ store",
            &[0x64, 0x66, 0x0f, 0xd6, 0x08],
            0xd6,
        ),
        ("GS override MOVHPS store", &[0x65, 0x0f, 0x17, 0x08], 0x17),
        ("repeated 66", &[0x66, 0x66, 0x0f, 0xfc, 0x08], 0xfc),
        ("repeated 67", &[0x67, 0x67, 0x66, 0x0f, 0xfc, 0x08], 0xfc),
        (
            "two segment overrides",
            &[0x2e, 0x3e, 0x66, 0x0f, 0xfc, 0x08],
            0xfc,
        ),
        ("repeated REX", &[0x66, 0x48, 0x48, 0x0f, 0xfc, 0x08], 0xfc),
        ("66 and F2 together", &[0x66, 0xf2, 0x0f, 0xfc, 0x08], 0xfc),
        ("66 and F3 together", &[0x66, 0xf3, 0x0f, 0xfc, 0xca], 0xfc),
        (
            "F3 PADDB (not an instruction)",
            &[0xf3, 0x0f, 0xfc, 0x08],
            0xfc,
        ),
        (
            "F2 MOVQ load (not an instruction)",
            &[0xf2, 0x0f, 0x7e, 0xca],
            0x7e,
        ),
        (
            "F3 MOVQ store (not an instruction)",
            &[0xf3, 0x0f, 0xd6, 0xca],
            0xd6,
        ),
        (
            "MOVDQU memory form of the F3 6F pair needs the S2-34 prefix",
            &[0xf2, 0x0f, 0x6f, 0xca],
            0x6f,
        ),
    ];
    for (name, code, opcode) in forms {
        let case = Case::new(false);
        let (observed, before, after) = in_set(code, &case);
        assert_eq!(
            observed,
            unchanged(&case, code, Result_::Unsupported(*opcode)),
            "{name}"
        );
        assert_eq!(after, before, "{name}");
    }
}

#[test]
fn without_the_fp_member_the_set_keeps_refusing_the_forms_of_the_slice() {
    let forms: &[(&str, &[u8])] = &[
        ("PADDB xmm1,[rax]", &[0x66, 0x0f, 0xfc, 0x08]),
        ("PSLLW xmm1,xmm2", &[0x66, 0x0f, 0xf1, 0xca]),
        ("PSHUFHW xmm1,xmm2,1", &[0xf3, 0x0f, 0x70, 0xca, 0x01]),
        ("MOVD ecx,xmm1", &[0x66, 0x0f, 0x7e, 0xc9]),
        ("MOVLPS [rax],xmm1", &[0x0f, 0x13, 0x08]),
        ("MOVHLPS xmm1,xmm2", &[0x0f, 0x12, 0xca]),
        ("PEXTRW ecx,xmm2,3", &[0x66, 0x0f, 0xc5, 0xca, 0x03]),
        ("MOVDQU xmm1,xmm2", &[0xf3, 0x0f, 0x6f, 0xca]),
    ];
    for (name, code) in forms {
        let case = Case::new(false);
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

/// The plain step runs the same forms on a state that carries `sse` and no control registers (no gates); the set
/// result is the plain-step result for the memory and the new register forms alike (PSHUFLW with a register source is
/// a form of the CPL3 handler only: the plain step refuses it, as before).
#[test]
fn the_plain_step_with_sse_state_and_no_control_registers_gives_the_same_result_as_the_set() {
    let forms: &[&[u8]] = &[
        &[0x66, 0x0f, 0xfc, 0x08],
        &[0x66, 0x0f, 0xe5, 0xca],
        &[0x66, 0x0f, 0xf1, 0xca],
        &[0x66, 0x0f, 0xd3, 0x08],
        &[0xf3, 0x0f, 0x70, 0x08, 0x1b],
        &[0x66, 0x0f, 0x7e, 0xc9],
        &[0x66, 0x48, 0x0f, 0x7e, 0x08],
        &[0xf3, 0x0f, 0x7e, 0xca],
        &[0x0f, 0x13, 0x08],
        &[0x0f, 0x12, 0xca],
        &[0x66, 0x0f, 0xc5, 0xca, 0x05],
        &[0x66, 0x0f, 0xc4, 0xd1, 0x05],
        &[0xf3, 0x0f, 0x6f, 0xca],
    ];
    for code in forms {
        let case = Case::new(false);
        let reference = bare(code, &case, false);
        assert_eq!(reference.result, Result_::Retired, "{code:02x?}");
        let (observed, _, _) = in_set(code, &case);
        assert_eq!(observed, reference, "{code:02x?}");
    }
}

#[test]
fn a_program_of_the_forms_equals_the_reference_composition() {
    let case = Case::new(false);
    let program: Vec<u8> = [
        &[0xf3, 0x0f, 0x7e, 0x08][..],       // movq xmm1,[rax]
        &[0x66, 0x0f, 0xfc, 0x08][..],       // paddb xmm1,[rax]
        &[0x66, 0x0f, 0x70, 0xd1, 0x1b][..], // pshufd xmm2,xmm1,0x1b
        &[0x66, 0x0f, 0xd3, 0xd1][..],       // psrlq xmm2,xmm1
        &[0x66, 0x0f, 0xc5, 0xca, 0x03][..], // pextrw ecx,xmm2,3
        &[0x66, 0x0f, 0xc4, 0xd1, 0x02][..], // pinsrw xmm2,ecx,2
        &[0x66, 0x0f, 0x7e, 0xd3][..],       // movd ebx,xmm2
        &[0x66, 0x0f, 0xd6, 0x50, 0x10][..], // movq [rax+0x10],xmm2
    ]
    .concat();
    let (observed, before, after) = in_set_program(&program, &case, 8);
    // The same composition on the reference model.
    let memory = case.memory;
    let mut xmm1 = join(lo(memory), [0; 8]);
    xmm1 = reference(0xfc, &xmm1, &memory);
    let mut xmm2 = reference_shuffle(0x66, &xmm1, 0x1b);
    xmm2 = reference(0xd3, &xmm2, &xmm1);
    let word3 = u16::from_le_bytes([xmm2[6], xmm2[7]]);
    xmm2[4..6].copy_from_slice(&word3.to_le_bytes());
    let mut want = expected(&case, &program);
    want.xmm[1] = xmm1;
    want.xmm[2] = xmm2;
    want.gpr[1] = u64::from(word3);
    want.gpr[3] = u64::from(u32::from_le_bytes(xmm2[..4].try_into().unwrap()));
    want.data[0x10..0x18].copy_from_slice(&lo(xmm2));
    assert_eq!(observed, want);
    assert_eq!(after.x87, before.x87);
}
