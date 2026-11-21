//! S7-31i: `LDMXCSR m32` (`0F AE /2`, memory operand) in the FP member of the CPL0 profile set. The
//! kernel runs `0F AE 55 EC` (`ldmxcsr [rbp-0x14]`) in `kernel_fpu_begin_mask` (ISSUE-636).
//!
//! Semantics from the Intel SDM Vol. 2A, LDMXCSR: MXCSR is loaded from the 32-bit operand;
//! #UD if CR0.EM = 1 or CR4.OSFXSR = 0 (and on a CPU without SSE); #NM if CR0.TS = 1; #GP(0) if a
//! reserved MXCSR bit is set (here: outside `TARGET_FXSAVE_MXCSR_MASK` = 0xFFBF); the memory
//! faults of the 32-bit read. No flag or register but RIP changes.

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
const CR4_OSFXSR: u64 = 1 << 9;
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
    /// A read at this address is a page fault.
    fault_read_at: Option<u64>,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            data: vec![0xee; 0x100],
            writes: Vec::new(),
            fault_write_at: None,
            fault_read_at: None,
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
        if self.fault_read_at == Some(address) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(address),
            }));
        }
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
    run_with(
        CpuidProfile::X8664V1Stage4AExt2,
        state,
        memory,
        cell,
        budget,
    )
}

fn run_with(
    profile: CpuidProfile,
    state: &mut LongModeState,
    memory: &mut Memory,
    cell: &RefCell<GuestFpState>,
    budget: u64,
) -> (RunExit, u64, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&profile)
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

const LDMXCSR_RBP_M14: [u8; 4] = [0x0f, 0xae, 0x55, 0xec];

/// A state with CR4.OSFXSR set, RBP = DATA + 0x14 and the dword at DATA = `value`.
fn setup(value: u32) -> (LongModeState, Memory) {
    let mut state = state(CR0_BASE, 2 | CF | ZF | (1 << 9));
    state.cr4 = Some(CR4_BASE | CR4_OSFXSR);
    state.gpr[Register::Rbp as usize] = DATA + 0x14;
    let mut memory = Memory::new(&LDMXCSR_RBP_M14);
    memory.data[..4].copy_from_slice(&value.to_le_bytes());
    (state, memory)
}

fn mxcsr_cell(mxcsr: u32) -> RefCell<GuestFpState> {
    let cell = fp(0, 0x037f);
    cell.borrow_mut().sse.mxcsr = mxcsr;
    cell
}

#[test]
fn ldmxcsr_rbp_minus_0x14_loads_mxcsr_and_changes_nothing_else() {
    for value in [0x1f80u32, 0x1fa0, 0x0000, 0xffbf, 0x3f80] {
        let cell = mxcsr_cell(0x1f80 ^ 0x0aa0);
        let before_fp = cell.borrow().clone();
        let (mut state, mut memory) = setup(value);
        let before = (state.gpr, state.rflags);
        let (exit, retired, defined) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!(
            (exit, retired),
            (RunExit::InstructionBudgetExhausted, 1),
            "{value:#x}"
        );
        assert_eq!(cell.borrow().sse.mxcsr, value, "{value:#x}");
        // Only MXCSR of the FP state changed.
        let mut expected = before_fp.clone();
        expected.sse.mxcsr = value;
        assert_eq!(*cell.borrow(), expected);
        assert_eq!(state.rip, CODE + 4);
        assert_eq!((state.gpr, state.rflags), before);
        assert_eq!(defined, od_isa::INTEGER_FLAGS);
        assert!(memory.writes.is_empty(), "LDMXCSR does not write memory");
    }
}

#[test]
fn ldmxcsr_accepts_other_addressing_forms() {
    // RBX = DATA, RCX = 3, RSP = DATA.
    let forms: [(&[u8], usize); 5] = [
        (&[0x0f, 0xae, 0x13], 0x00),                         // [rbx]
        (&[0x0f, 0xae, 0x54, 0x8b, 0x10], 0x1c),             // [rbx+rcx*4+0x10]
        (&[0x0f, 0xae, 0x93, 0x40, 0x00, 0x00, 0x00], 0x40), // [rbx+0x40]
        (&[0x0f, 0xae, 0x54, 0x24, 0x20], 0x20),             // [rsp+0x20]
        (&[0x0f, 0xae, 0x15, 0xf9, 0x0f, 0x00, 0x00], 0x00), // [rip+0xffa] = DATA
    ];
    for (code, offset) in forms {
        let cell = mxcsr_cell(0x1f80);
        let mut state = state(CR0_BASE, 2);
        state.cr4 = Some(CR4_BASE | CR4_OSFXSR);
        let mut memory = Memory::new(code);
        memory.data[offset..offset + 4].copy_from_slice(&0x1fa0u32.to_le_bytes());
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!(
            (exit, retired),
            (RunExit::InstructionBudgetExhausted, 1),
            "{code:02x?}"
        );
        assert_eq!(cell.borrow().sse.mxcsr, 0x1fa0, "{code:02x?}");
        assert_eq!(state.rip, CODE + code.len() as u64, "{code:02x?}");
    }
}

#[test]
fn ldmxcsr_raises_nm_when_cr0_ts_is_set_before_any_read() {
    let cell = mxcsr_cell(0x1f80);
    let (mut state, mut memory) = setup(0x1fa0);
    state.cr0 = Some(CR0_BASE | CR0_TS);
    let before = state.clone();
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!((exit, retired), (nm(), 0));
    assert_eq!(state, before);
    assert_eq!(cell.borrow().sse.mxcsr, 0x1f80);
}

#[test]
fn ldmxcsr_reserved_mxcsr_bits_raise_gp0_and_leave_mxcsr() {
    // Bit 6 (DAZ, outside the target mask) and the upper half are reserved.
    for value in [0x1fc0u32, 0x0001_1f80, 0xffff_ffff, 0x8000_0000] {
        let cell = mxcsr_cell(0x1f80);
        let (mut state, mut memory) = setup(value);
        let before = state.clone();
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert_eq!(
            (exit, retired),
            (
                RunExit::Fault(ArchitecturalFault {
                    vector: 13,
                    error_code: Some(0),
                    linear_address: None,
                }),
                0
            ),
            "{value:#x}"
        );
        assert_eq!(state, before, "{value:#x}");
        assert_eq!(cell.borrow().sse.mxcsr, 0x1f80, "{value:#x}");
    }
}

#[test]
fn ldmxcsr_is_ud_with_cr0_em_or_without_cr4_osfxsr_or_without_declared_sse() {
    let cell = mxcsr_cell(0x1f80);
    // CR0.EM = 1.
    let (mut state, mut memory) = setup(0x1fa0);
    state.cr0 = Some(CR0_BASE | CR0_EM);
    let before = state.clone();
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!((exit, retired), (RunExit::Unsupported { opcode: 0xae }, 0));
    assert_eq!(state, before);
    // EM and TS together: #UD has priority over #NM.
    let (mut state, mut memory) = setup(0x1fa0);
    state.cr0 = Some(CR0_BASE | CR0_EM | CR0_TS);
    let (exit, _, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!(exit, RunExit::Unsupported { opcode: 0xae });
    // CR4.OSFXSR = 0.
    let (mut state, mut memory) = setup(0x1fa0);
    state.cr4 = Some(CR4_BASE);
    let before = state.clone();
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!((exit, retired), (RunExit::Unsupported { opcode: 0xae }, 0));
    assert_eq!(state, before);
    // A CPUID profile without SSE (Ext1, Stage4A): the form stays Unsupported.
    for profile in [
        CpuidProfile::X8664V1Stage4A,
        CpuidProfile::X8664V1Stage4AExt1,
    ] {
        let (mut state, mut memory) = setup(0x1fa0);
        let before = state.clone();
        let (exit, retired, _) = run_with(profile, &mut state, &mut memory, &cell, 1);
        assert_eq!(
            (exit, retired),
            (RunExit::Unsupported { opcode: 0xae }, 0),
            "{profile:?}"
        );
        assert_eq!(state, before, "{profile:?}");
        assert_eq!(cell.borrow().sse.mxcsr, 0x1f80);
    }
    // The decoder refuses prefixes and the register form.
    for code in [
        &[0x66, 0x0f, 0xae, 0x55, 0xec][..],
        &[0x48, 0x0f, 0xae, 0x55, 0xec][..],
        &[0x0f, 0xae, 0xd0][..],
    ] {
        let (mut state, mut memory) = setup(0x1fa0);
        memory.code = code.to_vec();
        let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
        assert!(
            matches!(exit, RunExit::Unsupported { .. }),
            "{code:02x?}: {exit:?}"
        );
        assert_eq!(retired, 0);
    }
}

#[test]
fn ldmxcsr_page_fault_on_the_read_leaves_the_state_alone() {
    let cell = mxcsr_cell(0x1f80);
    let (mut state, mut memory) = setup(0x1fa0);
    // The operand lies on an unmapped page: #PF (not present, read, supervisor), no retirement.
    memory.fault_read_at = Some(DATA);
    let before = state.clone();
    let (exit, retired, _) = run_fp(&mut state, &mut memory, &cell, 1);
    assert_eq!(retired, 0);
    assert_eq!(
        exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(state, before);
    assert_eq!(cell.borrow().sse.mxcsr, 0x1f80);
}

/// S7-31k: the contract says the FP member runs at CPL0 only; the step has no privilege check of
/// its own, so the CPL0 set must refuse the access context of ring 3 before reaching it. LDMXCSR is
/// not privileged in the SDM (it is valid at CPL3 under the CR0/CR4 conditions), so this refusal
/// is the set's scope, not an architectural rule.
#[test]
fn ldmxcsr_in_the_cpl0_set_does_not_run_under_a_ring3_context() {
    let cell = mxcsr_cell(0x1f80);
    let (mut state, mut memory) = setup(0x1fa0);
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4AExt2)
        .with_fp_state(&cell);
    let mut ring3 = config(1);
    ring3.context.privilege = PrivilegeLevel::Ring3;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        ring3,
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    );
    let error = result.err().expect("the CPL0 set refuses a ring 3 context");
    assert_eq!(format!("{error:?}"), "WrongPrivilege");
    // Nothing ran: no retirement, RIP, MXCSR and memory untouched.
    assert_eq!(state.rip, CODE);
    assert_eq!(cell.borrow().sse.mxcsr, 0x1f80);
    assert!(memory.writes.is_empty());
}
