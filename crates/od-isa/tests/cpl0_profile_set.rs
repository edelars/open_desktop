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
use std::cell::Cell;

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
            |s| s.cr4 = Some(0x1020),
            Cpl0ProfileSetInputError::Cr4OutsidePolicy { value: 0x1020 },
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
    // 0x1020 adds LA57 (bit 12); bit 17 is PCIDE, bit 18 OSXSAVE, bit 2 TSD and
    // bit 40 is reserved: all outside the CR4 policy (only PSE/PGE/OSFXSR/
    // OSXMMEXCPT may vary). PAE cleared is outside as well.
    for value in [
        0x1020u64,
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
    // Architecturally these are not #UD (0x66 is an operand-size prefix that MOV CR
    // ignores in 64-bit mode); they are outside the set's forms, so the decoder
    // reports the opcode byte `0x22` as unsupported. REX.W and REX.B forms of
    // `MOV CR3, r64` are supported since S4-38L7h (see `mov_cr3_r64_*` below).
    for bytes in [
        &[0x66, 0x0f, 0x22, 0xd8][..],
        &[0x66, 0x0f, 0x22, 0xdf][..],
        &[0x44, 0x0f, 0x22, 0xdf][..],
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

// ---- S4-38L7h: MOV CR3, r64 from any general register ------------------------

/// The Linux CR3 written by the real kernel (`RDI` in the S4-38L7g boundary).
const CR3_LINUX: u64 = 0x3ffe000;

#[test]
fn mov_cr3_r64_writes_rdi_rbx_r8_and_r15_and_follows_the_new_cr3() {
    for (bytes, src) in [
        (&[0x0f, 0x22, 0xdf][..], Register::Rdi),
        (&[0x0f, 0x22, 0xdb][..], Register::Rbx),
        (&[0x41, 0x0f, 0x22, 0xd8][..], Register::R8),
        (&[0x41, 0x0f, 0x22, 0xdf][..], Register::R15),
    ] {
        let mut state = cr3_state(0x1000);
        state.gpr[src as usize] = CR3_LINUX;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
        assert_eq!(result.exit, cr3_write(0x1000, CR3_LINUX), "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
        let mut expected = before.clone();
        expected.cr3 = Some(CR3_LINUX);
        expected.rip = RIP + bytes.len() as u64;
        assert_eq!(state, expected, "{bytes:02x?}");
    }
}

#[test]
fn mov_cr3_r64_with_any_rex_but_r_reads_the_selected_register_only() {
    for rm in 0u8..8 {
        for rex in [
            None,
            Some(0x40u8),
            Some(0x41),
            Some(0x48),
            Some(0x49),
            Some(0x4b),
        ] {
            let mut bytes = Vec::new();
            bytes.extend(rex);
            bytes.extend_from_slice(&[0x0f, 0x22, 0xd8 | rm]);
            let src = (rm | ((rex.unwrap_or(0) & 1) << 3)) as usize;
            let mut state = cr3_state(0x1000);
            // The register with the other REX.B value holds an out-of-range CR3.
            state.gpr[src ^ 8] = 1 << 40;
            state.gpr[src] = CR3_LINUX;
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
            assert_eq!(result.exit, cr3_write(0x1000, CR3_LINUX), "{bytes:02x?}");
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            let mut expected = before.clone();
            expected.cr3 = Some(CR3_LINUX);
            expected.rip = RIP + bytes.len() as u64;
            assert_eq!(state, expected, "{bytes:02x?}");
        }
    }
}

#[test]
fn mov_cr3_r64_beyond_36_bits_is_gp0_without_mutation() {
    for (bytes, src) in [
        (&[0x0f, 0x22, 0xdf][..], Register::Rdi),
        (&[0x41, 0x0f, 0x22, 0xd8][..], Register::R8),
        (&[0x41, 0x0f, 0x22, 0xdf][..], Register::R15),
    ] {
        for value in [1u64 << 36, 1 << 40, u64::MAX] {
            let mut state = cr3_state(0x1000);
            state.gpr[src as usize] = value;
            let before = state.clone();
            let mut memory = Memory::code(bytes);
            let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
            assert_eq!(exit(&result), &gp0(), "{bytes:02x?} {value:#x}");
            assert_eq!(result.instructions_executed, 0, "{bytes:02x?} {value:#x}");
            assert_eq!(state, before, "{bytes:02x?} {value:#x}");
        }
    }
}

#[test]
fn mov_cr3_r64_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0xf0, 0x0f, 0x22, 0xdf][..], // LOCK with a non-RAX source
        &[0x44, 0x0f, 0x22, 0xdf][..], // REX.R: CR11
        &[0x4d, 0x0f, 0x22, 0xdf][..],
        &[0x0f, 0x22, 0x1f][..], // mod 00: conservative refusal (SDM not verified)
        &[0x0f, 0x22, 0x5f, 0x08][..], // mod 01
        &[0x0f, 0x22, 0x9f, 0, 0, 0, 0][..], // mod 10
        &[0x66, 0x0f, 0x22, 0xdf][..],
        &[0xf3, 0x0f, 0x22, 0xdf][..],
        &[0x67, 0x0f, 0x22, 0xdf][..],
        &[0x41, 0x66, 0x0f, 0x22, 0xdf][..],
        &[0x40, 0x48, 0x0f, 0x22, 0xdf][..], // two REX prefixes
    ] {
        let mut state = cr3_state(0x1000);
        state.gpr[Register::Rdi as usize] = CR3_LINUX;
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
fn lock_mov_cr3_r64_is_unsupported_under_the_cr3_set() {
    let mut state = cr3_state(0x1000);
    state.gpr[Register::Rdi as usize] = CR3_LINUX;
    let before = state.clone();
    let mut memory = Memory::code(&[0xf0, 0x0f, 0x22, 0xdf]);
    let result = run_set(&mut state, &mut memory, &cr3_set(), 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x22 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn mov_cr3_r64_stays_unsupported_without_cr3_ownership() {
    for bytes in [&[0x0f, 0x22, 0xdf][..], &[0x41, 0x0f, 0x22, 0xd8][..]] {
        let mut state = board_state();
        state.gpr[Register::Rdi as usize] = CR3_LINUX;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x22 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_cr3_r64_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch, as for the RAX form.
    let mut state = cr3_state(0x1000);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xdf]);
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

// ---- S4-38L7b: MOV r64, CR4 into any general register ----------------------

/// A CR4 inside the CPL0 policy that differs from `CR4_BASE` (OSFXSR|OSXMMEXCPT).
const CR4_DISTINCT: u64 = CR4_BASE | 0x600;

fn cr4_any_reg_state(cr4: u64) -> LongModeState {
    let mut state = board_state();
    state.cr4 = Some(cr4);
    state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
    for (index, gpr) in state.gpr.iter_mut().enumerate() {
        *gpr = 0x1111_0000_0000_0000 + index as u64;
    }
    state.gpr[Register::Rsp as usize] = 0x7000;
    state
}

#[test]
fn mov_rdx_cr4_is_the_real_kernel_form() {
    // `0F 20 E2` at RIP 0x3f13352 in the real kernel (CR4.LA57 probe).
    for cr4 in [CR4_BASE, CR4_DISTINCT] {
        let mut state = cr4_any_reg_state(cr4);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
        assert_eq!(state.gpr[Register::Rdx as usize], cr4);
        assert_eq!(
            state,
            cr2_any_reg_expected(&before, Register::Rdx as usize, cr4, 3)
        );
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(state.cr4, Some(cr4));
    }
}

#[test]
fn mov_r64_cr4_writes_every_low_register_and_touches_nothing_else() {
    for rm in 0u8..8 {
        for rex in [None, Some(0x40u8), Some(0x48), Some(0x42), Some(0x4a)] {
            let mut bytes = Vec::new();
            bytes.extend(rex);
            bytes.extend_from_slice(&[0x0f, 0x20, 0xe0 | rm]);
            let mut state = cr4_any_reg_state(CR4_DISTINCT);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{bytes:02x?}"
            );
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
            assert_eq!(
                state,
                cr2_any_reg_expected(&before, rm as usize, CR4_DISTINCT, bytes.len() as u64),
                "{bytes:02x?}"
            );
            assert_eq!(state.rflags, before.rflags, "{bytes:02x?}");
        }
    }
}

#[test]
fn mov_r64_cr4_with_rex_b_writes_r8_to_r15() {
    for rm in 0u8..8 {
        for rex in [0x41u8, 0x49, 0x43, 0x4b] {
            let bytes = [rex, 0x0f, 0x20, 0xe0 | rm];
            let mut state = cr4_any_reg_state(CR4_DISTINCT);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(
                state,
                cr2_any_reg_expected(&before, 8 + rm as usize, CR4_DISTINCT, 4),
                "{bytes:02x?}"
            );
            assert_eq!(state.gpr[rm as usize], before.gpr[rm as usize]);
        }
    }
    // `41 0F 20 E0` is MOV R8, CR4.
    let mut state = cr4_any_reg_state(CR4_BASE);
    let mut memory = Memory::code(&[0x41, 0x0f, 0x20, 0xe0]);
    run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(state.gpr[Register::R8 as usize], CR4_BASE);
}

#[test]
fn mov_r64_cr4_works_in_the_cr3_owning_set_too() {
    let mut state = cr2_state(Some(CR2_DISTINCT));
    state.cr4 = Some(CR4_DISTINCT);
    let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
    run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(state.gpr[Register::Rdx as usize], CR4_DISTINCT);
    assert_eq!(state.rip, RIP + 3);
}

#[test]
fn mov_r64_cr4_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0x44, 0x0f, 0x20, 0xe0][..],       // REX.R: CR12
        &[0x4d, 0x0f, 0x20, 0xe7][..],       // REX.W+R+B: CR12 into R15
        &[0x0f, 0x20, 0x23][..],             // mod 00
        &[0x0f, 0x20, 0x63, 0x08][..],       // mod 01
        &[0x0f, 0x20, 0xa3, 0, 0, 0, 0][..], // mod 10
        &[0x44, 0x0f, 0x22, 0xe0][..],       // REX.R on the write: CR12
        &[0x66, 0x0f, 0x20, 0xe2][..],
        &[0x66, 0x41, 0x0f, 0x20, 0xe2][..],
        &[0xf3, 0x0f, 0x20, 0xe2][..],
        &[0xf2, 0x0f, 0x20, 0xe2][..],
        &[0x67, 0x0f, 0x20, 0xe2][..],
        &[0x2e, 0x0f, 0x20, 0xe2][..],
        &[0x64, 0x0f, 0x20, 0xe2][..],
        &[0x41, 0x66, 0x0f, 0x20, 0xe2][..],
        &[0x40, 0x41, 0x0f, 0x20, 0xe2][..],
        &[0x0f, 0x20, 0xea][..], // CR5: not a modeled register
    ] {
        let mut state = cr4_any_reg_state(CR4_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
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
fn lock_mov_r64_cr4_does_not_execute() {
    // LOCK before MOV CR is `#UD` for the RAX forms and closed for the others;
    // either way nothing retires and nothing is mutated.
    for bytes in [
        &[0xf0, 0x0f, 0x20, 0xe2][..],
        &[0xf0, 0x41, 0x0f, 0x20, 0xe0][..],
    ] {
        let mut state = cr4_any_reg_state(CR4_DISTINCT);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_ne!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{bytes:02x?}"
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_r64_cr4_with_unmodeled_cr4_is_rejected_without_mutation() {
    // The set refuses a missing CR4 at entry (nothing is fetched).
    let mut state = cr4_any_reg_state(CR4_BASE);
    state.cr4 = None;
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
    let error = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::Cr4Missing);
    assert_eq!((memory.fetches, &state), (0, &before));
}

#[test]
fn mov_r64_cr4_stays_unsupported_outside_the_set() {
    for bytes in [&[0x0f, 0x20, 0xe2][..], &[0x41, 0x0f, 0x20, 0xe0][..]] {
        let mut state = LongModeState {
            rip: RIP,
            rflags: 2 | IF,
            cr4: None,
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
fn mov_r64_cr4_is_not_reachable_from_cpl3() {
    let mut state = cr4_any_reg_state(CR4_BASE);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

#[test]
fn mov_rax_cr4_keeps_its_result() {
    let mut state = cr4_any_reg_state(CR4_DISTINCT);
    let before = state.clone();
    let mut memory = Memory::code(&MOV_RAX_CR4);
    let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, cr2_any_reg_expected(&before, 0, CR4_DISTINCT, 3));
}

// ---- S4-38L7c: MOV CR4, r64 from any general register ----------------------

/// A CR4 inside the CPL0 policy that adds PSE (the value the real kernel writes).
const CR4_PSE_PAE: u64 = 0x30;

fn mov_cr4_state(src: usize, value: u64) -> LongModeState {
    let mut state = cr4_any_reg_state(CR4_BASE);
    state.gpr[src] = value;
    state
}

#[test]
fn mov_cr4_rcx_with_pse_and_pae_is_the_real_kernel_form() {
    // `0F 22 E1` at 0xffffffff8139c0c0 in the real kernel, RCX = 0x30.
    let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PSE_PAE);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
    assert_eq!(state.cr4, Some(CR4_PSE_PAE));
    let mut expected = before.clone();
    expected.cr4 = Some(CR4_PSE_PAE);
    expected.rip = RIP + 3;
    assert_eq!(state, expected);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.cr0, before.cr0);
}

#[test]
fn mov_cr4_r64_reads_every_register_and_touches_nothing_else() {
    for rm in 0u8..8 {
        for rex in [None, Some(0x40u8), Some(0x48), Some(0x42), Some(0x4a)] {
            let mut bytes = Vec::new();
            bytes.extend(rex);
            bytes.extend_from_slice(&[0x0f, 0x22, 0xe0 | rm]);
            let mut state = mov_cr4_state(rm as usize, CR4_PSE_PAE | 0x600);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
            let mut expected = before.clone();
            expected.cr4 = Some(CR4_PSE_PAE | 0x600);
            expected.rip = RIP + bytes.len() as u64;
            assert_eq!(state, expected, "{bytes:02x?}");
        }
    }
}

#[test]
fn mov_cr4_r64_with_rex_b_reads_r8_to_r15() {
    for rm in 0u8..8 {
        for rex in [0x41u8, 0x49, 0x43, 0x4b] {
            let bytes = [rex, 0x0f, 0x22, 0xe0 | rm];
            // Only the selected extended register holds the value; the low one
            // of the same encoding holds an out-of-policy value.
            let mut state = mov_cr4_state(rm as usize, 0x1000);
            state.gpr[8 + rm as usize] = CR4_PSE_PAE;
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            let mut expected = before.clone();
            expected.cr4 = Some(CR4_PSE_PAE);
            expected.rip = RIP + 4;
            assert_eq!(state, expected, "{bytes:02x?}");
        }
    }
    // `41 0F 22 E0` is MOV CR4, R8.
    let mut state = mov_cr4_state(Register::R8 as usize, CR4_PSE_PAE);
    let mut memory = Memory::code(&[0x41, 0x0f, 0x22, 0xe0]);
    run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(state.cr4, Some(CR4_PSE_PAE));
}

#[test]
fn mov_cr4_r64_without_pae_is_gp0_without_mutation() {
    for value in [0x10u64, 0, 0x610, 0x600] {
        let mut state = mov_cr4_state(Register::Rcx as usize, value);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(exit(&result), &gp0(), "{value:#x}");
        assert_eq!(result.instructions_executed, 0, "{value:#x}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{value:#x}");
        assert_eq!(state, before, "{value:#x}");
    }
}

#[test]
fn mov_cr4_r64_with_bits_outside_the_policy_is_unsupported_without_mutation() {
    for bit in [2u32, 3, 6, 8, 11, 12, 14, 16, 17, 18, 20, 21, 22, 40, 63] {
        let value = CR4_PSE_PAE | (1u64 << bit);
        let mut state = mov_cr4_state(Register::Rcx as usize, value);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x22 },
            "bit {bit}"
        );
        assert_eq!(result.instructions_executed, 0, "bit {bit}");
        assert_eq!(state, before, "bit {bit}");
    }
}

#[test]
fn mov_cr4_r64_accepts_exactly_the_policy_masks() {
    for value in [
        0x20u64, 0x30, 0x220, 0x420, 0x620, 0x230, 0x430, 0x630, 0xa0, 0xb0, 0x2a0, 0x6b0,
    ] {
        let mut state = mov_cr4_state(Register::Rdx as usize, value);
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe2]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
        assert_eq!(result.instructions_executed, 1, "{value:#x}");
        assert_eq!(state.cr4, Some(value), "{value:#x}");
    }
}

#[test]
fn cr4_with_pse_is_admitted_on_reentry_and_by_the_rax_write() {
    // The board re-enters the set with the CR4 the previous run left behind.
    let mut state = cr4_any_reg_state(CR4_PSE_PAE);
    let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
    run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(state.gpr[Register::Rdx as usize], CR4_PSE_PAE);
    // The RAX form shares the set policy, so PSE is accepted there too.
    let mut code = movabs_rax(CR4_PSE_PAE);
    code.extend_from_slice(&MOV_CR4_RAX);
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr_set(), 2).unwrap();
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.cr4, Some(CR4_PSE_PAE));
}

#[test]
fn mov_cr4_r64_other_forms_are_unsupported_without_mutation() {
    for bytes in [
        &[0x44, 0x0f, 0x22, 0xe1][..], // REX.R: CR12
        &[0x4d, 0x0f, 0x22, 0xe1][..],
        &[0x0f, 0x22, 0x21][..],             // mod 00
        &[0x0f, 0x22, 0x61, 0x08][..],       // mod 01
        &[0x0f, 0x22, 0xa1, 0, 0, 0, 0][..], // mod 10
        &[0x66, 0x0f, 0x22, 0xe1][..],
        &[0x66, 0x41, 0x0f, 0x22, 0xe0][..],
        &[0xf3, 0x0f, 0x22, 0xe1][..],
        &[0xf2, 0x0f, 0x22, 0xe1][..],
        &[0x67, 0x0f, 0x22, 0xe1][..],
        &[0x2e, 0x0f, 0x22, 0xe1][..],
        &[0x64, 0x0f, 0x22, 0xe1][..],
        &[0x41, 0x66, 0x0f, 0x22, 0xe1][..],
        &[0x40, 0x41, 0x0f, 0x22, 0xe1][..],
        &[0x0f, 0x22, 0xe9][..],       // CR5: not a modeled register
        &[0x41, 0x0f, 0x22, 0xc0][..], // CR0 stays RAX-only
        &[0x0f, 0x22, 0xc1][..],
        &[0x0f, 0x22, 0xdf][..], // MOV CR3, RDI: outside this CR4-only set
        &[0x41, 0x0f, 0x22, 0xd8][..],
    ] {
        let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PSE_PAE);
        state.gpr[8] = CR4_PSE_PAE;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert!(
            matches!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x22 })
            ),
            "{bytes:02x?}: {:?}",
            result.exit
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn lock_mov_cr4_r64_does_not_execute() {
    for bytes in [
        &[0xf0, 0x0f, 0x22, 0xe1][..],
        &[0xf0, 0x41, 0x0f, 0x22, 0xe0][..],
    ] {
        let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PSE_PAE);
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_ne!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{bytes:02x?}"
        );
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_cr4_r64_works_in_the_cr3_owning_set_too() {
    let mut state = cr2_state(Some(CR2_DISTINCT));
    state.gpr[Register::Rcx as usize] = CR4_PSE_PAE;
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    run_set(&mut state, &mut memory, &cr3_set(), 1).unwrap();
    assert_eq!(state.cr4, Some(CR4_PSE_PAE));
    assert_eq!(state.rip, RIP + 3);
}

#[test]
fn mov_cr4_r64_stays_unsupported_outside_the_set() {
    for bytes in [&[0x0f, 0x22, 0xe1][..], &[0x41, 0x0f, 0x22, 0xe0][..]] {
        let mut state = LongModeState {
            rip: RIP,
            rflags: 2 | IF,
            cr4: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rcx as usize] = CR4_PSE_PAE;
        state.gpr[8] = CR4_PSE_PAE;
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
            RunExit::Unsupported { opcode: 0x22 },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }
}

#[test]
fn mov_cr4_r64_is_not_reachable_from_cpl3() {
    let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PSE_PAE);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

#[test]
fn mov_cr4_rax_write_keeps_its_result() {
    // Regression: the bare `0F 22 E0` is still the RAX write.
    let mut state = mov_cr4_state(Register::Rax as usize, CR4_BASE | 0x600);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe0]);
    let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    expected.cr4 = Some(CR4_BASE | 0x600);
    expected.rip = RIP + 3;
    assert_eq!(state, expected);
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

// ---- S4-38L7w: PCI configuration mechanism #1 ports (dword/word IN/OUT) ----

const PCI_RAX: u64 = 0x1111_2222_3333_4444;

fn pci_set<'a>(cli: &'a CliProfile, cell: &'a Cell<u32>) -> Cpl0ProfileSet<'a> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(cli)
        .with_pci_config(cell)
}

type PciRun = (
    LongModeState,
    LongModeState,
    Cpl0ProfileSetResult<&'static str>,
    TestPio,
);

/// Runs `code` with RAX/RDX preset through the PIO entry. Returns the state before the run too.
fn run_pci(code: &[u8], rax: u64, rdx: u64, cell: &Cell<u32>, budget: u64) -> PciRun {
    let cli = CliProfile::NonPviLong64V1;
    let mut state = board_state();
    state.gpr[0] = rax;
    state.gpr[2] = rdx;
    let before = state.clone();
    let mut memory = Memory::code(code);
    let mut pio = TestPio::default();
    let result = run_pio(
        &mut state,
        &mut memory,
        &pci_set(&cli, cell),
        &mut pio,
        budget,
    );
    (before, state, result, pio)
}

fn budget_exhausted() -> Cpl0ProfileSetExit<&'static str> {
    Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
}

#[test]
fn pci_dword_out_to_config_address_is_stored_and_read_back_by_in() {
    let cell = Cell::new(0);
    // Non-zero high RAX bits: OUT leaves RAX alone, IN EAX zero-extends into RAX.
    let (before, state, result, pio) =
        run_pci(&[0xef, 0xed], 0xaaaa_bbbb_8000_1000, 0xcf8, &cell, 2);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(cell.get(), 0x8000_1000);
    assert_eq!(state.gpr[0], 0x0000_0000_8000_1000);
    assert_eq!(state.gpr[2], 0xcf8);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.rflags, before.rflags);
    assert!(pio.reads.is_empty() && pio.writes.is_empty());

    // The OUT alone leaves RAX and everything but RIP and the cell untouched.
    let cell = Cell::new(0);
    let (before, state, result, _) = run_pci(&[0xef], 0xaaaa_bbbb_8000_1000, 0xcf8, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0x8000_1000);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.rip, RIP + 1);
}

#[test]
fn pci_word_access_to_config_address_is_refused_without_mutation() {
    // The word form of CONFIG_ADDRESS has no hardware-confirmed semantics: both 66 EF and
    // 66 ED are refused with the exact opcode, with no change to the registers or the address.
    for (code, opcode) in [(&[0x66, 0xef][..], 0xef), (&[0x66, 0xed][..], 0xed)] {
        let cell = Cell::new(0xdead_beef);
        let (before, state, result, pio) = run_pci(code, PCI_RAX, 0xcf8, &cell, 8);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode }),
            "{code:x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0xdead_beef);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn pci_port_is_truncated_to_dx() {
    // RDX[63:16] is ignored: the port is DX.
    let cell = Cell::new(0);
    let (_, state, result, _) = run_pci(&[0xef], 0x8000_1000, 0xffff_0000_0001_0cf8, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(cell.get(), 0x8000_1000);
    assert_eq!(state.rip, RIP + 1);
    let (_, state, result, _) = run_pci(&[0xed], PCI_RAX, 0xffff_0000_0001_0cf8, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(state.gpr[0], 0x8000_1000);
    // The same for the data port.
    let (_, state, result, _) = run_pci(&[0xed], PCI_RAX, 0xffff_0000_0001_0cfc, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(state.gpr[0], 0xffff_ffff);
}

#[test]
fn pci_config_address_is_kept_between_runs_on_the_same_cell() {
    let cell = Cell::new(0);
    let (_, _, result, _) = run_pci(&[0xef], 0x8000_1000, 0xcf8, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    let (_, state, result, _) = run_pci(&[0xed], 0xaaaa_bbbb_cccc_dddd, 0xcf8, &cell, 1);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(state.gpr[0], 0x8000_1000);
}

#[test]
fn pci_config_data_in_without_a_device_returns_all_ones() {
    for port in [0xcfc_u64, 0xcfd, 0xcfe, 0xcff] {
        // Dword: EAX = 0xffff_ffff, RAX[63:32] cleared.
        let cell = Cell::new(0x8000_0000);
        let (before, state, result, pio) = run_pci(&[0xed], 0xaaaa_bbbb_cccc_dddd, port, &cell, 1);
        assert_eq!(result.exit, budget_exhausted(), "{port:#x}");
        assert_eq!(state.gpr[0], 0x0000_0000_ffff_ffff, "{port:#x}");
        assert_eq!(state.rip, RIP + 1);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(cell.get(), 0x8000_0000);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
        // Word: AX = 0xffff, RAX[63:16] kept.
        let (_, state, result, _) = run_pci(&[0x66, 0xed], PCI_RAX, port, &cell, 1);
        assert_eq!(result.exit, budget_exhausted(), "{port:#x}");
        assert_eq!(state.gpr[0], 0x1111_2222_3333_ffff, "{port:#x}");
        assert_eq!(state.rip, RIP + 2);
    }
}

#[test]
fn pci_config_data_out_is_discarded_and_keeps_the_address() {
    for (code, length) in [(&[0xef][..], 1u64), (&[0x66, 0xef][..], 2)] {
        for port in [0xcfc_u64, 0xcfe] {
            let cell = Cell::new(0x8000_1000);
            let (before, state, result, pio) = run_pci(code, PCI_RAX, port, &cell, 1);
            assert_eq!(result.exit, budget_exhausted());
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(cell.get(), 0x8000_1000);
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
            assert_eq!(state.rip, RIP + length);
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn pci_guest_sequence_address_then_data_read_is_all_ones() {
    let cell = Cell::new(0);
    let code = [
        0xef, // out dx, eax
        0xba, 0xfc, 0x0c, 0x00, 0x00, // mov edx, 0xcfc
        0xed, // in eax, dx
    ];
    let (_, state, result, _) = run_pci(&code, 0x8000_1000, 0xcf8, &cell, 3);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(cell.get(), 0x8000_1000);
    assert_eq!(state.gpr[0], 0xffff_ffff);
    assert_eq!(state.rip, RIP + 7);
}

#[test]
fn pci_other_ports_are_unsupported_without_mutation() {
    for (code, opcode) in [
        (&[0xef][..], 0xef),
        (&[0x66, 0xef][..], 0xef),
        (&[0xed][..], 0xed),
        (&[0x66, 0xed][..], 0xed),
    ] {
        for port in [0x3f8_u64, 0xcf9, 0xcfa, 0xcfb, 0xd00, 0xcf0, 0x0] {
            let cell = Cell::new(0x1234_5678);
            let (before, state, result, pio) = run_pci(code, PCI_RAX, port, &cell, 8);
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode }),
                "{code:x?} {port:#x}"
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
            assert_eq!(cell.get(), 0x1234_5678);
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn pci_prefixed_forms_are_refused_with_the_exact_opcode_and_no_mutation() {
    for (code, opcode) in [
        (&[0xf3, 0xef][..], 0xef), // REP OUTSD is not this instruction
        (&[0xf3, 0xed][..], 0xed),
        (&[0xf2, 0xef][..], 0xef),
        (&[0xf2, 0xed][..], 0xed),
        (&[0xf0, 0xef][..], 0xef),
        (&[0xf0, 0xed][..], 0xed),
        (&[0x48, 0xef][..], 0xef), // REX.W
        (&[0x48, 0xed][..], 0xed),
        (&[0x66, 0x48, 0xef][..], 0xef),
        (&[0x48, 0x66, 0xef][..], 0xef), // REX before 66: refused, not an ignored REX
        (&[0x40, 0x66, 0xed][..], 0xed),
        (&[0x40, 0xef][..], 0xef), // any REX
        (&[0x67, 0xef][..], 0xef),
        (&[0x67, 0xed][..], 0xed),
        (&[0x2e, 0xef][..], 0xef), // segment overrides
        (&[0x64, 0xed][..], 0xed),
        (&[0x65, 0xef][..], 0xef),
        (&[0x66, 0xf3, 0xef][..], 0xef),
        (&[0x66, 0x66, 0xef][..], 0xef), // two operand-size bytes: not the exact word form
        (&[0x66, 0x67, 0xed][..], 0xed),
    ] {
        for port in [0xcf8_u64, 0xcfc] {
            let cell = Cell::new(0x1234_5678);
            let (before, state, result, pio) = run_pci(code, PCI_RAX, port, &cell, 8);
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode }),
                "{code:x?}"
            );
            assert_eq!(result.instructions_executed, 0, "{code:x?}");
            assert_eq!(state, before, "{code:x?}");
            assert_eq!(cell.get(), 0x1234_5678);
            assert!(pio.reads.is_empty() && pio.writes.is_empty());
        }
    }
}

#[test]
fn pci_forms_are_outside_a_set_without_the_member() {
    let cli = CliProfile::NonPviLong64V1;
    for code in [
        &[0xef][..],
        &[0x66, 0xef][..],
        &[0xed][..],
        &[0x66, 0xed][..],
        &[0x48, 0x66, 0xef][..],
        &[0x40, 0x66, 0xed][..],
    ] {
        let opcode = *code.last().unwrap();
        // The byte PIO member alone does not admit the PCI forms.
        let mut state = board_state();
        state.gpr[2] = 0xcf8;
        let before = state.clone();
        let mut memory = Memory::code(code);
        let mut pio = TestPio::default();
        let result = run_pio(&mut state, &mut memory, &pio_set(&cli), &mut pio, 8);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode })
        );
        assert_eq!(state, before);
        // The PCI member through an entry without a port bridge stays Unsupported.
        let cell = Cell::new(0);
        let mut state = board_state();
        state.gpr[2] = 0xcf8;
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &pci_set(&cli, &cell), 8).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0);
    }
}

#[test]
fn pci_member_does_not_admit_the_byte_forms() {
    let cli = CliProfile::NonPviLong64V1;
    let cell = Cell::new(0);
    for (code, opcode) in [(&[0xee][..], 0xee), (&[0xec][..], 0xec)] {
        let mut state = board_state();
        state.gpr[2] = 0x3f8;
        let before = state.clone();
        let mut memory = Memory::code(code);
        let mut pio = TestPio::default();
        let result = run_pio(&mut state, &mut memory, &pci_set(&cli, &cell), &mut pio, 8);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode })
        );
        assert_eq!(state, before);
        assert!(pio.reads.is_empty() && pio.writes.is_empty());
    }
}

#[test]
fn pci_member_together_with_pio_keeps_the_byte_forms_unchanged() {
    let cli = CliProfile::NonPviLong64V1;
    let cell = Cell::new(0);
    let set = pio_set(&cli).with_pci_config(&cell);
    let mut state = board_state();
    let mut memory = Memory::code(&pio_code(OUT_DX_AL));
    let mut pio = TestPio::default();
    let result = run_pio(&mut state, &mut memory, &set, &mut pio, 3);
    assert_eq!(result.exit, budget_exhausted());
    assert_eq!(pio.writes, [(0x3f8, 0x58)]);
}

#[test]
fn pci_forms_in_ring3_are_rejected_at_entry() {
    let cli = CliProfile::NonPviLong64V1;
    let cell = Cell::new(0x55);
    let mut state = board_state();
    state.gpr[2] = 0xcf8;
    let before = state.clone();
    let mut memory = Memory::code(&[0xef]);
    let mut pio = TestPio::default();
    let error = run_with_cpl0_profile_set_and_pio(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 8),
        od_isa::INTEGER_FLAGS,
        &pci_set(&cli, &cell),
        None,
        &mut pio,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
    assert_eq!(cell.get(), 0x55);
    assert_eq!(memory.fetches, 0);
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
        &[0x64, 0x64, 0x0f, 0x1f, 0x00], // duplicate FS (S4-38L7v: repeated CS is accepted)
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
        &[0x64, 0x64, 0x0f, 0x18, 0x0a], // duplicate FS (S4-38L7v: repeated CS is accepted)
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

// ---- S4-38L7e: RDMSR IA32_EFER in the set under the set CR4 policy (PSE/PGE) ----

/// Like `run_msr_base`, but with an explicit CR4 and CR0 in the state.
fn run_efer_with_cr(
    set: &Cpl0ProfileSet<'_>,
    code: &[u8],
    rcx: u64,
    cr4: u64,
) -> (LongModeState, LongModeState, Cpl0ProfileSetResult) {
    let mut state = board_state();
    state.cr4 = Some(cr4);
    state.gpr[1] = rcx;
    state.gpr[0] = 0x1111_2222_3333_4444;
    state.gpr[2] = 0x5555_6666_7777_8888;
    let before = state.clone();
    let mut memory = Memory::code(code);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap();
    (state, before, result)
}

#[test]
fn rdmsr_efer_works_with_pse_pge_cr4_and_leaves_other_state_alone() {
    let efer = MsrProfile::EferReadOnlyV1(0xd00);
    let set = base().with_msr(&efer).with_msr_fs_gs_base();
    // CR4 of the Linux board after the kernel's PSE and PGE writes: PAE | PSE | PGE.
    for cr4 in [
        CR4_BASE,
        CR4_BASE | (1 << 4),
        CR4_BASE | (1 << 4) | (1 << 7),
    ] {
        let (state, before, result) = run_efer_with_cr(&set, &[0x0f, 0x32], 0xc000_0080, cr4);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[0], 0xd00);
        assert_eq!(state.gpr[2], 0);
        assert_eq!(state.rip, before.rip + 2);
        let mut expected = before.clone();
        expected.gpr[0] = 0xd00;
        expected.gpr[2] = 0;
        expected.rip += 2;
        assert_eq!(state, expected, "only RAX, RDX and RIP change");
    }
    // WRMSR of EFER and other MSRs stay Unsupported without mutation.
    for (code, rcx) in [
        ([0x0f, 0x30], 0xc000_0080u64),
        ([0x0f, 0x32], 0xc000_0081),
        ([0x0f, 0x32], 0x1b),
        ([0x0f, 0x32], 0x174),
        ([0x0f, 0x30], 0xc000_0081),
    ] {
        let (state, before, result) =
            run_efer_with_cr(&set, &code, rcx, CR4_BASE | (1 << 4) | (1 << 7));
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: code[1] },
            "rcx={rcx:#x}"
        );
        assert_eq!(state, before);
    }
}

// ---- S4-38L7f: WRMSR IA32_EFER (SCE) and CR0.AM (EFER bits per SDM, from memory) ----

const EFER_MSR: u64 = 0xc000_0080;
const CR0_AM: u64 = 1 << 18;
const WRMSR: [u8; 2] = [0x0f, 0x30];
const RDMSR: [u8; 2] = [0x0f, 0x32];

/// Runs `code` in a set with the read-only EFER profile seeded with `initial` and its write
/// member on `cell`; ECX, EAX, EDX are given.
fn run_efer_write(
    cell: &Cell<u64>,
    initial: u64,
    code: &[u8],
    (rcx, rax, rdx): (u64, u64, u64),
    budget: u64,
) -> (LongModeState, LongModeState, Cpl0ProfileSetResult) {
    let profile = MsrProfile::EferReadOnlyV1(initial);
    let set = base().with_msr(&profile).with_msr_efer_write(cell);
    let mut state = board_state();
    state.cr4 = Some(CR4_BASE | (1 << 4) | (1 << 7));
    state.gpr[1] = rcx;
    state.gpr[0] = rax;
    state.gpr[2] = rdx;
    let before = state.clone();
    let mut memory = Memory::code(code);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    (state, before, result)
}

#[test]
fn wrmsr_efer_sce_is_stored_and_rdmsr_returns_it() {
    let cell = Cell::new(0xd00);
    let (state, before, result) = run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, 0xd01, 0), 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0xd01);
    let mut expected = before.clone();
    expected.rip += 2;
    assert_eq!(state, expected, "only RIP changes");
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);

    // WRMSR then RDMSR in one run: the read returns the written value (EAX=0xd01, EDX=0).
    let mut code = WRMSR.to_vec();
    code.extend_from_slice(&RDMSR);
    let cell = Cell::new(0xd00);
    let (state, _, result) = run_efer_write(&cell, 0xd00, &code, (EFER_MSR, 0xd01, 0), 2);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[0], 0xd01);
    assert_eq!(state.gpr[2], 0);
    assert_eq!(state.rip, RIP + 4);

    // SCE can be cleared again; 0xd00 is accepted from 0xd01.
    let cell = Cell::new(0xd01);
    let (_, _, result) = run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, 0xd00, 0), 1);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0xd00);
    // The same value is a no-op write.
    let cell = Cell::new(0xd00);
    let (_, _, result) = run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, 0xd00, 0), 1);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0xd00);
    // A long-mode-disabled model value keeps working too: 0x500 -> 0x501.
    let cell = Cell::new(0x500);
    let (_, _, result) = run_efer_write(&cell, 0x500, &WRMSR, (EFER_MSR, 0x501, 0), 1);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0x501);
    // RDMSR reads the cell, not the profile seed.
    let cell = Cell::new(0xd01);
    let (state, _, _) = run_efer_write(&cell, 0xd00, &RDMSR, (EFER_MSR, 0, 0), 1);
    assert_eq!(state.gpr[0], 0xd01);
}

#[test]
fn wrmsr_efer_ignores_the_upper_halves_of_rax_rdx_like_edx_eax() {
    // EDX:EAX is the value: RAX[63:32] and RDX[63:32] are not part of it.
    let cell = Cell::new(0xd00);
    let (_, _, result) = run_efer_write(
        &cell,
        0xd00,
        &WRMSR,
        (
            EFER_MSR | (1 << 32),
            0xffff_ffff_0000_0d01,
            0xffff_ffff_0000_0000,
        ),
        1,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cell.get(), 0xd01);
}

#[test]
fn wrmsr_efer_changing_lme_lma_or_nxe_is_unsupported_without_mutation() {
    for value in [
        0x001u64, 0x000, 0x500, 0x501, 0x900, 0x901, 0xc00, 0xc01, 0x101,
    ] {
        let cell = Cell::new(0xd00);
        let (state, before, result) = run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, value, 0), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{value:#x}"
        );
        assert_eq!(result.instructions_executed, 0, "{value:#x}");
        assert_eq!(state, before, "{value:#x}");
        assert_eq!(cell.get(), 0xd00, "{value:#x}");
    }
}

#[test]
fn wrmsr_efer_reserved_bits_are_gp0_and_cpuid_dependent_bits_unsupported() {
    // Reserved: bits 1..=7, 9, 16..=63 (including EDX bits).
    for (eax, edx) in [
        (0xd02u64, 0u64),
        (0xd04, 0),
        (0xd80, 0),
        (0xf00, 0),
        (0x1_0d00, 0),
        (0x8000_0d00, 0),
        (0xd00, 1),
        (0xd01, 0x8000_0000),
    ] {
        let cell = Cell::new(0xd00);
        let (state, before, result) = run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, eax, edx), 1);
        assert_eq!(exit(&result), &gp0(), "{eax:#x}:{edx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0xd00);
    }
    // Bits 12..=15 (SVME, LMSLE, FFXSR, TCE) depend on CPUID features the model lacks.
    for bit in 12..=15 {
        let cell = Cell::new(0xd00);
        let (state, before, result) =
            run_efer_write(&cell, 0xd00, &WRMSR, (EFER_MSR, 0xd00 | (1 << bit), 0), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{bit}"
        );
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0xd00);
    }
}

#[test]
fn wrmsr_efer_without_the_write_member_or_for_other_msrs_is_unsupported() {
    // Read-only profile without the cell: WRMSR EFER stays Unsupported (S4-38L7e).
    let profile = MsrProfile::EferReadOnlyV1(0xd00);
    let set = base().with_msr(&profile);
    let mut state = board_state();
    state.gpr[1] = EFER_MSR;
    state.gpr[0] = 0xd01;
    state.gpr[2] = 0;
    let before = state.clone();
    let mut memory = Memory::code(&WRMSR);
    let result = run_set(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
    assert_eq!(state, before);

    // With the member, other MSRs (STAR, APIC_BASE) stay Unsupported and the cell is untouched.
    for msr in [0xc000_0081u64, 0x1b, 0xc000_0082] {
        let cell = Cell::new(0xd00);
        let (state, before, result) = run_efer_write(&cell, 0xd00, &WRMSR, (msr, 0xd01, 0), 1);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{msr:#x}"
        );
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0xd00);
    }

    // A set with the member but no MSR profile has no MSR to write.
    let cell = Cell::new(0xd00);
    let set = base().with_msr_efer_write(&cell);
    let mut state = board_state();
    state.gpr[1] = EFER_MSR;
    state.gpr[0] = 0xd01;
    let before = state.clone();
    let mut memory = Memory::code(&WRMSR);
    let result = run_set(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
    assert_eq!(state, before);
    assert_eq!(cell.get(), 0xd00);
}

#[test]
fn wrmsr_efer_with_prefixes_or_a_cell_outside_the_domain_is_unsupported() {
    // 66 0F 30 / F3 0F 30 / 48 0F 30: prefixed forms stay Unsupported without mutation.
    for code in [
        &[0x66u8, 0x0f, 0x30][..],
        &[0xf3, 0x0f, 0x30],
        &[0x48, 0x0f, 0x30],
    ] {
        let cell = Cell::new(0xd00);
        let (state, before, result) = run_efer_write(&cell, 0xd00, code, (EFER_MSR, 0xd01, 0), 1);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0xd00);
    }
    // A cell value outside the S2-25e domain (no LME) closes both RDMSR and WRMSR.
    for code in [WRMSR, RDMSR] {
        let cell = Cell::new(0x000);
        let (state, before, result) = run_efer_write(&cell, 0xd00, &code, (EFER_MSR, 0xd01, 0), 1);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: code[1] });
        assert_eq!(state, before);
        assert_eq!(cell.get(), 0x000);
    }
}

#[test]
fn wrmsr_efer_at_cpl3_never_reaches_the_member() {
    // The set is CPL0 only: a CPL3 run is rejected at entry, before any fetch or write.
    let cell = Cell::new(0xd00);
    let profile = MsrProfile::EferReadOnlyV1(0xd00);
    let set = base().with_msr(&profile).with_msr_efer_write(&cell);
    let mut state = board_state();
    state.gpr[1] = EFER_MSR;
    state.gpr[0] = 0xd01;
    let before = state.clone();
    let mut memory = Memory::code(&WRMSR);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    );
    assert_eq!(
        result.unwrap_err(),
        Cpl0ProfileSetInputError::WrongPrivilege
    );
    assert_eq!(state, before);
    assert_eq!(memory.fetches, 0);
    assert_eq!(cell.get(), 0xd00);
}

#[test]
fn cr0_am_is_accepted_by_the_set_policy() {
    let set = cr_set();
    // The kernel's CR0 write (PG|WP|AM|NE|ET|MP|PE): MOV RAX, imm; MOV CR0, RAX; MOV RAX, CR0.
    let value = 0x8005_0033u64;
    assert_eq!(value, CR0_BASE | CR0_AM);
    let mut code = movabs_rax(value);
    code.extend_from_slice(&MOV_CR0_RAX);
    code.extend_from_slice(&[0x48, 0x31, 0xc0]); // xor rax, rax
    code.extend_from_slice(&MOV_RAX_CR0);
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &set, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state.cr0, Some(value));
    assert_eq!(state.gpr[0], value);
    assert_eq!(state.cr4, Some(CR4_BASE));
    // AM together with EM/TS, and a state that already carries AM, are admitted too.
    for cr0 in [value | 0xc, value | 4, value | 8] {
        let mut code = movabs_rax(cr0);
        code.extend_from_slice(&MOV_CR0_RAX);
        let mut state = board_state();
        let mut memory = Memory::code(&code);
        let result = run_set(&mut state, &mut memory, &set, 2).unwrap();
        assert_eq!(result.instructions_executed, 2, "{cr0:#x}");
        assert_eq!(state.cr0, Some(cr0));
    }
    let mut state = board_state();
    state.cr0 = Some(value);
    let mut memory = Memory::code(&MOV_RAX_CR0);
    let result = run_set(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], value);
    // AM is not admitted by the per-register profile (only the set policy gained it).
    assert!(!od_isa::execute::Cr0Profile::FpInitLong64V1.accepts(value));
}

#[test]
fn cr0_am_does_not_widen_the_policy_to_other_bits() {
    let set = cr_set();
    // Bit 17 (reserved), bit 19, bit 29 (NW), bit 30 (CD), bit 32; PG/PE/WP cleared with AM set.
    for value in [
        CR0_BASE | CR0_AM | (1 << 17),
        CR0_BASE | CR0_AM | (1 << 19),
        CR0_BASE | CR0_AM | (1 << 29),
        CR0_BASE | CR0_AM | (1 << 30),
        CR0_BASE | CR0_AM | (1 << 32),
        (CR0_BASE | CR0_AM) & !1,
        (CR0_BASE | CR0_AM) & !(1 << 16),
        CR0_AM,
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
        assert_eq!(state.cr0, Some(CR0_BASE), "{label}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{label}");
    }
}

#[test]
fn cr0_am_state_is_admitted_at_entry_and_a_stray_bit_is_not() {
    let set = cr_set();
    let mut state = board_state();
    state.cr0 = Some(CR0_BASE | CR0_AM);
    let mut memory = Memory::code(&[0x90]);
    assert!(run_set(&mut state, &mut memory, &set, 1).is_ok());
    let mut state = board_state();
    state.cr0 = Some(CR0_BASE | CR0_AM | (1 << 17));
    let mut memory = Memory::code(&[0x90]);
    assert_eq!(
        run_set(&mut state, &mut memory, &set, 1).unwrap_err(),
        Cpl0ProfileSetInputError::Cr0OutsidePolicy {
            value: CR0_BASE | CR0_AM | (1 << 17)
        }
    );
}

// ---- S4-38L6c: MOV Sreg, AX with a nonzero GDT selector (checks per SDM, from memory) ----

/// Data, writable, DPL0, present, A = 0, base 0x1234_5678, 4 KiB granularity, limit 0xfffff.
const SREG_DATA: u64 = 0x12cf_9234_5678_ffff;
const SREG_DATA_ACCESSED: u64 = 0x12cf_9334_5678_ffff;
const SREG_INDEX3: u64 = LRETQ_GDT + 0x18;

fn sreg_state(selector: u16) -> (LongModeState, LretqMemory) {
    let mut state = LongModeState {
        rip: RIP,
        rflags: 2,
        gdtr: Some(od_isa::state::Gdtr {
            base: LRETQ_GDT,
            limit: 0x1f,
        }),
        ..loaded_state()
    };
    state.gpr[0] = u64::from(selector);
    let mut memory = LretqMemory::new(&[0x8e, 0xd0]);
    memory.put(SREG_INDEX3, SREG_DATA);
    (state, memory)
}

fn run_sreg(
    state: &mut LongModeState,
    memory: &mut LretqMemory,
    modrm: u8,
) -> Cpl0ProfileSetResult {
    memory.code = vec![0x8e, modrm];
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

const SS_MODRM: u8 = 0xd0;
const DS_MODRM: u8 = 0xd8;
const ES_MODRM: u8 = 0xc0;
const FS_MODRM: u8 = 0xe0;
const GS_MODRM: u8 = 0xe8;
const ALL_SREG_MODRM: [u8; 5] = [SS_MODRM, DS_MODRM, ES_MODRM, FS_MODRM, GS_MODRM];
const NON_SS_SREG_MODRM: [u8; 4] = [DS_MODRM, ES_MODRM, FS_MODRM, GS_MODRM];

fn sreg_loaded_cache() -> DataSegmentDescriptorCache {
    DataSegmentDescriptorCache {
        base: 0x1234_5678,
        limit: 0xffff_ffff,
        access: 0x93,
        flags: 0xc,
    }
}

#[test]
fn mov_ss_nonnull_loads_the_cache_sets_accessed_and_opens_the_shadow() {
    let (mut state, mut memory) = sreg_state(0x18);
    let mut expected = state.clone();
    let result = run_sreg(&mut state, &mut memory, SS_MODRM);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    expected.rip = RIP + 2;
    expected.ss_selector = Some(0x18);
    expected.ss_cache = Some(sreg_loaded_cache());
    expected.ss_unusable = false;
    expected.sti_shadow = Some(StiShadow::Blocking);
    assert_eq!(state, expected);
    assert_eq!(
        memory.write_log,
        vec![(
            SREG_INDEX3,
            SREG_DATA_ACCESSED.to_le_bytes().to_vec(),
            AccessOrigin::DescriptorTable
        )]
    );
    assert_eq!(
        memory.reads,
        vec![(SREG_INDEX3, 8, AccessOrigin::DescriptorTable)]
    );
}

#[test]
fn mov_ss_nonnull_with_the_accessed_bit_set_writes_nothing_and_clears_unusable() {
    let (mut state, mut memory) = sreg_state(0x18);
    memory.put(SREG_INDEX3, SREG_DATA_ACCESSED);
    state.ss_unusable = true;
    let result = run_sreg(&mut state, &mut memory, SS_MODRM);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.ss_selector, Some(0x18));
    assert_eq!(state.ss_cache, Some(sreg_loaded_cache()));
    assert!(!state.ss_unusable);
    assert_eq!(memory.writes, 0);
}

#[test]
fn mov_ds_es_fs_gs_nonnull_load_data_segments() {
    for modrm in NON_SS_SREG_MODRM {
        let (mut state, mut memory) = sreg_state(0x18);
        let mut expected = state.clone();
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(result.instructions_executed, 1, "{modrm:#x}");
        expected.rip = RIP + 2;
        let fs_gs = FsDescriptorCache {
            limit: 0xffff_ffff,
            access: 0x93,
            flags: 0xc,
        };
        match modrm {
            DS_MODRM => {
                expected.ds_selector = 0x18;
                expected.ds_cache = Some(sreg_loaded_cache());
            }
            ES_MODRM => {
                expected.es_selector = 0x18;
                expected.es_cache = Some(sreg_loaded_cache());
            }
            FS_MODRM => {
                expected.fs_selector = 0x18;
                expected.fs_cache = Some(fs_gs);
                expected.fs_base = 0x1234_5678;
            }
            _ => {
                expected.gs_selector = 0x18;
                expected.gs_cache = Some(fs_gs);
                expected.gs_base = 0x1234_5678;
            }
        }
        // MOV DS/ES/FS/GS open no interrupt shadow; RFLAGS and the registers are untouched.
        assert_eq!(state, expected, "{modrm:#x}");
        assert_eq!(memory.writes, 1, "{modrm:#x}");
    }
}

#[test]
fn mov_data_sreg_nonnull_clears_only_its_own_null_mark() {
    for (loaded, modrm) in NON_SS_SREG_MODRM.into_iter().enumerate() {
        let (mut state, mut memory) = sreg_state(0x18);
        state.ds_unusable = true;
        state.es_unusable = true;
        state.fs_unusable = true;
        state.gs_unusable = true;
        run_sreg(&mut state, &mut memory, modrm);
        let unusable = [
            state.ds_unusable,
            state.es_unusable,
            state.fs_unusable,
            state.gs_unusable,
        ];
        for (index, flag) in unusable.into_iter().enumerate() {
            assert_eq!(flag, index != loaded, "{modrm:#x} {index}");
        }
    }
}

#[test]
fn mov_ds_es_fs_gs_accept_a_readable_code_segment_but_ss_refuses_it() {
    // Code, readable, non-conforming, DPL0, present, A = 0.
    let code_readable = 0x00cf_9a00_0000_ffffu64;
    for modrm in NON_SS_SREG_MODRM {
        let (mut state, mut memory) = sreg_state(0x18);
        memory.put(SREG_INDEX3, code_readable);
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(result.instructions_executed, 1, "{modrm:#x}");
        assert_eq!(state.rip, RIP + 2);
        assert_eq!(
            memory.write_log[0].1,
            0x00cf_9b00_0000_ffffu64.to_le_bytes().to_vec()
        );
    }
    assert_sreg_refused(SS_MODRM, 0x18, code_readable, selector_fault_exit(13, 0x18));
}

/// Run a load that must fail with `expected`, `state == before`, no memory write.
fn assert_sreg_refused(modrm: u8, selector: u16, descriptor: u64, expected: RunExit) {
    let (mut state, mut memory) = sreg_state(selector);
    memory.put(SREG_INDEX3, descriptor);
    let before = state.clone();
    let result = run_sreg(&mut state, &mut memory, modrm);
    assert_eq!(
        exit(&result),
        &expected,
        "{modrm:#x} {selector:#x} {descriptor:#x}"
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.writes, 0);
}

#[test]
fn mov_ds_es_fs_gs_refuse_bad_descriptors_with_exact_faults() {
    let gp = selector_fault_exit(13, 0x18);
    for modrm in NON_SS_SREG_MODRM {
        // Execute-only code: #GP(selector).
        assert_sreg_refused(modrm, 0x18, 0x00cf_9800_0000_ffff, gp.clone());
        // System descriptor (S = 0): #GP(selector).
        assert_sreg_refused(modrm, 0x18, 0x00cf_8900_0000_ffff, gp.clone());
        // Not present data: #NP(selector).
        assert_sreg_refused(
            modrm,
            0x18,
            0x00cf_1200_0000_ffff,
            selector_fault_exit(11, 0x18),
        );
        // Not present, execute-only: the type check wins, #GP.
        assert_sreg_refused(modrm, 0x18, 0x00cf_1800_0000_ffff, gp.clone());
        // DPL 0 data with RPL 3: DPL < RPL, #GP(selector) (error code without RPL bits).
        assert_sreg_refused(modrm, 0x1b, SREG_DATA, gp.clone());
        // Index beyond the GDT limit (0x1f): #GP(selector).
        assert_sreg_refused(modrm, 0x20, SREG_DATA, selector_fault_exit(13, 0x20));
    }
    // DPL 3 data with RPL 3 is legal.
    let (mut state, mut memory) = sreg_state(0x1b);
    memory.put(SREG_INDEX3, 0x12cf_f234_5678_ffff);
    let result = run_sreg(&mut state, &mut memory, DS_MODRM);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.ds_selector, 0x1b);
    // Conforming readable code with DPL 0 and RPL 3 is legal (the DPL check is skipped).
    let (mut state, mut memory) = sreg_state(0x1b);
    memory.put(SREG_INDEX3, 0x00cf_9e00_0000_ffff);
    let result = run_sreg(&mut state, &mut memory, ES_MODRM);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.es_selector, 0x1b);
}

#[test]
fn mov_ss_refuses_bad_selectors_and_descriptors_with_exact_faults() {
    let gp = selector_fault_exit(13, 0x18);
    // RPL != CPL: #GP(selector), even with a valid descriptor.
    assert_sreg_refused(SS_MODRM, 0x19, SREG_DATA, gp.clone());
    assert_sreg_refused(SS_MODRM, 0x1b, SREG_DATA, gp.clone());
    // DPL != CPL (DPL 3 data, RPL 0): #GP(selector).
    assert_sreg_refused(SS_MODRM, 0x18, 0x12cf_f234_5678_ffff, gp.clone());
    // Read-only data: #GP(selector).
    assert_sreg_refused(SS_MODRM, 0x18, 0x12cf_9034_5678_ffff, gp.clone());
    // Code: #GP(selector).
    assert_sreg_refused(SS_MODRM, 0x18, 0x00cf_9a00_0000_ffff, gp.clone());
    // System descriptor: #GP(selector).
    assert_sreg_refused(SS_MODRM, 0x18, 0x00cf_8900_0000_ffff, gp);
    // Not present writable data: #SS(selector).
    assert_sreg_refused(
        SS_MODRM,
        0x18,
        0x00cf_1200_0000_ffff,
        selector_fault_exit(12, 0x18),
    );
    // Index beyond the GDT limit: #GP(selector).
    assert_sreg_refused(SS_MODRM, 0x20, SREG_DATA, selector_fault_exit(13, 0x20));
    assert_sreg_refused(SS_MODRM, 0x100, SREG_DATA, selector_fault_exit(13, 0x100));
}

#[test]
fn mov_sreg_nonnull_ti_one_and_null_with_rpl_are_unsupported_without_mutation() {
    for modrm in ALL_SREG_MODRM {
        for selector in [0x1c_u16, 0x04, 0x01, 0x03] {
            assert_sreg_refused(
                modrm,
                selector,
                SREG_DATA,
                RunExit::Unsupported { opcode: 0x8e },
            );
        }
    }
}

#[test]
fn mov_sreg_nonnull_without_a_gdtr_or_cache_is_unsupported_without_mutation() {
    for modrm in ALL_SREG_MODRM {
        let (mut state, mut memory) = sreg_state(0x18);
        state.gdtr = None;
        let before = state.clone();
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes == 0);

        let (mut state, mut memory) = sreg_state(0x18);
        match modrm {
            SS_MODRM => state.ss_cache = None,
            DS_MODRM => state.ds_cache = None,
            ES_MODRM => state.es_cache = None,
            FS_MODRM => state.fs_cache = None,
            _ => state.gs_cache = None,
        }
        let before = state.clone();
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes == 0);
    }
}

#[test]
fn mov_ss_nonnull_keeps_the_if_gate() {
    // IF = 1: refused without mutation, descriptor not read.
    let (mut state, mut memory) = sreg_state(0x18);
    state.rflags |= IF;
    let before = state.clone();
    let result = run_sreg(&mut state, &mut memory, SS_MODRM);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8e });
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes == 0);
}

#[test]
fn mov_sreg_nonnull_outside_the_set_stays_unsupported_for_ss_fs_gs() {
    // The legacy `run` entry has no CPL0 set: SS, FS and GS nonnull loads at CPL0 are refused.
    for modrm in [SS_MODRM, FS_MODRM, GS_MODRM] {
        let (mut state, mut memory) = sreg_state(0x18);
        state.cr0 = None;
        state.cr4 = None;
        state.activity = None;
        state.sti_shadow = None;
        memory.code = vec![0x8e, modrm];
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x8e });
        assert_eq!(state, before, "{modrm:#x}");
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn mov_sreg_nonnull_descriptor_read_and_accessed_write_errors_leave_the_cpu_untouched() {
    for modrm in ALL_SREG_MODRM {
        // Faulting GDT page on read.
        let (mut state, mut memory) = sreg_state(0x18);
        memory.fault_pages.push(LRETQ_GDT);
        let before = state.clone();
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(SREG_INDEX3),
            }),
            "{modrm:#x}"
        );
        assert_eq!(state, before);
        assert_eq!(memory.writes, 0);
        // Accessed-bit write fault.
        let (mut state, mut memory) = sreg_state(0x18);
        memory.write_fault_pages.push(LRETQ_GDT);
        let before = state.clone();
        let ram = memory.ram.clone();
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(SREG_INDEX3),
            }),
            "{modrm:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.ram, ram);
    }
}

#[test]
fn mov_sreg_null_loads_still_behave_as_s2_46_and_read_no_descriptor() {
    for modrm in ALL_SREG_MODRM {
        let (mut state, mut memory) = sreg_state(0);
        let result = run_sreg(&mut state, &mut memory, modrm);
        assert_eq!(result.instructions_executed, 1, "{modrm:#x}");
        assert_eq!(state.rip, RIP + 2);
        assert!(memory.reads.is_empty() && memory.writes == 0);
        let unusable = match modrm {
            SS_MODRM => state.ss_unusable,
            DS_MODRM => state.ds_unusable,
            ES_MODRM => state.es_unusable,
            FS_MODRM => state.fs_unusable,
            _ => state.gs_unusable,
        };
        assert!(unusable, "{modrm:#x}");
        let shadow = if modrm == SS_MODRM {
            StiShadow::Blocking
        } else {
            StiShadow::Clear
        };
        assert_eq!(state.sti_shadow, Some(shadow));
    }
}

#[test]
fn mov_fs_gs_at_cpl3_never_reaches_the_cpl0_loader() {
    // The set is rejected at CPL3 on entry, before any fetch; the loader additionally
    // dispatches only at Ring0, so a CPL3 load can never take the CPL0 checks.
    for modrm in [FS_MODRM, GS_MODRM] {
        let (mut state, mut memory) = sreg_state(0x18);
        memory.put(SREG_INDEX3, SREG_DATA_ACCESSED);
        memory.code = vec![0x8e, modrm];
        let before = state.clone();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 1),
            od_isa::INTEGER_FLAGS,
            &base(),
            None,
            || false,
        );
        assert_eq!(result, Err(Cpl0ProfileSetInputError::WrongPrivilege));
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes == 0);
    }
}

// ---- S4-38L6d: LEAVE (`C9`) with a 64-bit stack in the CPL0 set ----

const LEAVE_FRAME: u64 = RAM_BASE + 0x1800;
const LEAVE_SAVED_RBP: u64 = 0x0000_7fff_1234_5670;

/// `RBP` points at a frame whose saved value is `LEAVE_SAVED_RBP`; `RSP` is elsewhere.
fn leave_state(code: &[u8]) -> (LongModeState, LretqMemory) {
    let mut state = board_state();
    state.gpr[4] = STACK - 0x100; // RSP, distinct from RBP
    state.gpr[5] = LEAVE_FRAME; // RBP
    state.gpr[1] = 0xaaaa_bbbb_cccc_dddd;
    state.gpr[3] = 0x0123_4567_89ab_cdef;
    state.rflags = 2 | IF | 0x1 | 0x40 | 0x800;
    let mut memory = LretqMemory::new(code);
    memory.put(LEAVE_FRAME, LEAVE_SAVED_RBP);
    (state, memory)
}

fn run_leave(
    state: &mut LongModeState,
    memory: &mut LretqMemory,
    budget: u64,
) -> Cpl0ProfileSetResult {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
    .unwrap()
}

#[test]
fn leave_moves_rbp_to_rsp_pops_rbp_and_retires() {
    let (mut state, mut memory) = leave_state(&[0xc9, 0x90]);
    let before = state.clone();
    let result = run_leave(&mut state, &mut memory, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[5], LEAVE_SAVED_RBP);
    assert_eq!(state.gpr[4], LEAVE_FRAME + 8);
    assert_ne!(
        before.gpr[4], before.gpr[5],
        "RSP and RBP are distinguishable"
    );
    assert_eq!(state.rip, RIP + 1);
    // Flags, their definedness and every other register are untouched.
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    let mut expected = before;
    expected.gpr[4] = LEAVE_FRAME + 8;
    expected.gpr[5] = LEAVE_SAVED_RBP;
    expected.rip = RIP + 1;
    assert_eq!(state, expected);
    // One qword read at [RBP] (same origin as the set's POP), no write.
    assert_eq!(memory.reads, vec![(LEAVE_FRAME, 8, AccessOrigin::Explicit)]);
    assert_eq!(memory.writes, 0);
}

#[test]
fn leave_with_rsp_equal_to_rbp_still_pops_the_frame() {
    let (mut state, mut memory) = leave_state(&[0xc9]);
    state.gpr[4] = LEAVE_FRAME;
    let result = run_leave(&mut state, &mut memory, 1);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[4], LEAVE_FRAME + 8);
    assert_eq!(state.gpr[5], LEAVE_SAVED_RBP);
}

#[test]
fn leave_with_a_non_canonical_rbp_is_ss_zero_without_mutation() {
    for rbp in [0x0000_8000_0000_0000u64, 0xffff_7fff_ffff_ffff] {
        let (mut state, mut memory) = leave_state(&[0xc9]);
        state.gpr[5] = rbp;
        let before = state.clone();
        let result = run_leave(&mut state, &mut memory, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            }),
            "{rbp:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn leave_with_rbp_on_an_unmapped_page_is_a_page_fault_without_mutation() {
    let (mut state, mut memory) = leave_state(&[0xc9]);
    memory.fault_pages.push(LEAVE_FRAME & !0xfff);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run_leave(&mut state, &mut memory, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(LEAVE_FRAME),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 0);

    // The second page of a straddling qword.
    let (mut state, mut memory) = leave_state(&[0xc9]);
    state.gpr[5] = RAM_BASE + 0x1ffc;
    memory.fault_pages.push(RAM_BASE + 0x2000);
    let before = state.clone();
    let result = run_leave(&mut state, &mut memory, 1);
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault { vector: 14, .. })
    ));
    assert_eq!(state, before);
}

#[test]
fn leave_with_prefixes_stays_unsupported_without_mutation() {
    for code in [
        &[0x66u8, 0xc9][..],
        &[0x48, 0xc9],
        &[0x40, 0xc9],
        &[0xf3, 0xc9],
        &[0xf2, 0xc9],
        &[0xf0, 0xc9],
        &[0x2e, 0xc9],
        &[0x67, 0xc9],
    ] {
        let (mut state, mut memory) = leave_state(code);
        let before = state.clone();
        let result = run_leave(&mut state, &mut memory, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xc9 },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty());
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn leave_outside_the_set_stays_unsupported_without_mutation() {
    let (mut state, mut memory) = leave_state(&[0xc9]);
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
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xc9 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
    assert_eq!(memory.writes, 0);
}

// ---- S4-38L7d: CR4.PGE joins the CPL0 set CR4 policy -------------------------

/// The second CR4 value the real kernel writes (PSE|PAE|PGE).
const CR4_PGE_PSE_PAE: u64 = 0xb0;

#[test]
fn mov_cr4_rcx_with_pge_is_the_second_real_kernel_write() {
    // `0F 22 E1` at 0xffffffff8139c0c7 in the real kernel, RCX = 0xb0.
    let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PGE_PSE_PAE);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !ARITHMETIC);
    let mut expected = before.clone();
    expected.cr4 = Some(CR4_PGE_PSE_PAE);
    expected.rip = RIP + 3;
    assert_eq!(state, expected);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.cr0, before.cr0);
}

#[test]
fn mov_cr4_pge_reads_every_register_and_rex_b() {
    for rm in 0u8..8 {
        for rex in [None, Some(0x40u8), Some(0x48)] {
            let mut bytes = Vec::new();
            bytes.extend(rex);
            bytes.extend_from_slice(&[0x0f, 0x22, 0xe0 | rm]);
            let mut state = mov_cr4_state(rm as usize, CR4_PGE_PSE_PAE | 0x600);
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            let mut expected = before.clone();
            expected.cr4 = Some(CR4_PGE_PSE_PAE | 0x600);
            expected.rip = RIP + bytes.len() as u64;
            assert_eq!(state, expected, "{bytes:02x?}");
        }
        for rex in [0x41u8, 0x49] {
            let bytes = [rex, 0x0f, 0x22, 0xe0 | rm];
            let mut state = mov_cr4_state(rm as usize, 0x1000);
            state.gpr[8 + rm as usize] = CR4_PGE_PSE_PAE;
            let before = state.clone();
            let mut memory = Memory::code(&bytes);
            let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            let mut expected = before.clone();
            expected.cr4 = Some(CR4_PGE_PSE_PAE);
            expected.rip = RIP + 4;
            assert_eq!(state, expected, "{bytes:02x?}");
        }
    }
}

#[test]
fn mov_cr4_pge_without_pae_is_gp0_without_mutation() {
    for value in [0x90u64, 0x80, 0x680, 0x690] {
        let mut state = mov_cr4_state(Register::Rcx as usize, value);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(exit(&result), &gp0(), "{value:#x}");
        assert_eq!(result.instructions_executed, 0, "{value:#x}");
        assert_eq!(state, before, "{value:#x}");
    }
}

#[test]
fn mov_cr4_pge_with_other_bits_outside_the_policy_is_unsupported_without_mutation() {
    // LA57 (bit 12), PCE (bit 8), PCIDE (bit 17), SMEP (bit 20): still outside the policy.
    for extra in [0x1000u64, 0x100, 0x2_0000, 0x10_0000] {
        let value = CR4_PGE_PSE_PAE | extra;
        let mut state = mov_cr4_state(Register::Rcx as usize, value);
        let before = state.clone();
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 64).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x22 },
            "{value:#x}"
        );
        assert_eq!(result.instructions_executed, 0, "{value:#x}");
        assert_eq!(state, before, "{value:#x}");
    }
}

#[test]
fn cr4_with_pge_is_admitted_on_reentry_and_by_the_rax_write() {
    let mut state = cr4_any_reg_state(CR4_PGE_PSE_PAE);
    let mut memory = Memory::code(&[0x0f, 0x20, 0xe2]);
    run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
    assert_eq!(state.gpr[Register::Rdx as usize], CR4_PGE_PSE_PAE);
    let mut code = movabs_rax(CR4_PGE_PSE_PAE);
    code.extend_from_slice(&MOV_CR4_RAX);
    let mut state = board_state();
    let mut memory = Memory::code(&code);
    let result = run_set(&mut state, &mut memory, &cr_set(), 2).unwrap();
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.cr4, Some(CR4_PGE_PSE_PAE));
}

#[test]
fn mov_cr4_pge_regressions_keep_pse_only_and_pae_only() {
    for value in [0x30u64, 0x20] {
        let mut state = mov_cr4_state(Register::Rcx as usize, value);
        let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
        let result = run_set(&mut state, &mut memory, &cr_set(), 1).unwrap();
        assert_eq!(result.instructions_executed, 1, "{value:#x}");
        assert_eq!(state.cr4, Some(value));
    }
}

#[test]
fn mov_cr4_pge_stays_unsupported_outside_the_set_and_gp_at_cpl3() {
    let mut state = LongModeState {
        rip: RIP,
        rflags: 2 | IF,
        cr4: None,
        ..LongModeState::default()
    };
    state.gpr[Register::Rcx as usize] = CR4_PGE_PSE_PAE;
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 4),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x22 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    let mut state = mov_cr4_state(Register::Rcx as usize, CR4_PGE_PSE_PAE);
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f, 0x22, 0xe1]);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &cr_set(),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!((memory.fetches, &state), (0, &before));
}

// ---- S4-38L7g: FS/GS-based qword store `MOV m64, r64` (SDM Vol.2 segment override prefixes,
// Vol.3 3.4.4: in 64-bit mode only the FS/GS base is added; from memory, not re-checked) ----

/// Code at `RIP` plus a log of every write; reads are not expected.
struct StoreMemory {
    code: Vec<u8>,
    writes: Vec<(u64, Vec<u8>)>,
}

impl StoreMemory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            code: bytes.to_vec(),
            writes: Vec::new(),
        }
    }
}

impl GuestMemory for StoreMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!(
            "unexpected read {address:#x}"
        )))
    }

    fn write(&mut self, address: u64, _: AccessContext, bytes: &[u8]) -> Result<(), MemoryError> {
        self.writes.push((address, bytes.to_vec()));
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

/// `mov %rax, %gs:0x1234(%rip)`.
const GS_STORE_RIP: [u8; 8] = [0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00];
/// `mov %rax, %fs:0x1234(%rip)`.
const FS_STORE_RIP: [u8; 8] = [0x64, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00];
const STORE_RAX: u64 = 0x1111_2222_3333_4444;
const GP0: ArchitecturalFault = ArchitecturalFault {
    vector: 13,
    error_code: Some(0),
    linear_address: None,
};

fn run_store_set(
    state: &mut LongModeState,
    memory: &mut StoreMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
}

#[test]
fn gs_rip_relative_qword_store_writes_at_gs_base_plus_effective_address() {
    let gs_base = 0xffff_8880_1234_0000u64;
    let mut state = board_state();
    state.gs_base = gs_base;
    state.fs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let mut memory = StoreMemory::code(&GS_STORE_RIP);
    let result = run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // RIP-relative: next RIP (after all 8 bytes, the prefix included) + disp32, plus the base.
    let at = gs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.writes, vec![(at, STORE_RAX.to_le_bytes().to_vec())]);
    expected.rip += 8;
    assert_eq!(state, expected, "only RIP changes");
}

#[test]
fn fs_rip_relative_qword_store_writes_at_fs_base_plus_effective_address() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let mut memory = StoreMemory::code(&FS_STORE_RIP);
    let result = run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    let at = fs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.writes, vec![(at, STORE_RAX.to_le_bytes().to_vec())]);
    expected.rip += 8;
    assert_eq!(state, expected);
}

#[test]
fn gs_and_fs_stores_with_a_zero_base_use_the_effective_address() {
    for code in [GS_STORE_RIP, FS_STORE_RIP] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = StoreMemory::code(&code);
        let result = run_store_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            memory.writes,
            vec![(RIP + 8 + 0x1234, STORE_RAX.to_le_bytes().to_vec())]
        );
    }
}

#[test]
fn gs_store_with_base_register_index_and_extended_source_register() {
    // `mov %r8, %gs:0x10(%rbx,%rcx,4)` = 65 4c 89 44 8b 10.
    let code = [0x65, 0x4c, 0x89, 0x44, 0x8b, 0x10];
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::R8 as usize] = 0xdead_beef_cafe_f00d;
    let mut expected = state.clone();
    let mut memory = StoreMemory::code(&code);
    run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        memory.writes,
        vec![(
            0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10,
            0xdead_beef_cafe_f00du64.to_le_bytes().to_vec()
        )]
    );
    expected.rip += 6;
    assert_eq!(state, expected);
}

#[test]
fn gs_store_hands_the_whole_qword_to_the_backend_across_a_page_boundary() {
    // Splitting across pages is the backend's (the S4-38L4h adapter's) job: the executor issues
    // one 8-byte write at the 4 KiB-straddling address.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0ffc;
    state.gpr[Register::Rbx as usize] = STORE_RAX;
    state.gpr[Register::Rax as usize] = 0;
    // mov %rbx, %gs:(%rax)
    let mut memory = StoreMemory::code(&[0x65, 0x48, 0x89, 0x18]);
    run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        memory.writes,
        vec![(0xffff_8880_0000_0ffc, STORE_RAX.to_le_bytes().to_vec())]
    );
}

#[test]
fn gs_store_non_canonical_result_is_gp0_without_mutation_or_write() {
    // base + effective address (or its last byte) lands in the non-canonical hole.
    for base in [
        0x0000_8000_0000_0000u64,
        0x7fff_ffff_ffff_fff0,
        // The start is canonical but the qword's last byte crosses into the hole.
        0x0000_7fff_ffff_ffff - 0x1234 - 8 - RIP - 3,
    ] {
        let mut state = board_state();
        state.gs_base = base;
        let before = state.clone();
        let mut memory = StoreMemory::code(&GS_STORE_RIP);
        let result = run_store_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn gs_store_through_rsp_base_is_gp0_not_ss_when_non_canonical() {
    // `mov %rax, %gs:(%rsp)`: the GS segment is the one referenced, so #GP(0), not #SS.
    let mut state = board_state();
    state.gs_base = 0x0000_8000_0000_0000;
    state.gpr[Register::Rsp as usize] = 0;
    let before = state.clone();
    let mut memory = StoreMemory::code(&[0x65, 0x48, 0x89, 0x04, 0x24]);
    let result = run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
}

#[test]
fn segment_store_forms_outside_the_slice_are_refused_without_mutation() {
    // (code, expected refused opcode): the store forms refuse with 0x89,
    // the byte store refuses with its own opcode 0x88.
    let cases: [(&[u8], u8); 9] = [
        // No REX.W (dword store) and REX without W.
        (&[0x65, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00], 0x89),
        (&[0x64, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00], 0x89),
        (&[0x65, 0x40, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00], 0x89),
        // Byte store.
        (&[0x65, 0x88, 0x05, 0x34, 0x12, 0x00, 0x00], 0x88),
        // Register destination (mod = 11).
        (&[0x65, 0x48, 0x89, 0xc3], 0x89),
        // LOCK, 0x66 and 0x67.
        (
            &[0xf0, 0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00],
            0x89,
        ),
        (
            &[0x66, 0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00],
            0x89,
        ),
        (
            &[0x65, 0x67, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00],
            0x89,
        ),
        // REP.
        (
            &[0xf3, 0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00],
            0x89,
        ),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = StoreMemory::code(code);
        let result = run_store_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn unsupported_segment_store_forms_report_the_store_opcode() {
    for code in [
        &[0x65, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00][..],
        &[0x66, 0x65, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut state = board_state();
        let mut memory = StoreMemory::code(code);
        let result = run_store_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x89 });
    }
}

#[test]
fn plain_unprefixed_qword_store_still_writes_at_the_effective_address_in_the_set() {
    // Regression: `mov %rax, 0x1234(%rip)` without a segment override ignores the bases.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = StoreMemory::code(&[0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00]);
    run_store_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        memory.writes,
        vec![(RIP + 7 + 0x1234, STORE_RAX.to_le_bytes().to_vec())]
    );
}

#[test]
fn segment_store_stays_unsupported_outside_the_set() {
    for code in [GS_STORE_RIP, FS_STORE_RIP] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = StoreMemory::code(&code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x89 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cs_ds_es_ss_overrides_on_a_qword_store_keep_ignoring_the_segment_base() {
    // Unchanged by S4-38L7g: in 64-bit mode the CS/DS/ES/SS bases are ignored (SDM Vol.3 3.4.4),
    // and the decoder always accepted these prefixes on a plain MOV. The FS/GS bases do not apply.
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory = StoreMemory::code(&[prefix, 0x48, 0x89, 0x05, 0x34, 0x12, 0x00, 0x00]);
        let result = run_store_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(
            memory.writes,
            vec![(RIP + 8 + 0x1234, STORE_RAX.to_le_bytes().to_vec())],
            "{prefix:#x}"
        );
    }
}

// ---- S4-38L7i: FS/GS-based qword load `MOV r64, m64` (SDM Vol.2 segment override prefixes,
// Vol.3 3.4.4: in 64-bit mode only the FS/GS base is added; from memory, not re-checked) ----

/// Code at `RIP` plus a log of every read; the byte at address `a` reads as `a & 0xff`. Writes
/// are not expected.
struct LoadMemory {
    code: Vec<u8>,
    reads: Vec<(u64, usize)>,
    /// When set, every read returns this little-endian value instead of the address pattern.
    fixed: Option<u64>,
}

impl LoadMemory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            code: bytes.to_vec(),
            reads: Vec::new(),
            fixed: None,
        }
    }

    fn with_value(bytes: &[u8], value: u64) -> Self {
        Self {
            fixed: Some(value),
            ..Self::code(bytes)
        }
    }
}

fn pattern_qword(address: u64) -> u64 {
    let mut bytes = [0u8; 8];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (address.wrapping_add(i as u64) & 0xff) as u8;
    }
    u64::from_le_bytes(bytes)
}

impl GuestMemory for LoadMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        bytes: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads.push((address, bytes.len()));
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = match self.fixed {
                Some(value) => value.to_le_bytes()[i],
                None => (address.wrapping_add(i as u64) & 0xff) as u8,
            };
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

/// `mov %gs:0x1234(%rip), %r12`.
const GS_LOAD_R12_RIP: [u8; 8] = [0x65, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00];
/// `mov %fs:0x1234(%rip), %r12`.
const FS_LOAD_R12_RIP: [u8; 8] = [0x64, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00];

fn run_load_set(
    state: &mut LongModeState,
    memory: &mut LoadMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
}

#[test]
fn gs_rip_relative_qword_load_into_r12_reads_at_gs_base_plus_effective_address() {
    let gs_base = 0xffff_8880_1234_0000u64;
    let mut state = board_state();
    state.gs_base = gs_base;
    state.fs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&GS_LOAD_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // RIP-relative: next RIP (after all 8 bytes, the prefix included) + disp32, plus the base.
    let at = gs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 8)]);
    expected.gpr[Register::R12 as usize] = pattern_qword(at);
    expected.rip += 8;
    assert_eq!(state, expected, "only R12 and RIP change");
}

#[test]
fn fs_rip_relative_qword_load_reads_at_fs_base_plus_effective_address() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&FS_LOAD_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    let at = fs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 8)]);
    expected.gpr[Register::R12 as usize] = pattern_qword(at);
    expected.rip += 8;
    assert_eq!(state, expected);
}

#[test]
fn gs_and_fs_loads_with_a_zero_base_use_the_effective_address() {
    for code in [GS_LOAD_R12_RIP, FS_LOAD_R12_RIP] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = LoadMemory::code(&code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        let at = RIP + 8 + 0x1234;
        assert_eq!(memory.reads, vec![(at, 8)]);
        assert_eq!(state.gpr[Register::R12 as usize], pattern_qword(at));
    }
}

#[test]
fn gs_load_with_base_register_index_scale_and_displacement() {
    // `mov %gs:0x10(%rbx,%rcx,4), %r8` = 65 4c 8b 44 8b 10.
    let code = [0x65, 0x4c, 0x8b, 0x44, 0x8b, 0x10];
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&code);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 8)]);
    expected.gpr[Register::R8 as usize] = pattern_qword(at);
    expected.rip += 6;
    assert_eq!(state, expected);
}

#[test]
fn fs_load_absolute_sib_disp32_without_base_or_index_adds_only_the_fs_base() {
    // `mov %fs:0x20, %rdx` = 64 48 8b 14 25 20 00 00 00.
    let code = [0x64, 0x48, 0x8b, 0x14, 0x25, 0x20, 0, 0, 0];
    let mut state = board_state();
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = LoadMemory::code(&code);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0020;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(state.gpr[Register::Rdx as usize], pattern_qword(at));
}

#[test]
fn gs_load_into_rax_with_a_wider_form_than_the_exact_rax_one() {
    // `mov %gs:0x8(%rax), %rax` = 65 48 8b 40 08: not the exact S2-22b form.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rax as usize] = 0x100;
    let mut memory = LoadMemory::code(&[0x65, 0x48, 0x8b, 0x40, 0x08]);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0108;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(state.gpr[Register::Rax as usize], pattern_qword(at));
}

#[test]
fn exact_gs_and_fs_rax_loads_keep_working_in_the_set() {
    for (code, base_is_gs) in [
        ([0x65u8, 0x48, 0x8b, 0x00], true),
        ([0x64u8, 0x48, 0x8b, 0x00], false),
    ] {
        let mut state = board_state();
        let base = 0xffff_8880_0000_0000u64;
        if base_is_gs {
            state.gs_base = base;
        } else {
            state.fs_base = base;
        }
        state.gpr[Register::Rax as usize] = 0x40;
        let mut memory = LoadMemory::code(&code);
        run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(base + 0x40, 8)]);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            pattern_qword(base + 0x40)
        );
    }
}

#[test]
fn gs_load_non_canonical_result_is_gp0_without_mutation_or_read() {
    for base in [
        0x0000_8000_0000_0000u64,
        0x7fff_ffff_ffff_fff0,
        // The start is canonical but the qword's last byte crosses into the hole.
        0x0000_7fff_ffff_ffff - 0x1234 - 8 - RIP - 3,
    ] {
        let mut state = board_state();
        state.gs_base = base;
        let before = state.clone();
        let mut memory = LoadMemory::code(&GS_LOAD_R12_RIP);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn fs_load_non_canonical_result_is_gp0_without_mutation_or_read() {
    let mut state = board_state();
    state.fs_base = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = LoadMemory::code(&FS_LOAD_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn gs_load_through_rsp_or_rbp_base_is_gp0_not_ss_when_non_canonical() {
    // `mov %gs:(%rsp), %rax` = 65 48 8b 04 24 and `mov %gs:0x0(%rbp), %rax` = 65 48 8b 45 00:
    // the GS segment is the one referenced, so #GP(0), not #SS(0).
    for (code, register) in [
        (&[0x65u8, 0x48, 0x8b, 0x04, 0x24][..], Register::Rsp),
        (&[0x65u8, 0x48, 0x8b, 0x45, 0x00][..], Register::Rbp),
    ] {
        let mut state = board_state();
        state.gs_base = 0x0000_8000_0000_0000;
        state.gpr[register as usize] = 0;
        let before = state.clone();
        let mut memory = LoadMemory::code(code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn segment_load_forms_outside_the_slice_are_refused_without_mutation() {
    // (code, expected refused opcode): the load forms refuse with 0x8b,
    // the byte load refuses with its own opcode 0x8a.
    // (The dword load without REX.W became supported in S4-38L7m.)
    let cases: [(&[u8], u8); 8] = [
        // Byte load.
        (&[0x65, 0x4c, 0x8a, 0x25, 0x34, 0x12, 0x00, 0x00], 0x8a),
        // Register source (mod = 11).
        (&[0x65, 0x4c, 0x8b, 0xe3], 0x8b),
        (&[0x65, 0x48, 0x8b, 0xc0], 0x8b),
        // LOCK, 0x66, 0x67 and REP.
        (
            &[0xf0, 0x65, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
        (
            &[0x66, 0x65, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
        (
            &[0x65, 0x67, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
        (
            &[0xf3, 0x65, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
        (
            &[0xf2, 0x64, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::code(code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn unsupported_segment_load_forms_report_the_load_opcode() {
    for code in [
        &[0x65, 0x4c, 0x8b, 0xe3][..],
        &[0x66, 0x65, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00][..],
    ] {
        let mut state = board_state();
        let mut memory = LoadMemory::code(code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8b });
    }
}

#[test]
fn plain_unprefixed_qword_load_still_reads_at_the_effective_address_in_the_set() {
    // Regression: `mov 0x1234(%rip), %r12` without a segment override ignores the bases.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = LoadMemory::code(&[0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00]);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(RIP + 7 + 0x1234, 8)]);
}

#[test]
fn segment_load_stays_unsupported_outside_the_set() {
    for code in [GS_LOAD_R12_RIP, FS_LOAD_R12_RIP] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::code(&code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x8b });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn segment_load_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = LoadMemory::code(&GS_LOAD_R12_RIP);
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
fn cs_ds_es_ss_overrides_on_a_qword_load_keep_ignoring_the_segment_base() {
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory = LoadMemory::code(&[prefix, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00]);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(RIP + 8 + 0x1234, 8)], "{prefix:#x}");
    }
}

// ---- S4-38L7j: FS/GS-based qword `SUB r64, m64` (opcode 2B /r with REX.W; SDM Vol.2 SUB,
// segment override prefixes, Vol.3 3.4.4: in 64-bit mode only the FS/GS base is added) ----

const FLAG_CF: u64 = 1;
const FLAG_PF: u64 = 1 << 2;
const FLAG_AF: u64 = 1 << 4;
const FLAG_ZF: u64 = 1 << 6;
const FLAG_SF: u64 = 1 << 7;
const FLAG_OF: u64 = 1 << 11;
const ARITH: u64 = FLAG_CF | FLAG_PF | FLAG_AF | FLAG_ZF | FLAG_SF | FLAG_OF;

/// `sub %gs:0x1234(%rip), %r12`.
const GS_SUB_R12_RIP: [u8; 8] = [0x65, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00];
/// `sub %fs:0x1234(%rip), %r12`.
const FS_SUB_R12_RIP: [u8; 8] = [0x64, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00];

#[test]
fn gs_rip_relative_qword_sub_into_r12_reads_at_gs_base_plus_effective_address() {
    let gs_base = 0xffff_8880_1234_0000u64;
    let mut state = board_state();
    state.gs_base = gs_base;
    state.fs_base = 0xffff_8880_9999_0000;
    state.gpr[Register::R12 as usize] = 0x1000;
    let mut expected = state.clone();
    let mut memory = LoadMemory::with_value(&GS_SUB_R12_RIP, 0x0234);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // RIP-relative: next RIP (after all 8 bytes, the prefix included) + disp32, plus the base.
    assert_eq!(memory.reads, vec![(gs_base + RIP + 8 + 0x1234, 8)]);
    expected.gpr[Register::R12 as usize] = 0x1000 - 0x0234;
    expected.rip += 8;
    // 0x1000 - 0x234 = 0xdcc: PF (0xcc has four set bits), AF (low bytes 0x00, 0x34, 0xcc).
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF | FLAG_AF;
    assert_eq!(
        state, expected,
        "only R12, RIP and the arithmetic flags change"
    );
}

#[test]
fn fs_rip_relative_qword_sub_reads_at_fs_base_plus_effective_address() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    state.gpr[Register::R12 as usize] = 0x1000;
    let mut expected = state.clone();
    let mut memory = LoadMemory::with_value(&FS_SUB_R12_RIP, 0x0234);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(fs_base + RIP + 8 + 0x1234, 8)]);
    expected.gpr[Register::R12 as usize] = 0x1000 - 0x0234;
    expected.rip += 8;
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF | FLAG_AF;
    assert_eq!(state, expected);
}

#[test]
fn gs_and_fs_sub_with_a_zero_base_use_the_effective_address() {
    for code in [GS_SUB_R12_RIP, FS_SUB_R12_RIP] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        state.gpr[Register::R12 as usize] = 10;
        let mut memory = LoadMemory::with_value(&code, 3);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(memory.reads, vec![(RIP + 8 + 0x1234, 8)]);
        assert_eq!(state.gpr[Register::R12 as usize], 7);
    }
}

#[test]
fn gs_sub_with_base_register_index_scale_and_displacement() {
    // `sub %gs:0x10(%rbx,%rcx,4), %r8` = 65 4c 2b 44 8b 10.
    let code = [0x65, 0x4c, 0x2b, 0x44, 0x8b, 0x10];
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::R8 as usize] = 100;
    let mut expected = state.clone();
    let mut memory = LoadMemory::with_value(&code, 58);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        memory.reads,
        vec![(0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10, 8)]
    );
    expected.gpr[Register::R8 as usize] = 42;
    expected.rip += 6;
    // 100 - 58 = 0x2a: odd parity (PF clear), nibble borrow (AF).
    expected.rflags = (expected.rflags & !ARITH) | FLAG_AF;
    assert_eq!(state, expected);
}

#[test]
fn fs_sub_absolute_sib_disp32_without_base_or_index_adds_only_the_fs_base() {
    // `sub %fs:0x20, %rdx` = 64 48 2b 14 25 20 00 00 00.
    let code = [0x64, 0x48, 0x2b, 0x14, 0x25, 0x20, 0, 0, 0];
    let mut state = board_state();
    state.fs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rdx as usize] = 9;
    let mut memory = LoadMemory::with_value(&code, 4);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_0020, 8)]);
    assert_eq!(state.gpr[Register::Rdx as usize], 5);
}

#[test]
fn gs_sub_into_rax_with_a_base_register_aliasing_the_destination() {
    // `sub %gs:0x8(%rax), %rax` = 65 48 2b 40 08: the address uses the pre-state RAX.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rax as usize] = 0x100;
    let mut memory = LoadMemory::with_value(&[0x65, 0x48, 0x2b, 0x40, 0x08], 0x20);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_0108, 8)]);
    assert_eq!(state.gpr[Register::Rax as usize], 0xe0);
}

#[test]
fn gs_sub_the_diagnostic_boundary_form_decodes_and_executes() {
    // The S4-38L7i boundary bytes `65 48 2b 05 29 70 4f 00`: SUB RAX, %gs:disp32(%rip).
    let code = [0x65, 0x48, 0x2b, 0x05, 0x29, 0x70, 0x4f, 0x00];
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rax as usize] = 0x5000;
    let mut memory = LoadMemory::with_value(&code, 0x1000);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        memory.reads,
        vec![(0xffff_8880_0000_0000 + RIP + 8 + 0x4f7029, 8)]
    );
    assert_eq!(state.gpr[Register::Rax as usize], 0x4000);
    assert_eq!(state.rip, RIP + 8);
}

/// (minuend, subtrahend, result, expected arithmetic flags).
const SUB_FLAG_CASES: [(u64, u64, u64, u64); 7] = [
    // Equal operands: ZF and PF.
    (5, 5, 0, FLAG_ZF | FLAG_PF),
    // Borrow out of zero: CF, SF, PF, AF.
    (0, 1, u64::MAX, FLAG_CF | FLAG_SF | FLAG_PF | FLAG_AF),
    // Signed overflow, positive result from INT_MIN - 1: OF, PF, AF.
    (
        0x8000_0000_0000_0000,
        1,
        0x7fff_ffff_ffff_ffff,
        FLAG_OF | FLAG_PF | FLAG_AF,
    ),
    // Signed overflow, negative result from INT_MAX - (-1): OF, CF, SF, PF.
    (
        0x7fff_ffff_ffff_ffff,
        u64::MAX,
        0x8000_0000_0000_0000,
        FLAG_OF | FLAG_CF | FLAG_SF | FLAG_PF,
    ),
    // Low-nibble borrow only: PF, AF.
    (0x10, 1, 0x0f, FLAG_PF | FLAG_AF),
    // Subtracting zero from 7: no flag (odd parity, no borrow).
    (7, 0, 7, 0),
    // Negative minus a larger negative: CF, SF, PF, AF.
    (
        0xffff_ffff_ffff_fff0,
        0xffff_ffff_ffff_fff1,
        u64::MAX,
        FLAG_CF | FLAG_SF | FLAG_PF | FLAG_AF,
    ),
];

#[test]
fn gs_and_fs_sub_flags_and_results_on_boundary_values() {
    for code in [GS_SUB_R12_RIP, FS_SUB_R12_RIP] {
        for (lhs, rhs, result_value, arithmetic) in SUB_FLAG_CASES {
            for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
                let mut state = board_state();
                state.rflags = initial;
                state.gpr[Register::R12 as usize] = lhs;
                let mut memory = LoadMemory::with_value(&code, rhs);
                let result = run_load_set(&mut state, &mut memory).unwrap();
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(
                    state.gpr[Register::R12 as usize],
                    result_value,
                    "{lhs:#x}-{rhs:#x}"
                );
                // Non-arithmetic bits (IF, DF, bit 1) are preserved.
                assert_eq!(
                    state.rflags,
                    (initial & !ARITH) | arithmetic,
                    "{lhs:#x}-{rhs:#x} from {initial:#x}"
                );
            }
        }
    }
}

#[test]
fn gs_sub_flags_equal_the_non_prefixed_sub_r64_m64() {
    // The prefixed form shares the arithmetic of `sub 0x1234(%rip), %r12` (4c 2b 25 ...).
    let plain = [0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00];
    for (lhs, rhs) in [
        (0u64, 0u64),
        (1, 2),
        (0x8000_0000_0000_0000, 0x7fff_ffff_ffff_ffff),
        (0xffff_ffff_0000_0000, 0x0000_0000_ffff_ffff),
        (0x1234_5678_9abc_def0, 0x0fed_cba9_8765_4321),
        (u64::MAX, u64::MAX),
    ] {
        let mut prefixed = board_state();
        prefixed.gpr[Register::R12 as usize] = lhs;
        let mut plain_state = prefixed.clone();
        let mut memory = LoadMemory::with_value(&GS_SUB_R12_RIP, rhs);
        run_load_set(&mut prefixed, &mut memory).unwrap();
        let mut memory = LoadMemory::with_value(&plain, rhs);
        run_load_set(&mut plain_state, &mut memory).unwrap();
        assert_eq!(prefixed.gpr, plain_state.gpr, "{lhs:#x}-{rhs:#x}");
        assert_eq!(prefixed.rflags, plain_state.rflags, "{lhs:#x}-{rhs:#x}");
    }
}

#[test]
fn gs_sub_non_canonical_result_is_gp0_without_mutation_or_read() {
    for base in [
        0x0000_8000_0000_0000u64,
        0x7fff_ffff_ffff_fff0,
        // The start is canonical but the qword's last byte crosses into the hole.
        0x0000_7fff_ffff_ffff - 0x1234 - 8 - RIP - 3,
    ] {
        let mut state = board_state();
        state.gs_base = base;
        state.rflags |= FLAG_CF;
        state.gpr[Register::R12 as usize] = 77;
        let before = state.clone();
        let mut memory = LoadMemory::with_value(&GS_SUB_R12_RIP, 1);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn fs_sub_non_canonical_result_is_gp0_without_mutation_or_read() {
    let mut state = board_state();
    state.fs_base = 0x0000_8000_0000_0000;
    state.gpr[Register::R12 as usize] = 77;
    let before = state.clone();
    let mut memory = LoadMemory::with_value(&FS_SUB_R12_RIP, 1);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn gs_sub_through_rsp_or_rbp_base_is_gp0_not_ss_when_non_canonical() {
    // `sub %gs:(%rsp), %rax` = 65 48 2b 04 24 and `sub %gs:0x0(%rbp), %rax` = 65 48 2b 45 00.
    for (code, register) in [
        (&[0x65u8, 0x48, 0x2b, 0x04, 0x24][..], Register::Rsp),
        (&[0x65u8, 0x48, 0x2b, 0x45, 0x00][..], Register::Rbp),
    ] {
        let mut state = board_state();
        state.gs_base = 0x0000_8000_0000_0000;
        state.gpr[register as usize] = 0;
        let before = state.clone();
        let mut memory = LoadMemory::with_value(code, 1);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn segment_sub_forms_outside_the_slice_are_refused_without_mutation() {
    // (code, expected refused opcode): the SUB r64, m64 forms refuse with 0x2b; the other ALU
    // opcodes with a FS/GS memory operand refuse with their own opcode.
    let cases: [(&[u8], u8); 20] = [
        // Dword (no REX.W) and REX without W.
        (&[0x65, 0x2b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x2b),
        (&[0x64, 0x2b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x2b),
        (&[0x65, 0x44, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00], 0x2b),
        // Byte SUB r8, m8.
        (&[0x65, 0x4c, 0x2a, 0x25, 0x34, 0x12, 0x00, 0x00], 0x2a),
        // Register source (mod = 11).
        (&[0x65, 0x4c, 0x2b, 0xe3], 0x2b),
        (&[0x65, 0x48, 0x2b, 0xc0], 0x2b),
        (&[0x64, 0x48, 0x2b, 0xc0], 0x2b),
        // LOCK, 0x66, 0x67 and REP.
        (
            &[0xf0, 0x65, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x2b,
        ),
        (
            &[0x66, 0x65, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x2b,
        ),
        (
            &[0x65, 0x67, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x2b,
        ),
        (
            &[0xf3, 0x65, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x2b,
        ),
        (
            &[0xf2, 0x64, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x2b,
        ),
        // Memory destination SUB m64, r64 (29) and the neighbouring ALU forms stay refused.
        (&[0x65, 0x4c, 0x29, 0x25, 0x34, 0x12, 0x00, 0x00], 0x29),
        (&[0x65, 0x4c, 0x03, 0x25, 0x34, 0x12, 0x00, 0x00], 0x03),
        (&[0x65, 0x4c, 0x3b, 0x25, 0x34, 0x12, 0x00, 0x00], 0x3b),
        (&[0x64, 0x4c, 0x1b, 0x25, 0x34, 0x12, 0x00, 0x00], 0x1b),
        (&[0x64, 0x4c, 0x13, 0x25, 0x34, 0x12, 0x00, 0x00], 0x13),
        (&[0x64, 0x4c, 0x23, 0x25, 0x34, 0x12, 0x00, 0x00], 0x23),
        (&[0x64, 0x4c, 0x0b, 0x25, 0x34, 0x12, 0x00, 0x00], 0x0b),
        (&[0x64, 0x4c, 0x33, 0x25, 0x34, 0x12, 0x00, 0x00], 0x33),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::with_value(code, 1);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn plain_unprefixed_qword_sub_still_reads_at_the_effective_address_in_the_set() {
    // Regression: `sub 0x1234(%rip), %r12` without a segment override ignores the bases.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::R12 as usize] = 10;
    let mut memory = LoadMemory::with_value(&[0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00], 4);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(RIP + 7 + 0x1234, 8)]);
    assert_eq!(state.gpr[Register::R12 as usize], 6);
}

#[test]
fn segment_sub_stays_unsupported_outside_the_set() {
    for code in [GS_SUB_R12_RIP, FS_SUB_R12_RIP] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::with_value(&code, 1);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x2b });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn segment_sub_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = LoadMemory::with_value(&GS_SUB_R12_RIP, 1);
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
fn cs_ds_es_ss_overrides_on_a_qword_sub_keep_ignoring_the_segment_base() {
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory =
            LoadMemory::with_value(&[prefix, 0x4c, 0x2b, 0x25, 0x34, 0x12, 0x00, 0x00], 1);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(RIP + 8 + 0x1234, 8)], "{prefix:#x}");
    }
}

// ---- S4-38L7k: `LOCK CMPXCHG r/m64, r64` (`F0 REX.W 0F B1 /r`, memory destination; SDM Vol.2
// CMPXCHG: compare RAX with the destination, ZF = 1 and destination = source when equal,
// otherwise ZF = 0 and RAX = destination; the destination is written in both cases; from
// memory, not re-checked against the SDM text) ----

/// One qword of guest data plus a log of every read and write. Reads of other addresses return the
/// same cell, so the tests can address it with any base.
struct CasMemory {
    code: Vec<u8>,
    cell: u64,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
    /// When set, every data write fails with this fault (a read-only page).
    write_fault: Option<ArchitecturalFault>,
}

impl CasMemory {
    fn new(code: &[u8], cell: u64) -> Self {
        Self {
            code: code.to_vec(),
            cell,
            reads: Vec::new(),
            writes: Vec::new(),
            write_fault: None,
        }
    }
}

impl GuestMemory for CasMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        bytes: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads.push((address, bytes.len()));
        bytes.copy_from_slice(&self.cell.to_le_bytes()[..bytes.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, bytes: &[u8]) -> Result<(), MemoryError> {
        if let Some(fault) = self.write_fault.clone() {
            return Err(MemoryError::Architectural(fault));
        }
        self.writes.push((address, bytes.to_vec()));
        let mut value = [0u8; 8];
        value[..bytes.len()].copy_from_slice(bytes);
        self.cell = u64::from_le_bytes(value);
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
        unreachable!("the CPL0 set must not reach the backend atomic compare-exchange")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

fn run_cas_set(
    state: &mut LongModeState,
    memory: &mut CasMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
}

/// `lock cmpxchg %rdx, (%rbx)` (the S4-38L7j diagnostic boundary bytes).
const LOCK_CMPXCHG_RDX_RBX: [u8; 5] = [0xf0, 0x48, 0x0f, 0xb1, 0x13];
/// `lock cmpxchg %r12, 0x1234(%rip)`.
const LOCK_CMPXCHG_R12_RIP: [u8; 9] = [0xf0, 0x4c, 0x0f, 0xb1, 0x25, 0x34, 0x12, 0x00, 0x00];

#[test]
fn lock_cmpxchg_equal_sets_zf_writes_the_source_and_keeps_rax() {
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gpr[Register::Rdx as usize] = 0xdead_beef_0000_0001;
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_SF | FLAG_AF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, 0x1234_5678_9abc_def0);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 8)]);
    assert_eq!(
        memory.writes,
        vec![(0x20_0000, 0xdead_beef_0000_0001u64.to_le_bytes().to_vec())]
    );
    assert_eq!(memory.cell, 0xdead_beef_0000_0001);
    expected.rip += 5;
    // CMP RAX, dest with equal operands: ZF and PF (result 0), everything else clear.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_ZF | FLAG_PF;
    assert_eq!(state, expected, "only RIP and the arithmetic flags change");
}

#[test]
fn lock_cmpxchg_not_equal_clears_zf_loads_rax_and_writes_the_old_value_back() {
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x1000;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gpr[Register::Rdx as usize] = 0xdead_beef_0000_0001;
    state.rflags |= FLAG_ZF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, 0x0234);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 8)]);
    // The destination is written even though the compare failed, with its own value.
    assert_eq!(
        memory.writes,
        vec![(0x20_0000, 0x0234u64.to_le_bytes().to_vec())]
    );
    assert_eq!(memory.cell, 0x0234);
    expected.gpr[Register::Rax as usize] = 0x0234;
    expected.rip += 5;
    // CMP 0x1000, 0x234 = 0xdcc: PF (0xcc has four set bits) and AF; ZF cleared.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF | FLAG_AF;
    assert_eq!(
        state, expected,
        "only RAX, RIP and the arithmetic flags change"
    );
}

#[test]
fn lock_cmpxchg_flags_equal_cmp_rax_dest_on_boundary_values() {
    for (rax, dest, _, arithmetic) in SUB_FLAG_CASES {
        for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
            let mut state = board_state();
            state.rflags = initial;
            state.gpr[Register::Rax as usize] = rax;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            state.gpr[Register::Rdx as usize] = 0x77;
            let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, dest);
            let result = run_cas_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{rax:#x} vs {dest:#x}");
            // Non-arithmetic bits (IF, DF, bit 1) are preserved.
            assert_eq!(
                state.rflags,
                (initial & !ARITH) | arithmetic,
                "{rax:#x} vs {dest:#x} from {initial:#x}"
            );
            let equal = rax == dest;
            assert_eq!(
                state.gpr[Register::Rax as usize],
                if equal { rax } else { dest },
                "{rax:#x} vs {dest:#x}"
            );
            assert_eq!(memory.writes.len(), 1, "always written");
            assert_eq!(
                memory.cell,
                if equal { 0x77 } else { dest },
                "{rax:#x} vs {dest:#x}"
            );
        }
    }
}

#[test]
fn lock_cmpxchg_sets_cf_of_sf_af_and_pf_from_dedicated_pairs() {
    // CMP RAX, dest: (RAX, dest, expected arithmetic flags).
    let cases: [(u64, u64, u64); 5] = [
        // Borrow out of zero: CF, SF, PF, AF (0 - 1 = all ones).
        (0, 1, FLAG_CF | FLAG_SF | FLAG_PF | FLAG_AF),
        // Signed overflow into a positive result: INT_MIN - 1 = 0x7fff...ffff.
        (0x8000_0000_0000_0000, 1, FLAG_OF | FLAG_PF | FLAG_AF),
        // 1 - INT_MIN = 0x8000...0001: borrow, overflow and a negative result, odd low byte.
        (1, 0x8000_0000_0000_0000, FLAG_CF | FLAG_OF | FLAG_SF),
        // Low-nibble borrow: 0x10 - 1 = 0x0f has four set bits, so PF as well.
        (0x10, 1, FLAG_PF | FLAG_AF),
        // 3 - 0 = 3: two set bits, PF alone.
        (3, 0, FLAG_PF),
    ];
    for (rax, dest, arithmetic) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = rax;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, dest);
        run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(state.rflags & ARITH, arithmetic, "{rax:#x} vs {dest:#x}");
    }
}

#[test]
fn lock_cmpxchg_with_r12_source_and_rip_relative_destination() {
    // REX.R selects R12 as the source; the address is next RIP (all 9 bytes) + disp32.
    let at = RIP + 9 + 0x1234;
    // Equal: R12 is written to memory.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x55;
    state.gpr[Register::R12 as usize] = 0x99;
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_R12_RIP, 0x55);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x99u64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
    assert_ne!(state.rflags & FLAG_ZF, 0);
    assert_eq!(state.gpr[Register::Rax as usize], 0x55);
    // Not equal: RAX takes the old value, R12 is untouched.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x56;
    state.gpr[Register::R12 as usize] = 0x99;
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_R12_RIP, 0x55);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(state.gpr[Register::Rax as usize], 0x55);
    assert_eq!(state.gpr[Register::R12 as usize], 0x99);
    assert_eq!(state.rflags & FLAG_ZF, 0);
    assert_eq!(memory.writes, vec![(at, 0x55u64.to_le_bytes().to_vec())]);
}

const EXTENDED: [Register; 8] = [
    Register::R8,
    Register::R9,
    Register::R10,
    Register::R11,
    Register::R12,
    Register::R13,
    Register::R14,
    Register::R15,
];

#[test]
fn lock_cmpxchg_with_every_extended_source_register_r8_to_r15() {
    // `lock cmpxchg %rN, (%rbx)`: REX = 0x4c (W + R), reg = N & 7, modrm = 0x03 | (reg << 3).
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let code = [0xf0, 0x4c, 0x0f, 0xb1, 0x03 | ((i as u8) << 3)];
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 7;
        state.gpr[Register::Rbx as usize] = 0x30_0000;
        state.gpr[register as usize] = 0x1000 + i as u64;
        let mut memory = CasMemory::new(&code, 7);
        run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.cell, 0x1000 + i as u64, "source r{}", i + 8);
        assert_ne!(state.rflags & FLAG_ZF, 0, "source r{}", i + 8);
    }
}

#[test]
fn lock_cmpxchg_with_every_extended_base_register_r8_to_r15() {
    // `lock cmpxchg %rdx, 0x0(%rN)` via REX.B: mod = 01 with disp8 0; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x49, 0x0f, 0xb1];
        if i == 4 {
            code.extend([0x54, 0x24, 0x00]);
        } else {
            code.extend([0x50 | i as u8, 0x00]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 3;
        state.gpr[Register::Rdx as usize] = 0xabcd;
        state.gpr[register as usize] = base;
        let mut memory = CasMemory::new(&code, 3);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base, 8)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(base, 0xabcdu64.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_cmpxchg_with_base_index_scale_and_displacement() {
    // `lock cmpxchg %rdx, 0x10(%rbx,%rcx,4)` = f0 48 0f b1 54 8b 10.
    let code = [0xf0, 0x48, 0x0f, 0xb1, 0x54, 0x8b, 0x10];
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 9;
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::Rdx as usize] = 0x42;
    let mut memory = CasMemory::new(&code, 9);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = 0x50_0000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x42u64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 7);
}

#[test]
fn lock_cmpxchg_resolves_the_address_from_the_pre_state_rax_and_source_aliases() {
    // `lock cmpxchg %rax, (%rax)` = f0 48 0f b1 00: RAX is accumulator, source and base. The
    // destination holds a different value, so RAX takes it and memory gets its own value back.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x60_0000;
    let mut memory = CasMemory::new(&[0xf0, 0x48, 0x0f, 0xb1, 0x00], 0x1111);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0x60_0000, 8)]);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1111);
    assert_eq!(memory.cell, 0x1111);
    assert_eq!(state.rflags & FLAG_ZF, 0);
    // Equal case: memory holds RAX itself; the write stores RAX (the source), ZF = 1.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x60_0000;
    let mut memory = CasMemory::new(&[0xf0, 0x48, 0x0f, 0xb1, 0x00], 0x60_0000);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.cell, 0x60_0000);
    assert_ne!(state.rflags & FLAG_ZF, 0);
}

#[test]
fn lock_cmpxchg_non_canonical_address_is_gp0_without_mutation_or_access() {
    for rbx in [
        0x0000_8000_0000_0000u64,
        0xffff_7fff_ffff_ffff,
        // The start is canonical but the qword's last byte crosses into the hole.
        0x0000_7fff_ffff_fffd,
    ] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = rbx;
        state.gpr[Register::Rdx as usize] = 6;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x}");
        assert!(memory.reads.is_empty(), "{rbx:#x}");
        assert!(memory.writes.is_empty(), "{rbx:#x}");
        assert_eq!(memory.cell, 5);
    }
}

#[test]
fn lock_cmpxchg_through_a_non_canonical_rsp_base_is_ss0() {
    // `lock cmpxchg %rdx, (%rsp)` = f0 48 0f b1 14 24: the stack-based form reports #SS(0).
    let mut state = board_state();
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = CasMemory::new(&[0xf0, 0x48, 0x0f, 0xb1, 0x14, 0x24], 5);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn lock_cmpxchg_write_fault_leaves_rax_flags_and_memory_unchanged_for_both_outcomes() {
    // A read-only page: the always-performed write faults even when the compare fails.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0000),
    };
    for (rax, dest) in [(5u64, 5u64), (5, 9)] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = rax;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gpr[Register::Rdx as usize] = 0x77;
        state.rflags |= FLAG_CF | FLAG_ZF;
        let before = state.clone();
        let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, dest);
        memory.write_fault = Some(fault.clone());
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(fault.clone()),
            "{rax} vs {dest}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rax} vs {dest}");
        assert_eq!(memory.cell, dest);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_cmpxchg_forms_outside_the_slice_are_refused_with_opcode_0xb1_without_mutation() {
    // S4-38L7y: the LOCK dword form (without REX.W, with or without another REX) is now
    // admitted; its refusals are in `lock_cmpxchg_dword_forms_outside_the_slice_*`.
    let cases: [&[u8]; 17] = [
        // No LOCK: the plain memory form stays outside the CPL0 set.
        &[0x48, 0x0f, 0xb1, 0x13],
        &[0x4c, 0x0f, 0xb1, 0x25, 0x34, 0x12, 0x00, 0x00],
        // Register forms (mod = 11).
        &[0xf0, 0x48, 0x0f, 0xb1, 0xd3],
        &[0xf0, 0x4d, 0x0f, 0xb1, 0xc8],
        // 0x66 and 0x67.
        &[0x66, 0xf0, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x66, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x67, 0x48, 0x0f, 0xb1, 0x13],
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        &[0xf3, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf2, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0xf3, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf3, 0xf0, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0xf0, 0x48, 0x0f, 0xb1, 0x13],
        // Segment prefixes (FS/GS is a separate slice, the others stay refused here).
        &[0xf0, 0x64, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x65, 0x48, 0x0f, 0xb1, 0x13],
        &[0x65, 0xf0, 0x48, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x2e, 0x48, 0x0f, 0xb1, 0x13],
        // REX not directly before the opcode loses its W bit.
        &[0xf0, 0x48, 0x66, 0x0f, 0xb1, 0x13],
    ];
    for code in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xb1 },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn lock_cmpxchg_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK form is refused without mutation.
    let mut state = legacy_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, 5);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xb1 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn lock_cmpxchg_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_RDX_RBX, 5);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7y: `LOCK CMPXCHG r/m32, r32` (`F0 [REX] 0F B1 /r` without REX.W, memory destination;
// the dword extension of S4-38L7k: compare EAX with the dword destination, ZF = 1 and destination
// = source when equal, otherwise ZF = 0 and EAX = destination, which zero-extends into RAX; the
// destination is written in both cases; flags as CMP EAX, destination; from memory, not
// re-checked against the SDM text) ----

/// `lock cmpxchg %edx, (%rbx)` (the S4-38L7x diagnostic boundary bytes).
const LOCK_CMPXCHG_EDX_RBX: [u8; 4] = [0xf0, 0x0f, 0xb1, 0x13];

#[test]
fn lock_cmpxchg_dword_equal_sets_zf_writes_the_source_and_keeps_all_of_rax() {
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gpr[Register::Rdx as usize] = 0xdead_beef_0000_0001;
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_SF | FLAG_AF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 0x9abc_def0);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 4)]);
    assert_eq!(
        memory.writes,
        vec![(0x20_0000, 1u32.to_le_bytes().to_vec())]
    );
    assert_eq!(memory.cell, 1);
    expected.rip += 4;
    expected.rflags = (expected.rflags & !ARITH) | FLAG_ZF | FLAG_PF;
    assert_eq!(
        state, expected,
        "only RIP and the arithmetic flags change; the upper half of RAX is kept"
    );
}

#[test]
fn lock_cmpxchg_dword_not_equal_clears_zf_loads_eax_zero_extended_and_writes_back() {
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0xffff_ffff_0000_1000;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gpr[Register::Rdx as usize] = 0xdead_beef_0000_0001;
    state.rflags |= FLAG_ZF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 0x0234);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 4)]);
    assert_eq!(
        memory.writes,
        vec![(0x20_0000, 0x0234u32.to_le_bytes().to_vec())]
    );
    assert_eq!(memory.cell, 0x0234);
    // EAX = destination and the upper half of RAX is zeroed (32-bit register write).
    expected.gpr[Register::Rax as usize] = 0x0234;
    expected.rip += 4;
    // CMP 0x1000, 0x234 = 0xdcc: PF and AF; ZF cleared.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF | FLAG_AF;
    assert_eq!(
        state, expected,
        "only RAX, RIP and the arithmetic flags change"
    );
}

#[test]
fn lock_cmpxchg_dword_compares_only_eax_and_flags_follow_the_dword_result() {
    // (EAX, dest, expected arithmetic flags of CMP EAX, dest on 32 bits).
    let cases: [(u32, u32, u64); 9] = [
        (0x7fff_ffff, 0x7fff_ffff, FLAG_ZF | FLAG_PF),
        (0x8000_0000, 1, FLAG_OF | FLAG_PF | FLAG_AF),
        (1, 0x8000_0000, FLAG_CF | FLAG_OF | FLAG_SF),
        (0, 0xffff_ffff, FLAG_CF | FLAG_AF),
        (0xffff_ffff, 0, FLAG_SF | FLAG_PF),
        (0, 0, FLAG_ZF | FLAG_PF),
        (0x8000_0000, 0x7fff_ffff, FLAG_OF | FLAG_AF),
        (
            0x7fff_ffff,
            0x8000_0000,
            FLAG_CF | FLAG_OF | FLAG_SF | FLAG_PF,
        ),
        (0x1234_5678, 0x1234_5678, FLAG_ZF | FLAG_PF),
    ];
    for (eax, dest, arithmetic) in cases {
        for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
            // Garbage in the upper half of RAX must not take part in the compare.
            let rax = 0xa5a5_a5a5_0000_0000 | u64::from(eax);
            let mut state = board_state();
            state.rflags = initial;
            state.gpr[Register::Rax as usize] = rax;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            state.gpr[Register::Rdx as usize] = 0x77;
            let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, u64::from(dest));
            let result = run_cas_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{eax:#x} vs {dest:#x}");
            assert_eq!(
                state.rflags,
                (initial & !ARITH) | arithmetic,
                "{eax:#x} vs {dest:#x} from {initial:#x}"
            );
            let equal = eax == dest;
            assert_eq!(
                state.gpr[Register::Rax as usize],
                if equal { rax } else { u64::from(dest) },
                "{eax:#x} vs {dest:#x}"
            );
            assert_eq!(memory.writes.len(), 1, "always written");
            assert_eq!(memory.writes[0].1.len(), 4);
            assert_eq!(
                memory.cell,
                if equal { 0x77 } else { u64::from(dest) },
                "{eax:#x} vs {dest:#x}"
            );
        }
    }
}

#[test]
fn lock_cmpxchg_dword_with_r12d_source_and_rip_relative_destination() {
    // REX.R without W selects R12D as the source; the address is next RIP (all 9 bytes) + disp32.
    let code = [0xf0, 0x44, 0x0f, 0xb1, 0x25, 0x34, 0x12, 0x00, 0x00];
    let at = RIP + 9 + 0x1234;
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x55;
    state.gpr[Register::R12 as usize] = 0x1_0000_0099;
    let mut memory = CasMemory::new(&code, 0x55);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x99u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
    assert_ne!(state.rflags & FLAG_ZF, 0);
    assert_eq!(state.gpr[Register::Rax as usize], 0x55);
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x56;
    state.gpr[Register::R12 as usize] = 0x99;
    let mut memory = CasMemory::new(&code, 0x55);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(state.gpr[Register::Rax as usize], 0x55);
    assert_eq!(state.gpr[Register::R12 as usize], 0x99);
    assert_eq!(state.rflags & FLAG_ZF, 0);
    assert_eq!(memory.writes, vec![(at, 0x55u32.to_le_bytes().to_vec())]);
}

#[test]
fn lock_cmpxchg_dword_with_every_extended_source_register_r8d_to_r15d() {
    // `lock cmpxchg %rNd, (%rbx)`: REX = 0x44 (R only), reg = N & 7.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let code = [0xf0, 0x44, 0x0f, 0xb1, 0x03 | ((i as u8) << 3)];
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 7;
        state.gpr[Register::Rbx as usize] = 0x30_0000;
        state.gpr[register as usize] = 0x1000 + i as u64;
        let mut memory = CasMemory::new(&code, 7);
        run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.cell, 0x1000 + i as u64, "source r{}d", i + 8);
        assert_ne!(state.rflags & FLAG_ZF, 0, "source r{}d", i + 8);
    }
}

#[test]
fn lock_cmpxchg_dword_with_every_extended_base_register_r8_to_r15() {
    // `lock cmpxchg %edx, 0x0(%rN)` via REX.B (0x41); R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x41, 0x0f, 0xb1];
        if i == 4 {
            code.extend([0x54, 0x24, 0x00]);
        } else {
            code.extend([0x50 | i as u8, 0x00]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 3;
        state.gpr[Register::Rdx as usize] = 0xabcd;
        state.gpr[register as usize] = base;
        let mut memory = CasMemory::new(&code, 3);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base, 4)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(base, 0xabcdu32.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_cmpxchg_dword_with_base_index_scale_and_displacement() {
    // `lock cmpxchg %edx, 0x10(%rbx,%rcx,4)` = f0 0f b1 54 8b 10.
    let code = [0xf0, 0x0f, 0xb1, 0x54, 0x8b, 0x10];
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 9;
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::Rdx as usize] = 0x42;
    let mut memory = CasMemory::new(&code, 9);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = 0x50_0000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x42u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 6);
}

#[test]
fn lock_cmpxchg_dword_non_canonical_last_byte_is_gp0_without_mutation_or_access() {
    for rbx in [
        0x0000_8000_0000_0000u64,
        0xffff_7fff_ffff_ffff,
        // The start is canonical but the dword's last byte (address + 3) crosses into the hole.
        0x0000_7fff_ffff_fffd,
    ] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = rbx;
        state.gpr[Register::Rdx as usize] = 6;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x}");
        assert!(memory.reads.is_empty(), "{rbx:#x}");
        assert!(memory.writes.is_empty(), "{rbx:#x}");
        assert_eq!(memory.cell, 5);
    }
    // The last canonical dword (address + 3 = 0x7fff_ffff_ffff) is accepted.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 5;
    state.gpr[Register::Rbx as usize] = 0x0000_7fff_ffff_fffc;
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 5);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
}

#[test]
fn lock_cmpxchg_dword_through_a_non_canonical_rsp_or_rbp_base_is_ss0() {
    // `lock cmpxchg %edx, (%rsp)` = f0 0f b1 14 24; `lock cmpxchg %edx, 0x0(%rbp)` = f0 0f b1 55 00.
    for (code, base) in [
        (&[0xf0, 0x0f, 0xb1, 0x14, 0x24][..], Register::Rsp),
        (&[0xf0, 0x0f, 0xb1, 0x55, 0x00][..], Register::Rbp),
    ] {
        let mut state = board_state();
        state.gpr[base as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            }),
            "{base:?}"
        );
        assert_eq!(state, before, "{base:?}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_cmpxchg_dword_write_fault_leaves_rax_flags_and_memory_unchanged_for_both_outcomes() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0000),
    };
    for (eax, dest) in [(5u64, 5u64), (5, 9)] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 0xffff_ffff_0000_0000 | eax;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gpr[Register::Rdx as usize] = 0x77;
        state.rflags |= FLAG_CF | FLAG_ZF;
        let before = state.clone();
        let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, dest);
        memory.write_fault = Some(fault.clone());
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(fault.clone()),
            "{eax} vs {dest}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{eax} vs {dest}");
        assert_eq!(memory.cell, dest);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_cmpxchg_dword_forms_outside_the_slice_are_refused_with_opcode_0xb1_without_mutation() {
    let cases: [&[u8]; 21] = [
        // No LOCK: the plain dword form (memory or register) stays outside the CPL0 set.
        &[0x0f, 0xb1, 0x13],
        &[0x40, 0x0f, 0xb1, 0x13],
        &[0x44, 0x0f, 0xb1, 0x25, 0x34, 0x12, 0x00, 0x00],
        &[0x0f, 0xb1, 0xd3],
        // Register forms (mod = 11) with LOCK.
        &[0xf0, 0x0f, 0xb1, 0xd3],
        &[0xf0, 0x41, 0x0f, 0xb1, 0xc8],
        // 0x66 (16-bit form) and 0x67.
        &[0x66, 0xf0, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x66, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x67, 0x0f, 0xb1, 0x13],
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        &[0xf3, 0x0f, 0xb1, 0x13],
        &[0xf2, 0x0f, 0xb1, 0x13],
        &[0xf0, 0xf3, 0x0f, 0xb1, 0x13],
        &[0xf3, 0xf0, 0x0f, 0xb1, 0x13],
        &[0xf0, 0xf0, 0x0f, 0xb1, 0x13],
        // Segment prefixes, FS/GS included.
        &[0xf0, 0x64, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x65, 0x0f, 0xb1, 0x13],
        &[0x65, 0xf0, 0x0f, 0xb1, 0x13],
        &[0xf0, 0x2e, 0x0f, 0xb1, 0x13],
        // A REX not directly before the opcode is refused for the dword form (stale REX).
        &[0x41, 0xf0, 0x0f, 0xb1, 0x13],
        &[0x44, 0xf0, 0x0f, 0xb1, 0x13],
        &[0x48, 0xf0, 0x0f, 0xb1, 0x13],
    ];
    for code in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xb1 },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn lock_cmpxchg_dword_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK dword form is refused without mutation.
    let mut state = legacy_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 5);
    let result = run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xb1 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn lock_cmpxchg_dword_is_not_reachable_from_cpl3() {
    let mut state = board_state();
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_CMPXCHG_EDX_RBX, 5);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7l: `LOCK INC r/m32` (`F0 FF /0`, memory destination; SDM Vol.2 INC: destination + 1,
// OF/SF/ZF/AF/PF from the result, CF unchanged; from memory, not re-checked against the SDM
// text). The qword form (`F0 REX.W FF /0`) is decoded and executed the same way ----

/// `lock incl 0x58(%rbx)` (the S4-38L7k diagnostic boundary bytes).
const LOCK_INCL_RBX_58: [u8; 4] = [0xf0, 0xff, 0x43, 0x58];
/// `lock incq 0x58(%rbx)`.
const LOCK_INCQ_RBX_58: [u8; 5] = [0xf0, 0x48, 0xff, 0x43, 0x58];
/// `lock incl 0x1234(%rip)`.
const LOCK_INCL_RIP: [u8; 7] = [0xf0, 0xff, 0x05, 0x34, 0x12, 0x00, 0x00];

#[test]
fn lock_inc_dword_increments_and_writes_four_bytes_through_the_base_register() {
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.rflags |= FLAG_CF | FLAG_ZF | FLAG_SF | FLAG_OF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_INCL_RBX_58, 0x41);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0058, 4)]);
    assert_eq!(
        memory.writes,
        vec![(0x20_0058, 0x42u32.to_le_bytes().to_vec())]
    );
    assert_eq!(memory.cell, 0x42);
    expected.rip += 4;
    // 0x42 has two set bits: PF; no AF; CF survives, ZF/SF/OF are recomputed.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_CF | FLAG_PF;
    assert_eq!(state, expected, "only RIP and the arithmetic flags change");
}

#[test]
fn lock_inc_dword_overflow_and_wraparound_flags() {
    // (old value, expected OF/SF/ZF/AF/PF; CF is checked separately).
    let cases: [(u32, u64); 8] = [
        // INT_MAX + 1: overflow, negative result, even parity of the zero low byte, nibble carry.
        (0x7fff_ffff, FLAG_OF | FLAG_SF | FLAG_PF | FLAG_AF),
        // All ones wrap to zero: ZF, PF, AF.
        (0xffff_ffff, FLAG_ZF | FLAG_PF | FLAG_AF),
        (0, 0),
        // 0x0f -> 0x10: AF alone (one set bit, odd parity).
        (0x0f, FLAG_AF),
        // 2 -> 3: PF alone.
        (2, FLAG_PF),
        // 0x7f -> 0x80: AF alone.
        (0x7f, FLAG_AF),
        // 0xfffffffe -> 0xffffffff: negative, eight set bits in the low byte.
        (0xffff_fffe, FLAG_SF | FLAG_PF),
        (0x8000_0000, FLAG_SF),
    ];
    for (old, arithmetic) in cases {
        for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
            let mut state = board_state();
            state.rflags = initial;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            let mut memory = CasMemory::new(&LOCK_INCL_RBX_58, u64::from(old));
            let result = run_cas_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{old:#x}");
            assert_eq!(
                memory.cell,
                u64::from(old.wrapping_add(1)),
                "{old:#x} from {initial:#x}"
            );
            // IF, DF and bit 1 are preserved, and so is the previous CF.
            assert_eq!(
                state.rflags,
                (initial & !ARITH) | (initial & FLAG_CF) | arithmetic,
                "{old:#x} from {initial:#x}"
            );
        }
    }
}

#[test]
fn lock_inc_keeps_cf_clear_and_set_on_the_wrap_to_zero() {
    for cf in [0, FLAG_CF] {
        let mut state = board_state();
        state.rflags = (state.rflags & !ARITH) | cf;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let mut memory = CasMemory::new(&LOCK_INCL_RBX_58, 0xffff_ffff);
        run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.cell, 0);
        assert_eq!(state.rflags & FLAG_CF, cf);
        assert_ne!(state.rflags & FLAG_ZF, 0);
    }
}

#[test]
fn lock_inc_qword_form_increments_all_eight_bytes() {
    let cases: [(u64, u64); 4] = [
        (0xffff_ffff_ffff_ffff, FLAG_ZF | FLAG_PF | FLAG_AF),
        (0x7fff_ffff_ffff_ffff, FLAG_OF | FLAG_SF | FLAG_PF | FLAG_AF),
        // Carry out of the low dword does not wrap the qword.
        (0xffff_ffff, FLAG_PF | FLAG_AF),
        (0x1234, FLAG_PF),
    ];
    for (old, arithmetic) in cases {
        let mut state = board_state();
        state.rflags |= FLAG_CF;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let mut memory = CasMemory::new(&LOCK_INCQ_RBX_58, old);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{old:#x}");
        assert_eq!(memory.reads, vec![(0x20_0058, 8)]);
        assert_eq!(
            memory.writes,
            vec![(0x20_0058, old.wrapping_add(1).to_le_bytes().to_vec())]
        );
        assert_eq!(state.rip, RIP + 5);
        assert_eq!(state.rflags & ARITH, FLAG_CF | arithmetic, "{old:#x}");
    }
}

#[test]
fn lock_inc_with_rip_relative_and_sib_displacement_addresses() {
    // The RIP-relative address is next RIP (all 7 bytes) + disp32.
    let mut state = board_state();
    let mut memory = CasMemory::new(&LOCK_INCL_RIP, 9);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = RIP + 7 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 10u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 7);
    // `lock incl 0x10(%rbx,%rcx,4)` = f0 ff 44 8b 10.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut memory = CasMemory::new(&[0xf0, 0xff, 0x44, 0x8b, 0x10], 9);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = 0x50_0000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 10u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 5);
}

#[test]
fn lock_inc_with_every_extended_base_register_r8_to_r15() {
    // `lock incl 0x0(%rN)` via REX.B: mod = 01 with disp8 0; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x41, 0xff];
        if i == 4 {
            code.extend([0x44, 0x24, 0x00]);
        } else {
            code.extend([0x40 | i as u8, 0x00]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[register as usize] = base;
        let mut memory = CasMemory::new(&code, 7);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base, 4)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(base, 8u32.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_inc_non_canonical_address_is_gp0_without_mutation_or_access() {
    // 0x58 is added by the instruction: pick bases so the start, or the last byte, is outside.
    for (rbx, code) in [
        (0x0000_8000_0000_0000u64 - 0x58, &LOCK_INCL_RBX_58[..]),
        (0xffff_7fff_ffff_ffffu64 - 0x58, &LOCK_INCL_RBX_58[..]),
        // The start is canonical but the dword's last byte crosses into the hole.
        (0x0000_7fff_ffff_fffe - 0x58, &LOCK_INCL_RBX_58[..]),
        // And the same for the qword form.
        (0x0000_7fff_ffff_fffc - 0x58, &LOCK_INCQ_RBX_58[..]),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = rbx;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x}");
        assert!(memory.reads.is_empty(), "{rbx:#x}");
        assert!(memory.writes.is_empty(), "{rbx:#x}");
        assert_eq!(memory.cell, 5);
    }
}

#[test]
fn lock_inc_through_a_non_canonical_rsp_or_rbp_base_is_ss0() {
    // `lock incl (%rsp)` = f0 ff 04 24 and `lock incl 0x0(%rbp)` = f0 ff 45 00.
    for (register, code) in [
        (Register::Rsp, &[0xf0, 0xff, 0x04, 0x24][..]),
        (Register::Rbp, &[0xf0, 0xff, 0x45, 0x00][..]),
    ] {
        let mut state = board_state();
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_inc_write_fault_leaves_flags_rip_and_memory_unchanged() {
    // A read-only page: the always-performed write faults.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0058),
    };
    for (old, code) in [
        (0x7fff_ffffu64, &LOCK_INCL_RBX_58[..]),
        (0xffff_ffff_ffff_ffff, &LOCK_INCQ_RBX_58[..]),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = CasMemory::new(code, old);
        memory.write_fault = Some(fault.clone());
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.cell, old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_inc_forms_outside_the_slice_are_refused_with_opcode_0xff_without_mutation() {
    let cases: [&[u8]; 24] = [
        // DEC and the other FF group members (CALL, far CALL, JMP, far JMP, PUSH, /7).
        &[0xf0, 0xff, 0x4b, 0x58],
        &[0xf0, 0xff, 0x53, 0x58],
        &[0xf0, 0xff, 0x5b, 0x58],
        &[0xf0, 0xff, 0x63, 0x58],
        &[0xf0, 0xff, 0x6b, 0x58],
        &[0xf0, 0xff, 0x73, 0x58],
        &[0xf0, 0xff, 0x7b, 0x58],
        // Register forms (mod = 11).
        &[0xf0, 0xff, 0xc3],
        &[0xf0, 0x41, 0xff, 0xc0],
        &[0xf0, 0x48, 0xff, 0xc3],
        // 0x66 (word) and 0x67.
        &[0x66, 0xf0, 0xff, 0x43, 0x58],
        &[0xf0, 0x66, 0xff, 0x43, 0x58],
        &[0xf0, 0x67, 0xff, 0x43, 0x58],
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        &[0xf3, 0xff, 0x43, 0x58],
        &[0xf2, 0xff, 0x43, 0x58],
        &[0xf0, 0xf3, 0xff, 0x43, 0x58],
        &[0xf3, 0xf0, 0xff, 0x43, 0x58],
        &[0xf0, 0xf0, 0xff, 0x43, 0x58],
        // Segment prefixes.
        &[0xf0, 0x64, 0xff, 0x43, 0x58],
        &[0xf0, 0x65, 0xff, 0x43, 0x58],
        &[0x65, 0xf0, 0xff, 0x43, 0x58],
        &[0xf0, 0x2e, 0xff, 0x43, 0x58],
        // REX not directly before the opcode loses its W bit (and 0x66 refuses the form).
        &[0xf0, 0x48, 0x66, 0xff, 0x43, 0x58],
        &[0xf0, 0x66, 0x48, 0xff, 0x43, 0x58],
    ];
    for code in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xff },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn lock_inc_byte_form_stays_refused_with_opcode_0xfe() {
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let before = state.clone();
    let mut memory = CasMemory::new(&[0xf0, 0xfe, 0x43, 0x58], 5);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xfe });
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn plain_inc_memory_without_lock_keeps_its_existing_arithmetic_path_in_the_set() {
    // `ff 43 58` was already executed as an ordinary arithmetic INC; S4-38L7l does not narrow it.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = CasMemory::new(&[0xff, 0x43, 0x58], 0x41);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.cell, 0x42);
    assert_eq!(state.rip, RIP + 3);
}

#[test]
fn lock_inc_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK form is refused without mutation.
    for code in [&LOCK_INCL_RBX_58[..], &LOCK_INCQ_RBX_58[..]] {
        let mut state = legacy_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xff });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_inc_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_INCL_RBX_58, 5);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7m: FS/GS-based dword load `MOV r32, m32` (opcode 8B /r without REX.W; SDM Vol.2
// MOV, segment override prefixes, Vol.3 3.4.4; from memory, not re-checked against the primary
// source). The upper 32 bits of the destination are zeroed, flags are untouched. ----

fn pattern_dword(address: u64) -> u64 {
    pattern_qword(address) & 0xffff_ffff
}

/// `mov %gs:0x1234(%rip), %r12d`.
const GS_LOAD32_R12_RIP: [u8; 8] = [0x65, 0x44, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00];
/// `mov %fs:0x1234(%rip), %r12d`.
const FS_LOAD32_R12_RIP: [u8; 8] = [0x64, 0x44, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00];
/// `mov %gs:0x4ca573(%rip), %eax` (the S4-38L7l diagnostic boundary bytes).
const GS_LOAD32_EAX_BOUNDARY: [u8; 7] = [0x65, 0x8b, 0x05, 0x73, 0xa5, 0x4c, 0x00];

#[test]
fn gs_rip_relative_dword_load_into_r12d_reads_four_bytes_and_zero_extends() {
    let gs_base = 0xffff_8880_1234_0000u64;
    let mut state = board_state();
    state.gs_base = gs_base;
    state.fs_base = 0xffff_8880_9999_0000;
    state.gpr[Register::R12 as usize] = 0xdead_beef_cafe_f00d;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&GS_LOAD32_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // RIP-relative: next RIP (after all 8 bytes, the prefix included) + disp32, plus the base.
    let at = gs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 4)]);
    expected.gpr[Register::R12 as usize] = pattern_dword(at);
    expected.rip += 8;
    assert_eq!(
        state, expected,
        "only R12 (upper half zeroed) and RIP change"
    );
    assert_eq!(state.gpr[Register::R12 as usize] >> 32, 0);
}

#[test]
fn fs_rip_relative_dword_load_reads_at_fs_base_plus_effective_address() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    state.gpr[Register::R12 as usize] = u64::MAX;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&FS_LOAD32_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    let at = fs_base + RIP + 8 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 4)]);
    expected.gpr[Register::R12 as usize] = pattern_dword(at);
    expected.rip += 8;
    assert_eq!(state, expected);
}

#[test]
fn gs_dword_load_the_diagnostic_boundary_form_decodes_and_executes() {
    let mut state = board_state();
    assert_eq!(state.gpr[Register::Rax as usize] >> 32, 0x1111_2222);
    state.gs_base = 0xffff_8880_0000_0000;
    let mut expected = state.clone();
    let mut memory = LoadMemory::with_value(&GS_LOAD32_EAX_BOUNDARY, 0x8765_4321);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        memory.reads,
        vec![(0xffff_8880_0000_0000 + RIP + 7 + 0x4ca573, 4)]
    );
    // The old upper half 0x1111_2222 of RAX is gone.
    expected.gpr[Register::Rax as usize] = 0x8765_4321;
    expected.rip += 7;
    assert_eq!(state, expected);
}

#[test]
fn gs_and_fs_dword_loads_with_a_zero_base_use_the_effective_address() {
    for code in [GS_LOAD32_R12_RIP, FS_LOAD32_R12_RIP] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = LoadMemory::code(&code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        let at = RIP + 8 + 0x1234;
        assert_eq!(memory.reads, vec![(at, 4)]);
        assert_eq!(state.gpr[Register::R12 as usize], pattern_dword(at));
    }
}

#[test]
fn gs_dword_load_with_base_register_index_scale_and_displacement() {
    // `mov %gs:0x10(%rbx,%rcx,4), %r8d` = 65 44 8b 44 8b 10.
    let code = [0x65, 0x44, 0x8b, 0x44, 0x8b, 0x10];
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::R8 as usize] = u64::MAX;
    let mut expected = state.clone();
    let mut memory = LoadMemory::code(&code);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 4)]);
    expected.gpr[Register::R8 as usize] = pattern_dword(at);
    expected.rip += 6;
    assert_eq!(state, expected);
}

#[test]
fn fs_dword_load_absolute_sib_disp32_without_base_or_index_adds_only_the_fs_base() {
    // `mov %fs:0x20, %edx` = 64 8b 14 25 20 00 00 00.
    let code = [0x64, 0x8b, 0x14, 0x25, 0x20, 0, 0, 0];
    let mut state = board_state();
    state.fs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rdx as usize] = u64::MAX;
    let mut memory = LoadMemory::code(&code);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0020;
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(state.gpr[Register::Rdx as usize], pattern_dword(at));
}

#[test]
fn gs_dword_load_with_a_base_register_aliasing_the_destination() {
    // `mov %gs:0x8(%rax), %eax` = 65 8b 40 08: the address uses the pre-state RAX, and RAX is
    // fully replaced (upper half zeroed).
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rax as usize] = 0x100;
    let mut memory = LoadMemory::code(&[0x65, 0x8b, 0x40, 0x08]);
    run_load_set(&mut state, &mut memory).unwrap();
    let at = 0xffff_8880_0000_0108;
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(state.gpr[Register::Rax as usize], pattern_dword(at));
}

#[test]
fn dword_load_leaves_every_flag_unchanged() {
    for flags in [2 | IF, 2 | IF | ARITH, 2 | IF | FLAG_ZF | FLAG_CF] {
        let mut state = board_state();
        state.rflags = flags;
        state.gs_base = 0xffff_8880_0000_0000;
        let mut memory = LoadMemory::with_value(&GS_LOAD32_R12_RIP, 0);
        run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(state.rflags, flags, "{flags:#x}");
        assert_eq!(state.gpr[Register::R12 as usize], 0);
    }
}

#[test]
fn gs_dword_load_non_canonical_result_is_gp0_without_mutation_or_read() {
    for base in [
        0x0000_8000_0000_0000u64,
        0x7fff_ffff_ffff_fff0,
        // The start is canonical but the dword's last byte (start + 3) enters the hole.
        0x0000_7fff_ffff_fffe - 0x1234 - 8 - RIP,
        0x0000_7fff_ffff_fffd - 0x1234 - 8 - RIP,
    ] {
        let mut state = board_state();
        state.gs_base = base;
        state.gpr[Register::R12 as usize] = 0x5555_5555_5555_5555;
        let before = state.clone();
        let mut memory = LoadMemory::code(&GS_LOAD32_R12_RIP);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn dword_load_ending_exactly_at_the_canonical_boundary_is_read() {
    // Start 0x7fff_ffff_fffc: the last byte (start + 3) is 0x7fff_ffff_ffff, still canonical.
    let mut state = board_state();
    state.gs_base = 0x0000_7fff_ffff_fffc - 0x1234 - 8 - RIP;
    let mut memory = LoadMemory::code(&GS_LOAD32_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x0000_7fff_ffff_fffc, 4)]);
}

#[test]
fn fs_dword_load_non_canonical_result_is_gp0_without_mutation_or_read() {
    let mut state = board_state();
    state.fs_base = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = LoadMemory::code(&FS_LOAD32_R12_RIP);
    let result = run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty());
}

#[test]
fn gs_dword_load_through_rsp_or_rbp_base_is_gp0_not_ss_when_non_canonical() {
    // `mov %gs:(%rsp), %eax` = 65 8b 04 24 and `mov %gs:0x0(%rbp), %eax` = 65 8b 45 00.
    for (code, register) in [
        (&[0x65u8, 0x8b, 0x04, 0x24][..], Register::Rsp),
        (&[0x65u8, 0x8b, 0x45, 0x00][..], Register::Rbp),
    ] {
        let mut state = board_state();
        state.gs_base = 0x0000_8000_0000_0000;
        state.gpr[register as usize] = 0;
        let before = state.clone();
        let mut memory = LoadMemory::code(code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn segment_dword_load_forms_outside_the_slice_are_refused_with_the_exact_opcode() {
    // (code, expected refused opcode); none of them mutates or reads.
    let cases: [(&[u8], u8); 12] = [
        // Register source (mod = 11).
        (&[0x65, 0x8b, 0xc0], 0x8b),
        (&[0x64, 0x44, 0x8b, 0xe3], 0x8b),
        // LOCK, REP/REPNE, 0x66 (16-bit) and 0x67.
        (&[0xf0, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        (&[0xf3, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        (&[0xf2, 0x64, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        (&[0x66, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        (&[0x65, 0x67, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        // A second segment prefix.
        (&[0x64, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        // REX before a legacy prefix (not immediately before the opcode). SDM Vol.2 2.2.1
        // ignores such a REX; the dword form is refused conservatively here.
        (&[0x44, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        (&[0x48, 0x65, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8b),
        // The byte load stays refused with its own opcode.
        (&[0x65, 0x8a, 0x05, 0x34, 0x12, 0x00, 0x00], 0x8a),
        // The word load via 0x66 with REX.R.
        (
            &[0x66, 0x65, 0x44, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            0x8b,
        ),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::code(code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn qword_load_with_a_stale_rex_before_the_segment_prefix_is_still_a_qword_load() {
    // `48 65 48 8b 05 ..` and `44 64 4c 8b 25 ..` (accepted in S4-38L7i): the first REX is not
    // immediately before the opcode and is ignored (SDM Vol.2 2.2.1); the last REX has W, so the
    // form is the qword load, not refused.
    for (code, fs, register) in [
        (
            [0x48u8, 0x65, 0x48, 0x8b, 0x05, 0x34, 0x12, 0x00, 0x00],
            false,
            Register::Rax,
        ),
        (
            [0x44u8, 0x64, 0x4c, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00],
            true,
            Register::R12,
        ),
    ] {
        let base = 0xffff_8880_1234_0000u64;
        let mut state = board_state();
        state.gs_base = if fs { 0xffff_8880_9999_0000 } else { base };
        state.fs_base = if fs { base } else { 0xffff_8880_9999_0000 };
        let mut expected = state.clone();
        let mut memory = LoadMemory::code(&code);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        let at = base + RIP + 9 + 0x1234;
        assert_eq!(memory.reads, vec![(at, 8)], "{code:02x?}");
        expected.gpr[register as usize] = pattern_qword(at);
        expected.rip += 9;
        assert_eq!(state, expected, "{code:02x?}");
    }
}

#[test]
fn plain_unprefixed_dword_load_still_ignores_the_segment_bases_in_the_set() {
    // Regression: `mov 0x1234(%rip), %r12d` without a segment override ignores the bases.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = LoadMemory::code(&[0x44, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00]);
    run_load_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(RIP + 7 + 0x1234, 4)]);
}

#[test]
fn segment_dword_load_stays_unsupported_outside_the_set() {
    for code in [
        &GS_LOAD32_R12_RIP[..],
        &FS_LOAD32_R12_RIP[..],
        &GS_LOAD32_EAX_BOUNDARY[..],
    ] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = LoadMemory::code(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x8b });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn segment_dword_load_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = LoadMemory::code(&GS_LOAD32_R12_RIP);
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
fn cs_ds_es_ss_overrides_on_a_dword_load_keep_ignoring_the_segment_base() {
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory = LoadMemory::code(&[prefix, 0x44, 0x8b, 0x25, 0x34, 0x12, 0x00, 0x00]);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(RIP + 8 + 0x1234, 4)], "{prefix:#x}");
    }
}

// ---- S4-38L7n: `LOCK BTS r/m64, r64` (`F0 REX.W 0F AB /r`, memory operand; SDM Vol.2 BTS: the
// register is a signed bit offset into a bit string starting at the memory operand, the qword that
// holds the bit is at EA + floor(offset / 64) * 8, CF = the old bit, the bit is set; OF/SF/AF/PF are
// undefined and ZF is not affected, and all of them are left unchanged here; from memory, not
// re-checked against the SDM text). The qword is always written back. ----

/// Sparse guest data (zero by default, byte-granular) plus a log of every read and write.
struct BitsMemory {
    code: Vec<u8>,
    data: std::collections::BTreeMap<u64, u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
    /// When set, every data write fails with this fault (a read-only page).
    write_fault: Option<ArchitecturalFault>,
    /// When set, every data read fails with this fault (a not-present page).
    read_fault: Option<ArchitecturalFault>,
}

impl BitsMemory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            data: std::collections::BTreeMap::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            write_fault: None,
            read_fault: None,
        }
    }

    fn set_qword(&mut self, address: u64, value: u64) {
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.data.insert(address + index as u64, byte);
        }
    }

    fn qword(&self, address: u64) -> u64 {
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self
                .data
                .get(&(address + index as u64))
                .copied()
                .unwrap_or(0);
        }
        u64::from_le_bytes(bytes)
    }
}

impl GuestMemory for BitsMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        bytes: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.reads.push((address, bytes.len()));
        if let Some(fault) = self.read_fault.clone() {
            return Err(MemoryError::Architectural(fault));
        }
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self
                .data
                .get(&(address + index as u64))
                .copied()
                .unwrap_or(0);
        }
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, bytes: &[u8]) -> Result<(), MemoryError> {
        if let Some(fault) = self.write_fault.clone() {
            return Err(MemoryError::Architectural(fault));
        }
        self.writes.push((address, bytes.to_vec()));
        for (index, byte) in bytes.iter().enumerate() {
            self.data.insert(address + index as u64, *byte);
        }
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
        unreachable!("the CPL0 set must not reach the backend atomic compare-exchange")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

fn run_bts_set(
    state: &mut LongModeState,
    memory: &mut BitsMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &base(),
        None,
        || false,
    )
}

/// `lock bts %rax, (%rbx)`.
const LOCK_BTS_RAX_RBX: [u8; 5] = [0xf0, 0x48, 0x0f, 0xab, 0x03];
/// `lock bts %rax, 0x1234(%rip)`.
const LOCK_BTS_RAX_RIP: [u8; 9] = [0xf0, 0x48, 0x0f, 0xab, 0x05, 0x34, 0x12, 0x00, 0x00];
/// `lock bts %rax, -0x883ed(%rbx)` (the S4-38L7m diagnostic displacement on a base register).
const LOCK_BTS_RAX_RBX_NEG: [u8; 9] = [0xf0, 0x48, 0x0f, 0xab, 0x83, 0x13, 0x7c, 0xf7, 0xff];
/// The S4-38L7m diagnostic boundary: `lock bts %rax, -0x883ed(%rip)`.
const LOCK_BTS_BOUNDARY: [u8; 9] = [0xf0, 0x48, 0x0f, 0xab, 0x05, 0x13, 0x7c, 0xf7, 0xff];

fn bts_state(offset: u64) -> LongModeState {
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = offset;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state
}

#[test]
fn lock_bts_sets_every_bit_of_the_first_qword_and_writes_it_back() {
    for offset in 0u64..64 {
        let mut state = bts_state(offset);
        let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
        let neighbour = 0xa5a5_5a5a_dead_beefu64;
        memory.set_qword(0x20_0008, neighbour);
        memory.set_qword(0x1f_fff8, neighbour);
        let mut expected = state.clone();
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1, "offset {offset}");
        assert_eq!(memory.reads, vec![(0x20_0000, 8)], "offset {offset}");
        assert_eq!(
            memory.writes,
            vec![(0x20_0000, (1u64 << offset).to_le_bytes().to_vec())],
            "offset {offset}"
        );
        assert_eq!(memory.qword(0x20_0000), 1u64 << offset);
        assert_eq!(memory.qword(0x20_0008), neighbour, "neighbour after");
        assert_eq!(memory.qword(0x1f_fff8), neighbour, "neighbour before");
        expected.rip += 5;
        assert_eq!(
            state, expected,
            "offset {offset}: only RIP changes (old bit was 0, CF stays 0)"
        );
    }
}

#[test]
fn lock_bts_reports_the_old_bit_in_cf_and_still_writes_when_it_was_already_set() {
    for (old, offset, cf) in [
        (0u64, 5u64, 0u64),
        (1 << 5, 5, 1),
        (u64::MAX, 63, 1),
        (u64::MAX >> 1, 63, 0),
        (0x8000_0000_0000_0000, 0, 0),
    ] {
        for initial_cf in [0, FLAG_CF] {
            let mut state = bts_state(offset);
            state.rflags = (state.rflags & !FLAG_CF) | initial_cf;
            let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
            memory.set_qword(0x20_0000, old);
            run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(state.rflags & FLAG_CF, cf, "{old:#x} bit {offset}");
            assert_eq!(memory.qword(0x20_0000), old | (1 << offset));
            // The write happens even when the bit was already set.
            assert_eq!(
                memory.writes,
                vec![(0x20_0000, (old | (1 << offset)).to_le_bytes().to_vec())],
                "{old:#x} bit {offset}"
            );
        }
    }
}

#[test]
fn lock_bts_leaves_every_flag_but_cf_unchanged() {
    for undefined in [0, FLAG_OF | FLAG_SF | FLAG_ZF | FLAG_AF | FLAG_PF] {
        for old in [0u64, 1 << 9] {
            let mut state = bts_state(9);
            state.rflags = (state.rflags & !ARITH) | undefined;
            let before = state.clone();
            let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
            memory.set_qword(0x20_0000, old);
            run_bts_set(&mut state, &mut memory).unwrap();
            let cf = u64::from(old != 0);
            assert_eq!(state.rflags & ARITH, undefined | cf);
            assert_eq!(state.rflags & !ARITH, before.rflags & !ARITH);
        }
    }
}

#[test]
fn lock_bts_register_offset_selects_the_qword_by_an_arithmetic_shift() {
    // (offset, byte delta of the qword that holds the bit, bit inside it).
    let cases: [(i64, i64, u32); 12] = [
        (-1, -8, 63),
        (-64, -8, 0),
        (-65, -16, 63),
        (-128, -16, 0),
        (-129, -24, 63),
        (64, 8, 0),
        (65, 8, 1),
        (64 * 5 + 3, 40, 3),
        (64 * 5 + 63, 40, 63),
        (64 * 1000 + 7, 8000, 7),
        (-64 * 1000, -8000, 0),
        (-(64 * 1000) - 1, -8008, 63),
    ];
    for (offset, delta, bit) in cases {
        let mut state = bts_state(offset as u64);
        state.gpr[Register::Rbx as usize] = 0x40_0000;
        let at = (0x40_0000i64 + delta) as u64;
        let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
        let marker = 0x0123_4567_89ab_cdefu64 & !(1u64 << bit);
        memory.set_qword(at, marker);
        let mut expected = state.clone();
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "offset {offset}");
        assert_eq!(memory.reads, vec![(at, 8)], "offset {offset}");
        assert_eq!(
            memory.writes,
            vec![(at, (marker | (1u64 << bit)).to_le_bytes().to_vec())],
            "offset {offset}"
        );
        expected.rip += 5;
        assert_eq!(state, expected, "offset {offset}");
        // The bit string base itself is untouched unless the offset lands in it.
        if delta != 0 {
            assert_eq!(memory.qword(0x40_0000), 0, "offset {offset}");
        }
    }
}

#[test]
fn lock_bts_with_every_extended_source_register_r8_to_r15() {
    // `lock bts %rN, (%rbx)`: REX = 0x4c (W + R), reg = N & 7, modrm = 0x03 | (reg << 3).
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let code = [0xf0, 0x4c, 0x0f, 0xab, 0x03 | ((i as u8) << 3)];
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x30_0000;
        state.gpr[register as usize] = 64 + 10 + i as u64;
        // RAX must not be used as the offset by mistake.
        state.gpr[Register::Rax as usize] = 3;
        let mut memory = BitsMemory::new(&code);
        run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(0x30_0008, 8)], "source r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(0x30_0008, (1u64 << (10 + i)).to_le_bytes().to_vec())],
            "source r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + 5);
    }
}

#[test]
fn lock_bts_with_every_extended_base_register_r8_to_r15() {
    // `lock bts %rax, 0x0(%rN)` via REX.B: mod = 01 with disp8 0; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x49, 0x0f, 0xab];
        if i == 4 {
            code.extend([0x44, 0x24, 0x00]);
        } else {
            code.extend([0x40 | i as u8, 0x00]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[register as usize] = base;
        state.gpr[Register::Rax as usize] = 64 * 2 + 1;
        let mut memory = BitsMemory::new(&code);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base + 16, 8)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(base + 16, 2u64.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_bts_with_rip_relative_and_negative_displacement_addresses() {
    // The RIP-relative address is next RIP (all 9 bytes) + disp32, plus the bit string delta.
    let mut state = bts_state(64 * 3 + 2);
    let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RIP);
    run_bts_set(&mut state, &mut memory).unwrap();
    let at = RIP + 9 + 0x1234 + 24;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 4u64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
    // A negative disp32 on a base register, with a negative bit offset.
    let mut state = bts_state((-1i64) as u64);
    state.gpr[Register::Rbx as usize] = 0x100_0000;
    let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX_NEG);
    run_bts_set(&mut state, &mut memory).unwrap();
    let at = 0x100_0000 - 0x883ed - 8;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(
        memory.writes,
        vec![(at, (1u64 << 63).to_le_bytes().to_vec())]
    );
    // The diagnostic boundary bytes decode and run: next RIP - 0x883ed wraps into the canonical
    // high half, and the bit string delta of offset 70 is one qword.
    let mut state = bts_state(70);
    let mut memory = BitsMemory::new(&LOCK_BTS_BOUNDARY);
    run_bts_set(&mut state, &mut memory).unwrap();
    let at = (RIP + 9).wrapping_sub(0x883ed) + 8;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x40u64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
}

#[test]
fn lock_bts_with_sib_base_index_scale_and_displacement() {
    // `lock bts %rax, 0x10(%rbx,%rcx,4)` = f0 48 0f ab 44 8b 10.
    let mut state = bts_state(64 * 2 + 5);
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut memory = BitsMemory::new(&[0xf0, 0x48, 0x0f, 0xab, 0x44, 0x8b, 0x10]);
    run_bts_set(&mut state, &mut memory).unwrap();
    let at = 0x50_0000 + 12 + 0x10 + 16;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x20u64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 7);
}

#[test]
fn lock_bts_resolves_the_address_and_offset_from_the_pre_state_when_they_alias() {
    // `lock bts %rbx, (%rbx)`: the base register is also the offset register.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000 + 3;
    let mut memory = BitsMemory::new(&[0xf0, 0x48, 0x0f, 0xab, 0x1b]);
    run_bts_set(&mut state, &mut memory).unwrap();
    // offset = 0x200003: qword delta = (0x200003 >> 6) * 8, bit = 3.
    let at = 0x20_0003 + (0x20_0003u64 >> 6) * 8;
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 8u64.to_le_bytes().to_vec())]);
    assert_eq!(
        state.gpr[Register::Rbx as usize],
        0x20_0003,
        "registers are not written"
    );
}

#[test]
fn lock_bts_non_canonical_address_is_gp0_without_mutation_or_access() {
    // (rbx, offset): the start, the shifted start, or the last byte of the qword leaves the
    // canonical ranges.
    for (rbx, offset) in [
        (0x0000_8000_0000_0000u64, 0u64),
        (0xffff_7fff_ffff_ffff, 0),
        // The start is canonical but the qword's last byte crosses into the hole.
        (0x0000_7fff_ffff_fffc, 0),
        // The bit string delta pushes the qword into the hole.
        (0x0000_7fff_ffff_fff8, 64),
        // ... and from the high half down into it.
        (0xffff_8000_0000_0000, (-65i64) as u64),
        // Extreme offsets wrap the 64-bit address arithmetic.
        (0x20_0000, i64::MIN as u64),
        (0x20_0000, i64::MAX as u64),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = offset;
        state.gpr[Register::Rbx as usize] = rbx;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x} {offset:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x} {offset:#x}");
        assert!(memory.reads.is_empty(), "{rbx:#x} {offset:#x}");
        assert!(memory.writes.is_empty(), "{rbx:#x} {offset:#x}");
    }
}

#[test]
fn lock_bts_through_an_rsp_or_rbp_base_is_ss0() {
    // `lock bts %rax, (%rsp)` = f0 48 0f ab 04 24 and `lock bts %rax, 0x0(%rbp)` = f0 48 0f ab 45 00.
    for (register, code) in [
        (Register::Rsp, &[0xf0, 0x48, 0x0f, 0xab, 0x04, 0x24][..]),
        (Register::Rbp, &[0xf0, 0x48, 0x0f, 0xab, 0x45, 0x00][..]),
    ] {
        for (base, offset) in [
            (0x0000_8000_0000_0000u64, 0u64),
            // A canonical stack pointer whose bit string delta leaves the canonical range.
            (0x0000_7fff_ffff_fff8, 64),
        ] {
            let mut state = board_state();
            state.gpr[register as usize] = base;
            state.gpr[Register::Rax as usize] = offset;
            let before = state.clone();
            let mut memory = BitsMemory::new(code);
            let result = run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::Fault(ArchitecturalFault {
                    vector: 12,
                    error_code: Some(0),
                    linear_address: None,
                }),
                "{register:?} {base:#x}"
            );
            assert_eq!(state, before);
            assert!(memory.reads.is_empty() && memory.writes.is_empty());
        }
    }
}

#[test]
fn lock_bts_write_fault_leaves_cf_rip_registers_and_memory_unchanged() {
    // A read-only page: the always-performed write faults, even when the bit was already set.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0008),
    };
    for (old, cf) in [(0u64, FLAG_CF), (1 << 3, 0), (1 << 3, FLAG_CF)] {
        let mut state = bts_state(64 + 3);
        state.rflags = (state.rflags & !ARITH) | cf | FLAG_ZF | FLAG_OF;
        let before = state.clone();
        let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
        memory.set_qword(0x20_0008, old);
        memory.write_fault = Some(fault.clone());
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.qword(0x20_0008), old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_bts_forms_outside_the_slice_are_refused_with_the_exact_opcode_without_mutation() {
    let cases: [(&[u8], u8); 25] = [
        // No REX.W (dword), REX without W, REX.R without W.
        (&[0xf0, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x40, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x44, 0x0f, 0xab, 0x03], 0xab),
        // Register forms (mod = 11).
        (&[0xf0, 0x48, 0x0f, 0xab, 0xc3], 0xab),
        (&[0xf0, 0x4d, 0x0f, 0xab, 0xc8], 0xab),
        // 0x66 and 0x67.
        (&[0x66, 0xf0, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x66, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x67, 0x48, 0x0f, 0xab, 0x03], 0xab),
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        (&[0xf3, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf2, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0xf3, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf3, 0xf0, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0xf0, 0x48, 0x0f, 0xab, 0x03], 0xab),
        // Segment prefixes.
        (&[0xf0, 0x64, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x65, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0x65, 0xf0, 0x48, 0x0f, 0xab, 0x03], 0xab),
        (&[0xf0, 0x2e, 0x48, 0x0f, 0xab, 0x03], 0xab),
        // REX not directly before the opcode loses its W bit.
        (&[0xf0, 0x48, 0x66, 0x0f, 0xab, 0x03], 0xab),
        // The immediate form (`0F BA /4..7 ib`) and the other LOCK bit-test forms stay outside
        // (LOCK BTR `0F B3` was admitted by S4-38L7t).
        (&[0xf0, 0x48, 0x0f, 0xba, 0x2b, 0x05], 0xba),
        (&[0xf0, 0x48, 0x0f, 0xba, 0x23, 0x05], 0xba),
        (&[0xf0, 0x48, 0x0f, 0xba, 0x33, 0x05], 0xba),
        (&[0xf0, 0x48, 0x0f, 0xba, 0x3b, 0x05], 0xba),
        (&[0xf0, 0x48, 0x0f, 0xa3, 0x03], 0xa3),
        (&[0xf0, 0x48, 0x0f, 0xbb, 0x03], 0xbb),
        (&[0xf0, 0x4c, 0x0f, 0xbb, 0x03], 0xbb),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn plain_bts_without_lock_keeps_its_existing_path_in_the_set() {
    // S4-38L7n does not narrow the non-LOCK BTS: its read -> write path needs no operation the
    // adapter lacks. Register and immediate offsets keep executing as before.
    let mut state = bts_state(64 + 4);
    let mut memory = BitsMemory::new(&[0x48, 0x0f, 0xab, 0x03]);
    let result = run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.qword(0x20_0008), 1 << 4);
    assert_eq!(state.rip, RIP + 4);
    let mut state = bts_state(0);
    let mut memory = BitsMemory::new(&[0x48, 0x0f, 0xba, 0x2b, 0x07]);
    let result = run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.qword(0x20_0000), 1 << 7);
}

#[test]
fn lock_bts_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK form is refused without mutation.
    for code in [&LOCK_BTS_RAX_RBX[..], &LOCK_BTS_BOUNDARY[..]] {
        let mut state = legacy_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xab });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_bts_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&LOCK_BTS_RAX_RBX);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7t: `F0 REX.W 0F B3 /r` (LOCK BTR m64, r64). Same bit string addressing as LOCK BTS
// (S4-38L7n); the bit is cleared instead of set (SDM Vol.2 BTR, from memory, not re-checked
// against the SDM text). ----

/// `lock btr %rax, (%rbx)`.
const LOCK_BTR_RAX_RBX: [u8; 5] = [0xf0, 0x48, 0x0f, 0xb3, 0x03];
/// `lock btr %rax, 0x1234(%rip)`.
const LOCK_BTR_RAX_RIP: [u8; 9] = [0xf0, 0x48, 0x0f, 0xb3, 0x05, 0x34, 0x12, 0x00, 0x00];
/// `lock btr %rax, -0x883ed(%rip)`.
const LOCK_BTR_RIP_NEG: [u8; 9] = [0xf0, 0x48, 0x0f, 0xb3, 0x05, 0x13, 0x7c, 0xf7, 0xff];
/// The S4-38L7s diagnostic boundary: `lock btr %r12, 0x30(%r13)`.
const LOCK_BTR_BOUNDARY: [u8; 6] = [0xf0, 0x4d, 0x0f, 0xb3, 0x65, 0x30];

#[test]
fn lock_btr_clears_every_bit_of_the_first_qword_and_writes_it_back() {
    for offset in 0u64..64 {
        for old_bit in [0u64, 1] {
            let mut state = bts_state(offset);
            let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
            let neighbour = 0xa5a5_5a5a_dead_beefu64;
            memory.set_qword(0x20_0008, neighbour);
            memory.set_qword(0x1f_fff8, neighbour);
            let old = (u64::MAX & !(1u64 << offset)) | (old_bit << offset);
            memory.set_qword(0x20_0000, old);
            let mut expected = state.clone();
            let result = run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, 1, "offset {offset}");
            assert_eq!(memory.reads, vec![(0x20_0000, 8)], "offset {offset}");
            let new = old & !(1u64 << offset);
            assert_eq!(
                memory.writes,
                vec![(0x20_0000, new.to_le_bytes().to_vec())],
                "offset {offset}"
            );
            assert_eq!(memory.qword(0x20_0000), new);
            assert_eq!(memory.qword(0x20_0008), neighbour, "neighbour after");
            assert_eq!(memory.qword(0x1f_fff8), neighbour, "neighbour before");
            expected.rip += 5;
            expected.rflags = (expected.rflags & !FLAG_CF) | old_bit;
            assert_eq!(state, expected, "offset {offset}: only RIP and CF change");
        }
    }
}

#[test]
fn lock_btr_still_writes_when_the_bit_was_already_clear() {
    for (old, offset) in [(0u64, 5u64), (!(1u64 << 5), 5), (u64::MAX >> 1, 63), (1, 1)] {
        for initial_cf in [0, FLAG_CF] {
            let mut state = bts_state(offset);
            state.rflags = (state.rflags & !FLAG_CF) | initial_cf;
            let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
            memory.set_qword(0x20_0000, old);
            run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(
                state.rflags & FLAG_CF,
                0,
                "{old:#x} bit {offset}: old bit was 0"
            );
            assert_eq!(memory.qword(0x20_0000), old);
            // The value is unchanged but the write still happens.
            assert_eq!(
                memory.writes,
                vec![(0x20_0000, old.to_le_bytes().to_vec())],
                "{old:#x} bit {offset}"
            );
        }
    }
}

#[test]
fn lock_btr_leaves_every_flag_but_cf_unchanged() {
    for undefined in [0, FLAG_OF | FLAG_SF | FLAG_ZF | FLAG_AF | FLAG_PF] {
        for old in [0u64, 1 << 9] {
            let mut state = bts_state(9);
            state.rflags = (state.rflags & !ARITH) | undefined;
            let before = state.clone();
            let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
            memory.set_qword(0x20_0000, old);
            run_bts_set(&mut state, &mut memory).unwrap();
            let cf = u64::from(old != 0);
            assert_eq!(state.rflags & ARITH, undefined | cf);
            assert_eq!(state.rflags & !ARITH, before.rflags & !ARITH);
        }
    }
}

#[test]
fn lock_btr_register_offset_selects_the_qword_by_an_arithmetic_shift() {
    // (offset, byte delta of the qword that holds the bit, bit inside it).
    let cases: [(i64, i64, u32); 9] = [
        (-1, -8, 63),
        (-64, -8, 0),
        (-65, -16, 63),
        (-129, -24, 63),
        (64, 8, 0),
        (64 * 5 + 3, 40, 3),
        (64 * 5 + 63, 40, 63),
        (64 * 1000 + 7, 8000, 7),
        (-(64 * 1000) - 1, -8008, 63),
    ];
    for (offset, delta, bit) in cases {
        for old_bit in [0u64, 1] {
            let mut state = bts_state(offset as u64);
            state.gpr[Register::Rbx as usize] = 0x40_0000;
            let at = (0x40_0000i64 + delta) as u64;
            let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
            let old = (0x0123_4567_89ab_cdefu64 & !(1u64 << bit)) | (old_bit << bit);
            memory.set_qword(at, old);
            let mut expected = state.clone();
            let result = run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "offset {offset}");
            assert_eq!(memory.reads, vec![(at, 8)], "offset {offset}");
            assert_eq!(
                memory.writes,
                vec![(at, (old & !(1u64 << bit)).to_le_bytes().to_vec())],
                "offset {offset}"
            );
            expected.rip += 5;
            expected.rflags = (expected.rflags & !FLAG_CF) | old_bit;
            assert_eq!(state, expected, "offset {offset}");
            if delta != 0 {
                assert_eq!(memory.qword(0x40_0000), 0, "offset {offset}");
            }
        }
    }
}

#[test]
fn lock_btr_with_every_extended_source_register_r8_to_r15() {
    // `lock btr %rN, (%rbx)`: REX = 0x4c (W + R), reg = N & 7, modrm = 0x03 | (reg << 3).
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let code = [0xf0, 0x4c, 0x0f, 0xb3, 0x03 | ((i as u8) << 3)];
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x30_0000;
        state.gpr[register as usize] = 64 + 10 + i as u64;
        // RAX must not be used as the offset by mistake.
        state.gpr[Register::Rax as usize] = 3;
        let mut memory = BitsMemory::new(&code);
        memory.set_qword(0x30_0008, u64::MAX);
        run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(0x30_0008, 8)], "source r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(0x30_0008, (!(1u64 << (10 + i))).to_le_bytes().to_vec())],
            "source r{}",
            i + 8
        );
        assert_eq!(state.rflags & FLAG_CF, FLAG_CF, "old bit was 1");
        assert_eq!(state.rip, RIP + 5);
    }
}

#[test]
fn lock_btr_with_every_extended_base_register_r8_to_r15() {
    // `lock btr %rax, 0x0(%rN)` via REX.B: mod = 01 with disp8 0; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x49, 0x0f, 0xb3];
        if i == 4 {
            code.extend([0x44, 0x24, 0x00]);
        } else {
            code.extend([0x40 | i as u8, 0x00]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[register as usize] = base;
        state.gpr[Register::Rax as usize] = 64 * 2 + 1;
        let mut memory = BitsMemory::new(&code);
        memory.set_qword(base + 16, 7);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base + 16, 8)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(base + 16, 5u64.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rflags & FLAG_CF, FLAG_CF);
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_btr_diagnostic_boundary_form_uses_r12_as_offset_and_r13_plus_disp_as_base() {
    // `lock btr %r12, 0x30(%r13)` with r12 = 0x191 (= 6 * 64 + 17: bit 17 of the seventh qword)
    // and r13 a canonical kernel address.
    let r13 = 0xffff_8880_0123_4000u64;
    let mut state = board_state();
    state.gpr[Register::R12 as usize] = 0x191;
    state.gpr[Register::R13 as usize] = r13;
    // RAX/RBX must not be used by mistake.
    state.gpr[Register::Rax as usize] = 1;
    state.gpr[Register::Rbx as usize] = 2;
    let at = r13 + 0x30 + 6 * 8;
    let mut memory = BitsMemory::new(&LOCK_BTR_BOUNDARY);
    memory.set_qword(at, u64::MAX);
    memory.set_qword(at - 8, 0x1111);
    memory.set_qword(at + 8, 0x2222);
    let mut expected = state.clone();
    let result = run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(
        memory.writes,
        vec![(at, (!(1u64 << 17)).to_le_bytes().to_vec())]
    );
    assert_eq!(memory.qword(at - 8), 0x1111);
    assert_eq!(memory.qword(at + 8), 0x2222);
    expected.rip += 6;
    expected.rflags |= FLAG_CF;
    assert_eq!(state, expected);
}

#[test]
fn lock_btr_with_rip_relative_and_negative_displacement_addresses() {
    // The RIP-relative address is next RIP (all 9 bytes) + disp32, plus the bit string delta.
    let mut state = bts_state(64 * 3 + 2);
    let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RIP);
    let at = RIP + 9 + 0x1234 + 24;
    memory.set_qword(at, 0xff);
    run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0xfbu64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
    assert_eq!(state.rflags & FLAG_CF, FLAG_CF);
    // The `-0x883ed(%rip)` form: next RIP - 0x883ed wraps into the canonical high half, and the
    // bit string delta of offset 70 is one qword.
    let mut state = bts_state(70);
    let mut memory = BitsMemory::new(&LOCK_BTR_RIP_NEG);
    let at = (RIP + 9).wrapping_sub(0x883ed) + 8;
    memory.set_qword(at, 0x7f);
    run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x3fu64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 9);
}

#[test]
fn lock_btr_with_sib_base_index_scale_and_displacement() {
    // `lock btr %rax, 0x10(%rbx,%rcx,4)` = f0 48 0f b3 44 8b 10.
    let mut state = bts_state(64 * 2 + 5);
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut memory = BitsMemory::new(&[0xf0, 0x48, 0x0f, 0xb3, 0x44, 0x8b, 0x10]);
    let at = 0x50_0000 + 12 + 0x10 + 16;
    memory.set_qword(at, 0x3f);
    run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.writes, vec![(at, 0x1fu64.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 7);
}

#[test]
fn lock_btr_non_canonical_address_is_gp0_without_mutation_or_access() {
    for (rbx, offset) in [
        (0x0000_8000_0000_0000u64, 0u64),
        (0xffff_7fff_ffff_ffff, 0),
        (0x0000_7fff_ffff_fffc, 0),
        (0x0000_7fff_ffff_fff8, 64),
        (0xffff_8000_0000_0000, (-65i64) as u64),
        (0x20_0000, i64::MIN as u64),
        (0x20_0000, i64::MAX as u64),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = offset;
        state.gpr[Register::Rbx as usize] = rbx;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x} {offset:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x} {offset:#x}");
        assert!(memory.reads.is_empty(), "{rbx:#x} {offset:#x}");
        assert!(memory.writes.is_empty(), "{rbx:#x} {offset:#x}");
    }
}

#[test]
fn lock_btr_through_an_rsp_or_rbp_base_is_ss0() {
    // `lock btr %rax, (%rsp)` = f0 48 0f b3 04 24 and `lock btr %rax, 0x0(%rbp)` = f0 48 0f b3 45 00.
    for (register, code) in [
        (Register::Rsp, &[0xf0, 0x48, 0x0f, 0xb3, 0x04, 0x24][..]),
        (Register::Rbp, &[0xf0, 0x48, 0x0f, 0xb3, 0x45, 0x00][..]),
    ] {
        for (base, offset) in [
            (0x0000_8000_0000_0000u64, 0u64),
            (0x0000_7fff_ffff_fff8, 64),
        ] {
            let mut state = board_state();
            state.gpr[register as usize] = base;
            state.gpr[Register::Rax as usize] = offset;
            let before = state.clone();
            let mut memory = BitsMemory::new(code);
            let result = run_bts_set(&mut state, &mut memory).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::Fault(ArchitecturalFault {
                    vector: 12,
                    error_code: Some(0),
                    linear_address: None,
                }),
                "{register:?} {base:#x}"
            );
            assert_eq!(state, before);
            assert!(memory.reads.is_empty() && memory.writes.is_empty());
        }
    }
}

#[test]
fn lock_btr_write_fault_leaves_cf_rip_registers_and_memory_unchanged() {
    // A read-only page: the always-performed write faults, even when the bit was already clear.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0008),
    };
    for (old, cf) in [(0u64, FLAG_CF), (1 << 3, 0), (1 << 3, FLAG_CF)] {
        let mut state = bts_state(64 + 3);
        state.rflags = (state.rflags & !ARITH) | cf | FLAG_ZF | FLAG_OF;
        let before = state.clone();
        let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
        memory.set_qword(0x20_0008, old);
        memory.write_fault = Some(fault.clone());
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.qword(0x20_0008), old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_btr_forms_outside_the_slice_are_refused_with_the_exact_opcode_without_mutation() {
    let cases: [(&[u8], u8); 22] = [
        // No REX.W (dword), REX without W, REX.R without W.
        (&[0xf0, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x40, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x44, 0x0f, 0xb3, 0x03], 0xb3),
        // Register forms (mod = 11).
        (&[0xf0, 0x48, 0x0f, 0xb3, 0xc3], 0xb3),
        (&[0xf0, 0x4d, 0x0f, 0xb3, 0xc8], 0xb3),
        // 0x66 and 0x67.
        (&[0x66, 0xf0, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x66, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x67, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        (&[0xf3, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf2, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0xf3, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf3, 0xf0, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0xf0, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        // Segment prefixes.
        (&[0xf0, 0x64, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x65, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0x65, 0xf0, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        (&[0xf0, 0x2e, 0x48, 0x0f, 0xb3, 0x03], 0xb3),
        // REX not directly before the opcode loses its W bit.
        (&[0xf0, 0x48, 0x66, 0x0f, 0xb3, 0x03], 0xb3),
        // The immediate form (`0F BA /6 ib`) and the neighbouring LOCK bit-test forms.
        (&[0xf0, 0x48, 0x0f, 0xba, 0x33, 0x05], 0xba),
        (&[0xf0, 0x48, 0x0f, 0xa3, 0x03], 0xa3),
        (&[0xf0, 0x48, 0x0f, 0xbb, 0x03], 0xbb),
        (&[0xf0, 0x4c, 0x0f, 0xbb, 0x03], 0xbb),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn plain_btr_without_lock_keeps_its_existing_path_in_the_set() {
    // S4-38L7t does not narrow the non-LOCK BTR (`Operation::Bit`): register and immediate offsets
    // keep executing as before.
    let mut state = bts_state(64 + 4);
    let mut memory = BitsMemory::new(&[0x48, 0x0f, 0xb3, 0x03]);
    memory.set_qword(0x20_0008, 0xff);
    let result = run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.qword(0x20_0008), 0xef);
    assert_eq!(state.rip, RIP + 4);
    let mut state = bts_state(0);
    let mut memory = BitsMemory::new(&[0x48, 0x0f, 0xba, 0x33, 0x07]);
    memory.set_qword(0x20_0000, 0xff);
    let result = run_bts_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.qword(0x20_0000), 0x7f);
}

#[test]
fn lock_btr_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK form is refused without mutation.
    for code in [&LOCK_BTR_RAX_RBX[..], &LOCK_BTR_BOUNDARY[..]] {
        let mut state = legacy_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xb3 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_btr_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&LOCK_BTR_RAX_RBX);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7u: `MOV r/m16/32/64, Sreg` (`[REX] 8C /r`, Sreg 0..=5 = ES, CS, SS, DS, FS, GS; SDM
// Vol.2 MOV: a register destination receives the zero-extended selector, a memory destination
// receives 16 bits whatever the operand size; no flags; from memory, not re-checked against the SDM
// text). The selector is the visible value held in the state; the model does not check it against a
// descriptor table. ----

const SREG_CS: u16 = 0x10;
const SREG_SS: u16 = 0x20;
const SREG_DS: u16 = 0x28;
const SREG_ES: u16 = 0x30;
const SREG_FS: u16 = 0x38;
const SREG_GS: u16 = 0x40;

fn ms_state() -> LongModeState {
    let mut state = board_state();
    state.cs_selector = Some(SREG_CS);
    state.ss_selector = Some(SREG_SS);
    state.ds_selector = SREG_DS;
    state.es_selector = SREG_ES;
    state.fs_selector = SREG_FS;
    state.gs_selector = SREG_GS;
    for (index, register) in state.gpr.iter_mut().enumerate() {
        *register = 0xdead_beef_0000_0000 | (index as u64 + 1) << 8;
    }
    state.rflags |= FLAG_CF | FLAG_ZF | FLAG_OF;
    state
}

fn run_ms(state: &mut LongModeState, code: &[u8]) -> (Cpl0ProfileSetResult, BitsMemory) {
    let mut memory = BitsMemory::new(code);
    let result = run_bts_set(state, &mut memory).unwrap();
    (result, memory)
}

#[test]
fn mov_sreg_register_forms_zero_extend_the_selector_into_the_destination() {
    // (bytes, destination register, selector)
    let cases: [(&[u8], Register, u16); 20] = [
        // The S4-38L7t diagnostic boundary: `mov %ss, %eax`.
        (&[0x8c, 0xd0], Register::Rax, SREG_SS),
        (&[0x8c, 0xd8], Register::Rax, SREG_DS),
        (&[0x8c, 0xc0], Register::Rax, SREG_ES),
        (&[0x8c, 0xc8], Register::Rax, SREG_CS),
        (&[0x8c, 0xe0], Register::Rax, SREG_FS),
        (&[0x8c, 0xe8], Register::Rax, SREG_GS),
        (&[0x8c, 0xd3], Register::Rbx, SREG_SS),
        (&[0x8c, 0xd9], Register::Rcx, SREG_DS),
        // REX.W: a 64-bit destination (the exact `48 8C E0|E8` forms keep their own operations).
        (&[0x48, 0x8c, 0xd8], Register::Rax, SREG_DS),
        (&[0x48, 0x8c, 0xd0], Register::Rax, SREG_SS),
        (&[0x48, 0x8c, 0xc0], Register::Rax, SREG_ES),
        (&[0x48, 0x8c, 0xe0], Register::Rax, SREG_FS),
        (&[0x48, 0x8c, 0xe8], Register::Rax, SREG_GS),
        (&[0x48, 0x8c, 0xe1], Register::Rcx, SREG_FS),
        // REX.B selects r8..r15; a bare REX and REX.R/REX.X do not change the Sreg.
        (&[0x41, 0x8c, 0xd0], Register::R8, SREG_SS),
        (&[0x49, 0x8c, 0xd8], Register::R8, SREG_DS),
        (&[0x41, 0x8c, 0xd7], Register::R15, SREG_SS),
        (&[0x40, 0x8c, 0xd0], Register::Rax, SREG_SS),
        (&[0x44, 0x8c, 0xd0], Register::Rax, SREG_SS),
        (&[0x42, 0x8c, 0xd0], Register::Rax, SREG_SS),
    ];
    for (code, destination, selector) in cases {
        let mut state = ms_state();
        let mut expected = state.clone();
        let (result, memory) = run_ms(&mut state, code);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        // The selector is zero-extended: with or without REX.W every bit above bit 15 is clear.
        expected.gpr[destination as usize] = u64::from(selector);
        expected.rip += code.len() as u64;
        assert_eq!(state, expected, "{code:02x?}");
        assert!(
            memory.reads.is_empty() && memory.writes.is_empty(),
            "{code:02x?}"
        );
    }
}

#[test]
fn mov_sreg_register_form_leaves_flags_and_definedness_alone() {
    let mut state = ms_state();
    state.rflags |= FLAG_AF | FLAG_PF | FLAG_SF;
    let flags = state.rflags;
    let (result, _) = run_ms(&mut state, &[0x8c, 0xd0]);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rflags, flags);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
}

#[test]
fn mov_sreg_memory_forms_write_two_bytes_at_the_effective_address() {
    // (bytes, selector, address); address 0 means "RIP-relative, computed from the displacement".
    let cases: [(&[u8], u16, u64); 8] = [
        // `mov %ds, (%rbx)`; neighbours stay untouched.
        (&[0x8c, 0x1b], SREG_DS, 0x20_0000),
        // `mov %ss, 0x10(%rbx)`, `mov %cs, (%rbx)`.
        (&[0x8c, 0x53, 0x10], SREG_SS, 0x20_0010),
        (&[0x8c, 0x0b], SREG_CS, 0x20_0000),
        // REX.W does not widen the store; REX.B selects r8 as the base.
        (&[0x48, 0x8c, 0x1b], SREG_DS, 0x20_0000),
        (&[0x49, 0x8c, 0x18], SREG_DS, 0x30_0000),
        // RIP-relative (the next RIP is after the displacement).
        (&[0x8c, 0x05, 0x00, 0xf0, 0xff, 0xff], SREG_ES, 0),
        (&[0x8c, 0x25, 0x10, 0x00, 0x00, 0x00], SREG_FS, 0),
        (&[0x8c, 0x2d, 0x20, 0x00, 0x00, 0x00], SREG_GS, 0),
    ];
    for (code, selector, address) in cases {
        let address = if address == 0 {
            let disp = i32::from_le_bytes(code[2..6].try_into().unwrap());
            (RIP + 6).wrapping_add(disp as i64 as u64)
        } else {
            address
        };
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gpr[Register::R8 as usize] = 0x30_0000;
        let mut expected = state.clone();
        let mut memory = BitsMemory::new(code);
        memory.data.insert(address - 1, 0xa5);
        memory.data.insert(address + 2, 0xa5);
        memory.data.insert(address, 0xff);
        memory.data.insert(address + 1, 0xff);
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(
            memory.writes,
            vec![(address, selector.to_le_bytes().to_vec())],
            "{code:02x?}"
        );
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert_eq!(memory.data[&(address - 1)], 0xa5, "{code:02x?}");
        assert_eq!(memory.data[&(address + 2)], 0xa5, "{code:02x?}");
        expected.rip += code.len() as u64;
        assert_eq!(state, expected, "{code:02x?}");
    }
}

#[test]
fn mov_sreg_memory_form_with_fs_or_gs_prefix_adds_the_segment_base() {
    let gs_base = 0xffff_8880_0000_0000u64;
    let fs_base = 0xffff_8880_4000_0000u64;
    // `mov %es, %gs:(%rbx)`, `mov %ss, %fs:(%rbx)`, `mov %ds, %gs:0x1234(%rip)`,
    // `mov %cs, %fs:(%rbx)` with REX.W, `mov %gs, %gs:0x10(%rbx)`.
    let cases: [(&[u8], u16, u64); 5] = [
        (&[0x65, 0x8c, 0x03], SREG_ES, gs_base + 0x20_0000),
        (&[0x64, 0x8c, 0x13], SREG_SS, fs_base + 0x20_0000),
        (
            &[0x65, 0x8c, 0x1d, 0x34, 0x12, 0x00, 0x00],
            SREG_DS,
            gs_base + RIP + 7 + 0x1234,
        ),
        (&[0x64, 0x48, 0x8c, 0x0b], SREG_CS, fs_base + 0x20_0000),
        (&[0x65, 0x8c, 0x6b, 0x10], SREG_GS, gs_base + 0x20_0010),
    ];
    for (code, selector, address) in cases {
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = gs_base;
        state.fs_base = fs_base;
        let mut expected = state.clone();
        let (result, memory) = run_ms(&mut state, code);
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(
            memory.writes,
            vec![(address, selector.to_le_bytes().to_vec())],
            "{code:02x?}"
        );
        assert!(memory.reads.is_empty(), "{code:02x?}");
        expected.rip += code.len() as u64;
        assert_eq!(state, expected, "{code:02x?}");
    }
    // Without the prefix the base is not added (the flat DS path): the base values are ignored.
    let mut state = ms_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gs_base = gs_base;
    state.fs_base = fs_base;
    let (_, memory) = run_ms(&mut state, &[0x8c, 0x03]);
    assert_eq!(
        memory.writes,
        vec![(0x20_0000, SREG_ES.to_le_bytes().to_vec())]
    );
}

#[test]
fn mov_sreg_memory_store_is_gp0_when_the_start_or_last_byte_is_not_canonical() {
    // (bytes, RBX, GS base)
    for (code, rbx, gs_base) in [
        (&[0x8c, 0x1b][..], 0x0000_8000_0000_0000u64, 0u64),
        (&[0x8c, 0x1b][..], 0xffff_7fff_ffff_ffff, 0),
        // The start is canonical but the second byte is not.
        (&[0x8c, 0x1b][..], 0x0000_7fff_ffff_ffff, 0),
        (&[0x65, 0x8c, 0x1b][..], 0x0000_7fff_ffff_ffff, 0),
        // GS base plus offset leaves the canonical range.
        (&[0x65, 0x8c, 0x1b][..], 0x0000_8000_0000_0000, 0),
        (&[0x65, 0x8c, 0x1b][..], 0x20_0000, 0x0000_8000_0000_0000),
    ] {
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = rbx;
        state.gs_base = gs_base;
        let before = state.clone();
        let (result, memory) = run_ms(&mut state, code);
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?} {rbx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{code:02x?} {rbx:#x}");
        assert!(memory.writes.is_empty(), "{code:02x?} {rbx:#x}");
    }
}

#[test]
fn mov_sreg_memory_store_through_rsp_or_rbp_is_ss0_without_a_prefix_and_gp0_with_gs() {
    let ss0 = ArchitecturalFault {
        vector: 12,
        error_code: Some(0),
        linear_address: None,
    };
    // `mov %ds, (%rsp)` and `mov %ds, 0x0(%rbp)`, plain and with a GS prefix.
    for (register, code, prefixed) in [
        (
            Register::Rsp,
            &[0x8c, 0x1c, 0x24][..],
            &[0x65, 0x8c, 0x1c, 0x24][..],
        ),
        (
            Register::Rbp,
            &[0x8c, 0x5d, 0x00][..],
            &[0x65, 0x8c, 0x5d, 0x00][..],
        ),
    ] {
        for base in [0x0000_8000_0000_0000u64, 0x0000_7fff_ffff_ffff] {
            let mut state = ms_state();
            state.gpr[register as usize] = base;
            let before = state.clone();
            let (result, memory) = run_ms(&mut state, code);
            assert_eq!(
                exit(&result),
                &RunExit::Fault(ss0.clone()),
                "{register:?} {base:#x}"
            );
            assert_eq!(state, before);
            assert!(memory.writes.is_empty());
            // With a GS prefix the segment is GS, not SS: the fault is #GP(0).
            let (result, memory) = run_ms(&mut state, prefixed);
            assert_eq!(
                exit(&result),
                &RunExit::Fault(GP0),
                "{register:?} {base:#x}"
            );
            assert_eq!(state, before);
            assert!(memory.writes.is_empty());
        }
    }
}

#[test]
fn mov_sreg_write_fault_leaves_registers_rip_and_memory_unchanged() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0000),
    };
    for code in [&[0x8c, 0x1b][..], &[0x48, 0x8c, 0x1b][..]] {
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        memory.write_fault = Some(fault.clone());
        let result = run_bts_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty() && memory.data.is_empty());
    }
}

#[test]
fn mov_sreg_forms_outside_the_slice_are_refused_with_the_exact_opcode_without_mutation() {
    let cases: [&[u8]; 28] = [
        // Reserved Sreg encodings 6 and 7, register and memory forms.
        &[0x8c, 0xf0],
        &[0x8c, 0xf8],
        &[0x8c, 0x33],
        &[0x8c, 0x3b],
        &[0x48, 0x8c, 0xf0],
        &[0x65, 0x8c, 0x33],
        // LOCK, REP and REPNE.
        &[0xf0, 0x8c, 0xd0],
        &[0xf0, 0x8c, 0x1b],
        &[0xf3, 0x8c, 0xd0],
        &[0xf2, 0x8c, 0xd0],
        &[0xf3, 0x8c, 0x1b],
        &[0xf0, 0x48, 0x8c, 0xd0],
        // 0x66 (16-bit register form) and 0x67 are refused conservatively.
        &[0x66, 0x8c, 0xd0],
        &[0x66, 0x48, 0x8c, 0xd0],
        &[0x66, 0x8c, 0x1b],
        &[0x67, 0x8c, 0x1b],
        &[0x67, 0x8c, 0xd0],
        // A segment prefix on a register form, a non-FS/GS prefix on a memory form, and two
        // segment prefixes.
        &[0x65, 0x8c, 0xd0],
        &[0x64, 0x48, 0x8c, 0xd0],
        &[0x26, 0x8c, 0x1b],
        &[0x2e, 0x8c, 0x1b],
        &[0x36, 0x8c, 0x1b],
        &[0x3e, 0x8c, 0x1b],
        &[0x64, 0x65, 0x8c, 0x1b],
        &[0x65, 0x65, 0x8c, 0x1b],
        // A REX that is not immediately before the opcode.
        &[0x48, 0x66, 0x8c, 0xd0],
        &[0x41, 0x65, 0x8c, 0x03],
        &[0x48, 0xf3, 0x8c, 0xd0],
    ];
    for code in cases {
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let (result, memory) = run_ms(&mut state, code);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8c },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(
            memory.reads.is_empty() && memory.writes.is_empty(),
            "{code:02x?}"
        );
    }
}

#[test]
fn mov_sreg_with_an_unknown_visible_selector_is_refused_without_mutation() {
    // CS and SS selectors are optional model state: None is not guessed.
    for code in [&[0x8c, 0xd0][..], &[0x8c, 0xc8][..], &[0x8c, 0x13][..]] {
        let mut state = ms_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.cs_selector = None;
        state.ss_selector = None;
        let before = state.clone();
        let (result, memory) = run_ms(&mut state, code);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8c },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
    // `mov %ds, (%rbx)` does not depend on CS/SS.
    let mut state = ms_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.cs_selector = None;
    state.ss_selector = None;
    let (result, memory) = run_ms(&mut state, &[0x8c, 0x1b]);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.writes.len(), 1);
}

#[test]
fn mov_sreg_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the forms are refused without mutation.
    for code in [
        &[0x8c, 0xd0][..],
        &[0x48, 0x8c, 0xd8][..],
        &[0x8c, 0x1b][..],
        &[0x65, 0x8c, 0x03][..],
    ] {
        let mut state = legacy_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.ss_selector = Some(SREG_SS);
        state.cs_selector = Some(SREG_CS);
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0x8c },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn mov_sreg_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = ms_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&[0x8c, 0xd0]);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn mov_sreg_two_stores_retire_in_order() {
    let mut state = ms_state();
    let mut memory = BitsMemory::new(&[0x8c, 0xd0, 0x8c, 0xd9]);
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
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[Register::Rax as usize], u64::from(SREG_SS));
    assert_eq!(state.gpr[Register::Rcx as usize], u64::from(SREG_DS));
    assert_eq!(state.rip, RIP + 4);
}

// ---- S4-38L7o: FS/GS-based `TEST r/m8, imm8` (`[64|65] [REX] F6 /0 ib`, memory operand; SDM
// Vol.2 TEST: the byte is ANDed with the immediate, the result is discarded, CF = OF = 0, SF/ZF/PF
// from the byte result, AF undefined and left unchanged here; Vol.3 3.4.4 FS/GS base; from memory,
// not re-checked against the SDM text). Nothing is written to memory or to a register. ----

/// `testb $0xf0, %gs:0x2f0692c(%rip)` (the S4-38L7n diagnostic boundary bytes, 8 bytes long: the
/// next RIP is after the immediate).
const GS_TEST8_BOUNDARY: [u8; 8] = [0x65, 0xf6, 0x05, 0x2c, 0x69, 0xf0, 0x02, 0xf0];
/// `testb $0x0e, %fs:0x1234(%rip)`.
const FS_TEST8_RIP: [u8; 8] = [0x64, 0xf6, 0x05, 0x34, 0x12, 0x00, 0x00, 0x0e];

fn run_test8_set(
    state: &mut LongModeState,
    memory: &mut BitsMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_bts_set(state, memory)
}

const TEST8_FLAGS: u64 = FLAG_CF | FLAG_PF | FLAG_ZF | FLAG_SF | FLAG_OF;

#[test]
fn gs_test_byte_boundary_form_reads_one_byte_at_gs_base_plus_next_rip_and_changes_only_flags() {
    let gs_base = 0xffff_8880_0000_0000u64;
    let mut state = board_state();
    state.gs_base = gs_base;
    state.fs_base = 0xffff_8880_9999_0000;
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_AF;
    let mut expected = state.clone();
    let at = gs_base + RIP + 8 + 0x02f0_692c;
    let mut memory = BitsMemory::new(&GS_TEST8_BOUNDARY);
    // 0x8f & 0xf0 = 0x80: SF = 1, ZF = 0, PF = 0 (one bit), CF = OF = 0, AF as before.
    memory.data.insert(at, 0x8f);
    // The neighbouring byte must stay unread.
    memory.data.insert(at + 1, 0xff);
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 1)]);
    assert!(memory.writes.is_empty());
    // As the plain TEST: CF/OF/SF/ZF/PF become defined, AF is undefined.
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !FLAG_AF);
    expected.rflags = (expected.rflags & !TEST8_FLAGS) | FLAG_SF;
    expected.rip += 8;
    assert_eq!(state, expected, "only RFLAGS and RIP change");
    assert_ne!(state.rflags & FLAG_AF, 0, "AF is kept");
}

#[test]
fn fs_test_byte_rip_relative_reads_at_fs_base_plus_effective_address() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let at = fs_base + RIP + 8 + 0x1234;
    let mut memory = BitsMemory::new(&FS_TEST8_RIP);
    memory.data.insert(at, 0xf1);
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 1)]);
    assert!(memory.writes.is_empty());
    // 0xf1 & 0x0e = 0: ZF = 1, PF = 1, SF = 0.
    expected.rflags |= FLAG_ZF | FLAG_PF;
    expected.rip += 8;
    assert_eq!(state, expected);
}

#[test]
fn test_byte_with_a_zero_base_uses_the_effective_address() {
    for code in [GS_TEST8_BOUNDARY, FS_TEST8_RIP] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = BitsMemory::new(&code);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        let disp = if code[0] == 0x65 { 0x02f0_692c } else { 0x1234 };
        assert_eq!(memory.reads, vec![(RIP + 8 + disp, 1)]);
    }
}

#[test]
fn test_byte_flags_follow_the_byte_result_on_boundary_values() {
    // (memory byte, immediate, expected ZF/SF/PF bits). CF and OF are always cleared, and the
    // result is the byte (not widened): 0x80 sets SF, only the low eight bits count for PF.
    let cases: [(u8, u8, u64); 14] = [
        (0x00, 0xff, FLAG_ZF | FLAG_PF),
        (0xff, 0x00, FLAG_ZF | FLAG_PF),
        (0x01, 0x01, 0),
        (0x03, 0x03, FLAG_PF),
        (0x80, 0x80, FLAG_SF),
        (0xf0, 0xf0, FLAG_SF | FLAG_PF),
        (0xff, 0xff, FLAG_SF | FLAG_PF),
        (0x7f, 0xff, 0),
        (0x0f, 0xf0, FLAG_ZF | FLAG_PF),
        (0xaa, 0x55, FLAG_ZF | FLAG_PF),
        (0xfe, 0x01, FLAG_ZF | FLAG_PF),
        (0x81, 0x81, FLAG_SF | FLAG_PF),
        (0x7e, 0x7e, FLAG_PF),
        (0x07, 0xff, 0),
    ];
    for (value, immediate, expected_flags) in cases {
        for initial in [
            0u64,
            TEST8_FLAGS,
            FLAG_CF | FLAG_OF,
            FLAG_AF,
            TEST8_FLAGS | FLAG_AF,
        ] {
            let mut code = GS_TEST8_BOUNDARY;
            code[7] = immediate;
            let mut state = board_state();
            state.gs_base = 0xffff_8880_0000_0000;
            state.rflags = 2 | IF | initial;
            let mut expected = state.clone();
            let at = 0xffff_8880_0000_0000 + RIP + 8 + 0x02f0_692c;
            let mut memory = BitsMemory::new(&code);
            memory.data.insert(at, value);
            let result = run_test8_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{value:#x} {immediate:#x}");
            assert_eq!(
                state.rflags & (FLAG_CF | FLAG_OF),
                0,
                "{value:#x} {immediate:#x}"
            );
            assert_eq!(
                state.rflags & TEST8_FLAGS,
                expected_flags,
                "{value:#x} & {immediate:#x} from {initial:#x}"
            );
            expected.rflags = (expected.rflags & !TEST8_FLAGS) | expected_flags;
            expected.rip += 8;
            assert_eq!(state, expected, "{value:#x} {immediate:#x} {initial:#x}");
            assert!(memory.writes.is_empty());
        }
    }
}

#[test]
fn test_byte_matches_the_non_prefixed_test_on_the_same_byte() {
    // `testb $imm, (%rbx)` (f6 03 ib) has the same flag result as the GS form on every value.
    for value in [0u8, 1, 2, 3, 0x7f, 0x80, 0x81, 0xf0, 0xff] {
        for immediate in [0u8, 1, 0x80, 0xf0, 0xff, 0x55] {
            let mut plain_state = bts_state(0);
            plain_state.rflags |= FLAG_CF | FLAG_OF;
            let mut plain_memory = BitsMemory::new(&[0xf6, 0x03, immediate]);
            plain_memory.data.insert(0x20_0000, value);
            run_test8_set(&mut plain_state, &mut plain_memory).unwrap();

            let mut state = bts_state(0);
            state.rflags |= FLAG_CF | FLAG_OF;
            state.gs_base = 0xffff_8880_0000_0000;
            let mut memory = BitsMemory::new(&[0x65, 0xf6, 0x03, immediate]);
            memory.data.insert(0xffff_8880_0020_0000, value);
            run_test8_set(&mut state, &mut memory).unwrap();
            assert_eq!(
                state.rflags, plain_state.rflags,
                "{value:#x} {immediate:#x}"
            );
            assert_eq!(memory.reads, vec![(0xffff_8880_0020_0000, 1)]);
        }
    }
}

#[test]
fn gs_test_byte_with_base_register_index_scale_and_displacement() {
    // `testb $0x40, %gs:0x10(%rbx,%rcx,4)` = 65 f6 44 8b 10 40.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut expected = state.clone();
    let at = 0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10;
    let mut memory = BitsMemory::new(&[0x65, 0xf6, 0x44, 0x8b, 0x10, 0x40]);
    memory.data.insert(at, 0x40);
    run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 1)]);
    assert!(memory.writes.is_empty());
    // 0x40 & 0x40 = 0x40: not zero, not negative, one bit set (PF = 0).
    expected.rflags &= !TEST8_FLAGS;
    expected.rip += 6;
    assert_eq!(state, expected);
}

#[test]
fn fs_test_byte_absolute_sib_disp32_without_base_or_index_adds_only_the_fs_base() {
    // `testb $0x01, %fs:0x20` = 64 f6 04 25 20 00 00 00 01.
    let mut state = board_state();
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = BitsMemory::new(&[0x64, 0xf6, 0x04, 0x25, 0x20, 0, 0, 0, 0x01]);
    memory.data.insert(0xffff_8880_0000_0020, 0x01);
    run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_0020, 1)]);
    assert_eq!(state.rip, RIP + 9);
    assert_eq!(state.rflags & TEST8_FLAGS, 0);
}

#[test]
fn test_byte_with_rex_b_uses_r8_to_r15_as_the_base_and_changes_no_register() {
    // `testb $0x80, %gs:0x8(%r8..%r15)`: 65 41 f6 40+n 08 80 (REX.B, disp8).
    for (n, register) in [
        (0u8, Register::R8),
        (1, Register::R9),
        (7, Register::R15),
        (5, Register::R13),
    ] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.gpr[register as usize] = 0x4000;
        let mut expected = state.clone();
        let mut memory = BitsMemory::new(&[0x65, 0x41, 0xf6, 0x40 + n, 0x08, 0x80]);
        let at = 0xffff_8880_0000_4008;
        memory.data.insert(at, 0x80);
        run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(at, 1)], "{register:?}");
        assert!(memory.writes.is_empty());
        expected.rflags = (expected.rflags & !TEST8_FLAGS) | FLAG_SF;
        expected.rip += 6;
        assert_eq!(state, expected, "{register:?}");
    }
    // R12 through a SIB byte: `testb $0x80, %gs:0x8(%r12)` = 65 41 f6 44 24 08 80.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::R12 as usize] = 0x4000;
    let mut memory = BitsMemory::new(&[0x65, 0x41, 0xf6, 0x44, 0x24, 0x08, 0x80]);
    memory.data.insert(0xffff_8880_0000_4008, 0x80);
    run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_4008, 1)]);
    assert_eq!(state.rip, RIP + 7);
}

#[test]
fn test_byte_accepts_a_rex_directly_before_the_opcode_with_any_w_r_bits() {
    // REX.W and REX.R do not change the byte operand; the rest of the instruction is identical.
    for rex in [0x40u8, 0x44, 0x48, 0x4c] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(&[0x65, rex, 0xf6, 0x03, 0xff]);
        memory.data.insert(0xffff_8880_0020_0000, 0x00);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{rex:#x}");
        assert_eq!(memory.reads, vec![(0xffff_8880_0020_0000, 1)], "{rex:#x}");
        assert_eq!(state.rip, RIP + 5, "{rex:#x}");
        assert_eq!(state.gpr, before.gpr, "{rex:#x}");
        assert_eq!(state.rflags & TEST8_FLAGS, FLAG_ZF | FLAG_PF, "{rex:#x}");
    }
}

#[test]
fn test_byte_never_writes_memory_even_on_a_read_only_page() {
    // The adapter fails every write; the TEST must still retire.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0000),
    };
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = BitsMemory::new(&[0x65, 0xf6, 0x03, 0x01]);
    memory.write_fault = Some(fault);
    memory.data.insert(0x20_0000, 0x55);
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.data.get(&0x20_0000), Some(&0x55));
}

#[test]
fn test_byte_non_canonical_address_is_gp0_without_mutation_or_read() {
    for (gs, base) in [
        (true, 0x0000_8000_0000_0000u64),
        (true, 0x7fff_ffff_ffff_fff0),
        // The effective address itself lands one past the canonical boundary.
        (true, 0x0000_8000_0000_0000 - 0x02f0_692c - 8 - RIP),
        (false, 0x0000_8000_0000_0000),
    ] {
        let mut state = board_state();
        state.gs_base = base;
        state.fs_base = base;
        state.rflags |= FLAG_CF | FLAG_ZF;
        let before = state.clone();
        let code = if gs { GS_TEST8_BOUNDARY } else { FS_TEST8_RIP };
        let mut memory = BitsMemory::new(&code);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{gs} {base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{gs} {base:#x}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn test_byte_last_canonical_address_is_read() {
    // A single byte is read, so the last canonical byte 0x7fff_ffff_ffff is accessible.
    let mut state = board_state();
    state.gs_base = 0x0000_7fff_ffff_ffff - 0x02f0_692c - 8 - RIP;
    let mut memory = BitsMemory::new(&GS_TEST8_BOUNDARY);
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x0000_7fff_ffff_ffff, 1)]);
}

#[test]
fn gs_test_byte_through_rsp_or_rbp_base_is_gp0_not_ss_when_non_canonical() {
    // `testb $1, %gs:(%rsp)` = 65 f6 04 24 01 and `testb $1, %gs:0x0(%rbp)` = 65 f6 45 00 01.
    for (code, register) in [
        (&[0x65u8, 0xf6, 0x04, 0x24, 0x01][..], Register::Rsp),
        (&[0x65u8, 0xf6, 0x45, 0x00, 0x01][..], Register::Rbp),
    ] {
        let mut state = board_state();
        state.gs_base = 0x0000_8000_0000_0000;
        state.gpr[register as usize] = 0;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn test_byte_read_fault_leaves_flags_rip_and_registers_unchanged() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x0),
        linear_address: Some(0xffff_8880_0000_0000 + RIP + 8 + 0x02f0_692c),
    };
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_ZF;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_TEST8_BOUNDARY);
    memory.read_fault = Some(fault.clone());
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn segment_test_byte_forms_outside_the_slice_are_refused_with_the_exact_opcode() {
    let cases: [(&[u8], u8); 24] = [
        // Other F6 group members with a segment base stay outside this slice.
        (&[0x65, 0xf6, 0x0b, 0x01], 0xf6), // /1 (TEST alias)
        (&[0x65, 0xf6, 0x13], 0xf6),       // NOT
        (&[0x65, 0xf6, 0x1b], 0xf6),       // NEG
        (&[0x64, 0xf6, 0x23], 0xf6),       // MUL
        (&[0x64, 0xf6, 0x2b], 0xf6),       // IMUL
        (&[0x65, 0xf6, 0x33], 0xf6),       // DIV
        (&[0x65, 0xf6, 0x3b], 0xf6),       // IDIV
        // LOCK (invalid TEST form), REP, REPNE.
        (&[0xf0, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x65, 0xf0, 0xf6, 0x03, 0x01], 0xf6),
        (&[0xf3, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        (&[0xf2, 0x64, 0xf6, 0x03, 0x01], 0xf6),
        // 0x66 and 0x67.
        (&[0x66, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x65, 0x66, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x65, 0x67, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x67, 0x64, 0xf6, 0x03, 0x01], 0xf6),
        // A second segment prefix.
        (&[0x64, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x65, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        // REX not immediately before the opcode (stale).
        (&[0x44, 0x65, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x48, 0x64, 0xf6, 0x03, 0x01], 0xf6),
        (&[0x41, 0x65, 0x40, 0xf6, 0x03, 0x01], 0xf6),
        // The word/dword/qword TEST (F7) and TEST r/m8, r8 (84) with a segment prefix stay
        // refused with their own opcodes.
        (&[0x65, 0xf7, 0x03, 0x01, 0, 0, 0], 0xf7),
        (&[0x65, 0x48, 0xf7, 0x03, 0x01, 0, 0, 0], 0xf7),
        (&[0x65, 0x84, 0x03], 0x84),
        // FS-form NOT with an absolute address.
        (&[0x64, 0xf6, 0x14, 0x25, 0x20, 0, 0, 0], 0xf6),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn test_byte_register_form_and_plain_form_keep_their_old_behavior() {
    // The register form (mod = 11) is not memory: the segment prefix is irrelevant and it keeps
    // decoding as the plain `TEST r8, imm8` (unchanged by S4-38L7o). The memory form without a
    // segment prefix keeps ignoring the FS/GS bases.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x00;
    let mut memory = BitsMemory::new(&[0x65, 0xf6, 0xc3, 0x01]); // test $1, %bl
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert!(memory.reads.is_empty());
    assert_ne!(state.rflags & FLAG_ZF, 0);

    let mut state = bts_state(0);
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = BitsMemory::new(&[0xf6, 0x03, 0x01]); // testb $1, (%rbx)
    memory.data.insert(0x20_0000, 0x01);
    let result = run_test8_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 1)]);
    assert_eq!(state.rflags & FLAG_ZF, 0);
}

#[test]
fn cs_ds_es_ss_overrides_on_a_test_byte_keep_ignoring_the_segment_base() {
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = bts_state(0);
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory = BitsMemory::new(&[prefix, 0xf6, 0x03, 0x01]);
        let result = run_test8_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(0x20_0000, 1)], "{prefix:#x}");
    }
}

#[test]
fn segment_test_byte_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the FS/GS form is refused without mutation.
    for code in [&GS_TEST8_BOUNDARY[..], &FS_TEST8_RIP[..]] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xf6 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_test_byte_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_TEST8_BOUNDARY);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7p: FS/GS-based `INC r/m32` (`[64|65] FF /0`, memory operand, no LOCK; SDM Vol.2 INC:
// destination + 1, OF/SF/ZF/AF/PF from the result, CF unchanged; Vol.3 3.4.4 FS/GS base; from
// memory, not re-checked against the SDM text). The memory operand is always read and written back.
// The qword form (`[64|65] REX.W FF /0`) is decoded and executed the same way. ----

/// `incl %gs:0x2f26373(%rip)` (the S4-38L7o diagnostic boundary bytes, 7 bytes long).
const GS_INCL_BOUNDARY: [u8; 7] = [0x65, 0xff, 0x05, 0x73, 0x63, 0xf2, 0x02];
/// `incl %fs:0x1234(%rip)`.
const FS_INCL_RIP: [u8; 7] = [0x64, 0xff, 0x05, 0x34, 0x12, 0x00, 0x00];
/// `incq %gs:0x1234(%rip)`.
const GS_INCQ_RIP: [u8; 8] = [0x65, 0x48, 0xff, 0x05, 0x34, 0x12, 0x00, 0x00];
/// `incl %gs:0x58(%rbx)`.
const GS_INCL_RBX_58: [u8; 4] = [0x65, 0xff, 0x43, 0x58];
/// `incq %fs:0x58(%rbx)`.
const FS_INCQ_RBX_58: [u8; 5] = [0x64, 0x48, 0xff, 0x43, 0x58];

const INC_GS_BASE: u64 = 0xffff_8880_0000_0000;

fn run_inc_set(
    state: &mut LongModeState,
    memory: &mut BitsMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_bts_set(state, memory)
}

fn dword_at(memory: &BitsMemory, address: u64) -> u32 {
    memory.qword(address) as u32
}

fn set_dword(memory: &mut BitsMemory, address: u64, value: u32) {
    for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
        memory.data.insert(address + index as u64, byte);
    }
}

#[test]
fn gs_inc_dword_boundary_form_reads_and_writes_four_bytes_at_gs_base_plus_next_rip() {
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.fs_base = 0xffff_8880_9999_0000;
    state.rflags |= FLAG_CF | FLAG_ZF | FLAG_SF | FLAG_OF;
    let mut expected = state.clone();
    let at = INC_GS_BASE + RIP + 7 + 0x02f2_6373;
    let mut memory = BitsMemory::new(&GS_INCL_BOUNDARY);
    set_dword(&mut memory, at, 0x41);
    // The neighbouring bytes must survive.
    memory.data.insert(at - 1, 0xaa);
    memory.data.insert(at + 4, 0xbb);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x42u32.to_le_bytes().to_vec())]);
    assert_eq!(dword_at(&memory, at), 0x42);
    assert_eq!(memory.data.get(&(at - 1)), Some(&0xaa));
    assert_eq!(memory.data.get(&(at + 4)), Some(&0xbb));
    // INC leaves no flag undefined (CF is kept, not undefined).
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    expected.rip += 7;
    // 0x42 has two set bits: PF; CF survives, ZF/SF/OF are recomputed.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_CF | FLAG_PF;
    assert_eq!(state, expected, "only RFLAGS and RIP change");
}

#[test]
fn fs_inc_dword_rip_relative_uses_the_fs_base_and_ignores_gs() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let at = fs_base + RIP + 7 + 0x1234;
    let mut memory = BitsMemory::new(&FS_INCL_RIP);
    set_dword(&mut memory, at, 9);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 10u32.to_le_bytes().to_vec())]);
    expected.rip += 7;
    // 10 = 0b1010: PF, no other flag.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF;
    assert_eq!(state, expected);
}

#[test]
fn segment_inc_with_a_zero_base_uses_the_effective_address() {
    for (code, disp) in [
        (&GS_INCL_BOUNDARY[..], 0x02f2_6373u64),
        (&FS_INCL_RIP[..], 0x1234),
    ] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(memory.reads, vec![(RIP + 7 + disp, 4)]);
        assert_eq!(
            memory.writes,
            vec![(RIP + 7 + disp, 1u32.to_le_bytes().to_vec())]
        );
    }
}

#[test]
fn segment_inc_dword_overflow_and_wraparound_flags() {
    // (old value, expected OF/SF/ZF/AF/PF; CF is checked separately).
    let cases: [(u32, u64); 8] = [
        // INT_MAX + 1: overflow, negative result, even parity of the zero low byte, nibble carry.
        (0x7fff_ffff, FLAG_OF | FLAG_SF | FLAG_PF | FLAG_AF),
        // All ones wrap to zero: ZF, PF, AF.
        (0xffff_ffff, FLAG_ZF | FLAG_PF | FLAG_AF),
        // 0 -> 1: no flag at all (odd parity).
        (0, 0),
        // 0x0f -> 0x10: AF alone.
        (0x0f, FLAG_AF),
        (2, FLAG_PF),
        (0x7f, FLAG_AF),
        (0xffff_fffe, FLAG_SF | FLAG_PF),
        (0x8000_0000, FLAG_SF),
    ];
    for fs in [false, true] {
        for (old, arithmetic) in cases {
            for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
                let mut state = board_state();
                state.rflags = initial;
                state.gs_base = INC_GS_BASE;
                state.fs_base = INC_GS_BASE;
                state.gpr[Register::Rbx as usize] = 0x20_0000;
                let code: &[u8] = if fs {
                    &[0x64, 0xff, 0x43, 0x58]
                } else {
                    &GS_INCL_RBX_58
                };
                let mut memory = BitsMemory::new(code);
                let at = INC_GS_BASE + 0x20_0058;
                set_dword(&mut memory, at, old);
                let result = run_inc_set(&mut state, &mut memory).unwrap();
                assert_eq!(result.instructions_executed, 1, "{old:#x}");
                assert_eq!(
                    dword_at(&memory, at),
                    old.wrapping_add(1),
                    "{old:#x} from {initial:#x}"
                );
                // IF, DF and bit 1 are preserved, and so is the previous CF.
                assert_eq!(
                    state.rflags,
                    (initial & !ARITH) | (initial & FLAG_CF) | arithmetic,
                    "fs={fs} {old:#x} from {initial:#x}"
                );
            }
        }
    }
}

#[test]
fn segment_inc_keeps_cf_clear_and_set_on_the_wrap_to_zero() {
    for cf in [0, FLAG_CF] {
        for old in [0xffff_ffffu32, 0x7fff_ffff, 0] {
            let mut state = board_state();
            state.rflags = (state.rflags & !ARITH) | cf;
            state.gs_base = INC_GS_BASE;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            let mut memory = BitsMemory::new(&GS_INCL_RBX_58);
            set_dword(&mut memory, INC_GS_BASE + 0x20_0058, old);
            run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(state.rflags & FLAG_CF, cf, "{old:#x}");
        }
    }
}

#[test]
fn segment_inc_matches_the_non_prefixed_inc_on_the_same_value() {
    // `incl (%rbx)` (ff 03) has the same flag and memory result as the GS form.
    for old in [
        0u32,
        1,
        0x0f,
        0x7f,
        0xff,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_fffe,
        0xffff_ffff,
    ] {
        for initial_cf in [0, FLAG_CF] {
            let mut plain_state = bts_state(0);
            plain_state.rflags |= initial_cf;
            let mut plain_memory = BitsMemory::new(&[0xff, 0x03]);
            set_dword(&mut plain_memory, 0x20_0000, old);
            run_inc_set(&mut plain_state, &mut plain_memory).unwrap();

            let mut state = bts_state(0);
            state.rflags |= initial_cf;
            state.gs_base = INC_GS_BASE;
            let mut memory = BitsMemory::new(&[0x65, 0xff, 0x03]);
            set_dword(&mut memory, INC_GS_BASE + 0x20_0000, old);
            run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(state.rflags, plain_state.rflags, "{old:#x} {initial_cf}");
            assert_eq!(
                dword_at(&memory, INC_GS_BASE + 0x20_0000),
                dword_at(&plain_memory, 0x20_0000),
                "{old:#x}"
            );
            assert_eq!(
                memory.writes,
                vec![(
                    INC_GS_BASE + 0x20_0000,
                    old.wrapping_add(1).to_le_bytes().to_vec()
                )]
            );
        }
    }
}

#[test]
fn segment_inc_with_base_register_index_scale_and_displacement() {
    // `incl %gs:0x10(%rbx,%rcx,4)` = 65 ff 44 8b 10.
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    let before_gpr = state.gpr;
    let mut memory = BitsMemory::new(&[0x65, 0xff, 0x44, 0x8b, 0x10]);
    let at = INC_GS_BASE + 0x1000 + 12 + 0x10;
    set_dword(&mut memory, at, 0x1234);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x1235u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 5);
    assert_eq!(state.gpr, before_gpr, "no register changes");
    // `incl %fs:0x20` (absolute SIB, no base or index) = 64 ff 04 25 20 00 00 00.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    let mut memory = BitsMemory::new(&[0x64, 0xff, 0x04, 0x25, 0x20, 0, 0, 0]);
    set_dword(&mut memory, INC_GS_BASE + 0x20, 5);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(INC_GS_BASE + 0x20, 4)]);
    assert_eq!(dword_at(&memory, INC_GS_BASE + 0x20), 6);
    assert_eq!(state.rip, RIP + 8);
}

#[test]
fn segment_inc_with_rex_b_uses_r8_to_r15_as_the_base_and_changes_no_register() {
    // `incl %gs:0x8(%rN)` via REX.B: 65 41 ff 40+n 08; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0x65, 0x41, 0xff];
        if i == 4 {
            code.extend([0x44, 0x24, 0x08]);
        } else {
            code.extend([0x40 | i as u8, 0x08]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[register as usize] = base;
        let before_gpr = state.gpr;
        let at = INC_GS_BASE + base + 8;
        let mut memory = BitsMemory::new(&code);
        set_dword(&mut memory, at, 7);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(at, 4)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(at, 8u32.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(state.gpr, before_gpr, "base r{}", i + 8);
    }
}

#[test]
fn segment_inc_with_rex_x_uses_r8_to_r15_as_the_index() {
    // `incl %gs:0x10(%rbx,%rN,4)` via REX.X: 65 42 ff 44 SIB 10 with SIB = scale 4, index N, base
    // rbx. The legacy register with the same index number holds a decoy that must not be used
    // (index 4 would be "no index" without REX.X, so it has no decoy).
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let sib = 0x80 | ((i as u8) << 3) | 0x03;
        let code = [0x65, 0x42, 0xff, 0x44, sib, 0x10];
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x1000;
        state.gpr[register as usize] = 3 + i as u64;
        let legacy = [
            Register::Rax,
            Register::Rcx,
            Register::Rdx,
            Register::Rbx,
            Register::Rsp,
            Register::Rbp,
            Register::Rsi,
            Register::Rdi,
        ][i];
        if i != 3 && i != 4 {
            state.gpr[legacy as usize] = 0x7000;
        }
        let before_gpr = state.gpr;
        // rbx (index 3) is also the base, so keep its value and check the exact address.
        let at = INC_GS_BASE + 0x1000 + (3 + i as u64) * 4 + 0x10;
        let mut memory = BitsMemory::new(&code);
        set_dword(&mut memory, at, 0x1234);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "index r{}", i + 8);
        assert_eq!(memory.reads, vec![(at, 4)], "index r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(at, 0x1235u32.to_le_bytes().to_vec())],
            "index r{}",
            i + 8
        );
        assert_eq!(dword_at(&memory, at), 0x1235, "index r{}", i + 8);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(state.gpr, before_gpr, "index r{}", i + 8);
    }
    // The qword form with REX.W+X directly before the opcode: `incq %fs:(%rbx,%r9,8)` =
    // 64 4a ff 04 cb.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x2000;
    state.gpr[Register::R9 as usize] = 5;
    state.gpr[Register::Rcx as usize] = 0x9000;
    let at = INC_GS_BASE + 0x2000 + 5 * 8;
    let mut memory = BitsMemory::new(&[0x64, 0x4a, 0xff, 0x04, 0xcb]);
    memory.set_qword(at, 0xffff_ffff);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.qword(at), 0x1_0000_0000);
}

#[test]
fn segment_inc_accepts_a_rex_directly_before_the_opcode_and_ignores_rex_r() {
    // REX.R does not change the opcode extension (/0 stays INC) and there is no register
    // destination: the instruction is `incl %gs:(%rbx)` for REX 0x40 and 0x44.
    for rex in [0x40u8, 0x44] {
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before_gpr = state.gpr;
        let at = INC_GS_BASE + 0x20_0000;
        let mut memory = BitsMemory::new(&[0x65, rex, 0xff, 0x03]);
        set_dword(&mut memory, at, 0xffff_ffff);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{rex:#x}");
        assert_eq!(memory.reads, vec![(at, 4)], "{rex:#x}");
        assert_eq!(dword_at(&memory, at), 0, "{rex:#x}");
        assert_eq!(state.rip, RIP + 4, "{rex:#x}");
        assert_eq!(state.gpr, before_gpr, "{rex:#x}");
    }
}

#[test]
fn segment_inc_qword_form_increments_all_eight_bytes() {
    let cases: [(u64, u64); 4] = [
        (0xffff_ffff_ffff_ffff, FLAG_ZF | FLAG_PF | FLAG_AF),
        (0x7fff_ffff_ffff_ffff, FLAG_OF | FLAG_SF | FLAG_PF | FLAG_AF),
        // Carry out of the low dword does not wrap the qword.
        (0xffff_ffff, FLAG_PF | FLAG_AF),
        (0x1234, FLAG_PF),
    ];
    for (old, arithmetic) in cases {
        for cf in [0, FLAG_CF] {
            let mut state = board_state();
            state.rflags = (state.rflags & !ARITH) | cf;
            state.gs_base = INC_GS_BASE;
            let at = INC_GS_BASE + RIP + 8 + 0x1234;
            let mut memory = BitsMemory::new(&GS_INCQ_RIP);
            memory.set_qword(at, old);
            memory.data.insert(at + 8, 0xcc);
            let result = run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{old:#x}");
            assert_eq!(memory.reads, vec![(at, 8)]);
            assert_eq!(
                memory.writes,
                vec![(at, old.wrapping_add(1).to_le_bytes().to_vec())]
            );
            assert_eq!(memory.data.get(&(at + 8)), Some(&0xcc));
            assert_eq!(state.rip, RIP + 8);
            assert_eq!(state.rflags & ARITH, cf | arithmetic, "{old:#x}");
        }
    }
    // FS qword through a base register.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let at = INC_GS_BASE + 0x20_0058;
    let mut memory = BitsMemory::new(&FS_INCQ_RBX_58);
    memory.set_qword(at, 41);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.qword(at), 42);
    assert_eq!(memory.reads, vec![(at, 8)]);
}

#[test]
fn segment_inc_non_canonical_address_is_gp0_without_mutation_or_access() {
    // (segment base, effective address before the 0x58 displacement, code): the start, or the last
    // byte of the dword/qword, is outside the canonical range.
    let cases: [(u64, u64, &[u8]); 6] = [
        (0x0000_8000_0000_0000, 0x58, &GS_INCL_RBX_58),
        (0xffff_7fff_ffff_ff00, 0x58, &GS_INCL_RBX_58),
        // The start is canonical but the dword's last byte crosses into the hole.
        (0x0000_7fff_ffff_fffe, 0x58, &GS_INCL_RBX_58),
        (0x0000_7fff_ffff_ffff, 0x58, &GS_INCL_RBX_58),
        // The same for the qword form.
        (0x0000_7fff_ffff_fffc, 0x58, &FS_INCQ_RBX_58),
        // The base is fine but the register pushes the effective address out.
        (0, 0x0000_8000_0000_0000, &GS_INCL_RBX_58),
    ];
    for (segment_base, address, code) in cases {
        let mut state = board_state();
        state.gs_base = segment_base;
        state.fs_base = segment_base;
        // The code adds 0x58; pick RBX so that the final effective address is `address`.
        state.gpr[Register::Rbx as usize] = address.wrapping_sub(0x58);
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(GP0),
            "{segment_base:#x} {address:#x} {code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{segment_base:#x} {address:#x}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
    // RIP-relative: the effective address crosses the canonical boundary.
    let mut state = board_state();
    state.gs_base = 0x0000_8000_0000_0000 - 0x02f2_6373 - 7 - RIP;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_INCL_BOUNDARY);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn segment_inc_last_canonical_dword_is_accessible() {
    let mut state = board_state();
    state.gs_base = 0x0000_7fff_ffff_fffc - 0x02f2_6373 - 7 - RIP;
    let mut memory = BitsMemory::new(&GS_INCL_BOUNDARY);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x0000_7fff_ffff_fffc, 4)]);
}

#[test]
fn segment_inc_through_a_non_canonical_rsp_or_rbp_base_is_gp0_not_ss() {
    // `incl %gs:(%rsp)` = 65 ff 04 24 and `incl %gs:0x0(%rbp)` = 65 ff 45 00.
    for (code, register) in [
        (&[0x65u8, 0xff, 0x04, 0x24][..], Register::Rsp),
        (&[0x65u8, 0xff, 0x45, 0x00][..], Register::Rbp),
        (&[0x64u8, 0xff, 0x04, 0x24][..], Register::Rsp),
    ] {
        let mut state = board_state();
        state.gs_base = 0;
        state.fs_base = 0;
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_inc_write_fault_leaves_flags_rip_registers_and_memory_unchanged() {
    // A read-only page: the always-performed write faults.
    let at = INC_GS_BASE + 0x20_0058;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(at),
    };
    for (old, code) in [
        (0x7fff_ffffu64, &GS_INCL_RBX_58[..]),
        (0xffff_ffff_ffff_ffff, &FS_INCQ_RBX_58[..]),
    ] {
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        memory.set_qword(at, old);
        memory.write_fault = Some(fault.clone());
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.qword(at), old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn segment_inc_read_fault_leaves_everything_unchanged() {
    let at = INC_GS_BASE + RIP + 7 + 0x02f2_6373;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(at),
    };
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.rflags |= FLAG_CF | FLAG_OF;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_INCL_BOUNDARY);
    memory.read_fault = Some(fault.clone());
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn segment_inc_forms_outside_the_slice_are_refused_with_the_exact_opcode() {
    let cases: [(&[u8], u8); 31] = [
        // The other FF group members with a segment base (CALL, far CALL, JMP, far JMP, PUSH, /7)
        // stay outside this slice (DEC, /1, is S4-38L7q).
        (&[0x65, 0xff, 0x13], 0xff),
        (&[0x65, 0xff, 0x1b], 0xff),
        (&[0x64, 0xff, 0x23], 0xff),
        (&[0x64, 0xff, 0x2b], 0xff),
        (&[0x65, 0xff, 0x33], 0xff),
        (&[0x65, 0xff, 0x3b], 0xff),
        // LOCK (S4-38L7l does not take a segment prefix), REP, REPNE.
        (&[0xf0, 0x65, 0xff, 0x03], 0xff),
        (&[0x65, 0xf0, 0xff, 0x03], 0xff),
        (&[0xf0, 0x64, 0x48, 0xff, 0x03], 0xff),
        (&[0xf3, 0x65, 0xff, 0x03], 0xff),
        (&[0xf2, 0x64, 0xff, 0x03], 0xff),
        // 0x66 (word) and 0x67.
        (&[0x66, 0x65, 0xff, 0x03], 0xff),
        (&[0x65, 0x66, 0xff, 0x03], 0xff),
        (&[0x65, 0x66, 0x48, 0xff, 0x03], 0xff),
        (&[0x65, 0x67, 0xff, 0x03], 0xff),
        (&[0x67, 0x64, 0xff, 0x03], 0xff),
        // A second segment prefix.
        (&[0x64, 0x65, 0xff, 0x03], 0xff),
        (&[0x65, 0x65, 0xff, 0x03], 0xff),
        // REX not immediately before the opcode (stale), for the dword form.
        (&[0x44, 0x65, 0xff, 0x03], 0xff),
        (&[0x48, 0x64, 0xff, 0x03], 0xff),
        (&[0x41, 0x65, 0x40, 0xff, 0x03], 0xff),
        (&[0x65, 0x48, 0x67, 0xff, 0x03], 0xff),
        // The qword form with a REX.W that is not immediately before the opcode (the REX before
        // the segment prefix is stale, even though the final REX.W is present) is refused too, unlike
        // the qword `8B` of S4-38L7i.
        (&[0x48, 0x65, 0x48, 0xff, 0x03], 0xff),
        (&[0x41, 0x65, 0x48, 0xff, 0x03], 0xff),
        (&[0x48, 0x64, 0x48, 0xff, 0x03], 0xff),
        // The byte form (FE /0) with a segment base stays refused with its own opcode.
        (&[0x65, 0xfe, 0x03], 0xfe),
        (&[0x64, 0xfe, 0x03], 0xfe),
        // Neighbouring FS/GS arithmetic forms that are not part of this slice.
        (&[0x65, 0xf7, 0x1b], 0xf7), // NEG
        (&[0x65, 0xf7, 0x13], 0xf7), // NOT
        (&[0x65, 0x01, 0x03], 0x01), // ADD m32, r32
        (&[0x65, 0x29, 0x03], 0x29), // SUB m32, r32
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn segment_inc_register_form_and_plain_form_keep_their_old_behavior() {
    // The register form (mod = 11) is not memory: the segment prefix is irrelevant and it keeps
    // decoding as the plain `INC r32` (unchanged by S4-38L7p).
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0xffff_ffff;
    let mut memory = BitsMemory::new(&[0x65, 0xff, 0xc3]); // inc %ebx
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
    assert_eq!(state.gpr[Register::Rbx as usize], 0);
    assert_ne!(state.rflags & FLAG_ZF, 0);

    // The memory form without a segment prefix keeps ignoring the FS/GS bases and the old path.
    let mut state = bts_state(0);
    state.gs_base = INC_GS_BASE;
    state.fs_base = INC_GS_BASE;
    let mut memory = BitsMemory::new(&[0xff, 0x03]); // incl (%rbx)
    set_dword(&mut memory, 0x20_0000, 0x41);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 4)]);
    assert_eq!(dword_at(&memory, 0x20_0000), 0x42);
    // CS/DS/ES/SS overrides keep ignoring the segment base too.
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = bts_state(0);
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let mut memory = BitsMemory::new(&[prefix, 0xff, 0x03]);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(0x20_0000, 4)], "{prefix:#x}");
    }
}

#[test]
fn segment_inc_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the FS/GS form is refused without mutation.
    for code in [&GS_INCL_BOUNDARY[..], &FS_INCL_RIP[..], &GS_INCQ_RIP[..]] {
        let mut state = legacy_state();
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xff });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_inc_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_INCL_BOUNDARY);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7q: FS/GS-based `DEC r/m32` (`[64|65] FF /1`, memory operand, no LOCK; SDM Vol.2 DEC:
// destination - 1, OF/SF/ZF/AF/PF from the result, CF unchanged; Vol.3 3.4.4 FS/GS base; from
// memory, not re-checked against the SDM text). The memory operand is always read and written back.
// The qword form (`[64|65] REX.W FF /1`) is decoded and executed the same way, through the same
// operation as INC (S4-38L7p) with the DEC arithmetic. ----

/// `decl %gs:0x2f26364(%rip)` (the S4-38L7p diagnostic boundary bytes, 7 bytes long).
const GS_DECL_BOUNDARY: [u8; 7] = [0x65, 0xff, 0x0d, 0x64, 0x63, 0xf2, 0x02];
/// `decl %fs:0x1234(%rip)`.
const FS_DECL_RIP: [u8; 7] = [0x64, 0xff, 0x0d, 0x34, 0x12, 0x00, 0x00];
/// `decq %gs:0x1234(%rip)`.
const GS_DECQ_RIP: [u8; 8] = [0x65, 0x48, 0xff, 0x0d, 0x34, 0x12, 0x00, 0x00];
/// `decl %gs:0x58(%rbx)`.
const GS_DECL_RBX_58: [u8; 4] = [0x65, 0xff, 0x4b, 0x58];
/// `decq %fs:0x58(%rbx)`.
const FS_DECQ_RBX_58: [u8; 5] = [0x64, 0x48, 0xff, 0x4b, 0x58];

#[test]
fn gs_dec_dword_boundary_form_reads_and_writes_four_bytes_at_gs_base_plus_next_rip() {
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.fs_base = 0xffff_8880_9999_0000;
    state.rflags |= FLAG_CF | FLAG_ZF | FLAG_SF | FLAG_OF;
    let mut expected = state.clone();
    let at = INC_GS_BASE + RIP + 7 + 0x02f2_6364;
    let mut memory = BitsMemory::new(&GS_DECL_BOUNDARY);
    set_dword(&mut memory, at, 0x42);
    // The neighbouring bytes must survive.
    memory.data.insert(at - 1, 0xaa);
    memory.data.insert(at + 4, 0xbb);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x41u32.to_le_bytes().to_vec())]);
    assert_eq!(dword_at(&memory, at), 0x41);
    assert_eq!(memory.data.get(&(at - 1)), Some(&0xaa));
    assert_eq!(memory.data.get(&(at + 4)), Some(&0xbb));
    // DEC leaves no flag undefined (CF is kept, not undefined).
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    expected.rip += 7;
    // 0x41 has two set bits: PF; CF survives, ZF/SF/OF are recomputed.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_CF | FLAG_PF;
    assert_eq!(state, expected, "only RFLAGS and RIP change");
}

#[test]
fn fs_dec_dword_rip_relative_uses_the_fs_base_and_ignores_gs() {
    let fs_base = 0xffff_8880_4321_0000u64;
    let mut state = board_state();
    state.fs_base = fs_base;
    state.gs_base = 0xffff_8880_9999_0000;
    let mut expected = state.clone();
    let at = fs_base + RIP + 7 + 0x1234;
    let mut memory = BitsMemory::new(&FS_DECL_RIP);
    set_dword(&mut memory, at, 10);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 9u32.to_le_bytes().to_vec())]);
    expected.rip += 7;
    // 9 = 0b1001: PF, no other flag.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_PF;
    assert_eq!(state, expected);
}

#[test]
fn segment_dec_with_a_zero_base_uses_the_effective_address() {
    for (code, disp) in [
        (&GS_DECL_BOUNDARY[..], 0x02f2_6364u64),
        (&FS_DECL_RIP[..], 0x1234),
    ] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = BitsMemory::new(code);
        set_dword(&mut memory, RIP + 7 + disp, 5);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(memory.reads, vec![(RIP + 7 + disp, 4)]);
        assert_eq!(
            memory.writes,
            vec![(RIP + 7 + disp, 4u32.to_le_bytes().to_vec())]
        );
    }
}

#[test]
fn segment_dec_dword_overflow_and_wraparound_flags() {
    // (old value, expected OF/SF/ZF/AF/PF; CF is checked separately). Worked by hand from the DEC
    // rule: result = old - 1; AF = bit 4 of old ^ 1 ^ result (a borrow out of bit 3); PF = even
    // number of set bits in the low result byte; OF only for 0x80000000 -> 0x7fffffff.
    let cases: [(u32, u64); 10] = [
        // INT_MIN - 1: overflow, positive result, low byte 0xff (even parity), nibble borrow.
        (0x8000_0000, FLAG_OF | FLAG_PF | FLAG_AF),
        // 0 wraps to all ones: SF, PF, AF (CF stays as it was).
        (0, FLAG_SF | FLAG_PF | FLAG_AF),
        // 1 -> 0: ZF and PF; no nibble borrow, so AF stays clear.
        (1, FLAG_ZF | FLAG_PF),
        // 0x10 -> 0x0f: AF (borrow out of the low nibble) and PF (four bits).
        (0x10, FLAG_AF | FLAG_PF),
        // 2 -> 1: one bit, no flag.
        (2, 0),
        (0x0f, 0),
        (0x7fff_ffff, 0),
        // 0xffffffff -> 0xfffffffe: SF only (seven bits in the low byte).
        (0xffff_ffff, FLAG_SF),
        // 0x100 -> 0xff: PF and AF.
        (0x100, FLAG_PF | FLAG_AF),
        // 0x80000001 -> 0x80000000: SF, and PF (the low byte is zero).
        (0x8000_0001, FLAG_SF | FLAG_PF),
    ];
    for fs in [false, true] {
        for (old, arithmetic) in cases {
            for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
                let mut state = board_state();
                state.rflags = initial;
                state.gs_base = INC_GS_BASE;
                state.fs_base = INC_GS_BASE;
                state.gpr[Register::Rbx as usize] = 0x20_0000;
                let code: &[u8] = if fs {
                    &[0x64, 0xff, 0x4b, 0x58]
                } else {
                    &GS_DECL_RBX_58
                };
                let mut memory = BitsMemory::new(code);
                let at = INC_GS_BASE + 0x20_0058;
                set_dword(&mut memory, at, old);
                let result = run_inc_set(&mut state, &mut memory).unwrap();
                assert_eq!(result.instructions_executed, 1, "{old:#x}");
                assert_eq!(
                    dword_at(&memory, at),
                    old.wrapping_sub(1),
                    "{old:#x} from {initial:#x}"
                );
                // IF, DF and bit 1 are preserved, and so is the previous CF.
                assert_eq!(
                    state.rflags,
                    (initial & !ARITH) | (initial & FLAG_CF) | arithmetic,
                    "fs={fs} {old:#x} from {initial:#x}"
                );
            }
        }
    }
}

#[test]
fn segment_dec_keeps_cf_clear_and_set_on_the_wrap_and_the_overflow() {
    for cf in [0, FLAG_CF] {
        for old in [0u32, 0x8000_0000, 1, 0x10] {
            let mut state = board_state();
            state.rflags = (state.rflags & !ARITH) | cf;
            state.gs_base = INC_GS_BASE;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            let mut memory = BitsMemory::new(&GS_DECL_RBX_58);
            set_dword(&mut memory, INC_GS_BASE + 0x20_0058, old);
            run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(state.rflags & FLAG_CF, cf, "{old:#x}");
        }
    }
}

#[test]
fn segment_dec_matches_the_non_prefixed_dec_on_the_same_value() {
    // `decl (%rbx)` (ff 0b) has the same flag and memory result as the GS form.
    for old in [
        0u32,
        1,
        0x0f,
        0x10,
        0x7f,
        0x100,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_fffe,
        0xffff_ffff,
    ] {
        for initial_cf in [0, FLAG_CF] {
            let mut plain_state = bts_state(0);
            plain_state.rflags |= initial_cf;
            let mut plain_memory = BitsMemory::new(&[0xff, 0x0b]);
            set_dword(&mut plain_memory, 0x20_0000, old);
            run_inc_set(&mut plain_state, &mut plain_memory).unwrap();

            let mut state = bts_state(0);
            state.rflags |= initial_cf;
            state.gs_base = INC_GS_BASE;
            let mut memory = BitsMemory::new(&[0x65, 0xff, 0x0b]);
            set_dword(&mut memory, INC_GS_BASE + 0x20_0000, old);
            run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(state.rflags, plain_state.rflags, "{old:#x} {initial_cf}");
            assert_eq!(
                dword_at(&memory, INC_GS_BASE + 0x20_0000),
                dword_at(&plain_memory, 0x20_0000),
                "{old:#x}"
            );
            assert_eq!(
                memory.writes,
                vec![(
                    INC_GS_BASE + 0x20_0000,
                    old.wrapping_sub(1).to_le_bytes().to_vec()
                )]
            );
        }
    }
}

#[test]
fn segment_dec_with_base_register_index_scale_and_displacement() {
    // `decl %gs:0x10(%rbx,%rcx,4)` = 65 ff 4c 8b 10.
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x1000;
    state.gpr[Register::Rcx as usize] = 3;
    let before_gpr = state.gpr;
    let mut memory = BitsMemory::new(&[0x65, 0xff, 0x4c, 0x8b, 0x10]);
    let at = INC_GS_BASE + 0x1000 + 12 + 0x10;
    set_dword(&mut memory, at, 0x1234);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 4)]);
    assert_eq!(memory.writes, vec![(at, 0x1233u32.to_le_bytes().to_vec())]);
    assert_eq!(state.rip, RIP + 5);
    assert_eq!(state.gpr, before_gpr, "no register changes");
    // `decl %fs:0x20` (absolute SIB, no base or index) = 64 ff 0c 25 20 00 00 00.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    let mut memory = BitsMemory::new(&[0x64, 0xff, 0x0c, 0x25, 0x20, 0, 0, 0]);
    set_dword(&mut memory, INC_GS_BASE + 0x20, 5);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(INC_GS_BASE + 0x20, 4)]);
    assert_eq!(dword_at(&memory, INC_GS_BASE + 0x20), 4);
    assert_eq!(state.rip, RIP + 8);
}

#[test]
fn segment_dec_with_rex_b_uses_r8_to_r15_as_the_base_and_changes_no_register() {
    // `decl %gs:0x8(%rN)` via REX.B: 65 41 ff 48+n 08; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0x65, 0x41, 0xff];
        if i == 4 {
            code.extend([0x4c, 0x24, 0x08]);
        } else {
            code.extend([0x48 | i as u8, 0x08]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[register as usize] = base;
        let before_gpr = state.gpr;
        let at = INC_GS_BASE + base + 8;
        let mut memory = BitsMemory::new(&code);
        set_dword(&mut memory, at, 7);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(at, 4)], "base r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(at, 6u32.to_le_bytes().to_vec())],
            "base r{}",
            i + 8
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(state.gpr, before_gpr, "base r{}", i + 8);
    }
}

#[test]
fn segment_dec_with_rex_x_uses_r8_to_r15_as_the_index() {
    // `decl %gs:0x10(%rbx,%rN,4)` via REX.X: 65 42 ff 4c SIB 10 with SIB = scale 4, index N, base
    // rbx. The legacy register with the same index number holds a decoy that must not be used
    // (index 4 would be "no index" without REX.X, so it has no decoy).
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let sib = 0x80 | ((i as u8) << 3) | 0x03;
        let code = [0x65, 0x42, 0xff, 0x4c, sib, 0x10];
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x1000;
        state.gpr[register as usize] = 3 + i as u64;
        let legacy = [
            Register::Rax,
            Register::Rcx,
            Register::Rdx,
            Register::Rbx,
            Register::Rsp,
            Register::Rbp,
            Register::Rsi,
            Register::Rdi,
        ][i];
        if i != 3 && i != 4 {
            state.gpr[legacy as usize] = 0x7000;
        }
        let before_gpr = state.gpr;
        let at = INC_GS_BASE + 0x1000 + (3 + i as u64) * 4 + 0x10;
        let mut memory = BitsMemory::new(&code);
        set_dword(&mut memory, at, 0x1234);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "index r{}", i + 8);
        assert_eq!(memory.reads, vec![(at, 4)], "index r{}", i + 8);
        assert_eq!(
            memory.writes,
            vec![(at, 0x1233u32.to_le_bytes().to_vec())],
            "index r{}",
            i + 8
        );
        assert_eq!(dword_at(&memory, at), 0x1233, "index r{}", i + 8);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(state.gpr, before_gpr, "index r{}", i + 8);
    }
    // The qword form with REX.W+X directly before the opcode: `decq %fs:(%rbx,%r9,8)` =
    // 64 4a ff 0c cb.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x2000;
    state.gpr[Register::R9 as usize] = 5;
    state.gpr[Register::Rcx as usize] = 0x9000;
    let at = INC_GS_BASE + 0x2000 + 5 * 8;
    let mut memory = BitsMemory::new(&[0x64, 0x4a, 0xff, 0x0c, 0xcb]);
    memory.set_qword(at, 0x1_0000_0000);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(at, 8)]);
    assert_eq!(memory.qword(at), 0xffff_ffff);
}

#[test]
fn segment_dec_accepts_a_rex_directly_before_the_opcode_and_ignores_rex_r() {
    // REX.R does not change the opcode extension (/1 stays DEC) and there is no register
    // destination: the instruction is `decl %gs:(%rbx)` for REX 0x40 and 0x44.
    for rex in [0x40u8, 0x44] {
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before_gpr = state.gpr;
        let at = INC_GS_BASE + 0x20_0000;
        let mut memory = BitsMemory::new(&[0x65, rex, 0xff, 0x0b]);
        set_dword(&mut memory, at, 0);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{rex:#x}");
        assert_eq!(memory.reads, vec![(at, 4)], "{rex:#x}");
        assert_eq!(dword_at(&memory, at), 0xffff_ffff, "{rex:#x}");
        assert_eq!(state.rip, RIP + 4, "{rex:#x}");
        assert_eq!(state.gpr, before_gpr, "{rex:#x}");
    }
}

#[test]
fn segment_dec_qword_form_decrements_all_eight_bytes() {
    let cases: [(u64, u64); 5] = [
        (0, FLAG_SF | FLAG_PF | FLAG_AF),
        (0x8000_0000_0000_0000, FLAG_OF | FLAG_PF | FLAG_AF),
        // Borrow out of the low dword does not wrap the qword: 0x1_0000_0000 -> 0xffff_ffff.
        (0x1_0000_0000, FLAG_PF | FLAG_AF),
        (1, FLAG_ZF | FLAG_PF),
        // 0x1234 -> 0x1233: low byte 0x33 has four set bits.
        (0x1234, FLAG_PF),
    ];
    for (old, arithmetic) in cases {
        for cf in [0, FLAG_CF] {
            let mut state = board_state();
            state.rflags = (state.rflags & !ARITH) | cf;
            state.gs_base = INC_GS_BASE;
            let at = INC_GS_BASE + RIP + 8 + 0x1234;
            let mut memory = BitsMemory::new(&GS_DECQ_RIP);
            memory.set_qword(at, old);
            memory.data.insert(at + 8, 0xcc);
            let result = run_inc_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{old:#x}");
            assert_eq!(memory.reads, vec![(at, 8)]);
            assert_eq!(
                memory.writes,
                vec![(at, old.wrapping_sub(1).to_le_bytes().to_vec())]
            );
            assert_eq!(memory.data.get(&(at + 8)), Some(&0xcc));
            assert_eq!(state.rip, RIP + 8);
            assert_eq!(state.rflags & ARITH, cf | arithmetic, "{old:#x}");
        }
    }
    // FS qword through a base register.
    let mut state = board_state();
    state.fs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let at = INC_GS_BASE + 0x20_0058;
    let mut memory = BitsMemory::new(&FS_DECQ_RBX_58);
    memory.set_qword(at, 43);
    run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.qword(at), 42);
    assert_eq!(memory.reads, vec![(at, 8)]);
}

#[test]
fn segment_dec_non_canonical_address_is_gp0_without_mutation_or_access() {
    // (segment base, effective address before the 0x58 displacement, code): the start, or the last
    // byte of the dword/qword, is outside the canonical range.
    let cases: [(u64, u64, &[u8]); 6] = [
        (0x0000_8000_0000_0000, 0x58, &GS_DECL_RBX_58),
        (0xffff_7fff_ffff_ff00, 0x58, &GS_DECL_RBX_58),
        // The start is canonical but the dword's last byte crosses into the hole.
        (0x0000_7fff_ffff_fffe, 0x58, &GS_DECL_RBX_58),
        (0x0000_7fff_ffff_ffff, 0x58, &GS_DECL_RBX_58),
        // The same for the qword form.
        (0x0000_7fff_ffff_fffc, 0x58, &FS_DECQ_RBX_58),
        // The base is fine but the register pushes the effective address out.
        (0, 0x0000_8000_0000_0000, &GS_DECL_RBX_58),
    ];
    for (segment_base, address, code) in cases {
        let mut state = board_state();
        state.gs_base = segment_base;
        state.fs_base = segment_base;
        // The code adds 0x58; pick RBX so that the final effective address is `address`.
        state.gpr[Register::Rbx as usize] = address.wrapping_sub(0x58);
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(GP0),
            "{segment_base:#x} {address:#x} {code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{segment_base:#x} {address:#x}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
    // RIP-relative: the effective address crosses the canonical boundary.
    let mut state = board_state();
    state.gs_base = 0x0000_8000_0000_0000 - 0x02f2_6364 - 7 - RIP;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_DECL_BOUNDARY);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(GP0));
    assert_eq!(state, before);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

#[test]
fn segment_dec_last_canonical_dword_is_accessible() {
    let mut state = board_state();
    state.gs_base = 0x0000_7fff_ffff_fffc - 0x02f2_6364 - 7 - RIP;
    let mut memory = BitsMemory::new(&GS_DECL_BOUNDARY);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x0000_7fff_ffff_fffc, 4)]);
}

#[test]
fn segment_dec_through_a_non_canonical_rsp_or_rbp_base_is_gp0_not_ss() {
    // `decl %gs:(%rsp)` = 65 ff 0c 24 and `decl %gs:0x0(%rbp)` = 65 ff 4d 00.
    for (code, register) in [
        (&[0x65u8, 0xff, 0x0c, 0x24][..], Register::Rsp),
        (&[0x65u8, 0xff, 0x4d, 0x00][..], Register::Rbp),
        (&[0x64u8, 0xff, 0x0c, 0x24][..], Register::Rsp),
    ] {
        let mut state = board_state();
        state.gs_base = 0;
        state.fs_base = 0;
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_dec_write_fault_leaves_flags_rip_registers_and_memory_unchanged() {
    // A read-only page: the always-performed write faults.
    let at = INC_GS_BASE + 0x20_0058;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(at),
    };
    for (old, code) in [
        (0x8000_0000u64, &GS_DECL_RBX_58[..]),
        (0, &FS_DECQ_RBX_58[..]),
    ] {
        let mut state = board_state();
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        memory.set_qword(at, old);
        memory.write_fault = Some(fault.clone());
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.qword(at), old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn segment_dec_read_fault_leaves_everything_unchanged() {
    let at = INC_GS_BASE + RIP + 7 + 0x02f2_6364;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(at),
    };
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.rflags |= FLAG_CF | FLAG_OF;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_DECL_BOUNDARY);
    memory.read_fault = Some(fault.clone());
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn segment_dec_forms_outside_the_slice_are_refused_with_the_exact_opcode() {
    let cases: &[(&[u8], u8)] = &[
        // The other FF group members with a segment base stay outside this slice (CALL, far CALL,
        // JMP, far JMP, PUSH, /7); INC (/0) is S4-38L7p and DEC (/1) is this slice.
        (&[0x65, 0xff, 0x13], 0xff),
        (&[0x65, 0xff, 0x1b], 0xff),
        (&[0x64, 0xff, 0x23], 0xff),
        (&[0x64, 0xff, 0x2b], 0xff),
        (&[0x65, 0xff, 0x33], 0xff),
        (&[0x65, 0xff, 0x3b], 0xff),
        // LOCK (no LOCK DEC and no segment prefix with LOCK), REP, REPNE.
        (&[0xf0, 0x65, 0xff, 0x0b], 0xff),
        (&[0x65, 0xf0, 0xff, 0x0b], 0xff),
        (&[0xf0, 0x64, 0x48, 0xff, 0x0b], 0xff),
        (&[0xf0, 0xff, 0x0b], 0xff),
        (&[0xf3, 0x65, 0xff, 0x0b], 0xff),
        (&[0xf2, 0x64, 0xff, 0x0b], 0xff),
        (&[0x65, 0xf3, 0xff, 0x0b], 0xff),
        // 0x66 (word) and 0x67.
        (&[0x66, 0x65, 0xff, 0x0b], 0xff),
        (&[0x65, 0x66, 0xff, 0x0b], 0xff),
        (&[0x65, 0x66, 0x48, 0xff, 0x0b], 0xff),
        (&[0x65, 0x67, 0xff, 0x0b], 0xff),
        (&[0x67, 0x64, 0xff, 0x0b], 0xff),
        // A second segment prefix.
        (&[0x64, 0x65, 0xff, 0x0b], 0xff),
        (&[0x65, 0x65, 0xff, 0x0b], 0xff),
        // REX not immediately before the opcode (stale), for the dword form.
        (&[0x44, 0x65, 0xff, 0x0b], 0xff),
        (&[0x48, 0x64, 0xff, 0x0b], 0xff),
        (&[0x41, 0x65, 0x40, 0xff, 0x0b], 0xff),
        (&[0x65, 0x48, 0x67, 0xff, 0x0b], 0xff),
        // The qword form with a REX.W that is not immediately before the opcode (the REX before
        // the segment prefix is stale, even though the final REX.W is present) is refused too.
        (&[0x48, 0x65, 0x48, 0xff, 0x0b], 0xff),
        (&[0x41, 0x65, 0x48, 0xff, 0x0b], 0xff),
        (&[0x48, 0x64, 0x48, 0xff, 0x0b], 0xff),
        (&[0x44, 0x64, 0x48, 0xff, 0x4b, 0x58], 0xff),
        // The byte form (FE /1) with a segment base stays refused with its own opcode.
        (&[0x65, 0xfe, 0x0b], 0xfe),
        (&[0x64, 0xfe, 0x0b], 0xfe),
        (&[0x65, 0xfe, 0x4b, 0x58], 0xfe),
        // Neighbouring FS/GS arithmetic forms that are not part of this slice.
        (&[0x65, 0xf7, 0x1b], 0xf7),       // NEG
        (&[0x65, 0xf7, 0x13], 0xf7),       // NOT
        (&[0x65, 0x01, 0x03], 0x01),       // ADD m32, r32
        (&[0x65, 0x29, 0x03], 0x29),       // SUB m32, r32
        (&[0x65, 0x83, 0x2b, 0x01], 0x83), // SUB m32, imm8
        (&[0x65, 0x83, 0x03, 0x01], 0x83), // ADD m32, imm8
    ];
    for &(code, opcode) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn segment_dec_register_form_and_plain_form_keep_their_old_behavior() {
    // The register form (mod = 11) is not memory: the segment prefix is irrelevant and it keeps
    // decoding as the plain `DEC r32` (unchanged by S4-38L7q).
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 1;
    let mut memory = BitsMemory::new(&[0x65, 0xff, 0xcb]); // dec %ebx
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
    assert_eq!(state.gpr[Register::Rbx as usize], 0);
    assert_ne!(state.rflags & FLAG_ZF, 0);

    // The memory form without a segment prefix keeps ignoring the FS/GS bases and the old path.
    let mut state = bts_state(0);
    state.gs_base = INC_GS_BASE;
    state.fs_base = INC_GS_BASE;
    let mut memory = BitsMemory::new(&[0xff, 0x0b]); // decl (%rbx)
    set_dword(&mut memory, 0x20_0000, 0x41);
    let result = run_inc_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 4)]);
    assert_eq!(dword_at(&memory, 0x20_0000), 0x40);
    // CS/DS/ES/SS overrides keep ignoring the segment base too.
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = bts_state(0);
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let mut memory = BitsMemory::new(&[prefix, 0xff, 0x0b]);
        let result = run_inc_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(0x20_0000, 4)], "{prefix:#x}");
    }
}

#[test]
fn segment_dec_leaves_the_inc_forms_working_in_the_same_run() {
    // `incl %gs:0x58(%rbx)` and `decl %gs:0x58(%rbx)` back to back (INC is S4-38L7p).
    let mut state = board_state();
    state.gs_base = INC_GS_BASE;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.rflags |= FLAG_CF;
    let at = INC_GS_BASE + 0x20_0058;
    let mut memory = BitsMemory::new(&[0x65, 0xff, 0x43, 0x58, 0x65, 0xff, 0x4b, 0x58]);
    set_dword(&mut memory, at, 0x7fff_ffff);
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
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(
        memory.writes,
        vec![
            (at, 0x8000_0000u32.to_le_bytes().to_vec()),
            (at, 0x7fff_ffffu32.to_le_bytes().to_vec())
        ]
    );
    assert_eq!(dword_at(&memory, at), 0x7fff_ffff);
    assert_ne!(state.rflags & FLAG_CF, 0, "CF kept through both");
}

#[test]
fn segment_dec_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the FS/GS form is refused without mutation.
    for code in [&GS_DECL_BOUNDARY[..], &FS_DECL_RIP[..], &GS_DECQ_RIP[..]] {
        let mut state = legacy_state();
        state.gs_base = INC_GS_BASE;
        state.fs_base = INC_GS_BASE;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xff });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_dec_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_DECL_BOUNDARY);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7r: FS/GS-based `TEST r/m16, imm16` (`[64|65] 66 [REX] F7 /0 iw` and
// `66 [64|65] [REX] F7 /0 iw`, memory operand; SDM Vol.2 TEST: the word is ANDed with the
// immediate, the result is discarded, CF = OF = 0, SF/ZF/PF from the word result (PF from the low
// byte of the result), AF undefined and left unchanged here; Vol.3 3.4.4 FS/GS base; from memory,
// not re-checked against the SDM text). Nothing is written to memory or to a register. Both prefix
// orders are accepted; REX.W together with 0x66 is refused (no guess about the precedence). ----

/// `testw $0xff01, %gs:0x2f06873(%rip)` (the S4-38L7q diagnostic boundary bytes, 10 bytes long:
/// the next RIP is after the imm16).
const GS_TEST16_BOUNDARY: [u8; 10] = [0x65, 0x66, 0xf7, 0x05, 0x73, 0x68, 0xf0, 0x02, 0x01, 0xff];
/// The same instruction with the 0x66 and the segment prefix swapped.
const GS_TEST16_BOUNDARY_SWAPPED: [u8; 10] =
    [0x66, 0x65, 0xf7, 0x05, 0x73, 0x68, 0xf0, 0x02, 0x01, 0xff];
/// `testw $0x000e, %fs:0x1234(%rip)`, both prefix orders.
const FS_TEST16_RIP: [u8; 10] = [0x64, 0x66, 0xf7, 0x05, 0x34, 0x12, 0x00, 0x00, 0x0e, 0x00];
const FS_TEST16_RIP_SWAPPED: [u8; 10] =
    [0x66, 0x64, 0xf7, 0x05, 0x34, 0x12, 0x00, 0x00, 0x0e, 0x00];
const GS_TEST16_DISP: u64 = 0x02f0_6873;

fn run_test16_set(
    state: &mut LongModeState,
    memory: &mut BitsMemory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_bts_set(state, memory)
}

fn set_word(memory: &mut BitsMemory, address: u64, value: u16) {
    memory.data.insert(address, value as u8);
    memory.data.insert(address + 1, (value >> 8) as u8);
}

#[test]
fn gs_test_word_boundary_form_reads_one_word_at_gs_base_plus_next_rip_and_changes_only_flags() {
    for code in [GS_TEST16_BOUNDARY, GS_TEST16_BOUNDARY_SWAPPED] {
        let gs_base = 0xffff_8880_0000_0000u64;
        let mut state = board_state();
        state.gs_base = gs_base;
        state.fs_base = 0xffff_8880_9999_0000;
        state.rflags |= FLAG_CF | FLAG_OF | FLAG_AF;
        let mut expected = state.clone();
        let at = gs_base + RIP + 10 + GS_TEST16_DISP;
        let mut memory = BitsMemory::new(&code);
        // 0x8f01 & 0xff01 = 0x8f01: SF = 1, ZF = 0, low byte 0x01 -> PF = 0, CF = OF = 0.
        set_word(&mut memory, at, 0x8f01);
        // The neighbouring bytes must stay unread.
        memory.data.insert(at - 1, 0xff);
        memory.data.insert(at + 2, 0xff);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(memory.reads, vec![(at, 2)], "{code:02x?}");
        assert!(memory.writes.is_empty());
        // As the plain TEST: CF/OF/SF/ZF/PF become defined, AF is undefined.
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !FLAG_AF);
        expected.rflags = (expected.rflags & !TEST8_FLAGS) | FLAG_SF;
        expected.rip += 10;
        assert_eq!(state, expected, "only RFLAGS and RIP change: {code:02x?}");
        assert_ne!(state.rflags & FLAG_AF, 0, "AF is kept");
    }
}

#[test]
fn fs_test_word_rip_relative_reads_at_fs_base_plus_effective_address_in_both_prefix_orders() {
    for code in [FS_TEST16_RIP, FS_TEST16_RIP_SWAPPED] {
        let fs_base = 0xffff_8880_4321_0000u64;
        let mut state = board_state();
        state.fs_base = fs_base;
        state.gs_base = 0xffff_8880_9999_0000;
        let mut expected = state.clone();
        let at = fs_base + RIP + 10 + 0x1234;
        let mut memory = BitsMemory::new(&code);
        set_word(&mut memory, at, 0xfff1);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(memory.reads, vec![(at, 2)], "{code:02x?}");
        assert!(memory.writes.is_empty());
        // 0xfff1 & 0x000e = 0: ZF = 1, PF = 1, SF = 0.
        expected.rflags |= FLAG_ZF | FLAG_PF;
        expected.rip += 10;
        assert_eq!(state, expected, "{code:02x?}");
    }
}

#[test]
fn test_word_with_a_zero_base_uses_the_effective_address() {
    for (code, disp) in [
        (GS_TEST16_BOUNDARY, GS_TEST16_DISP),
        (GS_TEST16_BOUNDARY_SWAPPED, GS_TEST16_DISP),
        (FS_TEST16_RIP, 0x1234),
        (FS_TEST16_RIP_SWAPPED, 0x1234),
    ] {
        let mut state = board_state();
        assert_eq!((state.gs_base, state.fs_base), (0, 0));
        let mut memory = BitsMemory::new(&code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(memory.reads, vec![(RIP + 10 + disp, 2)]);
    }
}

#[test]
fn test_word_flags_follow_the_word_result_on_boundary_values() {
    // (memory word, immediate, expected ZF/SF/PF bits). CF and OF are always cleared, and the
    // result is the word (not widened or narrowed): 0x8000 sets SF, only the low eight bits of the
    // result count for PF (hand-checked: 0x0100 and 0x8000 have a zero low byte, so PF = 1).
    let cases: [(u16, u16, u64); 17] = [
        (0x0000, 0xffff, FLAG_ZF | FLAG_PF),
        (0xffff, 0x0000, FLAG_ZF | FLAG_PF),
        (0x8000, 0x8000, FLAG_SF | FLAG_PF),
        (0xffff, 0x0003, FLAG_PF),
        (0x0100, 0x0100, FLAG_PF),
        (0x0001, 0x0001, 0),
        (0x00ff, 0xffff, FLAG_PF),
        (0x7fff, 0xffff, FLAG_PF),
        (0xff00, 0xffff, FLAG_SF | FLAG_PF),
        (0x8001, 0x8001, FLAG_SF),
        (0x0100, 0xff00, FLAG_PF),
        (0x1234, 0x00ff, 0),
        (0xa5a5, 0xffff, FLAG_SF | FLAG_PF),
        (0x8000, 0x7fff, FLAG_ZF | FLAG_PF),
        (0xff00, 0x00ff, FLAG_ZF | FLAG_PF),
        (0x0300, 0x0100, FLAG_PF),
        (0x0107, 0x0107, 0),
    ];
    for (value, immediate, expected_flags) in cases {
        for (order, template) in [(0, GS_TEST16_BOUNDARY), (1, GS_TEST16_BOUNDARY_SWAPPED)] {
            for initial in [
                0u64,
                TEST8_FLAGS,
                FLAG_CF | FLAG_OF,
                FLAG_AF,
                TEST8_FLAGS | FLAG_AF,
            ] {
                let mut code = template;
                code[8] = immediate as u8;
                code[9] = (immediate >> 8) as u8;
                let mut state = board_state();
                state.gs_base = 0xffff_8880_0000_0000;
                state.rflags = 2 | IF | initial;
                let mut expected = state.clone();
                let at = 0xffff_8880_0000_0000 + RIP + 10 + GS_TEST16_DISP;
                let mut memory = BitsMemory::new(&code);
                set_word(&mut memory, at, value);
                let result = run_test16_set(&mut state, &mut memory).unwrap();
                let label = format!("{value:#x} & {immediate:#x} from {initial:#x} order {order}");
                assert_eq!(result.instructions_executed, 1, "{label}");
                assert_eq!(state.rflags & (FLAG_CF | FLAG_OF), 0, "{label}");
                assert_eq!(state.rflags & TEST8_FLAGS, expected_flags, "{label}");
                expected.rflags = (expected.rflags & !TEST8_FLAGS) | expected_flags;
                expected.rip += 10;
                assert_eq!(state, expected, "{label}");
                assert!(memory.writes.is_empty(), "{label}");
            }
        }
    }
}

#[test]
fn test_word_matches_the_non_prefixed_test_on_the_same_word() {
    // `testw $imm, (%rbx)` (66 f7 03 iw) has the same flag result as the GS form on every value.
    for value in [
        0u16, 1, 2, 3, 0x7f, 0x80, 0x100, 0x7fff, 0x8000, 0x8001, 0xff00, 0xffff,
    ] {
        for immediate in [0u16, 1, 0x80, 0x100, 0x8000, 0xff00, 0xffff, 0x5555] {
            let mut plain_state = bts_state(0);
            plain_state.rflags |= FLAG_CF | FLAG_OF;
            let mut plain_memory =
                BitsMemory::new(&[0x66, 0xf7, 0x03, immediate as u8, (immediate >> 8) as u8]);
            set_word(&mut plain_memory, 0x20_0000, value);
            run_test16_set(&mut plain_state, &mut plain_memory).unwrap();
            assert_eq!(plain_memory.reads, vec![(0x20_0000, 2)]);

            let mut state = bts_state(0);
            state.rflags |= FLAG_CF | FLAG_OF;
            state.gs_base = 0xffff_8880_0000_0000;
            let mut memory = BitsMemory::new(&[
                0x65,
                0x66,
                0xf7,
                0x03,
                immediate as u8,
                (immediate >> 8) as u8,
            ]);
            set_word(&mut memory, 0xffff_8880_0020_0000, value);
            run_test16_set(&mut state, &mut memory).unwrap();
            assert_eq!(
                state.rflags, plain_state.rflags,
                "{value:#x} {immediate:#x}"
            );
            assert_eq!(memory.reads, vec![(0xffff_8880_0020_0000, 2)]);
        }
    }
}

#[test]
fn gs_test_word_with_base_register_index_scale_and_displacement() {
    // `testw $0x40, %gs:0x10(%rbx,%rcx,4)` = 65 66 f7 44 8b 10 40 00, in both prefix orders.
    for code in [
        [0x65u8, 0x66, 0xf7, 0x44, 0x8b, 0x10, 0x40, 0x00],
        [0x66, 0x65, 0xf7, 0x44, 0x8b, 0x10, 0x40, 0x00],
    ] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.gpr[Register::Rbx as usize] = 0x1000;
        state.gpr[Register::Rcx as usize] = 3;
        let mut expected = state.clone();
        let at = 0xffff_8880_0000_0000 + 0x1000 + 12 + 0x10;
        let mut memory = BitsMemory::new(&code);
        set_word(&mut memory, at, 0x0040);
        run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(at, 2)]);
        assert!(memory.writes.is_empty());
        // 0x0040 & 0x0040 = 0x40: not zero, not negative, one bit set (PF = 0).
        expected.rflags &= !TEST8_FLAGS;
        expected.rip += 8;
        assert_eq!(state, expected);
    }
}

#[test]
fn fs_test_word_absolute_sib_disp32_without_base_or_index_adds_only_the_fs_base() {
    // `testw $0x0001, %fs:0x20` = 64 66 f7 04 25 20 00 00 00 01 00.
    let mut state = board_state();
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = BitsMemory::new(&[0x64, 0x66, 0xf7, 0x04, 0x25, 0x20, 0, 0, 0, 0x01, 0x00]);
    set_word(&mut memory, 0xffff_8880_0000_0020, 0x0001);
    run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_0020, 2)]);
    assert_eq!(state.rip, RIP + 11);
    assert_eq!(state.rflags & TEST8_FLAGS, 0);
}

#[test]
fn test_word_with_rex_b_uses_r8_to_r15_as_the_base_and_changes_no_register() {
    // `testw $0x8000, %gs:0x8(%r8..%r15)`: 65 66 41 f7 40+n 08 00 80 (REX.B, disp8) and with the
    // prefixes swapped.
    for prefixes in [[0x65u8, 0x66], [0x66, 0x65]] {
        for (n, register) in [
            (0u8, Register::R8),
            (1, Register::R9),
            (7, Register::R15),
            (5, Register::R13),
        ] {
            let mut state = board_state();
            state.gs_base = 0xffff_8880_0000_0000;
            state.gpr[register as usize] = 0x4000;
            let mut expected = state.clone();
            let mut memory = BitsMemory::new(&[
                prefixes[0],
                prefixes[1],
                0x41,
                0xf7,
                0x40 + n,
                0x08,
                0x00,
                0x80,
            ]);
            let at = 0xffff_8880_0000_4008;
            set_word(&mut memory, at, 0x8000);
            run_test16_set(&mut state, &mut memory).unwrap();
            assert_eq!(memory.reads, vec![(at, 2)], "{register:?}");
            assert!(memory.writes.is_empty());
            // 0x8000: SF = 1, ZF = 0, low byte 0 -> PF = 1.
            expected.rflags = (expected.rflags & !TEST8_FLAGS) | FLAG_SF | FLAG_PF;
            expected.rip += 8;
            assert_eq!(state, expected, "{register:?}");
        }
    }
    // R12 through a SIB byte: `testw $0x8000, %gs:0x8(%r12)` = 65 66 41 f7 44 24 08 00 80.
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.gpr[Register::R12 as usize] = 0x4000;
    let mut memory = BitsMemory::new(&[0x65, 0x66, 0x41, 0xf7, 0x44, 0x24, 0x08, 0x00, 0x80]);
    set_word(&mut memory, 0xffff_8880_0000_4008, 0x8000);
    run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.reads, vec![(0xffff_8880_0000_4008, 2)]);
    assert_eq!(state.rip, RIP + 9);
}

#[test]
fn test_word_with_rex_x_uses_r8_to_r15_as_the_index() {
    // `testw $3, %gs:0x10(%rbx,%r9,4)` = 65 66 42 f7 44 8b 10 03 00 (REX.X, SIB index = r9), and
    // with REX.X + REX.B (0x43) the base becomes r11: SIB base field 3 -> r11.
    for (rex, base_register, base_value) in [
        (0x42u8, Register::Rbx, 0x1000u64),
        (0x43, Register::R11, 0x2000),
    ] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.gpr[base_register as usize] = base_value;
        state.gpr[Register::R9 as usize] = 5;
        // A decoy: with the index wrongly taken as rcx the access would move.
        state.gpr[Register::Rcx as usize] = 0x9_0000;
        let mut expected = state.clone();
        let at = 0xffff_8880_0000_0000 + base_value + 20 + 0x10;
        let mut memory = BitsMemory::new(&[0x65, 0x66, rex, 0xf7, 0x44, 0x8b, 0x10, 0x03, 0x00]);
        set_word(&mut memory, at, 0x0007);
        run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(memory.reads, vec![(at, 2)], "{rex:#x}");
        // 0x0007 & 0x0003 = 3: two bits -> PF = 1.
        expected.rflags = (expected.rflags & !TEST8_FLAGS) | FLAG_PF;
        expected.rip += 9;
        assert_eq!(state, expected, "{rex:#x}");
    }
}

#[test]
fn test_word_accepts_a_rex_directly_before_the_opcode_without_w() {
    // REX.R does not change the word operand; the rest of the instruction is identical.
    for rex in [0x40u8, 0x44] {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(&[0x65, 0x66, rex, 0xf7, 0x03, 0xff, 0xff]);
        set_word(&mut memory, 0xffff_8880_0020_0000, 0x0000);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{rex:#x}");
        assert_eq!(memory.reads, vec![(0xffff_8880_0020_0000, 2)], "{rex:#x}");
        assert_eq!(state.rip, RIP + 7, "{rex:#x}");
        assert_eq!(state.gpr, before.gpr, "{rex:#x}");
        assert_eq!(state.rflags & TEST8_FLAGS, FLAG_ZF | FLAG_PF, "{rex:#x}");
    }
}

#[test]
fn test_word_never_writes_memory_even_on_a_read_only_page() {
    // The adapter fails every write; the TEST must still retire and leave the word intact.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_0000),
    };
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = BitsMemory::new(&[0x65, 0x66, 0xf7, 0x03, 0x01, 0x00]);
    memory.write_fault = Some(fault);
    set_word(&mut memory, 0x20_0000, 0x5555);
    let result = run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.data.get(&0x20_0000), Some(&0x55));
    assert_eq!(memory.data.get(&0x20_0001), Some(&0x55));
}

#[test]
fn test_word_non_canonical_address_is_gp0_without_mutation_or_read() {
    for (gs, base) in [
        (true, 0x0000_8000_0000_0000u64),
        (true, 0x7fff_ffff_ffff_fff0),
        // The effective address itself lands one past the canonical boundary.
        (true, 0x0000_8000_0000_0000 - GS_TEST16_DISP - 10 - RIP),
        // The start is the last canonical byte: the second byte of the word is not canonical.
        (true, 0x0000_7fff_ffff_ffff - GS_TEST16_DISP - 10 - RIP),
        (false, 0x0000_8000_0000_0000),
    ] {
        let mut state = board_state();
        state.gs_base = base;
        state.fs_base = base;
        state.rflags |= FLAG_CF | FLAG_ZF;
        let before = state.clone();
        let code = if gs {
            GS_TEST16_BOUNDARY
        } else {
            FS_TEST16_RIP
        };
        let mut memory = BitsMemory::new(&code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{gs} {base:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{gs} {base:#x}");
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn test_word_last_word_with_both_bytes_canonical_is_read() {
    // The word at 0x7fff_ffff_fffe covers the last two canonical bytes; one byte further is #GP.
    let mut state = board_state();
    state.gs_base = 0x0000_7fff_ffff_fffe - GS_TEST16_DISP - 10 - RIP;
    let mut memory = BitsMemory::new(&GS_TEST16_BOUNDARY);
    let result = run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x0000_7fff_ffff_fffe, 2)]);
}

#[test]
fn gs_test_word_through_rsp_or_rbp_base_is_gp0_not_ss_when_non_canonical() {
    // `testw $1, %gs:(%rsp)` = 65 66 f7 04 24 01 00 and `testw $1, %gs:0x0(%rbp)` =
    // 65 66 f7 45 00 01 00, in both prefix orders.
    for (code, register) in [
        (
            &[0x65u8, 0x66, 0xf7, 0x04, 0x24, 0x01, 0x00][..],
            Register::Rsp,
        ),
        (
            &[0x65u8, 0x66, 0xf7, 0x45, 0x00, 0x01, 0x00][..],
            Register::Rbp,
        ),
        (
            &[0x66u8, 0x65, 0xf7, 0x04, 0x24, 0x01, 0x00][..],
            Register::Rsp,
        ),
        (
            &[0x66u8, 0x65, 0xf7, 0x45, 0x00, 0x01, 0x00][..],
            Register::Rbp,
        ),
    ] {
        let mut state = board_state();
        state.gs_base = 0x0000_8000_0000_0000;
        state.gpr[register as usize] = 0;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
    }
}

#[test]
fn test_word_read_fault_leaves_flags_rip_and_registers_unchanged() {
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x0),
        linear_address: Some(0xffff_8880_0000_0000 + RIP + 10 + GS_TEST16_DISP),
    };
    let mut state = board_state();
    state.gs_base = 0xffff_8880_0000_0000;
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_ZF;
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_TEST16_BOUNDARY);
    memory.read_fault = Some(fault.clone());
    let result = run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn segment_test_word_forms_outside_the_slice_are_refused_with_the_exact_opcode() {
    let cases: [&[u8]; 36] = [
        // Other F7 group members with a segment base stay outside this slice (both orders).
        &[0x65, 0x66, 0xf7, 0x0b, 0x01, 0x00], // /1 (TEST alias)
        &[0x65, 0x66, 0xf7, 0x13],             // NOT
        &[0x66, 0x65, 0xf7, 0x13],
        &[0x65, 0x66, 0xf7, 0x1b], // NEG
        &[0x64, 0x66, 0xf7, 0x23], // MUL
        &[0x66, 0x64, 0xf7, 0x23],
        &[0x64, 0x66, 0xf7, 0x2b], // IMUL
        &[0x65, 0x66, 0xf7, 0x33], // DIV
        &[0x65, 0x66, 0xf7, 0x3b], // IDIV
        // The dword and qword TEST (F7) with a segment prefix stay refused.
        &[0x65, 0xf7, 0x03, 0x01, 0, 0, 0],
        &[0x65, 0x48, 0xf7, 0x03, 0x01, 0, 0, 0],
        // REX.W together with 0x66: refused rather than guessing the precedence.
        &[0x65, 0x66, 0x48, 0xf7, 0x03, 0x01, 0, 0, 0],
        &[0x66, 0x65, 0x48, 0xf7, 0x03, 0x01, 0, 0, 0],
        &[0x64, 0x66, 0x4c, 0xf7, 0x03, 0x01, 0, 0, 0],
        // LOCK, REP, REPNE.
        &[0xf0, 0x65, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0xf0, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x66, 0xf0, 0xf7, 0x03, 0x01, 0x00],
        &[0xf3, 0x66, 0x65, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x66, 0xf3, 0xf7, 0x03, 0x01, 0x00],
        &[0xf2, 0x64, 0x66, 0xf7, 0x03, 0x01, 0x00],
        // 0x67.
        &[0x65, 0x66, 0x67, 0xf7, 0x03, 0x01, 0x00],
        &[0x67, 0x66, 0x64, 0xf7, 0x03, 0x01, 0x00],
        // A second segment prefix, and a repeated 0x66.
        &[0x64, 0x65, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x65, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x66, 0x64, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x66, 0x66, 0xf7, 0x03, 0x01, 0x00],
        // REX not immediately before the opcode (stale).
        &[0x44, 0x65, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x48, 0x66, 0x64, 0xf7, 0x03, 0x01, 0x00],
        &[0x65, 0x41, 0x66, 0xf7, 0x03, 0x01, 0x00],
        &[0x66, 0x41, 0x65, 0xf7, 0x03, 0x01, 0x00],
        &[0x41, 0x65, 0x66, 0x40, 0xf7, 0x03, 0x01, 0x00],
        // FS/GS absolute (SIB, no base) NOT/NEG with 0x66.
        &[0x64, 0x66, 0xf7, 0x14, 0x25, 0x20, 0, 0, 0],
        &[0x65, 0x66, 0xf7, 0x1c, 0x25, 0x20, 0, 0, 0],
        // RIP-relative MUL/DIV with 0x66.
        &[0x65, 0x66, 0xf7, 0x25, 0x34, 0x12, 0, 0],
        &[0x66, 0x64, 0xf7, 0x35, 0x34, 0x12, 0, 0],
        // RIP-relative /1 (TEST alias) with 0x66.
        &[0x65, 0x66, 0xf7, 0x0d, 0x34, 0x12, 0, 0, 0x01, 0x00],
    ];
    for code in cases {
        let mut state = board_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xf7 },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
    // The byte form keeps refusing 0x66 with the F6 opcode.
    for code in [
        &[0x66u8, 0x65, 0xf6, 0x03, 0x01][..],
        &[0x65, 0x66, 0xf6, 0x03, 0x01][..],
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf6 });
        assert_eq!(state, before);
        assert!(memory.reads.is_empty());
    }
}

#[test]
fn test_word_register_form_and_plain_form_keep_their_old_behavior() {
    // The register form (mod = 11) is not memory: the segment prefix is irrelevant and it keeps
    // decoding as the plain `TEST r16, imm16` (unchanged by S4-38L7r). The memory form without a
    // segment prefix keeps ignoring the FS/GS bases.
    for code in [
        &[0x66u8, 0xf7, 0xc3, 0x01, 0x00][..],     // test $1, %bx
        &[0x65, 0x66, 0xf7, 0xc3, 0x01, 0x00][..], // gs-prefixed register form
        &[0x66, 0x64, 0xf7, 0xc3, 0x01, 0x00][..],
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0xabcd_0000;
        let mut memory = BitsMemory::new(code);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert_ne!(state.rflags & FLAG_ZF, 0, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
    }
    // The register form with a non-zero word: %bx = 0x8001 & 0x8000 -> 0x8000: SF, PF.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x8001;
    let mut memory = BitsMemory::new(&[0x65, 0x66, 0xf7, 0xc3, 0x00, 0x80]);
    run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(state.rflags & TEST8_FLAGS, FLAG_SF | FLAG_PF);

    let mut state = bts_state(0);
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut memory = BitsMemory::new(&[0x66, 0xf7, 0x03, 0x01, 0x00]); // testw $1, (%rbx)
    set_word(&mut memory, 0x20_0000, 0x0001);
    let result = run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 2)]);
    assert_eq!(state.rflags & FLAG_ZF, 0);

    // The dword form without a prefix is untouched: `testl $1, (%rbx)`.
    let mut state = bts_state(0);
    let mut memory = BitsMemory::new(&[0xf7, 0x03, 0x01, 0x00, 0x00, 0x00]);
    set_word(&mut memory, 0x20_0000, 0x0001);
    let result = run_test16_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_0000, 4)]);
}

#[test]
fn cs_ds_es_ss_overrides_on_a_test_word_keep_ignoring_the_segment_base() {
    for prefix in [0x2eu8, 0x3e, 0x36, 0x26] {
        let mut state = bts_state(0);
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let mut memory = BitsMemory::new(&[0x66, prefix, 0xf7, 0x03, 0x01, 0x00]);
        let result = run_test16_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{prefix:#x}");
        assert_eq!(memory.reads, vec![(0x20_0000, 2)], "{prefix:#x}");
    }
}

#[test]
fn segment_test_word_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the FS/GS form is refused without mutation.
    for code in [
        &GS_TEST16_BOUNDARY[..],
        &GS_TEST16_BOUNDARY_SWAPPED[..],
        &FS_TEST16_RIP[..],
        &FS_TEST16_RIP_SWAPPED[..],
    ] {
        let mut state = legacy_state();
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = BitsMemory::new(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xf7 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn segment_test_word_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = BitsMemory::new(&GS_TEST16_BOUNDARY);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7s: `LOCK OR r/m8, imm8` (`F0 80 /1 ib`, memory destination; SDM Vol.2 OR: destination
// | immediate, CF = OF = 0, SF/ZF/PF from the result, AF undefined; from memory, not re-checked
// against the SDM text). The dword/qword forms `F0 [REX.W] 81 /1 id` and `F0 [REX.W] 83 /1 ib`
// (sign-extended immediate) are decoded and executed the same way. ----

/// `lock orb $0x80, 0x3d(%rbx)` (the S4-38L7r diagnostic boundary bytes).
const LOCK_ORB_RBX_3D: [u8; 5] = [0xf0, 0x80, 0x4b, 0x3d, 0x80];
/// `lock orb $0x80, 0x1234(%rip)`.
const LOCK_ORB_RIP: [u8; 8] = [0xf0, 0x80, 0x0d, 0x34, 0x12, 0x00, 0x00, 0x80];

#[test]
fn lock_or_byte_boundary_form_reads_and_writes_one_byte_and_changes_only_rip_and_flags() {
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    // CF, OF and ZF are cleared by OR; AF is not defined by OR and stays as it was.
    state.rflags |= FLAG_CF | FLAG_OF | FLAG_ZF | FLAG_AF;
    let mut expected = state.clone();
    let mut memory = CasMemory::new(&LOCK_ORB_RBX_3D, 0x4141);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.reads, vec![(0x20_003d, 1)]);
    assert_eq!(memory.writes, vec![(0x20_003d, vec![0xc1u8])]);
    expected.rip += 5;
    // 0xc1 has three set bits: PF clear; SF set; ZF, CF and OF cleared; AF kept.
    expected.rflags = (expected.rflags & !ARITH) | FLAG_SF | FLAG_AF;
    assert_eq!(state, expected, "only RIP and the flags change");
}

#[test]
fn lock_or_byte_flags_follow_the_byte_result_on_boundary_values() {
    // (old byte, immediate, expected result, expected SF/ZF/PF).
    let cases: [(u8, u8, u8, u64); 6] = [
        (0x00, 0x00, 0x00, FLAG_ZF | FLAG_PF),
        (0x80, 0x80, 0x80, FLAG_SF),
        (0x01, 0x02, 0x03, FLAG_PF),
        (0xf0, 0x0f, 0xff, FLAG_SF | FLAG_PF),
        (0x00, 0x01, 0x01, 0),
        // The result equals the old value: the byte is still written back.
        (0xff, 0x00, 0xff, FLAG_SF | FLAG_PF),
    ];
    for (old, immediate, result_byte, flags) in cases {
        for initial in [2 | IF, 2 | IF | ARITH | (1 << 10)] {
            let mut state = board_state();
            state.rflags = initial;
            state.gpr[Register::Rbx as usize] = 0x20_0000;
            let code = [0xf0, 0x80, 0x4b, 0x3d, immediate];
            let mut memory = CasMemory::new(&code, u64::from(old));
            let result = run_cas_set(&mut state, &mut memory).unwrap();
            assert_eq!(result.instructions_executed, 1, "{old:#x}|{immediate:#x}");
            assert_eq!(
                memory.writes,
                vec![(0x20_003d, vec![result_byte])],
                "{old:#x}|{immediate:#x}: the write is unconditional"
            );
            // CF and OF are always clear; AF keeps its previous value; IF, DF, bit 1 survive.
            assert_eq!(
                state.rflags,
                (initial & !ARITH) | (initial & FLAG_AF) | flags,
                "{old:#x}|{immediate:#x} from {initial:#x}"
            );
        }
    }
}

#[test]
fn lock_or_byte_with_rip_relative_and_sib_displacement_addresses() {
    // The RIP-relative address is next RIP (all 8 bytes, the immediate included) + disp32.
    let mut state = board_state();
    let mut memory = CasMemory::new(&LOCK_ORB_RIP, 0x01);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = RIP + 8 + 0x1234;
    assert_eq!(memory.reads, vec![(at, 1)]);
    assert_eq!(memory.writes, vec![(at, vec![0x81u8])]);
    assert_eq!(state.rip, RIP + 8);
    // `lock orb $0x4, 0x10(%rbx,%rcx,4)` = f0 80 4c 8b 10 04.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x50_0000;
    state.gpr[Register::Rcx as usize] = 3;
    let mut memory = CasMemory::new(&[0xf0, 0x80, 0x4c, 0x8b, 0x10, 0x04], 0x01);
    run_cas_set(&mut state, &mut memory).unwrap();
    let at = 0x50_0000 + 12 + 0x10;
    assert_eq!(memory.reads, vec![(at, 1)]);
    assert_eq!(memory.writes, vec![(at, vec![0x05u8])]);
    assert_eq!(state.rip, RIP + 6);
}

#[test]
fn lock_or_byte_with_every_extended_base_register_r8_to_r15() {
    // `lock orb $0x10, 0x0(%rN)` via REX.B: mod = 01 with disp8 0; R12 needs a SIB byte.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let mut code = vec![0xf0, 0x41, 0x80];
        if i == 4 {
            code.extend([0x4c, 0x24, 0x00, 0x10]);
        } else {
            code.extend([0x48 | i as u8, 0x00, 0x10]);
        }
        let base = 0x40_0000 + (i as u64) * 0x1000;
        let mut state = board_state();
        state.gpr[register as usize] = base;
        let mut memory = CasMemory::new(&code, 0x01);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "base r{}", i + 8);
        assert_eq!(memory.reads, vec![(base, 1)], "base r{}", i + 8);
        assert_eq!(memory.writes, vec![(base, vec![0x11u8])], "base r{}", i + 8);
        assert_eq!(state.rip, RIP + code.len() as u64);
    }
}

#[test]
fn lock_or_byte_with_every_extended_index_register_r8_to_r15() {
    // `lock orb $0x10, 0x20(%rbx,%rN)` via REX.X; scale 1.
    for (i, register) in EXTENDED.into_iter().enumerate() {
        let code = [0xf0, 0x42, 0x80, 0x4c, ((i as u8) << 3) | 3, 0x20, 0x10];
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x40_0000;
        state.gpr[register as usize] = 0x100 * (i as u64 + 1);
        let mut memory = CasMemory::new(&code, 0x01);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "index r{}", i + 8);
        let at = 0x40_0000 + 0x100 * (i as u64 + 1) + 0x20;
        assert_eq!(memory.reads, vec![(at, 1)], "index r{}", i + 8);
        assert_eq!(memory.writes, vec![(at, vec![0x11u8])], "index r{}", i + 8);
        assert_eq!(state.rip, RIP + 7);
    }
}

#[test]
fn lock_or_rex_r_does_not_change_the_group_selector_and_writes_no_register() {
    // REX.R (0x44) is irrelevant for a group opcode: `f0 44 80 4b 3d 80` is still OR /1.
    for rex in [0x40u8, 0x44, 0x48] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gpr[Register::R9 as usize] = 0x1111;
        let mut expected = state.clone();
        let mut memory = CasMemory::new(&[0xf0, rex, 0x80, 0x4b, 0x3d, 0x80], 0x01);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{rex:#x}");
        assert_eq!(memory.reads, vec![(0x20_003d, 1)], "{rex:#x}");
        assert_eq!(memory.writes, vec![(0x20_003d, vec![0x81u8])], "{rex:#x}");
        expected.rip += 6;
        expected.rflags = (expected.rflags & !ARITH) | FLAG_SF | FLAG_PF;
        assert_eq!(state, expected, "{rex:#x}: no register changes");
    }
}

#[test]
fn lock_or_dword_and_qword_forms_use_the_extended_immediate() {
    // (code, old cell, expected written bytes, expected SF/ZF/PF, expected length).
    let cases: [(&[u8], u64, Vec<u8>, u64, u64); 5] = [
        // `lock orl $0x80000000, 0x3d(%rbx)`: imm32.
        (
            &[0xf0, 0x81, 0x4b, 0x3d, 0x00, 0x00, 0x00, 0x80],
            0x1234,
            0x8000_1234u32.to_le_bytes().to_vec(),
            FLAG_SF,
            8,
        ),
        // `lock orl $0xffffff80, 0x3d(%rbx)` from the sign-extended imm8 of 83 /1.
        (
            &[0xf0, 0x83, 0x4b, 0x3d, 0x80],
            0x1234,
            0xffff_ffb4u32.to_le_bytes().to_vec(),
            // The low byte 0xb4 has four set bits: PF.
            FLAG_SF | FLAG_PF,
            5,
        ),
        // `lock orq $-1, 0x3d(%rbx)`: imm8 -1 sign-extended to 64 bits.
        (
            &[0xf0, 0x48, 0x83, 0x4b, 0x3d, 0xff],
            0x1234,
            u64::MAX.to_le_bytes().to_vec(),
            FLAG_SF | FLAG_PF,
            6,
        ),
        // `lock orq $0x80000000, 0x3d(%rbx)`: imm32 sign-extended to 64 bits.
        (
            &[0xf0, 0x48, 0x81, 0x4b, 0x3d, 0x00, 0x00, 0x00, 0x80],
            0x1234,
            0xffff_ffff_8000_1234u64.to_le_bytes().to_vec(),
            FLAG_SF,
            9,
        ),
        // `lock orl $0x7f, 0x3d(%rbx)` on zero: the positive imm8 stays positive.
        (
            &[0xf0, 0x83, 0x4b, 0x3d, 0x7f],
            0,
            0x7fu32.to_le_bytes().to_vec(),
            0,
            5,
        ),
    ];
    for (code, old, written, flags, length) in cases {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.rflags |= FLAG_CF | FLAG_OF | FLAG_ZF;
        let mut memory = CasMemory::new(code, old);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(
            memory.reads,
            vec![(0x20_003d, written.len())],
            "{code:02x?}"
        );
        assert_eq!(memory.writes, vec![(0x20_003d, written)], "{code:02x?}");
        assert_eq!(state.rip, RIP + length, "{code:02x?}");
        assert_eq!(state.rflags & ARITH & !FLAG_AF, flags, "{code:02x?}");
    }
}

#[test]
fn lock_or_non_canonical_address_is_gp0_without_mutation_or_access() {
    for (rbx, code) in [
        (0x0000_8000_0000_0000u64 - 0x3d, &LOCK_ORB_RBX_3D[..]),
        (0xffff_7fff_ffff_ffffu64 - 0x3d, &LOCK_ORB_RBX_3D[..]),
        // The start is canonical but the dword's last byte crosses into the hole.
        (
            0x0000_7fff_ffff_fffe - 0x3d,
            &[0xf0, 0x83, 0x4b, 0x3d, 0x80][..],
        ),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = rbx;
        state.rflags |= FLAG_CF;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(GP0), "{rbx:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{rbx:#x}");
        assert!(
            memory.reads.is_empty() && memory.writes.is_empty(),
            "{rbx:#x}"
        );
        assert_eq!(memory.cell, 5);
    }
}

#[test]
fn lock_or_through_a_non_canonical_rsp_or_rbp_base_is_ss0() {
    // `lock orb $0x1, (%rsp)` = f0 80 0c 24 01 and `lock orb $0x1, 0x0(%rbp)` = f0 80 4d 00 01.
    for (register, code) in [
        (Register::Rsp, &[0xf0, 0x80, 0x0c, 0x24, 0x01][..]),
        (Register::Rbp, &[0xf0, 0x80, 0x4d, 0x00, 0x01][..]),
    ] {
        let mut state = board_state();
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_or_write_fault_leaves_flags_rip_and_memory_unchanged() {
    // A read-only page: the always-performed write faults, even when the result equals the old byte.
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(0x20_003d),
    };
    for (old, code) in [
        (0x01u64, &LOCK_ORB_RBX_3D[..]),
        (0x80, &LOCK_ORB_RBX_3D[..]),
        (0x1234, &[0xf0, 0x83, 0x4b, 0x3d, 0x80][..]),
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.rflags |= FLAG_CF | FLAG_OF;
        let before = state.clone();
        let mut memory = CasMemory::new(code, old);
        memory.write_fault = Some(fault.clone());
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(exit(&result), &RunExit::Fault(fault.clone()), "{old:#x}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{old:#x}");
        assert_eq!(memory.cell, old);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn lock_or_forms_outside_the_slice_are_refused_with_the_group_opcode_without_mutation() {
    let cases: [(&[u8], u8); 47] = [
        // The other group members with LOCK (ADD, ADC, SBB, AND, SUB, XOR, CMP): byte form.
        (&[0xf0, 0x80, 0x43, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x53, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x5b, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x63, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x6b, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x73, 0x3d, 0x01], 0x80),
        (&[0xf0, 0x80, 0x7b, 0x3d, 0x01], 0x80),
        // The same members in the dword/qword forms.
        (&[0xf0, 0x81, 0x43, 0x3d, 1, 0, 0, 0], 0x81),
        (&[0xf0, 0x81, 0x63, 0x3d, 1, 0, 0, 0], 0x81),
        (&[0xf0, 0x48, 0x81, 0x73, 0x3d, 1, 0, 0, 0], 0x81),
        (&[0xf0, 0x83, 0x43, 0x3d, 0x01], 0x83),
        (&[0xf0, 0x83, 0x6b, 0x3d, 0x01], 0x83),
        (&[0xf0, 0x48, 0x83, 0x7b, 0x3d, 0x01], 0x83),
        // Register forms (mod = 11) of OR.
        (&[0xf0, 0x80, 0xcb, 0x01], 0x80),
        (&[0xf0, 0x41, 0x80, 0xc8, 0x01], 0x80),
        (&[0xf0, 0x81, 0xcb, 1, 0, 0, 0], 0x81),
        (&[0xf0, 0x83, 0xcb, 0x01], 0x83),
        // 0x66 (word) and 0x67.
        (&[0x66, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x66, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x67, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x66, 0x81, 0x4b, 0x3d, 0x00, 0x80], 0x81),
        (&[0xf0, 0x66, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf0, 0x67, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        // REP/REPNE instead of LOCK, or alongside it, and a repeated LOCK.
        (&[0xf3, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf2, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0xf3, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf3, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0xf2, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0xf0, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf0, 0xf3, 0x81, 0x4b, 0x3d, 1, 0, 0, 0], 0x81),
        (&[0xf0, 0xf2, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf3, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf2, 0x81, 0x4b, 0x3d, 1, 0, 0, 0], 0x81),
        // Segment prefixes (LOCK with FS/GS and the others), in both orders.
        (&[0xf0, 0x64, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x65, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0x64, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0x65, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x2e, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x65, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0x65, 0xf0, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf0, 0x64, 0x81, 0x4b, 0x3d, 1, 0, 0, 0], 0x81),
        // REX not directly before the opcode.
        (&[0x48, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0x41, 0xf0, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0x48, 0xf0, 0x83, 0x4b, 0x3d, 0x80], 0x83),
        (&[0xf0, 0x48, 0x66, 0x80, 0x4b, 0x3d, 0x80], 0x80),
        (&[0xf0, 0x66, 0x48, 0x83, 0x4b, 0x3d, 0x80], 0x83),
    ];
    for (code, opcode) in cases {
        let mut state = board_state();
        state.gpr[Register::Rax as usize] = 5;
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_0000_0000;
        state.fs_base = 0xffff_8880_0000_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run_cas_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{code:02x?}: {result:?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert!(memory.reads.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn plain_or_memory_without_lock_keeps_its_existing_logic_path_in_the_set() {
    // `80 4b 3d 80`, `81 4b 3d ...` and `83 4b 3d ...` were already executed as an ordinary
    // logic OR; S4-38L7s does not narrow them. The other group members stay in the set too.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = CasMemory::new(&[0x80, 0x4b, 0x3d, 0x80], 0x01);
    let result = run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.cell, 0x81);
    assert_eq!(state.rip, RIP + 4);
    assert_eq!(
        state.rflags & (FLAG_SF | FLAG_ZF | FLAG_PF),
        FLAG_SF | FLAG_PF
    );

    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = CasMemory::new(&[0x83, 0x4b, 0x3d, 0x80], 0x1234);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.cell, 0xffff_ffb4);
    assert_eq!(state.rip, RIP + 4);

    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = CasMemory::new(&[0x81, 0x4b, 0x3d, 0x00, 0x00, 0x00, 0x80], 0x1234);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.cell, 0x8000_1234);
    assert_eq!(state.rip, RIP + 7);

    // `80 /0` (ADD) without LOCK: 0x01 + 0x01.
    let mut state = board_state();
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    let mut memory = CasMemory::new(&[0x80, 0x43, 0x3d, 0x01], 0x01);
    run_cas_set(&mut state, &mut memory).unwrap();
    assert_eq!(memory.cell, 0x02);
}

#[test]
fn lock_or_stays_unsupported_outside_the_set() {
    // No control policy (legacy state, bare `run`): the LOCK form is refused without mutation.
    for (code, opcode) in [
        (&LOCK_ORB_RBX_3D[..], 0x80u8),
        (&LOCK_ORB_RIP[..], 0x80),
        (&[0xf0, 0x83, 0x4b, 0x3d, 0x80][..], 0x83),
        (
            &[0xf0, 0x48, 0x81, 0x4b, 0x3d, 0x00, 0x00, 0x00, 0x80][..],
            0x81,
        ),
    ] {
        let mut state = legacy_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        let before = state.clone();
        let mut memory = CasMemory::new(code, 5);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode }, "{code:02x?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.reads.is_empty() && memory.writes.is_empty());
    }
}

#[test]
fn lock_or_is_not_reachable_from_cpl3() {
    // Set admission rejects CPL > 0 before any fetch.
    let mut state = board_state();
    let before = state.clone();
    let mut memory = CasMemory::new(&LOCK_ORB_RBX_3D, 5);
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
    assert!(memory.reads.is_empty() && memory.writes.is_empty());
}

// ---- S4-38L7v: repeated CS/DS/ES/SS segment overrides (`26`/`2E`/`36`/`3E`) before an ordinary
// opcode. In 64-bit mode these bases are zero, so a repeat is redundant padding (the kernel pads
// alternatives with `2E 2E 2E 31 C0`). SDM Vol.2 2.1.1 / Vol.3 3.4.4 cited from memory, not
// checked against the primary source. Repeats involving FS/GS, and any repeat of 66/67/LOCK/REP,
// stay refused. ----

const NOOP_SEGMENT_PREFIXES: [u8; 4] = [0x2e, 0x3e, 0x26, 0x36];
const XOR_EAX_EAX: [u8; 2] = [0x31, 0xc0];

fn run_one_in_set(
    state: &mut LongModeState,
    memory: &mut Memory,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_set(state, memory, &base(), 1)
}

fn assert_xor_eax_eax_executed(code: &[u8]) {
    let mut state = board_state();
    state.rflags |= 0x1 | 0x800 | 0x10; // CF, OF and AF set beforehand
    state.gpr[Register::Rax as usize] = 0xdead_beef_1111_2222;
    state.gpr[Register::Rbx as usize] = 0x20_0000;
    state.gs_base = 0xffff_8880_0000_0000;
    state.fs_base = 0xffff_8880_0000_0000;
    let mut expected = state.clone();
    let mut memory = Memory::code(code);
    let result = run_one_in_set(&mut state, &mut memory).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::InstructionBudgetExhausted,
        "{code:02x?}"
    );
    assert_eq!(result.instructions_executed, 1, "{code:02x?}");
    expected.gpr[Register::Rax as usize] = 0;
    expected.rip = RIP + code.len() as u64;
    assert_eq!(state.rip, expected.rip, "{code:02x?}");
    assert_eq!(state.gpr, expected.gpr, "{code:02x?}");
    // ZF = 1, PF = 1, CF = 0, OF = 0 (and SF = 0); the other flag bits are untouched.
    assert_eq!(
        state.rflags & (ARITHMETIC & !0x10),
        0x40 | 0x4,
        "{code:02x?}"
    );
    assert_eq!(state.rflags & !ARITHMETIC, expected.rflags & !ARITHMETIC);
}

#[test]
fn repeated_cs_ds_es_ss_overrides_before_xor_are_ignored() {
    // The kernel's boundary instruction: `cs cs cs xor %eax, %eax`.
    assert_xor_eax_eax_executed(&[0x2e, 0x2e, 0x2e, 0x31, 0xc0]);
    assert_xor_eax_eax_executed(&[0x2e, 0x2e, 0x31, 0xc0]);
    // A single prefix keeps working as before, and so does none.
    assert_xor_eax_eax_executed(&[0x2e, 0x31, 0xc0]);
    assert_xor_eax_eax_executed(&XOR_EAX_EAX);
    // Mixed CS/DS/ES/SS repeats.
    assert_xor_eax_eax_executed(&[0x3e, 0x36, 0x26, 0x2e, 0x31, 0xc0]);
    for prefix in NOOP_SEGMENT_PREFIXES {
        for count in 1..=6usize {
            let mut code = vec![prefix; count];
            code.extend_from_slice(&XOR_EAX_EAX);
            assert_xor_eax_eax_executed(&code);
        }
    }
}

#[test]
fn repeated_segment_overrides_before_a_register_mov_do_not_change_the_operation() {
    // `ds ds mov %rax, %rbx` = `3e 3e 48 89 c3`.
    let mut state = board_state();
    state.gpr[Register::Rax as usize] = 0x0123_4567_89ab_cdef;
    state.gpr[Register::Rbx as usize] = 0x5555;
    let flags = state.rflags;
    let mut memory = Memory::code(&[0x3e, 0x3e, 0x48, 0x89, 0xc3]);
    let result = run_one_in_set(&mut state, &mut memory).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rbx as usize], 0x0123_4567_89ab_cdef);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0123_4567_89ab_cdef);
    assert_eq!(state.rip, RIP + 5);
    assert_eq!(state.rflags, flags);
}

#[test]
fn repeated_segment_overrides_before_a_memory_load_keep_a_zero_base() {
    // `cs cs mov (%rbx), %eax`: the FS/GS bases must not be added; the address is rbx.
    for code in [
        &[0x2e, 0x2e, 0x8b, 0x03][..],
        &[0x3e, 0x36, 0x26, 0x2e, 0x8b, 0x03][..],
        &[0x2e, 0x8b, 0x03][..],
    ] {
        let mut state = board_state();
        state.gpr[Register::Rbx as usize] = 0x20_0000;
        state.gs_base = 0xffff_8880_1234_0000;
        state.fs_base = 0xffff_8880_4321_0000;
        let mut memory = LoadMemory::with_value(code, 0xaabb_ccdd_1122_3344);
        let result = run_load_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(memory.reads, vec![(0x20_0000, 4)], "{code:02x?}");
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0x1122_3344,
            "{code:02x?}"
        );
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
    }
}

#[test]
fn repeated_segment_override_length_limit_is_fifteen_bytes() {
    // 13 prefixes + `31 C0` = 15 bytes: accepted. 14 or more prefixes push the instruction
    // past 15 bytes: the 16th byte fetch is #GP(0), as for any over-long instruction.
    let mut code = vec![0x2e; 13];
    code.extend_from_slice(&XOR_EAX_EAX);
    assert_eq!(code.len(), 15);
    assert_xor_eax_eax_executed(&code);
    for count in [14usize, 15, 20] {
        let mut code = vec![0x2e; count];
        code.extend_from_slice(&XOR_EAX_EAX);
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(&code);
        let result = run_one_in_set(&mut state, &mut memory).unwrap();
        assert_eq!(result.instructions_executed, 0, "{count}");
        match exit(&result) {
            RunExit::Fault(fault) => {
                assert_eq!((fault.vector, fault.error_code), (13, Some(0)), "{count}")
            }
            other => panic!("{count}: expected #GP(0), got {other:?}"),
        }
        assert_eq!(state, before, "{count}");
    }
}

#[test]
fn repeated_fs_gs_and_non_segment_prefixes_stay_unsupported_before_xor() {
    let cases: &[&[u8]] = &[
        &[0x64, 0x64, 0x31, 0xc0],
        &[0x65, 0x65, 0x31, 0xc0],
        &[0x64, 0x65, 0x31, 0xc0],
        &[0x64, 0x2e, 0x31, 0xc0],
        &[0x2e, 0x64, 0x31, 0xc0],
        &[0x2e, 0x2e, 0x65, 0x31, 0xc0],
        &[0x65, 0x3e, 0x3e, 0x31, 0xc0],
        &[0x66, 0x66, 0x31, 0xc0],
        &[0x67, 0x67, 0x31, 0xc0],
        &[0xf0, 0xf0, 0x31, 0xc0],
        &[0xf3, 0xf3, 0x31, 0xc0],
        &[0xf2, 0xf2, 0x31, 0xc0],
        &[0xf2, 0xf3, 0x31, 0xc0],
        &[0x2e, 0x2e, 0x66, 0x66, 0x31, 0xc0],
        &[0x2e, 0x2e, 0x67, 0x67, 0x31, 0xc0],
        &[0x66, 0x2e, 0x2e, 0x66, 0x31, 0xc0],
    ];
    for code in cases {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run_one_in_set(&mut state, &mut memory).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x31 },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn repeated_segment_overrides_outside_the_set_behave_like_a_single_prefix() {
    for code in [&[0x2e, 0x31, 0xc0][..], &[0x2e, 0x2e, 0x2e, 0x31, 0xc0][..]] {
        let mut state = legacy_state();
        let mut memory = Memory::code(code);
        let result = run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
        assert_eq!(state.gpr[Register::Rax as usize], 0, "{code:02x?}");
    }
}

#[test]
fn repeated_cs_overrides_are_also_ignored_before_nop_and_prefetch_hints() {
    // S4-38L7v: the shared prefix policy changed `2E 2E 0F 1F 00` and `2E 2E 0F 18 0A` from
    // refused (duplicate segment) to accepted; both stay no-ops.
    for code in [
        &[0x2eu8, 0x2e, 0x0f, 0x1f, 0x00][..],
        &[0x2e, 0x2e, 0x0f, 0x18, 0x0a][..],
    ] {
        let mut state = endbr_state();
        let mut expected = state.clone();
        let mut memory = Memory::code(code);
        let result = run_set(&mut state, &mut memory, &base(), 1).unwrap();
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        expected.rip += code.len() as u64;
        assert_eq!(state, expected, "{code:02x?}");
    }
}
