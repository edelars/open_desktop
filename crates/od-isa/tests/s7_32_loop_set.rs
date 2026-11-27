//! S7-32 slice S (ISSUE-713): `LOOP`, `LOOPE`, `LOOPNE` (`E2`/`E1`/`E0 cb`) and `JRCXZ`/`JECXZ`
//! (`E3 cb`), bare (counter RCX) and with `67` (counter ECX), through the CPL0 profile set at CPL0,
//! checked against an independent model written from the Intel SDM (325462-093US):
//!
//! - Vol. 2A "LOOP/LOOPcc—Loop According to ECX Counter": `Count := Count − 1` (RCX at address
//!   size 64, ECX at 32) without changing the flags; the branch is taken when the new count is not
//!   0, and for LOOPE also ZF = 1, for LOOPNE ZF = 0; target `RIP + rel8`; a target outside the
//!   canonical space is `#GP(0)`.
//! - Vol. 2A "Jcc—Jump if Condition Is Met": JRCXZ (JECXZ with `67`) jumps when RCX (ECX) is 0;
//!   the counter does not change.
//! - Vol. 1 §3.4.1.1: a 32-bit result is zero-extended into the 64-bit register (the model writes
//!   ECX zero-extended at address size 32, the contract of the `67` REP forms, S2-19m/S2-19o).
//!
//! A fault is a fault: the instruction does not execute, RCX and RIP stay.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, run as bare_run,
    run_with_cpl0_profile_set, run_with_rep_continuation,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const ZF: u64 = 1 << 6;
const AF: u64 = 1 << 4;

/// Code at `code_base`, data in one writable region.
struct Memory {
    code_base: u64,
    code: Vec<u8>,
    data_base: u64,
    data: Vec<u8>,
}

impl Memory {
    fn new(code_base: u64, code: &[u8]) -> Self {
        Self {
            code_base,
            code: code.to_vec(),
            data_base: 0,
            data: Vec::new(),
        }
    }

    fn offset(&self, address: u64, len: usize) -> Result<usize, MemoryError> {
        let offset = address.wrapping_sub(self.data_base);
        if offset.saturating_add(len as u64) <= self.data.len() as u64 {
            Ok(offset as usize)
        } else {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.wrapping_sub(self.code_base) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let at = self.offset(address, out.len())?;
        out.copy_from_slice(&self.data[at..at + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let at = self.offset(address, data.len())?;
        self.data[at..at + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        let mut bytes = [0u8; 8];
        self.read(address, context, &mut bytes[..size.bytes()])?;
        Ok(u64::from_le_bytes(bytes))
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

fn state(rip: u64, rflags: u64) -> LongModeState {
    LongModeState {
        rip,
        rflags,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

fn run(
    state: &mut LongModeState,
    memory: &mut Memory,
    budget: u64,
    stop_rip: Option<u64>,
    defined: u64,
) -> (RunExit, u64, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let result = run_with_cpl0_profile_set(
        state,
        memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip,
            context: AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
        },
        defined,
        &set,
        None,
        || false,
    )
    .expect("the state is inside the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("expected a Run exit");
    };
    (exit, result.instructions_executed, result.rflags_defined)
}

fn canonical(address: u64) -> bool {
    let top = address >> 47;
    top == 0 || top == 0x1_ffff
}

/// The model: `(RCX, RIP)` after the instruction, or `None` for `#GP(0)`.
fn model(
    opcode: u8,
    address32: bool,
    rcx: u64,
    zf: bool,
    rip: u64,
    length: u64,
    rel8: i8,
) -> Option<(u64, u64)> {
    let next = rip + length;
    let count = if address32 { rcx & 0xffff_ffff } else { rcx };
    let (branch, new_rcx) = if opcode == 0xe3 {
        (count == 0, rcx)
    } else {
        let decremented = if address32 {
            (count as u32).wrapping_sub(1) as u64
        } else {
            count.wrapping_sub(1)
        };
        let condition = match opcode {
            0xe1 => zf,
            0xe0 => !zf,
            _ => true,
        };
        (decremented != 0 && condition, decremented)
    };
    if branch {
        let target = next.wrapping_add_signed(i64::from(rel8));
        if !canonical(target) {
            return None;
        }
        Some((new_rcx, target))
    } else {
        Some((new_rcx, next))
    }
}

#[test]
fn the_loop_family_matches_the_sdm_model() {
    let mut cases = 0;
    for opcode in [0xe0u8, 0xe1, 0xe2, 0xe3] {
        for address32 in [false, true] {
            for rcx in [
                0u64,
                1,
                2,
                0xffff_ffff,
                0x1_0000_0000,
                0x1_0000_0001,
                u64::MAX,
            ] {
                for zf in [false, true] {
                    for rel8 in [-128i8, -2, 0, 127] {
                        let code: Vec<u8> = if address32 {
                            vec![0x67, opcode, rel8 as u8]
                        } else {
                            vec![opcode, rel8 as u8]
                        };
                        let rflags = 2 | AF | if zf { ZF } else { 0 };
                        let defined = od_isa::INTEGER_FLAGS & !AF;
                        let mut memory = Memory::new(CODE, &code);
                        let mut state = state(CODE, rflags);
                        state.gpr[Register::Rcx as usize] = rcx;
                        let before = state.clone();
                        let (exit, retired, defined_after) =
                            run(&mut state, &mut memory, 1, None, defined);
                        let expected =
                            model(opcode, address32, rcx, zf, CODE, code.len() as u64, rel8)
                                .expect("canonical");
                        let what = format!("{code:02x?} rcx {rcx:#x} zf {zf}");
                        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{what}");
                        assert_eq!(retired, 1, "{what}");
                        assert_eq!(
                            (state.gpr[Register::Rcx as usize], state.rip),
                            expected,
                            "{what}"
                        );
                        assert_eq!(state.rflags, rflags, "{what}: flags unchanged");
                        assert_eq!(defined_after, defined, "{what}: definedness unchanged");
                        let mut other = state.clone();
                        other.gpr[Register::Rcx as usize] = before.gpr[Register::Rcx as usize];
                        other.rip = before.rip;
                        assert_eq!(other, before, "{what}: nothing else changed");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 4 * 2 * 7 * 2 * 4);
}

/// The cases the review named: RCX = 2^32 — JECXZ jumps (ECX = 0), JRCXZ does not; `67 LOOP`
/// leaves ECX' = 0xffff_ffff zero-extended and jumps.
#[test]
fn a_count_of_two_to_the_32_separates_the_counter_widths() {
    let rcx = 0x1_0000_0000u64;
    let cases: [(&[u8], u64, u64); 4] = [
        (&[0xe3, 0x10], rcx, CODE + 2),                      // JRCXZ: RCX != 0
        (&[0x67, 0xe3, 0x10], rcx, CODE + 3 + 0x10),         // JECXZ: ECX = 0
        (&[0xe2, 0x10], rcx - 1, CODE + 2 + 0x10),           // LOOP: RCX − 1 != 0
        (&[0x67, 0xe2, 0x10], 0xffff_ffff, CODE + 3 + 0x10), // 67 LOOP: ECX − 1 = 0xffff_ffff
    ];
    for (code, rcx_after, rip_after) in cases {
        let mut memory = Memory::new(CODE, code);
        let mut state = state(CODE, 2);
        state.gpr[Register::Rcx as usize] = rcx;
        run(&mut state, &mut memory, 1, None, od_isa::INTEGER_FLAGS);
        assert_eq!(
            (state.gpr[Register::Rcx as usize], state.rip),
            (rcx_after, rip_after),
            "{code:02x?}"
        );
    }
}

/// A taken branch to a non-canonical target is `#GP(0)`: RCX and RIP stay (the instruction does
/// not execute); a branch not taken from the same place executes.
#[test]
fn a_non_canonical_target_is_a_general_protection_fault_before_the_counter_changes() {
    // The instruction ends at 0x0000_7fff_ffff_fff0; +127 leaves the lower canonical half.
    let rip = 0x0000_7fff_ffff_fff0 - 3;
    for (code, rcx, faults) in [
        (vec![0x67, 0xe2, 0x7f], 5u64, true),
        (vec![0x67, 0xe2, 0x7f], 1, false),
        (vec![0x67, 0xe3, 0x7f], 0, true),
        (vec![0x67, 0xe1, 0x7f], 5, false), // ZF = 0: LOOPE not taken
        (vec![0x67, 0xe0, 0x7f], 5, true),
    ] {
        let mut memory = Memory::new(rip, &code);
        let mut state = state(rip, 2);
        state.gpr[Register::Rcx as usize] = rcx;
        let before = state.clone();
        let (exit, retired, _) = run(&mut state, &mut memory, 1, None, od_isa::INTEGER_FLAGS);
        if faults {
            let RunExit::Fault(fault) = exit else {
                panic!("{code:02x?}: {exit:?}");
            };
            assert_eq!((fault.vector, fault.error_code), (13, Some(0)));
            assert_eq!(retired, 0);
            assert_eq!(state, before, "{code:02x?}");
        } else {
            assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{code:02x?}");
            assert_eq!(state.rip, rip + 3);
        }
    }
}

/// The PatchGuard context decryption stub of ISSUE-713 (`ntkrnlmp` + `0xc1870f`…`+0xc18720`):
/// `xor [rdx+rcx*8+0xc0], rax; ror rax, cl; btc rax, rax; loop -0x11`, byte for byte, against the
/// model of the same loop.
#[test]
fn the_patchguard_decryption_loop_runs_to_its_end() {
    const STUB: [u8; 17] = [
        0x48, 0x31, 0x84, 0xca, 0xc0, 0x00, 0x00, 0x00, // xor [rdx+rcx*8+0xc0], rax
        0x48, 0xd3, 0xc8, // ror rax, cl
        0x48, 0x0f, 0xbb, 0xc0, // btc rax, rax
        0xe2, 0xef, // loop -0x11
    ];
    const DATA: u64 = 0x10_0000;
    for count in [3u64, 0x3a3a] {
        let words = count as usize + 0x20;
        let initial: Vec<u64> = (0..words as u64)
            .map(|i| 0x0123_4567_89ab_cdef ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .collect();
        let key = 0xad1b_6ff5_bc2a_27dbu64;
        // The model.
        let mut words_model = initial.clone();
        let (mut rax, mut rcx) = (key, count);
        loop {
            let index = (rcx as usize) + 0xc0 / 8;
            words_model[index] ^= rax;
            rax = rax.rotate_right((rcx & 63) as u32);
            rax ^= 1 << (rax & 63);
            rcx -= 1;
            if rcx == 0 {
                break;
            }
        }
        let mut memory = Memory::new(CODE, &STUB);
        memory.data_base = DATA;
        memory.data = initial.iter().flat_map(|word| word.to_le_bytes()).collect();
        let mut state = state(CODE, 2);
        state.gpr[Register::Rax as usize] = key;
        state.gpr[Register::Rcx as usize] = count;
        state.gpr[Register::Rdx as usize] = DATA;
        let end = CODE + STUB.len() as u64;
        let (exit, retired, _) = run(
            &mut state,
            &mut memory,
            4 * count + 1,
            Some(end),
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(exit, RunExit::StopAddress, "count {count:#x}");
        assert_eq!(retired, 4 * count);
        assert_eq!(state.gpr[Register::Rcx as usize], 0);
        assert_eq!(state.gpr[Register::Rax as usize], rax);
        let words_after: Vec<u64> = memory
            .data
            .chunks(8)
            .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        assert_eq!(words_after, words_model, "count {count:#x}");
    }
}

/// Outside the CPL0 set the LOOP family stays what it was before slice S: the bare step (`run`,
/// no profile) and the bounded continuation runner refuse `E0`..`E3` and their `67` forms with the
/// opcode, before any change of state.
#[test]
fn the_loop_family_stays_unsupported_outside_the_set() {
    for opcode in [0xe0u8, 0xe1, 0xe2, 0xe3] {
        for code in [vec![opcode, 0xfe], vec![0x67, opcode, 0xfe]] {
            for rcx in [0u64, 1, 5] {
                let fresh = || {
                    let mut state = LongModeState {
                        rip: CODE,
                        rflags: 2 | ZF,
                        activity: Some(CpuActivity::Running),
                        sti_shadow: Some(StiShadow::Clear),
                        ..LongModeState::default()
                    };
                    state.gpr[Register::Rcx as usize] = rcx;
                    state
                };
                let config = RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext {
                        mode: InstructionMode::Long64,
                        privilege: PrivilegeLevel::Ring0,
                        origin: AccessOrigin::Explicit,
                    },
                };
                let unsupported = RunExit::Unsupported { opcode };

                let mut state = fresh();
                let result = bare_run(&mut state, &mut Memory::new(CODE, &code), config, || false);
                assert_eq!(result.exit, unsupported, "bare {code:02x?} rcx {rcx}");
                assert_eq!(result.instructions_executed, 0);
                assert_eq!(state, fresh(), "bare {code:02x?} rcx {rcx}");

                let mut state = fresh();
                let mut continuation = None;
                let result = run_with_rep_continuation(
                    &mut state,
                    &mut Memory::new(CODE, &code),
                    config,
                    od_isa::INTEGER_FLAGS,
                    &mut continuation,
                    || false,
                )
                .unwrap();
                assert_eq!(
                    result.exit, unsupported,
                    "continuation {code:02x?} rcx {rcx}"
                );
                assert_eq!(result.instructions_executed, 0);
                assert_eq!(state, fresh(), "continuation {code:02x?} rcx {rcx}");
            }
        }
    }
}
