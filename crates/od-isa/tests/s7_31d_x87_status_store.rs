//! S7-31d: `FNSTSW m2byte` (`DD /7`) and `FNSTCW m2byte` (`D9 /7`) with any memory operand in the
//! CPL0 profile set that owns the FP state, and the four instructions the EFI stub of the Ubuntu
//! kernel runs right after them (`66 83 7C 24 0E 00`, `0F B7 44 24 0C`, `66 25 3F 10`,
//! `66 83 F8 3F`).
//!
//! Semantics from the Intel SDM:
//!
//! - FNSTSW/FNSTCW (Vol. 2A): store the x87 status word / control word at the 16-bit operand
//!   without checking for pending unmasked exceptions (no FWAIT); #NM if CR0.EM or CR0.TS is 1;
//!   the usual memory faults. They are x87 control instructions: they change no flag, no
//!   register, and not the x87 instruction/data pointers or the opcode register (Vol. 1, 8.1.8).
//! - After FNINIT (Vol. 1, Table 8-1): FCW = 0x037F, FSW = 0.
//! - CMP r/m16, imm8 and CMP AX, imm8 (sign-extended): flags as SUB; AND AX, imm16: CF = OF = 0,
//!   SF/ZF/PF from the 16-bit result, AF undefined; MOVZX r32, m16 zero-extends into RAX.

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig, RunExit,
    run, run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};
use std::cell::RefCell;

const CODE: u64 = 0x1000;
const DATA: u64 = 0x2000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const ARITHMETIC: u64 = CF | PF | AF | ZF | SF | OF;

struct Memory {
    code: Vec<u8>,
    data: Vec<u8>,
    writes: Vec<(u64, usize)>,
    /// A write to this address is a page fault.
    fault_write_at: Option<u64>,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            data: vec![0xee; 0x100],
            writes: Vec::new(),
            fault_write_at: None,
        }
    }

    fn word(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    fn set_word(&mut self, offset: usize, value: u16) {
        self.data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
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
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, bytes: &[u8]) -> Result<(), MemoryError> {
        if self.fault_write_at == Some(address) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(2),
                linear_address: Some(address),
            }));
        }
        let offset = address.wrapping_sub(DATA) as usize;
        let target =
            self.data
                .get_mut(offset..offset + bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })?;
        target.copy_from_slice(bytes);
        self.writes.push((address, bytes.len()));
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

fn state(cr0: u64, rflags: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags,
        cr0: Some(cr0),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0xa5a5_a5a5_a5a5_a5a5;
    state.gpr[Register::Rbx as usize] = DATA;
    state.gpr[Register::Rcx as usize] = 3;
    state
}

/// FP state away from the initial values, with known selectors and pointers.
fn fp(status_word: u16, control_word: u16) -> RefCell<GuestFpState> {
    let mut fp = GuestFpState::initial();
    fp.x87.status_word = status_word;
    fp.x87.control_word = control_word;
    fp.x87.instruction_pointer = 0x1111;
    fp.x87.data_pointer = 0x2222;
    fp.x87.instruction_selector = Some(0x2b);
    fp.x87.data_selector = Some(0x53);
    fp.x87.last_opcode = 0x155;
    RefCell::new(fp)
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn config(budget: u64) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: context(),
    }
}

/// Run `budget` instructions through the set with the FP member over `cell`.
fn run_fp(
    state: &mut LongModeState,
    memory: &mut Memory,
    cell: &RefCell<GuestFpState>,
    budget: u64,
) -> (RunExit, u64, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(cell);
    let result = run_with_cpl0_profile_set(
        state,
        memory,
        config(budget),
        od_isa::INTEGER_FLAGS,
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

fn nm() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 7,
        error_code: None,
        linear_address: None,
    })
}

#[test]
fn fnstsw_and_fnstcw_store_fsw_and_fcw_after_fninit_through_rsp_disp8() {
    // fninit; fnstsw [rsp+0xe]; fnstcw [rsp+0xc] -- the stub's own sequence of addresses.
    let cell = fp(0x4520, 0x0c7f);
    let mut state = state(CR0_BASE, 2 | CF | ZF | (1 << 9));
    let mut memory = Memory::new(&[0xdb, 0xe3, 0xdd, 0x7c, 0x24, 0x0e, 0xd9, 0x7c, 0x24, 0x0c]);
    let before = (state.gpr, state.rflags);
    let (exit, retired, defined) = run_fp(&mut state, &mut memory, &cell, 3);
    assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 3));
    // FNINIT: FCW 0x037F, FSW 0 (Vol. 1, Table 8-1).
    assert_eq!(memory.word(0x0e), 0x0000);
    assert_eq!(memory.word(0x0c), 0x037f);
    // Exactly the two words were written; nothing else of the page changed.
    assert_eq!(memory.writes, vec![(DATA + 0x0e, 2), (DATA + 0x0c, 2)]);
    assert!(memory.data[..0x0c].iter().all(|&byte| byte == 0xee));
    assert!(memory.data[0x10..].iter().all(|&byte| byte == 0xee));
    assert_eq!(state.rip, CODE + 10);
    assert_eq!((state.gpr, state.rflags), before);
    assert_eq!(defined, od_isa::INTEGER_FLAGS);
}

#[test]
fn fnstsw_and_fnstcw_store_the_current_words_without_changing_anything_else() {
    // No FNINIT: whatever the words are (here a status word with busy, C3, C2, C0 and a pending
    // exception summary, and a control word with non-default precision and rounding).
    let cell = fp(0xc5a1, 0x0c7f);
    let before_fp = cell.borrow().clone();
    let mut state = state(CR0_BASE, 2 | CF | OF | SF | (1 << 10));
    let before = (state.gpr, state.rflags);
    let mut memory = Memory::new(&[0xdd, 0x7c, 0x24, 0x0e, 0xd9, 0x7c, 0x24, 0x0c]);
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 2);
    assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 2));
    assert_eq!(memory.word(0x0e), 0xc5a1);
    assert_eq!(memory.word(0x0c), 0x0c7f);
    assert_eq!((state.gpr, state.rflags), before);
    // Control instructions: FIP, FDP, FOP, selectors and the words stay (Vol. 1, 8.1.8).
    assert_eq!(*cell.borrow(), before_fp);
}

#[test]
fn fnstsw_and_fnstcw_accept_every_memory_addressing_form() {
    // RBX = DATA, RCX = 3; no prefix.
    let forms: [(&[u8], usize); 7] = [
        // [rbx]
        (&[0xdd, 0x3b], 0x00),
        // [rbx+rcx*4] (SIB, no displacement)
        (&[0xdd, 0x3c, 0x8b], 0x0c),
        // [rbx+rcx*4+0x10] (SIB, disp8)
        (&[0xdd, 0x7c, 0x8b, 0x10], 0x1c),
        // [rbx+0x40] (disp32)
        (&[0xdd, 0xbb, 0x40, 0x00, 0x00, 0x00], 0x40),
        // [rsp+0x20] (SIB base rsp, disp8)
        (&[0xdd, 0x7c, 0x24, 0x20], 0x20),
        // [rbx+rcx*8+0x2] (SIB scale 8)
        (&[0xdd, 0x7c, 0xcb, 0x02], 0x1a),
        // [rip+0x0ffa]: 0x1006 (next RIP) + 0xffa = 0x2000 = DATA
        (&[0xdd, 0x3d, 0xfa, 0x0f, 0x00, 0x00], 0x00),
    ];
    for (code, offset) in forms {
        for control in [false, true] {
            let mut bytes = code.to_vec();
            if control {
                // The same operand with opcode D9 instead of DD.
                let at = bytes.iter().position(|&byte| byte == 0xdd).unwrap();
                bytes[at] = 0xd9;
            }
            let cell = fp(0x3801, 0x027f);
            let mut state = state(CR0_BASE, 2);
            let mut memory = Memory::new(&bytes);
            let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
            assert_eq!(
                (exit, retired),
                (RunExit::InstructionBudgetExhausted, 1),
                "{bytes:02x?}"
            );
            let expected = if control { 0x027f } else { 0x3801 };
            assert_eq!(memory.word(offset), expected, "{bytes:02x?}");
            assert_eq!(
                memory.writes,
                vec![(DATA + offset as u64, 2)],
                "{bytes:02x?}"
            );
            assert_eq!(state.rip, CODE + bytes.len() as u64, "{bytes:02x?}");
        }
    }
}

#[test]
fn fnstsw_and_fnstcw_raise_nm_when_cr0_em_or_ts_is_set_and_store_nothing() {
    for cr0 in [
        CR0_BASE | CR0_EM,
        CR0_BASE | CR0_TS,
        CR0_BASE | CR0_EM | CR0_TS,
    ] {
        for code in [[0xdd, 0x7c, 0x24, 0x0e], [0xd9, 0x7c, 0x24, 0x0c]] {
            let cell = fp(0x1234, 0x037f);
            let mut state = state(cr0, 2 | CF);
            let before = (state.gpr, state.rflags);
            let mut memory = Memory::new(&code);
            let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
            assert_eq!((exit, retired), (nm(), 0), "cr0 {cr0:#x} {code:02x?}");
            assert!(memory.writes.is_empty());
            assert_eq!(state.rip, CODE, "the faulting instruction does not retire");
            assert_eq!((state.gpr, state.rflags), before);
        }
    }
}

#[test]
fn fnstsw_and_fnstcw_page_fault_on_the_store_leaves_the_instruction_unretired() {
    for code in [[0xdd, 0x7c, 0x24, 0x0e], [0xd9, 0x7c, 0x24, 0x0c]] {
        let cell = fp(0x1234, 0x037f);
        let mut state = state(CR0_BASE, 2 | CF | ZF);
        let before = (state.gpr, state.rflags);
        let mut memory = Memory::new(&code);
        memory.fault_write_at = Some(DATA + u64::from(code[3]));
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!(
            (exit, retired),
            (
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(2),
                    linear_address: Some(DATA + u64::from(code[3])),
                }),
                0
            ),
            "{code:02x?}"
        );
        assert!(memory.writes.is_empty());
        assert_eq!(state.rip, CODE);
        assert_eq!((state.gpr, state.rflags), before);
    }
}

#[test]
fn fnstsw_and_fnstcw_with_a_non_canonical_stack_address_fault_as_ss_zero() {
    // RSP-based and non-canonical: #SS(0) (SDM Vol. 1, 3.3.7.1: RSP/RBP bases use SS).
    let cell = fp(0, 0x037f);
    let mut state = state(CR0_BASE, 2);
    state.gpr[Register::Rsp as usize] = 0x0001_0000_0000_0000;
    let mut memory = Memory::new(&[0xdd, 0x7c, 0x24, 0x0e]);
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!(
        (exit, retired),
        (
            RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            }),
            0
        )
    );
    assert!(memory.writes.is_empty());
    assert_eq!(state.rip, CODE);
}

#[test]
fn the_new_forms_stay_outside_every_path_but_the_set_with_the_fp_member() {
    for code in [[0xdd, 0x7c, 0x24, 0x0e], [0xd9, 0x7c, 0x24, 0x0c]] {
        // The plain run loop (no profile set).
        let mut state_a = state(CR0_BASE, 2);
        let mut memory = Memory::new(&code);
        let result = run(&mut state_a, &mut memory, config(1), || false);
        assert!(
            matches!(result.exit, RunExit::Unsupported { .. }),
            "{code:02x?}: {:?}",
            result.exit
        );
        assert!(memory.writes.is_empty());
        assert_eq!(state_a.rip, CODE);
        // The set without the FP member (CPUID declared, no `with_fp_state`).
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&CpuidProfile::X8664V1Stage4A);
        let mut state_b = state(CR0_BASE, 2);
        let mut memory = Memory::new(&code);
        let result = run_with_cpl0_profile_set(
            &mut state_b,
            &mut memory,
            config(1),
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap();
        let Cpl0ProfileSetExit::Run(exit) = result.exit else {
            panic!("expected a Run exit");
        };
        assert_eq!(
            exit,
            RunExit::Unsupported { opcode: code[0] },
            "{code:02x?}"
        );
        assert!(memory.writes.is_empty());
        assert_eq!(state_b.rip, CODE);
        // A prefix or the register form is not this instruction (`DD /7` with mod = 3 is not
        // FNSTSW; a segment override or an operand-size prefix is outside the form).
    }
    let cell = fp(0, 0x037f);
    for code in [
        // A REX prefix (REX.B would reach r8..r15) is outside the form, as every prefix is.
        &[0x41, 0xdd, 0x79, 0x08][..],
        &[0x66, 0xdd, 0x7c, 0x24, 0x0e][..],
        &[0x67, 0xdd, 0x7c, 0x24, 0x0e][..],
        &[0x64, 0xdd, 0x7c, 0x24, 0x0e][..],
        &[0x66, 0xd9, 0x7c, 0x24, 0x0c][..],
        &[0xdd, 0xf8][..],
    ] {
        let mut state = state(CR0_BASE, 2);
        let mut memory = Memory::new(code);
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert!(
            matches!(exit, RunExit::Unsupported { .. }),
            "{code:02x?}: {exit:?}"
        );
        assert_eq!(retired, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cmp_word_memory_imm8_sets_the_flags_of_a_16_bit_subtraction() {
    // 66 83 7C 24 0E 00: cmp word ptr [rsp+0xe], 0
    // (value, expected flags): flags are those of value - 0 (CF = AF = OF = 0).
    for (value, flags) in [
        (0x0000, ZF | PF),
        (0x8000, SF | PF),
        (0x0001, 0),
        (0x0005, PF),
        (0x0100, PF),
    ] {
        let cell = fp(0, 0x037f);
        let mut state = state(CR0_BASE, 2 | CF | OF | AF);
        let mut memory = Memory::new(&[0x66, 0x83, 0x7c, 0x24, 0x0e, 0x00]);
        memory.set_word(0x0e, value);
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 1));
        assert_eq!(state.rflags & ARITHMETIC, flags, "value {value:#x}");
        assert_eq!(state.rip, CODE + 6);
        assert!(memory.writes.is_empty(), "CMP writes nothing");
        assert_eq!(memory.word(0x0e), value);
    }
}

#[test]
fn movzx_eax_word_memory_zero_extends_into_rax_and_keeps_the_flags() {
    // 0F B7 44 24 0C: movzx eax, word ptr [rsp+0xc]
    let cell = fp(0, 0x037f);
    let mut state = state(CR0_BASE, 2 | CF | ZF | OF);
    let mut memory = Memory::new(&[0x0f, 0xb7, 0x44, 0x24, 0x0c]);
    memory.set_word(0x0c, 0x037f);
    let flags = state.rflags;
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 1));
    // A 32-bit destination zero-extends to RAX (SDM Vol. 1, 3.4.1.1).
    assert_eq!(state.gpr[Register::Rax as usize], 0x037f);
    assert_eq!(state.rflags, flags);
    assert_eq!(state.rip, CODE + 5);
}

#[test]
fn and_ax_imm16_masks_the_low_word_only_and_sets_the_flags_of_the_result() {
    // 66 25 3F 10: and ax, 0x103f. RAX bits 16..63 stay (a 16-bit write keeps them).
    for (ax, result, flags) in [
        (0x037f_u16, 0x003f_u16, PF),
        (0xffff, 0x103f, PF),
        (0x8000, 0x0000, ZF | PF),
        (0x1000, 0x1000, PF),
        (0x0001, 0x0001, 0),
    ] {
        let cell = fp(0, 0x037f);
        let mut state = state(CR0_BASE, 2 | CF | OF | SF);
        state.gpr[Register::Rax as usize] = 0xa5a5_a5a5_a5a5_0000 | u64::from(ax);
        let mut memory = Memory::new(&[0x66, 0x25, 0x3f, 0x10]);
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 1));
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0xa5a5_a5a5_a5a5_0000 | u64::from(result),
            "ax {ax:#x}"
        );
        // CF = OF = 0; SF, ZF, PF from the 16-bit result (AF is undefined, not checked).
        assert_eq!(state.rflags & (CF | OF | SF | ZF | PF), flags, "ax {ax:#x}");
        assert_eq!(state.rip, CODE + 4);
    }
}

#[test]
fn cmp_ax_imm8_compares_the_low_word_with_the_sign_extended_immediate() {
    // 66 83 F8 3F: cmp ax, 0x3f. Flags are those of ax - 0x003f in 16 bits.
    for (ax, flags) in [
        (0x003f_u16, ZF | PF),
        // 0x20 - 0x3f = 0xffe1: borrow, AF (low nibble 0 < 0xf), sign; PF of 0xe1 (4 ones).
        (0x0020, CF | AF | SF | PF),
        // 0x40 - 0x3f = 1: nothing set but AF (0 - 0xf borrows from bit 4), PF clear.
        (0x0040, AF),
        // 0x8000 - 0x3f = 0x7fc1: signed overflow (negative minus positive gives positive), AF;
        // PF of 0xc1 (three ones) is clear.
        (0x8000, OF | AF),
    ] {
        let cell = fp(0, 0x037f);
        let mut state = state(CR0_BASE, 2 | CF | OF | ZF);
        state.gpr[Register::Rax as usize] = 0xa5a5_a5a5_a5a5_0000 | u64::from(ax);
        let mut memory = Memory::new(&[0x66, 0x83, 0xf8, 0x3f]);
        let before = state.gpr;
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 1));
        assert_eq!(state.rflags & ARITHMETIC, flags, "ax {ax:#x}");
        assert_eq!(state.gpr, before, "CMP writes no register");
        assert_eq!(state.rip, CODE + 4);
    }
}
