//! S7-32 slice S (ISSUE-712): `REPNE SCAS` (`F2 AE`; `F2 AF` with `66`, none or `REX.W`; with and
//! without `67`) through the CPL0 profile set at CPL0, checked against an independent model written
//! from the Intel SDM (325462-093US), Vol. 2B:
//!
//! - "REP/REPE/REPZ/REPNE/REPNZ—Repeat String Operation Prefix": REPNE repeats while the count
//!   register (RCX, or ECX with `67`) is not 0 and ZF = 0; the count is tested before each
//!   iteration (a zero count runs nothing and changes nothing); SCAS sets ZF by its comparison.
//! - "SCAS/SCASB/SCASW/SCASD/SCASQ—Scan String": `temp := AL/AX/EAX/RAX − SRC;
//!   SetStatusFlags(temp)` with SRC at ES:(E/R)DI, then (E/R)DI ± operand size by DF.
//! - The status flags of the subtraction as `CMP` sets them.
//!
//! With `67` the model writes back zero-extended EDI and ECX (`CPU_CONTRACT.md`, S2-19m/S2-19o, as
//! REPE SCAS of slice P). Recorded deviations the tests pin rather than the SDM: the partial exit
//! every `REP_ITERATION_WORK_LIMIT` iterations (RIP on the instruction, progress kept), and the flags
//! of the last comparison after a fault on element k ≥ 2 (ISSUE-708; the SDM restores the entry
//! EFLAGS, which `run_with_rep_continuation` does for the `67` forms — the differential test).

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RepContinuation, RunConfig, RunExit,
    run_with_cpl0_profile_set, run_with_rep_continuation,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const ARITHMETIC: u64 = CF | PF | AF | ZF | SF | OF;
const WORK_LIMIT: u64 = od_isa::execute::REP_ITERATION_WORK_LIMIT;
const BUFFER: u64 = 0x10_0000;
const BUFFER_BYTES: usize = 0x6000;

/// The eight forms: (width in bytes, address size 32, code).
const FORMS: [(u64, bool, &[u8]); 8] = [
    (1, false, &[0xf2, 0xae]),
    (2, false, &[0x66, 0xf2, 0xaf]),
    (4, false, &[0xf2, 0xaf]),
    (8, false, &[0xf2, 0x48, 0xaf]),
    (1, true, &[0xf2, 0x67, 0xae]),
    (2, true, &[0xf2, 0x67, 0x66, 0xaf]),
    (4, true, &[0xf2, 0x67, 0xaf]),
    (8, true, &[0xf2, 0x67, 0x48, 0xaf]),
];

/// Code at `CODE`; data in regions; every other address is unmapped. Counts reads.
#[derive(Clone)]
struct Memory {
    code: Vec<u8>,
    regions: Vec<(u64, Vec<u8>)>,
    /// Addresses `[start, end)` whose reads raise a not-present read `#PF`.
    faulting: Option<(u64, u64)>,
    reads: usize,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            regions: Vec::new(),
            faulting: None,
            reads: 0,
        }
    }

    fn region(mut self, base: u64, bytes: Vec<u8>) -> Self {
        self.regions.push((base, bytes));
        self
    }

    fn byte(&self, address: u64) -> Option<u8> {
        self.regions.iter().find_map(|(base, bytes)| {
            let offset = address.wrapping_sub(*base);
            (offset < bytes.len() as u64).then(|| bytes[offset as usize])
        })
    }

    fn element(&self, address: u64, width: u64) -> u64 {
        (0..width).fold(0, |value, index| {
            value | u64::from(self.byte(address + index).expect("mapped")) << (8 * index)
        })
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.wrapping_sub(CODE) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        if let Some((start, end)) = self.faulting
            && address < end
            && address.wrapping_add(out.len() as u64) > start
        {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(address.max(start)),
            }));
        }
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = self
                .byte(address.wrapping_add(index as u64))
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
        }
        self.reads += 1;
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn state(rflags: u64) -> LongModeState {
    LongModeState {
        rip: CODE,
        rflags,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

/// Runs the set until it leaves `RepIterationBudgetExhausted` (the partial REP exit a board resumes)
/// or reaches `stop_rip`. Returns the exit, the defined-flags mask, the retired instructions and the
/// number of partial exits.
fn run(
    state: &mut LongModeState,
    memory: &mut Memory,
    budget: u64,
    stop_rip: Option<u64>,
    defined: u64,
) -> (RunExit, u64, u64, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let (mut defined, mut retired, mut partial) = (defined, 0, 0);
    loop {
        let result = run_with_cpl0_profile_set(
            state,
            memory,
            RunConfig {
                instruction_budget: budget - retired,
                stop_rip,
                context: context(),
            },
            defined,
            &set,
            None,
            || false,
        )
        .expect("the state is inside the set");
        defined = result.rflags_defined;
        retired += result.instructions_executed;
        let Cpl0ProfileSetExit::Run(exit) = result.exit else {
            panic!("expected a Run exit");
        };
        if exit != RunExit::RepIterationBudgetExhausted {
            return (exit, defined, retired, partial);
        }
        assert_eq!(defined & ARITHMETIC, ARITHMETIC, "partial exit {partial}");
        partial += 1;
    }
}

/// The six status flags of `a - b` at `width` bytes, as `CMP` sets them.
fn cmp_flags(width: u64, a: u64, b: u64) -> u64 {
    let bits = 8 * width;
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    };
    let (a, b) = (a & mask, b & mask);
    let r = a.wrapping_sub(b) & mask;
    let sign = 1u64 << (bits - 1);
    let mut flags = 0;
    if a < b {
        flags |= CF;
    }
    if (r as u8).count_ones() % 2 == 0 {
        flags |= PF;
    }
    if (a ^ b ^ r) & 0x10 != 0 {
        flags |= AF;
    }
    if r == 0 {
        flags |= ZF;
    }
    if r & sign != 0 {
        flags |= SF;
    }
    if (a ^ b) & (a ^ r) & sign != 0 {
        flags |= OF;
    }
    flags
}

/// `REPNE SCAS` by the SDM pseudocode: RCX, RDI, RFLAGS and the number of iterations.
fn model(
    memory: &Memory,
    width: u64,
    address32: bool,
    rax: u64,
    rcx: u64,
    rdi: u64,
    rflags: u64,
) -> (u64, u64, u64, u64) {
    let mut count = if address32 { rcx & 0xffff_ffff } else { rcx };
    let mut address = if address32 { rdi & 0xffff_ffff } else { rdi };
    let mut flags = rflags;
    let mut iterations = 0;
    while count != 0 {
        let element = memory.element(address, width);
        flags = (flags & !ARITHMETIC) | cmp_flags(width, rax, element);
        address = if rflags & DF == 0 {
            address.wrapping_add(width)
        } else {
            address.wrapping_sub(width)
        };
        if address32 {
            address &= 0xffff_ffff;
        }
        count -= 1;
        iterations += 1;
        if flags & ZF != 0 {
            break;
        }
    }
    if iterations == 0 {
        return (rcx, rdi, rflags, 0);
    }
    (count, address, flags, iterations)
}

/// A buffer of `elements` elements of `width` bytes from `BUFFER`, none equal to the scanned value
/// `0x5a…`, except the element `hit` (if any), which equals it.
fn buffer(width: u64, elements: u64, hit: Option<u64>) -> Vec<u8> {
    scan_buffer(width, elements, hit, false, BUFFER)
}

/// As [`buffer`], with element i of the scan at `start + i·width` (DF = 0) or at `start − i·width`
/// (DF = 1: the scan reads `[RDI]`, then RDI decreases).
fn scan_buffer(width: u64, elements: u64, hit: Option<u64>, df: bool, start: u64) -> Vec<u8> {
    let mut bytes = vec![0u8; BUFFER_BYTES];
    for index in 0..elements {
        let address = if df {
            start - index * width
        } else {
            start + index * width
        };
        let value = if Some(index) == hit {
            0x5a
        } else {
            0x11 + (index as u8 & 0x3f)
        };
        for byte in 0..width {
            bytes[(address - BUFFER + byte) as usize] = value;
        }
    }
    bytes
}

#[test]
fn repne_scas_matches_the_sdm_model_for_every_form() {
    let mut cases = 0;
    for (width, address32, code) in FORMS {
        for count in [0, 1, 3, WORK_LIMIT, WORK_LIMIT + 5] {
            let hits = [None, Some(0), Some(1), count.checked_sub(1)];
            for hit in hits {
                for df in [false, true] {
                    for (entry_flags, entry_defined) in [
                        (2 | ARITHMETIC, od_isa::INTEGER_FLAGS),
                        (2, od_isa::INTEGER_FLAGS & !ARITHMETIC),
                    ] {
                        let start = BUFFER + 0x3000;
                        let bytes = scan_buffer(width, count + 2, hit, df, start);
                        let mut memory = Memory::new(code).region(BUFFER, bytes);
                        let rflags = entry_flags | if df { DF } else { 0 };
                        let mut state = state(rflags);
                        // Upper halves set: with `67` they are ignored and the written registers
                        // are zero-extended.
                        let high = if address32 { 0xdead_beef_0000_0000 } else { 0 };
                        state.gpr[Register::Rax as usize] = 0x5a5a_5a5a_5a5a_5a5a;
                        state.gpr[Register::Rcx as usize] = high | count;
                        state.gpr[Register::Rdi as usize] = high | start;
                        let expected = model(
                            &memory,
                            width,
                            address32,
                            state.gpr[Register::Rax as usize],
                            state.gpr[Register::Rcx as usize],
                            state.gpr[Register::Rdi as usize],
                            rflags,
                        );
                        let next = CODE + code.len() as u64;
                        let (exit, defined, retired, partial) =
                            run(&mut state, &mut memory, 1, None, entry_defined);
                        let what = format!(
                            "{code:02x?} count {count} hit {hit:?} df {df} flags {entry_flags:#x}"
                        );
                        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{what}");
                        assert_eq!(retired, 1, "{what}");
                        assert_eq!(state.rip, next, "{what}");
                        assert_eq!(
                            (
                                state.gpr[Register::Rcx as usize],
                                state.gpr[Register::Rdi as usize],
                                state.rflags
                            ),
                            (expected.0, expected.1, expected.2),
                            "{what}"
                        );
                        assert_eq!(memory.reads as u64, expected.3, "{what}");
                        assert_eq!(
                            partial,
                            expected.3.saturating_sub(1) / WORK_LIMIT,
                            "{what}: partial exits"
                        );
                        // Six flags defined after the instruction, also for a zero count
                        // (ISSUE-707, as REPE SCAS).
                        assert_eq!(defined & ARITHMETIC, ARITHMETIC, "{what}");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 8 * 5 * 4 * 2 * 2);
}

/// A `#PF` on element k: RCX/RDI keep the progress of the k − 1 finished elements and RIP stays on
/// the instruction (SDM); the flags are the entry flags for k = 1 and those of the last finished
/// comparison for k ≥ 2 (ISSUE-708, recorded deviation).
#[test]
fn a_fault_inside_repne_scas_keeps_the_progress() {
    for (width, address32, code) in FORMS {
        for k in [1u64, 2, 3] {
            let bytes = buffer(width, 8, None);
            let mut memory = Memory::new(code).region(BUFFER, bytes);
            let fault_at = BUFFER + (k - 1) * width;
            memory.faulting = Some((fault_at, fault_at + width));
            let entry = 2 | ZF | CF;
            let mut state = state(entry);
            state.gpr[Register::Rax as usize] = 0x5a5a_5a5a_5a5a_5a5a;
            state.gpr[Register::Rcx as usize] = 6;
            state.gpr[Register::Rdi as usize] = BUFFER;
            let (exit, _, retired, _) =
                run(&mut state, &mut memory, 1, None, od_isa::INTEGER_FLAGS);
            let RunExit::Fault(fault) = exit else {
                panic!("{code:02x?} k {k}: {exit:?}");
            };
            assert_eq!(fault.vector, 14);
            assert_eq!(retired, 0);
            assert_eq!(state.rip, CODE);
            assert_eq!(state.gpr[Register::Rcx as usize], 6 - (k - 1));
            assert_eq!(state.gpr[Register::Rdi as usize], BUFFER + (k - 1) * width);
            let expected_flags = if k == 1 {
                entry
            } else {
                let element = Memory::new(code)
                    .region(BUFFER, buffer(width, 8, None))
                    .element(BUFFER + (k - 2) * width, width);
                (entry & !ARITHMETIC) | cmp_flags(width, 0x5a5a_5a5a_5a5a_5a5a, element)
            };
            assert_eq!(state.rflags, expected_flags, "{code:02x?} k {k}");
            let _ = address32;
        }
    }
}

/// A stop request between two iterations ends the step with RIP on the instruction and the
/// progress of the finished iterations (the board resumes it).
#[test]
fn a_stop_between_iterations_keeps_the_progress() {
    let code: &[u8] = &[0xf2, 0xae];
    for stop_after in 0..5u32 {
        let bytes = buffer(1, 16, None);
        let mut memory = Memory::new(code).region(BUFFER, bytes);
        let mut state = state(2);
        state.gpr[Register::Rax as usize] = 0x5a;
        state.gpr[Register::Rcx as usize] = 10;
        state.gpr[Register::Rdi as usize] = BUFFER;
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
        let mut calls = 0;
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(),
            },
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || {
                calls += 1;
                calls > stop_after
            },
        )
        .unwrap();
        let rcx = state.gpr[Register::Rcx as usize];
        let done = 10 - rcx;
        assert_eq!(state.gpr[Register::Rdi as usize], BUFFER + done);
        assert_eq!(memory.reads as u64, done);
        if result.exit == Cpl0ProfileSetExit::Run(RunExit::StopRequested) {
            assert_eq!(state.rip, CODE, "stop after {stop_after}");
            assert_eq!(result.instructions_executed, 0);
        } else {
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
            );
            assert_eq!(rcx, 0);
        }
    }
}

/// ISSUE-712: the inline `wcslen` of `iaStorV.sys` + `0x19d0` byte for byte (`or rcx, -1;
/// xor eax, eax; mov rdi, rbp; repne scasw; not rcx; sub rcx, r13`) gives the length of the string.
#[test]
fn the_iastorv_wcslen_gives_the_string_length() {
    let code: &[u8] = &[
        0x48, 0x83, 0xc9, 0xff, // or rcx, -1
        0x33, 0xc0, // xor eax, eax
        0x48, 0x8b, 0xfd, // mov rdi, rbp
        0x66, 0xf2, 0xaf, // repne scasw
        0x48, 0xf7, 0xd1, // not rcx
        0x49, 0x2b, 0xcd, // sub rcx, r13
    ];
    let text: Vec<u8> = "iaStorV\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut memory = Memory::new(code).region(BUFFER, text);
    let mut state = state(2);
    state.gpr[Register::Rbp as usize] = BUFFER;
    state.gpr[Register::R13 as usize] = 1;
    let end = CODE + code.len() as u64;
    let (exit, _, retired, _) = run(
        &mut state,
        &mut memory,
        100,
        Some(end),
        od_isa::INTEGER_FLAGS,
    );
    assert_eq!(exit, RunExit::StopAddress);
    assert_eq!(retired, 6);
    assert_eq!(state.gpr[Register::Rcx as usize], 7, "wcslen(\"iaStorV\")");
    assert_eq!(state.gpr[Register::Rdi as usize], BUFFER + 16);
}

/// Outside the set REPNE SCAS without `67` stays `Unsupported`: the bounded continuation runner,
/// which takes only the `67` spellings, refuses it before any access.
#[test]
fn repne_scas_without_67_stays_refused_outside_the_set() {
    for code in [
        &[0xf2, 0xae][..],
        &[0x66, 0xf2, 0xaf],
        &[0xf2, 0xaf],
        &[0xf2, 0x48, 0xaf],
    ] {
        let mut memory = Memory::new(code).region(BUFFER, buffer(8, 8, None));
        let mut state = LongModeState {
            rip: CODE,
            rflags: 2,
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        };
        state.gpr[Register::Rcx as usize] = 2;
        state.gpr[Register::Rdi as usize] = BUFFER;
        let before = state.clone();
        let mut continuation = None;
        let result = run_with_rep_continuation(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(),
            },
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            RunExit::Unsupported {
                opcode: code[code.len() - 1]
            },
            "{code:02x?}"
        );
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
    }
}

/// The `67` forms through the set and through `run_with_rep_continuation` (the bounded runner with
/// a continuation token) on the same inputs, EDI wrapping at 4 GiB among them: the same RCX, RDI,
/// RIP, flags and reads; after a `#PF` on element k ≥ 2 only the flags differ (the set keeps the
/// last comparison, ISSUE-708; the runner restores the entry flags, as the SDM).
#[test]
fn the_set_and_the_continuation_runner_agree_on_the_67_forms() {
    const WRAP: u64 = 0xffff_fff8;
    for (width, address32, code) in FORMS {
        if !address32 {
            continue;
        }
        for (count, hit, fault_k) in [
            (1u64, None, None),
            (3, Some(2u64), None),
            (5, None, None),
            (1500, None, None),
            (1500, Some(1200), None),
            (6, None, Some(3u64)),
        ] {
            // Elements from EDI = 0xffff_fff8 upwards wrap to 0.
            let elements = count + 2;
            let mut low = vec![0u8; (elements * width) as usize + 16];
            let mut top = vec![0u8; 8];
            for index in 0..elements {
                let address = (WRAP + index * width) & 0xffff_ffff;
                let value = if Some(index) == hit {
                    0x5a
                } else {
                    0x11 + (index as u8 & 0x3f)
                };
                for byte in 0..width {
                    let at = (address + byte) & 0xffff_ffff;
                    if at >= WRAP {
                        top[(at - WRAP) as usize] = value;
                    } else {
                        low[at as usize] = value;
                    }
                }
            }
            let memory = Memory::new(code).region(WRAP, top).region(0, low);
            let mut memory = memory;
            if let Some(k) = fault_k {
                let at = (WRAP + (k - 1) * width) & 0xffff_ffff;
                memory.faulting = Some((at, at + width));
            }
            let entry = 2 | ZF | PF;
            let setup = |state: &mut LongModeState| {
                state.gpr[Register::Rax as usize] = 0x5a5a_5a5a_5a5a_5a5a;
                state.gpr[Register::Rcx as usize] = 0xabcd_0000_0000_0000 | count;
                state.gpr[Register::Rdi as usize] = 0x1234_0000_0000_0000 | WRAP;
            };
            let mut set_memory = memory.clone();
            let mut set_state = state(entry);
            setup(&mut set_state);
            let (set_exit, _, set_retired, _) = run(
                &mut set_state,
                &mut set_memory,
                1,
                None,
                od_isa::INTEGER_FLAGS,
            );
            let mut runner_memory = memory.clone();
            let mut runner_state = LongModeState {
                rip: CODE,
                rflags: entry,
                activity: Some(CpuActivity::Running),
                sti_shadow: Some(StiShadow::Clear),
                ..LongModeState::default()
            };
            setup(&mut runner_state);
            let mut continuation: Option<RepContinuation> = None;
            let (mut runner_exit, mut runner_retired);
            loop {
                let result = run_with_rep_continuation(
                    &mut runner_state,
                    &mut runner_memory,
                    RunConfig {
                        instruction_budget: 1,
                        stop_rip: None,
                        context: context(),
                    },
                    od_isa::INTEGER_FLAGS,
                    &mut continuation,
                    || false,
                )
                .unwrap();
                runner_exit = result.exit;
                runner_retired = result.instructions_executed;
                if runner_exit != RunExit::RepIterationBudgetExhausted {
                    break;
                }
            }
            let what = format!("{code:02x?} count {count} hit {hit:?} fault {fault_k:?}");
            assert_eq!(set_exit, runner_exit, "{what}");
            assert_eq!(set_retired, runner_retired, "{what}");
            assert_eq!(set_state.rip, runner_state.rip, "{what}");
            assert_eq!(set_state.gpr, runner_state.gpr, "{what}");
            assert_eq!(set_memory.reads, runner_memory.reads, "{what}");
            match fault_k {
                Some(k) if k >= 2 => {
                    assert_ne!(set_state.rflags, runner_state.rflags, "{what}");
                    assert_eq!(runner_state.rflags, entry, "{what}: the runner restores");
                }
                _ => assert_eq!(set_state.rflags, runner_state.rflags, "{what}"),
            }
            let _ = width;
        }
    }
}
