use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, RdtscControls, RunConfig, RunExit, run, run_with_cli_profile,
    run_with_cpl0_profile_set, run_with_cpuid_profile, run_with_efer_rdmsr_profile,
    run_with_halt_profile, run_with_msr_profile, run_with_rdtsc_source,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile, CpuActivity,
    GuestMemory, HaltProfile, InstructionMode, LongModeState, MemoryError, OperandSize,
    PrivilegeLevel, StiShadow, cpuid::CpuidProfile, msr::MsrProfile,
};

const RIP: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const IF: u64 = 1 << 9;
/// `cld; cli; xor eax,eax; mov ds,ax; mov es,ax; mov ss,ax`.
const BOARD_PROLOGUE: [u8; 10] = [0xfc, 0xfa, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0];

#[derive(Clone)]
struct Memory {
    bytes: Vec<u8>,
    fetches: usize,
}

impl Memory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            fetches: 0,
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.fetches += 1;
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.bytes
            .get(offset)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "unexpected read {address:#x}"
        )))
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "unexpected write {address:#x}"
        )))
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

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn config(privilege: PrivilegeLevel, budget: u64) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: context(privilege),
    }
}

fn board_state() -> LongModeState {
    let mut state = LongModeState {
        rip: RIP,
        rflags: 2 | IF,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    };
    state.gpr[0] = 0x1111_2222_3333_4444;
    state
}

fn run_set(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
}

fn exit(result: &Cpl0ProfileSetResult) -> &RunExit {
    let Cpl0ProfileSetExit::Run(exit) = &result.exit;
    exit
}

#[test]
fn board_prologue_with_cli_stops_at_mov_ss_after_five_instructions() {
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    let mut state = board_state();
    let mut memory = Memory::code(&BOARD_PROLOGUE);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
    assert_eq!(result.instructions_executed, 5);
    // XOR leaves AF undefined; the definedness mask is threaded through.
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !(1 << 4));
    assert_eq!(state.rip, RIP + 8);
    assert_eq!(state.rflags & IF, 0);
    assert_eq!(state.cr0, Some(CR0_BASE));
    assert_eq!(state.cr4, Some(CR4_BASE));
    assert_eq!((state.ds_selector, state.es_selector), (0, 0));
    assert_eq!(state.gpr[0], 0);
}

#[test]
fn board_prologue_without_cli_stops_at_cli() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = board_state();
    let mut memory = Memory::code(&BOARD_PROLOGUE);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xfa });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 1);
    assert_eq!(state.rflags & IF, IF);
}

#[test]
fn cli_keeps_iopl_and_other_flags_and_definedness() {
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    let mut state = board_state();
    state.rflags = 2 | IF | (3 << 12) | (1 << 10) | 1 | (1 << 6);
    let before = state.rflags;
    let defined = od_isa::INTEGER_FLAGS & !(1 << 4);
    let mut memory = Memory::code(&[0xfa]);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        defined,
        &set,
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    assert_eq!(state.rflags, before & !IF);
}

#[test]
fn lock_cli_is_ud_and_unadmitted_forms_leave_state_untouched() {
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    let ud = RunExit::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    });
    let cases: [(&[u8], RunExit); 3] = [
        (&[0xf0, 0xfa], ud),
        (
            &[0x66, 0x0f, 0x57, 0xc1],
            RunExit::Unsupported { opcode: 0x57 },
        ),
        (&[0xdb, 0xe3], RunExit::Unsupported { opcode: 0xdb }),
    ];
    for (bytes, expected) in cases {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        assert_eq!(exit(&result), &expected, "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn rdtsc_sti_and_hlt_stay_closed() {
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    let mut source = || 0x1_2345_6789u64;
    for (bytes, opcode) in [
        (&[0x0f, 0x31][..], 0x31),
        (&[0xfb][..], 0xfb),
        (&[0xf4][..], 0xf4),
    ] {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            od_isa::INTEGER_FLAGS,
            &set,
            Some(&mut source),
            || false,
        )
        .unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(state, before);
    }
}

#[test]
fn admission_rejects_before_any_fetch_and_without_mutation() {
    let cli = CliProfile::NonPviLong64V1;
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let with_cli = plain.with_cli(&cli);
    type Mutate = fn(&mut LongModeState);
    let cases: Vec<(&str, Mutate, Cpl0ProfileSetInputError)> = vec![
        (
            "cr0 outside policy",
            |s| s.cr0 = Some(0x8001_0031),
            Cpl0ProfileSetInputError::Cr0OutsidePolicy { value: 0x8001_0031 },
        ),
        (
            "cr4 outside policy",
            |s| s.cr4 = Some(0xa0),
            Cpl0ProfileSetInputError::Cr4OutsidePolicy { value: 0xa0 },
        ),
        (
            "cr0 missing",
            |s| s.cr0 = None,
            Cpl0ProfileSetInputError::Cr0Missing,
        ),
        (
            "cr4 missing",
            |s| s.cr4 = None,
            Cpl0ProfileSetInputError::Cr4Missing,
        ),
        (
            "cr3 modeled",
            |s| s.cr3 = Some(0x1000),
            Cpl0ProfileSetInputError::Cr3NotAdmitted,
        ),
        (
            "x87 modeled",
            |s| s.x87 = Some(od_isa::state::X87State::fixture_seed()),
            Cpl0ProfileSetInputError::FpStateNotAdmitted,
        ),
        (
            "sse modeled",
            |s| {
                s.sse = Some(od_isa::state::SseState {
                    xmm: [[0; 16]; 16],
                    mxcsr: 0x1f80,
                })
            },
            Cpl0ProfileSetInputError::FpStateNotAdmitted,
        ),
        (
            "activity missing",
            |s| s.activity = None,
            Cpl0ProfileSetInputError::ActivityMissing,
        ),
        (
            "halted",
            |s| s.activity = Some(CpuActivity::Halted),
            Cpl0ProfileSetInputError::NotRunning,
        ),
        (
            "shadow missing",
            |s| s.sti_shadow = None,
            Cpl0ProfileSetInputError::StiShadowMissing,
        ),
        (
            "shadow active",
            |s| s.sti_shadow = Some(StiShadow::Blocking),
            Cpl0ProfileSetInputError::StiShadowActive,
        ),
    ];
    for (name, mutate, expected) in cases {
        let mut state = board_state();
        mutate(&mut state);
        let before = state.clone();
        let mut memory = Memory::code(&BOARD_PROLOGUE);
        let err = run_set(&mut state, &mut memory, &with_cli, 64).unwrap_err();
        assert_eq!(err, expected, "{name}");
        assert_eq!(memory.fetches, 0, "{name}");
        assert_eq!(state, before, "{name}");
        let mut memory = Memory::code(&BOARD_PROLOGUE);
        let err = run_set(&mut state, &mut memory, &plain, 64).unwrap_err();
        assert_eq!(err, expected, "{name} without CLI");
        assert_eq!(memory.fetches, 0, "{name} without CLI");
    }
    // RFLAGS.VM is a CLI precondition only; PE/PVI are covered by the policy.
    let mut vm = board_state();
    vm.rflags |= 1 << 17;
    let mut memory = Memory::code(&BOARD_PROLOGUE);
    let err = run_set(&mut vm, &mut memory, &with_cli, 64).unwrap_err();
    assert_eq!(err, Cpl0ProfileSetInputError::CliPreconditionFailed);
    assert_eq!(memory.fetches, 0);
    let mut memory = Memory::code(&BOARD_PROLOGUE);
    let result = run_set(&mut vm, &mut memory, &plain, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xfa });
}

#[test]
fn admission_rejects_wrong_privilege_and_rflags_definedness() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for privilege in [
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        let mut state = board_state();
        let mut memory = Memory::code(&[0x90]);
        let err = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(privilege, 1),
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap_err();
        assert_eq!(err, Cpl0ProfileSetInputError::WrongPrivilege);
        assert_eq!(memory.fetches, 0);
    }
    let mut state = board_state();
    let mut memory = Memory::code(&[0x90]);
    let err = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS | (1 << 20),
        &set,
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        Cpl0ProfileSetInputError::InvalidRflagsDefined {
            value: od_isa::INTEGER_FLAGS | (1 << 20)
        }
    );
    assert_eq!(memory.fetches, 0);
}

#[test]
fn legacy_entries_keep_their_behavior_with_modeled_control_registers() {
    // `run` and the single-profile CLI entry still refuse modeled CR0/CR4.
    let mut state = board_state();
    let mut memory = Memory::code(&[0xfa]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let mut memory = Memory::code(&[0xfa]);
    let result = run_with_cli_profile(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        &cli,
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
}

#[test]
fn legacy_run_without_control_registers_still_rejects_cli() {
    // Without modeled CR0/CR4 `run` reaches the opcode decoder, where CLI has
    // no profile and is reported by its own opcode byte.
    let mut state = board_state();
    state.cr0 = None;
    state.cr4 = None;
    let before = state.clone();
    let mut memory = Memory::code(&[0xfa]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xfa });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn policy_values_beyond_base_admit_but_keep_fp_and_sse_closed() {
    // CR0 EM/TS (mask 0xc) and CR4 OSFXSR/OSXMMEXCPT (mask 0x600) are inside
    // the policy but differ from the BASE values used by the other tests.
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    for (cr0, cr4) in [
        (CR0_BASE | 0xc, CR4_BASE | 0x600),
        (CR0_BASE | 0x4, CR4_BASE | 0x600),
        (CR0_BASE | 0xc, CR4_BASE),
    ] {
        for (bytes, opcode) in [
            (&[0x66, 0x0f, 0x57, 0xc1][..], 0x57),
            (&[0xdb, 0xe3][..], 0xdb),
        ] {
            let mut state = board_state();
            state.cr0 = Some(cr0);
            state.cr4 = Some(cr4);
            let before = state.clone();
            let mut memory = Memory::code(bytes);
            let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
            let label = format!("cr0={cr0:#x} cr4={cr4:#x} {bytes:02x?}");
            assert_eq!(exit(&result), &RunExit::Unsupported { opcode }, "{label}");
            assert_eq!(result.instructions_executed, 0, "{label}");
            assert_eq!(state, before, "{label}");
            assert_eq!(memory.bytes, bytes, "{label}");
        }
    }
}

/// CF, PF, AF, ZF, SF and OF: the flags MOV CR leaves undefined.
const ARITHMETIC: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x800;
const MOV_RAX_CR0: [u8; 3] = [0x0f, 0x20, 0xc0];
const MOV_CR0_RAX: [u8; 3] = [0x0f, 0x22, 0xc0];
const MOV_RAX_CR4: [u8; 3] = [0x0f, 0x20, 0xe0];
const MOV_CR4_RAX: [u8; 3] = [0x0f, 0x22, 0xe0];

/// `movabs rax, value`.
fn movabs_rax(value: u64) -> Vec<u8> {
    let mut bytes = vec![0x48, 0xb8];
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

fn cr_set() -> Cpl0ProfileSet<'static> {
    static CLI: CliProfile = CliProfile::NonPviLong64V1;
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&CLI)
}

#[test]
fn cr0_cr4_read_modify_write_sequence_retires_every_instruction() {
    let mut code = Vec::new();
    code.extend_from_slice(&MOV_RAX_CR4);
    code.extend_from_slice(&[0x48, 0x0d, 0x00, 0x06, 0x00, 0x00]); // or rax,0x600
    code.extend_from_slice(&MOV_CR4_RAX);
    code.extend_from_slice(&MOV_RAX_CR0);
    code.extend_from_slice(&[0x48, 0x83, 0xc8, 0x0c]); // or rax,0xc
    code.extend_from_slice(&MOV_CR0_RAX);
    code.extend_from_slice(&[0x48, 0x83, 0xe0, 0xf3]); // and rax,~0xc
    code.extend_from_slice(&MOV_CR0_RAX);
    let set = cr_set();
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 8);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.cr4, Some(CR4_BASE | 0x600));
    assert_eq!(state.cr4, Some(0x620));
    assert_eq!(state.cr0, Some(CR0_BASE));
    assert_eq!(state.gpr[0], CR0_BASE);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
}

#[test]
fn cr_reads_return_current_values_and_clear_arithmetic_definedness() {
    let set = cr_set();
    for (bytes, expected) in [(MOV_RAX_CR0, CR0_BASE), (MOV_RAX_CR4, CR4_BASE)] {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(&bytes);
        let result = run_set(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
        assert_eq!(state.gpr[0], expected);
        assert_eq!(state.rip, RIP + 3);
        assert_eq!((state.cr0, state.cr4), (before.cr0, before.cr4));
        assert_eq!(state.rflags, before.rflags);
    }
}

#[test]
fn cr4_write_outside_policy_is_unsupported_without_mutation() {
    let set = cr_set();
    // PGE, OSXSAVE and a reserved bit are outside the CR4 policy; PAE cleared
    // is outside as well.
    for value in [0xa0u64, CR4_BASE | 0x2_0000, CR4_BASE | (1 << 40), 0] {
        let mut code = movabs_rax(value);
        code.extend_from_slice(&MOV_CR4_RAX);
        let mut state = board_state();
        let mut memory = Memory::code(&code);
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        let label = format!("cr4 value {value:#x}");
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x22 },
            "{label}"
        );
        // Only the MOV RAX,imm retired.
        assert_eq!(result.instructions_executed, 1, "{label}");
        assert_eq!(state.rip, RIP + 10, "{label}");
        assert_eq!(state.cr4, Some(CR4_BASE), "{label}");
        assert_eq!(state.cr0, Some(CR0_BASE), "{label}");
        assert_eq!(state.gpr[0], value, "{label}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{label}");
    }
}

#[test]
fn cr0_write_outside_policy_is_unsupported_without_mutation() {
    let set = cr_set();
    for value in [CR0_BASE & !1, CR0_BASE & !0x8000_0000, 0] {
        let mut code = movabs_rax(value);
        code.extend_from_slice(&MOV_CR0_RAX);
        let mut state = board_state();
        let mut memory = Memory::code(&code);
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        let label = format!("cr0 value {value:#x}");
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x22 },
            "{label}"
        );
        assert_eq!(result.instructions_executed, 1, "{label}");
        assert_eq!(state.rip, RIP + 10, "{label}");
        assert_eq!(state.cr0, Some(CR0_BASE), "{label}");
        assert_eq!(state.cr4, Some(CR4_BASE), "{label}");
        assert_eq!(state.gpr[0], value, "{label}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{label}");
    }
}

#[test]
fn lock_mov_cr_is_ud_without_mutation() {
    let set = cr_set();
    let ud = RunExit::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    });
    for bytes in [
        &[0xf0, 0x0f, 0x22, 0xc0][..],
        &[0xf0, 0x0f, 0x22, 0xe0][..],
        &[0xf0, 0x0f, 0x20, 0xc0][..],
        &[0xf0, 0x0f, 0x20, 0xe0][..],
    ] {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        assert_eq!(exit(&result), &ud, "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn cr_moves_stay_closed_without_a_policy_entry() {
    // `run` refuses modeled control registers before decoding; with CR0/CR4
    // not modeled it reaches the decoder and CR moves have no profile.
    for bytes in [MOV_RAX_CR0, MOV_CR0_RAX, MOV_RAX_CR4, MOV_CR4_RAX] {
        let mut state = board_state();
        state.cr0 = None;
        state.cr4 = None;
        let before = state.clone();
        let mut memory = Memory::code(&bytes);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: bytes[1] });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn em_ts_set_through_mov_cr0_keeps_fninit_closed() {
    let mut code = Vec::new();
    code.extend_from_slice(&MOV_RAX_CR0);
    code.extend_from_slice(&[0x48, 0x83, 0xc8, 0x0c]); // or rax,0xc
    code.extend_from_slice(&MOV_CR0_RAX);
    code.extend_from_slice(&[0xdb, 0xe3]); // fninit
    let set = cr_set();
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xdb });
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + 10);
    assert_eq!(state.cr0, Some(CR0_BASE | 0xc));
    assert_eq!(state.cr4, Some(CR4_BASE));
    assert_eq!(state.gpr[0], CR0_BASE | 0xc);
    assert!(state.x87.is_none());
}

/// Same input as `board_state` but without the control-register, activity and
/// STI-shadow modeling that the set requires; what the legacy entries accept.
fn legacy_state() -> LongModeState {
    LongModeState {
        cr0: None,
        cr4: None,
        activity: None,
        sti_shadow: None,
        ..board_state()
    }
}

fn run_set_with_source(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
    source: Option<&mut dyn FnMut() -> u64>,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        source,
        || false,
    )
}

fn base() -> Cpl0ProfileSet<'static> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
}

#[test]
fn cpuid_leaves_match_the_legacy_entry() {
    let cpuid = CpuidProfile::DiagnosticV1;
    let set = base().with_cpuid(&cpuid);
    // `xor eax,eax; cpuid`, `mov eax,1; cpuid` and an unknown leaf.
    let programs: [&[u8]; 3] = [
        &[0x31, 0xc0, 0x0f, 0xa2],
        &[0xb8, 1, 0, 0, 0, 0x0f, 0xa2],
        &[0xb8, 0x34, 0x12, 0, 0, 0x0f, 0xa2],
    ];
    for bytes in programs {
        let mut set_state = board_state();
        let mut set_memory = Memory::code(bytes);
        let set_result = run_set(&mut set_state, &mut set_memory, &set, 2).unwrap();
        let mut old_state = legacy_state();
        let mut old_memory = Memory::code(bytes);
        let old_result = run_with_cpuid_profile(
            &mut old_state,
            &mut old_memory,
            config(PrivilegeLevel::Ring0, 2),
            &cpuid,
            || false,
        );
        assert_eq!(exit(&set_result), &old_result.exit, "{bytes:02x?}");
        assert_eq!(
            set_result.instructions_executed, old_result.instructions_executed,
            "{bytes:02x?}"
        );
        assert_eq!(
            set_result.rflags_defined, old_result.rflags_defined,
            "{bytes:02x?}"
        );
        assert_eq!(set_state.gpr, old_state.gpr, "{bytes:02x?}");
        assert_eq!(set_state.rip, old_state.rip, "{bytes:02x?}");
        assert_eq!(set_state.rflags, old_state.rflags, "{bytes:02x?}");
        assert_eq!(set_state.cr0, Some(CR0_BASE));
        assert_eq!(set_state.cr4, Some(CR4_BASE));
    }
    // Leaf 0: the diagnostic vendor identity in EBX, EDX, ECX order.
    let mut state = board_state();
    let mut memory = Memory::code(&[0x31, 0xc0, 0x0f, 0xa2]);
    run_set(&mut state, &mut memory, &set, 2).unwrap();
    assert_eq!(state.gpr[0], 1);
    let mut vendor = Vec::new();
    for register in [3, 2, 1] {
        vendor.extend_from_slice(&(state.gpr[register] as u32).to_le_bytes());
    }
    assert_eq!(vendor, b"OpenDesktop ");
}

#[test]
fn cpuid_stays_closed_without_its_builder() {
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0xa2]);
    let result = run_set(&mut state, &mut memory, &base(), 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xa2 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn rdtsc_splits_the_injected_source_into_edx_eax() {
    let set = base().with_rdtsc();
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        0x1_2345_6789u64
    };
    let mut state = board_state();
    state.gpr[2] = u64::MAX;
    let mut memory = Memory::code(&[0x0f, 0x31]);
    let result = run_set_with_source(&mut state, &mut memory, &set, 1, Some(&mut source)).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 0x2345_6789);
    assert_eq!(state.gpr[2], 1);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(calls, 1);

    // The legacy RDTSC entry with TSD clear gives the same registers.
    let mut old_state = legacy_state();
    let mut old_memory = Memory::code(&[0x0f, 0x31]);
    let mut old_source = || 0x1_2345_6789u64;
    let old = run_with_rdtsc_source(
        &mut old_state,
        &mut old_memory,
        config(PrivilegeLevel::Ring0, 1),
        RdtscControls { cr4_tsd: false },
        &mut old_source,
        || false,
    );
    assert_eq!(&old.exit, exit(&result));
    assert_eq!(old_state.gpr, state.gpr);
    assert_eq!(old_state.rip, state.rip);
}

#[test]
fn rdtsc_stays_closed_without_its_builder_even_with_a_source() {
    let cpuid = CpuidProfile::DiagnosticV1;
    let set = base().with_cpuid(&cpuid);
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        0x1_2345_6789u64
    };
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x31]);
    let result = run_set_with_source(&mut state, &mut memory, &set, 4, Some(&mut source)).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x31 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(calls, 0);
}

#[test]
fn rdtsc_without_a_source_is_unsupported_and_changes_nothing() {
    let set = base().with_rdtsc();
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x31]);
    let result = run_set(&mut state, &mut memory, &set, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x31 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn rdtsc_ignores_prefixed_forms_like_the_legacy_entry() {
    // `0x66 0x0f 0x31` is not the exact bare form; both entries agree.
    let set = base().with_rdtsc();
    let mut source = || 7u64;
    let mut state = board_state();
    let mut memory = Memory::code(&[0x66, 0x0f, 0x31]);
    let result = run_set_with_source(&mut state, &mut memory, &set, 1, Some(&mut source)).unwrap();
    let mut old_state = legacy_state();
    let mut old_memory = Memory::code(&[0x66, 0x0f, 0x31]);
    let mut old_source = || 7u64;
    let old = run_with_rdtsc_source(
        &mut old_state,
        &mut old_memory,
        config(PrivilegeLevel::Ring0, 1),
        RdtscControls { cr4_tsd: false },
        &mut old_source,
        || false,
    );
    assert_eq!(&old.exit, exit(&result));
    assert_eq!(old.instructions_executed, result.instructions_executed);
    assert_eq!(old_state.gpr, state.gpr);
}

#[test]
fn efer_rdmsr_returns_the_synthetic_value() {
    let msr = MsrProfile::EferReadOnlyV1(0xd00);
    let set = base().with_msr(&msr);
    // `mov ecx,0xc0000080; rdmsr`.
    let mut state = board_state();
    state.gpr[2] = u64::MAX;
    let mut memory = Memory::code(&[0xb9, 0x80, 0x00, 0x00, 0xc0, 0x0f, 0x32]);
    let result = run_set(&mut state, &mut memory, &set, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[0], 0xd00);
    assert_eq!(state.gpr[2], 0);
    assert_eq!(state.rip, RIP + 7);

    // Same bytes and register input as the dedicated EFER entry.
    let mut set_state = board_state();
    set_state.gpr[1] = 0xc000_0080;
    let mut set_memory = Memory::code(&[0x0f, 0x32]);
    let set_result = run_set(&mut set_state, &mut set_memory, &set, 1).unwrap();
    let mut old_state = board_state();
    old_state.gpr[1] = 0xc000_0080;
    let mut old_memory = Memory::code(&[0x0f, 0x32]);
    let old = run_with_efer_rdmsr_profile(
        &mut old_state,
        &mut old_memory,
        config(PrivilegeLevel::Ring0, 1),
        0xd00,
        || false,
    );
    assert_eq!(&old.exit, exit(&set_result));
    assert_eq!(old_state.gpr, set_state.gpr);
    assert_eq!(old_state.rip, set_state.rip);
}

#[test]
fn msr_set_holds_exactly_one_profile_and_rejects_other_indices() {
    let fs = MsrProfile::FsBaseV1;
    let set = base().with_msr(&fs);
    // GS_BASE is not handled by the FS_BASE profile.
    let mut state = board_state();
    state.gpr[1] = 0xc000_0101;
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x32]);
    let result = run_set(&mut state, &mut memory, &set, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x32 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    // EFER is not handled either, and RDMSR stays closed with no profile.
    let mut state = board_state();
    state.gpr[1] = 0xc000_0080;
    let mut memory = Memory::code(&[0x0f, 0x32]);
    let result = run_set(&mut state, &mut memory, &set, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x32 });
    let mut state = board_state();
    state.gpr[1] = 0xc000_0100;
    let mut memory = Memory::code(&[0x0f, 0x32]);
    let result = run_set(&mut state, &mut memory, &base(), 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x32 });
    assert_eq!(result.instructions_executed, 0);

    // A later with_msr replaces the earlier profile.
    let efer = MsrProfile::EferReadOnlyV1(0x500);
    let replaced = set.with_msr(&efer);
    let mut state = board_state();
    state.gpr[1] = 0xc000_0080;
    let mut memory = Memory::code(&[0x0f, 0x32]);
    run_set(&mut state, &mut memory, &replaced, 1).unwrap();
    assert_eq!(state.gpr[0], 0x500);
}

#[test]
fn fs_base_msr_read_and_write_match_the_legacy_entry() {
    let fs = MsrProfile::FsBaseV1;
    let set = base().with_msr(&fs);
    // `mov ecx,0xc0000100; rdmsr` and a WRMSR of a non-canonical value (#GP).
    let read: &[u8] = &[0xb9, 0x00, 0x01, 0x00, 0xc0, 0x0f, 0x32];
    let write: &[u8] = &[0xb9, 0x00, 0x01, 0x00, 0xc0, 0x0f, 0x30];
    for (bytes, edx_eax) in [(read, (0u64, 0u64)), (write, (0x8000_0000, 0))] {
        let mut set_state = board_state();
        set_state.fs_base = 0xffff_8000_dead_beef;
        set_state.gpr[0] = edx_eax.1;
        set_state.gpr[2] = edx_eax.0;
        let mut set_memory = Memory::code(bytes);
        let set_result = run_set(&mut set_state, &mut set_memory, &set, 2).unwrap();
        let mut old_state = legacy_state();
        old_state.fs_base = 0xffff_8000_dead_beef;
        old_state.gpr[0] = edx_eax.1;
        old_state.gpr[2] = edx_eax.0;
        let mut old_memory = Memory::code(bytes);
        let old = run_with_msr_profile(
            &mut old_state,
            &mut old_memory,
            config(PrivilegeLevel::Ring0, 2),
            &fs,
            || false,
        );
        assert_eq!(&old.exit, exit(&set_result), "{bytes:02x?}");
        assert_eq!(
            old.instructions_executed, set_result.instructions_executed,
            "{bytes:02x?}"
        );
        assert_eq!(old_state.gpr, set_state.gpr, "{bytes:02x?}");
        assert_eq!(old_state.fs_base, set_state.fs_base, "{bytes:02x?}");
        assert_eq!(old_state.rip, set_state.rip, "{bytes:02x?}");
    }
}

#[test]
fn cli_then_hlt_halts_after_two_retired_instructions() {
    let cli = CliProfile::NonPviLong64V1;
    let halt = HaltProfile::Long64V1;
    let set = base().with_cli(&cli).with_halt(&halt);
    let mut state = board_state();
    let mut memory = Memory::code(&[0xfa, 0xf4, 0x90]);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Halted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.activity, Some(CpuActivity::Halted));
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.rflags & IF, 0);
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    // The byte after HLT is never fetched.
    assert_eq!(memory.fetches, 2);

    // The same bytes under the legacy HLT entry end in the same state.
    let mut old_state = legacy_state();
    old_state.activity = Some(CpuActivity::Running);
    old_state.sti_shadow = Some(StiShadow::Clear);
    let mut old_memory = Memory::code(&[0xfa, 0xf4, 0x90]);
    let old = run_with_halt_profile(
        &mut old_state,
        &mut old_memory,
        config(PrivilegeLevel::Ring0, 64),
        &halt,
        || false,
    );
    assert_eq!(&old.exit, exit(&result));
    assert_eq!(old.instructions_executed, result.instructions_executed);
    assert_eq!(old_state.activity, state.activity);
    assert_eq!(old_state.rip, state.rip);
    assert_eq!(old_state.rflags, state.rflags);
}

#[test]
fn bare_hlt_without_cli_halts_with_interrupts_untouched() {
    let halt = HaltProfile::Long64V1;
    let set = base().with_halt(&halt);
    let mut state = board_state();
    let mut memory = Memory::code(&[0xf4]);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Halted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rflags & IF, IF);
    assert_eq!(state.rip, RIP + 1);
}

#[test]
fn hlt_stays_closed_without_its_builder_and_sti_stays_closed_with_it() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0xf4]);
    let result = run_set(&mut state, &mut memory, &base().with_cli(&cli), 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    assert_eq!(state, before);

    // STI is not part of the set even when HLT is.
    let halt = HaltProfile::Long64V1;
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0xfb]);
    let result = run_set(&mut state, &mut memory, &base().with_halt(&halt), 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xfb });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn halted_input_is_rejected_by_admission_unlike_the_legacy_entry() {
    let halt = HaltProfile::Long64V1;
    let set = base().with_halt(&halt);
    let mut state = board_state();
    state.activity = Some(CpuActivity::Halted);
    let before = state.clone();
    let mut memory = Memory::code(&[0xf4]);
    assert_eq!(
        run_set(&mut state, &mut memory, &set, 4).unwrap_err(),
        Cpl0ProfileSetInputError::NotRunning
    );
    assert_eq!(state, before);
    assert_eq!(memory.fetches, 0);

    // The legacy entry accepts a Halted state and reports Halted at once.
    let mut old_state = legacy_state();
    old_state.activity = Some(CpuActivity::Halted);
    old_state.sti_shadow = Some(StiShadow::Clear);
    let mut old_memory = Memory::code(&[0xf4]);
    let old = run_with_halt_profile(
        &mut old_state,
        &mut old_memory,
        config(PrivilegeLevel::Ring0, 4),
        &halt,
        || false,
    );
    assert_eq!(old.exit, RunExit::Halted);
    assert_eq!(old.instructions_executed, 0);
}

static ALL_CPUID: CpuidProfile = CpuidProfile::DiagnosticV1;
static ALL_MSR: MsrProfile = MsrProfile::EferReadOnlyV1(0xd00);
static ALL_HALT: HaltProfile = HaltProfile::Long64V1;
static ALL_CLI: CliProfile = CliProfile::NonPviLong64V1;
const FULL_SET: Cpl0ProfileSet<'static> =
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&ALL_CLI)
        .with_cpuid(&ALL_CPUID)
        .with_rdtsc()
        .with_msr(&ALL_MSR)
        .with_halt(&ALL_HALT);

#[test]
fn full_set_is_const_and_runs_one_system_sequence() {
    // `xor eax,eax; cpuid; rdtsc; mov ecx,0xc0000080; rdmsr; cli; hlt`.
    let bytes = [
        0x31, 0xc0, 0x0f, 0xa2, 0x0f, 0x31, 0xb9, 0x80, 0x00, 0x00, 0xc0, 0x0f, 0x32, 0xfa, 0xf4,
    ];
    let mut source = || 0x1_2345_6789u64;
    let mut state = board_state();
    let mut memory = Memory::code(&bytes);
    let result =
        run_set_with_source(&mut state, &mut memory, &FULL_SET, 64, Some(&mut source)).unwrap();
    assert_eq!(exit(&result), &RunExit::Halted);
    assert_eq!(result.instructions_executed, 7);
    assert_eq!(state.gpr[0], 0xd00);
    assert_eq!(state.gpr[2], 0);
    assert_eq!(state.activity, Some(CpuActivity::Halted));
    assert_eq!(state.cr0, Some(CR0_BASE));
    assert_eq!(state.cr4, Some(CR4_BASE));
}

#[test]
fn system_members_keep_fp_and_cr_admission_unchanged() {
    // Admission is identical with and without the new members.
    let mut state = board_state();
    state.cr3 = Some(0x1000);
    let before = state.clone();
    let mut memory = Memory::code(&[0xf4]);
    assert_eq!(
        run_set(&mut state, &mut memory, &FULL_SET, 4).unwrap_err(),
        Cpl0ProfileSetInputError::Cr3NotAdmitted
    );
    assert_eq!(state, before);
    let mut state = board_state();
    state.cr0 = None;
    let mut memory = Memory::code(&[0xf4]);
    assert_eq!(
        run_set(&mut state, &mut memory, &FULL_SET, 4).unwrap_err(),
        Cpl0ProfileSetInputError::Cr0Missing
    );
    assert_eq!(memory.fetches, 0);
}
