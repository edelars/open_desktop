//! S7-32 slice S: `DF E0` FNSTSW AX in the CPL0 set with the FP member (the x87/SIMD exception path
//! of the Windows kernel, `ntkrnlmp` + `0x6be08b`: `fnstcw [rbp+0xe0]; fnstsw ax`). Intel SDM
//! 325462-093US Vol. 2A "FSTSW/FNSTSW": `AX := FPUStatusWord` without checking for pending unmasked
//! exceptions; #NM when CR0.EM or CR0.TS is 1. A control instruction: no flag, no other register,
//! and not the x87 instruction/data pointers or the opcode register change (Vol. 1 §8.1.8). The
//! FP state is seeded directly (not by x87 loads).

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig, RunExit,
    run as bare_run, run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};
use std::cell::RefCell;

const CODE: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x620;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;

struct Memory(Vec<u8>);

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.wrapping_sub(CODE) as usize;
        self.0.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
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

fn state(cr0: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2 | 1 | (1 << 6),
        cr0: Some(cr0),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
    state
}

/// The seeded state: TOP = 6, C3 C2 C1 C0 = 1, ES and B = 1, and a precision exception flag.
fn seeded() -> GuestFpState {
    let mut fp = GuestFpState::initial();
    fp.x87.status_word =
        (1 << 15) | (1 << 14) | (6 << 11) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 7) | (1 << 5);
    fp.x87.tag_word = 0x0fff;
    fp.x87.registers[6] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    fp.x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40];
    fp
}

fn run(
    state: &mut LongModeState,
    code: &[u8],
    cell: Option<&RefCell<GuestFpState>>,
) -> (RunExit, u64, u64) {
    let mut set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A);
    if let Some(cell) = cell {
        set = set.with_fp_state(cell);
    }
    let defined = od_isa::INTEGER_FLAGS & !(1 << 4);
    let result = run_with_cpl0_profile_set(
        state,
        &mut Memory(code.to_vec()),
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
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

#[test]
fn fnstsw_ax_stores_the_status_word_of_the_fp_member() {
    for fp in [seeded(), GuestFpState::initial()] {
        let cell = RefCell::new(fp.clone());
        let mut state = state(CR0_BASE);
        let before = state.clone();
        let (exit, retired, defined) = run(&mut state, &[0xdf, 0xe0], Some(&cell));
        assert_eq!((exit, retired), (RunExit::InstructionBudgetExhausted, 1));
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0x1122_3344_5566_0000 | u64::from(fp.x87.status_word),
            "AX is the status word, RAX[63:16] stay"
        );
        assert_eq!(state.rip, CODE + 2);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(defined, od_isa::INTEGER_FLAGS & !(1 << 4));
        let mut rest = state.clone();
        rest.gpr[Register::Rax as usize] = before.gpr[Register::Rax as usize];
        rest.rip = before.rip;
        assert_eq!(rest, before, "nothing else changed");
        assert_eq!(*cell.borrow(), fp, "the FP state is unchanged");
    }
}

#[test]
fn fnstsw_ax_is_a_device_not_available_fault_with_em_or_ts() {
    for cr0 in [CR0_BASE | CR0_TS, CR0_BASE | CR0_EM] {
        let cell = RefCell::new(seeded());
        let mut state = state(cr0);
        let before = state.clone();
        let (exit, retired, _) = run(&mut state, &[0xdf, 0xe0], Some(&cell));
        assert_eq!(
            exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 7,
                error_code: None,
                linear_address: None,
            }),
            "cr0 {cr0:#x}"
        );
        assert_eq!(retired, 0);
        assert_eq!(state, before);
        assert_eq!(*cell.borrow(), seeded());
    }
}

#[test]
fn fnstsw_ax_outside_its_exact_form_or_member_stays_unsupported() {
    let cell = RefCell::new(seeded());
    for code in [
        &[0x48, 0xdf, 0xe0][..],
        &[0x66, 0xdf, 0xe0],
        &[0x41, 0xdf, 0xe0],
    ] {
        let mut state = state(CR0_BASE);
        let before = state.clone();
        let (exit, retired, _) = run(&mut state, code, Some(&cell));
        assert!(
            matches!(exit, RunExit::Unsupported { .. }),
            "{code:02x?}: {exit:?}"
        );
        assert_eq!(retired, 0);
        assert_eq!(state, before);
    }
    let mut state = state(CR0_BASE);
    let (exit, _, _) = run(&mut state, &[0xdf, 0xe0], None);
    assert_eq!(exit, RunExit::Unsupported { opcode: 0xdf }, "no FP member");
}

/// Outside the set (the bare step, x87 state in `state.x87`) FNSTSW AX now runs also while the FCS/FDS
/// selectors are represented: it is exempt from the bounded profile's refusal of x87 forms that would
/// update the pointers (S7-32 slice S; Vol. 1 §8.1.8). Pointers, selectors and the rest stay.
#[test]
fn the_bare_step_runs_fnstsw_ax_with_represented_selectors() {
    let mut x87 = seeded().x87;
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    x87.instruction_pointer = 0x1111;
    x87.data_pointer = 0x2222;
    x87.last_opcode = 0x155;
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2,
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        x87: Some(x87.clone()),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
    let result = bare_run(
        &mut state,
        &mut Memory(vec![0xdf, 0xe0]),
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(
        state.gpr[Register::Rax as usize],
        0x1122_3344_5566_0000 | u64::from(x87.status_word)
    );
    assert_eq!(state.rip, CODE + 2);
    assert_eq!(
        state.x87,
        Some(x87),
        "selectors, pointers and the opcode stay"
    );
}
