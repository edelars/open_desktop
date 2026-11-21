//! S7-31c: the two forms the firmware ROM and the EFI stub of the Ubuntu kernel need on the
//! unified "UEFI + Linux" board (ADR-0013, variant (b)), checked through the CPL0 profile set.
//!
//! The semantics are written from the Intel SDM, Vol. 2:
//!
//! - `AND r/m64, imm8` (`REX.W 83 /4 ib`): the immediate is sign-extended to 64 bits, the
//!   destination (here a memory qword) becomes `dst & imm`; CF and OF are cleared; SF, ZF and PF
//!   are set from the result (PF from its low byte); AF is undefined.
//! - `ENDBR64` (`F3 0F 1E FA`): without CET indirect-branch tracking it is a four-byte NOP: no
//!   register, flag or memory effect.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
    run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x1000;
const DATA: u64 = 0x2000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const ARITHMETIC: u64 = CF | PF | AF | ZF | SF | OF;

/// Code at `CODE`, one data page at `DATA`; every other address is unmapped. Counts the data
/// accesses so a NOP can be shown to make none.
struct Memory {
    code: Vec<u8>,
    data: Vec<u8>,
    reads: usize,
    writes: usize,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            data: vec![0; 0x100],
            reads: 0,
            writes: 0,
        }
    }

    fn set_qword(&mut self, offset: usize, value: u64) {
        self.data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn qword(&self, offset: usize) -> u64 {
        u64::from_le_bytes(self.data[offset..offset + 8].try_into().unwrap())
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
        let offset = address.wrapping_sub(DATA) as usize;
        let source = self
            .data
            .get(offset..offset + out.len())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })?;
        out.copy_from_slice(source);
        self.reads += 1;
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, bytes: &[u8]) -> Result<(), MemoryError> {
        let offset = address.wrapping_sub(DATA) as usize;
        let target =
            self.data
                .get_mut(offset..offset + bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })?;
        target.copy_from_slice(bytes);
        self.writes += 1;
        Ok(())
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

fn state(rflags: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0xa5a5_a5a5_a5a5_a5a5;
    state.gpr[Register::Rbx as usize] = DATA;
    state.gpr[Register::Rcx as usize] = 3;
    state
}

/// Run exactly one instruction through the set; returns the exit and the defined-flags mask.
fn one(state: &mut LongModeState, memory: &mut Memory) -> (RunExit, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let result = run_with_cpl0_profile_set(
        state,
        memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .expect("the state is inside the set");
    assert_eq!(result.instructions_executed, 1);
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("expected a Run exit");
    };
    (exit, result.rflags_defined)
}

/// `and qword ptr [rsp+8], -2`: the form of `od_firmware::pe::loader::load_into+0x276` (ISSUE-621).
const AND_RSP8_M2: [u8; 6] = [0x48, 0x83, 0x64, 0x24, 0x08, 0xfe];

#[test]
fn and_qword_memory_imm8_clears_bit_zero_and_sets_flags_from_the_result() {
    // value, expected result, expected SF|ZF|PF
    let cases: [(u64, u64, u64); 4] = [
        // 0xee has six one bits: PF = 1.
        (0x0123_4567_89ab_cdef, 0x0123_4567_89ab_cdee, PF),
        // all ones: 0xfe has seven one bits (PF = 0), bit 63 stays set (SF = 1).
        (u64::MAX, u64::MAX - 1, SF),
        // 1 & ~1 = 0: ZF = 1, PF = 1 (zero has no one bits).
        (1, 0, ZF | PF),
        (0, 0, ZF | PF),
    ];
    for (value, expected, flags) in cases {
        // CF and OF set before: AND clears them. The unrelated flags stay.
        let mut state = state(2 | CF | OF | (1 << 9) | (1 << 10));
        let mut memory = Memory::new(&AND_RSP8_M2);
        memory.set_qword(8, value);
        let (exit, defined) = one(&mut state, &mut memory);
        assert_eq!(
            exit,
            RunExit::InstructionBudgetExhausted,
            "value {value:#x}"
        );
        assert_eq!(memory.qword(8), expected, "value {value:#x}");
        assert_eq!((memory.reads, memory.writes), (1, 1));
        assert_eq!(state.rip, CODE + 6);
        assert_eq!(state.rflags & (CF | OF), 0, "CF and OF are cleared");
        assert_eq!(state.rflags & (SF | ZF | PF), flags, "value {value:#x}");
        assert_eq!(state.rflags & ((1 << 9) | (1 << 10)), (1 << 9) | (1 << 10));
        assert_eq!(state.rflags & 2, 2);
        // AF is undefined after AND: the mask the CPU reports must not claim it.
        assert_eq!(defined & AF, 0);
        assert_eq!(defined & (ARITHMETIC & !AF), ARITHMETIC & !AF);
        // Nothing but memory, RIP and RFLAGS changed.
        assert_eq!(state.gpr[Register::Rax as usize], 0xa5a5_a5a5_a5a5_a5a5);
        assert_eq!(state.gpr[Register::Rsp as usize], DATA);
    }
}

#[test]
fn and_qword_memory_imm8_sign_extends_the_immediate() {
    // imm8 = 0x0f (positive): the high 56 bits of the mask are zero.
    let mut state_a = state(2);
    let mut memory = Memory::new(&[0x48, 0x83, 0x64, 0x24, 0x08, 0x0f]);
    memory.set_qword(8, 0xffff_ffff_ffff_ffff);
    one(&mut state_a, &mut memory);
    assert_eq!(memory.qword(8), 0x0f);
    // imm8 = 0x80 (-128): the high 56 bits of the mask are one.
    let mut state_b = state(2);
    let mut memory = Memory::new(&[0x48, 0x83, 0x64, 0x24, 0x08, 0x80]);
    memory.set_qword(8, 0x1234_5678_9abc_def0);
    one(&mut state_b, &mut memory);
    // 0x...def0 & 0x...ff80: only bit 7 of the low byte survives, 0xde above it is kept.
    assert_eq!(memory.qword(8), 0x1234_5678_9abc_de80);
}

#[test]
fn and_qword_memory_imm8_with_sib_and_displacement_forms() {
    // and qword [rbx + rcx*4 + 0x10], -2  (REX.W 83 /4, SIB, disp8): 0x2000 + 12 + 0x10 = 0x201c.
    let mut state_a = state(2);
    let mut memory = Memory::new(&[0x48, 0x83, 0x64, 0x8b, 0x10, 0xfe]);
    memory.set_qword(0x1c, 0x0000_0000_0000_0003);
    one(&mut state_a, &mut memory);
    assert_eq!(memory.qword(0x1c), 2);
    assert_eq!(state_a.rip, CODE + 6);
    // and qword [rbx + 0x40], -2  (disp32 = 0x00000040 spelled with mod = 10).
    let mut state_b = state(2);
    let mut memory = Memory::new(&[0x48, 0x83, 0xa3, 0x40, 0x00, 0x00, 0x00, 0xfe]);
    memory.set_qword(0x40, 0x8000_0000_0000_0001);
    one(&mut state_b, &mut memory);
    assert_eq!(memory.qword(0x40), 0x8000_0000_0000_0000);
    assert_eq!(state_b.rflags & SF, SF);
    assert_eq!(state_b.rip, CODE + 8);
    // and qword [rbx], -2  (no displacement).
    let mut state_c = state(2);
    let mut memory = Memory::new(&[0x48, 0x83, 0x23, 0xfe]);
    memory.set_qword(0, 0x11);
    one(&mut state_c, &mut memory);
    assert_eq!(memory.qword(0), 0x10);
    assert_eq!(state_c.rip, CODE + 4);
}

#[test]
fn endbr64_is_a_four_byte_nop() {
    let mut state = state(2 | CF | ZF | (1 << 9));
    let before = (state.gpr, state.rflags);
    let mut memory = Memory::new(&[0xf3, 0x0f, 0x1e, 0xfa]);
    let (exit, defined) = one(&mut state, &mut memory);
    assert_eq!(exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, CODE + 4);
    assert_eq!((state.gpr, state.rflags), before);
    assert_eq!(defined, od_isa::INTEGER_FLAGS);
    assert_eq!((memory.reads, memory.writes), (0, 0));
}
