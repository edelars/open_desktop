use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, MovCr3Long64V1, RdtscControls, RunConfig, RunExit, run,
    run_with_cli_profile, run_with_cpl0_profile_set, run_with_cpl0_profile_set_and_pio,
    run_with_cpuid_profile, run_with_efer_rdmsr_profile, run_with_halt_profile,
    run_with_msr_profile, run_with_rdtsc_source,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile, CpuActivity,
    DataSegmentDescriptorCache, FsDescriptorCache, GuestMemory, HaltProfile, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register, StiShadow,
    TranslationContextChange, cpuid::CpuidProfile, msr::MsrProfile,
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
    match &result.exit {
        Cpl0ProfileSetExit::Run(exit) => exit,
        other => panic!("expected a Run exit, got {other:?}"),
    }
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
    // RFLAGS.VM and a clear RFLAGS bit 1 are rejected for every set, with or
    // without CLI, with their own typed errors; PE/PVI are covered by the policy.
    for (name, mutate, expected) in [
        (
            "vm",
            (|s: &mut LongModeState| s.rflags |= 1 << 17) as fn(&mut LongModeState),
            Cpl0ProfileSetInputError::RflagsVmSet,
        ),
        (
            "bit 1 clear",
            |s| s.rflags &= !2,
            Cpl0ProfileSetInputError::RflagsReservedBitClear,
        ),
    ] {
        for set in [&with_cli, &plain] {
            let mut state = board_state();
            mutate(&mut state);
            let before = state.clone();
            let mut memory = Memory::code(&BOARD_PROLOGUE);
            let err = run_set(&mut state, &mut memory, set, 64).unwrap_err();
            assert_eq!(err, expected, "{name}");
            assert_eq!(memory.fetches, 0, "{name}");
            assert_eq!(state, before, "{name}");
        }
    }
}

#[test]
fn set_without_cli_rejects_rflags_vm_and_clear_bit1_before_any_fetch() {
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for (rflags, expected) in [
        (2 | IF | (1 << 17), Cpl0ProfileSetInputError::RflagsVmSet),
        (IF | (1 << 17), Cpl0ProfileSetInputError::RflagsVmSet),
        (IF, Cpl0ProfileSetInputError::RflagsReservedBitClear),
        (0, Cpl0ProfileSetInputError::RflagsReservedBitClear),
    ] {
        let mut state = board_state();
        state.rflags = rflags;
        let before = state.clone();
        let mut memory = Memory::code(&[0x90]);
        let err = run_set(&mut state, &mut memory, &plain, 4).unwrap_err();
        assert_eq!(err, expected, "rflags {rflags:#x}");
        assert_eq!(memory.fetches, 0, "rflags {rflags:#x}");
        assert_eq!(state, before, "rflags {rflags:#x}");
    }
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
    // 0xa0 adds PGE (bit 7); bit 17 is PCIDE, bit 18 OSXSAVE, bit 2 TSD and
    // bit 40 is reserved: all outside the CR4 policy (only OSFXSR/OSXMMEXCPT
    // may vary). PAE cleared is outside as well.
    for value in [
        0xa0u64,
        CR4_BASE | (1 << 17),
        CR4_BASE | (1 << 18),
        CR4_BASE | (1 << 2),
        CR4_BASE | (1 << 40),
        0,
    ] {
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
    // PE, PG and WP cleared, bit 32 set (reserved), CD and NW set, and zero: the
    // policy only lets EM/TS vary on top of CR0_BASE.
    for value in [
        CR0_BASE & !1,
        CR0_BASE & !0x8000_0000,
        CR0_BASE & !(1 << 16),
        CR0_BASE | (1 << 32),
        CR0_BASE | (1 << 30),
        CR0_BASE | (1 << 29),
        0,
    ] {
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
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        7u64
    };
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x66, 0x0f, 0x31]);
    let result = run_set_with_source(&mut state, &mut memory, &set, 1, Some(&mut source)).unwrap();
    // Strict expectations on their own, not only agreement with the legacy entry.
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x31 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(calls, 0);
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

const MOV_CR3_RAX: [u8; 3] = [0x0f, 0x22, 0xd8];

fn cr3_set() -> Cpl0ProfileSet<'static> {
    cr_set().with_cpu_owned_cr3(MovCr3Long64V1)
}

fn cr3_state(cr3: u64) -> LongModeState {
    let mut state = board_state();
    state.cr3 = Some(cr3);
    state
}

fn cr3_write(old_cr3: u64, new_cr3: u64) -> Cpl0ProfileSetExit {
    Cpl0ProfileSetExit::TranslationContextChange(TranslationContextChange::Cr3Write {
        old_cr3,
        new_cr3,
    })
}

fn ud() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    })
}

#[test]
fn cli_mov_rax_mov_cr3_stops_at_the_boundary_after_the_write() {
    // `cli; movabs rax, 0x2000; mov cr3, rax`; no bytes follow, so a fetch
    // past the MOV CR3 would surface as a memory error instead of this exit.
    let mut code = vec![0xfa];
    code.extend(movabs_rax(0x2000));
    code.extend_from_slice(&MOV_CR3_RAX);
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x2000));
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.cr3, Some(0x2000));
    assert_eq!(state.gpr[0], 0x2000);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.rflags, 2);
    assert_eq!((state.cr0, state.cr4), (Some(CR0_BASE), Some(CR4_BASE)));
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
}

#[test]
fn mov_cr3_without_cli_retires_two_and_leaves_other_state_alone() {
    let mut code = movabs_rax(0x2000);
    code.extend_from_slice(&MOV_CR3_RAX);
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpu_owned_cr3(MovCr3Long64V1);
    let mut state = cr3_state(0x1000);
    let before = state.clone();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x2000));
    assert_eq!(result.instructions_executed, 2);
    let mut expected = before;
    expected.gpr[0] = 0x2000;
    expected.cr3 = Some(0x2000);
    expected.rip = RIP + code.len() as u64;
    assert_eq!(state, expected);
}

#[test]
fn mov_cr3_with_an_unchanged_value_still_reports_the_write() {
    let mut code = movabs_rax(0x1000);
    code.extend_from_slice(&MOV_CR3_RAX);
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x1000));
    assert_eq!(result.instructions_executed, 2);
}

#[test]
fn mov_cr3_exit_wins_over_budget_and_stop_at_the_same_boundary() {
    let mut code = movabs_rax(0x2000);
    code.extend_from_slice(&MOV_CR3_RAX);
    code.push(0x90);
    let end = RIP + code.len() as u64 - 1;
    // Budget exhausted exactly at the MOV CR3.
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 2).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x2000));
    assert_eq!(result.instructions_executed, 2);
    // STOP address equal to the next RIP.
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let mut stop = config(PrivilegeLevel::Ring0, 64);
    stop.stop_rip = Some(end);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        stop,
        od_isa::INTEGER_FLAGS,
        &cr3_set(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x2000));
    assert_eq!(state.rip, end);
    // Resuming after the exit continues with the next instruction.
    let mut resume = config(PrivilegeLevel::Ring0, 1);
    resume.stop_rip = None;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        resume,
        od_isa::INTEGER_FLAGS,
        &cr3_set(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, end + 1);
}

#[test]
fn mov_cr3_beyond_36_bits_is_gp0_without_changing_cr3() {
    for value in [1u64 << 40, 1 << 36, u64::MAX] {
        let mut code = movabs_rax(value);
        code.extend_from_slice(&MOV_CR3_RAX);
        let mut state = cr3_state(0x1000);
        let mut memory = Memory::code(&code);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }),
            "{value:#x}"
        );
        assert_eq!(result.instructions_executed, 1, "{value:#x}");
        assert_eq!(state.cr3, Some(0x1000), "{value:#x}");
        assert_eq!(state.rip, RIP + 10, "{value:#x}");
        assert_eq!(state.gpr[0], value, "{value:#x}");
    }
    // The highest accepted value is 2^36 - 1.
    let mut code = movabs_rax((1 << 36) - 1);
    code.extend_from_slice(&MOV_CR3_RAX);
    let mut state = cr3_state(0);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.exit, cr3_write(0, (1 << 36) - 1));
}

#[test]
fn lock_and_invalid_forms_are_ud_under_the_cr3_set() {
    let set = cr3_set();
    for bytes in [
        &[0xf0, 0x0f, 0x22, 0xd8][..],
        &[0xf0, 0x0f, 0x22, 0xc0][..],
        &[0xf0, 0x0f, 0x20, 0xc0][..],
        &[0xf0, 0x0f, 0x22, 0xe0][..],
    ] {
        let mut state = cr3_state(0x1000);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        assert_eq!(exit(&result), &ud(), "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn lock_mov_cr3_without_cr3_ownership_keeps_its_old_classification() {
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0xf0, 0x0f, 0x22, 0xd8]);
    let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x22 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn mov_cr3_stays_unsupported_without_cr3_ownership() {
    let mut code = movabs_rax(0x2000);
    code.extend_from_slice(&MOV_CR3_RAX);
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x22 });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cr3, None);
}

#[test]
fn prefixed_mov_cr3_forms_are_unsupported_without_mutation() {
    // Architecturally these are not #UD (REX.W and 0x66 are ignored by MOV CR in
    // 64-bit mode, and `0F 22 D9` is a valid `MOV CR3, RCX`); they are outside
    // the exact `MOV CR3, RAX` profile, so the decoder reports the opcode byte
    // `0x22` as unsupported in every case.
    for bytes in [
        &[0x48, 0x0f, 0x22, 0xd8][..],
        &[0x66, 0x0f, 0x22, 0xd8][..],
        &[0x0f, 0x22, 0xd9][..],
    ] {
        let mut state = cr3_state(0x1000);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x22 }),
            "{bytes:02x?}"
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn cr3_admission_is_explicit_and_checked_before_fetch() {
    // A modeled CR3 without ownership is rejected as before.
    let mut state = cr3_state(0x1000);
    let before = state.clone();
    let mut memory = Memory::code(&MOV_CR3_RAX);
    assert_eq!(
        run_set(&mut state, &mut memory, &cr_set(), 64).unwrap_err(),
        Cpl0ProfileSetInputError::Cr3NotAdmitted
    );
    assert_eq!((memory.fetches, &state), (0, &before));
    // Ownership needs a modeled CR3 within 36 bits.
    let mut state = board_state();
    let mut memory = Memory::code(&MOV_CR3_RAX);
    assert_eq!(
        run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap_err(),
        Cpl0ProfileSetInputError::Cr3Missing
    );
    assert_eq!(memory.fetches, 0);
    let mut state = cr3_state(1 << 40);
    let before = state.clone();
    let mut memory = Memory::code(&MOV_CR3_RAX);
    assert_eq!(
        run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap_err(),
        Cpl0ProfileSetInputError::InvalidInitialCr3 { value: 1 << 40 }
    );
    assert_eq!((memory.fetches, &state), (0, &before));
    // A valid CR3 admits and the first instruction runs.
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&movabs_rax(7));
    let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 7);
    assert_eq!(state.cr3, Some(0x1000));
}

#[test]
fn cr3_ownership_keeps_cr0_cr4_moves_and_other_members_working() {
    // `mov rax,cr4; mov cr3,rax` writes CR4's value (0x20) to CR3.
    let mut code = MOV_RAX_CR4.to_vec();
    code.extend_from_slice(&MOV_CR3_RAX);
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, CR4_BASE));
    assert_eq!(result.instructions_executed, 2);
    // CR0 writes outside the policy stay Unsupported with CR3 owned.
    let mut code = movabs_rax(0x8001_0031);
    code.extend_from_slice(&MOV_CR0_RAX);
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x22 });
    assert_eq!(state.cr0, Some(CR0_BASE));
    assert_eq!(state.cr3, Some(0x1000));
}

// ---- S4-38L4c: MOV RAX, CR3 read in the CR3-owning set ----------------------

const MOV_RAX_CR3: [u8; 3] = [0x0f, 0x20, 0xd8];
const MOV_RAX_CR3_REX_W: [u8; 4] = [0x48, 0x0f, 0x20, 0xd8];
/// A 64-bit-looking value that stays within the 36-bit CR3 limit.
const CR3_DISTINCT: u64 = 0x8_0013_0000;

#[test]
fn mov_rax_cr3_returns_state_cr3_and_touches_nothing_else() {
    for (bytes, length) in [(&MOV_RAX_CR3[..], 3u64), (&MOV_RAX_CR3_REX_W[..], 4u64)] {
        // Read followed by a NOP boundary: the budget ends the run after the read.
        let mut state = cr3_state(CR3_DISTINCT);
        state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
        state.gpr[Register::Rsp as usize] = 0x7000;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
        let mut expected = before;
        expected.gpr[0] = CR3_DISTINCT;
        expected.rip = RIP + length;
        assert_eq!(state, expected, "{bytes:02x?}");
        assert_eq!(state.cr3, Some(CR3_DISTINCT));
    }
}

#[test]
fn mov_rax_cr3_follows_a_cr3_write_and_reads_the_new_value() {
    // `movabs rax, 0x2000; mov cr3, rax` stops with a translation-context
    // change; after resuming, `mov rax, cr3` returns the new CR3.
    let mut code = movabs_rax(0x2000);
    code.extend_from_slice(&MOV_CR3_RAX);
    code.extend_from_slice(&[0x31, 0xc0]); // xor eax, eax
    code.extend_from_slice(&MOV_RAX_CR3);
    let mut state = cr3_state(0x1000);
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.exit, cr3_write(0x1000, 0x2000));
    let result = run_set(&mut state, &mut memory, &cr3_set(), 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[0], 0x2000);
    assert_eq!(state.cr3, Some(0x2000));
    assert_eq!(state.rip, RIP + code.len() as u64);
}

#[test]
fn mov_rax_cr3_stays_unsupported_without_cr3_ownership() {
    for bytes in [&MOV_RAX_CR3[..], &MOV_RAX_CR3_REX_W[..]] {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn mov_rax_cr3_stays_unsupported_outside_the_set() {
    // No CR0/CR3/CR4 model, so the plain entry admits the state and the decoder reaches
    // `ReadCr3`; the executor refuses it (all three bytes were fetched, nothing retired).
    for bytes in [&MOV_RAX_CR3[..], &MOV_RAX_CR3_REX_W[..]] {
        let mut state = LongModeState {
            rip: RIP,
            rflags: 2 | IF,
            ..LongModeState::default()
        };
        state.gpr[0] = 0x1111_2222_3333_4444;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(memory.fetches, bytes.len(), "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_rax_cr3_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0x66, 0x0f, 0x20, 0xd8][..],
        &[0xf0, 0x0f, 0x20, 0xd8][..],
        &[0x40, 0x0f, 0x20, 0xd8][..],
        &[0x49, 0x0f, 0x20, 0xd8][..],
        &[0x0f, 0x20, 0xd9][..],
        &[0x0f, 0x20, 0x18][..],
        &[0x48, 0x0f, 0x20, 0xd9][..],
        &[0xf3, 0x0f, 0x20, 0xd8][..],
        &[0x67, 0x0f, 0x20, 0xd8][..],
        &[0x2e, 0x0f, 0x20, 0xd8][..],
        &[0x41, 0x0f, 0x20, 0xd8][..],
        &[0x4c, 0x0f, 0x20, 0xd8][..],
        // Only the REX nearest the opcode is effective, but the length 5 is outside the
        // exact forms: the executor refuses it.
        &[0x40, 0x48, 0x0f, 0x20, 0xd8][..],
    ] {
        let mut state = cr3_state(CR3_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x20 }),
            "{bytes:02x?}"
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_rax_cr3_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch, so the read never runs.
    let mut state = cr3_state(CR3_DISTINCT);
    let before = state.clone();
    let mut memory = Memory::code(&MOV_RAX_CR3);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr3_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

// ---- S4-38L5a: MOV RAX, CR2 read in the CR3-owning set ----------------------

const MOV_RAX_CR2: [u8; 3] = [0x0f, 0x20, 0xd0];
const MOV_RAX_CR2_REX_W: [u8; 4] = [0x48, 0x0f, 0x20, 0xd0];
/// The faulting address of the real-kernel `#PF` (S4-38L4h evidence).
const CR2_DISTINCT: u64 = 0x0000_0000_0102_0020;

fn cr2_state(cr2: Option<u64>) -> LongModeState {
    let mut state = cr3_state(CR3_DISTINCT);
    state.cr2 = cr2;
    state
}

#[test]
fn mov_rax_cr2_returns_state_cr2_and_touches_nothing_else() {
    for (bytes, length) in [(&MOV_RAX_CR2[..], 3u64), (&MOV_RAX_CR2_REX_W[..], 4u64)] {
        for value in [CR2_DISTINCT, 0, 0xffff_8880_dead_beef] {
            let mut state = cr2_state(Some(value));
            state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
            state.gpr[Register::Rax as usize] = 0x1111_2222_3333_4444;
            state.gpr[Register::Rsp as usize] = 0x7000;
            let before = state.clone();
            let mut memory = Memory::code(bytes);
            let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{bytes:02x?}"
            );
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
            let mut expected = before.clone();
            expected.gpr[0] = value;
            expected.rip = RIP + length;
            assert_eq!(state, expected, "{bytes:02x?}");
            assert_eq!(state.rflags, before.rflags);
            assert_eq!(state.gpr[Register::Rsp as usize], 0x7000);
            assert_eq!(state.cr2, Some(value));
            assert_eq!(state.cr3, Some(CR3_DISTINCT));
        }
    }
}

#[test]
fn mov_rax_cr2_with_unmodeled_cr2_is_unsupported_without_mutation() {
    for bytes in [&MOV_RAX_CR2[..], &MOV_RAX_CR2_REX_W[..]] {
        let mut state = cr2_state(None);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn mov_rax_cr2_stays_unsupported_without_the_cr3_owning_set() {
    for bytes in [&MOV_RAX_CR2[..], &MOV_RAX_CR2_REX_W[..]] {
        let mut state = board_state();
        state.cr2 = Some(CR2_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn mov_rax_cr2_stays_unsupported_outside_the_set() {
    for bytes in [&MOV_RAX_CR2[..], &MOV_RAX_CR2_REX_W[..]] {
        let mut state = LongModeState {
            rip: RIP,
            rflags: 2 | IF,
            cr2: Some(CR2_DISTINCT),
            ..LongModeState::default()
        };
        state.gpr[0] = 0x1111_2222_3333_4444;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_rax_cr2_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0x0f, 0x22, 0xd0][..], // MOV CR2, RAX: write is not modeled
        &[0x48, 0x0f, 0x22, 0xd0][..],
        &[0x0f, 0x20, 0xc8][..],       // CR1
        &[0x0f, 0x20, 0xe8][..],       // CR5
        &[0xf0, 0x0f, 0x20, 0xd0][..], // LOCK
        &[0x66, 0x0f, 0x20, 0xd0][..],
        &[0xf3, 0x0f, 0x20, 0xd0][..],
        &[0x67, 0x0f, 0x20, 0xd0][..],
        &[0x40, 0x48, 0x0f, 0x20, 0xd0][..],
    ] {
        let mut state = cr2_state(Some(CR2_DISTINCT));
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert!(
            matches!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported {
                    opcode: 0x20 | 0x22
                })
            ),
            "{bytes:02x?}: {:?}",
            result.exit
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_rax_cr3_is_unchanged_by_the_cr2_form() {
    let mut state = cr2_state(Some(CR2_DISTINCT));
    let mut memory = Memory::code(&MOV_RAX_CR3);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], CR3_DISTINCT);
}

#[test]
fn mov_rax_cr2_is_not_reachable_from_cpl3() {
    let mut state = cr2_state(Some(CR2_DISTINCT));
    let before = state.clone();
    let mut memory = Memory::code(&MOV_RAX_CR2);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr3_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

// ---- S4-38L5d: MOV r64, CR2 into any general register ----------------------

/// Expected state after `MOV r64, CR2` into `dest`: only `dest` and RIP change.
fn cr2_any_reg_expected(
    before: &LongModeState,
    dest: usize,
    value: u64,
    length: u64,
) -> LongModeState {
    let mut expected = before.clone();
    expected.gpr[dest] = value;
    expected.rip = RIP + length;
    expected
}

fn cr2_any_reg_state(value: u64) -> LongModeState {
    let mut state = cr2_state(Some(value));
    state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
    for (index, gpr) in state.gpr.iter_mut().enumerate() {
        *gpr = 0x1111_0000_0000_0000 + index as u64;
    }
    // RSP is a stack pointer for the board; keep it a plain, distinct value.
    state.gpr[Register::Rsp as usize] = 0x7000;
    state
}

#[test]
fn mov_r64_cr2_writes_every_low_register_and_touches_nothing_else() {
    for rm in 0u8..8 {
        for rex in [None, Some(0x40u8), Some(0x48), Some(0x42), Some(0x4a)] {
            let mut bytes = Vec::new();
            bytes.extend(rex);
            bytes.extend_from_slice(&[0x0f, 0x20, 0xd0 | rm]);
            let mut state = cr2_any_reg_state(CR2_DISTINCT);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{bytes:02x?}"
            );
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
            let expected =
                cr2_any_reg_expected(&before, rm as usize, CR2_DISTINCT, bytes.len() as u64);
            assert_eq!(state, expected, "{bytes:02x?}");
            assert_eq!(state.rflags, before.rflags, "{bytes:02x?}");
            assert_eq!(state.cr2, Some(CR2_DISTINCT));
            assert_eq!(state.cr3, Some(CR3_DISTINCT));
        }
    }
}

#[test]
fn mov_rbx_cr2_is_the_real_kernel_form() {
    // `0F 20 D3` at RIP 0x543c35e in the real kernel after the #PF handler.
    let mut state = cr2_any_reg_state(CR2_DISTINCT);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x20, 0xd3]);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state,
        cr2_any_reg_expected(&before, Register::Rbx as usize, CR2_DISTINCT, 3)
    );
    assert_eq!(state.gpr[Register::Rbx as usize], CR2_DISTINCT);
}

#[test]
fn mov_r64_cr2_with_rex_b_writes_r8_to_r15() {
    for rm in 0u8..8 {
        for rex in [0x41u8, 0x49, 0x43, 0x4b] {
            let bytes = [rex, 0x0f, 0x20, 0xd0 | rm];
            let mut state = cr2_any_reg_state(0xffff_8880_dead_beef);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(
                state,
                cr2_any_reg_expected(&before, 8 + rm as usize, 0xffff_8880_dead_beef, 4),
                "{bytes:02x?}"
            );
            // The low register of the same number is untouched.
            assert_eq!(state.gpr[rm as usize], before.gpr[rm as usize]);
        }
    }
}

#[test]
fn mov_rsp_and_rbp_cr2_are_plain_register_writes() {
    for (rm, reg) in [(4u8, Register::Rsp), (5, Register::Rbp)] {
        let mut state = cr2_any_reg_state(CR2_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x20, 0xd0 | rm]);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[reg as usize], CR2_DISTINCT);
        assert_eq!(
            state,
            cr2_any_reg_expected(&before, reg as usize, CR2_DISTINCT, 3)
        );
    }
}

#[test]
fn mov_r64_cr2_writes_the_full_64_bits() {
    let mut state = cr2_any_reg_state(0xffff_ffff_8000_0001);
    let mut memory = Memory::code(&[0x0f, 0x20, 0xd1]);
    run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(state.gpr[Register::Rcx as usize], 0xffff_ffff_8000_0001);
}

#[test]
fn mov_r64_cr2_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0x0f, 0x20, 0xc9][..],             // CR1
        &[0x0f, 0x20, 0xe3][..],             // CR4 into RBX
        &[0x0f, 0x20, 0xdb][..],             // CR3 into RBX
        &[0x0f, 0x20, 0xc3][..],             // CR0 into RBX
        &[0x48, 0x0f, 0x20, 0xc3][..],       // REX.W CR0 into RBX
        &[0x41, 0x0f, 0x20, 0xd8][..],       // CR3 into R8 (REX.B)
        &[0x44, 0x0f, 0x20, 0xd3][..],       // REX.R: CR10
        &[0x4d, 0x0f, 0x20, 0xdf][..],       // REX.W+R+B: CR10 into R15
        &[0x0f, 0x20, 0x13][..],             // mod 00: not modeled (SDM ignores mod; conservative)
        &[0x0f, 0x20, 0x53, 0x08][..],       // mod 01
        &[0x0f, 0x20, 0x93, 0, 0, 0, 0][..], // mod 10
        &[0x0f, 0x22, 0xd3][..],             // MOV CR2, RBX: write is not modeled
        &[0x41, 0x0f, 0x22, 0xd0][..],
        &[0xf0, 0x0f, 0x20, 0xd3][..], // LOCK
        &[0xf0, 0x41, 0x0f, 0x20, 0xd3][..],
        &[0x66, 0x0f, 0x20, 0xd3][..],
        &[0x66, 0x41, 0x0f, 0x20, 0xd3][..],
        &[0xf3, 0x0f, 0x20, 0xd3][..],
        &[0xf2, 0x0f, 0x20, 0xd3][..],
        &[0x67, 0x0f, 0x20, 0xd3][..],
        &[0x2e, 0x0f, 0x20, 0xd3][..],
        &[0x64, 0x0f, 0x20, 0xd3][..],
        &[0x41, 0x66, 0x0f, 0x20, 0xd3][..], // REX before another prefix is ignored
        &[0x40, 0x41, 0x0f, 0x20, 0xd3][..], // several REX
        &[0x41, 0x40, 0x0f, 0x20, 0xd3][..],
    ] {
        let mut state = cr2_state(Some(CR2_DISTINCT));
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert!(
            matches!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported {
                    opcode: 0x20 | 0x22
                })
            ),
            "{bytes:02x?}: {:?}",
            result.exit
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_r64_cr2_with_unmodeled_cr2_is_unsupported_without_mutation() {
    for bytes in [&[0x0f, 0x20, 0xd3][..], &[0x41, 0x0f, 0x20, 0xd3][..]] {
        let mut state = cr2_state(None);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn mov_r64_cr2_stays_unsupported_without_the_cr3_owning_set() {
    for bytes in [&[0x0f, 0x20, 0xd3][..], &[0x41, 0x0f, 0x20, 0xd3][..]] {
        let mut state = board_state();
        state.cr2 = Some(CR2_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn mov_r64_cr2_stays_unsupported_outside_the_set() {
    for bytes in [&[0x0f, 0x20, 0xd3][..], &[0x41, 0x0f, 0x20, 0xd3][..]] {
        let mut state = LongModeState {
            rip: RIP,
            rflags: 2 | IF,
            cr2: Some(CR2_DISTINCT),
            ..LongModeState::default()
        };
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x20 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_r64_cr2_is_not_reachable_from_cpl3() {
    let mut state = cr2_state(Some(CR2_DISTINCT));
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x20, 0xd3]);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr3_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

#[test]
fn mov_r64_cr0_cr3_cr4_keep_their_results() {
    // CR3 stays RAX-only; the new CR2 forms must not widen it.
    let mut state = cr2_state(Some(CR2_DISTINCT));
    let mut memory = Memory::code(&MOV_RAX_CR3);
    run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(state.gpr[0], CR3_DISTINCT);
    let mut state = cr2_state(Some(CR2_DISTINCT));
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x20, 0xdb]);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

// ---- S2-45e: PIO u8 in the set -------------------------------------------

/// `mov edx, 0x3f8`.
const MOV_EDX_3F8: [u8; 5] = [0xba, 0xf8, 0x03, 0x00, 0x00];
/// `mov al, 'X'`.
const MOV_AL_X: [u8; 2] = [0xb0, 0x58];
const OUT_DX_AL: u8 = 0xee;
const IN_AL_DX: u8 = 0xec;

#[derive(Default)]
struct TestPio {
    read_value: u8,
    reads: Vec<u16>,
    writes: Vec<(u16, u8)>,
    fail_read: bool,
    fail_write: bool,
}

impl PortIo for TestPio {
    type Error = &'static str;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        self.reads.push(port);
        if self.fail_read {
            Err("read failed")
        } else {
            Ok(self.read_value)
        }
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        self.writes.push((port, value));
        if self.fail_write {
            Err("write failed")
        } else {
            Ok(())
        }
    }
}

fn pio_set(cli: &CliProfile) -> Cpl0ProfileSet<'_> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(cli)
        .with_pio_u8()
}

fn run_pio(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    pio: &mut TestPio,
    budget: u64,
) -> Cpl0ProfileSetResult<&'static str> {
    run_with_cpl0_profile_set_and_pio(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        pio,
        || false,
    )
    .unwrap()
}

fn pio_code(tail: u8) -> Vec<u8> {
    let mut code = MOV_EDX_3F8.to_vec();
    code.extend_from_slice(&MOV_AL_X);
    code.push(tail);
    code
}

#[test]
fn out_dx_al_writes_one_byte_and_retires_three_instructions() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let mut memory = Memory::code(&pio_code(OUT_DX_AL));
    let mut pio = TestPio::default();
    let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 3);
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(pio.writes, [(0x3f8, 0x58)]);
    assert!(pio.reads.is_empty());
    assert_eq!(state.rip, RIP + 8);
    assert_eq!(state.gpr[0] & 0xff, 0x58);
    assert_eq!(state.gpr[2], 0x3f8);
}

#[test]
fn out_dx_al_port_error_is_typed_and_leaves_the_out_unretired() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let mut memory = Memory::code(&pio_code(OUT_DX_AL));
    let mut pio = TestPio {
        fail_write: true,
        ..TestPio::default()
    };
    let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 64);
    assert_eq!(result.exit, Cpl0ProfileSetExit::PortIo("write failed"));
    assert_eq!(result.instructions_executed, 2);
    // Exactly one callback; it ran before the commit, so RIP stays on the OUT
    // and the register state is the state after the two MOVs.
    assert_eq!(pio.writes, [(0x3f8, 0x58)]);
    assert_eq!(state.rip, RIP + 7);
    assert_eq!(state.gpr[0], 0x1111_2222_3333_4458);
    assert_eq!(state.gpr[2], 0x3f8);
    assert_eq!(state.rflags & IF, IF);
}

#[test]
fn in_al_dx_loads_al_from_the_port_and_keeps_the_high_rax_bits() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let mut memory = Memory::code(&pio_code(IN_AL_DX));
    let mut pio = TestPio {
        read_value: 0xa5,
        ..TestPio::default()
    };
    let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 3);
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(pio.reads, [0x3f8]);
    assert!(pio.writes.is_empty());
    assert_eq!(state.gpr[0], 0x1111_2222_3333_44a5);
    assert_eq!(state.rip, RIP + 8);
}

#[test]
fn in_al_dx_port_error_keeps_al_and_rip() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    let mut memory = Memory::code(&pio_code(IN_AL_DX));
    let mut pio = TestPio {
        read_value: 0xa5,
        fail_read: true,
        ..TestPio::default()
    };
    let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 64);
    assert_eq!(result.exit, Cpl0ProfileSetExit::PortIo("read failed"));
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(pio.reads, [0x3f8]);
    assert_eq!(state.rip, RIP + 7);
    // AL still holds 'X' from the MOV, not the port value.
    assert_eq!(state.gpr[0], 0x1111_2222_3333_4458);
}

#[test]
fn pio_entry_without_the_set_flag_leaves_out_unsupported_and_untouched() {
    let cli = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    for (tail, opcode) in [(OUT_DX_AL, 0xee), (IN_AL_DX, 0xec)] {
        let mut state = board_state();
        let mut memory = Memory::code(&pio_code(tail));
        let mut pio = TestPio::default();
        let result = run_pio(&mut state, &mut memory, &set, &mut pio, 64);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode })
        );
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, RIP + 7);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn pio_flag_without_a_pio_entry_stays_unsupported() {
    let cli = CliProfile::NonPviLong64V1;
    let set = pio_set(&cli);
    for (tail, opcode) in [(OUT_DX_AL, 0xee), (IN_AL_DX, 0xec)] {
        let mut state = board_state();
        let mut memory = Memory::code(&pio_code(tail));
        let result = run_set(&mut state, &mut memory, &set, 64).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, RIP + 7);
    }
}

#[test]
fn pio_does_not_widen_to_prefixed_or_other_port_forms() {
    let cli = CliProfile::NonPviLong64V1;
    // The 0x66/REP prefixes are consumed as legacy prefixes and the following
    // opcode byte is reported, so `66 EE` and `F3 EE` give `Unsupported` with
    // the `OUT` opcode `0xee`, not #UD. The imm8 forms (E4/E6) and the word
    // form (ED/EF) report their own opcode.
    for (code, opcode) in [
        (&[0x66, 0xee][..], 0xee),
        (&[0xf3, 0xee][..], 0xee),
        (&[0xe6, 0x80][..], 0xe6),
        (&[0xe4, 0x80][..], 0xe4),
        (&[0xef][..], 0xef),
        (&[0xed][..], 0xed),
    ] {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let mut pio = TestPio::default();
        let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 64);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode }),
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state.rip, RIP);
        assert_eq!(state, before, "{code:x?}");
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn pio_entry_admission_is_unchanged() {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    state.cr0 = None;
    let mut memory = Memory::code(&pio_code(OUT_DX_AL));
    let mut pio = TestPio::default();
    let error = run_with_cpl0_profile_set_and_pio(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 8),
        od_isa::INTEGER_FLAGS,
        &pio_set(&cli),
        None,
        &mut pio,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::Cr0Missing);
    assert_eq!(memory.fetches, 0);
    assert!(pio.writes.is_empty());
}

// ---- S2-46: null segment loads over loaded hidden caches ----

const DATA_CACHE: DataSegmentDescriptorCache = DataSegmentDescriptorCache {
    base: 0,
    limit: 0xffff_ffff,
    access: 0x93,
    flags: 0xc,
};
const FS_GS_CACHE: FsDescriptorCache = FsDescriptorCache {
    limit: 0xffff_ffff,
    access: 0x93,
    flags: 0xc,
};

/// The Linux board entry: DS/ES/SS/FS/GS hold loaded data caches, IF = 0.
fn loaded_state() -> LongModeState {
    LongModeState {
        rflags: 2,
        ss_selector: Some(0x18),
        ss_cache: Some(DATA_CACHE),
        ds_cache: Some(DATA_CACHE),
        es_cache: Some(DATA_CACHE),
        fs_cache: Some(FS_GS_CACHE),
        gs_cache: Some(FS_GS_CACHE),
        fs_selector: 0x18,
        gs_selector: 0x18,
        fs_base: 0x1234_5000,
        gs_base: 0x5678_9000,
        ..board_state()
    }
}

fn cli_set() -> Cpl0ProfileSet<'static> {
    static CLI: CliProfile = CliProfile::NonPviLong64V1;
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&CLI)
}

#[test]
fn null_ds_es_ss_loads_over_loaded_caches_retire_and_open_the_mov_ss_shadow() {
    let mut state = loaded_state();
    let mut memory = Memory::code(&BOARD_PROLOGUE);
    let result = run_set(&mut state, &mut memory, &cli_set(), 64).unwrap();
    // cld, cli, xor, mov ds, mov es, mov ss retire; the fetch after the code is unmapped.
    assert_eq!(result.instructions_executed, 6);
    assert!(matches!(exit(&result), RunExit::MemoryError(_)));
    assert_eq!(state.rip, RIP + 10);
    assert_eq!(state.rflags & IF, 0);
    assert_eq!((state.ds_selector, state.es_selector), (0, 0));
    assert_eq!(state.ss_selector, Some(0));
    assert!(state.ds_unusable && state.es_unusable && state.ss_unusable);
    assert!(!state.fs_unusable && !state.gs_unusable);
    // The hidden caches are kept; Long64 ignores DS/ES/SS base and limit.
    assert_eq!(state.ds_cache, Some(DATA_CACHE));
    assert_eq!(state.es_cache, Some(DATA_CACHE));
    assert_eq!(state.ss_cache, Some(DATA_CACHE));
    assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
    assert_eq!(state.gpr[0], 0);
}

#[test]
fn mov_ss_shadow_lasts_one_instruction_and_survives_single_step_runs() {
    let mut code = BOARD_PROLOGUE.to_vec();
    code.extend([0x90, 0x90]);
    // One run.
    let mut state = loaded_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cli_set(), 7).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 7);
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    // The machine runs one instruction per call: the shadow must be accepted at
    // entry after MOV SS (IF = 0) and cleared by the next retired instruction.
    let mut stepped = loaded_state();
    let mut defined = od_isa::INTEGER_FLAGS;
    for index in 0..7 {
        let mut memory = Memory::code(&code);
        let result = run_with_cpl0_profile_set(
            &mut stepped,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            defined,
            &cli_set(),
            None,
            || false,
        )
        .unwrap();
        assert_eq!(result.instructions_executed, 1, "step {index}");
        defined = result.rflags_defined;
        let expected = if index == 5 {
            Some(StiShadow::Blocking)
        } else {
            Some(StiShadow::Clear)
        };
        assert_eq!(stepped.sti_shadow, expected, "step {index}");
    }
    assert_eq!(stepped.rip, state.rip);
    assert_eq!(stepped, state);
}

#[test]
fn null_fs_gs_loads_keep_the_base_and_mark_the_cache_unusable() {
    let mut state = loaded_state();
    // xor eax,eax; mov fs,ax; mov gs,ax
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xe0, 0x8e, 0xe8]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!((state.fs_selector, state.gs_selector), (0, 0));
    assert!(state.fs_unusable && state.gs_unusable);
    assert_eq!((state.fs_base, state.gs_base), (0x1234_5000, 0x5678_9000));
    assert_eq!(state.fs_cache, Some(FS_GS_CACHE));
    assert_eq!(state.gs_cache, Some(FS_GS_CACHE));
    // MOV FS/GS open no interrupt shadow and leave DS/ES/SS alone.
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    assert_eq!((state.ds_selector, state.ss_selector), (0x18, Some(0x18)));
    assert!(!state.ds_unusable && !state.ss_unusable);
}

#[test]
fn null_loads_without_a_modeled_cache_keep_the_legacy_ds_es_and_refuse_ss() {
    // DS/ES without caches: the legacy null load, no unusable-cache state.
    let mut state = board_state();
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 3).unwrap();
    assert_eq!(result.instructions_executed, 3);
    assert!(state.ds_unusable && state.es_unusable);
    assert!(state.ds_cache.is_none() && state.es_cache.is_none());
    // SS without a cache stays unsupported with nothing mutated.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xd0]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.ss_selector, before.ss_selector);
    assert_eq!(state.sti_shadow, before.sti_shadow);
    assert!(!state.ss_unusable);
    // FS/GS null loads need no modeled cache (the board models none): the base stays.
    let mut state = LongModeState {
        fs_base: 0x1111_0000,
        gs_base: 0x2222_0000,
        fs_selector: 0x18,
        gs_selector: 0x18,
        ..board_state()
    };
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xe0, 0x8e, 0xe8]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 3).unwrap();
    assert_eq!(result.instructions_executed, 3);
    assert_eq!((state.fs_selector, state.gs_selector), (0, 0));
    assert!(state.fs_unusable && state.gs_unusable);
    assert!(state.fs_cache.is_none() && state.gs_cache.is_none());
    assert_eq!((state.fs_base, state.gs_base), (0x1111_0000, 0x2222_0000));
}

#[test]
fn nonzero_selectors_over_loaded_caches_without_a_gdt_are_exact_refusals() {
    // mov eax, 0x10; mov <sreg>, ax
    for modrm in [0xd8u8, 0xc0, 0xd0, 0xe0, 0xe8] {
        let mut state = loaded_state();
        let mut memory = Memory::code(&[0xb8, 0x10, 0, 0, 0, 0x8e, modrm]);
        let result = run_set(&mut state, &mut memory, &cli_set(), 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8e },
            "modrm {modrm:#x}"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, RIP + 5);
        let mut expected = loaded_state();
        expected.rip = RIP + 5;
        expected.gpr[0] = 0x10;
        assert_eq!(state.gpr, expected.gpr);
        assert_eq!(state.ds_selector, expected.ds_selector);
        assert_eq!(state.es_selector, expected.es_selector);
        assert_eq!(state.ss_selector, expected.ss_selector);
        assert_eq!(state.fs_selector, expected.fs_selector);
        assert_eq!(state.gs_selector, expected.gs_selector);
        assert!(!state.ds_unusable && !state.es_unusable && !state.ss_unusable);
        assert!(!state.fs_unusable && !state.gs_unusable);
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    }
}

#[test]
fn mov_ss_stays_closed_with_interrupts_enabled_or_under_a_shadow() {
    // IF = 1: refused without mutation (the shadow would be observable).
    let mut state = loaded_state();
    state.rflags |= IF;
    let before = state.clone();
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xd0]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.ss_selector, before.ss_selector);
    assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    // MOV SS directly after MOV SS: the second one is refused under the shadow.
    let mut state = loaded_state();
    let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, 0xd0, 0x8e, 0xd0]);
    let result = run_set(&mut state, &mut memory, &cli_set(), 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 4);
    assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
}

#[test]
fn blocking_shadow_with_interrupts_enabled_is_still_rejected_at_entry() {
    let mut state = loaded_state();
    state.rflags |= IF;
    state.sti_shadow = Some(StiShadow::Blocking);
    let mut memory = Memory::code(&[0x90]);
    assert_eq!(
        run_set(&mut state, &mut memory, &cli_set(), 1),
        Err(Cpl0ProfileSetInputError::StiShadowActive)
    );
    assert_eq!(memory.fetches, 0);
}

#[test]
fn legacy_entries_do_not_gain_null_loads_over_loaded_caches() {
    // `run` with loaded caches: DS/ES/SS stay Unsupported as before.
    for modrm in [0xd8u8, 0xc0, 0xd0] {
        let mut state = LongModeState {
            cr0: None,
            cr4: None,
            activity: None,
            sti_shadow: None,
            ..loaded_state()
        };
        let before = state.clone();
        let mut memory = Memory::code(&[0x31, 0xc0, 0x8e, modrm]);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 8),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x8e });
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.ds_selector, before.ds_selector);
        assert_eq!(state.ss_selector, before.ss_selector);
        assert!(!state.ds_unusable && !state.es_unusable && !state.ss_unusable);
    }
}

// ---- S2-47a: LGDT m (`0F 01 /2`) in the CPL0 profile set ----

const LGDT_DATA: u64 = 0x2000;
const GDT_BASE: u64 = 0x0000_0000_0012_0000;
const OLD_GDTR: od_isa::state::Gdtr = od_isa::state::Gdtr {
    base: 0x7000,
    limit: 0x0007,
};

/// Code at `RIP` plus one data window; reads in the page that starts at `fail_page`
/// return `fail` instead of data.
struct LgdtMemory {
    code: Vec<u8>,
    data: Vec<u8>,
    fail_page: Option<(u64, MemoryError)>,
    reads: Vec<(u64, usize)>,
}

impl LgdtMemory {
    fn new(code: &[u8], descriptor: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            data: descriptor.to_vec(),
            fail_page: None,
            reads: Vec::new(),
        }
    }
}

impl GuestMemory for LgdtMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.code
            .get(address.wrapping_sub(RIP) as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads.push((address, destination.len()));
        if let Some((page, error)) = &self.fail_page
            && address + destination.len() as u64 > *page
            && address < page + 0x1000
        {
            return Err(error.clone());
        }
        for (index, byte) in destination.iter_mut().enumerate() {
            let offset = (address + index as u64).wrapping_sub(LGDT_DATA) as usize;
            *byte = self.data.get(offset).copied().unwrap_or(0);
        }
        Ok(())
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

fn pseudo_descriptor(limit: u16, base: u64) -> Vec<u8> {
    let mut bytes = limit.to_le_bytes().to_vec();
    bytes.extend_from_slice(&base.to_le_bytes());
    bytes
}

fn lgdt_state(rax: u64) -> LongModeState {
    let mut state = board_state();
    state.gdtr = Some(OLD_GDTR);
    state.gpr[0] = rax;
    state
}

/// `lgdt_state` without the control-register, activity and STI-shadow modeling
/// that the set requires: what `run` accepts.
fn lgdt_legacy_state(rax: u64) -> LongModeState {
    LongModeState {
        cr0: None,
        cr4: None,
        activity: None,
        sti_shadow: None,
        ..lgdt_state(rax)
    }
}

fn run_lgdt_set(
    state: &mut LongModeState,
    memory: &mut LgdtMemory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap()
}

#[test]
fn lgdt_rax_loads_gdtr_from_the_pseudo_descriptor() {
    let mut state = lgdt_state(LGDT_DATA);
    let mut before = state.clone();
    let mut memory = LgdtMemory::new(
        &[0x0f, 0x01, 0x10, 0x90],
        &pseudo_descriptor(0x001f, GDT_BASE),
    );
    // One instruction, then the budget ends before the NOP byte is decoded.
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.gdtr,
        Some(od_isa::state::Gdtr {
            base: GDT_BASE,
            limit: 0x001f
        })
    );
    assert_eq!(state.rip, RIP + 3);
    // Only RIP and GDTR changed.
    before.rip = RIP + 3;
    before.gdtr = state.gdtr;
    assert_eq!(state, before);
    // Two qword reads inside the ten-byte operand, no write.
    assert_eq!(memory.reads, vec![(LGDT_DATA, 8), (LGDT_DATA + 2, 8)]);
}

#[test]
fn lgdt_other_addressing_forms_load_gdtr() {
    // [RBX+0x10] (disp8), [RIP+disp32] (RIP-relative), [RAX+RCX*1].
    let cases: [(&[u8], u64); 3] = [
        (&[0x0f, 0x01, 0x53, 0x10], LGDT_DATA - 0x10),
        (&[0x0f, 0x01, 0x15, 0xf9, 0x0f, 0x00, 0x00], 0),
        (&[0x0f, 0x01, 0x14, 0x08], LGDT_DATA - 5),
    ];
    for (code, register) in cases {
        let mut state = lgdt_state(register);
        state.gpr[3] = register; // RBX
        state.gpr[1] = 5; // RCX
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x0fff, 0x0000_7fff_ffff_f000));
        // RIP-relative: RIP + 7 + 0xff9 = 0x2000.
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(result.instructions_executed, 1, "{code:x?}");
        assert_eq!(
            state.gdtr,
            Some(od_isa::state::Gdtr {
                base: 0x0000_7fff_ffff_f000,
                limit: 0x0fff
            }),
            "{code:x?}"
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lgdt_with_a_register_operand_stays_unsupported() {
    // `0F 01 D0` (mod = 11, /2) is XGETBV, not an LGDT form: it is not decoded as
    // one and stays Unsupported (no #UD classification invented for it).
    let mut state = lgdt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0xd0], &pseudo_descriptor(0x1f, GDT_BASE));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 4);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x01 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lock_lgdt_is_ud_in_the_set_and_unsupported_elsewhere() {
    for code in [
        &[0xf0u8, 0x0f, 0x01, 0x10][..],
        &[0xf0, 0x0f, 0x01, 0x53, 0x10],
        &[0xf0, 0x0f, 0x01, 0x14, 0x08],
    ] {
        let mut state = lgdt_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, GDT_BASE));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 4);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None
            }),
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());

        // Outside the set the form keeps its previous Unsupported exit.
        let mut state = lgdt_legacy_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, GDT_BASE));
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x01 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn lgdt_with_a_non_canonical_base_is_gp0_without_mutation() {
    // Bit 63 differs from bit 47 (SDM Vol. 3 §3.5.1: the GDTR base is a linear
    // address; AMD APM Vol. 2 LGDT lists #GP for a non-canonical base).
    for bad_base in [0x8000_0000_0000_0000u64, 0x0001_0000_0000_0000] {
        let mut state = lgdt_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, bad_base));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn lgdt_with_a_non_canonical_operand_address_faults_by_segment() {
    // DS-based operand: #GP(0); SS-based (RBP/RSP base): #SS(0).
    for (code, register, vector) in [
        (&[0x0f, 0x01, 0x10][..], 0x0001_0000_0000_0000u64, 13u8),
        (&[0x0f, 0x01, 0x55, 0x00], 0x0001_0000_0000_0000, 12),
        // Canonical first byte, non-canonical last byte of the ten-byte operand.
        (&[0x0f, 0x01, 0x10], 0x0000_7fff_ffff_fffa, 13),
    ] {
        let mut state = lgdt_state(register);
        state.gpr[5] = register; // RBP
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, GDT_BASE));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: Some(0),
                linear_address: None
            }),
            "{code:x?}"
        );
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn lgdt_unmapped_operand_reports_the_page_fault_without_mutation() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(LGDT_DATA),
    };
    let mut state = lgdt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, GDT_BASE));
    memory.fail_page = Some((LGDT_DATA, MemoryError::Architectural(fault.clone())));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn lgdt_second_read_fault_leaves_gdtr_unchanged() {
    // The operand starts in a mapped page and its tail is in an unmapped one: the
    // second qword read faults, the first one has no architectural effect.
    let start = LGDT_DATA + 0xff8;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(LGDT_DATA + 0x1000),
    };
    let mut state = lgdt_state(start);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, GDT_BASE));
    memory.fail_page = Some((
        LGDT_DATA + 0x1000,
        MemoryError::Architectural(fault.clone()),
    ));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads.len(), 2);
}

#[test]
fn lgdt_page_crossing_backend_classification_passes_through_without_mutation() {
    // The board adapter reports a 4 KiB crossing as a backend error (S4-11); the set
    // surfaces it as MemoryError exactly as for MOV and does not touch GDTR.
    let start = LGDT_DATA + 0xff8;
    let mut state = lgdt_state(start);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, GDT_BASE));
    memory.fail_page = Some((
        LGDT_DATA + 0x1000,
        MemoryError::Backend("cross-page qword read".to_string()),
    ));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(
        exit(&result),
        &RunExit::MemoryError(MemoryError::Backend("cross-page qword read".to_string()))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn lgdt_below_cpl0_is_rejected_at_set_entry() {
    let mut state = lgdt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, GDT_BASE));
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lgdt_without_a_modeled_gdtr_stays_unsupported() {
    let mut state = lgdt_state(LGDT_DATA);
    state.gdtr = None;
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10], &pseudo_descriptor(0x1f, GDT_BASE));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x01 });
    assert_eq!(state, before);
}

#[test]
fn lgdt_outside_the_set_stays_unsupported_for_general_forms() {
    // `run` without the set: `LGDT [RAX]`, `[RBX+disp8]` and RIP-relative forms keep
    // the Unsupported exit and read nothing; the legacy `[RBP+disp8]` form still loads.
    for code in [
        &[0x0fu8, 0x01, 0x10][..],
        &[0x0f, 0x01, 0x53, 0x10],
        &[0x0f, 0x01, 0x15, 0xf9, 0x0f, 0x00, 0x00],
    ] {
        let mut state = lgdt_legacy_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, GDT_BASE));
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x01 },
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
    let mut state = lgdt_legacy_state(0);
    state.gpr[5] = LGDT_DATA - 0x10;
    let mut memory = LgdtMemory::new(
        &[0x0f, 0x01, 0x55, 0x10],
        &pseudo_descriptor(0x1f, GDT_BASE),
    );
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.gdtr,
        Some(od_isa::state::Gdtr {
            base: GDT_BASE,
            limit: 0x1f
        })
    );
}

// ---- S2-49: LIDT m (`0F 01 /3`) in the CPL0 profile set ----

const IDT_BASE: u64 = 0x0000_0000_0013_0000;
const OLD_IDTR: od_isa::state::Idtr = od_isa::state::Idtr {
    base: 0x8000,
    limit: 0x000f,
};

fn lidt_state(rax: u64) -> LongModeState {
    let mut state = lgdt_state(rax);
    state.idtr = Some(OLD_IDTR);
    state
}

/// `lidt_state` without the modeling that only the set accepts: what `run` accepts.
fn lidt_legacy_state(rax: u64) -> LongModeState {
    LongModeState {
        idtr: Some(OLD_IDTR),
        ..lgdt_legacy_state(rax)
    }
}

fn idt_ud() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    })
}

#[test]
fn lidt_rax_loads_idtr_from_the_pseudo_descriptor() {
    let mut state = lidt_state(LGDT_DATA);
    let mut before = state.clone();
    let mut memory = LgdtMemory::new(
        &[0x0f, 0x01, 0x18, 0x90],
        &pseudo_descriptor(0x0fff, IDT_BASE),
    );
    // One instruction, then the budget ends before the NOP byte is decoded.
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.idtr,
        Some(od_isa::state::Idtr {
            base: IDT_BASE,
            limit: 0x0fff
        })
    );
    assert_eq!(state.rip, RIP + 3);
    // Only RIP and IDTR changed: GDTR keeps its old value.
    before.rip = RIP + 3;
    before.idtr = state.idtr;
    assert_eq!(state, before);
    assert_eq!(state.gdtr, Some(OLD_GDTR));
    // Two qword reads inside the ten-byte operand, no write (the IDT is not touched).
    assert_eq!(memory.reads, vec![(LGDT_DATA, 8), (LGDT_DATA + 2, 8)]);
}

#[test]
fn lidt_other_addressing_forms_load_idtr() {
    // [RBX+0x10] (disp8), [RIP+disp32] (the real kernel's `0F 01 1D` form), [RAX+RCX*1],
    // and the legacy [RSP+disp8] form.
    let cases: [(&[u8], u64); 4] = [
        (&[0x0f, 0x01, 0x5b, 0x10], LGDT_DATA - 0x10),
        (&[0x0f, 0x01, 0x1d, 0xf9, 0x0f, 0x00, 0x00], 0),
        (&[0x0f, 0x01, 0x1c, 0x08], LGDT_DATA - 5),
        (&[0x0f, 0x01, 0x5c, 0x24, 0x10], 0),
    ];
    for (code, register) in cases {
        let mut state = lidt_state(register);
        state.gpr[3] = register; // RBX
        state.gpr[1] = 5; // RCX
        state.gpr[4] = LGDT_DATA - 0x10; // RSP
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x0fff, 0x0000_7fff_ffff_f000));
        // RIP-relative: RIP + 7 + 0xff9 = 0x2000.
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(result.instructions_executed, 1, "{code:x?}");
        assert_eq!(
            state.idtr,
            Some(od_isa::state::Idtr {
                base: 0x0000_7fff_ffff_f000,
                limit: 0x0fff
            }),
            "{code:x?}"
        );
        assert_eq!(state.gdtr, Some(OLD_GDTR));
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lidt_loads_idtr_when_the_entry_state_does_not_model_one() {
    // The Linux entry state has `idtr: None`; LIDT replaces the whole register.
    let mut state = lidt_state(LGDT_DATA);
    state.idtr = None;
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x0fff, IDT_BASE));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.idtr,
        Some(od_isa::state::Idtr {
            base: IDT_BASE,
            limit: 0x0fff
        })
    );
}

#[test]
fn lidt_with_a_register_operand_is_not_lidt_and_stays_unsupported() {
    // `0F 01 D8` (mod = 11, /3) is VMRUN, not an LIDT form: it is not decoded as
    // one and stays Unsupported (no #UD classification invented for it).
    let mut state = lidt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0xd8], &pseudo_descriptor(0x1f, IDT_BASE));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 4);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x01 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lock_lidt_is_ud_in_the_set_and_unsupported_elsewhere() {
    for code in [
        &[0xf0u8, 0x0f, 0x01, 0x18][..],
        &[0xf0, 0x0f, 0x01, 0x5b, 0x10],
        &[0xf0, 0x0f, 0x01, 0x1c, 0x08],
        &[0xf0, 0x0f, 0x01, 0x1d, 0, 0, 0, 0],
        // The legacy `LOCK LIDT [RSP+disp8]` keeps its Invalid classification: also #UD.
        &[0xf0, 0x0f, 0x01, 0x5c, 0x24, 0x10],
    ] {
        let mut state = lidt_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, IDT_BASE));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 4);
        assert_eq!(exit(&result), &idt_ud(), "{code:x?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
    // Outside the set the general forms keep their previous Unsupported exit.
    for code in [
        &[0xf0u8, 0x0f, 0x01, 0x18][..],
        &[0xf0, 0x0f, 0x01, 0x5b, 0x10],
        &[0xf0, 0x0f, 0x01, 0x1c, 0x08],
    ] {
        let mut state = lidt_legacy_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, IDT_BASE));
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x01 },
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn lidt_with_a_non_canonical_base_is_gp0_without_mutation() {
    for bad_base in [0x8000_0000_0000_0000u64, 0x0001_0000_0000_0000] {
        let mut state = lidt_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, bad_base));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
    // Both canonical boundaries load.
    for good_base in [0x0000_7fff_ffff_ffffu64, 0xffff_8000_0000_0000] {
        let mut state = lidt_state(LGDT_DATA);
        let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, good_base));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.idtr.unwrap().base, good_base);
    }
}

#[test]
fn lidt_with_a_non_canonical_operand_address_faults_by_segment() {
    // DS-based operand: #GP(0); SS-based (RBP/RSP base): #SS(0).
    for (code, register, vector) in [
        (&[0x0f, 0x01, 0x18][..], 0x0001_0000_0000_0000u64, 13u8),
        (&[0x0f, 0x01, 0x5d, 0x00], 0x0001_0000_0000_0000, 12),
        // Canonical first byte, non-canonical last byte of the ten-byte operand.
        (&[0x0f, 0x01, 0x18], 0x0000_7fff_ffff_fffa, 13),
    ] {
        let mut state = lidt_state(register);
        state.gpr[5] = register; // RBP
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, IDT_BASE));
        let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: Some(0),
                linear_address: None
            }),
            "{code:x?}"
        );
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn lidt_unmapped_operand_reports_the_page_fault_without_mutation() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(LGDT_DATA),
    };
    let mut state = lidt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, IDT_BASE));
    memory.fail_page = Some((LGDT_DATA, MemoryError::Architectural(fault.clone())));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn lidt_second_read_fault_and_page_crossing_leave_idtr_unchanged() {
    // The operand starts in a mapped page and its tail is in an unmapped one: the
    // second qword read faults, the first one has no architectural effect.
    let start = LGDT_DATA + 0xff8;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(LGDT_DATA + 0x1000),
    };
    let mut state = lidt_state(start);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, IDT_BASE));
    memory.fail_page = Some((
        LGDT_DATA + 0x1000,
        MemoryError::Architectural(fault.clone()),
    ));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads.len(), 2);

    // The board adapter reports a 4 KiB crossing as a backend error (S4-11); the set
    // surfaces it as MemoryError exactly as for LGDT and does not touch IDTR.
    let mut state = lidt_state(start);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, IDT_BASE));
    memory.fail_page = Some((
        LGDT_DATA + 0x1000,
        MemoryError::Backend("cross-page qword read".to_string()),
    ));
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 1);
    assert_eq!(
        exit(&result),
        &RunExit::MemoryError(MemoryError::Backend("cross-page qword read".to_string()))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn lidt_below_cpl0_is_rejected_at_set_entry() {
    let mut state = lidt_state(LGDT_DATA);
    let before = state.clone();
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x18], &pseudo_descriptor(0x1f, IDT_BASE));
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lidt_outside_the_set_stays_unsupported_for_general_forms() {
    // `run` without the set: `LIDT [RAX]`, `[RBX+disp8]`, RIP-relative and a SIB form
    // other than the legacy one keep the Unsupported exit and read nothing; the legacy
    // `[RSP+disp8]` form still loads.
    for code in [
        &[0x0fu8, 0x01, 0x18][..],
        &[0x0f, 0x01, 0x5b, 0x10],
        &[0x0f, 0x01, 0x1d, 0xf9, 0x0f, 0x00, 0x00],
        &[0x0f, 0x01, 0x1c, 0x24],
        &[0x0f, 0x01, 0x5c, 0x25, 0x10],
        &[0x0f, 0x01, 0x5c, 0xa4, 0x10],
    ] {
        let mut state = lidt_legacy_state(LGDT_DATA);
        let before = state.clone();
        let mut memory = LgdtMemory::new(code, &pseudo_descriptor(0x1f, IDT_BASE));
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 4),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x01 },
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
    let mut state = lidt_legacy_state(0);
    state.gpr[4] = LGDT_DATA - 0x10; // RSP
    let mut memory = LgdtMemory::new(
        &[0x0f, 0x01, 0x5c, 0x24, 0x10],
        &pseudo_descriptor(0x1f, IDT_BASE),
    );
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.idtr,
        Some(od_isa::state::Idtr {
            base: IDT_BASE,
            limit: 0x1f
        })
    );
    // Without a modeled IDTR the legacy path stays Unsupported (the set loads it).
    let mut state = lidt_legacy_state(0);
    state.idtr = None;
    state.gpr[4] = LGDT_DATA - 0x10;
    let mut memory = LgdtMemory::new(
        &[0x0f, 0x01, 0x5c, 0x24, 0x10],
        &pseudo_descriptor(0x1f, IDT_BASE),
    );
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x01 });
    assert_eq!(state.idtr, None);
}

#[test]
fn lgdt_then_lidt_sequence_retires_two_instructions_and_loads_both_tables() {
    // `LGDT [RAX]; LIDT [RBX]` over two descriptors in the data window.
    let mut state = lidt_state(LGDT_DATA);
    state.gpr[3] = LGDT_DATA + 0x20; // RBX
    let mut data = pseudo_descriptor(0x001f, GDT_BASE);
    data.resize(0x20, 0);
    data.extend(pseudo_descriptor(0x0fff, IDT_BASE));
    let mut memory = LgdtMemory::new(&[0x0f, 0x01, 0x10, 0x0f, 0x01, 0x1b], &data);
    let result = run_lgdt_set(&mut state, &mut memory, &base(), 2);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(
        state.gdtr,
        Some(od_isa::state::Gdtr {
            base: GDT_BASE,
            limit: 0x1f
        })
    );
    assert_eq!(
        state.idtr,
        Some(od_isa::state::Idtr {
            base: IDT_BASE,
            limit: 0x0fff
        })
    );
    assert_eq!(state.rip, RIP + 6);
}

// ---- S2-47b: LRETQ with a CS reload from the loaded GDT ----

const RAM_BASE: u64 = 0x1_0000;
const RAM_SIZE: usize = 0x4000;
const STACK: u64 = RAM_BASE + 0x1000;
const LRETQ_GDT: u64 = RAM_BASE + 0x2000;
const LRETQ: [u8; 2] = [0x48, 0xcb];
const CODE64: u64 = 0x00af_9b00_0000_ffff;

/// Code at `RIP` and one RAM window (stack and GDT). A read or write that touches a page
/// in `fault_pages` returns the architectural #PF; every access is recorded.
struct LretqMemory {
    code: Vec<u8>,
    ram: Vec<u8>,
    fault_pages: Vec<u64>,
    /// Pages whose writes (only) fault.
    write_fault_pages: Vec<u64>,
    reads: Vec<(u64, usize, AccessOrigin)>,
    writes: usize,
    write_log: Vec<(u64, Vec<u8>, AccessOrigin)>,
    /// The write with this one-based number (counted in `writes`) faults with a #PF.
    fail_write_number: Option<usize>,
}

impl LretqMemory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            ram: vec![0; RAM_SIZE],
            fault_pages: Vec::new(),
            write_fault_pages: Vec::new(),
            reads: Vec::new(),
            writes: 0,
            write_log: Vec::new(),
            fail_write_number: None,
        }
    }

    fn put(&mut self, address: u64, value: u64) {
        let offset = (address - RAM_BASE) as usize;
        self.ram[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn page_fault(&self, address: u64, length: usize) -> Option<MemoryError> {
        let first = address & !0xfff;
        let last = (address + length as u64 - 1) & !0xfff;
        self.fault_pages
            .iter()
            .find(|page| (first..=last).contains(page))
            .map(|page| {
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(address.max(*page)),
                })
            })
    }
}

impl GuestMemory for LretqMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.code
            .get(address.wrapping_sub(RIP) as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads
            .push((address, destination.len(), context.origin));
        if let Some(error) = self.page_fault(address, destination.len()) {
            return Err(error);
        }
        for (index, byte) in destination.iter_mut().enumerate() {
            *byte = self.ram[(address + index as u64 - RAM_BASE) as usize];
        }
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.writes += 1;
        if self.fail_write_number == Some(self.writes) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(address),
            }));
        }
        if let Some(error) = self.page_fault(address, source.len()) {
            return Err(error);
        }
        if self.write_fault_pages.contains(&(address & !0xfff)) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(address),
            }));
        }
        self.write_log
            .push((address, source.to_vec(), context.origin));
        let offset = (address - RAM_BASE) as usize;
        self.ram[offset..offset + source.len()].copy_from_slice(source);
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

fn lretq_state(selector: u64, target: u64) -> (LongModeState, LretqMemory) {
    let mut state = board_state();
    state.gdtr = Some(od_isa::state::Gdtr {
        base: LRETQ_GDT,
        limit: 0x1f,
    });
    state.gpr[4] = STACK;
    let mut memory = LretqMemory::new(&LRETQ);
    memory.put(STACK, target);
    memory.put(STACK + 8, selector);
    memory.put(LRETQ_GDT + 8, 0x00cf_9300_0000_ffff);
    memory.put(LRETQ_GDT + 0x10, CODE64);
    memory.put(LRETQ_GDT + 0x18, 0x00cf_9300_0000_ffff);
    (state, memory)
}

fn run_lretq(state: &mut LongModeState, memory: &mut LretqMemory) -> Cpl0ProfileSetResult {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap()
}

fn selector_fault_exit(vector: u8, selector: u32) -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector,
        error_code: Some(selector),
        linear_address: None,
    })
}

/// Run `LRETQ` on a frame `[target, selector]` with `descriptor` at GDT index 2 and
/// expect `expected` with no CPU or memory change.
fn assert_lretq_refused(descriptor: u64, selector: u64, target: u64, expected: RunExit) {
    let (mut state, mut memory) = lretq_state(selector, target);
    memory.put(LRETQ_GDT + 0x10, descriptor);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &expected, "{descriptor:#x} {selector:#x}");
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 0);
}

#[test]
fn lretq_loads_cs_from_the_gdt_pops_the_frame_and_retires() {
    // The target is the NOP after LRETQ; budget 2 retires both.
    let mut code = LRETQ.to_vec();
    code.push(0x90);
    for old_cs in [None, Some((0x08u16, 0x9b_u8))] {
        let (mut state, mut memory) = lretq_state(0x10, RIP + 2);
        memory.code = code.clone();
        if let Some((selector, access)) = old_cs {
            state.cs_selector = Some(selector);
            state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access,
                flags: 0xa,
            });
        }
        let mut expected = state.clone();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 2),
            od_isa::INTEGER_FLAGS,
            &base(),
            None,
            || false,
        )
        .unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2);
        expected.rip = RIP + 3;
        expected.cs_selector = Some(0x10);
        expected.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        });
        expected.gpr[4] = STACK + 16;
        assert_eq!(state, expected);
        // RFLAGS (IF included) are untouched.
        assert_eq!(state.rflags, 2 | IF);
        // Two stack qwords, then the descriptor qword from the GDT as a descriptor-table
        // access; nothing is written.
        assert_eq!(
            memory.reads,
            vec![
                (STACK, 8, AccessOrigin::Explicit),
                (STACK + 8, 8, AccessOrigin::Explicit),
                (LRETQ_GDT + 0x10, 8, AccessOrigin::DescriptorTable),
            ]
        );
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn lretq_uses_only_the_low_sixteen_bits_of_the_cs_qword() {
    let (mut state, mut memory) = lretq_state(0xdead_beef_0000_0010, RIP + 2);
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cs_selector, Some(0x10));
}

#[test]
fn lretq_follows_the_kernel_push_lea_push_sequence() {
    // `push 0x10; lea rax,[rip+3]; push rax; lretq; nop`: the real kernel's far jump.
    let code = [
        0x6a, 0x10, 0x48, 0x8d, 0x05, 3, 0, 0, 0, 0x50, 0x48, 0xcb, 0x90,
    ];
    let (mut state, mut memory) = lretq_state(0, 0);
    memory.code = code.to_vec();
    state.gpr[4] = STACK + 0x100;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 5),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 5);
    assert_eq!(state.rip, RIP + 13);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.gpr[4], STACK + 0x100);
}

#[test]
fn lretq_rejects_wrong_descriptors_with_exact_faults_and_no_mutation() {
    // Reserved long-mode code: L = 1 and D = 1.
    assert_lretq_refused(
        0x00ef_9b00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    // Data descriptor and system descriptor (64-bit TSS type 9, S = 0).
    assert_lretq_refused(
        0x00cf_9300_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    assert_lretq_refused(
        0x00af_8900_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    // Code with DPL 3 under RPL 0 (conforming or not).
    assert_lretq_refused(
        0x00af_fb00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    assert_lretq_refused(
        0x00af_ff00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    // Not present: #NP(selector), vector 11.
    assert_lretq_refused(
        0x00af_1b00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(11, 0x10),
    );
}

#[test]
fn lretq_selector_checks_follow_the_gdt_limit_and_null_rules() {
    // Null selector: #GP(0). Index 4 (0x20) ends beyond limit 0x1f: #GP(selector).
    assert_lretq_refused(CODE64, 0, RIP, selector_fault_exit(13, 0));
    assert_lretq_refused(CODE64, 3, RIP, selector_fault_exit(13, 0));
    assert_lretq_refused(CODE64, 0x20, RIP, selector_fault_exit(13, 0x20));
    // A limit that cuts the descriptor in the middle (0x10..0x17 needs limit >= 0x17).
    let (mut state, mut memory) = lretq_state(0x10, RIP);
    state.gdtr = Some(od_isa::state::Gdtr {
        base: LRETQ_GDT,
        limit: 0x16,
    });
    let before = state.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &selector_fault_exit(13, 0x10));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    // The stack was read, the GDT was not.
    assert_eq!(memory.reads.len(), 2);
}

#[test]
fn lretq_non_canonical_target_is_gp0_after_the_descriptor_checks() {
    assert_lretq_refused(
        CODE64,
        0x10,
        0x0001_0000_0000_0000,
        selector_fault_exit(13, 0),
    );
    // A bad descriptor wins over a bad target.
    assert_lretq_refused(
        0x00ef_9b00_0000_ffff,
        0x10,
        0x0001_0000_0000_0000,
        selector_fault_exit(13, 0x10),
    );
}

#[test]
fn lretq_unmodeled_legal_returns_are_unsupported_without_mutation() {
    let unsupported = RunExit::Unsupported { opcode: 0xcb };
    // Compatibility-mode CS (L = 0, D = 1 or 0): legal but switches the instruction mode.
    assert_lretq_refused(0x00cf_9b00_0000_ffff, 0x10, RIP, unsupported.clone());
    assert_lretq_refused(0x008f_9b00_0000_ffff, 0x10, RIP, unsupported.clone());
    // RPL 3 (inter-privilege return) and a TI = 1 (LDT) selector.
    assert_lretq_refused(CODE64, 0x13, RIP, unsupported.clone());
    assert_lretq_refused(CODE64, 0x14, RIP, unsupported.clone());
}

#[test]
fn lretq_without_a_modeled_gdtr_is_unsupported() {
    let (mut state, mut memory) = lretq_state(0x10, RIP);
    state.gdtr = None;
    let before = state.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xcb });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lretq_stack_page_faults_are_exact_and_leave_state_untouched() {
    // First qword unmapped, second qword unmapped (frame crosses into a bad page), and the
    // GDT descriptor page unmapped.
    for (rsp, bad_page) in [
        (STACK, STACK),
        (STACK + 0xff8, STACK + 0x1000),
        (LRETQ_GDT + 0x10, LRETQ_GDT),
    ] {
        let (mut state, mut memory) = lretq_state(0x10, RIP);
        if rsp == LRETQ_GDT + 0x10 {
            // Frame in a good page, descriptor read faults.
            memory.fault_pages.push(bad_page);
            state.gpr[4] = STACK;
        } else {
            memory.put(rsp, RIP);
            if rsp != STACK {
                memory.put(rsp + 8, 0x10);
            }
            memory.fault_pages.push(bad_page);
            state.gpr[4] = rsp;
        }
        let before = state.clone();
        let result = run_lretq(&mut state, &mut memory);
        let RunExit::Fault(fault) = exit(&result) else {
            panic!(
                "expected a page fault for {rsp:#x}, got {:?}",
                exit(&result)
            );
        };
        assert_eq!(fault.vector, 14, "{rsp:#x}");
        assert_eq!(fault.error_code, Some(0));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn lretq_non_canonical_rsp_is_ss0_before_any_read() {
    let (mut state, mut memory) = lretq_state(0x10, RIP);
    state.gpr[4] = 0x0001_0000_0000_0000;
    let before = state.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &selector_fault_exit(12, 0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn lretq_outside_the_set_and_with_other_prefixes_stays_unsupported() {
    // `run` without the set: `48 CB` keeps the old exit and reads nothing.
    let (mut state, mut memory) = lretq_state(0x10, RIP);
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xcb });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
    // Inside the set only the exact REX.W form decodes.
    for code in [
        &[0xcbu8][..],
        &[0x66, 0x48, 0xcb],
        &[0x49, 0xcb],
        &[0x4b, 0xcb],
        &[0xf0, 0x48, 0xcb],
        &[0x2e, 0x48, 0xcb],
        &[0x67, 0x48, 0xcb],
    ] {
        let (mut state, mut memory) = lretq_state(0x10, RIP);
        memory.code = code.to_vec();
        let before = state.clone();
        let result = run_lretq(&mut state, &mut memory);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xcb },
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn lretq_sets_the_accessed_bit_with_one_qword_gdt_write() {
    // The real kernel's early GDT holds `0x00af9a000000ffff` (A = 0): a CS load writes the
    // accessed bit back (SDM Vol.3 §3.4.5.1), here as one descriptor-table qword write.
    let (mut state, mut memory) = lretq_state(0x10, RIP + 2);
    memory.put(LRETQ_GDT + 0x10, 0x00af_9a00_0000_ffff);
    let mut expected = state.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    expected.rip = RIP + 2;
    expected.cs_selector = Some(0x10);
    expected.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xa,
    });
    expected.gpr[4] = STACK + 16;
    assert_eq!(state, expected);
    assert_eq!(
        memory.write_log,
        vec![(
            LRETQ_GDT + 0x10,
            0x00af_9b00_0000_ffffu64.to_le_bytes().to_vec(),
            AccessOrigin::DescriptorTable
        )]
    );
}

#[test]
fn lretq_accessed_bit_write_fault_is_exact_and_leaves_the_cpu_untouched() {
    let (mut state, mut memory) = lretq_state(0x10, RIP);
    memory.put(LRETQ_GDT + 0x10, 0x00af_9a00_0000_ffff);
    memory.write_fault_pages.push(LRETQ_GDT);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(LRETQ_GDT + 0x10)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
}

#[test]
fn lretq_faults_before_the_descriptor_never_write_the_accessed_bit() {
    // Every refusal below fails with A = 0 and must name its exact exit; none of them may
    // reach the accessed-bit write.
    for (descriptor, target, expected) in [
        // L = 1 and D = 1.
        (0x00ef_9a00_0000_ffffu64, RIP, selector_fault_exit(13, 0x10)),
        // Not present.
        (0x00af_1a00_0000_ffff, RIP, selector_fault_exit(11, 0x10)),
        // DPL 3.
        (0x00af_fa00_0000_ffff, RIP, selector_fault_exit(13, 0x10)),
        // Non-canonical target.
        (
            0x00af_9a00_0000_ffff,
            0x0001_0000_0000_0000,
            selector_fault_exit(13, 0),
        ),
        // Compatibility-mode CS: legal but not modeled.
        (
            0x00cf_9a00_0000_ffff,
            RIP,
            RunExit::Unsupported { opcode: 0xcb },
        ),
    ] {
        let (mut state, mut memory) = lretq_state(0x10, target);
        memory.put(LRETQ_GDT + 0x10, descriptor);
        let before = state.clone();
        let result = run_lretq(&mut state, &mut memory);
        assert_eq!(exit(&result), &expected, "{descriptor:#x}");
        assert_eq!(result.instructions_executed, 0, "{descriptor:#x}");
        assert_eq!(state, before);
        assert_eq!(memory.writes, 0, "{descriptor:#x}");
    }
}

#[test]
fn lretq_reserved_l_and_d_wins_over_not_present() {
    // SDM Vol.2 RET (IA-32e): the L = 1 / D = 1 check comes before the present check, so a
    // not-present reserved descriptor is #GP(selector), not #NP.
    assert_lretq_refused(
        0x00ef_1a00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(13, 0x10),
    );
    // The same descriptor with D = 0 is a plain not-present 64-bit code segment: #NP.
    assert_lretq_refused(
        0x00af_1a00_0000_ffff,
        0x10,
        RIP,
        selector_fault_exit(11, 0x10),
    );
}

/// Code bytes followed by an unmapped page: a fetch past the end is an architectural `#PF`.
struct PageFaultAfterCode {
    bytes: Vec<u8>,
}

impl GuestMemory for PageFaultAfterCode {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address.wrapping_sub(RIP) as usize)
            .copied()
            .ok_or(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(address),
            }))
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

#[test]
fn lgdt_and_endbr64_fetch_their_operand_bytes_before_deciding_unsupported() {
    // Known deviation (S2-47a / S2-48): the decoder now fetches the SIB and displacement of
    // `0F 01 /2` and the ModRM of `F3 0F 1E` before it decides the instruction is outside
    // the legacy path. When the page ends inside the instruction, the legacy `run` reports
    // the architectural `#PF` of the fetch (correct hardware behavior) instead of the former
    // `Unsupported`. The behavior is pinned here.
    for (code, last_mapped) in [
        // `LGDT [SIB ...]`: the SIB byte is on the unmapped page.
        (&[0x0fu8, 0x01, 0x14][..], 3u64),
        // `LGDT [disp32]`: the displacement is cut after one byte.
        (&[0x0f, 0x01, 0x15, 0xf9][..], 4),
        // `F3 0F 1E`: the ModRM byte is on the unmapped page.
        (&[0xf3, 0x0f, 0x1e][..], 3),
    ] {
        let mut state = lgdt_legacy_state(0);
        let before = state.clone();
        let mut memory = PageFaultAfterCode {
            bytes: code.to_vec(),
        };
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x10),
                linear_address: Some(RIP + last_mapped),
            }),
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

const ENDBR64: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfa];

fn endbr_state() -> LongModeState {
    // Loaded caches and an unusual arithmetic flag pattern: ENDBR64 must not touch any of it.
    let mut state = loaded_state();
    state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
    state.gpr[4] = 0x7fff_f000;
    state
}

#[test]
fn endbr64_in_the_set_is_a_four_byte_nop() {
    // S2-48: no CET_IBT is declared, so the landing pad executes as a NOP.
    let mut state = endbr_state();
    let mut before = state.clone();
    before.rip += 4;
    let mut memory = Memory::code(&ENDBR64);
    let defined = 0x1 | 0x40;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        defined,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, defined);
    assert_eq!(state.rip, RIP + 4);
    assert_eq!(state, before);
}

#[test]
fn endbr64_then_push_rbp_retires_both() {
    // The kernel function prologue `endbr64; push rbp`.
    let (mut state, mut memory) = lretq_state(0, 0);
    memory.code = vec![0xf3, 0x0f, 0x1e, 0xfa, 0x55];
    state.gpr[4] = STACK + 0x100;
    state.gpr[5] = 0xdead_beef_cafe_f00d;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 2),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 5);
    assert_eq!(state.gpr[4], STACK + 0xf8);
    let offset = (STACK + 0xf8 - RAM_BASE) as usize;
    assert_eq!(
        memory.ram[offset..offset + 8],
        0xdead_beef_cafe_f00du64.to_le_bytes()
    );
}

#[test]
fn endbr64_lookalikes_stay_unsupported_in_the_set() {
    for code in [
        &[0xf3u8, 0x0f, 0x1e, 0xfb][..],
        &[0xf3, 0x0f, 0x1e, 0xf8],
        &[0x66, 0xf3, 0x0f, 0x1e, 0xfa],
        &[0xf3, 0x66, 0x0f, 0x1e, 0xfa],
        &[0xf2, 0x0f, 0x1e, 0xfa],
        &[0xf3, 0x48, 0x0f, 0x1e, 0xfa],
        &[0xf3, 0x2e, 0x0f, 0x1e, 0xfa],
        &[0xf3, 0x67, 0x0f, 0x1e, 0xfa],
        &[0x0f, 0x1e, 0xfa],
    ] {
        let mut state = endbr_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x1e },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn endbr64_outside_the_set_stays_unsupported() {
    let mut state = endbr_state();
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let mut memory = Memory::code(&ENDBR64);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x1e });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

// ---- S4-38L4d: multi-byte NOP `0F 1F /0` ----

fn run_nop_0f1f(code: &[u8]) -> (LongModeState, LongModeState, Cpl0ProfileSetResult) {
    // Pointers that cannot be dereferenced (unmapped / non-canonical) and an unusual flag
    // pattern: the NOP must not touch the operand, the registers or the flags.
    let mut state = endbr_state();
    state.gpr[0] = 0xffff_ffff_dead_0000;
    state.gpr[1] = 0x0000_8000_0000_0000;
    state.gpr[2] = 0x1234_5678;
    let mut before = state.clone();
    before.rip += code.len() as u64;
    let mut memory = Memory::code(code);
    let defined = 0x1 | 0x40;
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        defined,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(result.rflags_defined, defined, "{code:02x?}");
    (state, before, result)
}

#[test]
fn nop_0f1f_forms_are_architectural_nops_of_their_decoded_length() {
    for code in [
        &[0x0fu8, 0x1f, 0x00][..],                               // NOP DWORD [RAX]
        &[0x0f, 0x1f, 0x40, 0x00],                               // disp8
        &[0x0f, 0x1f, 0x44, 0x00, 0x00],                         // SIB + disp8
        &[0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00],             // disp32
        &[0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],       // SIB + disp32
        &[0x66, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00], // 66 prefix
        &[
            0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00,
        ], // kernel pad
        &[0x66, 0x2e, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00], // 66 CS
        &[0x48, 0x0f, 0x1f, 0x00],                               // REX.W
        &[0x4d, 0x0f, 0x1f, 0x04, 0x24],                         // REX.WRB, SIB
        &[0x67, 0x0f, 0x1f, 0x00],                               // 67 address override
        &[0x3e, 0x0f, 0x1f, 0x00],                               // DS
        &[0x0f, 0x1f, 0x05, 0x00, 0x00, 0x00, 0x00],             // RIP-relative
        &[0x0f, 0x1f, 0x04, 0x25, 0x00, 0x00, 0x00, 0x00],       // SIB no base, disp32
        &[0x0f, 0x1f, 0x01],       // [rcx] with non-canonical RCX: no #GP, no #PF
        &[0x0f, 0x1f, 0x04, 0x08], // [rax+rcx], non-canonical index
        &[0x0f, 0x1f, 0xc0],       // register form
        &[0x0f, 0x1f, 0xc1],
    ] {
        let (state, before, result) = run_nop_0f1f(code);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn nop_0f1f_then_next_instruction_continues_at_the_decoded_length() {
    // The kernel padding followed by `mov rax, rax`-like next bytes: two retirements.
    let mut code = vec![0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0];
    code.extend([0x48, 0x89, 0xc0]); // mov rax, rax
    let mut state = endbr_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &base(), 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 14);
}

#[test]
fn nop_0f1f_lookalikes_stay_unsupported_in_the_set() {
    for code in [
        &[0x0fu8, 0x1f, 0x08][..],       // /1
        &[0x0f, 0x1f, 0x38],             // /7
        &[0x0f, 0x1f, 0xc8],             // /1 register form
        &[0xf0, 0x0f, 0x1f, 0x00],       // LOCK
        &[0xf3, 0x0f, 0x1f, 0x00],       // REP
        &[0xf2, 0x0f, 0x1f, 0x00],       // REPNE
        &[0x66, 0xf0, 0x0f, 0x1f, 0x00], // LOCK after 66
        &[0x2e, 0x2e, 0x0f, 0x1f, 0x00], // duplicate segment
        &[0x67, 0x67, 0x0f, 0x1f, 0x00], // duplicate address override
        &[0x40, 0x41, 0x0f, 0x1f, 0x00], // duplicate REX
    ] {
        let mut state = endbr_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x1f },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn nop_0f1f_outside_the_set_stays_unsupported() {
    let mut state = endbr_state();
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x1f, 0x00]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x1f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn nop_0f1f_truncated_operand_bytes_report_the_fetch_error() {
    // `0F 1F 84` needs a SIB and disp32; the page ends after the ModRM byte.
    let mut state = endbr_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x1f, 0x84]);
    let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::MemoryError(MemoryError::Unmapped {
            address: RIP + 3,
            access: AccessKind::Fetch,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

// ---- S4-38L6a: PREFETCH hints `0F 18 /0../7 m` ----

#[test]
fn prefetch_0f18_forms_are_architectural_nops_of_their_decoded_length() {
    let mut forms: Vec<Vec<u8>> = vec![
        vec![0x0f, 0x18, 0x0a],             // PREFETCHT0 [RDX] (kernel boot form)
        vec![0x0f, 0x18, 0x4a, 0x40],       // PREFETCHT0 [RDX+0x40]
        vec![0x0f, 0x18, 0x4e, 0x40],       // disp8 [RSI+0x40]
        vec![0x0f, 0x18, 0x0c, 0x24],       // SIB [RSP]
        vec![0x41, 0x0f, 0x18, 0x00],       // REX.B [R8]
        vec![0x0f, 0x18, 0x05, 0, 0, 0, 0], // RIP-relative
        vec![0x0f, 0x18, 0x84, 0x00, 0, 0, 0, 0], // SIB + disp32
        vec![0x0f, 0x18, 0x01],             // [rcx], non-canonical RCX: no #GP
        vec![0x66, 0x0f, 0x18, 0x00],       // 66
        vec![0x67, 0x0f, 0x18, 0x00],       // 67
        vec![0x2e, 0x0f, 0x18, 0x00],       // CS
        vec![0x48, 0x0f, 0x18, 0x00],       // REX.W
    ];
    // Every sub-opcode /0../7 (memory form [RDX]).
    for reg in 0..8u8 {
        forms.push(vec![0x0f, 0x18, (reg << 3) | 0x02]);
    }
    for code in &forms {
        let (state, before, result) = run_nop_0f1f(code);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn prefetch_0f18_does_not_fault_on_an_unmapped_address() {
    // RDX points at an unmapped page: a prefetch performs no access, so no #PF/error.
    let mut state = endbr_state();
    state.gpr[2] = 0x0000_7fff_0000_0000;
    let mut before = state.clone();
    before.rip += 3;
    let mut memory = Memory::code(&[0x0f, 0x18, 0x0a]);
    let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, before);
}

#[test]
fn prefetch_0f18_then_next_instruction_continues_at_the_decoded_length() {
    let code = [0x0f, 0x18, 0x4a, 0x40, 0x0f, 0x18, 0x0a, 0x48, 0x89, 0xc0];
    let mut state = endbr_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &base(), 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + 10);
}

#[test]
fn prefetch_0f18_lookalikes_stay_unsupported_in_the_set() {
    for code in [
        &[0x0fu8, 0x18, 0xc8][..],       // register form (conservative)
        &[0x0f, 0x18, 0xc0],             // register form /0
        &[0xf0, 0x0f, 0x18, 0x0a],       // LOCK
        &[0xf3, 0x0f, 0x18, 0x0a],       // REP
        &[0xf2, 0x0f, 0x18, 0x0a],       // REPNE
        &[0x2e, 0x2e, 0x0f, 0x18, 0x0a], // duplicate segment
        &[0x40, 0x41, 0x0f, 0x18, 0x0a], // duplicate REX
    ] {
        let mut state = endbr_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x18 },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn prefetch_0f18_outside_the_set_stays_unsupported() {
    let mut state = endbr_state();
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x18, 0x0a]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x18 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

// ---- S2-50: POPFQ with the CPL0 IOPL/IF rule ----

const RF: u64 = 1 << 16;
const TF: u64 = 1 << 8;
/// CF PF AF ZF SF DF OF, IF, IOPL, NT, AC and ID: what POPFQ loads at CPL0.
const POPFQ_CPL0_MASK: u64 = 0x24_7ed5;

fn popf_set() -> Cpl0ProfileSet<'static> {
    base().with_popf_cpl0()
}

/// `code` at `RIP`, the popped qword `value` at `STACK`.
fn popf_state(code: &[u8], value: u64, rflags: u64) -> (LongModeState, LretqMemory) {
    let mut state = board_state();
    state.rflags = rflags;
    state.gpr[4] = STACK;
    let mut memory = LretqMemory::new(code);
    memory.put(STACK, value);
    (state, memory)
}

fn run_popf(
    state: &mut LongModeState,
    memory: &mut LretqMemory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap()
}

#[test]
fn popfq_loads_the_cpl0_flags_and_keeps_rf_vm_and_the_reserved_bits() {
    for prefix in [&[][..], &[0x48]] {
        let mut code = prefix.to_vec();
        code.extend_from_slice(&[0x9d, 0x90]);
        // Everything but TF set in the stack qword (RF, VM, VIF, VIP and the reserved bits
        // included); the current RFLAGS carry RF, VIF and a status flag.
        let value = u64::MAX & !TF;
        let old = 2 | RF | 0x1 | (1 << 19);
        let (mut state, mut memory) = popf_state(&code, value, old);
        let before = state.clone();
        let result = run_popf(&mut state, &mut memory, &popf_set(), 2);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2, "{prefix:02x?}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(state.rflags, (old & !POPFQ_CPL0_MASK) | POPFQ_CPL0_MASK);
        // IF, IOPL = 3, AC and ID came from the stack; RF and VIF kept, VM and VIP stayed clear.
        assert_eq!(state.rflags & (1 << 17), 0, "VM");
        assert_ne!(state.rflags & RF, 0, "RF");
        assert_eq!(state.rflags & (1 << 20), 0, "VIP is not loaded");
        assert_eq!(state.rflags & (1 << 19), 1 << 19, "VIF is not touched");
        assert_eq!(state.rflags & (3 << 12), 3 << 12);
        assert_eq!(state.gpr[4], STACK + 8);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(memory.reads.len(), 1);
        assert_eq!((memory.reads[0].0, memory.reads[0].1), (STACK, 8));
        let mut expected = before;
        expected.rflags = state.rflags;
        expected.gpr[4] = STACK + 8;
        expected.rip = RIP + code.len() as u64;
        assert_eq!(state, expected);
    }
}

#[test]
fn popfq_clears_if_iopl_ac_id_nt_and_does_not_set_rf_or_vm() {
    let old = 2 | IF | (3 << 12) | (1 << 14) | (1 << 18) | (1 << 21) | 0xcd5;
    // Stack: only RF, VM, VIF, VIP and bit 1; every loaded bit clear.
    let value = RF | (1 << 17) | (3 << 19) | 2;
    let (mut state, mut memory) = popf_state(&[0x9d], value, old);
    let result = run_popf(&mut state, &mut memory, &popf_set(), 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rflags, 2);
    assert_eq!(state.gpr[4], STACK + 8);
    assert_eq!(state.rip, RIP + 1);
}

#[test]
fn popfq_keeps_rflags_bit1_when_the_value_clears_it() {
    let (mut state, mut memory) = popf_state(&[0x9d], 0, 2 | IF);
    run_popf(&mut state, &mut memory, &popf_set(), 1);
    assert_eq!(state.rflags, 2);
}

#[test]
fn popfq_status_flags_become_defined() {
    let (mut state, mut memory) = popf_state(&[0x9d], 0x8d5, 2);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        2,
        &popf_set(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(result.rflags_defined, od_isa::POPFQ_INTEGER_FLAGS | 2);
    assert_eq!(state.rflags, 2 | 0x8d5);
}

#[test]
fn popfq_tf_word_form_prefixes_and_shadow_stay_unsupported_without_mutation() {
    let unsupported = RunExit::Unsupported { opcode: 0x9d };
    // (code, stack value, rflags, shadow)
    let cases: [(&[u8], u64, u64, StiShadow); 7] = [
        (&[0x9d], TF, 2 | IF, StiShadow::Clear),
        (&[0x48, 0x9d], 2 | TF, 2, StiShadow::Clear),
        (&[0x9d], 2, 2 | IF | TF, StiShadow::Clear),
        (&[0x66, 0x9d], 2, 2 | IF, StiShadow::Clear),
        (&[0x66, 0x48, 0x9d], 2, 2 | IF, StiShadow::Clear),
        (&[0x41, 0x9d], 2, 2 | IF, StiShadow::Clear),
        // A MOV SS shadow (IF = 0) would end with the pop; the set does not model that.
        (&[0x9d], IF | 2, 2, StiShadow::Blocking),
    ];
    for (code, value, rflags, shadow) in cases {
        let (mut state, mut memory) = popf_state(code, value, rflags);
        state.sti_shadow = Some(shadow);
        let before = state.clone();
        let result = run_popf(&mut state, &mut memory, &popf_set(), 1);
        assert_eq!(exit(&result), &unsupported, "{code:02x?} {value:#x}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn popfq_stack_faults_leave_the_state_unchanged() {
    // #PF on the stack page.
    let (mut state, mut memory) = popf_state(&[0x9d], u64::MAX & !TF, 2 | IF);
    memory.fault_pages.push(STACK & !0xfff);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_popf(&mut state, &mut memory, &popf_set(), 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(STACK),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 0);

    // #PF on the second page of a straddling qword.
    let (mut state, mut memory) = popf_state(&[0x48, 0x9d], 0, 2 | IF);
    state.gpr[4] = RAM_BASE + 0x1ffc;
    memory.fault_pages.push(RAM_BASE + 0x2000);
    let before = state.clone();
    let result = run_popf(&mut state, &mut memory, &popf_set(), 1);
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault { vector: 14, .. })
    ));
    assert_eq!(state, before);

    // Non-canonical RSP: #SS(0), no read.
    let (mut state, mut memory) = popf_state(&[0x9d], 0, 2 | IF);
    state.gpr[4] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let result = run_popf(&mut state, &mut memory, &popf_set(), 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn popfq_stays_unsupported_in_a_set_without_the_member() {
    for set in [base(), base().with_cpuid(&CpuidProfile::DiagnosticV1)] {
        let (mut state, mut memory) = popf_state(&[0x9d], 2, 2 | IF | (3 << 12));
        let before = state.clone();
        let result = run_popf(&mut state, &mut memory, &set, 1);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x9d });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn popfq_member_does_not_open_the_legacy_entries() {
    // The plain entry at CPL0 stays closed (it is the CPL3 diagnostic slice).
    let (mut state, mut memory) = popf_state(&[0x9d], 2, 2);
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x9d });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    // At CPL3 the old rule holds: IF and IOPL are not loaded from the stack.
    let (mut state, mut memory) = popf_state(&[0x9d], IF | (3 << 12) | 0x1 | 2, 2);
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        || false,
    );
    assert_eq!(result.instructions_executed, 1, "{result:?}");
    assert_eq!(state.rflags, 2 | 0x1);
    assert_eq!(state.gpr[4], STACK + 8);

    // The set entry itself still rejects CPL3.
    let (mut state, mut memory) = popf_state(&[0x9d], 2, 2);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &popf_set(),
        None,
        || false,
    );
    assert_eq!(result, Err(Cpl0ProfileSetInputError::WrongPrivilege));
}

#[test]
fn popfq_restores_if_from_a_saved_flags_value() {
    // `arch_local_irq_restore`: the saved RFLAGS (IF set) comes back through POPFQ.
    let (mut state, mut memory) = popf_state(&[0x9d, 0x90], 0x246, 2);
    let result = run_popf(&mut state, &mut memory, &popf_set(), 2);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rflags, 0x246);
    assert_eq!(state.rip, RIP + 2);
}

// ---- S4-38L4f: `MOVZX r32, r/m16` (`0F B7 /r`) in the set ----

const MOVZX_SEED: u64 = 0xdead_beef_cafe_f00d;

/// `code` at `RIP`; the RAM window holds `0x8877_6655_4433_abcd` at `RAM_BASE + 0x100`.
/// RCX points there, RSP is `STACK`, RAX and RSI hold `MOVZX_SEED`.
fn movzx_state(code: &[u8]) -> (LongModeState, LretqMemory) {
    let mut state = endbr_state();
    state.gpr[Register::Rax as usize] = MOVZX_SEED;
    state.gpr[Register::Rsi as usize] = MOVZX_SEED;
    state.gpr[Register::Rcx as usize] = RAM_BASE + 0x100;
    state.gpr[Register::Rsp as usize] = STACK;
    let mut memory = LretqMemory::new(code);
    memory.put(RAM_BASE + 0x100, 0x8877_6655_4433_abcd);
    memory.put(STACK, 0x1122_3344_5566_f234);
    (state, memory)
}

fn run_movzx(
    state: &mut LongModeState,
    memory: &mut LretqMemory,
    budget: u64,
) -> Cpl0ProfileSetResult {
    let defined = 0x1 | 0x40;
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        defined,
        &base(),
        None,
        || false,
    )
    .unwrap()
}

#[test]
fn movzx_r32_m16_reads_one_word_and_zero_extends_into_the_whole_register() {
    // (code, destination register, expected value, address of the word)
    let cases: [(&[u8], Register, u64, u64); 5] = [
        (&[0x0f, 0xb7, 0x31], Register::Rsi, 0xabcd, RAM_BASE + 0x100),
        (&[0x0f, 0xb7, 0x04, 0x24], Register::Rax, 0xf234, STACK),
        (
            &[0x0f, 0xb7, 0x41, 0x02],
            Register::Rax,
            0x4433,
            RAM_BASE + 0x102,
        ),
        (
            &[0x0f, 0xb7, 0x81, 0x04, 0x00, 0x00, 0x00],
            Register::Rax,
            0x6655,
            RAM_BASE + 0x104,
        ),
        // A 67 prefix only narrows the address (ECX).
        (
            &[0x67, 0x0f, 0xb7, 0x31],
            Register::Rsi,
            0xabcd,
            RAM_BASE + 0x100,
        ),
    ];
    for (code, destination, expected, address) in cases {
        let (mut state, mut memory) = movzx_state(code);
        let mut after = state.clone();
        let result = run_movzx(&mut state, &mut memory, 1);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        // Bit 15 of 0xf234 is set: no sign extension; the old upper bytes are gone.
        after.gpr[destination as usize] = expected;
        after.rip += code.len() as u64;
        assert_eq!(state, after, "{code:02x?}");
        // Flags and their definedness are untouched.
        assert_eq!(result.rflags_defined, 0x1 | 0x40, "{code:02x?}");
        // Exactly one two-byte read at the operand address, and no write.
        assert_eq!(
            memory.reads,
            [(address, 2, AccessOrigin::Explicit)],
            "{code:02x?}"
        );
        assert_eq!(memory.writes, 0, "{code:02x?}");
    }
}

#[test]
fn movzx_r32_r16_register_form_executes_without_a_read() {
    // `0F B7 C1`: MOVZX EAX, CX.
    let (mut state, mut memory) = movzx_state(&[0x0f, 0xb7, 0xc1]);
    state.gpr[Register::Rcx as usize] = 0x0123_4567_89ab_8001;
    let mut after = state.clone();
    let result = run_movzx(&mut state, &mut memory, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    after.gpr[Register::Rax as usize] = 0x8001;
    after.rip += 3;
    assert_eq!(state, after);
    assert!(memory.reads.is_empty());
}

#[test]
fn movzx_r64_m16_with_rex_w_zero_extends_to_64_bits() {
    let (mut state, mut memory) = movzx_state(&[0x48, 0x0f, 0xb7, 0x31]);
    let mut after = state.clone();
    let result = run_movzx(&mut state, &mut memory, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    after.gpr[Register::Rsi as usize] = 0xabcd;
    after.rip += 4;
    assert_eq!(state, after);
    assert_eq!(
        memory.reads,
        [(RAM_BASE + 0x100, 2, AccessOrigin::Explicit)]
    );
}

#[test]
fn movzx_r32_m16_then_the_next_instruction_continues_at_the_decoded_length() {
    // MOVZX ESI, word [RCX] ; MOV RAX, RSI
    let (mut state, mut memory) = movzx_state(&[0x0f, 0xb7, 0x31, 0x48, 0x89, 0xf0]);
    let result = run_movzx(&mut state, &mut memory, 2);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[Register::Rax as usize], 0xabcd);
    assert_eq!(state.rip, RIP + 6);
}

#[test]
fn movzx_r32_m16_page_fault_leaves_the_state_unchanged() {
    let (mut state, mut memory) = movzx_state(&[0x0f, 0xb7, 0x31]);
    memory.fault_pages.push((RAM_BASE + 0x100) & !0xfff);
    let before = state.clone();
    let result = run_movzx(&mut state, &mut memory, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(RAM_BASE + 0x100),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn movzx_r32_m16_lookalikes_keep_their_classification_in_the_set() {
    for code in [
        &[0xf0u8, 0x0f, 0xb7, 0x31][..], // LOCK
        &[0x66, 0x0f, 0xb7, 0x31],       // 66: the 16->16 form does not exist
        &[0xf3, 0x0f, 0xb7, 0x31],       // REP
    ] {
        let (mut state, mut memory) = movzx_state(code);
        let before = state.clone();
        let result = run_movzx(&mut state, &mut memory, 1);
        assert_ne!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert_eq!(memory.writes, 0, "{code:02x?}");
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xb7 });
    }
}

#[test]
fn movzx_r32_m16_in_the_plain_entry_is_the_same_instruction_and_the_set_adds_no_admission() {
    // The decoder and the executor of `0F B7` are not gated by the set (S4-38L4f changes none
    // of them): the plain entry executes the same bytes the same way. What the set's board
    // adapter adds is the two-byte read width, which lives in the memory backend.
    let (mut state, mut memory) = movzx_state(&[0x0f, 0xb7, 0x31]);
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let mut after = state.clone();
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    after.gpr[Register::Rsi as usize] = 0xabcd;
    after.rip += 3;
    assert_eq!(state, after);
    assert_eq!(
        memory.reads,
        [(RAM_BASE + 0x100, 2, AccessOrigin::Explicit)]
    );
}

// ---- S4-38L4g: `F3 0F BC /r` (REP BSF) as BSF on a profile without BMI1 ----

const ZF: u64 = 1 << 6;

fn run_bsf(code: &[u8], rax: u64, rcx: u64) -> (LongModeState, Cpl0ProfileSetResult) {
    let mut state = board_state();
    state.gpr[0] = rax;
    state.gpr[1] = rcx;
    let mut memory = Memory::code(code);
    let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
    (state, result)
}

#[test]
fn rep_bsf_is_bsf_with_an_ignored_prefix_in_the_set() {
    // (bytes, rax, rcx, expected rax, expected ZF)
    let cases: [(&[u8], u64, u64, u64, bool); 5] = [
        // EAX, EAX with a non-zero source: lowest set bit is bit 6; upper half is cleared.
        (
            &[0xf3, 0x0f, 0xbc, 0xc0],
            0x1111_2222_3333_4440,
            0,
            6,
            false,
        ),
        // Zero 32-bit source: ZF = 1 and the destination is left unchanged (like plain BSF).
        (
            &[0xf3, 0x0f, 0xbc, 0xc0],
            0x1111_2222_0000_0000,
            0,
            0x1111_2222_0000_0000,
            true,
        ),
        // REX.W, 64-bit source in RCX: bit 40.
        (
            &[0xf3, 0x48, 0x0f, 0xbc, 0xc1],
            0xdead_beef_dead_beef,
            0x0000_0100_0000_0000,
            40,
            false,
        ),
        // REX.W, zero 64-bit source.
        (
            &[0xf3, 0x48, 0x0f, 0xbc, 0xc1],
            0xdead_beef_dead_beef,
            0,
            0xdead_beef_dead_beef,
            true,
        ),
        // The 64-bit scan looks at the high half too.
        (
            &[0xf3, 0x48, 0x0f, 0xbc, 0xc1],
            0,
            0x8000_0000_0000_0000,
            63,
            false,
        ),
    ];
    for (code, rax, rcx, expected_rax, zero) in cases {
        let (state, result) = run_bsf(code, rax, rcx);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
        assert_eq!(state.gpr[0], expected_rax, "{code:02x?}");
        assert_eq!(state.gpr[1], rcx, "{code:02x?}");
        assert_eq!(state.rflags & ZF != 0, zero, "{code:02x?}");
        // ZF is defined; the whole arithmetic flag group is defined exactly as for plain BSF.
        assert_ne!(result.rflags_defined & ZF, 0, "{code:02x?}");

        // The same instruction without the REP prefix gives an identical outcome.
        let plain: Vec<u8> = code.iter().copied().filter(|&byte| byte != 0xf3).collect();
        let (plain_state, plain_result) = run_bsf(&plain, rax, rcx);
        assert_eq!(plain_state.gpr, state.gpr, "{code:02x?}");
        assert_eq!(plain_state.rflags, state.rflags, "{code:02x?}");
        assert_eq!(
            plain_result.rflags_defined, result.rflags_defined,
            "{code:02x?}"
        );
        assert_eq!(plain_state.rip + 1, state.rip, "{code:02x?}");
        assert_eq!(
            plain_result.instructions_executed, result.instructions_executed,
            "{code:02x?}"
        );
    }
}

#[test]
fn rep_bsf_non_zero_source_clears_zf_and_keeps_other_gprs() {
    let (state, _) = run_bsf(&[0xf3, 0x0f, 0xbc, 0xc0], 0x8000_0000, 0x55);
    assert_eq!(state.gpr[0], 31);
    assert_eq!(state.gpr[1], 0x55);
    assert_eq!(state.rflags & ZF, 0);
    assert_eq!(state.rflags & (1 << 9), 1 << 9, "IF is untouched");
}

#[test]
fn rep_bsf_lookalikes_stay_unsupported_without_mutation() {
    for code in [
        &[0xf2u8, 0x0f, 0xbc, 0xc0][..], // REPNE
        &[0xf0, 0xf3, 0x0f, 0xbc, 0xc0], // LOCK, then REP
        &[0xf3, 0xf0, 0x0f, 0xbc, 0xc0], // REP, then LOCK
        &[0xf0, 0x0f, 0xbc, 0xc0],       // LOCK alone
        &[0x66, 0xf3, 0x0f, 0xbc, 0xc0], // 16-bit form
        &[0xf3, 0x0f, 0xbd, 0xc0],       // LZCNT-shaped, not modelled
        &[0xf3, 0xf3, 0x0f, 0xbc, 0xc0], // duplicate REP
        &[0x48, 0xf3, 0x0f, 0xbc, 0xc0], // REX before REP is ignored by hardware
        &[0xf3, 0x64, 0x0f, 0xbc, 0xc0], // segment override
        &[0xf3, 0x67, 0x0f, 0xbc, 0xc0], // address-size override
    ] {
        let mut state = board_state();
        state.gpr[0] = 0x40;
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
        let opcode = if code.contains(&0xbd) { 0xbd } else { 0xbc };
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn rep_bsf_outside_the_set_stays_unsupported() {
    let mut state = board_state();
    state.gpr[0] = 0x40;
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let mut memory = Memory::code(&[0xf3, 0x0f, 0xbc, 0xc0]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xbc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    // Plain BSF outside the set keeps working.
    let mut state = board_state();
    state.gpr[0] = 0x40;
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let mut memory = Memory::code(&[0x0f, 0xbc, 0xc0]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 6);
}

// ---- S4-38L5b: IRETQ (48 CF), same-privilege return in the CPL0 set ----

const IRETQ: [u8; 2] = [0x48, 0xcf];
const IRETQ_NEW_RSP: u64 = RAM_BASE + 0x3000;
const DATA_DESCRIPTOR: u64 = 0x00cf_9300_0000_ffff;
const IRETQ_CS_SLOT: u64 = LRETQ_GDT + 0x10;
const IRETQ_SS_SLOT: u64 = LRETQ_GDT + 0x18;

/// Frame `[RIP][CS][RFLAGS][RSP][SS]` at `STACK`; GDT slot 2 is `CODE64`, slot 3 a flat data
/// segment (both with A = 1); the SS cache is modeled and differs from the frame's SS.
fn iretq_state(
    target: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
) -> (LongModeState, LretqMemory) {
    let (mut state, mut memory) = lretq_state(cs, target);
    memory.code = IRETQ.to_vec();
    memory.put(STACK + 16, rflags);
    memory.put(STACK + 24, rsp);
    memory.put(STACK + 32, ss);
    state.rflags = 2;
    state.ss_selector = Some(0x10);
    state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
        base: 0x1234,
        limit: 0xfff,
        access: 0x93,
        flags: 0x4,
    });
    state.ss_unusable = true;
    (state, memory)
}

fn default_iretq() -> (LongModeState, LretqMemory) {
    iretq_state(RIP + 2, 0x10, 0x202, IRETQ_NEW_RSP, 0x18)
}

fn run_iretq(state: &mut LongModeState, memory: &mut LretqMemory) -> Cpl0ProfileSetResult {
    run_lretq(state, memory)
}

/// Run `IRETQ` after `tweak` and expect `expected` with no CPU or memory change.
fn assert_iretq_refused(
    tweak: impl FnOnce(&mut LongModeState, &mut LretqMemory),
    expected: RunExit,
    label: &str,
) {
    let (mut state, mut memory) = default_iretq();
    tweak(&mut state, &mut memory);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &expected, "{label}");
    assert_eq!(result.instructions_executed, 0, "{label}");
    assert_eq!(state, before, "{label}");
    assert_eq!(memory.ram, ram, "{label}");
    assert_eq!(memory.writes, 0, "{label}");
}

fn unsupported_cf() -> RunExit {
    RunExit::Unsupported { opcode: 0xcf }
}

#[test]
fn iretq_loads_the_whole_frame_and_retires() {
    // Frame RFLAGS: every loaded bit, plus bits IRET never loads (reserved bits 3, 5, 15, 22;
    // the always-one bit 1 is clear in the image).
    const STATUS_DF: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x400 | 0x800;
    const LOADED: u64 = STATUS_DF | IF | (3 << 12) | (1 << 14) | (1 << 16) | (1 << 18) | (1 << 21);
    const NEVER_LOADED: u64 = (1 << 3) | (1 << 5) | (1 << 15) | (1 << 22);
    let (mut state, mut memory) =
        iretq_state(RIP + 2, 0x10, LOADED | NEVER_LOADED, IRETQ_NEW_RSP, 0x18);
    state.gpr[0] = 0xaaaa;
    let mut expected = state.clone();
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        0,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // The loaded flags are defined afterwards (as for POPFQ); nothing else is.
    assert_eq!(result.rflags_defined, od_isa::POPFQ_INTEGER_FLAGS);
    expected.rip = RIP + 2;
    expected.cs_selector = Some(0x10);
    expected.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xa,
    });
    expected.rflags = 2 | LOADED;
    expected.gpr[4] = IRETQ_NEW_RSP;
    expected.ss_selector = Some(0x18);
    expected.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0xc,
    });
    expected.ss_unusable = false;
    assert_eq!(state, expected);
    // Five stack qwords, then the CS and SS descriptors as descriptor-table accesses.
    assert_eq!(
        memory.reads,
        vec![
            (STACK, 8, AccessOrigin::Explicit),
            (STACK + 8, 8, AccessOrigin::Explicit),
            (STACK + 16, 8, AccessOrigin::Explicit),
            (STACK + 24, 8, AccessOrigin::Explicit),
            (STACK + 32, 8, AccessOrigin::Explicit),
            (IRETQ_CS_SLOT, 8, AccessOrigin::DescriptorTable),
            (IRETQ_SS_SLOT, 8, AccessOrigin::DescriptorTable),
        ]
    );
    assert_eq!(memory.writes, 0);
}

#[test]
fn iretq_clears_loaded_flags_the_frame_clears_and_keeps_bit_one() {
    // Current state: CF, IF, IOPL, RF and bit 1 set; the frame clears them. Loaded flags
    // follow the frame; bit 1 stays one.
    let (mut state, mut memory) = default_iretq();
    state.rflags = 2 | 1 | IF | (3 << 12) | (1 << 16);
    memory.put(STACK + 16, 0);
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rflags, 2);
}

#[test]
fn iretq_target_is_executed_next_and_rsp_comes_from_the_frame() {
    // `iretq` to a NOP right after it: the budget-2 run retires both, and RSP is the frame
    // value, not RSP + 40.
    let (mut state, mut memory) = default_iretq();
    memory.code = vec![0x48, 0xcf, 0x90];
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 2),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 3);
    assert_eq!(state.gpr[4], IRETQ_NEW_RSP);
    assert_eq!(state.rflags, 2 | IF);
}

#[test]
fn iretq_with_a_null_ss_marks_it_unusable_and_keeps_the_cache() {
    // The kernel's early boot runs with SS = 0, so the hardware-pushed frame holds a null SS.
    let (mut state, mut memory) = iretq_state(RIP + 2, 0x10, 0x202, IRETQ_NEW_RSP, 0);
    state.ss_unusable = false;
    let cache = state.ss_cache;
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.ss_selector, Some(0));
    assert!(state.ss_unusable);
    assert_eq!(state.ss_cache, cache);
    assert_eq!(state.gpr[4], IRETQ_NEW_RSP);
    assert_eq!(state.cs_selector, Some(0x10));
    // Only the CS descriptor was read from the GDT.
    assert_eq!(
        memory.reads.last(),
        Some(&(IRETQ_CS_SLOT, 8, AccessOrigin::DescriptorTable))
    );
}

#[test]
fn iretq_sets_the_accessed_bits_with_one_qword_write_each() {
    let (mut state, mut memory) = default_iretq();
    memory.put(IRETQ_CS_SLOT, 0x00af_9a00_0000_ffff);
    memory.put(IRETQ_SS_SLOT, 0x00cf_9200_0000_ffff);
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        memory.write_log,
        vec![
            (
                IRETQ_CS_SLOT,
                0x00af_9b00_0000_ffffu64.to_le_bytes().to_vec(),
                AccessOrigin::DescriptorTable
            ),
            (
                IRETQ_SS_SLOT,
                0x00cf_9300_0000_ffffu64.to_le_bytes().to_vec(),
                AccessOrigin::DescriptorTable
            ),
        ]
    );
    assert_eq!(state.ss_cache.unwrap().access, 0x93);
    assert_eq!(state.cs_cache.unwrap().access, 0x9b);
}

#[test]
fn iretq_cs_accessed_write_fault_writes_nothing_and_leaves_the_cpu_untouched() {
    let (mut state, mut memory) = default_iretq();
    memory.put(IRETQ_CS_SLOT, 0x00af_9a00_0000_ffff);
    memory.put(IRETQ_SS_SLOT, 0x00cf_9200_0000_ffff);
    memory.fail_write_number = Some(1);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert!(matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 14));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 1);
}

#[test]
fn iretq_ss_accessed_write_fault_keeps_the_cs_accessed_bit_and_the_cpu_untouched() {
    // Documented partial effect (CPU_CONTRACT S4-38L5b): the CS accessed-bit write has
    // happened when the SS accessed-bit write faults; the CPU state is unchanged.
    let (mut state, mut memory) = default_iretq();
    memory.put(IRETQ_CS_SLOT, 0x00af_9a00_0000_ffff);
    memory.put(IRETQ_SS_SLOT, 0x00cf_9200_0000_ffff);
    memory.fail_write_number = Some(2);
    let before = state.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(IRETQ_SS_SLOT)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.writes, 2);
    // The CS descriptor now has A = 1; the SS descriptor is unchanged.
    assert_eq!(
        memory.write_log,
        vec![(
            IRETQ_CS_SLOT,
            0x00af_9b00_0000_ffffu64.to_le_bytes().to_vec(),
            AccessOrigin::DescriptorTable
        )]
    );
    let offset = (IRETQ_SS_SLOT - RAM_BASE) as usize;
    assert_eq!(
        memory.ram[offset..offset + 8],
        0x00cf_9200_0000_ffffu64.to_le_bytes()
    );
}

#[test]
fn iretq_uses_only_the_low_sixteen_bits_of_the_cs_and_ss_qwords() {
    let (mut state, mut memory) = iretq_state(
        RIP + 2,
        0xdead_beef_0000_0010,
        0x202,
        IRETQ_NEW_RSP,
        0xbeef_dead_0000_0018,
    );
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.ss_selector, Some(0x18));
}

#[test]
fn iretq_non_canonical_rip_is_gp0_after_the_cs_checks() {
    assert_iretq_refused(
        |_, memory| memory.put(STACK, 0x0001_0000_0000_0000),
        selector_fault_exit(13, 0),
        "non-canonical RIP",
    );
    // A bad CS wins over a non-canonical RIP.
    assert_iretq_refused(
        |_, memory| {
            memory.put(STACK, 0x0001_0000_0000_0000);
            memory.put(IRETQ_CS_SLOT, DATA_DESCRIPTOR);
        },
        selector_fault_exit(13, 0x10),
        "bad CS before RIP",
    );
}

#[test]
fn iretq_rejects_bad_cs_with_exact_faults_and_no_mutation() {
    for (descriptor, expected, label) in [
        (DATA_DESCRIPTOR, selector_fault_exit(13, 0x10), "data CS"),
        (
            0x00ef_9b00_0000_ffff,
            selector_fault_exit(13, 0x10),
            "L = D = 1",
        ),
        (
            0x00af_fb00_0000_ffff,
            selector_fault_exit(13, 0x10),
            "DPL 3",
        ),
        (
            0x00af_1b00_0000_ffff,
            selector_fault_exit(11, 0x10),
            "not present",
        ),
        (
            0x00af_8100_0000_ffff,
            selector_fault_exit(13, 0x10),
            "system",
        ),
        (0, selector_fault_exit(13, 0x10), "empty slot"),
        (0x00cf_9b00_0000_ffff, unsupported_cf(), "compatibility CS"),
    ] {
        assert_iretq_refused(
            |_, memory| memory.put(IRETQ_CS_SLOT, descriptor),
            expected,
            label,
        );
    }
    for (cs, expected, label) in [
        (0, selector_fault_exit(13, 0), "null CS"),
        (3, selector_fault_exit(13, 0), "null CS with RPL"),
        (0x38, selector_fault_exit(13, 0x38), "beyond the GDT limit"),
        (0x14, unsupported_cf(), "LDT CS"),
        (0x13, unsupported_cf(), "RPL 3 CS"),
    ] {
        assert_iretq_refused(|_, memory| memory.put(STACK + 8, cs), expected, label);
    }
}

#[test]
fn iretq_rejects_bad_ss_with_exact_faults_and_no_mutation() {
    for (descriptor, expected, label) in [
        (
            0x00af_9b00_0000_ffff,
            selector_fault_exit(13, 0x18),
            "code SS",
        ),
        (
            0x00cf_9100_0000_ffff,
            selector_fault_exit(13, 0x18),
            "read-only SS",
        ),
        (
            0x00cf_b300_0000_ffff,
            selector_fault_exit(13, 0x18),
            "DPL 3 SS",
        ),
        (
            0x00cf_1300_0000_ffff,
            selector_fault_exit(12, 0x18),
            "not present SS",
        ),
        (
            0x00cf_8300_0000_ffff,
            selector_fault_exit(13, 0x18),
            "system SS",
        ),
    ] {
        assert_iretq_refused(
            |_, memory| memory.put(IRETQ_SS_SLOT, descriptor),
            expected,
            label,
        );
    }
    for (ss, expected, label) in [
        (
            0x38,
            selector_fault_exit(13, 0x38),
            "SS beyond the GDT limit",
        ),
        (0x1b, selector_fault_exit(13, 0x18), "SS RPL 3"),
        (0x1c, unsupported_cf(), "LDT SS"),
        (0x01, unsupported_cf(), "null SS with RPL bits"),
    ] {
        assert_iretq_refused(|_, memory| memory.put(STACK + 32, ss), expected, label);
    }
}

#[test]
fn iretq_unmodeled_flags_and_state_are_unsupported_without_mutation() {
    for (bit, label) in [
        (8, "frame TF"),
        (17, "frame VM"),
        (19, "frame VIF"),
        (20, "frame VIP"),
    ] {
        assert_iretq_refused(
            |_, memory| memory.put(STACK + 16, 0x202 | (1 << bit)),
            unsupported_cf(),
            label,
        );
    }
    for (bit, label) in [(8, "current TF"), (19, "current VIF"), (20, "current VIP")] {
        assert_iretq_refused(|state, _| state.rflags |= 1 << bit, unsupported_cf(), label);
    }
    // NT = 1 in IA-32e mode is #GP(0) before the frame is read.
    let (mut state, mut memory) = default_iretq();
    state.rflags |= 1 << 14;
    let before = state.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &selector_fault_exit(13, 0));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
    assert_eq!(memory.writes, 0);
    assert_iretq_refused(
        |state, _| state.sti_shadow = Some(StiShadow::Blocking),
        unsupported_cf(),
        "interrupt shadow",
    );
    assert_iretq_refused(|state, _| state.gdtr = None, unsupported_cf(), "no GDTR");
    assert_iretq_refused(
        |state, _| state.ss_cache = None,
        unsupported_cf(),
        "no SS cache",
    );
    assert_iretq_refused(
        |state, _| state.ss_selector = None,
        unsupported_cf(),
        "no SS selector",
    );
}

#[test]
fn iretq_stack_page_faults_are_exact_and_leave_state_untouched() {
    // Qword `k` of the frame is the first one in an unmapped page (the frame straddles the
    // page boundary at STACK + 0x1000); the earlier qwords are readable.
    for k in 0..5_u64 {
        let rsp = STACK + 0x1000 - 8 * k;
        let (mut state, mut memory) = default_iretq();
        state.gpr[4] = rsp;
        let frame = [RIP + 2, 0x10, 0x202, IRETQ_NEW_RSP, 0x18];
        for (slot, value) in frame.iter().enumerate().take(k as usize) {
            memory.put(rsp + 8 * slot as u64, *value);
        }
        memory.fault_pages.push(STACK + 0x1000);
        let before = state.clone();
        let ram = memory.ram.clone();
        let result = run_iretq(&mut state, &mut memory);
        let RunExit::Fault(fault) = exit(&result) else {
            panic!("expected a page fault for k = {k}, got {:?}", exit(&result));
        };
        assert_eq!(fault.vector, 14, "{k}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.ram, ram);
        assert_eq!(memory.writes, 0);
    }
    // The CS descriptor page unmapped (frame in a good page).
    let (mut state, mut memory) = default_iretq();
    memory.fault_pages.push(LRETQ_GDT);
    let before = state.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert!(matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 14));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn iretq_non_canonical_rsp_is_ss0_before_any_read() {
    let (mut state, mut memory) = default_iretq();
    state.gpr[4] = 0x0001_0000_0000_0000;
    let before = state.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &selector_fault_exit(12, 0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn iretq_ss_descriptor_page_fault_leaves_state_and_gdt_untouched() {
    // The SS descriptor sits in its own page; the CS descriptor (A = 0) and the frame are
    // fine, so the CS accessed-bit write must not have happened either.
    let (mut state, mut memory) = default_iretq();
    memory.put(IRETQ_CS_SLOT, 0x00af_9a00_0000_ffff);
    memory.put(STACK + 32, 0x1018);
    state.gdtr = Some(od_isa::state::Gdtr {
        base: LRETQ_GDT,
        limit: 0x1fff,
    });
    memory.fault_pages.push(LRETQ_GDT + 0x1000);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_iretq(&mut state, &mut memory);
    assert!(matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 14));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 0);
}

#[test]
fn iretq_outside_the_set_and_with_other_prefixes_stays_unsupported() {
    let (mut state, mut memory) = default_iretq();
    state.cr0 = None;
    state.cr4 = None;
    state.activity = None;
    state.sti_shadow = None;
    let before = state.clone();
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, unsupported_cf());
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
    // Inside the set only the exact REX.W form decodes.
    for code in [
        &[0xcfu8][..],
        &[0x66, 0x48, 0xcf],
        &[0x66, 0xcf],
        &[0x49, 0xcf],
        &[0xf0, 0x48, 0xcf],
        &[0xf3, 0x48, 0xcf],
        &[0xf2, 0x48, 0xcf],
        &[0x2e, 0x48, 0xcf],
        &[0x67, 0x48, 0xcf],
    ] {
        let (mut state, mut memory) = default_iretq();
        memory.code = code.to_vec();
        let before = state.clone();
        let result = run_iretq(&mut state, &mut memory);
        assert_eq!(exit(&result), &unsupported_cf(), "{code:x?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

// ---- S4-38L5c: delivery of a #PF through the IDT gate of vector 14 ----------

const PF_IDT: u64 = RAM_BASE + 0x3000;
/// The gate of vector 14 needs IDT bytes 224..=239.
const PF_IDT_LIMIT: u16 = 0xff;
const PF_GATE: u64 = PF_IDT + 14 * 16;
const PF_HANDLER: u64 = RIP + 0x100;
const PF_RSP: u64 = STACK + 0x800;
const PF_FAULT_PAGE: u64 = RAM_BASE;
const PF_FAULT_ADDRESS: u64 = PF_FAULT_PAGE + 0x20;
/// TF, NT and RF: the flags the gate entry clears besides VM (IF too for an interrupt gate).
/// AC is kept by the entry.
const PF_FLAGS_ALL: u64 = (1 << 8) | (1 << 14) | (1 << 16);
const PF_AC: u64 = 1 << 18;

fn pf_gate_bytes(offset: u64, selector: u16, ist: u8, attributes: u8) -> [u8; 16] {
    let mut gate = [0u8; 16];
    gate[0..2].copy_from_slice(&(offset as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&selector.to_le_bytes());
    gate[4] = ist;
    gate[5] = attributes;
    gate[6..8].copy_from_slice(&((offset >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((offset >> 32) as u32).to_le_bytes());
    gate
}

fn pf_put_gate(memory: &mut LretqMemory, gate: [u8; 16]) {
    let offset = (PF_GATE - RAM_BASE) as usize;
    memory.ram[offset..offset + 16].copy_from_slice(&gate);
}

fn pf_record() -> od_isa::PageFaultRecord {
    od_isa::PageFaultRecord::new(
        RIP,
        AccessKind::Write,
        context(PrivilegeLevel::Ring0),
        3,
        PF_FAULT_ADDRESS,
    )
}

/// A CPL0 state about to retry `MOV [RBX], AL` at `RIP`; GDT slot 2 is `CODE64` (the gate
/// selector 0x10), IDT gate 14 is an interrupt gate to `PF_HANDLER`, the handler is `IRETQ`.
fn pf_state() -> (LongModeState, LretqMemory) {
    let (mut state, mut memory) = lretq_state(0x10, 0);
    let mut code = vec![0x88, 0x03];
    code.resize((PF_HANDLER - RIP) as usize, 0x90);
    // The handler drops the error code (ADD RSP, 8) and returns.
    code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x08]);
    code.extend_from_slice(&IRETQ);
    memory.code = code;
    pf_put_gate(&mut memory, pf_gate_bytes(PF_HANDLER, 0x10, 0, 0x8e));
    state.idtr = Some(od_isa::state::Idtr {
        base: PF_IDT,
        limit: PF_IDT_LIMIT,
    });
    state.cs_selector = Some(0x10);
    state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xa,
    });
    state.ss_selector = Some(0x18);
    state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0xc,
    });
    state.gpr[Register::Rsp as usize] = PF_RSP;
    state.gpr[Register::Rbx as usize] = PF_FAULT_ADDRESS;
    state.rflags = 2 | IF | 0x1 | 0x40;
    (state, memory)
}

fn pf_deliver(
    state: &mut LongModeState,
    memory: &mut LretqMemory,
) -> od_isa::Cpl0PageFaultDelivery {
    od_isa::deliver_page_fault_cpl0_set(state, memory, &pf_record(), context(PrivilegeLevel::Ring0))
}

fn pf_qword(memory: &LretqMemory, address: u64) -> u64 {
    let offset = (address - RAM_BASE) as usize;
    u64::from_le_bytes(memory.ram[offset..offset + 8].try_into().unwrap())
}

#[test]
fn pf_delivery_writes_the_frame_and_enters_the_handler() {
    let (mut state, mut memory) = pf_state();
    // A live TF/NT (RF = 0) and AC in the image; the handler sees TF/NT cleared and AC kept.
    state.rflags |= (1 << 8) | (1 << 14) | (1 << 18);
    let before = state.clone();
    let rflags_in_frame = before.rflags | (1 << 16);
    let delivered = pf_deliver(&mut state, &mut memory);
    assert_eq!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Delivered {
            target_rip: PF_HANDLER,
            new_rsp: PF_RSP - 48,
            trap_gate: false,
        }
    );
    // Frame, from the highest address: SS, RSP, RFLAGS (RF = 1), CS, RIP, error code.
    assert_eq!(pf_qword(&memory, PF_RSP - 8), 0x18);
    assert_eq!(pf_qword(&memory, PF_RSP - 16), PF_RSP);
    assert_eq!(pf_qword(&memory, PF_RSP - 24), rflags_in_frame);
    assert_eq!(pf_qword(&memory, PF_RSP - 32), 0x10);
    assert_eq!(pf_qword(&memory, PF_RSP - 40), RIP);
    assert_eq!(pf_qword(&memory, PF_RSP - 48), 3);
    // Six qword stores with the implicit-stack origin, plus nothing else (A bit already set).
    assert_eq!(memory.writes, 6);
    assert!(
        memory
            .write_log
            .iter()
            .all(|(_, bytes, origin)| bytes.len() == 8 && *origin == AccessOrigin::ImplicitStack)
    );
    // The gate is read as two qwords and the descriptor as one, all as descriptor-table reads.
    assert_eq!(
        memory.reads,
        vec![
            (PF_GATE, 8, AccessOrigin::DescriptorTable),
            (PF_GATE + 8, 8, AccessOrigin::DescriptorTable),
            (IRETQ_CS_SLOT, 8, AccessOrigin::DescriptorTable),
        ]
    );
    // The CPU: handler entry, new stack, IF/TF/NT/RF/AC clear, CR2 = fault address, SS kept.
    let mut expected = before.clone();
    expected.rip = PF_HANDLER;
    expected.gpr[Register::Rsp as usize] = PF_RSP - 48;
    expected.rflags = 2 | 0x1 | 0x40 | PF_AC;
    expected.cr2 = Some(PF_FAULT_ADDRESS);
    assert_eq!(state, expected);
    assert_eq!(state.rflags & (IF | PF_FLAGS_ALL | (1 << 17)), 0);
    assert_eq!(state.rflags & PF_AC, PF_AC);
    assert_eq!(pf_qword(&memory, PF_RSP - 24) & PF_AC, PF_AC);
}

#[test]
fn pf_delivery_aligns_the_stack_down_to_sixteen_and_saves_the_real_rsp() {
    for rsp in [PF_RSP + 8, PF_RSP + 15, PF_RSP + 1] {
        let (mut state, mut memory) = pf_state();
        state.gpr[Register::Rsp as usize] = rsp;
        let delivered = pf_deliver(&mut state, &mut memory);
        assert!(
            matches!(delivered, od_isa::Cpl0PageFaultDelivery::Delivered { .. }),
            "{rsp:#x}: {delivered:?}"
        );
        assert_eq!(state.gpr[Register::Rsp as usize], PF_RSP - 48, "{rsp:#x}");
        assert_eq!(pf_qword(&memory, PF_RSP - 16), rsp, "{rsp:#x}");
        assert_eq!(state.gpr[Register::Rsp as usize] % 16, 0);
    }
}

#[test]
fn pf_delivery_through_a_trap_gate_keeps_if_and_clears_the_rest() {
    let (mut state, mut memory) = pf_state();
    pf_put_gate(&mut memory, pf_gate_bytes(PF_HANDLER, 0x10, 0, 0x8f));
    state.rflags |= (1 << 8) | (1 << 14);
    let delivered = pf_deliver(&mut state, &mut memory);
    assert!(
        matches!(
            delivered,
            od_isa::Cpl0PageFaultDelivery::Delivered {
                trap_gate: true,
                ..
            }
        ),
        "{delivered:?}"
    );
    assert_eq!(state.rflags & IF, IF);
    assert_eq!(state.rflags & PF_FLAGS_ALL, 0);
}

#[test]
fn pf_delivery_sets_the_accessed_bit_of_the_target_descriptor_after_the_frame() {
    let (mut state, mut memory) = pf_state();
    // CODE64 with A = 0 (type 0x9a): the cache of the current CS has A = 1.
    memory.put(LRETQ_GDT + 0x10, CODE64 & !(1 << 40));
    let delivered = pf_deliver(&mut state, &mut memory);
    assert!(matches!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Delivered { .. }
    ));
    assert_eq!(pf_qword(&memory, LRETQ_GDT + 0x10), CODE64);
    let last = memory.write_log.last().unwrap();
    assert_eq!(
        (last.0, last.1.len(), last.2),
        (IRETQ_CS_SLOT, 8, AccessOrigin::DescriptorTable)
    );
    assert_eq!(memory.writes, 7);
    assert_eq!(state.cs_cache.unwrap().access, 0x9b);
}

#[test]
fn pf_delivery_handler_iretq_restarts_the_instruction_with_rf_set() {
    let (mut state, mut memory) = pf_state();
    state.gpr[Register::Rax as usize] = 0x5a;
    memory.write_fault_pages = vec![PF_FAULT_PAGE];
    let start = state.clone();
    // The store faults (P = 1, W = 1, supervisor write to a read-only page) and retires nothing.
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(PF_FAULT_ADDRESS),
        })
    );
    assert_eq!((result.instructions_executed, state.rip), (0, RIP));
    assert_eq!(state, start);
    let delivered = pf_deliver(&mut state, &mut memory);
    assert!(matches!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Delivered { .. }
    ));
    // The handler repairs the page (here: the harness lifts the fault) and returns with IRETQ.
    memory.write_fault_pages.clear();
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rsp as usize], PF_RSP - 40);
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP, "back on the faulting instruction");
    assert_eq!(state.gpr[Register::Rsp as usize], PF_RSP);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.ss_selector, Some(0x18));
    // IRETQ restores the frame RFLAGS: IF back, RF = 1 (the board clears it after the retry).
    assert_eq!(state.rflags, start.rflags | (1 << 16));
    assert_eq!(memory.ram[(PF_FAULT_ADDRESS - RAM_BASE) as usize], 0);
    // The retried store now executes (RF = 1 does not suppress it in the model).
    let result = run_lretq(&mut state, &mut memory);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!((result.instructions_executed, state.rip), (1, RIP + 2));
    assert_eq!(memory.ram[(PF_FAULT_ADDRESS - RAM_BASE) as usize], 0x5a);
    assert_eq!(state.cr2, Some(PF_FAULT_ADDRESS));
}

/// Deliver after `tweak` and expect exactly `expected` with no write and no CPU change.
fn assert_pf_refused(
    tweak: impl FnOnce(&mut LongModeState, &mut LretqMemory),
    expected: od_isa::Cpl0PageFaultRefusal,
    label: &str,
) {
    let (mut state, mut memory) = pf_state();
    tweak(&mut state, &mut memory);
    let before = state.clone();
    let ram = memory.ram.clone();
    let delivered = pf_deliver(&mut state, &mut memory);
    assert_eq!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Refused(expected),
        "{label}"
    );
    assert_eq!(state, before, "{label}");
    assert_eq!(memory.ram, ram, "{label}");
    assert_eq!(memory.writes, 0, "{label}");
}

#[test]
fn pf_delivery_refuses_a_bad_gate_without_any_write() {
    use od_isa::Cpl0PageFaultRefusal as R;
    assert_pf_refused(|state, _| state.idtr = None, R::MissingIdtr, "no IDTR");
    assert_pf_refused(
        |state, _| state.idtr.as_mut().unwrap().limit = 14 * 16 + 14,
        R::GateBeyondIdtLimit,
        "limit one byte short of the gate",
    );
    assert_pf_refused(
        |state, _| state.idtr.as_mut().unwrap().base = u64::MAX - 100,
        R::NonCanonicalIdtSpan,
        "IDT base overflows",
    );
    assert_pf_refused(
        |_, memory| pf_put_gate(memory, pf_gate_bytes(PF_HANDLER, 0x10, 0, 0x0e)),
        R::GateNotPresent,
        "P = 0",
    );
    for attributes in [0x8c, 0x85, 0x8d, 0x9e, 0x80] {
        assert_pf_refused(
            |_, memory| pf_put_gate(memory, pf_gate_bytes(PF_HANDLER, 0x10, 0, attributes)),
            R::InvalidGateType(attributes),
            &format!("type/attributes {attributes:#x}"),
        );
    }
    for ist in 1..=7u8 {
        assert_pf_refused(
            |_, memory| pf_put_gate(memory, pf_gate_bytes(PF_HANDLER, 0x10, ist, 0x8e)),
            R::UnsupportedIst(ist),
            &format!("IST {ist}"),
        );
    }
    assert_pf_refused(
        |_, memory| {
            let mut gate = pf_gate_bytes(PF_HANDLER, 0x10, 0, 0x8e);
            gate[4] = 0x08;
            pf_put_gate(memory, gate);
        },
        R::GateReservedBitsSet,
        "reserved bit of byte 4",
    );
    assert_pf_refused(
        |_, memory| {
            let mut gate = pf_gate_bytes(PF_HANDLER, 0x10, 0, 0x8e);
            gate[12] = 1;
            pf_put_gate(memory, gate);
        },
        R::GateReservedBitsSet,
        "reserved dword",
    );
    for selector in [0u16, 3, 0x14, 0x12] {
        assert_pf_refused(
            |_, memory| pf_put_gate(memory, pf_gate_bytes(PF_HANDLER, selector, 0, 0x8e)),
            R::InvalidTargetSelector,
            &format!("selector {selector:#x}"),
        );
    }
    assert_pf_refused(
        |_, memory| pf_put_gate(memory, pf_gate_bytes(PF_HANDLER, 0x20, 0, 0x8e)),
        R::TargetBeyondGdtLimit,
        "selector beyond the GDT",
    );
    assert_pf_refused(
        |_, memory| pf_put_gate(memory, pf_gate_bytes(0x0000_8000_0000_0000, 0x10, 0, 0x8e)),
        R::NonCanonicalTargetRip,
        "non-canonical handler",
    );
}

#[test]
fn pf_delivery_refuses_a_bad_target_descriptor_without_any_write() {
    use od_isa::Cpl0PageFaultRefusal as R;
    for (label, descriptor) in [
        ("data segment", DATA_DESCRIPTOR),
        ("DPL 3", CODE64 | (3 << 45)),
        ("not present", CODE64 & !(1 << 47)),
        ("L = 0", CODE64 & !(1 << 53)),
        ("L = 1 and D = 1", CODE64 | (1 << 54)),
        ("conforming", CODE64 | (1 << 42)),
        ("system descriptor", CODE64 & !(1 << 44)),
    ] {
        assert_pf_refused(
            |_, memory| memory.put(LRETQ_GDT + 0x10, descriptor),
            R::InvalidTargetCodeDescriptor,
            label,
        );
    }
    assert_pf_refused(|state, _| state.gdtr = None, R::MissingGdtr, "no GDTR");
    assert_pf_refused(
        |state, _| state.gdtr.as_mut().unwrap().base = u64::MAX - 4,
        R::NonCanonicalGdtSpan,
        "GDT base overflows",
    );
}

#[test]
fn pf_delivery_refuses_unmodeled_state_without_any_write() {
    use od_isa::Cpl0PageFaultRefusal as R;
    assert_pf_refused(
        |state, _| state.rip = RIP + 1,
        R::FaultRipMismatch,
        "RIP moved",
    );
    assert_pf_refused(
        |state, _| state.activity = Some(CpuActivity::Halted),
        R::ActivityOutsideProfile,
        "halted",
    );
    assert_pf_refused(
        |state, _| state.sti_shadow = Some(StiShadow::Blocking),
        R::StiShadowActive,
        "interrupt shadow",
    );
    assert_pf_refused(
        |state, _| state.cs_cache = None,
        R::MissingCurrentSelectors,
        "no CS cache",
    );
    assert_pf_refused(
        |state, _| state.ss_selector = None,
        R::MissingCurrentSelectors,
        "no SS selector",
    );
    assert_pf_refused(
        |state, _| state.cs_cache.as_mut().unwrap().flags = 0xc,
        R::InvalidCurrentCodeSegment,
        "current CS is not 64-bit code",
    );
    assert_pf_refused(
        |state, _| state.gpr[Register::Rsp as usize] = 0x2f,
        R::StackAddressOverflow,
        "RSP below the frame size",
    );
    assert_pf_refused(
        |state, _| state.gpr[Register::Rsp as usize] = 0xffff_8000_0000_0010,
        R::NonCanonicalStackSpan,
        "non-canonical stack",
    );
}

#[test]
fn pf_delivery_refuses_a_cpl3_context() {
    let (mut state, mut memory) = pf_state();
    let before = state.clone();
    let delivered = od_isa::deliver_page_fault_cpl0_set(
        &mut state,
        &mut memory,
        &pf_record(),
        context(PrivilegeLevel::Ring3),
    );
    assert_eq!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Refused(od_isa::Cpl0PageFaultRefusal::NotRing0Long64)
    );
    assert_eq!(state, before);
    assert_eq!(memory.writes, 0);
}

#[test]
fn pf_delivery_stack_write_failure_leaves_the_cpu_unchanged() {
    // The first store (SS, the highest word) fails: nothing is stored at all.
    let (mut state, mut memory) = pf_state();
    memory.fail_write_number = Some(1);
    let before = state.clone();
    let ram = memory.ram.clone();
    let delivered = pf_deliver(&mut state, &mut memory);
    assert_eq!(
        delivered,
        od_isa::Cpl0PageFaultDelivery::Aborted {
            phase: od_isa::Cpl0PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::OldSs
            },
            cause: MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(PF_RSP - 8),
            }),
        }
    );
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.write_log.len(), 0);
    // A later store failing leaves the CPU unchanged too; the words already stored lie below
    // the old RSP (dead stack), CR2 is not written.
    for failing in 2..=6usize {
        let (mut state, mut memory) = pf_state();
        memory.fail_write_number = Some(failing);
        let before = state.clone();
        let delivered = pf_deliver(&mut state, &mut memory);
        assert!(
            matches!(
                delivered,
                od_isa::Cpl0PageFaultDelivery::Aborted {
                    phase: od_isa::Cpl0PageFaultDeliveryPhase::StackWrite { .. },
                    ..
                }
            ),
            "{failing}: {delivered:?}"
        );
        assert_eq!(state, before, "{failing}");
        assert_eq!(memory.write_log.len(), failing - 1, "{failing}");
        assert!(
            memory
                .write_log
                .iter()
                .all(|(address, _, _)| *address < PF_RSP),
            "{failing}"
        );
    }
}

#[test]
fn pf_delivery_descriptor_accessed_write_failure_leaves_the_cpu_unchanged() {
    let (mut state, mut memory) = pf_state();
    memory.put(LRETQ_GDT + 0x10, CODE64 & !(1 << 40));
    memory.fail_write_number = Some(7);
    let before = state.clone();
    let delivered = pf_deliver(&mut state, &mut memory);
    assert!(
        matches!(
            delivered,
            od_isa::Cpl0PageFaultDelivery::Aborted {
                phase: od_isa::Cpl0PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite,
                ..
            }
        ),
        "{delivered:?}"
    );
    assert_eq!(state, before);
}

#[test]
fn pf_delivery_idt_and_gdt_read_failures_write_nothing() {
    use od_isa::Cpl0PageFaultDeliveryPhase as Phase;
    for (page, phase) in [
        (PF_IDT, Phase::IdtGateRead),
        (LRETQ_GDT, Phase::TargetCodeDescriptorRead),
    ] {
        let (mut state, mut memory) = pf_state();
        memory.fault_pages = vec![page];
        let before = state.clone();
        let delivered = pf_deliver(&mut state, &mut memory);
        assert!(
            matches!(
                &delivered,
                od_isa::Cpl0PageFaultDelivery::Aborted { phase: got, .. } if *got == phase
            ),
            "{delivered:?}"
        );
        assert_eq!(state, before);
        assert_eq!(memory.writes, 0);
    }
}

// ---- S4-38L6b: WRMSR/RDMSR IA32_GS_BASE / IA32_FS_BASE in the CPL0 set ----

const GS_BASE_MSR: u64 = 0xc000_0101;
const FS_BASE_MSR: u64 = 0xc000_0100;

fn gs_set() -> Cpl0ProfileSet<'static> {
    base().with_msr_fs_gs_base()
}

/// Run `code` once at `privilege` with RCX/RAX/RDX set; returns state, before, result.
fn run_msr_base(
    set: &Cpl0ProfileSet<'_>,
    privilege: PrivilegeLevel,
    code: &[u8],
    (rcx, rax, rdx): (u64, u64, u64),
) -> (LongModeState, LongModeState, Cpl0ProfileSetResult) {
    let mut state = board_state();
    state.gpr[1] = rcx;
    state.gpr[0] = rax;
    state.gpr[2] = rdx;
    state.gs_base = 0xffff_8000_1234_5678;
    state.fs_base = 0x7fff_0000_0000;
    let before = state.clone();
    let mut memory = Memory::code(code);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(privilege, 1),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap();
    (state, before, result)
}

fn gp0() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

#[test]
fn wrmsr_gs_base_zero_is_the_real_kernel_entry_case() {
    let (state, before, result) = run_msr_base(
        &gs_set(),
        PrivilegeLevel::Ring0,
        &[0x0f, 0x30],
        (GS_BASE_MSR, 0, 0),
    );
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gs_base, 0);
    assert_eq!(state.fs_base, before.fs_base);
    assert_eq!(state.kernel_gs_base, before.kernel_gs_base);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.gpr, before.gpr);
}

#[test]
fn wrmsr_gs_and_fs_base_store_edx_eax_canonical_values() {
    for (index, value) in [
        (GS_BASE_MSR, 0xffff_8880_0000_1000u64),
        (GS_BASE_MSR, 0x0000_7fff_ffff_ffff),
        (FS_BASE_MSR, 0x0000_7fff_dead_beef),
        (FS_BASE_MSR, 0xffff_8000_0000_0000),
    ] {
        let (state, before, result) = run_msr_base(
            &gs_set(),
            PrivilegeLevel::Ring0,
            &[0x0f, 0x30],
            (index, value & 0xffff_ffff, value >> 32),
        );
        assert_eq!(result.instructions_executed, 1, "{index:#x} {value:#x}");
        if index == GS_BASE_MSR {
            assert_eq!(state.gs_base, value);
            assert_eq!(state.fs_base, before.fs_base);
        } else {
            assert_eq!(state.fs_base, value);
            assert_eq!(state.gs_base, before.gs_base);
        }
        assert_eq!(state.rip, RIP + 2);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(state.gpr, before.gpr);
    }
}

#[test]
fn wrmsr_gs_base_non_canonical_is_gp0_without_mutation() {
    for value in [0x0000_8000_0000_0000u64, 0xffff_7fff_ffff_ffff, 1 << 63] {
        let (state, before, result) = run_msr_base(
            &gs_set(),
            PrivilegeLevel::Ring0,
            &[0x0f, 0x30],
            (GS_BASE_MSR, value & 0xffff_ffff, value >> 32),
        );
        assert_eq!(exit(&result), &gp0(), "{value:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn wrmsr_and_rdmsr_gs_base_at_cpl3_never_reach_the_msr_member() {
    // The set is CPL0 only: a CPL3 run is rejected at entry, before any fetch.
    for code in [[0x0f, 0x30], [0x0f, 0x32]] {
        let mut state = board_state();
        state.gpr[1] = GS_BASE_MSR;
        let before = state.clone();
        let mut memory = Memory::code(&code);
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 1),
            od_isa::INTEGER_FLAGS,
            &gs_set(),
            None,
            || false,
        );
        assert_eq!(
            result.unwrap_err(),
            Cpl0ProfileSetInputError::WrongPrivilege
        );
        assert_eq!(state, before);
        assert_eq!(memory.fetches, 0);
    }
}

#[test]
fn msr_index_is_ecx_only_like_the_legacy_profiles() {
    // RCX[63:32] is ignored (SDM: the index is ECX), as in the FS/GS profiles.
    let (state, _, result) = run_msr_base(
        &gs_set(),
        PrivilegeLevel::Ring0,
        &[0x0f, 0x30],
        (GS_BASE_MSR | (1 << 32), 0, 0),
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gs_base, 0);
}

#[test]
fn wrmsr_other_msrs_stay_unsupported_without_mutation() {
    // APIC_BASE, SYSENTER_CS, EFER, KERNEL_GS_BASE, STAR, and an unknown index.
    for index in [
        0x1bu64,
        0x174,
        0xc000_0080,
        0xc000_0102,
        0xc000_0081,
        0x1234_5678,
    ] {
        let (state, before, result) = run_msr_base(
            &gs_set(),
            PrivilegeLevel::Ring0,
            &[0x0f, 0x30],
            (index, 0, 0),
        );
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{index:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn wrmsr_gs_base_without_the_member_is_unsupported() {
    let efer = MsrProfile::EferReadOnlyV1(0xd00);
    for set in [base(), base().with_msr(&efer)] {
        let (state, before, result) = run_msr_base(
            &set,
            PrivilegeLevel::Ring0,
            &[0x0f, 0x30],
            (GS_BASE_MSR, 0, 0),
        );
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn wrmsr_gs_base_rejects_prefixed_forms() {
    for code in [[0x66u8, 0x0f, 0x30], [0x48, 0x0f, 0x30]] {
        let (state, before, result) =
            run_msr_base(&gs_set(), PrivilegeLevel::Ring0, &code, (GS_BASE_MSR, 0, 0));
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
        assert_eq!(state, before);
    }
}

#[test]
fn rdmsr_gs_and_fs_base_return_the_stored_value_in_edx_eax() {
    for (index, expected) in [
        (GS_BASE_MSR, 0xffff_8000_1234_5678u64),
        (FS_BASE_MSR, 0x7fff_0000_0000),
    ] {
        let (state, before, result) = run_msr_base(
            &gs_set(),
            PrivilegeLevel::Ring0,
            &[0x0f, 0x32],
            (index, u64::MAX, u64::MAX),
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[0], expected & 0xffff_ffff);
        assert_eq!(state.gpr[2], expected >> 32);
        assert_eq!(state.rip, RIP + 2);
        assert_eq!(state.gs_base, before.gs_base);
        assert_eq!(state.fs_base, before.fs_base);
    }
}

#[test]
fn wrmsr_then_rdmsr_gs_base_round_trips_in_one_run() {
    // mov ecx,0xc0000101; mov eax,0x1000; mov edx,0xffff8880; wrmsr;
    // xor eax,eax; xor edx,edx; rdmsr
    let code: &[u8] = &[
        0xb9, 0x01, 0x01, 0x00, 0xc0, 0xb8, 0x00, 0x10, 0x00, 0x00, 0xba, 0x80, 0x88, 0xff, 0xff,
        0x0f, 0x30, 0x31, 0xc0, 0x31, 0xd2, 0x0f, 0x32,
    ];
    let mut state = board_state();
    let mut memory = Memory::code(code);
    let result = run_set(&mut state, &mut memory, &gs_set(), 7).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 7);
    assert_eq!(state.gs_base, 0xffff_8880_0000_1000);
    assert_eq!(state.gpr[0], 0x1000);
    assert_eq!(state.gpr[2], 0xffff_8880);
}

#[test]
fn gs_base_member_keeps_the_selected_efer_profile() {
    let efer = MsrProfile::EferReadOnlyV1(0xd00);
    let set = base().with_msr(&efer).with_msr_fs_gs_base();
    let (state, _, result) = run_msr_base(
        &set,
        PrivilegeLevel::Ring0,
        &[0x0f, 0x32],
        (0xc000_0080, 0, 0),
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 0xd00);
    // EFER stays read-only: WRMSR is Unsupported, GS_BASE is writable.
    let (state, before, result) = run_msr_base(
        &set,
        PrivilegeLevel::Ring0,
        &[0x0f, 0x30],
        (0xc000_0080, 0, 0),
    );
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
    assert_eq!(state, before);
    let (state, _, result) = run_msr_base(
        &set,
        PrivilegeLevel::Ring0,
        &[0x0f, 0x30],
        (GS_BASE_MSR, 0, 0),
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gs_base, 0);
}
