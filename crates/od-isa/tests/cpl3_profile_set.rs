//! S4-39a: the CPL3 entry of the CPL0 profile set. Admission by the Ring3 state caches, the
//! fixed allow-list of user forms, and the unchanged CPL0 entry.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, RunConfig, RunExit, run_with_cpl0_profile_set, run_with_cpl3_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile,
    CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, GuestMemory, HaltProfile,
    InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiProfile,
    StiShadow, page_fault_error_code,
};

const RIP: u64 = 0x40_00b0;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;

#[derive(Clone)]
struct Memory {
    bytes: Vec<u8>,
    fetches: usize,
    accesses: usize,
    /// The code page is supervisor-only: a fetch whose page privilege is Ring3 is a #PF with U.
    supervisor_only_code: bool,
    /// The linear address of `bytes[0]` (`RIP` unless built with `code_at`).
    base: u64,
}

impl Memory {
    fn code(bytes: &[u8]) -> Self {
        Self::code_at(RIP, bytes)
    }

    fn code_at(base: u64, bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            fetches: 0,
            accesses: 0,
            supervisor_only_code: false,
            base,
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetches += 1;
        if self.supervisor_only_code && context.page_privilege() == PrivilegeLevel::Ring3 {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(page_fault_error_code(
                    AccessKind::Fetch,
                    context,
                    true,
                    false,
                    false,
                )),
                linear_address: Some(address),
            }));
        }
        let offset = address.checked_sub(self.base).unwrap_or(u64::MAX) as usize;
        self.bytes
            .get(offset)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        self.accesses += 1;
        Err(MemoryError::Backend(format!(
            "unexpected read {address:#x}"
        )))
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        self.accesses += 1;
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

fn config(privilege: PrivilegeLevel, budget: u64) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: None,
        context: AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        },
    }
}

/// The state an `IRETQ` to CPL3 would leave (S4-38L9H): CS 0x33 (DPL 3, L = 1), SS 0x2b (DPL 3).
fn user_state() -> LongModeState {
    LongModeState {
        rip: RIP,
        rflags: 2 | (1 << 9),
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        cs_selector: Some(0x33),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xfb,
            flags: 0x2,
        }),
        ss_selector: Some(0x2b),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xf3,
            flags: 0xc,
        }),
        ..LongModeState::default()
    }
}

fn run3(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
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

/// `lea 0x10(%rip), %rdi; mov $0x16d, %esi; mov $0x53, %eax; syscall`: the `/init` prefix before
/// its first `syscall` (`tests/fixtures/guest/stage4/initramfs/init.S`).
const INIT_PREFIX: [u8; 19] = [
    0x48, 0x8d, 0x3d, 0x10, 0x00, 0x00, 0x00, 0xbe, 0x6d, 0x01, 0x00, 0x00, 0xb8, 0x53, 0x00, 0x00,
    0x00, 0x0f, 0x05,
];

#[test]
fn init_prefix_runs_in_ring3_up_to_syscall() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let mut memory = Memory::code(&INIT_PREFIX);
    let result = run3(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + 17);
    assert_eq!(state.gpr[Register::Rdi as usize], RIP + 7 + 0x10);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x16d);
    assert_eq!(state.gpr[Register::Rax as usize], 0x53);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn endbr64_in_cpl3_retires_as_a_four_byte_nop() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let bytes = [0xf3, 0x0f, 0x1e, 0xfa];
    let mut initial = user_state();
    initial.gpr[Register::Rbx as usize] = 0x1234_5678;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    let mut state = initial.clone();
    let mut memory = Memory::code(&bytes);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    let mut expected = initial.clone();
    expected.rip += 4;
    assert_eq!(state, expected);
    assert_eq!(memory.accesses, 0);

    let mut state = initial.clone();
    let mut memory = Memory::code(&bytes[..3]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::MemoryError(MemoryError::Unmapped {
            address: RIP + 3,
            access: AccessKind::Fetch,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, initial);
    assert_eq!(memory.accesses, 0);

    for (neighbor, opcode) in [
        (&[0xf3, 0x0f, 0x1e, 0xfb][..], 0x1e),
        (&[0xf3, 0x90][..], 0x90),
    ] {
        let mut state = initial.clone();
        let mut memory = Memory::code(neighbor);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert_eq!(memory.accesses, 0);
    }
}

#[test]
fn budget_and_stop_address_are_honored_per_instruction() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let mut memory = Memory::code(&INIT_PREFIX);
    let result = run3(&mut state, &mut memory, &set, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 12);

    let mut state = user_state();
    let mut memory = Memory::code(&INIT_PREFIX);
    let mut stop = config(PrivilegeLevel::Ring3, 64);
    stop.stop_rip = Some(RIP + 7);
    let result = run_with_cpl3_profile_set(
        &mut state,
        &mut memory,
        stop,
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);

    let mut state = user_state();
    let mut memory = Memory::code(&INIT_PREFIX);
    let result = run3(&mut state, &mut memory, &set, 0).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state.rip, RIP);
}

fn assert_refused_without_change(
    mutate: impl FnOnce(&mut LongModeState),
    privilege: PrivilegeLevel,
) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    mutate(&mut state);
    let before = state.clone();
    let mut memory = Memory::code(&INIT_PREFIX);
    let error = run_with_cpl3_profile_set(
        &mut state,
        &mut memory,
        config(privilege, 64),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
    assert_eq!(memory.fetches, 0);
}

#[test]
fn admission_refuses_states_that_are_not_ring3() {
    let ring3 = PrivilegeLevel::Ring3;
    assert_refused_without_change(|state| state.cs_selector = Some(0x30), ring3);
    assert_refused_without_change(|state| state.cs_selector = None, ring3);
    assert_refused_without_change(|state| state.cs_cache = None, ring3);
    // DPL 0 in the CS cache.
    assert_refused_without_change(
        |state| state.cs_cache.as_mut().unwrap().access = 0x9b,
        ring3,
    );
    // L = 0 in the CS cache.
    assert_refused_without_change(|state| state.cs_cache.as_mut().unwrap().flags = 0x4, ring3);
    assert_refused_without_change(|state| state.ss_selector = Some(0x28), ring3);
    assert_refused_without_change(|state| state.ss_selector = None, ring3);
    assert_refused_without_change(|state| state.ss_cache = None, ring3);
    // A Ring0 access context is not the CPL3 entry's.
    assert_refused_without_change(|_| {}, PrivilegeLevel::Ring0);
}

#[test]
fn cpl0_entry_still_refuses_a_ring3_context() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&INIT_PREFIX);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 64),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
    assert_eq!(memory.fetches, 0);
}

#[test]
fn forbidden_forms_stop_as_unsupported_without_change() {
    let cli = CliProfile::NonPviLong64V1;
    let sti = StiProfile::NonPviLong64V1;
    let halt = HaltProfile::Long64V1;
    // Members that would execute these forms at CPL0 are enabled: the allow-list sits above them.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&cli)
        .with_sti(&sti)
        .with_halt(&halt)
        .with_int3()
        .with_pio_u8()
        .with_rdtsc();
    let forms: &[(&str, &[u8], u8)] = &[
        ("syscall", &[0x0f, 0x05], 0x05),
        ("mov rax, cr0", &[0x0f, 0x20, 0xc0], 0x20),
        ("mov cr3, rax", &[0x0f, 0x22, 0xd8], 0x22),
        ("mov cr4, rax", &[0x0f, 0x22, 0xe0], 0x22),
        ("hlt", &[0xf4], 0xf4),
        ("cli", &[0xfa], 0xfa),
        ("sti", &[0xfb], 0xfb),
        ("in al, dx", &[0xec], 0xec),
        ("out dx, al", &[0xee], 0xee),
        ("wrmsr", &[0x0f, 0x30], 0x30),
        ("rdmsr", &[0x0f, 0x32], 0x32),
        ("rdtsc", &[0x0f, 0x31], 0x31),
        ("swapgs", &[0x0f, 0x01, 0xf8], 0x01),
        ("iretq", &[0x48, 0xcf], 0xcf),
        ("lgdt [rax]", &[0x0f, 0x01, 0x10], 0x01),
        ("lidt [rax]", &[0x0f, 0x01, 0x18], 0x01),
        ("ltr ax", &[0x0f, 0x00, 0xd8], 0x00),
        ("lldt ax", &[0x0f, 0x00, 0xd0], 0x00),
        ("invlpg [rax]", &[0x0f, 0x01, 0x38], 0x01),
        ("int3", &[0xcc], 0xcc),
        // S2-51: CMPXCHG16B executes in the bare run and in the CPL0 set at Ring0 only; the CPL3
        // profile (the Linux live profile) does not list it, and CPUID declares no CX16 until S2-57.
        ("cmpxchg16b [rdi]", &[0x48, 0x0f, 0xc7, 0x0f], 0xc7),
        (
            "lock cmpxchg16b [rdi]",
            &[0xf0, 0x48, 0x0f, 0xc7, 0x0f],
            0xc7,
        ),
        // S2-52/S2-53: LAHF, SAHF and PREFETCHW execute in the bare run and in the CPL0 set at Ring0
        // only; the CPL3 profile (the Linux live profile) does not list them, and CPUID declares
        // neither LAHF_SAHF_64 nor PREFETCHW until S2-57.
        ("lahf", &[0x9f], 0x9f),
        ("sahf", &[0x9e], 0x9e),
        ("prefetchw [rdi]", &[0x0f, 0x0d, 0x0f], 0x0d),
        // S2-34 (m128 addressing): the general decode of the six raw 128-bit moves is never admitted at
        // CPL3; the live profile keeps exactly its own MOVUPS/MOVAPS/MOVDQA/MOVDQU forms.
        (
            "movaps xmm0,[rdi] (general form)",
            &[0x0f, 0x28, 0x07],
            0x28,
        ),
        ("movapd xmm0,[rdi]", &[0x66, 0x0f, 0x28, 0x07], 0x28),
        ("movapd [rdi],xmm0", &[0x66, 0x0f, 0x29, 0x07], 0x29),
        ("movdqa [rdi],xmm0", &[0x66, 0x0f, 0x7f, 0x07], 0x7f),
        ("movdqu [rdi],xmm0", &[0xf3, 0x0f, 0x7f, 0x07], 0x7f),
        // Forms outside the allow-list. `xor eax, eax` / `xor r10d, r10d` / `xor r8d, r8d` of
        // S4-39f2 and S6-22 allow register XOR `31 /r` at dword and qword widths.
        // Memory operands, word width, and other XOR opcodes retain their form-specific gates.
        ("xor qword [rax], rax (REX.W)", &[0x48, 0x31, 0x00], 0x31),
        (
            "xor qword [r10], r10 (REX.W+R+B)",
            &[0x4d, 0x31, 0x12],
            0x31,
        ),
        ("xor [rax], eax (memory)", &[0x31, 0x00], 0x31),
        (
            "xor [rip+0], r10d (memory)",
            &[0x44, 0x31, 0x15, 0, 0, 0, 0],
            0x31,
        ),
        ("xor ax, ax (0x66)", &[0x66, 0x31, 0xc0], 0x31),
        ("lock xor [rax], eax", &[0xf0, 0x31, 0x00], 0x31),
        ("xor eax, ebx (33 /r)", &[0x33, 0xc3], 0x33),
        ("xor eax, imm32 (35)", &[0x35, 1, 0, 0, 0], 0x35),
        ("xor eax, imm8 (83 /6)", &[0x83, 0xf0, 0x05], 0x83),
        // S4-39f3: `REX.W C7 /0` in the register form is allowed; the variants below stay outside.
        (
            "mov edi, imm32 (C7 /0, no REX.W)",
            &[0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
            0xc7,
        ),
        (
            "mov r8d, imm32 (C7 /0, REX.B without W)",
            &[0x41, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov word [rax], imm16 (66 C7 /0)",
            &[0x66, 0xc7, 0x00, 0x01, 0x00],
            0xc7,
        ),
        (
            "C7 memory form with reserved /1 extension",
            &[0xc7, 0x08, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "addr32 mov qword [eip+0], imm32 (67 REX.W C7 /0)",
            &[0x67, 0x48, 0xc7, 0x05, 0, 0, 0, 0, 0x01, 0, 0, 0],
            0xc7,
        ),
        (
            "mov ax, imm16 (66 C7 /0)",
            &[0x66, 0xc7, 0xc0, 0x01, 0x00],
            0xc7,
        ),
        (
            "mov rdi, imm32 with 0x66 (66 REX.W C7 /0)",
            &[0x66, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov rdi, imm32 with fs prefix",
            &[0x64, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov rdi, imm32 with address-size prefix",
            &[0x67, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov rdi, imm32 with rep prefix",
            &[0xf3, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        ("mov bl, imm8 (C6 /0)", &[0xc6, 0xc3, 0x01], 0xc6),
        // C7 with the other /digit values is not MOV (XABORT/XBEGIN family): decoder refusal.
        ("c7 /1", &[0x48, 0xc7, 0xc8, 0x01, 0x00, 0x00, 0x00], 0xc7),
        // Register-form XOR with prefixes (S4-39f2 review gap): every prefix other than REX
        // keeps `31 c0` outside the allow-list.
        ("fs xor eax, eax", &[0x64, 0x31, 0xc0], 0x31),
        ("gs xor eax, eax", &[0x65, 0x31, 0xc0], 0x31),
        ("addr32 xor eax, eax", &[0x67, 0x31, 0xc0], 0x31),
        ("repne xor eax, eax", &[0xf2, 0x31, 0xc0], 0x31),
        ("rep xor eax, eax", &[0xf3, 0x31, 0xc0], 0x31),
        ("lock xor eax, eax (register)", &[0xf0, 0x31, 0xc0], 0x31),
        // S4-39f4/S4-39f9: `85 /r` in the register form (with or without REX.W) is allowed; the
        // variants below stay outside. Prefixes keep the dword register form outside too.
        ("66 test bl, cl (66 84 /r)", &[0x66, 0x84, 0xc1], 0x84),
        ("lock test eax, eax", &[0xf0, 0x85, 0xc0], 0x85),
        ("fs test eax, eax", &[0x64, 0x85, 0xc0], 0x85),
        ("gs test eax, eax", &[0x65, 0x85, 0xc0], 0x85),
        ("cs-hint test eax, eax", &[0x2e, 0x85, 0xc0], 0x85),
        ("ds-hint test eax, eax", &[0x3e, 0x85, 0xc0], 0x85),
        ("addr32 test eax, eax", &[0x67, 0x85, 0xc0], 0x85),
        ("repne test eax, eax", &[0xf2, 0x85, 0xc0], 0x85),
        ("rep test eax, eax", &[0xf3, 0x85, 0xc0], 0x85),
        ("66 REX test r8w, r9w", &[0x66, 0x45, 0x85, 0xc8], 0x85),
        (
            "test [rip+0], eax (85 /r, RIP-relative)",
            &[0x85, 0x05, 0, 0, 0, 0],
            0x85,
        ),
        (
            "test [r8], r9d (REX.RB 85 /r, memory)",
            &[0x45, 0x85, 0x08],
            0x85,
        ),
        ("test [rax], eax (85 /r, memory)", &[0x85, 0x00], 0x85),
        (
            "test [rax], rax (REX.W 85 /r, memory)",
            &[0x48, 0x85, 0x00],
            0x85,
        ),
        (
            "test [rip+0], rax (REX.W 85 /r, RIP-relative)",
            &[0x48, 0x85, 0x05, 0, 0, 0, 0],
            0x85,
        ),
        (
            "test rax, rax with 66 prefix (66 REX.W 85 /r)",
            &[0x66, 0x48, 0x85, 0xc0],
            0x85,
        ),
        ("lock test rax, rax", &[0xf0, 0x48, 0x85, 0xc0], 0x85),
        ("fs test rax, rax", &[0x64, 0x48, 0x85, 0xc0], 0x85),
        ("addr32 test rax, rax", &[0x67, 0x48, 0x85, 0xc0], 0x85),
        ("repne test rax, rax", &[0xf2, 0x48, 0x85, 0xc0], 0x85),
        ("rep test rax, rax", &[0xf3, 0x48, 0x85, 0xc0], 0x85),
        // S4-39f17: `84 c0` (register form) executes; the memory form stays outside.
        ("test [rax], al (84 /r, memory)", &[0x84, 0x00], 0x84),
        ("test [rbx], al (84 /r, memory)", &[0x84, 0x03], 0x84),
        (
            "test [rip+0], al (84 /r, RIP-relative)",
            &[0x84, 0x05, 0, 0, 0, 0],
            0x84,
        ),
        (
            "test [r8+rcx], al (REX.XB 84 /r)",
            &[0x43, 0x84, 0x04, 0x08],
            0x84,
        ),
        (
            "test rax, imm32 (REX.W A9)",
            &[0x48, 0xa9, 1, 0, 0, 0],
            0xa9,
        ),
        // S4-39f3 review gap: C7 /0 with the remaining prefixes stays outside.
        (
            "gs mov rdi, imm32 (65 REX.W C7 /0)",
            &[0x65, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "repne mov rdi, imm32 (F2 REX.W C7 /0)",
            &[0xf2, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "lock mov rdi, imm32 (F0 REX.W C7 /0)",
            &[0xf0, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        // S4-39f4 review gap: `F7 /0` (TEST r/m64, imm32) and `A8` (TEST AL, imm8) stay outside.
        (
            "test rax, imm32 (REX.W F7 /0)",
            &[0x48, 0xf7, 0xc0, 1, 0, 0, 0],
            0xf7,
        ),
        ("addr32 test al, imm8 (67 A8)", &[0x67, 0xa8, 1], 0xa8),
        // S4-39f5: `[REX.W] 89 /r` in the register form is allowed; the variants below stay outside.
        ("mov eax, ebx (8B /r, registers)", &[0x8b, 0xc3], 0x8b),
        ("mov r12d, r8d (8B /r, REX.RB)", &[0x45, 0x8b, 0xe0], 0x8b),
        (
            "addr32 mov [eip+0], rax (67 REX.W 89 /r)",
            &[0x67, 0x48, 0x89, 0x05, 0, 0, 0, 0],
            0x89,
        ),
        (
            "mov bx, ax (66 REX.W 89 /r)",
            &[0x66, 0x48, 0x89, 0xc3],
            0x89,
        ),
        ("lock mov rbx, rax", &[0xf0, 0x48, 0x89, 0xc3], 0x89),
        ("fs mov rbx, rax", &[0x64, 0x48, 0x89, 0xc3], 0x89),
        ("gs mov rbx, rax", &[0x65, 0x48, 0x89, 0xc3], 0x89),
        ("addr32 mov rbx, rax", &[0x67, 0x48, 0x89, 0xc3], 0x89),
        ("repne mov rbx, rax", &[0xf2, 0x48, 0x89, 0xc3], 0x89),
        ("rep mov rbx, rax", &[0xf3, 0x48, 0x89, 0xc3], 0x89),
        ("mov bl, al (88 /r)", &[0x88, 0xc3], 0x88),
        ("mov rbx, rax (REX.W 8B /r)", &[0x48, 0x8b, 0xd8], 0x8b),
        // S4-39f5: Jcc rel32 is allowed without any prefix and without REX; the rest stays outside.
        // S4-39f7: the short Jcc is allowed without REX/prefix; LOOP rel8 and prefixed Jcc stay
        // (S4-39f13: JMP rel8 `EB` joined; its refusals are in the S4-39f13 tests below).
        ("rex jne rel8", &[0x40, 0x75, 0x00], 0x75),
        ("rex.w je rel8", &[0x48, 0x74, 0x00], 0x74),
        ("operand-size jne rel8", &[0x66, 0x75, 0x00], 0x75),
        ("addr32 jne rel8", &[0x67, 0x75, 0x00], 0x75),
        ("fs jne rel8", &[0x64, 0x75, 0x00], 0x75),
        ("repne jne rel8", &[0xf2, 0x75, 0x00], 0x75),
        // S4-39f7 review: LOOP/LOOPE/LOOPNE/JRCXZ rel8 and the branch hints before a short Jcc stay outside.
        ("loopne rel8 (E0)", &[0xe0, 0x00], 0xe0),
        ("loope rel8 (E1)", &[0xe1, 0x00], 0xe1),
        ("loop rel8 (E2)", &[0xe2, 0x00], 0xe2),
        ("jrcxz rel8 (E3)", &[0xe3, 0x00], 0xe3),
        ("cs-hint jne rel8", &[0x2e, 0x75, 0x00], 0x75),
        ("ds-hint je rel8", &[0x3e, 0x74, 0x00], 0x74),
        ("lock je rel8", &[0xf0, 0x74, 0x00], 0x74),
        // CPL3 `REX.W 39 /r` requires the default address size and no legacy prefix.
        (
            "cmp bx, r13w (66 REX.R 39 /r)",
            &[0x66, 0x44, 0x39, 0xeb],
            0x39,
        ),
        ("cmp bx, cx (66 39 /r)", &[0x66, 0x39, 0xcb], 0x39),
        (
            "addr32 cmp [ebx], rcx (67 REX.W 39 /r)",
            &[0x67, 0x48, 0x39, 0x0b],
            0x39,
        ),
        ("operand-size cmp rbx, rcx", &[0x66, 0x48, 0x39, 0xcb], 0x39),
        ("lock cmp rbx, rcx", &[0xf0, 0x48, 0x39, 0xcb], 0x39),
        ("rep cmp rbx, rcx", &[0xf3, 0x48, 0x39, 0xcb], 0x39),
        ("fs cmp rbx, rcx", &[0x64, 0x48, 0x39, 0xcb], 0x39),
        ("addr32 cmp rbx, rcx", &[0x67, 0x48, 0x39, 0xcb], 0x39),
        ("cmp rcx, rbx (REX.W 3B /r)", &[0x48, 0x3b, 0xcb], 0x3b),
        ("addr32 cmp bl, cl (67 38 /r)", &[0x67, 0x38, 0xcb], 0x38),
        ("cmp bx, imm8 (66 83 /7)", &[0x66, 0x83, 0xfb, 0x01], 0x83),
        // CPL3 keeps the 64-bit address-size restriction for `80 /7`.
        ("addr32 cmp bl, imm8", &[0x67, 0x80, 0xfb, 0x20], 0x80),
        ("REX.W jmp rel32", &[0x48, 0xe9, 0, 0, 0, 0], 0xe9),
        ("REX.W call rel32", &[0x48, 0xe8, 0, 0, 0, 0], 0xe8),
        ("rex.w je rel32", &[0x48, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        ("rex je rel32", &[0x40, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        (
            "operand-size je rel32",
            &[0x66, 0x0f, 0x84, 0, 0, 0, 0],
            0x84,
        ),
        ("addr32 je rel32", &[0x67, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        ("fs je rel32", &[0x64, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        ("repne je rel32", &[0xf2, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        ("rep je rel32", &[0xf3, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        ("lock je rel32", &[0xf0, 0x0f, 0x84, 0, 0, 0, 0], 0x84),
        // Prefixed variants of the allowed forms and a different operand size.
        (
            "addr32 mov rax, imm64",
            &[0x67, 0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0],
            0xb8,
        ),
        ("mov ax, imm16", &[0x66, 0xb8, 1, 0], 0xb8),
        ("lea ax, [rbx]", &[0x66, 0x8d, 0x03], 0x8d),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
    }
}

#[test]
fn allowed_forms_stop_at_the_first_forbidden_form_after_retiring() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    // mov $1, %ebx ; hlt
    let mut memory = Memory::code(&[0xbb, 0x01, 0x00, 0x00, 0x00, 0xf4]);
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 5);
    assert_eq!(state.gpr[Register::Rbx as usize], 1);
    assert_eq!(state.activity, Some(CpuActivity::Running));
}

#[test]
fn fetch_from_a_supervisor_page_is_a_user_page_fault() {
    // Model-level check of the od-isa side: the fetch of a CPL3 run carries the Ring3 page
    // privilege, so the error code built for it has P and U set (0x5). The real page walk is the
    // adapter's, outside this slice.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&INIT_PREFIX);
    memory.supervisor_only_code = true;
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x5),
            linear_address: Some(RIP),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

// ---- S4-39b: gaps of the S4-39a review (CPU_CONTRACT S4-39a, "Замечания ревью", item 4) ----

#[test]
fn rex_b_mov_imm32_and_rex_r_lea_are_allowed() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // `mov $0x12345678, %r8d` (41 b8) then `lea 0x10(%rip), %r14` (4c 8d 35): REX.R, as in `/init`.
    let mut state = user_state();
    state.gpr[8] = u64::MAX;
    let mut memory = Memory::code(&[
        0x41, 0xb8, 0x78, 0x56, 0x34, 0x12, 0x4c, 0x8d, 0x35, 0x10, 0x00, 0x00, 0x00,
    ]);
    let result = run3(&mut state, &mut memory, &set, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    // A 32-bit write zero-extends into R8.
    assert_eq!(state.gpr[8], 0x1234_5678);
    assert_eq!(state.gpr[14], RIP + 13 + 0x10);
    assert_eq!(state.rip, RIP + 13);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn lea_variants_outside_the_allowed_form_stop_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let disp = [0x10, 0x00, 0x00, 0x00];
    let forms: &[(&str, Vec<u8>)] = &[
        (
            "lea di, [rip+0x10] (66 8D)",
            [&[0x66, 0x8d, 0x3d][..], &disp].concat(),
        ),
        (
            "address-size prefix",
            [&[0x67, 0x48, 0x8d, 0x3d][..], &disp].concat(),
        ),
        (
            "fs segment prefix",
            [&[0x64, 0x48, 0x8d, 0x3d][..], &disp].concat(),
        ),
        (
            "cs segment prefix",
            [&[0x2e, 0x48, 0x8d, 0x3d][..], &disp].concat(),
        ),
        (
            "gs segment prefix before REX",
            [&[0x65, 0x4c, 0x8d, 0x35][..], &disp].concat(),
        ),
    ];
    for (name, bytes) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8d },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

// ---- S4-39f8: `REX.W 8D /r` (LEA r64, m) with a SIB operand ----

/// RFLAGS with IF, CF, PF, ZF, SF and OF set: LEA must leave every bit as it is.
const LEA_FLAGS: u64 = 2 | (1 << 9) | 0x1 | 0x4 | 0x40 | 0x80 | 0x800;

fn gpr(state: &LongModeState, index: usize) -> u64 {
    state.gpr[index]
}

#[test]
fn init_lea_r14_rbx_into_rdi_is_allowed() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // `49 8d 3c 1e` = `leaq (%r14,%rbx), %rdi` of `/init` at 0x400162, then a stop on the budget.
    let mut state = user_state();
    state.rflags = LEA_FLAGS;
    state.gpr[14] = 0x7000_1000;
    state.gpr[Register::Rbx as usize] = 0x20;
    state.gpr[Register::Rdi as usize] = u64::MAX;
    let mut memory = Memory::code(&[0x49, 0x8d, 0x3c, 0x1e, 0x90]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x7000_1020);
    assert_eq!(state.rip, RIP + 4);
    assert_eq!(state.rflags, LEA_FLAGS);
    assert_eq!(memory.accesses, 0);
    // The destination is the only register that changed.
    assert_eq!(state.gpr[14], 0x7000_1000);
    assert_eq!(state.gpr[Register::Rbx as usize], 0x20);
}

#[test]
fn lea_sib_forms_compute_the_address() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, destination register, expected value). Registers: rbx = 0x1000, rcx = 0x30,
    // rsi = 0x2000, rsp = 0x7fff_0000, rbp = 0x5000, r11 = 0x300, r12 = 0x4000, r13 = 0x6000,
    // r14 = 0x9000.
    let forms: &[(&str, &[u8], usize, u64)] = &[
        // base rsi, index rbx
        ("rsi+rbx", &[0x48, 0x8d, 0x3c, 0x1e], 7, 0x3000),
        // REX.X: index r11
        ("rsi+r11 (REX.X)", &[0x4a, 0x8d, 0x3c, 0x1e], 7, 0x2300),
        // REX.R: destination r15
        (
            "rsi+rbx into r15 (REX.R)",
            &[0x4c, 0x8d, 0x3c, 0x1e],
            15,
            0x3000,
        ),
        // REX.B: base r14
        ("r14+rbx (REX.B)", &[0x49, 0x8d, 0x3c, 0x1e], 7, 0xa000),
        // all four
        (
            "r14+r11 into r15 (REX.WRXB)",
            &[0x4f, 0x8d, 0x3c, 0x1e],
            15,
            0x9300,
        ),
        // scale 4, disp8
        (
            "rbx+rcx*4+0x10",
            &[0x48, 0x8d, 0x44, 0x8b, 0x10],
            0,
            0x1000 + 0xc0 + 0x10,
        ),
        // negative disp8
        (
            "rbx+rcx*4-0x10",
            &[0x48, 0x8d, 0x44, 0x8b, 0xf0],
            0,
            0x1000 + 0xc0 - 0x10,
        ),
        // scale 8, disp32
        (
            "rbx+rcx*8+0x100",
            &[0x48, 0x8d, 0x84, 0xcb, 0x00, 0x01, 0x00, 0x00],
            0,
            0x1000 + 0x180 + 0x100,
        ),
        // negative disp32
        (
            "rbx-0x100",
            &[0x48, 0x8d, 0x84, 0x23, 0x00, 0xff, 0xff, 0xff],
            0,
            0x1000 - 0x100,
        ),
        // index field 100 without REX.X: no index (the scale is irrelevant)
        ("rbx, no index", &[0x48, 0x8d, 0x04, 0x23], 0, 0x1000),
        (
            "rbx, no index, scale 8",
            &[0x48, 0x8d, 0x04, 0xe3],
            0,
            0x1000,
        ),
        // index field 100 with REX.X: r12 is a real index
        (
            "rbx+r12 (REX.X, index 100)",
            &[0x4a, 0x8d, 0x04, 0x23],
            0,
            0x5000,
        ),
        // base rsp
        ("rsp", &[0x48, 0x8d, 0x04, 0x24], 0, 0x7fff_0000),
        ("rsp+0x8", &[0x48, 0x8d, 0x44, 0x24, 0x08], 0, 0x7fff_0008),
        // base rbp needs mod != 00; mod = 01 makes it a base
        ("rbp+0x8", &[0x48, 0x8d, 0x44, 0x25, 0x08], 0, 0x5008),
        // mod = 00 with base field 101: disp32 without a base
        (
            "disp32 only",
            &[0x48, 0x8d, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00],
            0,
            0x1000,
        ),
        // ... also with an index
        (
            "rcx*2+disp32",
            &[0x48, 0x8d, 0x04, 0x4d, 0x00, 0x10, 0x00, 0x00],
            0,
            0x1000 + 0x60,
        ),
        // ... and with REX.B: the base field 101 still means "no base" at mod = 00
        (
            "disp32 only (REX.B)",
            &[0x49, 0x8d, 0x04, 0x25, 0x00, 0x20, 0x00, 0x00],
            0,
            0x2000,
        ),
        // REX.B turns base 100 into r12 and base 101 (mod = 01) into r13
        ("r12 (REX.B)", &[0x49, 0x8d, 0x04, 0x24], 0, 0x4000),
        (
            "r13+0x8 (REX.B)",
            &[0x49, 0x8d, 0x44, 0x25, 0x08],
            0,
            0x6008,
        ),
    ];
    for (name, bytes, dst, expected) in forms {
        let mut state = user_state();
        state.rflags = LEA_FLAGS;
        state.gpr[Register::Rbx as usize] = 0x1000;
        state.gpr[Register::Rcx as usize] = 0x30;
        state.gpr[Register::Rsi as usize] = 0x2000;
        state.gpr[Register::Rsp as usize] = 0x7fff_0000;
        state.gpr[Register::Rbp as usize] = 0x5000;
        state.gpr[11] = 0x300;
        state.gpr[12] = 0x4000;
        state.gpr[13] = 0x6000;
        state.gpr[14] = 0x9000;
        let mut before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        assert_eq!(gpr(&state, *dst), *expected, "{name}");
        assert_eq!(state.rip, RIP + bytes.len() as u64, "{name}");
        assert_eq!(state.rflags, LEA_FLAGS, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
        // Nothing but the destination and RIP changed.
        before.gpr[*dst] = *expected;
        before.rip = state.rip;
        assert_eq!(state, before, "{name}");
    }
}

#[test]
fn lea_sib_never_touches_memory_and_does_not_check_the_address() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // A non-canonical result (and a wrapping one) is just a value: no #GP, no #SS, no access.
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    state.gpr[Register::Rcx as usize] = u64::MAX;
    state.gpr[Register::Rsp as usize] = 0xffff_ffff_ffff_fff8;
    let mut memory = Memory::code(&[
        0x48, 0x8d, 0x04, 0x23, // lea (%rbx), %rax (no index)
        0x48, 0x8d, 0x14, 0x0b, // lea (%rbx,%rcx), %rdx = rbx - 1
        0x48, 0x8d, 0x5c, 0x24, 0x10, // lea 0x10(%rsp), %rbx (wraps to 8)
    ]);
    let result = run3(&mut state, &mut memory, &set, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0000_8000_0000_0000);
    assert_eq!(state.gpr[Register::Rdx as usize], 0x0000_7fff_ffff_ffff);
    assert_eq!(state.gpr[Register::Rbx as usize], 8);
    assert_eq!(state.rip, RIP + 13);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn lea_with_a_register_operand_is_invalid_opcode_without_change() {
    // `LEA` with mod = 11 is #UD (the decoder rejects it); the allow-list does not change that.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for bytes in [&[0x48, 0x8d, 0xc3][..], &[0x49, 0x8d, 0xfe][..]] {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.accesses, 0);
    }
}

#[test]
fn lock_before_a_raw_sse_memory_move_is_invalid_opcode_without_change() {
    // S2-34 (m128 addressing): LOCK on MOVAPS/MOVUPS/MOVAPD/MOVUPD/MOVDQA/MOVDQU with a memory
    // operand is #UD (SDM). Before S2-34 these stopped as `Unsupported`; the decoder now reports
    // `Invalid`, so the live CPL3 profile gives #UD, not a refusal. The allow-list is not consulted.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for bytes in [
        &[0xf0, 0x0f, 0x28, 0x07][..],       // lock movaps xmm0, [rdi]
        &[0xf0, 0x0f, 0x29, 0x07][..],       // lock movaps [rdi], xmm0
        &[0xf0, 0x0f, 0x10, 0x07][..],       // lock movups xmm0, [rdi]
        &[0xf0, 0x0f, 0x11, 0x07][..],       // lock movups [rdi], xmm0
        &[0x66, 0xf0, 0x0f, 0x28, 0x07][..], // lock movapd xmm0, [rdi]
        &[0xf0, 0x66, 0x0f, 0x7f, 0x07][..], // lock movdqa [rdi], xmm0
    ] {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            }),
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
        assert_eq!(memory.accesses, 0, "{bytes:02x?}");
    }
}

#[test]
fn lea_sib_variants_outside_the_allowed_form_stop_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8])] = &[
        ("word lea di, [r14+rbx]", &[0x66, 0x41, 0x8d, 0x3c, 0x1e]),
        ("operand-size prefix", &[0x66, 0x49, 0x8d, 0x3c, 0x1e]),
        ("address-size prefix", &[0x67, 0x49, 0x8d, 0x3c, 0x1e]),
        ("lock prefix", &[0xf0, 0x49, 0x8d, 0x3c, 0x1e]),
        ("repe prefix", &[0xf3, 0x49, 0x8d, 0x3c, 0x1e]),
        ("repne prefix", &[0xf2, 0x49, 0x8d, 0x3c, 0x1e]),
        ("fs segment prefix", &[0x64, 0x49, 0x8d, 0x3c, 0x1e]),
        ("gs segment prefix", &[0x65, 0x49, 0x8d, 0x3c, 0x1e]),
        ("cs segment prefix", &[0x2e, 0x49, 0x8d, 0x3c, 0x1e]),
        ("ds segment prefix", &[0x3e, 0x49, 0x8d, 0x3c, 0x1e]),
        ("es segment prefix", &[0x26, 0x49, 0x8d, 0x3c, 0x1e]),
        ("ss segment prefix", &[0x36, 0x49, 0x8d, 0x3c, 0x1e]),
        ("addr32 register-base LEA", &[0x67, 0x8d, 0x03]),
    ];
    for (name, bytes) in forms {
        let mut state = user_state();
        state.gpr[14] = 0x9000;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8d },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn mov_imm32_with_a_repeat_prefix_is_not_allowed() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for prefix in [0xf2_u8, 0xf3] {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(&[prefix, 0xb8, 0x01, 0x00, 0x00, 0x00]);
        let result = run3(&mut state, &mut memory, &set, 8).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xb8 },
            "{prefix:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn a_stop_request_ends_the_run_at_a_boundary_before_the_next_instruction() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // An always-true request: the first boundary stops, nothing retires, nothing changes.
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&INIT_PREFIX);
    let mut polls = 0_u32;
    let result = run_with_cpl3_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 64),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || {
            polls += 1;
            true
        },
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::StopRequested);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    assert_eq!(polls, 1);

    // A request that turns true at a later poll: the polls are counted across the single-step
    // runs (up to two per boundary), and the stop still arrives on an instruction boundary with
    // `rip` following exactly the retired instructions.
    for true_from in 2..=6_u32 {
        let mut state = user_state();
        let mut memory = Memory::code(&INIT_PREFIX);
        let mut polls = 0_u32;
        let result = run_with_cpl3_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 64),
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || {
                polls += 1;
                polls >= true_from
            },
        )
        .unwrap();
        assert_eq!(exit(&result), &RunExit::StopRequested, "poll {true_from}");
        let expected_rip =
            [RIP, RIP + 7, RIP + 12, RIP + 17][result.instructions_executed as usize];
        assert_eq!(state.rip, expected_rip, "poll {true_from}");
    }
}

// ---- S4-39f2: `XOR r/m32, r32` (31 /r), register form, and the `/init` path to the mount syscall ----

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const ARITH: u64 = CF | PF | AF | ZF | SF | OF;

#[test]
fn xor_r32_register_form_executes_with_flags_and_zero_extension() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (bytes, dst register index, initial dst, src register index, initial src, result, flags)
    // The destination is ModRM.rm (REX.B), the source is ModRM.reg (REX.R); a 32-bit write
    // zeroes bits 63:32 of the destination.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, usize, u64, u64, u64)] = &[
        // xor eax, ebx: 0x8000_0003 ^ 0x1 = 0x8000_0002 (SF, odd parity of 0x02 -> PF clear).
        (
            "31 d8",
            &[0x31, 0xd8],
            0,
            0x1234_5678_8000_0003,
            3,
            0xffff_ffff_0000_0001,
            0x8000_0002,
            SF,
        ),
        // xor r8d, r9d (REX.R + REX.B): 0xff ^ 0xf0 = 0x0f (four bits -> PF set).
        (
            "45 31 c8",
            &[0x45, 0x31, 0xc8],
            8,
            0xffff_ffff_0000_00ff,
            9,
            0x0000_0001_0000_00f0,
            0x0f,
            PF,
        ),
        // xor eax, r9d (REX.R only): equal low halves -> zero (ZF, PF).
        (
            "44 31 c8",
            &[0x44, 0x31, 0xc8],
            0,
            0xdead_beef_0000_0042,
            9,
            0x0000_0000_0000_0042,
            0,
            ZF | PF,
        ),
        // xor r10d, r10d (REX.R + REX.B, `/init`): zero idiom, upper half cleared.
        (
            "45 31 d2",
            &[0x45, 0x31, 0xd2],
            10,
            0xffff_ffff_ffff_ffff,
            10,
            0xffff_ffff_ffff_ffff,
            0,
            ZF | PF,
        ),
        // xor r8d, r8d (`/init`).
        (
            "45 31 c0",
            &[0x45, 0x31, 0xc0],
            8,
            0x8000_0000_8000_0000,
            8,
            0x8000_0000_8000_0000,
            0,
            ZF | PF,
        ),
        // xor eax, eax and xor edx, edx without REX.
        ("31 c0", &[0x31, 0xc0], 0, u64::MAX, 0, u64::MAX, 0, ZF | PF),
        (
            "31 d2",
            &[0x31, 0xd2],
            2,
            0x1_0000_0001,
            2,
            0x1_0000_0001,
            0,
            ZF | PF,
        ),
        // xor r8d, ecx (REX.B only): 0x7fff_ffff ^ 0 = 0x7fff_ffff (positive; low byte 0xff has
        // eight bits -> PF set).
        (
            "41 31 c8",
            &[0x41, 0x31, 0xc8],
            8,
            0x5555_5555_7fff_ffff,
            1,
            0x0000_0007_0000_0000,
            0x7fff_ffff,
            PF,
        ),
    ];
    for (name, bytes, dst, dst_value, src, src_value, expected, flags) in cases {
        let mut state = user_state();
        // CF, OF, SF, ZF, PF, AF all set before: XOR must clear CF/OF and recompute the rest.
        state.rflags |= ARITH;
        state.gpr[*dst] = *dst_value;
        if dst != src {
            state.gpr[*src] = *src_value;
        }
        let src_before = state.gpr[*src];
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        assert_eq!(state.rip, RIP + bytes.len() as u64, "{name}");
        assert_eq!(state.gpr[*dst], *expected, "{name}");
        if dst != src {
            assert_eq!(state.gpr[*src], src_before, "{name}");
        }
        let arithmetic = state.rflags & ARITH;
        // CF = OF = 0; SF, ZF, PF from the result; AF is not asserted (undefined).
        assert_eq!(arithmetic & (CF | OF), 0, "{name}");
        assert_eq!(arithmetic & (SF | ZF | PF), *flags, "{name}");
        // AF is undefined after XOR (SDM, from memory): it leaves the defined set, like OR/AND.
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !AF, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn xor_r32_leaves_the_other_registers_and_non_arithmetic_flags_unchanged() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    for (index, value) in state.gpr.iter_mut().enumerate() {
        *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
    }
    let mut expected = state.clone();
    let mut memory = Memory::code(&[0x45, 0x31, 0xd2]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    expected.gpr[10] = 0;
    expected.rip = RIP + 3;
    // IF (bit 9) and the reserved bit 1 stay; the arithmetic flags are recomputed.
    expected.rflags = (expected.rflags & !ARITH) | ZF | PF;
    assert_eq!(state.gpr, expected.gpr);
    assert_eq!(state.rip, expected.rip);
    assert_eq!(state.rflags & !AF, expected.rflags & !AF);
}

/// `/init` after the first `SYSCALL` (`mkdir`) up to the second one (`mount`, 165), taken from
/// the `/init` ELF (`work/stage4/initramfs/init.elf`, the same file as `/init` inside
/// `work/stage4/initramfs/initramfs.cpio`) at `0x4000c3`:
/// `lea 0x18c(%rip), %rdi; lea 0x17f(%rip), %rsi; lea 0x17e(%rip), %rdx; xor %r10d, %r10d;
/// xor %r8d, %r8d; mov $0xa5, %eax; syscall`.
const INIT_MOUNT_PATH: [u8; 33] = [
    0x48, 0x8d, 0x3d, 0x8c, 0x01, 0x00, 0x00, 0x48, 0x8d, 0x35, 0x7f, 0x01, 0x00, 0x00, 0x48, 0x8d,
    0x15, 0x7e, 0x01, 0x00, 0x00, 0x45, 0x31, 0xd2, 0x45, 0x31, 0xc0, 0xb8, 0xa5, 0x00, 0x00, 0x00,
    0x0f,
];

#[test]
fn init_path_after_mkdir_runs_in_ring3_up_to_the_mount_syscall() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut bytes = INIT_MOUNT_PATH.to_vec();
    bytes.push(0x05);
    let mut state = user_state();
    state.gpr[10] = 0xdead_beef_dead_beef;
    state.gpr[8] = 0xcafe_f00d_cafe_f00d;
    state.rflags |= ARITH;
    let mut memory = Memory::code(&bytes);
    let result = run3(&mut state, &mut memory, &set, 64).unwrap();
    // Without the `with_syscall` member the loop stops at `SYSCALL` (`Unsupported 0x05`).
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 6);
    assert_eq!(state.rip, RIP + 32);
    assert_eq!(state.gpr[Register::Rdi as usize], RIP + 7 + 0x18c);
    assert_eq!(state.gpr[Register::Rsi as usize], RIP + 14 + 0x17f);
    assert_eq!(state.gpr[Register::Rdx as usize], RIP + 21 + 0x17e);
    assert_eq!(state.gpr[10], 0);
    assert_eq!(state.gpr[8], 0);
    assert_eq!(state.gpr[Register::Rax as usize], 0xa5);
    assert_eq!(state.rflags & (CF | OF | SF), 0);
    assert_eq!(state.rflags & (ZF | PF), ZF | PF);
    assert_eq!(memory.accesses, 0);
}

// ---- S4-39f3: `REX.W C7 /0 id` (MOV r/m64, imm32), register form, and the path to `openat` ----

#[test]
fn mov_r64_imm32_sign_extends_and_leaves_everything_else_unchanged() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, destination register index, expected value)
    let cases: &[(&str, &[u8], usize, u64)] = &[
        // `/init` at 0x4000e5: movq $-0x64, %rdi.
        (
            "48 c7 c7 9c ff ff ff",
            &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
            7,
            0xffff_ffff_ffff_ff9c,
        ),
        ("movq $0, %rax", &[0x48, 0xc7, 0xc0, 0, 0, 0, 0], 0, 0),
        (
            "movq $0x7fffffff, %rcx",
            &[0x48, 0xc7, 0xc1, 0xff, 0xff, 0xff, 0x7f],
            1,
            0x0000_0000_7fff_ffff,
        ),
        (
            "movq $0x80000000 (as imm32), %rdx",
            &[0x48, 0xc7, 0xc2, 0x00, 0x00, 0x00, 0x80],
            2,
            0xffff_ffff_8000_0000,
        ),
        (
            "movq $-1, %rsp",
            &[0x48, 0xc7, 0xc4, 0xff, 0xff, 0xff, 0xff],
            4,
            u64::MAX,
        ),
        // REX.W + REX.B: r8..r15.
        (
            "49 c7 c0 imm32 (r8)",
            &[0x49, 0xc7, 0xc0, 0x9c, 0xff, 0xff, 0xff],
            8,
            0xffff_ffff_ffff_ff9c,
        ),
        (
            "49 c7 c7 imm32 (r15)",
            &[0x49, 0xc7, 0xc7, 0x78, 0x56, 0x34, 0x12],
            15,
            0x0000_0000_1234_5678,
        ),
        // S4-39f3 review gap: REX.W + REX.X with mod = 11 (X is ignored without a SIB byte).
        (
            "4a c7 c0 imm32 (REX.WX, rax)",
            &[0x4a, 0xc7, 0xc0, 0x9c, 0xff, 0xff, 0xff],
            0,
            0xffff_ffff_ffff_ff9c,
        ),
        // REX.W + REX.R (ignored by /0: ModRM.reg is the opcode extension) + REX.B.
        (
            "4d c7 c2 imm32 (REX.WRB, r10)",
            &[0x4d, 0xc7, 0xc2, 0x00, 0x00, 0x00, 0xf0],
            10,
            0xffff_ffff_f000_0000,
        ),
    ];
    for (name, bytes, dst, expected) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        // The arithmetic flags are all set before: MOV must not touch any flag.
        state.rflags |= ARITH;
        let mut expected_state = state.clone();
        expected_state.gpr[*dst] = *expected;
        expected_state.rip = RIP + bytes.len() as u64;
        assert_eq!(bytes.len(), 7, "{name}");
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        assert_eq!(state, expected_state, "{name}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn mov_r64_imm32_refusals_keep_the_state_when_a_later_form_is_forbidden() {
    // The allowed C7 form retires, the word memory form that follows stops the run before any write.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let mut memory = Memory::code(&[
        0x48, 0xc7, 0xc3, 0x05, 0x00, 0x00, 0x00, 0x66, 0xc7, 0x00, 0x01, 0x00,
    ]);
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xc7 });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 7);
    assert_eq!(state.gpr[Register::Rbx as usize], 5);
    assert_eq!(memory.accesses, 0);
}

/// `/init` at `0x4000e5` up to the `openat` syscall at `0x4000fd` (`init.elf`, `objdump -d`):
/// `movq $-0x64, %rdi; leaq 0x168(%rip), %rsi; xorl %edx, %edx; xorl %r10d, %r10d;
/// movl $0x101, %eax`, then `syscall`.
const INIT_OPENAT_PATH: [u8; 26] = [
    0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff, 0x48, 0x8d, 0x35, 0x68, 0x01, 0x00, 0x00, 0x31, 0xd2,
    0x45, 0x31, 0xd2, 0xb8, 0x01, 0x01, 0x00, 0x00, 0x0f, 0x05,
];

#[test]
fn init_path_after_mount_runs_in_ring3_up_to_the_openat_syscall() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = 0xdead_beef;
    state.gpr[10] = 0xdead_beef_dead_beef;
    state.rflags |= ARITH;
    let mut memory = Memory::code(&INIT_OPENAT_PATH);
    let result = run3(&mut state, &mut memory, &set, 64).unwrap();
    // Without the `with_syscall` member the loop stops at `SYSCALL` (`Unsupported 0x05`).
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 5);
    assert_eq!(state.rip, RIP + 24);
    assert_eq!(state.gpr[Register::Rdi as usize], 0xffff_ffff_ffff_ff9c);
    assert_eq!(state.gpr[Register::Rsi as usize], RIP + 14 + 0x168);
    assert_eq!(state.gpr[Register::Rdx as usize], 0);
    assert_eq!(state.gpr[10], 0);
    assert_eq!(state.gpr[Register::Rax as usize], 0x101);
    assert_eq!(state.rflags & (CF | OF | SF), 0);
    assert_eq!(state.rflags & (ZF | PF), ZF | PF);
    assert_eq!(memory.accesses, 0);
}

// ---- S4-39f4: `REX.W 85 /r` (TEST r/m64, r64), register form ----

#[test]
fn test_r64_register_form_sets_flags_and_writes_no_result() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, first register index, its value, second register index, its value, flags)
    // The flags are those of `first & second`; CF = OF = 0.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, usize, u64, u64)] = &[
        // `/init` at 0x4000ff: testq %rax, %rax right after the `openat` `SYSCALL` (the `read`
        // `SYSCALL` is at 0x40011f), here with a negative value (`openat` returned -errno).
        (
            "48 85 c0 negative",
            &[0x48, 0x85, 0xc0],
            0,
            0xffff_ffff_ffff_fff2,
            0,
            0xffff_ffff_ffff_fff2,
            SF,
        ),
        // Zero: ZF and PF.
        ("48 85 c0 zero", &[0x48, 0x85, 0xc0], 0, 0, 0, 0, ZF | PF),
        // Positive with an odd number of bits in the low byte: no flag.
        ("48 85 c0 positive", &[0x48, 0x85, 0xc0], 0, 1, 0, 1, 0),
        // The high half decides SF and ZF (a 32-bit test would see zero here).
        (
            "48 85 c0 high half only",
            &[0x48, 0x85, 0xc0],
            0,
            0x8000_0000_0000_0000,
            0,
            0x8000_0000_0000_0000,
            SF | PF,
        ),
        (
            "48 85 c0 bit 32 only",
            &[0x48, 0x85, 0xc0],
            0,
            0x1_0000_0000,
            0,
            0x1_0000_0000,
            PF,
        ),
        // testq %rbx, %rcx with disjoint bits: ZF, PF.
        (
            "48 85 d9 disjoint",
            &[0x48, 0x85, 0xd9],
            1,
            0xf0f0_f0f0_f0f0_f0f0,
            3,
            0x0f0f_0f0f_0f0f_0f0f,
            ZF | PF,
        ),
        // REX.WRB: testq %r9, %r10 (4d 85 ca): rm = r10, reg = r9.
        (
            "4d 85 ca REX.WRB",
            &[0x4d, 0x85, 0xca],
            10,
            0x8000_0000_0000_0003,
            9,
            0xc000_0000_0000_0001,
            SF,
        ),
        // REX.WR only: testq %r8, %rax (4c 85 c0): rm = rax, reg = r8.
        ("4c 85 c0 REX.WR", &[0x4c, 0x85, 0xc0], 0, 0x7, 8, 0x6, PF),
        // REX.WB only: testq %rax, %r15 (49 85 c7): rm = r15, reg = rax.
        (
            "49 85 c7 REX.WB",
            &[0x49, 0x85, 0xc7],
            15,
            0x100,
            0,
            0x100,
            // The low byte of 0x100 is zero (even parity: PF), SF and ZF clear.
            PF,
        ),
    ];
    for (name, bytes, a, a_value, b, b_value, flags) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        state.gpr[*a] = *a_value;
        state.gpr[*b] = *b_value;
        // CF, OF, SF, ZF, PF, AF all set before: TEST clears CF/OF and recomputes SF/ZF/PF.
        state.rflags |= ARITH;
        let mut expected = state.clone();
        expected.rip = RIP + bytes.len() as u64;
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        // No register changes (the result is not written), RIP advances by the length.
        assert_eq!(state.gpr, expected.gpr, "{name}");
        assert_eq!(state.rip, expected.rip, "{name}");
        let arithmetic = state.rflags & ARITH;
        assert_eq!(arithmetic & (CF | OF), 0, "{name}");
        assert_eq!(arithmetic & (SF | ZF | PF), *flags, "{name}");
        // The non-arithmetic flags (IF, the reserved bit 1) stay.
        assert_eq!(state.rflags & !ARITH, expected.rflags & !ARITH, "{name}");
        // AF is undefined after TEST (SDM, from memory): it leaves the defined set, like XOR.
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !AF, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn test_r64_allowed_then_forbidden_form_keeps_the_retired_state() {
    // `testq %rax, %rax` retires, the taken `js rel32` (S4-39f5: allowed) skips one byte and lands
    // on `mov eax, ebx` (8B register form), which stops the run.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xffff_ffff_ffff_fff2;
    let mut memory = Memory::code(&[
        0x48, 0x85, 0xc0, 0x0f, 0x88, 0x01, 0x00, 0x00, 0x00, 0x90, 0x8b, 0xc3,
    ]);
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8b });
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 10);
    assert_eq!(state.rflags & (SF | ZF | CF | OF), SF);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn test_r64_with_rex_x_in_the_register_form_executes() {
    // `4a 85 c0`: REX.WX; with mod = 11 there is no SIB, REX.X is ignored (SDM from memory).
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x8000_0000_0000_0000;
    state.rflags |= ARITH;
    let mut memory = Memory::code(&[0x4a, 0x85, 0xc0]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 3);
    assert_eq!(state.gpr[Register::Rax as usize], 0x8000_0000_0000_0000);
    assert_eq!(state.rflags & ARITH & (CF | OF | SF | ZF | PF), SF | PF);
    assert_eq!(memory.accesses, 0);
}

// ---- S4-39f9: `85 /r` without REX.W (TEST r/m32, r32), register form ----

#[test]
fn test_r32_register_form_sets_flags_and_writes_no_result() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, rm register index, its value, reg register index, its value, flags)
    // The flags are those of the 32-bit `rm & reg`; CF = OF = 0.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, usize, u64, u64)] = &[
        // `/init` at 0x40016b: testl %ecx, %ecx.
        ("85 c9 zero", &[0x85, 0xc9], 1, 0, 1, 0, ZF | PF),
        (
            "85 c9 negative dword",
            &[0x85, 0xc9],
            1,
            0x8000_0000,
            1,
            0x8000_0000,
            SF | PF,
        ),
        ("85 c9 positive", &[0x85, 0xc9], 1, 1, 1, 1, 0),
        ("85 c9 positive even parity", &[0x85, 0xc9], 1, 3, 1, 3, PF),
        // The high half is not part of a 32-bit test: ZF and PF only.
        (
            "85 c9 high half ignored",
            &[0x85, 0xc9],
            1,
            0xffff_ffff_0000_0000,
            1,
            0xffff_ffff_0000_0000,
            ZF | PF,
        ),
        (
            "85 c9 bit 31 with garbage high half",
            &[0x85, 0xc9],
            1,
            0x1234_5678_8000_0001,
            1,
            0x1234_5678_8000_0001,
            SF,
        ),
        // testl %ebx, %ecx with disjoint bits: ZF, PF.
        (
            "85 d9 disjoint",
            &[0x85, 0xd9],
            1,
            0xf0f0_f0f0,
            3,
            0x0f0f_0f0f,
            ZF | PF,
        ),
        // REX.RB without W: testl %r9d, %r10d (45 85 ca): rm = r10, reg = r9.
        (
            "45 85 ca REX.RB",
            &[0x45, 0x85, 0xca],
            10,
            0x8000_0003,
            9,
            0xc000_0001,
            SF,
        ),
        // REX.R only: testl %r8d, %eax (44 85 c0): rm = rax, reg = r8.
        ("44 85 c0 REX.R", &[0x44, 0x85, 0xc0], 0, 0x7, 8, 0x6, PF),
        // REX.B only: testl %eax, %r15d (41 85 c7): rm = r15, reg = rax.
        (
            "41 85 c7 REX.B",
            &[0x41, 0x85, 0xc7],
            15,
            0x100,
            0,
            0x100,
            PF,
        ),
        // Plain REX (40) and REX.X (42) without W: the same register form.
        (
            "40 85 c9 REX",
            &[0x40, 0x85, 0xc9],
            1,
            0x8000_0000,
            1,
            0x8000_0000,
            SF | PF,
        ),
        ("42 85 c9 REX.X", &[0x42, 0x85, 0xc9], 1, 0, 1, 0, ZF | PF),
    ];
    for (name, bytes, a, a_value, b, b_value, flags) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        state.gpr[*a] = *a_value;
        state.gpr[*b] = *b_value;
        // CF, OF, SF, ZF, PF, AF all set before: TEST clears CF/OF and recomputes SF/ZF/PF.
        state.rflags |= ARITH;
        let mut expected = state.clone();
        expected.rip = RIP + bytes.len() as u64;
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        // No register changes (the result is not written, so no zero-extension of the high half).
        assert_eq!(state.gpr, expected.gpr, "{name}");
        assert_eq!(state.rip, expected.rip, "{name}");
        let arithmetic = state.rflags & ARITH;
        assert_eq!(arithmetic & (CF | OF), 0, "{name}");
        assert_eq!(arithmetic & (SF | ZF | PF), *flags, "{name}");
        assert_eq!(state.rflags & !ARITH, expected.rflags & !ARITH, "{name}");
        // AF is undefined after TEST (SDM, from memory): it leaves the defined set.
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !AF, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn test_r32_then_je_chain_keeps_the_boundary_state() {
    // Synthetic bytes modelled on the `/init` loop (not the real `/init` bytes at 0x40016b):
    // `85 c9` (testl %ecx, %ecx), `74 01` (je, taken on a zero low dword),
    // a skipped `90`, then `8a 06` (movb (%rsi), %al), `38 07` (cmpb %al, (%rdi); executes since
    // S4-39f11) and `48 ff c6` (incq %rsi; executes since S4-39f12) retire, `eb 00` (jmp rel8;
    // executes since S4-39f13) retires too, and `48 e9` (prefixed jmp rel32) stops the run.
    let mut state = user_state();
    state.gpr[Register::Rcx as usize] = 0xffff_ffff_0000_0000;
    state.gpr[Register::Rsi as usize] = DATA;
    state.gpr[Register::Rdi as usize] = DATA + 0x40;
    let mut memory = DataMemory::new(&[
        0x85, 0xc9, 0x74, 0x01, 0x90, 0x8a, 0x06, 0x38, 0x07, 0x48, 0xff, 0xc6, 0xeb, 0x00, 0x48,
        0xe9, 0x00, 0x00, 0x00, 0x00,
    ])
    .with_user(DATA, 0x5a)
    .with_user(DATA + 0x40, 0x5a);
    let result = run3_data(&mut state, &mut memory, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xe9 });
    assert_eq!(result.instructions_executed, 6);
    assert_eq!(state.rip, RIP + 14);
    assert_eq!(state.gpr[Register::Rcx as usize], 0xffff_ffff_0000_0000);
    assert_eq!(state.gpr[Register::Rax as usize], 0x5a);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA + 1);
    // The equal compare set ZF; INC of a non-zero result clears it and keeps CF (clear).
    assert_eq!(state.rflags & (ZF | CF | OF), 0);
    assert!(memory.writes.is_empty());
}

// ---- S4-39f5: Jcc rel32 and `[REX.W] 89 /r` (MOV r/m64, r64), register form ----

/// The condition of `0F 80+cc` on RFLAGS, written out per the SDM table (independent of the
/// decoder's `ConditionCode::evaluate`).
fn jcc_expected(cc: u8, flags: u64) -> bool {
    let (cf, pf, zf, sf, of) = (
        flags & CF != 0,
        flags & PF != 0,
        flags & ZF != 0,
        flags & SF != 0,
        flags & OF != 0,
    );
    match cc {
        0 => of,
        1 => !of,
        2 => cf,
        3 => !cf,
        4 => zf,
        5 => !zf,
        6 => cf || zf,
        7 => !cf && !zf,
        8 => sf,
        9 => !sf,
        10 => pf,
        11 => !pf,
        12 => sf != of,
        13 => sf == of,
        14 => zf || sf != of,
        15 => !zf && sf == of,
        _ => unreachable!(),
    }
}

#[test]
fn jcc_rel32_follows_every_condition_over_all_flag_combinations() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for cc in 0..16u8 {
        for combination in 0..32u64 {
            let mut flags = 0;
            for (bit, mask) in [CF, PF, ZF, SF, OF].into_iter().enumerate() {
                if combination & (1 << bit) != 0 {
                    flags |= mask;
                }
            }
            for rel in [0x10_i32, -0x20, 0] {
                let mut state = user_state();
                state.rflags = 2 | (1 << 9) | flags;
                for (index, value) in state.gpr.iter_mut().enumerate() {
                    *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
                }
                let before = state.clone();
                let mut bytes = vec![0x0f, 0x80 | cc];
                bytes.extend_from_slice(&rel.to_le_bytes());
                let mut memory = Memory::code(&bytes);
                let result = run3(&mut state, &mut memory, &set, 1).unwrap();
                let name = format!("cc {cc:#x} flags {flags:#x} rel {rel}");
                assert_eq!(
                    exit(&result),
                    &RunExit::InstructionBudgetExhausted,
                    "{name}"
                );
                assert_eq!(result.instructions_executed, 1, "{name}");
                let next = RIP + 6;
                let expected_rip = if jcc_expected(cc, flags) {
                    next.wrapping_add_signed(i64::from(rel))
                } else {
                    next
                };
                assert_eq!(state.rip, expected_rip, "{name}");
                // Flags, registers and everything else stay.
                assert_eq!(state.rflags, before.rflags, "{name}");
                assert_eq!(state.gpr, before.gpr, "{name}");
                assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
                assert_eq!(memory.accesses, 0, "{name}");
            }
        }
    }
}

#[test]
fn js_and_je_take_the_branch_only_on_sf_and_zf() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (opcode second byte, RFLAGS, taken). 0x206 = IF | PF | bit 1 (SF and ZF clear, as after
    // `testq %rax, %rax` with rax = 3 in `/init`), 0x246 adds ZF, 0x286 adds SF instead.
    let cases: &[(u8, u64, bool)] = &[
        (0x88, 0x206, false),
        (0x88, 0x286, true),
        (0x88, 0x246, false),
        (0x84, 0x246, true),
        (0x84, 0x206, false),
        (0x84, 0x286, false),
        (0x85, 0x206, true),
        (0x85, 0x246, false),
        (0x89, 0x206, true),
        (0x89, 0x286, false),
        (0x8a, 0x206, true),
    ];
    for (second, rflags, taken) in cases {
        let mut state = user_state();
        state.rflags = *rflags | (1 << 9);
        let mut memory = Memory::code(&[0x0f, *second, 0x0c, 0x01, 0x00, 0x00]);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        let name = format!("0f {second:02x} rflags {rflags:#x}");
        assert_eq!(result.instructions_executed, 1, "{name}");
        let expected = if *taken { RIP + 6 + 0x10c } else { RIP + 6 };
        assert_eq!(state.rip, expected, "{name}");
        assert_eq!(state.rflags, *rflags | (1 << 9), "{name}");
    }
}

#[test]
fn mov_r64_register_form_copies_all_64_bits_and_leaves_flags_alone() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, destination index, source index)
    let cases: &[(&str, &[u8], usize, usize)] = &[
        ("48 89 c3 mov rbx, rax", &[0x48, 0x89, 0xc3], 3, 0),
        ("49 89 c4 mov r12, rax (REX.WB)", &[0x49, 0x89, 0xc4], 12, 0),
        ("4c 89 e7 mov rdi, r12 (REX.WR)", &[0x4c, 0x89, 0xe7], 7, 12),
        ("4d 89 c7 mov r15, r8 (REX.WRB)", &[0x4d, 0x89, 0xc7], 15, 8),
        ("48 89 c0 mov rax, rax", &[0x48, 0x89, 0xc0], 0, 0),
        ("4a 89 c3 mov rbx, rax (REX.WX)", &[0x4a, 0x89, 0xc3], 3, 0),
    ];
    for (name, bytes, dst, src) in cases {
        for flags in [0, ARITH] {
            let mut state = user_state();
            for (index, value) in state.gpr.iter_mut().enumerate() {
                *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
            }
            state.gpr[*src] = 0xfedc_ba98_7654_3210;
            state.rflags |= flags;
            let mut expected = state.clone();
            expected.gpr[*dst] = 0xfedc_ba98_7654_3210;
            expected.rip = RIP + bytes.len() as u64;
            let mut memory = Memory::code(bytes);
            let result = run3(&mut state, &mut memory, &set, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(state.rip, RIP + 3, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.accesses, 0, "{name}");
        }
    }
}

/// `/init` from `0x4000ff` to the `read` `SYSCALL` at `0x40011f` (`init.elf`, `objdump -d`):
/// `testq %rax, %rax; js 0x400214; movq %rax, %r12; movq %r12, %rdi; leaq 0xeeb(%rip), %rsi;
/// movl $0xfff, %edx; movl $0, %eax`, then `syscall` (`0f 05`).
const INIT_READ_PATH: [u8; 33] = [
    0x48, 0x85, 0xc0, 0x0f, 0x88, 0x0c, 0x01, 0x00, 0x00, 0x49, 0x89, 0xc4, 0x4c, 0x89, 0xe7, 0x48,
    0x8d, 0x35, 0xeb, 0x0e, 0x00, 0x00, 0xba, 0xff, 0x0f, 0x00, 0x00, 0xb8, 0x00, 0x00, 0x00, 0x00,
    0x0f,
];

#[test]
fn init_path_after_openat_runs_in_ring3_up_to_the_read_syscall() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut bytes = INIT_READ_PATH.to_vec();
    bytes.push(0x05);
    let mut state = user_state();
    // The `openat` result: fd 3 (`js` is not taken).
    state.gpr[Register::Rax as usize] = 3;
    let mut memory = Memory::code(&bytes);
    let result = run3(&mut state, &mut memory, &set, 64).unwrap();
    // Without the `with_syscall` member the loop stops at `SYSCALL` (`Unsupported 0x05`).
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 7);
    assert_eq!(state.rip, RIP + 32);
    assert_eq!(state.gpr[Register::R12 as usize], 3);
    assert_eq!(state.gpr[Register::Rdi as usize], 3);
    assert_eq!(state.gpr[Register::Rsi as usize], RIP + 22 + 0xeeb);
    assert_eq!(state.gpr[Register::Rdx as usize], 0xfff);
    assert_eq!(state.gpr[Register::Rax as usize], 0);
    // `testq %rax, %rax` with rax = 3: SF = ZF = 0, PF = 1 (two bits set).
    assert_eq!(state.rflags & ARITH, PF);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn init_path_with_a_negative_openat_result_takes_the_js_to_the_error_exit() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // The `js` target `0x400214` is `0x115` bytes past `0x4000ff`: a `SYSCALL` stands there.
    let mut bytes = vec![0u8; 0x117];
    bytes[..9].copy_from_slice(&INIT_READ_PATH[..9]);
    bytes[0x115] = 0x0f;
    bytes[0x116] = 0x05;
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xffff_ffff_ffff_fff2;
    let mut memory = Memory::code(&bytes);
    let result = run3(&mut state, &mut memory, &set, 64).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 0x115);
    assert_eq!(state.rflags & (SF | ZF), SF);
}

// ---- S4-39f6: `[REX] C6 /0 ib` (MOV r/m8, imm8) with a memory operand ----

/// Guest memory with a code page at `RIP` and a sparse data area: user pages (writable by CPL3)
/// and supervisor pages (a CPL3 access is a #PF with U, a write with W too).
struct DataMemory {
    code: Vec<u8>,
    code_base: u64,
    user: std::collections::BTreeMap<u64, u8>,
    supervisor: std::collections::BTreeMap<u64, u8>,
    /// Addresses of a page without P: any access is a #PF with the P bit clear.
    absent: std::collections::BTreeSet<u64>,
    writes: Vec<(u64, u8)>,
    reads: usize,
    read_widths: Vec<(u64, usize)>,
    atomic_rmws: usize,
}

impl DataMemory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            code_base: RIP,
            user: std::collections::BTreeMap::new(),
            supervisor: std::collections::BTreeMap::new(),
            absent: std::collections::BTreeSet::new(),
            writes: Vec::new(),
            reads: 0,
            read_widths: Vec::new(),
            atomic_rmws: 0,
        }
    }

    fn with_user(mut self, address: u64, value: u8) -> Self {
        self.user.insert(address, value);
        self
    }

    fn with_supervisor(mut self, address: u64, value: u8) -> Self {
        self.supervisor.insert(address, value);
        self
    }

    fn with_absent(mut self, address: u64) -> Self {
        self.absent.insert(address);
        self
    }

    fn not_present_fault(kind: AccessKind, context: AccessContext, address: u64) -> MemoryError {
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(page_fault_error_code(kind, context, false, false, false)),
            linear_address: Some(address),
        })
    }

    fn page_fault(kind: AccessKind, context: AccessContext, address: u64) -> MemoryError {
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(page_fault_error_code(kind, context, true, false, false)),
            linear_address: Some(address),
        })
    }
}

impl GuestMemory for DataMemory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(self.code_base).unwrap_or(u64::MAX) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
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
        self.reads += 1;
        self.read_widths.push((address, destination.len()));
        for (index, byte) in destination.iter_mut().enumerate() {
            let at = address + index as u64;
            if self.absent.contains(&at) {
                return Err(Self::not_present_fault(AccessKind::Read, context, at));
            }
            if self.supervisor.contains_key(&at)
                && context.page_privilege() == PrivilegeLevel::Ring3
            {
                return Err(Self::page_fault(AccessKind::Read, context, at));
            }
            *byte = *self
                .user
                .get(&at)
                .or_else(|| self.supervisor.get(&at))
                .ok_or(MemoryError::Unmapped {
                    address: at,
                    access: AccessKind::Read,
                })?;
        }
        Ok(())
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        let length = usize::from(size.bytes());
        self.reads += 1;
        self.read_widths.push((address, length));
        for index in 0..length {
            let at = address + index as u64;
            if self.absent.contains(&at) {
                return Err(Self::not_present_fault(AccessKind::Write, context, at));
            }
            if self.supervisor.contains_key(&at)
                && context.page_privilege() == PrivilegeLevel::Ring3
            {
                return Err(Self::page_fault(AccessKind::Write, context, at));
            }
            if !self.user.contains_key(&at) && !self.supervisor.contains_key(&at) {
                return Err(MemoryError::Unmapped {
                    address: at,
                    access: AccessKind::Write,
                });
            }
        }
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes[..length].iter_mut().enumerate() {
            let at = address + index as u64;
            *byte = *self
                .user
                .get(&at)
                .or_else(|| self.supervisor.get(&at))
                .unwrap();
        }
        Ok(u64::from_le_bytes(bytes))
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        // All checks before the first store: a failed store changes nothing.
        for index in 0..source.len() as u64 {
            let at = address + index;
            if self.supervisor.contains_key(&at)
                && context.page_privilege() == PrivilegeLevel::Ring3
            {
                return Err(Self::page_fault(AccessKind::Write, context, at));
            }
            if !self.user.contains_key(&at) && !self.supervisor.contains_key(&at) {
                return Err(MemoryError::Unmapped {
                    address: at,
                    access: AccessKind::Write,
                });
            }
        }
        for (index, byte) in source.iter().enumerate() {
            let at = address + index as u64;
            if let Some(slot) = self.user.get_mut(&at) {
                *slot = *byte;
            } else if let Some(slot) = self.supervisor.get_mut(&at) {
                *slot = *byte;
            }
            self.writes.push((at, *byte));
        }
        Ok(())
    }

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_rmws += 1;
        let mut bytes = vec![0; usize::from(size.bytes())];
        self.read(address, context, &mut bytes)?;
        let mut padded = [0u8; 8];
        padded[..bytes.len()].copy_from_slice(&bytes);
        let old = u64::from_le_bytes(padded);
        let new = update(old).to_le_bytes();
        self.write(address, context, &new[..bytes.len()])?;
        Ok(old)
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

fn run3_data(
    state: &mut LongModeState,
    memory: &mut DataMemory,
    budget: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
}

const DATA: u64 = 0x0060_1000;

#[test]
fn cpl3_addr32_direct_call_uses_full_rip_and_rsp() {
    let code = [0x67, 0xe8, 0xeb, 0xe5, 0xfa, 0xff];
    let rip = 0x0000_0001_004c_7c9fu64;
    let stack = 0x0000_0002_0060_1000u64;
    let mut initial = user_state();
    initial.rip = rip;
    initial.gpr[Register::Rsp as usize] = stack + 8;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    let mut state = initial.clone();
    let mut memory = DataMemory::new(&code);
    memory.code_base = rip;
    for offset in 0..8 {
        memory = memory.with_user(stack + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = initial.clone();
    expected.rip = 0x0000_0001_0047_6290;
    expected.gpr[Register::Rsp as usize] = stack;
    assert_eq!(state, expected);
    let return_address = (rip + code.len() as u64).to_le_bytes();
    assert_eq!(
        memory.writes,
        (stack..stack + 8).zip(return_address).collect::<Vec<_>>()
    );
    assert_eq!(memory.reads, 0);

    let mut state = initial.clone();
    let mut memory = DataMemory::new(&code);
    memory.code_base = rip;
    for offset in 0..8 {
        memory = memory.with_supervisor(stack + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(stack),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, initial);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_movsxd_extends_only_the_source_dword_to_a_qword_register() {
    let mut initial = user_state();
    initial.gpr[Register::Rsi as usize] = 0xdead_beef_8000_0001;
    initial.gpr[Register::R14 as usize] = 0xdead_beef_7fff_ffff;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    for (code, destination, value) in [
        ([0x48, 0x63, 0xc6], Register::Rax, 0xffff_ffff_8000_0001),
        ([0x4d, 0x63, 0xc6], Register::R8, 0x7fff_ffff),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(&code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        let mut expected = initial.clone();
        expected.rip += code.len() as u64;
        expected.gpr[destination as usize] = value;
        assert_eq!(state, expected);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
    let code = [0x48, 0x63, 0x16]; // BusyBox MOVSXD RDX,[RSI]
    let mut state = initial.clone();
    state.gpr[Register::Rsi as usize] = DATA;
    let mut expected = state.clone();
    expected.rip += code.len() as u64;
    expected.gpr[Register::Rdx as usize] = 0xffff_ffff_8000_0001;
    let mut memory = DataMemory::new(&code);
    for (i, byte) in 0x8000_0001u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + i as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state, expected);
    assert_eq!(memory.read_widths, [(DATA, 4)]);
    assert!(memory.writes.is_empty());

    for code in [
        &[0x63, 0xc6][..],
        &[0x66, 0x63, 0xc6],
        &[0x67, 0x48, 0x63, 0x06],
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x63 });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_qword_mov_uses_user_memory_and_nop_does_not_read_its_operand() {
    let code = [0x48, 0x8b, 0x07, 0x48, 0x89, 0x06, 0x0f, 0x1f, 0x00];
    let value = 0xfedc_ba98_7654_3210u64;
    let output = DATA + 32;
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rsi as usize] = output;
    state.rflags |= od_isa::INTEGER_FLAGS;
    let mut expected = state.clone();
    expected.gpr[Register::Rax as usize] = value;
    expected.rip += code.len() as u64;

    let mut memory = DataMemory::new(&code);
    let bytes = value.to_le_bytes();
    for (offset, byte) in bytes.into_iter().enumerate() {
        memory = memory
            .with_user(DATA + offset as u64, byte)
            .with_user(output + offset as u64, 0);
    }
    let result = run3_data(&mut state, &mut memory, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(
        memory.writes,
        (output..output + 8).zip(bytes).collect::<Vec<_>>()
    );
}

#[test]
fn cpl3_qword_imm8_prologue_sign_extends_register_and_memory_operands() {
    let code = [
        0x48, 0x83, 0xe8, 0xf8, // sub rax, -8
        0x48, 0x83, 0xc0, 0xe8, // add rax, -24
        0x48, 0x83, 0x3f, 0xfe, // cmp qword ptr [rdi], -2
    ];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x2_0000_0000;
    state.gpr[Register::Rdi as usize] = DATA;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in (-2i64).to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + offset as u64, byte);
    }

    let result = run3_data(&mut state, &mut memory, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1_ffff_fff0);
    assert_eq!(
        state.rflags & od_isa::INTEGER_FLAGS,
        (1 << 6) | (1 << 2) | 2
    );
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_qword_prologue_operands_cannot_access_supervisor_pages() {
    for (code, error_code) in [
        (&[0x48, 0x8b, 0x07][..], 5),
        (&[0x48, 0x89, 0x07][..], 7),
        (&[0x48, 0x83, 0x3f, 0x00][..], 5),
    ] {
        let mut initial = user_state();
        initial.gpr[Register::Rdi as usize] = DATA;
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_supervisor(DATA + offset, offset as u8);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
                linear_address: Some(DATA),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_and_imm8_aligns_qword_registers_without_accessing_data_memory() {
    let mut initial = user_state();
    initial.gpr[Register::Rsp as usize] = 0x0000_7fff_abcd_123f;
    initial.gpr[Register::R12 as usize] = 0x0000_1234_5678_9aaf;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    for (code, register) in [
        ([0x48, 0x83, 0xe4, 0xf0], Register::Rsp),
        ([0x49, 0x83, 0xe4, 0xf0], Register::R12),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(&code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !AF);
        let mut expected = initial.clone();
        expected.rip += code.len() as u64;
        expected.gpr[register as usize] &= !0xf;
        expected.rflags &= !(CF | OF | SF | ZF);
        assert_eq!(state, expected);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }

    for (code, opcode) in [
        (&[0x66, 0x83, 0xe4, 0xf0][..], 0x83),
        (&[0x66, 0x83, 0x0f, 0xf0][..], 0x83),
        (&[0x48, 0x81, 0xe4, 0xf0, 0xff, 0xff, 0xff][..], 0x81),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_push_register_stores_one_qword_before_committing_rsp() {
    let mut initial = user_state();
    initial.gpr[Register::Rax as usize] = 0x0000_7fff_a1b2_c3d4;
    initial.gpr[Register::R14 as usize] = 0xfedc_ba98_7654_3210;
    initial.gpr[Register::Rsp as usize] = DATA + 8;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    for (code, register) in [
        (&[0x50][..], Register::Rax),
        (&[0x41, 0x56][..], Register::R14),
        (&[0x54][..], Register::Rsp),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_user(DATA + offset, 0);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        let mut expected = initial.clone();
        expected.rip += code.len() as u64;
        expected.gpr[Register::Rsp as usize] = DATA;
        let expected_bytes = initial.gpr[register as usize].to_le_bytes();
        let expected_writes: Vec<_> = (DATA..DATA + 8).zip(expected_bytes).collect();
        assert_eq!(state, expected);
        assert_eq!(memory.writes, expected_writes);
        assert_eq!(memory.reads, 0);
    }

    let next_page = DATA + 4096;
    let mut before_fault = initial.clone();
    before_fault.gpr[Register::Rsp as usize] = next_page + 1;
    let mut state = before_fault.clone();
    let mut memory = DataMemory::new(&[0x50]).with_supervisor(next_page, 0);
    for address in next_page - 7..next_page {
        memory = memory.with_user(address, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(next_page),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before_fault);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_pop_register_reads_one_qword_and_commits_the_stack_pointer() {
    const VALUE: u64 = 0x0000_7fff_a1b2_c3d4;
    let mut initial = user_state();
    initial.gpr[Register::Rsp as usize] = DATA;
    initial.rflags |= od_isa::INTEGER_FLAGS;
    for (code, register) in [
        (&[0x5e][..], Register::Rsi),
        (&[0x41, 0x5e][..], Register::R14),
        (&[0x5c][..], Register::Rsp),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        for (offset, byte) in VALUE.to_le_bytes().into_iter().enumerate() {
            memory = memory.with_user(DATA + offset as u64, byte);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        let mut expected = initial.clone();
        expected.rip += code.len() as u64;
        expected.gpr[Register::Rsp as usize] += 8;
        expected.gpr[register as usize] = VALUE;
        assert_eq!(state, expected);
        assert_eq!(memory.reads, 1);
        assert!(memory.writes.is_empty());
    }

    let next_page = DATA + 4096;
    let stack_end = next_page - 7;
    let mut before_fault = initial.clone();
    before_fault.gpr[Register::Rsp as usize] = stack_end;
    let mut state = before_fault.clone();
    let mut memory = DataMemory::new(&[0x5e]).with_supervisor(next_page, 0);
    for offset in 0..7 {
        memory = memory.with_user(stack_end + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(next_page),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before_fault);
    assert!(memory.writes.is_empty());

    for (code, opcode) in [
        (&[0x66, 0x5e][..], 0x5e),
        (&[0x66, 0x56][..], 0x56),
        (&[0x8f, 0x06][..], 0x8f),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

fn mov_byte_state() -> LongModeState {
    let mut state = user_state();
    for (index, value) in state.gpr.iter_mut().enumerate() {
        *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
    }
    state.gpr[Register::Rbx as usize] = DATA;
    state.gpr[Register::R13 as usize] = 3;
    state.gpr[Register::R14 as usize] = DATA + 0x100;
    state
}

#[test]
fn init_movb_with_sib_rex_x_and_rex_b_stores_one_byte_at_the_effective_address() {
    // `43 c6 04 2e 00`: `movb $0x0, (%r14,%r13)` of `/init` at `0x40013e` (REX.XB, SIB 2e).
    for flags in [0, ARITH] {
        let mut state = mov_byte_state();
        state.rflags |= flags;
        let mut expected = state.clone();
        expected.rip = RIP + 5;
        let target = DATA + 0x100 + 3;
        let mut memory = DataMemory::new(&[0x43, 0xc6, 0x04, 0x2e, 0x00])
            .with_user(target - 1, 0xaa)
            .with_user(target, 0xff)
            .with_user(target + 1, 0xbb);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state, expected);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(memory.writes, [(target, 0)]);
        assert_eq!(memory.user[&(target - 1)], 0xaa);
        assert_eq!(memory.user[&target], 0);
        assert_eq!(memory.user[&(target + 1)], 0xbb);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn movb_imm8_forms_store_the_immediate_without_touching_registers_or_flags() {
    // (name, bytes, effective address, stored byte)
    let forms: &[(&str, &[u8], u64, u8)] = &[
        ("c6 03 7f (%rbx)", &[0xc6, 0x03, 0x7f], DATA, 0x7f),
        (
            "c6 43 05 80 5(%rbx)",
            &[0xc6, 0x43, 0x05, 0x80],
            DATA + 5,
            0x80,
        ),
        (
            "c6 43 fe 01 -2(%rbx) (disp8 sign)",
            &[0xc6, 0x43, 0xfe, 0x01],
            DATA - 2,
            0x01,
        ),
        (
            "c6 83 00 01 00 00 11 0x100(%rbx) (disp32)",
            &[0xc6, 0x83, 0x00, 0x01, 0x00, 0x00, 0x11],
            DATA + 0x100,
            0x11,
        ),
        (
            "42 c6 04 2b 55 (%rbx,%r13) REX.X",
            &[0x42, 0xc6, 0x04, 0x2b, 0x55],
            DATA + 3,
            0x55,
        ),
        (
            "41 c6 04 5e 66 (%r14,%rbx,2) REX.B scale",
            &[0x41, 0xc6, 0x04, 0x5e, 0x66],
            DATA + 0x100 + 2 * DATA,
            0x66,
        ),
        (
            "c6 04 dd 00 10 60 00 22 0x601000(,%rbx,8) no base",
            &[0xc6, 0x04, 0xdd, 0x00, 0x10, 0x60, 0x00, 0x22],
            0x60_1000 + 8 * DATA,
            0x22,
        ),
        (
            "41 c6 06 99 (%r14) REX.B",
            &[0x41, 0xc6, 0x06, 0x99],
            DATA + 0x100,
            0x99,
        ),
        // S4-39f7: REX.B on `mod = 00, rm = 101` is still RIP-relative (not r13): the base is the
        // end of the 8-byte instruction, and r13 (= 3 here) plays no part.
        (
            "41 c6 05 10 00 00 00 5a 0x10(%rip) REX.B is not r13",
            &[0x41, 0xc6, 0x05, 0x10, 0x00, 0x00, 0x00, 0x5a],
            RIP + 8 + 0x10,
            0x5a,
        ),
        // S4-39f7: SIB 25 (scale 1, no index, base field 101, mod = 00) is a pure disp32 even with
        // REX.B (the base is none, not r13): the address is 0x601000; r13 (3) and rbx play no part.
        (
            "41 c6 04 25 00 10 60 00 3c 0x601000 REX.B base none",
            &[0x41, 0xc6, 0x04, 0x25, 0x00, 0x10, 0x60, 0x00, 0x3c],
            0x60_1000,
            0x3c,
        ),
        // REX.W and REX.R do not change a byte store.
        ("48 c6 03 33 REX.W", &[0x48, 0xc6, 0x03, 0x33], DATA, 0x33),
        ("44 c6 03 34 REX.R", &[0x44, 0xc6, 0x03, 0x34], DATA, 0x34),
    ];
    for (name, bytes, target, value) in forms {
        for flags in [0, ARITH] {
            let mut state = mov_byte_state();
            state.rflags |= flags;
            let mut expected = state.clone();
            expected.rip = RIP + bytes.len() as u64;
            let mut memory = DataMemory::new(bytes)
                .with_user(*target, 0xee)
                .with_user(target + 1, 0xdd);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(memory.writes, [(*target, *value)], "{name}");
            assert_eq!(memory.user[target], *value, "{name}");
            assert_eq!(memory.user[&(target + 1)], 0xdd, "{name}");
        }
    }
}

#[test]
fn movb_rip_relative_stores_at_next_rip_plus_displacement() {
    // `c6 05 10 00 00 00 7a`: `movb $0x7a, 0x10(%rip)`; the base is the end of the instruction.
    let mut state = mov_byte_state();
    let mut expected = state.clone();
    expected.rip = RIP + 7;
    let target = RIP + 7 + 0x10;
    let mut memory =
        DataMemory::new(&[0xc6, 0x05, 0x10, 0x00, 0x00, 0x00, 0x7a]).with_user(target, 0);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.writes, [(target, 0x7a)]);
}

#[test]
fn movb_to_a_supervisor_page_is_a_user_write_page_fault_without_a_store() {
    let mut state = mov_byte_state();
    let before = state.clone();
    let mut memory =
        DataMemory::new(&[0x43, 0xc6, 0x04, 0x2e, 0x00]).with_supervisor(DATA + 0x103, 0xcc);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    // P (the page is present) | W (a write) | U (a CPL3 access).
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x7),
            linear_address: Some(DATA + 0x103),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.supervisor[&(DATA + 0x103)], 0xcc);
}

#[test]
fn movb_to_a_non_canonical_address_is_a_general_protection_fault_without_a_store() {
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0xc6, 0x03, 0x01]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn movb_refusals_stop_as_unsupported_c6_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("c6 /1 (%rbx)", &[0xc6, 0x0b, 0x01]),
        ("c6 /2", &[0xc6, 0x13, 0x01]),
        ("c6 /3", &[0xc6, 0x1b, 0x01]),
        ("c6 /4", &[0xc6, 0x23, 0x01]),
        ("c6 /5", &[0xc6, 0x2b, 0x01]),
        ("c6 /6", &[0xc6, 0x33, 0x01]),
        ("c6 /7 (XABORT region)", &[0xc6, 0x3b, 0x01]),
        ("c6 f8 00 xabort", &[0xc6, 0xf8, 0x00]),
        ("c6 /1 with SIB and REX", &[0x43, 0xc6, 0x0c, 0x2e, 0x00]),
        ("c6 c0 01 mov al, 1 (mod = 11)", &[0xc6, 0xc0, 0x01]),
        ("41 c6 c0 01 mov r8b, 1", &[0x41, 0xc6, 0xc0, 0x01]),
        ("66 c6 03 01", &[0x66, 0xc6, 0x03, 0x01]),
        ("67 c6 03 01", &[0x67, 0xc6, 0x03, 0x01]),
        ("f0 c6 03 01 LOCK", &[0xf0, 0xc6, 0x03, 0x01]),
        ("f2 c6 03 01 REPNE", &[0xf2, 0xc6, 0x03, 0x01]),
        ("f3 c6 03 01 REP", &[0xf3, 0xc6, 0x03, 0x01]),
        ("64 c6 03 01 FS", &[0x64, 0xc6, 0x03, 0x01]),
        ("65 c6 03 01 GS", &[0x65, 0xc6, 0x03, 0x01]),
        ("26 c6 03 01 ES", &[0x26, 0xc6, 0x03, 0x01]),
        ("2e c6 03 01 CS", &[0x2e, 0xc6, 0x03, 0x01]),
        ("36 c6 03 01 SS", &[0x36, 0xc6, 0x03, 0x01]),
        ("3e c6 03 01 DS", &[0x3e, 0xc6, 0x03, 0x01]),
        ("66 43 c6 04 2e 00", &[0x66, 0x43, 0xc6, 0x04, 0x2e, 0x00]),
        ("2e 43 c6 04 2e 00", &[0x2e, 0x43, 0xc6, 0x04, 0x2e, 0x00]),
        ("3e 43 c6 04 2e 00", &[0x3e, 0x43, 0xc6, 0x04, 0x2e, 0x00]),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xc6 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn mov_r64_with_a_hint_or_segment_prefix_is_refused() {
    // S4-39f5 review item 6: `2e`/`3e` (branch hints, CS/DS override) before the memory form and
    // before the register form of `89` never reach the CPL0 `Operation::Mov`.
    let forms: &[(&str, &[u8])] = &[
        ("2e 89 03", &[0x2e, 0x89, 0x03]),
        ("3e 89 03", &[0x3e, 0x89, 0x03]),
        ("2e 48 89 03", &[0x2e, 0x48, 0x89, 0x03]),
        ("3e 48 89 03", &[0x3e, 0x48, 0x89, 0x03]),
        ("2e 48 89 c3", &[0x2e, 0x48, 0x89, 0xc3]),
        ("3e 48 89 c3", &[0x3e, 0x48, 0x89, 0xc3]),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x89 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn init_path_runs_the_movb_then_xor_and_stops_at_the_next_forbidden_form() {
    // `/init` from `0x400137`: `leaq 0xec2(%rip), %r14; movb $0, (%r14,%r13); xorl %ebx, %ebx;
    // cmpq %r13, %rbx; jae +0xc6; testq %rbx, %rbx; je +8; cmpb $0x20, -1(%r14,%rbx)` (S4-39f7:
    // `4c 39 eb` and the short `74 08` now execute; `jae` is not taken as 0 < 5, `je` is taken as
    // rbx = 0 and skips the 8 bytes of `cmpb` and `jne`, `leaq 0x107(%rip), %rsi` retires, and
    // `leaq (%r14,%rbx), %rdi` (`49 8d 3c 1e`) retires since S4-39f8, and the run stops at the
    // `hlt` placed after it).
    let bytes = [
        0x4c, 0x8d, 0x35, 0xc2, 0x0e, 0x00, 0x00, 0x43, 0xc6, 0x04, 0x2e, 0x00, 0x31, 0xdb, 0x4c,
        0x39, 0xeb, 0x0f, 0x83, 0xc6, 0x00, 0x00, 0x00, 0x48, 0x85, 0xdb, 0x74, 0x08, 0x41, 0x80,
        0x7c, 0x1e, 0xff, 0x20, 0x75, 0x24, 0x48, 0x8d, 0x35, 0x07, 0x01, 0x00, 0x00, 0x49, 0x8d,
        0x3c, 0x1e, 0xf4,
    ];
    let mut state = mov_byte_state();
    state.gpr[Register::R13 as usize] = 5;
    let target = RIP + 7 + 0xec2 + 5;
    let mut memory = DataMemory::new(&bytes).with_user(target, b'x');
    let result = run3_data(&mut state, &mut memory, 16).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    // lea, movb, xor, cmp, jae (not taken), test, je (taken), lea (rip-relative), lea (SIB).
    assert_eq!(result.instructions_executed, 9);
    assert_eq!(state.rip, RIP + 47);
    assert_eq!(state.gpr[Register::Rdi as usize], RIP + 7 + 0xec2);
    assert_eq!(state.gpr[Register::R14 as usize], RIP + 7 + 0xec2);
    assert_eq!(state.gpr[Register::Rbx as usize], 0);
    assert_eq!(state.gpr[Register::Rsi as usize], RIP + 43 + 0x107);
    assert_eq!(memory.writes, [(target, 0)]);
    // cmp 0 - 5: CF, SF, AF; the test of rbx = 0 then sets ZF and PF and clears CF, OF, SF; AF is
    // undefined after TEST and keeps the value of the CMP here (checked apart from the rest).
    assert_eq!(state.rflags & ARITH & !AF, ZF | PF);
}

#[test]
fn movb_with_a_stack_base_and_a_non_canonical_address_is_a_stack_fault_without_a_store() {
    // S4-39f7: SDM (from memory): an EA whose base is RSP or RBP uses SS, and a non-canonical
    // stack address is #SS(0) (vector 12); every other base is #GP(0) (vector 13).
    const BAD: u64 = 0x0000_8000_0000_0000;
    // (name, bytes, register to set, vector)
    let cases: &[(&str, &[u8], Register, u8)] = &[
        (
            "c6 04 24 01 movb $1, (%rsp)",
            &[0xc6, 0x04, 0x24, 0x01],
            Register::Rsp,
            12,
        ),
        (
            "c6 45 00 01 movb $1, 0(%rbp)",
            &[0xc6, 0x45, 0x00, 0x01],
            Register::Rbp,
            12,
        ),
        (
            "c6 45 08 01 movb $1, 8(%rbp)",
            &[0xc6, 0x45, 0x08, 0x01],
            Register::Rbp,
            12,
        ),
        (
            "c6 04 1c 01 movb $1, (%rsp,%rbx)",
            &[0xc6, 0x04, 0x1c, 0x01],
            Register::Rsp,
            12,
        ),
        (
            "c6 44 24 08 01 movb $1, 8(%rsp)",
            &[0xc6, 0x44, 0x24, 0x08, 0x01],
            Register::Rsp,
            12,
        ),
        // R12/R13 (REX.B on the same base fields) are not stack bases: #GP(0).
        (
            "41 c6 04 24 01 movb $1, (%r12)",
            &[0x41, 0xc6, 0x04, 0x24, 0x01],
            Register::R12,
            13,
        ),
        (
            "41 c6 45 00 01 movb $1, 0(%r13)",
            &[0x41, 0xc6, 0x45, 0x00, 0x01],
            Register::R13,
            13,
        ),
        // RBX as the base (RSP only appears as the "no index" field): not a stack base.
        (
            "c6 04 63 01 movb $1, (%rbx) via SIB",
            &[0xc6, 0x04, 0x63, 0x01],
            Register::Rbx,
            13,
        ),
    ];
    for (name, bytes, register, vector) in cases {
        let mut state = mov_byte_state();
        state.gpr[*register as usize] = BAD;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Fault(fault)
                if fault.vector == *vector && fault.error_code == Some(0)),
            "{name}: {:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

// ---- S4-39f7: `[REX.W] 39 /r` (CMP r/m64, r64), register form, and the short Jcc ----

#[test]
fn cmp_r64_register_form_sets_the_sub_flags_and_writes_no_result() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, dst index, dst value, src index, src value, flags of dst - src); the result of
    // the subtraction is only the source of the flags.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, usize, u64, u64)] = &[
        // `/init` at 0x400145: cmpq %r13, %rbx (rm = rbx = dst, reg = r13 = src). 0 - 5 =
        // 0xffff_ffff_ffff_fffb: CF (borrow), SF, AF (the low nibble borrows); the low byte 0xfb
        // has seven set bits (odd): PF clear.
        (
            "4c 39 eb 0-5",
            &[0x4c, 0x39, 0xeb],
            3,
            0,
            13,
            5,
            CF | SF | AF,
        ),
        // Equal: ZF and PF only.
        ("4c 39 eb equal", &[0x4c, 0x39, 0xeb], 3, 7, 13, 7, ZF | PF),
        // 5 - 3 = 2 (one set bit: PF clear): no flag.
        ("4c 39 eb 5-3", &[0x4c, 0x39, 0xeb], 3, 5, 13, 3, 0),
        // 9 - 1 = 8: no flag.
        ("4c 39 eb 9-1", &[0x4c, 0x39, 0xeb], 3, 9, 13, 1, 0),
        // Low-nibble borrow only: 0x10 - 1 = 0x0f (four set bits): AF, PF.
        (
            "4c 39 eb 0x10-1",
            &[0x4c, 0x39, 0xeb],
            3,
            0x10,
            13,
            1,
            AF | PF,
        ),
        // Signed overflow: i64::MIN - 1 = i64::MAX (low byte 0xff): OF, PF, AF; no CF, no SF.
        (
            "4c 39 eb min-1",
            &[0x4c, 0x39, 0xeb],
            3,
            0x8000_0000_0000_0000,
            13,
            1,
            OF | PF | AF,
        ),
        // The high half decides: 0x1_0000_0000 - 1 = 0xffff_ffff: PF, AF; no CF, no SF.
        (
            "4c 39 eb high half",
            &[0x4c, 0x39, 0xeb],
            3,
            0x1_0000_0000,
            13,
            1,
            PF | AF,
        ),
        // Borrow from the high half only: 0 - 2^32 = 0xffff_ffff_0000_0000: CF, SF, PF (low 0).
        (
            "4c 39 eb 0-2^32",
            &[0x4c, 0x39, 0xeb],
            3,
            0,
            13,
            0x1_0000_0000,
            CF | SF | PF,
        ),
        // REX.W only: cmpq %rcx, %rax (48 39 c8): rm = rax = dst, reg = rcx = src; 1 - 2 = -1.
        (
            "48 39 c8 REX.W",
            &[0x48, 0x39, 0xc8],
            0,
            1,
            1,
            2,
            CF | SF | PF | AF,
        ),
        // REX.WB: cmpq %rax, %r15 (49 39 c7): rm = r15 = dst, reg = rax = src.
        ("49 39 c7 REX.WB", &[0x49, 0x39, 0xc7], 15, 4, 0, 4, ZF | PF),
        // REX.WRB: cmpq %r9, %r10 (4d 39 ca): rm = r10 = dst, reg = r9 = src; 0x10 - 0x20 = -0x10.
        (
            "4d 39 ca REX.WRB",
            &[0x4d, 0x39, 0xca],
            10,
            0x10,
            9,
            0x20,
            CF | SF | PF,
        ),
        // REX.WX (X is ignored with mod = 11): cmpq %rcx, %rax (4a 39 c8).
        ("4a 39 c8 REX.WX", &[0x4a, 0x39, 0xc8], 0, 3, 1, 3, ZF | PF),
        // The same register: cmpq %rbx, %rbx.
        (
            "48 39 db same register",
            &[0x48, 0x39, 0xdb],
            3,
            0x1234,
            3,
            0x1234,
            ZF | PF,
        ),
    ];
    for (name, bytes, dst, dst_value, src, src_value, flags) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        state.gpr[*dst] = *dst_value;
        state.gpr[*src] = *src_value;
        // Every arithmetic flag set before: CMP recomputes all six.
        state.rflags |= ARITH;
        let mut expected = state.clone();
        expected.rip = RIP + bytes.len() as u64;
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        // No register changes (the result is not written), RIP advances by the length.
        assert_eq!(state.gpr, expected.gpr, "{name}");
        assert_eq!(state.rip, expected.rip, "{name}");
        assert_eq!(state.rflags & ARITH, *flags, "{name}");
        // The non-arithmetic flags (IF, the reserved bit 1) stay.
        assert_eq!(state.rflags & !ARITH, expected.rflags & !ARITH, "{name}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn cmp_then_jae_follows_the_unsigned_comparison() {
    // `/init` at 0x400145: `cmpq %r13, %rbx; jae +6; <six f4 bytes>; nop`: jae is taken exactly
    // when rbx >= r13 (CF = 0) and skips the six filler bytes.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let bytes = [
        0x4c, 0x39, 0xeb, 0x0f, 0x83, 0x06, 0x00, 0x00, 0x00, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4,
        0x90,
    ];
    for (rbx, r13, taken) in [(0_u64, 5_u64, false), (5, 5, true), (6, 5, true)] {
        let mut state = user_state();
        state.gpr[Register::Rbx as usize] = rbx;
        state.gpr[Register::R13 as usize] = r13;
        let mut memory = Memory::code(&bytes);
        let result = run3(&mut state, &mut memory, &set, 3).unwrap();
        assert_eq!(
            result.instructions_executed,
            if taken { 3 } else { 2 },
            "{rbx}"
        );
        if taken {
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted, "{rbx}");
            assert_eq!(state.rip, RIP + 16, "{rbx}");
        } else {
            assert_eq!(
                exit(&result),
                &RunExit::Unsupported { opcode: 0xf4 },
                "{rbx}"
            );
            assert_eq!(state.rip, RIP + 9, "{rbx}");
        }
        assert_eq!(state.gpr[Register::Rbx as usize], rbx);
        assert_eq!(state.gpr[Register::R13 as usize], r13);
    }
}

#[test]
fn short_jcc_follows_every_condition_over_all_flag_combinations() {
    // `7x 02; hlt; hlt; nop`: the branch skips the two `hlt` bytes when taken. The condition is
    // the SDM table (the same `jcc_expected` as `0F 8x`); the displacement is rel8.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for cc in 0..16_u8 {
        for bits in 0..32_u64 {
            let flags = [CF, ZF, SF, OF, PF]
                .iter()
                .enumerate()
                .filter(|(index, _)| bits & (1 << index) != 0)
                .fold(0, |acc, (_, flag)| acc | flag);
            let mut state = user_state();
            state.rflags |= flags;
            let before_flags = state.rflags;
            let taken = jcc_expected(cc, before_flags);
            let mut memory = Memory::code(&[0x70 | cc, 0x02, 0xf4, 0xf4, 0x90]);
            let result = run3(&mut state, &mut memory, &set, 2).unwrap();
            assert_eq!(state.rflags, before_flags, "cc {cc} flags {bits:#x}");
            assert_eq!(
                result.instructions_executed,
                if taken { 2 } else { 1 },
                "cc {cc} flags {bits:#x}"
            );
            if taken {
                assert_eq!(state.rip, RIP + 5, "cc {cc} flags {bits:#x}");
                assert_eq!(
                    exit(&result),
                    &RunExit::InstructionBudgetExhausted,
                    "cc {cc} flags {bits:#x}"
                );
            } else {
                assert_eq!(state.rip, RIP + 2, "cc {cc} flags {bits:#x}");
                assert_eq!(
                    exit(&result),
                    &RunExit::Unsupported { opcode: 0xf4 },
                    "cc {cc} flags {bits:#x}"
                );
            }
        }
    }
}

#[test]
fn short_jcc_backward_displacement_is_sign_extended() {
    // `74 fe` (je -2) with ZF set jumps to its own address: it retires three times and RIP stays.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.rflags |= ZF;
    let mut memory = Memory::code(&[0x74, 0xfe]);
    let result = run3(&mut state, &mut memory, &set, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP);
}

// ---- S4-39f10: `[REX] 8A /r` (MOV r8, r/m8) with a memory operand ----

/// Expected register file after loading `value` into the byte view of `register` (`shift` 8 for
/// AH..BH): the other bytes of the 64-bit register and every other register are unchanged.
fn with_byte(state: &LongModeState, register: Register, shift: u32, value: u8) -> LongModeState {
    let mut expected = state.clone();
    let old = expected.gpr[register as usize];
    expected.gpr[register as usize] = (old & !(0xff_u64 << shift)) | (u64::from(value) << shift);
    expected
}

#[test]
fn movb_load_forms_read_one_byte_into_the_byte_register_only() {
    // (name, bytes, effective address, register, shift of the byte inside it)
    let forms: &[(&str, &[u8], u64, Register, u32)] = &[
        // `/init` at 0x40016f with rsi = DATA + 0x20.
        (
            "8a 06 (%rsi) al",
            &[0x8a, 0x06],
            DATA + 0x20,
            Register::Rax,
            0,
        ),
        ("8a 03 (%rbx) al", &[0x8a, 0x03], DATA, Register::Rax, 0),
        ("8a 0b (%rbx) cl", &[0x8a, 0x0b], DATA, Register::Rcx, 0),
        ("8a 13 (%rbx) dl", &[0x8a, 0x13], DATA, Register::Rdx, 0),
        ("8a 1b (%rbx) bl", &[0x8a, 0x1b], DATA, Register::Rbx, 0),
        // No REX: ModRM.reg 4..7 are AH, CH, DH, BH (the high byte of ax, cx, dx, bx).
        ("8a 23 (%rbx) ah", &[0x8a, 0x23], DATA, Register::Rax, 8),
        ("8a 2b (%rbx) ch", &[0x8a, 0x2b], DATA, Register::Rcx, 8),
        ("8a 33 (%rbx) dh", &[0x8a, 0x33], DATA, Register::Rdx, 8),
        ("8a 3b (%rbx) bh", &[0x8a, 0x3b], DATA, Register::Rbx, 8),
        // Any REX: 4..7 are SPL, BPL, SIL, DIL.
        (
            "40 8a 23 (%rbx) spl",
            &[0x40, 0x8a, 0x23],
            DATA,
            Register::Rsp,
            0,
        ),
        (
            "40 8a 2b (%rbx) bpl",
            &[0x40, 0x8a, 0x2b],
            DATA,
            Register::Rbp,
            0,
        ),
        (
            "40 8a 33 (%rbx) sil",
            &[0x40, 0x8a, 0x33],
            DATA,
            Register::Rsi,
            0,
        ),
        (
            "40 8a 3b (%rbx) dil",
            &[0x40, 0x8a, 0x3b],
            DATA,
            Register::Rdi,
            0,
        ),
        // REX.R: destination r8b..r15b; REX.B: base r8..r15; REX.X: index.
        (
            "44 8a 03 (%rbx) r8b",
            &[0x44, 0x8a, 0x03],
            DATA,
            Register::R8,
            0,
        ),
        (
            "44 8a 3b (%rbx) r15b",
            &[0x44, 0x8a, 0x3b],
            DATA,
            Register::R15,
            0,
        ),
        (
            "41 8a 06 (%r14) al",
            &[0x41, 0x8a, 0x06],
            DATA + 0x100,
            Register::Rax,
            0,
        ),
        (
            "42 8a 04 2b (%rbx,%r13) al",
            &[0x42, 0x8a, 0x04, 0x2b],
            DATA + 3,
            Register::Rax,
            0,
        ),
        (
            "43 8a 04 2e (%r14,%r13) al",
            &[0x43, 0x8a, 0x04, 0x2e],
            DATA + 0x100 + 3,
            Register::Rax,
            0,
        ),
        (
            "47 8a 0c 2e (%r14,%r13) r9b",
            &[0x47, 0x8a, 0x0c, 0x2e],
            DATA + 0x100 + 3,
            Register::R9,
            0,
        ),
        (
            "41 8a 04 5e (%r14,%rbx,2) al",
            &[0x41, 0x8a, 0x04, 0x5e],
            DATA + 0x100 + 2 * DATA,
            Register::Rax,
            0,
        ),
        // disp8 (sign-extended), disp32, a SIB without a base.
        (
            "8a 43 05 5(%rbx)",
            &[0x8a, 0x43, 0x05],
            DATA + 5,
            Register::Rax,
            0,
        ),
        (
            "8a 43 fe -2(%rbx)",
            &[0x8a, 0x43, 0xfe],
            DATA - 2,
            Register::Rax,
            0,
        ),
        (
            "8a 83 00 01 00 00 0x100(%rbx)",
            &[0x8a, 0x83, 0x00, 0x01, 0x00, 0x00],
            DATA + 0x100,
            Register::Rax,
            0,
        ),
        (
            "8a 04 dd 00 10 60 00 0x601000(,%rbx,8)",
            &[0x8a, 0x04, 0xdd, 0x00, 0x10, 0x60, 0x00],
            0x60_1000 + 8 * DATA,
            Register::Rax,
            0,
        ),
        // RIP-relative: the base is the end of the instruction; REX.B does not make it r13.
        (
            "8a 05 10 00 00 00 0x10(%rip)",
            &[0x8a, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 6 + 0x10,
            Register::Rax,
            0,
        ),
        (
            "41 8a 05 10 00 00 00 REX.B on rip-relative",
            &[0x41, 0x8a, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 7 + 0x10,
            Register::Rax,
            0,
        ),
        // REX.W does not change a byte load.
        (
            "48 8a 03 (%rbx) REX.W",
            &[0x48, 0x8a, 0x03],
            DATA,
            Register::Rax,
            0,
        ),
    ];
    for (name, bytes, target, register, shift) in forms {
        for flags in [0, ARITH] {
            let mut state = mov_byte_state();
            state.gpr[Register::Rsi as usize] = DATA + 0x20;
            state.rflags |= flags;
            let mut expected = with_byte(&state, *register, *shift, 0x9c);
            expected.rip = RIP + bytes.len() as u64;
            let mut memory = DataMemory::new(bytes)
                .with_user(*target, 0x9c)
                .with_user(target - 1, 0x11)
                .with_user(target + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.reads, 1, "{name}");
            assert!(memory.writes.is_empty(), "{name}");
        }
    }
}

#[test]
fn movb_load_zero_and_high_values_leave_the_upper_bytes_of_rax() {
    // al = 0x00, 0xff and 0x80: bits 8..63 of rax keep their value (no zero-extension).
    for value in [0x00_u8, 0xff, 0x80] {
        let mut state = mov_byte_state();
        state.gpr[Register::Rax as usize] = 0xfedc_ba98_7654_3210;
        let mut memory = DataMemory::new(&[0x8a, 0x03]).with_user(DATA, value);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0xfedc_ba98_7654_3200 | u64::from(value)
        );
    }
}

#[test]
fn movb_load_from_a_supervisor_page_is_a_user_read_page_fault_without_a_register_write() {
    for bytes in [&[0x8a, 0x03][..], &[0x43, 0x8a, 0x04, 0x2e][..]] {
        let mut state = mov_byte_state();
        let before = state.clone();
        let fault_at = if bytes.len() == 2 { DATA } else { DATA + 0x103 };
        let mut memory = DataMemory::new(bytes).with_supervisor(fault_at, 0xcc);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        // P (the page is present) | U (a CPL3 access), no W: a read.
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x5),
                linear_address: Some(fault_at),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
        assert_eq!(memory.supervisor[&fault_at], 0xcc);
    }
}

#[test]
fn movb_load_from_a_page_without_p_is_a_not_present_user_page_fault() {
    let mut state = mov_byte_state();
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x8a, 0x03]).with_absent(DATA);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    // U only: not present, a read by CPL3.
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x4),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn movb_load_from_a_non_canonical_address_is_a_general_protection_fault() {
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x8a, 0x03]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13 && fault.error_code == Some(0)),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn movb_load_from_a_non_canonical_stack_base_is_a_stack_fault() {
    // `8a 04 24`: `movb (%rsp), %al`; RSP as the base goes through SS, so the fault is #SS(0).
    let mut state = mov_byte_state();
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x8a, 0x04, 0x24]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 12 && fault.error_code == Some(0)),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn movb_load_sib_with_rex_x_or_without_a_base_uses_the_decoded_registers() {
    // `42 8a 04 23`: REX.X makes the index r12 (base rbx): the byte at rbx + r12.
    let mut state = mov_byte_state();
    state.gpr[Register::R12 as usize] = 0x40;
    let target = DATA + 0x40;
    let mut expected = with_byte(&state, Register::Rax, 0, 0x9c);
    expected.rip = RIP + 4;
    let mut memory = DataMemory::new(&[0x42, 0x8a, 0x04, 0x23])
        .with_user(target, 0x9c)
        .with_user(target - 1, 0x11)
        .with_user(target + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);

    // `41 8a 04 25 disp32`: SIB base 101 with mod 00 has no base even with REX.B (r13 is not used).
    let state_before = mov_byte_state();
    let mut state = state_before.clone();
    let mut expected = with_byte(&state, Register::Rax, 0, 0x9c);
    expected.rip = RIP + 8;
    let mut memory = DataMemory::new(&[0x41, 0x8a, 0x04, 0x25, 0x00, 0x10, 0x60, 0x00])
        .with_user(DATA, 0x9c)
        .with_user(DATA - 1, 0x11)
        .with_user(DATA + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
}

#[test]
fn movb_load_refusals_stop_as_unsupported_8a_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("8a c0 mov al, al (mod = 11)", &[0x8a, 0xc0]),
        ("8a d9 mov bl, cl (mod = 11)", &[0x8a, 0xd9]),
        ("44 8a c3 mov r8b, bl (mod = 11)", &[0x44, 0x8a, 0xc3]),
        ("41 8a c0 mov al, r8b (mod = 11)", &[0x41, 0x8a, 0xc0]),
        ("66 8a 03", &[0x66, 0x8a, 0x03]),
        ("67 8a 03", &[0x67, 0x8a, 0x03]),
        ("f0 8a 03 LOCK", &[0xf0, 0x8a, 0x03]),
        ("f2 8a 03 REPNE", &[0xf2, 0x8a, 0x03]),
        ("f3 8a 03 REP", &[0xf3, 0x8a, 0x03]),
        ("64 8a 03 FS", &[0x64, 0x8a, 0x03]),
        ("65 8a 03 GS", &[0x65, 0x8a, 0x03]),
        ("26 8a 03 ES", &[0x26, 0x8a, 0x03]),
        ("2e 8a 03 CS", &[0x2e, 0x8a, 0x03]),
        ("36 8a 03 SS", &[0x36, 0x8a, 0x03]),
        ("3e 8a 03 DS", &[0x3e, 0x8a, 0x03]),
        ("2e 43 8a 04 2e", &[0x2e, 0x43, 0x8a, 0x04, 0x2e]),
        ("3e 43 8a 04 2e", &[0x3e, 0x43, 0x8a, 0x04, 0x2e]),
        ("66 41 8a 06", &[0x66, 0x41, 0x8a, 0x06]),
        ("67 41 8a 06", &[0x67, 0x41, 0x8a, 0x06]),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x8a },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn neighbouring_byte_mov_and_compare_opcodes_stay_outside() {
    // These byte stores and the register MOV remain outside this test's CPL3 slice.
    let forms: &[(&str, &[u8], u8)] = &[
        ("67 88 03 movb %al, (%ebx)", &[0x67, 0x88, 0x03], 0x88),
        ("8b c3 mov eax, ebx", &[0x8b, 0xc3], 0x8b),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = mov_byte_state();
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.reads, 0, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

#[test]
fn init_movb_load_then_cmpb_then_jne_then_incq_then_jmp_chain_stops_at_prefixed_jmp_rel32() {
    // Synthetic bytes modelled on the `/init` loop (the real `/init` at 0x40016f has `jne +0x0a`
    // and `incq %rsi`/`incq %rdi` at 0x400175/0x400178): `8a 06` (movb (%rsi), %al), `38 07`
    // (cmpb %al, (%rdi); executes since S4-39f11), `75 00` (jne rel8 with displacement 0: taken and
    // not taken reach the same RIP, so this test does not tell them apart), `48 ff c6` (incq %rsi;
    // executes since S4-39f12), `eb 00` (jmp rel8, executes since S4-39f13) and `48 e9` (prefixed jmp rel32).
    let mut state = mov_byte_state();
    state.gpr[Register::Rsi as usize] = DATA;
    state.gpr[Register::Rdi as usize] = DATA + 0x40;
    state.rflags |= ARITH;
    let mut expected = with_byte(&state, Register::Rax, 0, b'a');
    expected.rip = RIP + 11;
    expected.gpr[Register::Rsi as usize] = DATA + 1;
    // (rdi) - al = 'b' - 'a' = 1: no arithmetic flag set; DATA + 1 has none either.
    expected.rflags &= !ARITH;
    let mut memory = DataMemory::new(&[
        0x8a, 0x06, 0x38, 0x07, 0x75, 0x00, 0x48, 0xff, 0xc6, 0xeb, 0x00, 0x48, 0xe9, 0x00, 0x00,
        0x00, 0x00,
    ])
    .with_user(DATA, b'a')
    .with_user(DATA + 0x40, b'b');
    let result = run3_data(&mut state, &mut memory, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xe9 });
    assert_eq!(result.instructions_executed, 5);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 2);
    assert!(memory.writes.is_empty());
}

// ---- S4-39f11: `[REX] 38 /r` (CMP r/m8, r8) with a memory operand ----

/// `before` after a compare that retired: `len` bytes of RIP, the six arithmetic flags replaced by
/// `flags`, nothing else changed.
fn after_cmpb(before: &LongModeState, len: usize, flags: u64) -> LongModeState {
    let mut expected = before.clone();
    expected.rip = RIP + len as u64;
    expected.rflags = (expected.rflags & !ARITH) | flags;
    expected
}

#[test]
fn cmpb_sets_the_byte_sub_flags_and_writes_nothing() {
    // `38 03`: `cmpb %al, (%rbx)`: flags of `(%rbx) - %al` at byte size (the rest of rax is
    // ignored). (name, memory byte, al, flags)
    let cases: &[(&str, u8, u8, u64)] = &[
        ("0-5", 0x00, 0x05, CF | SF | AF),
        ("equal", 0x07, 0x07, ZF | PF),
        ("5-3", 0x05, 0x03, 0),
        ("0x10-1", 0x10, 0x01, AF | PF),
        // 0x80 - 1 = 0x7f: signed overflow and the low-nibble borrow.
        ("0x80-1", 0x80, 0x01, OF | AF),
        // 127 - (-1) = 128: CF, SF and OF; no nibble borrow.
        ("0x7f-0xff", 0x7f, 0xff, CF | SF | OF),
        ("0-0x80", 0x00, 0x80, CF | SF | OF),
        ("0xff-1", 0xff, 0x01, SF),
        ("0x80-0x80", 0x80, 0x80, ZF | PF),
        // 0x0f - 0x01 = 0x0e: three set bits (odd), PF clear.
        ("0x0f-1", 0x0f, 0x01, 0),
        // 0x04 - 0x01 = 0x03: two set bits (even), PF.
        ("4-1", 0x04, 0x01, PF),
    ];
    for (name, byte, al, flags) in cases {
        for initial in [0, ARITH] {
            let mut state = mov_byte_state();
            state.gpr[Register::Rax as usize] = 0xffff_ffff_ffff_ff00 | u64::from(*al);
            state.rflags |= initial;
            let expected = after_cmpb(&state, 2, *flags);
            let mut memory = DataMemory::new(&[0x38, 0x03])
                .with_user(DATA, *byte)
                .with_user(DATA - 1, 0x11)
                .with_user(DATA + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.reads, 1, "{name}");
            assert!(memory.writes.is_empty(), "{name}");
            assert_eq!(memory.user[&DATA], *byte, "{name}");
        }
    }
}

#[test]
fn init_cmpb_with_rdi_base_compares_the_byte_at_rdi_with_al() {
    // `38 07` of `/init` at 0x400171: `cmpb %al, (%rdi)`.
    for (al, byte, flags) in [
        (b'a', b'a', ZF | PF),
        (b'a', b'b', 0),
        (b'b', b'a', CF | SF | AF | PF),
    ] {
        let mut state = mov_byte_state();
        state.gpr[Register::Rax as usize] = 0x1234_5678_9abc_de00 | u64::from(al);
        state.gpr[Register::Rdi as usize] = DATA + 0x40;
        let expected = after_cmpb(&state, 2, flags);
        let mut memory = DataMemory::new(&[0x38, 0x07]).with_user(DATA + 0x40, byte);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state, expected, "{al:#x} {byte:#x}");
        assert_eq!(memory.reads, 1);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cmpb_forms_compare_the_selected_register_byte_with_the_addressed_byte() {
    // (name, bytes, effective address, register, shift of the byte inside it). The selected byte
    // is set to 0x31 and the addressed byte is 0x31: ZF | PF only when both are the right ones
    // (no other register byte of `mov_byte_state` is 0x31).
    let forms: &[(&str, &[u8], u64, Register, u32)] = &[
        ("38 03 (%rbx) al", &[0x38, 0x03], DATA, Register::Rax, 0),
        ("38 0b (%rbx) cl", &[0x38, 0x0b], DATA, Register::Rcx, 0),
        ("38 13 (%rbx) dl", &[0x38, 0x13], DATA, Register::Rdx, 0),
        // No REX: ModRM.reg 4..6 are AH, CH, DH.
        ("38 23 (%rbx) ah", &[0x38, 0x23], DATA, Register::Rax, 8),
        ("38 2b (%rbx) ch", &[0x38, 0x2b], DATA, Register::Rcx, 8),
        ("38 33 (%rbx) dh", &[0x38, 0x33], DATA, Register::Rdx, 8),
        // Any REX: 4..7 are SPL, BPL, SIL, DIL.
        (
            "40 38 23 (%rbx) spl",
            &[0x40, 0x38, 0x23],
            DATA,
            Register::Rsp,
            0,
        ),
        (
            "40 38 2b (%rbx) bpl",
            &[0x40, 0x38, 0x2b],
            DATA,
            Register::Rbp,
            0,
        ),
        (
            "40 38 33 (%rbx) sil",
            &[0x40, 0x38, 0x33],
            DATA,
            Register::Rsi,
            0,
        ),
        (
            "40 38 3b (%rbx) dil",
            &[0x40, 0x38, 0x3b],
            DATA,
            Register::Rdi,
            0,
        ),
        // REX.R: register r8b..r15b; REX.B: base r8..r15; REX.X: index.
        (
            "44 38 03 (%rbx) r8b",
            &[0x44, 0x38, 0x03],
            DATA,
            Register::R8,
            0,
        ),
        (
            "44 38 3b (%rbx) r15b",
            &[0x44, 0x38, 0x3b],
            DATA,
            Register::R15,
            0,
        ),
        (
            "41 38 06 (%r14) al",
            &[0x41, 0x38, 0x06],
            DATA + 0x100,
            Register::Rax,
            0,
        ),
        (
            "42 38 04 2b (%rbx,%r13) al",
            &[0x42, 0x38, 0x04, 0x2b],
            DATA + 3,
            Register::Rax,
            0,
        ),
        (
            "43 38 04 2e (%r14,%r13) al",
            &[0x43, 0x38, 0x04, 0x2e],
            DATA + 0x100 + 3,
            Register::Rax,
            0,
        ),
        (
            "47 38 0c 2e (%r14,%r13) r9b",
            &[0x47, 0x38, 0x0c, 0x2e],
            DATA + 0x100 + 3,
            Register::R9,
            0,
        ),
        (
            "41 38 04 5e (%r14,%rbx,2) al",
            &[0x41, 0x38, 0x04, 0x5e],
            DATA + 0x100 + 2 * DATA,
            Register::Rax,
            0,
        ),
        // disp8 (sign-extended), disp32, a SIB without a base.
        (
            "38 43 05 5(%rbx)",
            &[0x38, 0x43, 0x05],
            DATA + 5,
            Register::Rax,
            0,
        ),
        (
            "38 43 fe -2(%rbx)",
            &[0x38, 0x43, 0xfe],
            DATA - 2,
            Register::Rax,
            0,
        ),
        (
            "38 83 00 01 00 00 0x100(%rbx)",
            &[0x38, 0x83, 0x00, 0x01, 0x00, 0x00],
            DATA + 0x100,
            Register::Rax,
            0,
        ),
        (
            "38 04 dd 00 10 60 00 0x601000(,%rbx,8)",
            &[0x38, 0x04, 0xdd, 0x00, 0x10, 0x60, 0x00],
            0x60_1000 + 8 * DATA,
            Register::Rax,
            0,
        ),
        // RIP-relative: the base is the end of the instruction; REX.B does not make it r13.
        (
            "38 05 10 00 00 00 0x10(%rip)",
            &[0x38, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 6 + 0x10,
            Register::Rax,
            0,
        ),
        (
            "41 38 05 10 00 00 00 REX.B on rip-relative",
            &[0x41, 0x38, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 7 + 0x10,
            Register::Rax,
            0,
        ),
        // REX.W does not change a byte compare.
        (
            "48 38 03 (%rbx) REX.W",
            &[0x48, 0x38, 0x03],
            DATA,
            Register::Rax,
            0,
        ),
    ];
    for (name, bytes, target, register, shift) in forms {
        for initial in [0, ARITH] {
            let mut state = with_byte(&mov_byte_state(), *register, *shift, 0x31);
            state.rflags |= initial;
            let expected = after_cmpb(&state, bytes.len(), ZF | PF);
            let mut memory = DataMemory::new(bytes)
                .with_user(*target, 0x31)
                .with_user(target - 1, 0x11)
                .with_user(target + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.reads, 1, "{name}");
            assert!(memory.writes.is_empty(), "{name}");
            assert_eq!(memory.user[target], 0x31, "{name}");
        }
    }
}

#[test]
fn cmpb_sib_with_rex_x_or_without_a_base_uses_the_decoded_registers() {
    // `42 38 04 23`: REX.X makes the index r12 (base rbx): the byte at rbx + r12.
    let mut state = with_byte(&mov_byte_state(), Register::Rax, 0, 0x31);
    state.gpr[Register::R12 as usize] = 0x40;
    let target = DATA + 0x40;
    let expected = after_cmpb(&state, 4, ZF | PF);
    let mut memory = DataMemory::new(&[0x42, 0x38, 0x04, 0x23])
        .with_user(target, 0x31)
        .with_user(target - 1, 0x11)
        .with_user(target + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);

    // `41 38 04 25 disp32`: SIB base 101 with mod 00 has no base even with REX.B (r13 is unused).
    let mut state = with_byte(&mov_byte_state(), Register::Rax, 0, 0x31);
    let expected = after_cmpb(&state, 8, ZF | PF);
    let mut memory = DataMemory::new(&[0x41, 0x38, 0x04, 0x25, 0x00, 0x10, 0x60, 0x00])
        .with_user(DATA, 0x31)
        .with_user(DATA - 1, 0x11)
        .with_user(DATA + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
}

#[test]
fn cmpb_from_a_supervisor_page_is_a_user_read_page_fault_without_a_state_change() {
    for bytes in [&[0x38, 0x03][..], &[0x43, 0x38, 0x04, 0x2e][..]] {
        let mut state = mov_byte_state();
        state.rflags |= ARITH;
        let before = state.clone();
        let fault_at = if bytes.len() == 2 { DATA } else { DATA + 0x103 };
        let mut memory = DataMemory::new(bytes).with_supervisor(fault_at, 0xcc);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        // P (the page is present) | U (a CPL3 access), no W: a read.
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x5),
                linear_address: Some(fault_at),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
        assert_eq!(memory.supervisor[&fault_at], 0xcc);
    }
}

#[test]
fn cmpb_from_a_page_without_p_is_a_not_present_user_page_fault() {
    let mut state = mov_byte_state();
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x38, 0x03]).with_absent(DATA);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    // U only: not present, a read by CPL3.
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x4),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn cmpb_at_a_non_canonical_address_is_a_general_protection_fault() {
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x38, 0x03]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13 && fault.error_code == Some(0)),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cmpb_at_a_non_canonical_stack_base_is_a_stack_fault() {
    // `38 04 24`: `cmpb %al, (%rsp)` and `38 45 00`: `cmpb %al, 0(%rbp)`; RSP and RBP as the base
    // go through SS, so the fault is #SS(0).
    for (bytes, register) in [
        (&[0x38, 0x04, 0x24][..], Register::Rsp),
        (&[0x38, 0x45, 0x00][..], Register::Rbp),
    ] {
        let mut state = mov_byte_state();
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 12 && fault.error_code == Some(0)),
            "{:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn cmpb_refusals_stop_as_unsupported_38_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("67 38 c0 addr32 register form", &[0x67, 0x38, 0xc0]),
        ("66 38 03", &[0x66, 0x38, 0x03]),
        ("67 38 03", &[0x67, 0x38, 0x03]),
        ("f0 38 03 LOCK", &[0xf0, 0x38, 0x03]),
        ("f2 38 03 REPNE", &[0xf2, 0x38, 0x03]),
        ("f3 38 03 REP", &[0xf3, 0x38, 0x03]),
        ("64 38 03 FS", &[0x64, 0x38, 0x03]),
        ("65 38 03 GS", &[0x65, 0x38, 0x03]),
        ("26 38 03 ES", &[0x26, 0x38, 0x03]),
        ("2e 38 03 CS", &[0x2e, 0x38, 0x03]),
        ("36 38 03 SS", &[0x36, 0x38, 0x03]),
        ("3e 38 03 DS", &[0x3e, 0x38, 0x03]),
        ("2e 43 38 04 2e", &[0x2e, 0x43, 0x38, 0x04, 0x2e]),
        ("3e 43 38 04 2e", &[0x3e, 0x43, 0x38, 0x04, 0x2e]),
        ("66 41 38 06", &[0x66, 0x41, 0x38, 0x06]),
        ("67 41 38 06", &[0x67, 0x41, 0x38, 0x06]),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x38 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn neighbouring_compare_opcodes_stay_outside_after_cmpb() {
    // The other CMP forms with a memory operand are not part of S4-39f11 (`80 /7` joined in
    // S4-39f14, `3C ib` in S4-39f18).
    let forms: &[(&str, &[u8], u8)] = &[
        ("3a 03 cmpb (%rbx), %al", &[0x3a, 0x03], 0x3a),
        ("3b 03 cmpl (%rbx), %eax", &[0x3b, 0x03], 0x3b),
        (
            "81 3b 00 00 00 00 cmpl $0, (%rbx)",
            &[0x81, 0x3b, 0, 0, 0, 0],
            0x81,
        ),
        (
            "66 81 3b 01 00 cmpw $1, (%rbx)",
            &[0x66, 0x81, 0x3b, 0x01, 0x00],
            0x81,
        ),
        (
            "67 48 39 03 cmpq %rax, (%ebx)",
            &[0x67, 0x48, 0x39, 0x03],
            0x39,
        ),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = mov_byte_state();
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.reads, 0, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

// ---- S4-39f12: `REX.W FF /0` (INC r/m64), register form ----

/// The arithmetic flags of `value + 1` per the SDM (INC): OF, SF, ZF, AF, PF; CF is not among them.
fn inc64_flags(value: u64) -> u64 {
    let result = value.wrapping_add(1);
    let mut flags = 0;
    if value == i64::MAX as u64 {
        flags |= OF;
    }
    if result >> 63 != 0 {
        flags |= SF;
    }
    if result == 0 {
        flags |= ZF;
    }
    if value & 0xf == 0xf {
        flags |= AF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= PF;
    }
    flags
}

#[test]
fn inc_r64_register_form_sets_the_add_one_flags_and_keeps_cf() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, register index, value, expected result, expected OF/SF/ZF/AF/PF).
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, u64, u64)] = &[
        // `/init` at 0x40017f: incq %rbx.
        ("48 ff c3 0", &[0x48, 0xff, 0xc3], 3, 0, 1, 0),
        ("48 ff c3 1", &[0x48, 0xff, 0xc3], 3, 1, 2, 0),
        // Result 3 has two set bits: PF.
        ("48 ff c3 2", &[0x48, 0xff, 0xc3], 3, 2, 3, PF),
        // Low-nibble carry: 0x0f + 1 = 0x10 (one set bit: PF clear): AF only.
        ("48 ff c3 0x0f", &[0x48, 0xff, 0xc3], 3, 0x0f, 0x10, AF),
        // 0xff + 1 = 0x100: AF; low byte 0 (PF); the carry out of the byte stays in the qword.
        (
            "48 ff c3 0xff",
            &[0x48, 0xff, 0xc3],
            3,
            0xff,
            0x100,
            AF | PF,
        ),
        // Signed overflow: i64::MAX + 1 = i64::MIN: OF, SF, AF, PF (low byte 0).
        (
            "48 ff c3 max",
            &[0x48, 0xff, 0xc3],
            3,
            0x7fff_ffff_ffff_ffff,
            0x8000_0000_0000_0000,
            OF | SF | AF | PF,
        ),
        // All ones + 1 = 0: ZF, AF, PF; CF is NOT set (it is kept).
        (
            "48 ff c3 -1",
            &[0x48, 0xff, 0xc3],
            3,
            u64::MAX,
            0,
            ZF | AF | PF,
        ),
        // The carry crosses the dword boundary: a 64-bit write, no dword wrap.
        (
            "48 ff c3 dword max",
            &[0x48, 0xff, 0xc3],
            3,
            0xffff_ffff,
            0x1_0000_0000,
            AF | PF,
        ),
        // -2 + 1 = -1: SF; low nibble e (no AF); low byte 0xff (eight bits): PF.
        (
            "48 ff c3 -2",
            &[0x48, 0xff, 0xc3],
            3,
            u64::MAX - 1,
            u64::MAX,
            SF | PF,
        ),
        // `/init` at 0x400175 and 0x400178: incq %rsi, incq %rdi.
        (
            "48 ff c6 incq %rsi",
            &[0x48, 0xff, 0xc6],
            6,
            0x60_1000,
            0x60_1001,
            0,
        ),
        (
            "48 ff c7 incq %rdi",
            &[0x48, 0xff, 0xc7],
            7,
            0x60_1fff,
            0x60_2000,
            AF | PF,
        ),
        // The other legacy registers.
        ("48 ff c0 incq %rax", &[0x48, 0xff, 0xc0], 0, 7, 8, 0),
        ("48 ff c1 incq %rcx", &[0x48, 0xff, 0xc1], 1, 7, 8, 0),
        ("48 ff c2 incq %rdx", &[0x48, 0xff, 0xc2], 2, 7, 8, 0),
        ("48 ff c4 incq %rsp", &[0x48, 0xff, 0xc4], 4, 7, 8, 0),
        ("48 ff c5 incq %rbp", &[0x48, 0xff, 0xc5], 5, 7, 8, 0),
        // REX.WB: r8..r15.
        ("49 ff c0 incq %r8", &[0x49, 0xff, 0xc0], 8, 7, 8, 0),
        ("49 ff c1 incq %r9", &[0x49, 0xff, 0xc1], 9, 7, 8, 0),
        ("49 ff c2 incq %r10", &[0x49, 0xff, 0xc2], 10, 7, 8, 0),
        ("49 ff c3 incq %r11", &[0x49, 0xff, 0xc3], 11, 7, 8, 0),
        ("49 ff c4 incq %r12", &[0x49, 0xff, 0xc4], 12, 7, 8, 0),
        ("49 ff c5 incq %r13", &[0x49, 0xff, 0xc5], 13, 7, 8, 0),
        ("49 ff c6 incq %r14", &[0x49, 0xff, 0xc6], 14, 7, 8, 0),
        (
            "49 ff c7 incq %r15 max",
            &[0x49, 0xff, 0xc7],
            15,
            0x7fff_ffff_ffff_ffff,
            0x8000_0000_0000_0000,
            OF | SF | AF | PF,
        ),
        // REX.WR and REX.WX are ignored for a register rm; REX.WRXB reaches r15.
        ("4c ff c3 REX.WR", &[0x4c, 0xff, 0xc3], 3, 7, 8, 0),
        ("4a ff c3 REX.WX", &[0x4a, 0xff, 0xc3], 3, 7, 8, 0),
        ("4f ff c7 REX.WRXB", &[0x4f, 0xff, 0xc7], 15, 7, 8, 0),
    ];
    for (name, bytes, index, value, result_value, flags) in cases {
        assert_eq!(inc64_flags(*value), *flags, "{name} table");
        assert_eq!(value.wrapping_add(1), *result_value, "{name} table");
        // Four starting flag sets: INC replaces the five flags and never touches CF.
        for before_flags in [0, CF, ARITH & !CF, ARITH] {
            let mut state = user_state();
            for (i, register) in state.gpr.iter_mut().enumerate() {
                *register = 0x1111_1111_1111_1111_u64.wrapping_mul(i as u64 + 1);
            }
            state.gpr[*index] = *value;
            state.rflags |= before_flags;
            let mut expected = state.clone();
            expected.rip = RIP + bytes.len() as u64;
            expected.gpr[*index] = *result_value;
            expected.rflags = (expected.rflags & !ARITH) | (before_flags & CF) | flags;
            let mut memory = Memory::code(bytes);
            let result = run3(&mut state, &mut memory, &set, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name} {before_flags:#x}"
            );
            assert_eq!(result.instructions_executed, 1, "{name} {before_flags:#x}");
            // Only the destination, RIP and the flags change.
            assert_eq!(state, expected, "{name} {before_flags:#x}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.accesses, 0, "{name}");
        }
    }
}

#[test]
fn inc_r64_wraps_to_zero_with_zf_and_keeps_cf_in_both_states() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for carry in [0, CF] {
        let mut state = user_state();
        state.gpr[Register::Rbx as usize] = u64::MAX;
        state.rflags |= carry;
        let mut memory = Memory::code(&[0x48, 0xff, 0xc3]);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(state.gpr[Register::Rbx as usize], 0);
        assert_eq!(state.rip, RIP + 3);
        assert_eq!(state.rflags & ARITH, carry | ZF | AF | PF);
    }
}

#[test]
fn inc_r64_keeps_the_carry_for_a_following_jb() {
    // `4c 39 eb` (cmpq %r13, %rbx: 0 - 5 sets CF), `48 ff c3` (incq %rbx keeps CF), `72 01`
    // (jb +1, taken on CF), a skipped `90`, `90`.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = 0;
    state.gpr[Register::R13 as usize] = 5;
    let mut memory = Memory::code(&[0x4c, 0x39, 0xeb, 0x48, 0xff, 0xc3, 0x72, 0x01, 0x90, 0x90]);
    let result = run3(&mut state, &mut memory, &set, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[Register::Rbx as usize], 1);
    // jb taken: 3 + 3 + 2 + 1.
    assert_eq!(state.rip, RIP + 9);
    assert_eq!(state.rflags & CF, CF);
}

#[test]
fn ff_forms_outside_the_register_inc_r64_are_refused_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        // Dword INC (no REX.W) and REX without W.
        ("41 ff c3 incl %r11d", &[0x41, 0xff, 0xc3], 0xff),
        ("40 ff c3 REX incl %ebx", &[0x40, 0xff, 0xc3], 0xff),
        ("41 ff c3 REX.B incl %r11d", &[0x41, 0xff, 0xc3], 0xff),
        // INC word memory outside the byte/dword/qword RMW forms.
        ("66 ff 03 incw (%rbx)", &[0x66, 0xff, 0x03], 0xff),
        // FF /1 (DEC), register form with REX.W (the dword form joined in S4-39f15).
        ("48 ff c9 decq %rcx", &[0x48, 0xff, 0xc9], 0xff),
        // FF /2../7: call, lcall, jmp, ljmp, push, /7.
        ("48 ff d3 call *%rbx", &[0x48, 0xff, 0xd3], 0xff),
        ("48 ff db lcall *%rbx", &[0x48, 0xff, 0xdb], 0xff),
        ("48 ff e3 jmp *%rbx", &[0x48, 0xff, 0xe3], 0xff),
        ("48 ff eb ljmp *%rbx", &[0x48, 0xff, 0xeb], 0xff),
        ("48 ff f3 push %rbx", &[0x48, 0xff, 0xf3], 0xff),
        ("48 ff fb /7", &[0x48, 0xff, 0xfb], 0xff),
        // INC r/m8 is the opcode FE.
        ("fe c3 incb %bl", &[0xfe, 0xc3], 0xfe),
        ("48 fe c3 incb %bl", &[0x48, 0xfe, 0xc3], 0xfe),
        // Prefixes in front of the allowed form.
        ("66 48 ff c3", &[0x66, 0x48, 0xff, 0xc3], 0xff),
        ("67 48 ff c3", &[0x67, 0x48, 0xff, 0xc3], 0xff),
        ("f0 48 ff c3", &[0xf0, 0x48, 0xff, 0xc3], 0xff),
        ("f2 48 ff c3", &[0xf2, 0x48, 0xff, 0xc3], 0xff),
        ("f3 48 ff c3", &[0xf3, 0x48, 0xff, 0xc3], 0xff),
        ("64 48 ff c3", &[0x64, 0x48, 0xff, 0xc3], 0xff),
        ("65 48 ff c3", &[0x65, 0x48, 0xff, 0xc3], 0xff),
        ("2e 48 ff c3", &[0x2e, 0x48, 0xff, 0xc3], 0xff),
        ("3e 48 ff c3", &[0x3e, 0x48, 0xff, 0xc3], 0xff),
        ("26 48 ff c3", &[0x26, 0x48, 0xff, 0xc3], 0xff),
        ("36 48 ff c3", &[0x36, 0x48, 0xff, 0xc3], 0xff),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        state.gpr[Register::Rbx as usize] = 5;
        state.gpr[Register::Rsp as usize] = DATA;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn init_incq_then_jmp_chain_jumps_back() {
    // `/init` at 0x40017f: `48 ff c3` (incq %rbx) and `eb c1` (jmp rel8, since S4-39f13) retire;
    // the jump goes back to RIP + 5 - 63.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = 41;
    let mut memory = Memory::code(&[0x48, 0xff, 0xc3, 0xeb, 0xc1]);
    let result = run3(&mut state, &mut memory, &set, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + 5 - 63);
    assert_eq!(state.gpr[Register::Rbx as usize], 42);
}

// ---- S4-39f13: `EB cb` (JMP rel8) ----

#[test]
fn jmp_rel8_goes_to_next_rip_plus_the_sign_extended_displacement() {
    // `eb c1` is the `/init` jump at 0x400182 (target 0x400145 = 0x400184 - 63); the budget of one
    // instruction stops right after the jump, and the code buffer is not needed at the target.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u64)] = &[
        ("eb c1 backward", &[0xeb, 0xc1], RIP + 2 - 63),
        ("eb 05 forward", &[0xeb, 0x05], RIP + 2 + 5),
        ("eb 00 next", &[0xeb, 0x00], RIP + 2),
        ("eb 7f maximum forward", &[0xeb, 0x7f], RIP + 2 + 127),
        ("eb 80 maximum backward", &[0xeb, 0x80], RIP + 2 - 128),
        ("eb fe to itself", &[0xeb, 0xfe], RIP),
    ];
    for (name, bytes, target) in forms {
        let mut state = user_state();
        let mut expected = state.clone();
        expected.rip = *target;
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        assert_eq!(state, expected, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
    }
}

#[test]
fn jmp_rel8_keeps_every_flag_combination() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for bits in 0..32_u64 {
        let flags = [CF, ZF, SF, OF, PF]
            .iter()
            .enumerate()
            .filter(|(index, _)| bits & (1 << index) != 0)
            .fold(0, |acc, (_, flag)| acc | flag);
        let mut state = user_state();
        state.rflags |= flags;
        let mut expected = state.clone();
        expected.rip = RIP + 5;
        let mut memory = Memory::code(&[0xeb, 0x02, 0xf4, 0xf4, 0x90]);
        let result = run3(&mut state, &mut memory, &set, 2).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{bits:#x}"
        );
        assert_eq!(result.instructions_executed, 2, "{bits:#x}");
        assert_eq!(state, expected, "{bits:#x}");
    }
}

#[test]
fn jmp_rel8_loops_on_itself_without_writing_anything() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0xeb, 0xfe]);
    let result = run3(&mut state, &mut memory, &set, 5).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 5);
    assert_eq!(state, before);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn jmp_rel8_to_a_non_canonical_target_is_a_general_protection_fault_without_a_state_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // The first non-canonical address is 0x0000_8000_0000_0000; the lower hole edge is
    // 0xffff_8000_0000_0000.
    let cases: &[(&str, u64, &[u8])] = &[
        (
            "forward past the lower half",
            0x0000_7fff_ffff_fffe,
            &[0xeb, 0x05],
        ),
        (
            "forward to the first hole address",
            0x0000_7fff_ffff_fffe,
            &[0xeb, 0x00],
        ),
        (
            "backward below the upper half",
            0xffff_8000_0000_0000,
            &[0xeb, 0xfb],
        ),
    ];
    for (name, rip, bytes) in cases {
        let mut state = user_state();
        state.rip = *rip;
        let before = state.clone();
        let mut memory = Memory::code_at(*rip, bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13 && fault.error_code == Some(0)),
            "{name}: {:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
    // The last canonical addresses on both sides are legal targets.
    let mut state = user_state();
    state.rip = 0x0000_7fff_ffff_fffc;
    let mut memory = Memory::code_at(state.rip, &[0xeb, 0x00]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, 0x0000_7fff_ffff_fffe);
    let mut state = user_state();
    state.rip = 0xffff_8000_0000_0000;
    let mut memory = Memory::code_at(state.rip, &[0xeb, 0xfe]);
    let result = run3(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, 0xffff_8000_0000_0000);
}

#[test]
fn jmp_forms_other_than_unprefixed_relative_are_refused_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        ("66 eb 00", &[0x66, 0xeb, 0x00], 0xeb),
        ("67 eb 00", &[0x67, 0xeb, 0x00], 0xeb),
        ("f0 eb 00", &[0xf0, 0xeb, 0x00], 0xeb),
        ("f2 eb 00", &[0xf2, 0xeb, 0x00], 0xeb),
        ("f3 eb 00", &[0xf3, 0xeb, 0x00], 0xeb),
        ("64 eb 00", &[0x64, 0xeb, 0x00], 0xeb),
        ("65 eb 00", &[0x65, 0xeb, 0x00], 0xeb),
        ("2e eb 00", &[0x2e, 0xeb, 0x00], 0xeb),
        ("3e eb 00", &[0x3e, 0xeb, 0x00], 0xeb),
        ("26 eb 00", &[0x26, 0xeb, 0x00], 0xeb),
        ("36 eb 00", &[0x36, 0xeb, 0x00], 0xeb),
        ("40 eb 00 REX", &[0x40, 0xeb, 0x00], 0xeb),
        ("48 eb 00 REX.W", &[0x48, 0xeb, 0x00], 0xeb),
        ("41 eb 00 REX.B", &[0x41, 0xeb, 0x00], 0xeb),
        ("48 e9", &[0x48, 0xe9, 0, 0, 0, 0], 0xe9),
        ("66 e9 jmp", &[0x66, 0xe9, 0x00, 0x00, 0x00, 0x00], 0xe9),
        ("66 e8 rel32", &[0x66, 0xe8, 0x00, 0x00, 0x00, 0x00], 0xe8),
        ("ff e3 jmp *%rbx", &[0xff, 0xe3], 0xff),
        ("ff 20 jmp *(%rax)", &[0xff, 0x20], 0xff),
        ("ff 28 ljmp *(%rax)", &[0xff, 0x28], 0xff),
        ("ea ljmp", &[0xea, 0, 0, 0, 0, 0, 0], 0xea),
        ("e3 00 jrcxz", &[0xe3, 0x00], 0xe3),
        ("e2 00 loop", &[0xe2, 0x00], 0xe2),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn jmp_rel8_then_an_outside_form_stops_at_that_form() {
    // An operand-size-prefixed `cmp ax, imm16` remains outside the CPL3 allowlist.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let mut memory = Memory::code(&[0xeb, 0x01, 0xf4, 0x66, 0x3d, 0x20, 0x00]);
    let result = run3(&mut state, &mut memory, &set, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x3d });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 3);
}

// ---- S4-39f14: `[REX] 80 /7 ib` (CMP r/m8, imm8) with a memory operand ----

/// The arithmetic flags of `a - b` at byte size per the SDM (SUB/CMP): CF borrow, OF signed
/// overflow, SF, ZF, PF of the low byte, AF borrow from bit 4.
fn cmp8_flags(a: u8, b: u8) -> u64 {
    let result = a.wrapping_sub(b);
    let mut flags = 0;
    if a < b {
        flags |= CF;
    }
    if (a ^ b) & (a ^ result) & 0x80 != 0 {
        flags |= OF;
    }
    if result & 0x80 != 0 {
        flags |= SF;
    }
    if result == 0 {
        flags |= ZF;
    }
    if result.count_ones() % 2 == 0 {
        flags |= PF;
    }
    if (a & 0xf) < (b & 0xf) {
        flags |= AF;
    }
    flags
}

#[test]
fn cmp8_flags_helper_matches_known_values() {
    // Literal flags of `byte - 0x20`, so that the helper itself is checked. (byte, flags)
    let cases: &[(u8, u64)] = &[
        // 0x00 - 0x20 = 0xe0: three set bits, PF clear.
        (0x00, CF | SF),
        (0x20, ZF | PF),
        (0x7f, PF),
        (0x80, OF | PF),
        // 0xff - 0x20 = 0xdf: seven set bits, PF clear.
        (0xff, SF),
    ];
    for (byte, flags) in cases {
        assert_eq!(cmp8_flags(*byte, 0x20), *flags, "{byte:#x}");
    }
}

#[test]
fn init_cmpb_imm8_with_rex_b_sib_disp8_compares_the_byte_with_0x20() {
    // `41 80 7c 1e ff 20` of `/init` at 0x400153: `cmpb $0x20, -0x1(%r14,%rbx)`.
    let target = DATA + 0x100 + 3 - 1;
    for byte in [0x00_u8, 0x1f, 0x20, 0x21, 0x7f, 0x80, 0xa0, 0xff] {
        for initial in [0, ARITH] {
            let mut state = mov_byte_state();
            state.gpr[Register::Rbx as usize] = 3;
            state.rflags |= initial;
            let expected = after_cmpb(&state, 6, cmp8_flags(byte, 0x20));
            let mut memory = DataMemory::new(&[0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20])
                .with_user(target, byte)
                .with_user(target - 1, 0x11)
                .with_user(target + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(state, expected, "{byte:#x}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
            assert_eq!(memory.reads, 1);
            assert!(memory.writes.is_empty());
            assert_eq!(memory.user[&target], byte);
        }
    }
}

#[test]
fn cmpb_imm8_forms_compare_the_addressed_byte_with_the_immediate() {
    // (name, bytes, effective address). The addressed byte is 0x31 and the immediate 0x31:
    // ZF | PF only when the right byte is read.
    let forms: &[(&str, &[u8], u64)] = &[
        ("80 3b 31 (%rbx)", &[0x80, 0x3b, 0x31], DATA),
        (
            "41 80 3e 31 (%r14)",
            &[0x41, 0x80, 0x3e, 0x31],
            DATA + 0x100,
        ),
        (
            "42 80 3c 2b 31 (%rbx,%r13)",
            &[0x42, 0x80, 0x3c, 0x2b, 0x31],
            DATA + 3,
        ),
        (
            "43 80 3c 2e 31 (%r14,%r13)",
            &[0x43, 0x80, 0x3c, 0x2e, 0x31],
            DATA + 0x100 + 3,
        ),
        (
            "41 80 3c 5e 31 (%r14,%rbx,2)",
            &[0x41, 0x80, 0x3c, 0x5e, 0x31],
            DATA + 0x100 + 2 * DATA,
        ),
        ("80 7b 05 31 5(%rbx)", &[0x80, 0x7b, 0x05, 0x31], DATA + 5),
        ("80 7b fe 31 -2(%rbx)", &[0x80, 0x7b, 0xfe, 0x31], DATA - 2),
        (
            "80 bb 00 01 00 00 31 0x100(%rbx)",
            &[0x80, 0xbb, 0x00, 0x01, 0x00, 0x00, 0x31],
            DATA + 0x100,
        ),
        (
            "80 3c dd 00 10 60 00 31 0x601000(,%rbx,8)",
            &[0x80, 0x3c, 0xdd, 0x00, 0x10, 0x60, 0x00, 0x31],
            0x60_1000 + 8 * DATA,
        ),
        // RIP-relative: the base is the end of the instruction, imm8 included.
        (
            "80 3d 10 00 00 00 31 0x10(%rip)",
            &[0x80, 0x3d, 0x10, 0x00, 0x00, 0x00, 0x31],
            RIP + 7 + 0x10,
        ),
        (
            "41 80 3d 10 00 00 00 31 REX.B on rip-relative",
            &[0x41, 0x80, 0x3d, 0x10, 0x00, 0x00, 0x00, 0x31],
            RIP + 8 + 0x10,
        ),
        // REX.W does not change a byte compare.
        ("48 80 3b 31 (%rbx) REX.W", &[0x48, 0x80, 0x3b, 0x31], DATA),
        // REX.R does not extend the group selector (/7 stays CMP).
        ("44 80 3b 31 (%rbx) REX.R", &[0x44, 0x80, 0x3b, 0x31], DATA),
    ];
    for (name, bytes, target) in forms {
        for initial in [0, ARITH] {
            let mut state = mov_byte_state();
            state.rflags |= initial;
            let expected = after_cmpb(&state, bytes.len(), ZF | PF);
            let mut memory = DataMemory::new(bytes)
                .with_user(*target, 0x31)
                .with_user(target - 1, 0x11)
                .with_user(target + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.reads, 1, "{name}");
            assert!(memory.writes.is_empty(), "{name}");
            assert_eq!(memory.user[target], 0x31, "{name}");
        }
    }
}

#[test]
fn cmpb_imm8_sib_with_rex_x_or_without_a_base_uses_the_decoded_registers() {
    // `42 80 3c 23 31`: REX.X makes the index r12 (base rbx).
    let mut state = mov_byte_state();
    state.gpr[Register::R12 as usize] = 0x40;
    let target = DATA + 0x40;
    let expected = after_cmpb(&state, 5, ZF | PF);
    let mut memory = DataMemory::new(&[0x42, 0x80, 0x3c, 0x23, 0x31]).with_user(target, 0x31);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);

    // `41 80 3c 25 disp32 31`: SIB base 101 with mod 00 has no base even with REX.B.
    let mut state = mov_byte_state();
    let expected = after_cmpb(&state, 9, ZF | PF);
    let mut memory = DataMemory::new(&[0x41, 0x80, 0x3c, 0x25, 0x00, 0x10, 0x60, 0x00, 0x31])
        .with_user(DATA, 0x31);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
}

#[test]
fn cmpb_imm8_from_a_supervisor_page_is_a_user_read_page_fault_without_a_state_change() {
    for bytes in [
        &[0x80, 0x3b, 0x20][..],
        &[0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20][..],
    ] {
        let mut state = mov_byte_state();
        state.gpr[Register::Rbx as usize] = if bytes.len() == 3 { DATA } else { 3 };
        state.rflags |= ARITH;
        let before = state.clone();
        let fault_at = if bytes.len() == 3 { DATA } else { DATA + 0x102 };
        let mut memory = DataMemory::new(bytes).with_supervisor(fault_at, 0xcc);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        // P (the page is present) | U (a CPL3 access), no W: a read.
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x5),
                linear_address: Some(fault_at),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
        assert_eq!(memory.supervisor[&fault_at], 0xcc);
    }
}

#[test]
fn cmpb_imm8_from_a_page_without_p_is_a_not_present_user_page_fault() {
    let mut state = mov_byte_state();
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x80, 0x3b, 0x20]).with_absent(DATA);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    // U only: not present, a read by CPL3.
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x4),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn cmpb_imm8_at_a_non_canonical_address_is_a_general_protection_fault() {
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x80, 0x3b, 0x20]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13 && fault.error_code == Some(0)),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cmpb_imm8_at_a_non_canonical_stack_base_is_a_stack_fault() {
    // `80 3c 24 20`: `cmpb $0x20, (%rsp)` and `80 7d 00 20`: `cmpb $0x20, 0(%rbp)`; RSP and RBP as
    // the base go through SS, so the fault is #SS(0).
    for (bytes, register) in [
        (&[0x80, 0x3c, 0x24, 0x20][..], Register::Rsp),
        (&[0x80, 0x7d, 0x00, 0x20][..], Register::Rbp),
    ] {
        let mut state = mov_byte_state();
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 12 && fault.error_code == Some(0)),
            "{:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn cmpb_imm8_refusals_stop_as_unsupported_80_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("67 41 80 f8 20 addr32 cmp", &[0x67, 0x41, 0x80, 0xf8, 0x20]),
        ("80 03 05 addb $5, (%rbx) (/0)", &[0x80, 0x03, 0x05]),
        (
            "67 80 0b 05 addr32 orb $5, (%ebx)",
            &[0x67, 0x80, 0x0b, 0x05],
        ),
        ("80 13 05 adcb $5, (%rbx) (/2)", &[0x80, 0x13, 0x05]),
        ("80 1b 05 sbbb $5, (%rbx) (/3)", &[0x80, 0x1b, 0x05]),
        ("80 2b 05 subb $5, (%rbx) (/5)", &[0x80, 0x2b, 0x05]),
        ("80 33 05 xorb $5, (%rbx) (/6)", &[0x80, 0x33, 0x05]),
        ("80 c0 05 addb $5, %al (/0 mod = 11)", &[0x80, 0xc0, 0x05]),
        ("66 80 3b 20", &[0x66, 0x80, 0x3b, 0x20]),
        ("67 80 3b 20", &[0x67, 0x80, 0x3b, 0x20]),
        ("f0 80 3b 20 LOCK", &[0xf0, 0x80, 0x3b, 0x20]),
        ("f2 80 3b 20 REPNE", &[0xf2, 0x80, 0x3b, 0x20]),
        ("f3 80 3b 20 REP", &[0xf3, 0x80, 0x3b, 0x20]),
        ("64 80 3b 20 FS", &[0x64, 0x80, 0x3b, 0x20]),
        ("65 80 3b 20 GS", &[0x65, 0x80, 0x3b, 0x20]),
        ("26 80 3b 20 ES", &[0x26, 0x80, 0x3b, 0x20]),
        ("2e 80 3b 20 CS", &[0x2e, 0x80, 0x3b, 0x20]),
        ("36 80 3b 20 SS", &[0x36, 0x80, 0x3b, 0x20]),
        ("3e 80 3b 20 DS", &[0x3e, 0x80, 0x3b, 0x20]),
        (
            "66 41 80 7c 1e ff 20",
            &[0x66, 0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20],
        ),
        (
            "67 41 80 7c 1e ff 20",
            &[0x67, 0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20],
        ),
        (
            "2e 41 80 7c 1e ff 20",
            &[0x2e, 0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20],
        ),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes).with_user(DATA, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x80 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn init_cmpb_imm8_then_jne_then_leaq_then_decl_executes() {
    // Synthetic chain modelled on `/init` from 0x400153: `cmpb $0x20, -1(%r14,%rbx); jne +0x24;
    // leaq 0x107(%rip), %rsi; decl %ecx` (`ff c9`, FF /1, executes since S4-39f15). The
    // intermediate instructions of the real path are omitted here. The byte equals 0x20, so `jne`
    // is not taken.
    let bytes = [
        0x41, 0x80, 0x7c, 0x1e, 0xff, 0x20, 0x75, 0x24, 0x48, 0x8d, 0x35, 0x07, 0x01, 0x00, 0x00,
        0xff, 0xc9,
    ];
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 3;
    let mut expected = state.clone();
    expected.rip = RIP + 17;
    // cmpb leaves ZF|PF; decl then replaces them: 0x2222_2222 - 1 = 0x2222_2221 (two low bits: PF).
    expected.rflags = (expected.rflags & !ARITH) | PF;
    expected.gpr[Register::Rsi as usize] = RIP + 15 + 0x107;
    expected.gpr[Register::Rcx as usize] = 0x2222_2221;
    let mut memory = DataMemory::new(&bytes).with_user(DATA + 0x102, 0x20);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());
}

// ---- S4-39f15: `FF /1` (DEC r/m32), register form ----

/// The arithmetic flags of `value - 1` (dword) per the SDM (DEC): OF, SF, ZF, AF, PF; CF is not
/// among them.
fn dec32_flags(value: u32) -> u64 {
    let result = value.wrapping_sub(1);
    let mut flags = 0;
    if value == 0x8000_0000 {
        flags |= OF;
    }
    if result >> 31 != 0 {
        flags |= SF;
    }
    if result == 0 {
        flags |= ZF;
    }
    if value & 0xf == 0 {
        flags |= AF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= PF;
    }
    flags
}

#[test]
fn dec_r32_register_form_sets_the_sub_one_flags_keeps_cf_and_zero_extends() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, register index, dword value, expected dword result, expected OF/SF/ZF/AF/PF).
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u32, u32, u64)] = &[
        // `/init` at 0x40017b: decl %ecx.
        ("ff c9 0", &[0xff, 0xc9], 1, 0, 0xffff_ffff, SF | AF | PF),
        ("ff c9 1", &[0xff, 0xc9], 1, 1, 0, ZF | PF),
        ("ff c9 2", &[0xff, 0xc9], 1, 2, 1, 0),
        ("ff c9 3", &[0xff, 0xc9], 1, 3, 2, 0),
        // Result 0x0f (four set bits): PF; the low nibble borrows: AF.
        ("ff c9 0x10", &[0xff, 0xc9], 1, 0x10, 0x0f, AF | PF),
        // Result 0xff: AF and PF.
        ("ff c9 0x100", &[0xff, 0xc9], 1, 0x100, 0xff, AF | PF),
        // Signed overflow: i32::MIN - 1 = i32::MAX: OF, AF, PF (low byte 0xff).
        (
            "ff c9 min",
            &[0xff, 0xc9],
            1,
            0x8000_0000,
            0x7fff_ffff,
            OF | AF | PF,
        ),
        // i32::MAX - 1 = 0x7fff_fffe: seven set bits in the low byte, no flag.
        ("ff c9 max", &[0xff, 0xc9], 1, 0x7fff_ffff, 0x7fff_fffe, 0),
        // 0x8000_0001 - 1 = 0x8000_0000: SF; low byte 0 (PF); no AF (low nibble 1).
        (
            "ff c9 0x80000001",
            &[0xff, 0xc9],
            1,
            0x8000_0001,
            0x8000_0000,
            SF | PF,
        ),
        // -1 - 1 = -2: SF; low byte 0xfe: seven bits, no PF.
        ("ff c9 -1", &[0xff, 0xc9], 1, 0xffff_ffff, 0xffff_fffe, SF),
        // The other legacy registers.
        ("ff c8 decl %eax", &[0xff, 0xc8], 0, 5, 4, 0),
        ("ff ca decl %edx", &[0xff, 0xca], 2, 5, 4, 0),
        ("ff cb decl %ebx", &[0xff, 0xcb], 3, 5, 4, 0),
        ("ff cc decl %esp", &[0xff, 0xcc], 4, 5, 4, 0),
        ("ff cd decl %ebp", &[0xff, 0xcd], 5, 5, 4, 0),
        ("ff ce decl %esi", &[0xff, 0xce], 6, 5, 4, 0),
        ("ff cf decl %edi", &[0xff, 0xcf], 7, 5, 4, 0),
        // Bare REX (no W), REX.R and REX.X are ignored for a register rm.
        ("40 ff c9 REX", &[0x40, 0xff, 0xc9], 1, 5, 4, 0),
        ("44 ff c9 REX.R", &[0x44, 0xff, 0xc9], 1, 5, 4, 0),
        ("42 ff c9 REX.X", &[0x42, 0xff, 0xc9], 1, 5, 4, 0),
        ("46 ff c9 REX.RX", &[0x46, 0xff, 0xc9], 1, 5, 4, 0),
        // REX.B: r8d..r15d.
        ("41 ff c8 decl %r8d", &[0x41, 0xff, 0xc8], 8, 5, 4, 0),
        ("41 ff c9 decl %r9d", &[0x41, 0xff, 0xc9], 9, 5, 4, 0),
        ("41 ff ca decl %r10d", &[0x41, 0xff, 0xca], 10, 5, 4, 0),
        ("41 ff cb decl %r11d", &[0x41, 0xff, 0xcb], 11, 5, 4, 0),
        ("41 ff cc decl %r12d", &[0x41, 0xff, 0xcc], 12, 5, 4, 0),
        ("41 ff cd decl %r13d", &[0x41, 0xff, 0xcd], 13, 5, 4, 0),
        ("41 ff ce decl %r14d", &[0x41, 0xff, 0xce], 14, 5, 4, 0),
        (
            "41 ff cf decl %r15d min",
            &[0x41, 0xff, 0xcf],
            15,
            0x8000_0000,
            0x7fff_ffff,
            OF | AF | PF,
        ),
        ("45 ff c8 REX.RB", &[0x45, 0xff, 0xc8], 8, 5, 4, 0),
        ("47 ff cf REX.RXB", &[0x47, 0xff, 0xcf], 15, 1, 0, ZF | PF),
    ];
    for (name, bytes, index, value, result_value, flags) in cases {
        assert_eq!(dec32_flags(*value), *flags, "{name} table");
        assert_eq!(value.wrapping_sub(1), *result_value, "{name} table");
        // The upper dword of the register never affects the result and is cleared by the write.
        for high in [0_u64, 0xdead_beef] {
            // Four starting flag sets: DEC replaces the five flags and never touches CF.
            for before_flags in [0, CF, ARITH & !CF, ARITH] {
                let mut state = user_state();
                for (i, register) in state.gpr.iter_mut().enumerate() {
                    *register = 0x1111_1111_1111_1111_u64.wrapping_mul(i as u64 + 1);
                }
                state.gpr[*index] = (high << 32) | u64::from(*value);
                state.rflags |= before_flags;
                let mut expected = state.clone();
                expected.rip = RIP + bytes.len() as u64;
                expected.gpr[*index] = u64::from(*result_value);
                expected.rflags = (expected.rflags & !ARITH) | (before_flags & CF) | flags;
                let mut memory = Memory::code(bytes);
                let result = run3(&mut state, &mut memory, &set, 1).unwrap();
                assert_eq!(
                    exit(&result),
                    &RunExit::InstructionBudgetExhausted,
                    "{name} {high:#x} {before_flags:#x}"
                );
                assert_eq!(
                    result.instructions_executed, 1,
                    "{name} {high:#x} {before_flags:#x}"
                );
                // Only the destination, RIP and the flags change.
                assert_eq!(state, expected, "{name} {high:#x} {before_flags:#x}");
                assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
                assert_eq!(memory.accesses, 0, "{name}");
            }
        }
    }
}

#[test]
fn dec_r32_to_zero_sets_zf_and_clears_the_upper_dword_in_both_cf_states() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for carry in [0, CF] {
        let mut state = user_state();
        state.gpr[Register::Rcx as usize] = 0xffff_ffff_0000_0001;
        state.rflags |= carry;
        let mut memory = Memory::code(&[0xff, 0xc9]);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(state.gpr[Register::Rcx as usize], 0);
        assert_eq!(state.rip, RIP + 2);
        assert_eq!(state.rflags & ARITH, carry | ZF | PF);
    }
}

#[test]
fn dec_r32_keeps_the_carry_for_a_following_jb() {
    // `4c 39 eb` (cmpq %r13, %rbx: 0 - 5 sets CF), `ff c9` (decl %ecx keeps CF), `72 01` (jb +1,
    // taken on CF), a skipped `90`, `90`.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = 0;
    state.gpr[Register::R13 as usize] = 5;
    state.gpr[Register::Rcx as usize] = 9;
    let mut memory = Memory::code(&[0x4c, 0x39, 0xeb, 0xff, 0xc9, 0x72, 0x01, 0x90, 0x90]);
    let result = run3(&mut state, &mut memory, &set, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[Register::Rcx as usize], 8);
    // jb taken: 3 + 2 + 2 + 1.
    assert_eq!(state.rip, RIP + 8);
    assert_eq!(state.rflags & CF, CF);
}

#[test]
fn init_decl_then_jmp_then_testl_chain_follows_the_ecx_value() {
    // `/init` at 0x40017b: `ff c9` (decl %ecx), `eb 00` (jmp rel8, here to the next byte), `85 c9`
    // (testl %ecx, %ecx at 0x40016b in the real image). A nonzero result leaves ZF clear and a
    // zero result sets it; the stop after three instructions is the budget, not a refusal.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for (start, zf) in [(1_u64, ZF), (2, 0)] {
        let mut state = user_state();
        state.gpr[Register::Rcx as usize] = 0xabcd_0000_0000 | start;
        let mut memory = Memory::code(&[0xff, 0xc9, 0xeb, 0x00, 0x85, 0xc9]);
        let result = run3(&mut state, &mut memory, &set, 3).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 3);
        assert_eq!(state.rip, RIP + 6);
        assert_eq!(state.gpr[Register::Rcx as usize], start - 1);
        assert_eq!(state.rflags & ZF, zf);
    }
}

#[test]
fn ff_forms_outside_the_register_dec_r32_are_refused_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        // DEC memory widths outside the dword RMW forms.
        ("66 ff 09 decw (%rcx)", &[0x66, 0xff, 0x09], 0xff),
        ("48 ff 09 decq (%rcx)", &[0x48, 0xff, 0x09], 0xff),
        // REX.W: DEC r/m64.
        ("48 ff c9 decq %rcx", &[0x48, 0xff, 0xc9], 0xff),
        ("49 ff c9 decq %r9", &[0x49, 0xff, 0xc9], 0xff),
        ("4c ff c9 REX.WR", &[0x4c, 0xff, 0xc9], 0xff),
        // FF /0 without REX.W: INC r/m32 is not part of the slice.
        ("ff c1 incl %ecx", &[0xff, 0xc1], 0xff),
        ("41 ff c3 incl %r11d", &[0x41, 0xff, 0xc3], 0xff),
        // FF /2../7 with ModRM reg-direct rm = ecx.
        ("ff d1 call *%rcx", &[0xff, 0xd1], 0xff),
        ("ff d9 lcall *%rcx", &[0xff, 0xd9], 0xff),
        ("ff e1 jmp *%rcx", &[0xff, 0xe1], 0xff),
        ("ff e9 ljmp *%rcx", &[0xff, 0xe9], 0xff),
        ("ff f1 push %rcx", &[0xff, 0xf1], 0xff),
        ("ff f9 /7", &[0xff, 0xf9], 0xff),
        // DEC r/m8 is the opcode FE.
        ("fe c9 decb %cl", &[0xfe, 0xc9], 0xfe),
        // Prefixes in front of the allowed form.
        ("66 ff c9", &[0x66, 0xff, 0xc9], 0xff),
        ("67 ff c9", &[0x67, 0xff, 0xc9], 0xff),
        ("f0 ff c9", &[0xf0, 0xff, 0xc9], 0xff),
        ("f2 ff c9", &[0xf2, 0xff, 0xc9], 0xff),
        ("f3 ff c9", &[0xf3, 0xff, 0xc9], 0xff),
        ("64 ff c9", &[0x64, 0xff, 0xc9], 0xff),
        ("65 ff c9", &[0x65, 0xff, 0xc9], 0xff),
        ("2e ff c9", &[0x2e, 0xff, 0xc9], 0xff),
        ("3e ff c9", &[0x3e, 0xff, 0xc9], 0xff),
        ("26 ff c9", &[0x26, 0xff, 0xc9], 0xff),
        ("36 ff c9", &[0x36, 0xff, 0xc9], 0xff),
        ("66 41 ff c9", &[0x66, 0x41, 0xff, 0xc9], 0xff),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        state.gpr[Register::Rbx as usize] = DATA;
        state.gpr[Register::Rcx as usize] = DATA;
        state.gpr[Register::R9 as usize] = DATA;
        state.gpr[Register::Rsp as usize] = DATA;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

// ---- S4-39f16: `[REX] 0F B6 /r` (MOVZX r32, r/m8) with a memory source ----

/// `state` after a MOVZX that retired: `register` holds `value` (upper 32 bits cleared), RIP moved
/// by `len`, nothing else changed.
fn after_movzx(state: &LongModeState, register: Register, value: u8, len: usize) -> LongModeState {
    let mut expected = state.clone();
    expected.gpr[register as usize] = u64::from(value);
    expected.rip = RIP + len as u64;
    expected
}

#[test]
fn movzbl_init_form_reads_one_byte_and_zero_extends_into_eax() {
    // `41 0f b6 04 08`: `movzbl (%r8,%rcx), %eax` of `/init` at `0x400189` (REX.B, SIB 08).
    for value in [0x00_u8, 0x20, 0x7f, 0x80, 0xff] {
        for flags in [0, ARITH] {
            let mut state = mov_byte_state();
            state.gpr[Register::Rax as usize] = 0xfedc_ba98_7654_3210;
            state.gpr[Register::R8 as usize] = DATA;
            state.gpr[Register::Rcx as usize] = 5;
            state.rflags |= flags;
            let expected = after_movzx(&state, Register::Rax, value, 5);
            let mut memory = DataMemory::new(&[0x41, 0x0f, 0xb6, 0x04, 0x08])
                .with_user(DATA + 5, value)
                .with_user(DATA + 4, 0x11)
                .with_user(DATA + 6, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(state, expected, "value {value:#x}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
            assert_eq!(memory.reads, 1);
            assert!(memory.writes.is_empty());
        }
    }
}

#[test]
fn movzbl_forms_use_the_decoded_destination_and_address() {
    // (name, bytes, effective address, destination)
    let forms: &[(&str, &[u8], u64, Register)] = &[
        (
            "0f b6 03 (%rbx) eax",
            &[0x0f, 0xb6, 0x03],
            DATA,
            Register::Rax,
        ),
        (
            "0f b6 0b (%rbx) ecx",
            &[0x0f, 0xb6, 0x0b],
            DATA,
            Register::Rcx,
        ),
        // No byte-register aliasing: reg 4..7 are esp, ebp, esi, edi even without REX.
        (
            "0f b6 23 (%rbx) esp",
            &[0x0f, 0xb6, 0x23],
            DATA,
            Register::Rsp,
        ),
        (
            "0f b6 3b (%rbx) edi",
            &[0x0f, 0xb6, 0x3b],
            DATA,
            Register::Rdi,
        ),
        // REX.R: r8d..r15d; REX.B: base r8..r15; REX.X: index.
        (
            "44 0f b6 03 (%rbx) r8d",
            &[0x44, 0x0f, 0xb6, 0x03],
            DATA,
            Register::R8,
        ),
        (
            "44 0f b6 3b (%rbx) r15d",
            &[0x44, 0x0f, 0xb6, 0x3b],
            DATA,
            Register::R15,
        ),
        (
            "41 0f b6 06 (%r14) eax",
            &[0x41, 0x0f, 0xb6, 0x06],
            DATA + 0x100,
            Register::Rax,
        ),
        (
            "42 0f b6 04 2b (%rbx,%r13) eax",
            &[0x42, 0x0f, 0xb6, 0x04, 0x2b],
            DATA + 3,
            Register::Rax,
        ),
        (
            "43 0f b6 04 2e (%r14,%r13) eax",
            &[0x43, 0x0f, 0xb6, 0x04, 0x2e],
            DATA + 0x103,
            Register::Rax,
        ),
        (
            "47 0f b6 0c 2e (%r14,%r13) r9d",
            &[0x47, 0x0f, 0xb6, 0x0c, 0x2e],
            DATA + 0x103,
            Register::R9,
        ),
        (
            "41 0f b6 04 5e (%r14,%rbx,2) eax",
            &[0x41, 0x0f, 0xb6, 0x04, 0x5e],
            DATA + 0x100 + 2 * DATA,
            Register::Rax,
        ),
        (
            "0f b6 43 05 5(%rbx)",
            &[0x0f, 0xb6, 0x43, 0x05],
            DATA + 5,
            Register::Rax,
        ),
        (
            "0f b6 43 fe -2(%rbx)",
            &[0x0f, 0xb6, 0x43, 0xfe],
            DATA - 2,
            Register::Rax,
        ),
        (
            "0f b6 83 00 01 00 00 0x100(%rbx)",
            &[0x0f, 0xb6, 0x83, 0x00, 0x01, 0x00, 0x00],
            DATA + 0x100,
            Register::Rax,
        ),
        (
            "0f b6 04 dd 00 10 60 00 0x601000(,%rbx,8)",
            &[0x0f, 0xb6, 0x04, 0xdd, 0x00, 0x10, 0x60, 0x00],
            0x60_1000 + 8 * DATA,
            Register::Rax,
        ),
        // RIP-relative: the base is the end of the instruction; REX.B does not make it r13.
        (
            "0f b6 05 10 00 00 00 0x10(%rip)",
            &[0x0f, 0xb6, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 7 + 0x10,
            Register::Rax,
        ),
        (
            "41 0f b6 05 10 00 00 00 REX.B on rip-relative",
            &[0x41, 0x0f, 0xb6, 0x05, 0x10, 0x00, 0x00, 0x00],
            RIP + 8 + 0x10,
            Register::Rax,
        ),
    ];
    for (name, bytes, target, register) in forms {
        for flags in [0, ARITH] {
            let mut state = mov_byte_state();
            state.rflags |= flags;
            let expected = after_movzx(&state, *register, 0x9c, bytes.len());
            let mut memory = DataMemory::new(bytes)
                .with_user(*target, 0x9c)
                .with_user(target - 1, 0x11)
                .with_user(target + 1, 0x22);
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(memory.reads, 1, "{name}");
            assert!(memory.writes.is_empty(), "{name}");
        }
    }
}

#[test]
fn movzbl_sib_with_rex_x_or_without_a_base_uses_the_decoded_registers() {
    // `42 0f b6 04 23`: REX.X makes the index r12 (base rbx): the byte at rbx + r12.
    let mut state = mov_byte_state();
    state.gpr[Register::R12 as usize] = 0x40;
    let target = DATA + 0x40;
    let expected = after_movzx(&state, Register::Rax, 0x9c, 5);
    let mut memory = DataMemory::new(&[0x42, 0x0f, 0xb6, 0x04, 0x23])
        .with_user(target, 0x9c)
        .with_user(target - 1, 0x11)
        .with_user(target + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);

    // `41 0f b6 04 25 disp32`: SIB base 101 with mod 00 has no base even with REX.B (not r13).
    let mut state = mov_byte_state();
    let expected = after_movzx(&state, Register::Rax, 0x9c, 9);
    let mut memory = DataMemory::new(&[0x41, 0x0f, 0xb6, 0x04, 0x25, 0x00, 0x10, 0x60, 0x00])
        .with_user(DATA, 0x9c)
        .with_user(DATA - 1, 0x11)
        .with_user(DATA + 1, 0x22);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
}

#[test]
fn movzbl_from_a_supervisor_page_is_a_user_read_page_fault_without_a_register_write() {
    for bytes in [&[0x0f, 0xb6, 0x03][..], &[0x41, 0x0f, 0xb6, 0x04, 0x08][..]] {
        let mut state = mov_byte_state();
        state.gpr[Register::R8 as usize] = DATA;
        state.gpr[Register::Rcx as usize] = 5;
        let before = state.clone();
        let fault_at = if bytes.len() == 3 { DATA } else { DATA + 5 };
        let mut memory = DataMemory::new(bytes).with_supervisor(fault_at, 0xcc);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x5),
                linear_address: Some(fault_at),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn movzbl_from_a_page_without_p_is_a_not_present_user_page_fault() {
    let mut state = mov_byte_state();
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x0f, 0xb6, 0x03]).with_absent(DATA);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x4),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn movzbl_from_a_non_canonical_address_is_a_general_protection_fault() {
    let mut state = mov_byte_state();
    state.gpr[Register::Rbx as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x0f, 0xb6, 0x03]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 13 && fault.error_code == Some(0)),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn movzbl_from_a_non_canonical_stack_base_is_a_stack_fault() {
    // `0f b6 04 24` (base rsp) and `0f b6 45 00` (base rbp) go through SS: #SS(0).
    for (bytes, register) in [
        (&[0x0f, 0xb6, 0x04, 0x24][..], Register::Rsp),
        (&[0x0f, 0xb6, 0x45, 0x00][..], Register::Rbp),
    ] {
        let mut state = mov_byte_state();
        state.gpr[register as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Fault(fault) if fault.vector == 12 && fault.error_code == Some(0)),
            "{:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn movzbl_refusals_stop_as_unsupported_b6_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("48 0f b6 c1 movzbq %cl, %rax", &[0x48, 0x0f, 0xb6, 0xc1]),
        ("67 0f b6 d9 movzbl %cl, %ebx", &[0x67, 0x0f, 0xb6, 0xd9]),
        ("48 0f b6 03 REX.W movzbq", &[0x48, 0x0f, 0xb6, 0x03]),
        (
            "49 0f b6 04 08 REX.WB movzbq",
            &[0x49, 0x0f, 0xb6, 0x04, 0x08],
        ),
        ("66 0f b6 03", &[0x66, 0x0f, 0xb6, 0x03]),
        ("67 0f b6 03", &[0x67, 0x0f, 0xb6, 0x03]),
        ("f0 0f b6 03 LOCK", &[0xf0, 0x0f, 0xb6, 0x03]),
        ("f2 0f b6 03 REPNE", &[0xf2, 0x0f, 0xb6, 0x03]),
        ("f3 0f b6 03 REP", &[0xf3, 0x0f, 0xb6, 0x03]),
        ("64 0f b6 03 FS", &[0x64, 0x0f, 0xb6, 0x03]),
        ("65 0f b6 03 GS", &[0x65, 0x0f, 0xb6, 0x03]),
        ("26 0f b6 03 ES", &[0x26, 0x0f, 0xb6, 0x03]),
        ("2e 0f b6 03 CS", &[0x2e, 0x0f, 0xb6, 0x03]),
        ("36 0f b6 03 SS", &[0x36, 0x0f, 0xb6, 0x03]),
        ("3e 0f b6 03 DS", &[0x3e, 0x0f, 0xb6, 0x03]),
        ("66 41 0f b6 04 08", &[0x66, 0x41, 0x0f, 0xb6, 0x04, 0x08]),
        ("67 41 0f b6 04 08", &[0x67, 0x41, 0x0f, 0xb6, 0x04, 0x08]),
        ("2e 41 0f b6 04 08", &[0x2e, 0x41, 0x0f, 0xb6, 0x04, 0x08]),
    ];
    for (name, bytes) in forms {
        let mut state = mov_byte_state();
        state.gpr[Register::R8 as usize] = DATA;
        state.gpr[Register::Rcx as usize] = 5;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes)
            .with_user(DATA, 0xee)
            .with_user(DATA + 5, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xb6 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
        assert_eq!(memory.reads, 0, "{name}");
    }
}

#[test]
fn neighbouring_movzx_and_movsx_opcodes_stay_outside() {
    let forms: &[(&str, &[u8], u8)] = &[
        ("0f b7 c3 movzwl bx, eax", &[0x0f, 0xb7, 0xc3], 0xb7),
        ("48 0f b7 03 movzwq", &[0x48, 0x0f, 0xb7, 0x03], 0xb7),
        (
            "66 0f be 03 movsbw (%rbx), %ax",
            &[0x66, 0x0f, 0xbe, 0x03],
            0xbe,
        ),
        (
            "67 41 0f be 04 08 movsbl",
            &[0x67, 0x41, 0x0f, 0xbe, 0x04, 0x08],
            0xbe,
        ),
        ("48 0f be 03 movsbq", &[0x48, 0x0f, 0xbe, 0x03], 0xbe),
        ("0f bf 03 movswl (%rbx), %eax", &[0x0f, 0xbf, 0x03], 0xbf),
        (
            "67 48 63 03 movslq (%ebx), %rax",
            &[0x67, 0x48, 0x63, 0x03],
            0x63,
        ),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = mov_byte_state();
        state.gpr[Register::R8 as usize] = DATA;
        state.gpr[Register::Rcx as usize] = 5;
        let before = state.clone();
        let mut memory = DataMemory::new(bytes)
            .with_user(DATA, 0xee)
            .with_user(DATA + 5, 0xee);
        let result = run3_data(&mut state, &mut memory, 4).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.reads, 0, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

#[test]
fn init_movzbl_testb_cmpb_cmpq_rep_movsb_subq_chain_stops_at_hlt() {
    // `41 0f b6 04 08` (movzbl (%r8,%rcx), %eax; executes since S4-39f16), `84 c0`
    // (testb %al, %al; S4-39f17), `3c 20` (cmpb $0x20, %al; S4-39f18), `48 81 f9 80 00 00 00`
    // (cmpq $0x80, %rcx; S4-39f19), `f3 a4` (rep movsb; executes since S4-39f20) and then
    // `48 29 f2` (subq %rsi, %rdx; executes since S4-39f21) and then `f4` (outside, a sentinel
    // instead of the `movl` of `/init`): the jumps between them are not exercised here.
    let mut state = mov_byte_state();
    state.gpr[Register::R8 as usize] = DATA;
    state.gpr[Register::Rcx as usize] = 5;
    state.gpr[Register::Rsi as usize] = DATA + 0x10;
    state.gpr[Register::Rdi as usize] = DATA + 0x200;
    state.gpr[Register::Rdx as usize] = DATA + 0x215;
    state.rflags |= ARITH;
    let mut expected = after_movzx(&state, Register::Rax, b' ', 5 + 2 + 2);
    // al = 0x20 - 0x20: ZF | PF; CF = OF = SF = AF = 0; other flags stay.
    expected.rflags = (expected.rflags & !ARITH) | ZF | PF;
    // rcx = 5: 5 - 0x80 = 0xffff_ffff_ffff_ff85: CF | SF (three set bits: PF clear), no AF.
    expected.rip = RIP + 16;
    expected.rflags = (expected.rflags & !ARITH) | CF | SF;
    // `rep movsb` copies five bytes and leaves the flags of the `cmpq`.
    expected.rip = RIP + 18;
    expected.gpr[Register::Rcx as usize] = 0;
    expected.gpr[Register::Rsi as usize] = DATA + 0x15;
    expected.gpr[Register::Rdi as usize] = DATA + 0x205;
    // `subq %rsi, %rdx`: rdx = (DATA + 0x215) - (DATA + 0x15) = 0x200: PF (low byte 0) only.
    expected.rip = RIP + 21;
    expected.gpr[Register::Rdx as usize] = 0x200;
    expected.rflags = (expected.rflags & !ARITH) | PF;
    let mut memory = DataMemory::new(&[
        0x41, 0x0f, 0xb6, 0x04, 0x08, 0x84, 0xc0, 0x3c, 0x20, 0x48, 0x81, 0xf9, 0x80, 0x00, 0x00,
        0x00, 0xf3, 0xa4, 0x48, 0x29, 0xf2, 0xf4,
    ])
    .with_user(DATA + 5, b' ');
    for index in 0..5 {
        memory = memory
            .with_user(DATA + 0x10 + index, 0x41 + index as u8)
            .with_user(DATA + 0x200 + index, 0);
    }
    let result = run3_data(&mut state, &mut memory, 20).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    // Four instructions, then one unit per iteration of the string, then the `subq`.
    assert_eq!(result.instructions_executed, 4 + 5 + 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1 + 5);
    for index in 0..5 {
        assert_eq!(memory.user[&(DATA + 0x200 + index)], 0x41 + index as u8);
    }
}

// ---- S4-39f17: `[REX] 84 /r` (TEST r/m8, r8), register form ----

#[test]
fn test_r8_register_form_sets_flags_and_writes_no_result() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, registers to preset (index, value), flags)
    // The flags are those of the byte `rm & reg`; CF = OF = 0. The other bytes of the
    // registers are garbage and must neither matter nor change.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], &[(usize, u64)], u64)] = &[
        // `/init` at 0x40018e: testb %al, %al.
        (
            "84 c0 al=00",
            &[0x84, 0xc0],
            &[(0, 0xaabb_ccdd_eeff_ff00)],
            ZF | PF,
        ),
        (
            "84 c0 al=01",
            &[0x84, 0xc0],
            &[(0, 0xaabb_ccdd_eeff_ff01)],
            0,
        ),
        (
            "84 c0 al=03",
            &[0x84, 0xc0],
            &[(0, 0x1234_5678_9abc_de03)],
            PF,
        ),
        (
            "84 c0 al=7f",
            &[0x84, 0xc0],
            &[(0, 0xffff_ffff_ffff_ff7f)],
            0,
        ),
        (
            "84 c0 al=80",
            &[0x84, 0xc0],
            &[(0, 0xaabb_ccdd_eeff_ff80)],
            SF,
        ),
        (
            "84 c0 al=ff",
            &[0x84, 0xc0],
            &[(0, 0x0123_4567_89ab_cdff)],
            SF | PF,
        ),
        // testb %bl, %al (reg = bl, rm = al): disjoint bits.
        (
            "84 d8 al/bl disjoint",
            &[0x84, 0xd8],
            &[(0, 0x1111_2222_3333_44f0), (3, 0x5555_6666_7777_880f)],
            ZF | PF,
        ),
        // testb %cl, %al: 0xaa & 0x55 = 0.
        (
            "84 c8 al/cl disjoint",
            &[0x84, 0xc8],
            &[(0, 0x55), (1, 0xaa)],
            ZF | PF,
        ),
        // REX.R: reg = r8b, rm = al: 0xc3 & 0x83 = 0x83 (SF, three bits).
        (
            "44 84 c0 REX.R",
            &[0x44, 0x84, 0xc0],
            &[(0, 0xc3), (8, 0x83)],
            SF,
        ),
        // REX.B: rm = r9b, reg = al: 0x0f & 0x0f (four bits).
        (
            "41 84 c1 REX.B",
            &[0x41, 0x84, 0xc1],
            &[(0, 0x0f), (9, 0x0f)],
            PF,
        ),
        // REX.RB: reg = r9b, rm = r10b: 0x03 & 0x06 = 0x02 (one bit).
        (
            "45 84 ca REX.RB",
            &[0x45, 0x84, 0xca],
            &[(9, 0x03), (10, 0x06)],
            0,
        ),
        // REX.WRB: the same byte test on r15b (REX.W does not widen a byte test).
        ("4d 84 ff REX.WRB", &[0x4d, 0x84, 0xff], &[(15, 0x40)], 0),
        // REX.W: the byte test ignores bits 8..63 (a 64-bit test of this value would set SF).
        (
            "48 84 c0 REX.W",
            &[0x48, 0x84, 0xc0],
            &[(0, 0xffff_ffff_ffff_ff00)],
            ZF | PF,
        ),
        // With REX the byte registers are SPL, BPL, SIL, DIL.
        ("40 84 e4 spl", &[0x40, 0x84, 0xe4], &[(4, 0x80)], SF),
        ("40 84 ed bpl", &[0x40, 0x84, 0xed], &[(5, 0)], ZF | PF),
        (
            "40 84 f7 sil/dil",
            &[0x40, 0x84, 0xf7],
            &[(6, 0xf0), (7, 0x0f)],
            ZF | PF,
        ),
        ("40 84 ff dil", &[0x40, 0x84, 0xff], &[(7, 0x01)], 0),
        // Without REX the encodings 4..7 are AH, CH, DH, BH (bits 8..15 of RAX, RCX, RDX, RBX).
        ("84 e4 ah", &[0x84, 0xe4], &[(0, 0x1234_5678_9abc_8001)], SF),
        (
            "84 ed ch",
            &[0x84, 0xed],
            &[(1, 0xffff_ffff_ffff_00ff)],
            ZF | PF,
        ),
        // BH = 0x01 here; DIL (the wrong register) would be 0x88 and give SF | PF.
        ("84 ff bh", &[0x84, 0xff], &[(3, 0x1111_2222_3333_0100)], 0),
    ];
    for (name, bytes, presets, flags) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        for (index, value) in *presets {
            state.gpr[*index] = *value;
        }
        // CF, OF, SF, ZF, PF, AF all set before: TEST clears CF/OF and recomputes SF/ZF/PF.
        state.rflags |= ARITH;
        let mut expected = state.clone();
        expected.rip = RIP + bytes.len() as u64;
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        // No register changes (the result is not written), RIP advances by the length.
        assert_eq!(state.gpr, expected.gpr, "{name}");
        assert_eq!(state.rip, expected.rip, "{name}");
        let arithmetic = state.rflags & ARITH;
        assert_eq!(arithmetic & (CF | OF), 0, "{name}");
        assert_eq!(arithmetic & (SF | ZF | PF), *flags, "{name}");
        assert_eq!(state.rflags & !ARITH, expected.rflags & !ARITH, "{name}");
        // AF is undefined after TEST (SDM, from memory): it leaves the defined set.
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS & !AF, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn test_r8_with_a_prefix_or_a_neighbouring_opcode_stays_outside() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        ("66 84 c0 operand-size", &[0x66, 0x84, 0xc0], 0x84),
        ("67 84 c0 address-size", &[0x67, 0x84, 0xc0], 0x84),
        ("f0 84 c0 lock", &[0xf0, 0x84, 0xc0], 0x84),
        ("f2 84 c0 repne", &[0xf2, 0x84, 0xc0], 0x84),
        ("f3 84 c0 rep", &[0xf3, 0x84, 0xc0], 0x84),
        ("64 84 c0 fs", &[0x64, 0x84, 0xc0], 0x84),
        ("65 84 c0 gs", &[0x65, 0x84, 0xc0], 0x84),
        ("2e 84 c0 cs", &[0x2e, 0x84, 0xc0], 0x84),
        ("3e 84 c0 ds", &[0x3e, 0x84, 0xc0], 0x84),
        ("26 84 c0 es", &[0x26, 0x84, 0xc0], 0x84),
        ("36 84 c0 ss", &[0x36, 0x84, 0xc0], 0x84),
        (
            "66 40 84 c0 prefix before REX",
            &[0x66, 0x40, 0x84, 0xc0],
            0x84,
        ),
        ("f0 a8 00 LOCK test al, imm8", &[0xf0, 0xa8, 0x00], 0xa8),
        (
            "f0 f6 c0 01 LOCK test al, imm8",
            &[0xf0, 0xf6, 0xc0, 0x01],
            0xf6,
        ),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn test_r8_then_je_then_cmpb_imm8_executes() {
    // `84 c0` (testb %al, %al), `74 02` (je, taken on al == 0) skipping two bytes, then
    // `3c 20` (cmpb $0x20, %al; executes since S4-39f18): the sequence of `/init` after `je`.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x1234_5678_9abc_de00;
    let mut memory = Memory::code(&[0x84, 0xc0, 0x74, 0x02, 0xf4, 0xf4, 0x3c, 0x20]);
    let result = run3(&mut state, &mut memory, &set, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + 8);
    // al = 0x00 - 0x20: CF | SF (0xe0 has three bits: PF clear), ZF clear.
    assert_eq!(state.rflags & ARITH, CF | SF);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678_9abc_de00);
}

// ---- S4-39f19: `REX.W 81 /7 id` (CMP r/m64, imm32), register form ----

/// The arithmetic flags of `a - b` at qword size per the SDM (SUB/CMP): CF borrow, OF signed
/// overflow, SF, ZF, PF of the low byte, AF borrow from bit 4.
fn cmp64_flags(a: u64, b: u64) -> u64 {
    let result = a.wrapping_sub(b);
    let mut flags = 0;
    if a < b {
        flags |= CF;
    }
    if ((a ^ b) & (a ^ result)) >> 63 != 0 {
        flags |= OF;
    }
    if result >> 63 != 0 {
        flags |= SF;
    }
    if result == 0 {
        flags |= ZF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= PF;
    }
    if (a & 0xf) < (b & 0xf) {
        flags |= AF;
    }
    flags
}

#[test]
fn cmp_r64_imm32_register_form_sets_the_sub_flags_and_writes_nothing() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, register index, value, literal flags). The flags are those of
    // `reg - sign_extend(imm32)` at qword size, checked by hand against the helper below.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, u64)] = &[
        // `/init` at 0x40019d: cmpq $0x80, %rcx.
        (
            "48 81 f9 80 rcx=0x80",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0x80,
            ZF | PF,
        ),
        // 0x7f - 0x80 = -1: CF | SF, eight set bits: PF, no low-nibble borrow.
        (
            "48 81 f9 80 rcx=0x7f",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0x7f,
            CF | SF | PF,
        ),
        // 0 - 0x80 = 0xffff_ffff_ffff_ff80: CF | SF, one set bit in the low byte: PF clear.
        (
            "48 81 f9 80 rcx=0",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0,
            CF | SF,
        ),
        // 0x81 - 0x80 = 1: one set bit, PF clear; no flag at all.
        (
            "48 81 f9 80 rcx=0x81",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0x81,
            0,
        ),
        // i64::MIN - 0x80 = 0x7fff_ffff_ffff_ff80: signed overflow only.
        (
            "48 81 f9 80 rcx=i64::MIN",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0x8000_0000_0000_0000,
            OF,
        ),
        // The upper dword matters (64-bit compare): 0x1_0000_0080 - 0x80 = 0x1_0000_0000.
        (
            "48 81 f9 80 rcx=0x1_0000_0080",
            &[0x48, 0x81, 0xf9, 0x80, 0, 0, 0],
            1,
            0x1_0000_0080,
            PF,
        ),
        // `cmpq $-1, %rbx`: the imm32 0xffff_ffff sign-extends to u64::MAX.
        (
            "48 81 fb -1 rbx=-1",
            &[0x48, 0x81, 0xfb, 0xff, 0xff, 0xff, 0xff],
            3,
            u64::MAX,
            ZF | PF,
        ),
        // 0 - (-1) = 1: CF and the low-nibble borrow AF.
        (
            "48 81 fb -1 rbx=0",
            &[0x48, 0x81, 0xfb, 0xff, 0xff, 0xff, 0xff],
            3,
            0,
            CF | AF,
        ),
        // 0x7fff_ffff - (-1) = 0x8000_0000 (qword compare, no dword wrap): CF, PF (low byte 0);
        // equal low nibbles: no AF; no OF (the sign of the qword result is clear).
        (
            "48 81 fb -1 rbx=0x7fff_ffff",
            &[0x48, 0x81, 0xfb, 0xff, 0xff, 0xff, 0xff],
            3,
            0x7fff_ffff,
            CF | PF,
        ),
        // 0x8000_0000 imm32 is negative: sign-extended to 0xffff_ffff_8000_0000.
        (
            "48 81 f8 imm=0x8000_0000 rax=0xffff_ffff_8000_0000",
            &[0x48, 0x81, 0xf8, 0x00, 0x00, 0x00, 0x80],
            0,
            0xffff_ffff_8000_0000,
            ZF | PF,
        ),
        // The same imm32 against a zero-extended 0x8000_0000: 0x8000_0000 - 0xffff_ffff_8000_0000
        // = 0x1_0000_0000: CF only (low byte 0: PF).
        (
            "48 81 f8 imm=0x8000_0000 rax=0x8000_0000",
            &[0x48, 0x81, 0xf8, 0x00, 0x00, 0x00, 0x80],
            0,
            0x8000_0000,
            CF | PF,
        ),
        // REX.B: `cmpq $0x80, %r8` and `cmpq $0x80, %r15`.
        (
            "49 81 f8 80 r8=0x80",
            &[0x49, 0x81, 0xf8, 0x80, 0, 0, 0],
            8,
            0x80,
            ZF | PF,
        ),
        (
            "49 81 ff 80 r15=0x7f",
            &[0x49, 0x81, 0xff, 0x80, 0, 0, 0],
            15,
            0x7f,
            CF | SF | PF,
        ),
    ];
    for (name, bytes, register, value, flags) in cases {
        let imm = i64::from(i32::from_le_bytes(bytes[3..7].try_into().unwrap())) as u64;
        assert_eq!(cmp64_flags(*value, imm), *flags, "{name} helper");
        for initial in [0, ARITH] {
            let mut state = user_state();
            for (index, slot) in state.gpr.iter_mut().enumerate() {
                *slot = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
            }
            state.gpr[*register] = *value;
            state.rflags |= initial;
            let expected = after_cmpb(&state, 7, *flags);
            let mut memory = Memory::code(bytes);
            let result = run3(&mut state, &mut memory, &set, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.accesses, 0, "{name}");
        }
    }
}

#[test]
fn cmp_r64_imm32_then_jb_loops_as_in_init() {
    // `48 81 f9 80 00 00 00` (cmpq $0x80, %rcx) then `72 e3` (jb back by 29 bytes, taken for
    // rcx < 0x80): the loop tail of `/init` at 0x40019d.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for (rcx, taken) in [(0x7f_u64, true), (0x80, false), (0x81, false)] {
        let mut state = user_state();
        state.gpr[Register::Rcx as usize] = rcx;
        let mut memory = Memory::code(&[0x48, 0x81, 0xf9, 0x80, 0, 0, 0, 0x72, 0xe3]);
        let result = run3(&mut state, &mut memory, &set, 2).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2);
        let next = RIP + 9;
        assert_eq!(
            state.rip,
            if taken { next - 29 } else { next },
            "rcx={rcx:#x}"
        );
        assert_eq!(state.gpr[Register::Rcx as usize], rcx);
    }
}

#[test]
fn cmp_r64_imm32_outside_the_register_form_stays_refused() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        (
            "66 81 f9 80 00 cmp cx, imm16",
            &[0x66, 0x81, 0xf9, 0x80, 0],
            0x81,
        ),
        // Memory forms.
        (
            "48 81 3b 80 cmpq $0x80, (%rbx)",
            &[0x48, 0x81, 0x3b, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "48 81 7b 08 80 cmpq $0x80, 8(%rbx)",
            &[0x48, 0x81, 0x7b, 0x08, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "49 81 38 80 cmpq $0x80, (%r8)",
            &[0x49, 0x81, 0x38, 0x80, 0, 0, 0],
            0x81,
        ),
        // The other `81 /n` selectors, register form.
        (
            "48 81 e1 80 andq $0x80, %rcx (/4)",
            &[0x48, 0x81, 0xe1, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "48 81 c9 80 orq $0x80, %rcx (/1)",
            &[0x48, 0x81, 0xc9, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "48 81 e1 80 andq $0x80, %rcx (/4)",
            &[0x48, 0x81, 0xe1, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "48 81 e1 80 andq $0x80, %rcx (/4)",
            &[0x48, 0x81, 0xe1, 0x80, 0, 0, 0],
            0x81,
        ),
        (
            "48 81 f1 80 xorq $0x80, %rcx (/6)",
            &[0x48, 0x81, 0xf1, 0x80, 0, 0, 0],
            0x81,
        ),
        // Neighbouring opcodes.
        ("66 83 f9 80 cmp cx, imm8", &[0x66, 0x83, 0xf9, 0x80], 0x83),
        ("66 3d 80 00 cmp ax, imm16", &[0x66, 0x3d, 0x80, 0], 0x3d),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn cmp_r64_imm32_with_a_prefix_stays_refused() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for prefix in [
        0x66_u8, 0x67, 0xf0, 0xf2, 0xf3, 0x64, 0x65, 0x2e, 0x3e, 0x26, 0x36,
    ] {
        let bytes = [prefix, 0x48, 0x81, 0xf9, 0x80, 0, 0, 0];
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(&bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x81 },
            "prefix {prefix:#x}"
        );
        assert_eq!(result.instructions_executed, 0, "prefix {prefix:#x}");
        assert_eq!(state, before, "prefix {prefix:#x}");
        assert_eq!(memory.accesses, 0, "prefix {prefix:#x}");
    }
}

// ---- S4-39f18: `3C ib` (CMP AL, imm8) ----

#[test]
fn cmp_al_imm8_sets_the_byte_sub_flags_and_writes_nothing() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, al, expected flags). Flags of `al - imm8` at byte size. Hand-checked:
    // 0x00 - 0x20 = 0xe0 (CF, SF, three bits: PF clear), 0x20 - 0x20 = 0 (ZF, PF),
    // 0x80 - 0x20 = 0x60 (OF, two bits: PF), 0xff - 0x20 = 0xdf (SF, seven bits: PF clear).
    let cases: &[(&str, &[u8], u8, u64)] = &[
        ("3c 20 al=00", &[0x3c, 0x20], 0x00, CF | SF),
        ("3c 20 al=01", &[0x3c, 0x20], 0x01, CF | SF | PF),
        ("3c 20 al=20", &[0x3c, 0x20], 0x20, ZF | PF),
        ("3c 20 al=7f", &[0x3c, 0x20], 0x7f, PF),
        ("3c 20 al=80", &[0x3c, 0x20], 0x80, OF | PF),
        ("3c 20 al=ff", &[0x3c, 0x20], 0xff, SF),
        // `/init` at 0x400196 (after the `je` that follows the first `cmpb`).
        ("3c 0a al=0a", &[0x3c, 0x0a], 0x0a, ZF | PF),
        ("3c 0a al=20", &[0x3c, 0x0a], 0x20, AF),
        ("3c 0a al=00", &[0x3c, 0x0a], 0x00, CF | SF | PF | AF),
        // A negative immediate (0x80) is a plain byte.
        ("3c 80 al=7f", &[0x3c, 0x80], 0x7f, CF | OF | SF | PF),
        ("3c 00 al=00", &[0x3c, 0x00], 0x00, ZF | PF),
    ];
    for (name, bytes, al, _) in cases {
        for initial in [0, ARITH] {
            let mut state = user_state();
            for (index, value) in state.gpr.iter_mut().enumerate() {
                *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
            }
            // The upper bytes of RAX are garbage and must neither matter nor change.
            state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_ee00 | u64::from(*al);
            state.rflags |= initial;
            let flags = cmp8_flags(*al, bytes[1]);
            let expected = after_cmpb(&state, 2, flags);
            let mut memory = Memory::code(bytes);
            let result = run3(&mut state, &mut memory, &set, 1).unwrap();
            assert_eq!(
                exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{name}"
            );
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert_eq!(state, expected, "{name}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
            assert_eq!(memory.accesses, 0, "{name}");
        }
    }
    // The literal flags of the table agree with the SDM helper.
    for (name, bytes, al, flags) in cases {
        assert_eq!(cmp8_flags(*al, bytes[1]), *flags, "{name}");
    }
}

#[test]
fn cmp_al_imm8_with_a_prefix_or_a_neighbouring_opcode_stays_outside() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        ("66 3c 20 operand-size", &[0x66, 0x3c, 0x20], 0x3c),
        ("67 3c 20 address-size", &[0x67, 0x3c, 0x20], 0x3c),
        ("f0 3c 20 lock", &[0xf0, 0x3c, 0x20], 0x3c),
        ("f2 3c 20 repne", &[0xf2, 0x3c, 0x20], 0x3c),
        ("f3 3c 20 rep", &[0xf3, 0x3c, 0x20], 0x3c),
        ("64 3c 20 fs", &[0x64, 0x3c, 0x20], 0x3c),
        ("65 3c 20 gs", &[0x65, 0x3c, 0x20], 0x3c),
        ("2e 3c 20 cs", &[0x2e, 0x3c, 0x20], 0x3c),
        ("3e 3c 20 ds", &[0x3e, 0x3c, 0x20], 0x3c),
        ("26 3c 20 es", &[0x26, 0x3c, 0x20], 0x3c),
        ("36 3c 20 ss", &[0x36, 0x3c, 0x20], 0x3c),
        ("40 3c 20 REX", &[0x40, 0x3c, 0x20], 0x3c),
        ("41 3c 20 REX.B", &[0x41, 0x3c, 0x20], 0x3c),
        ("48 3c 20 REX.W", &[0x48, 0x3c, 0x20], 0x3c),
        (
            "66 40 3c 20 prefix before REX",
            &[0x66, 0x40, 0x3c, 0x20],
            0x3c,
        ),
        ("66 3d 20 00 cmp ax, imm16", &[0x66, 0x3d, 0x20, 0], 0x3d),
        ("3a c0 cmp al, al (3A)", &[0x3a, 0xc0], 0x3a),
        ("3b c0 cmp eax, eax (3B)", &[0x3b, 0xc0], 0x3b),
        ("f0 a8 20 LOCK test al, imm8", &[0xf0, 0xa8, 0x20], 0xa8),
        (
            "f2 80 f8 20 repne cmpb $0x20, %al",
            &[0xf2, 0x80, 0xf8, 0x20],
            0x80,
        ),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

// ---- S4-39f20: `F3 A4` (REP MOVSB) ----

const REP_SRC: u64 = DATA + 0x10;
const REP_DST: u64 = DATA + 0x200;

/// User state for `rep movsb`: `rsi = REP_SRC`, `rdi = REP_DST`, the given `rcx`, all arithmetic
/// flags set (REP MOVSB must leave them alone) and DF clear.
fn rep_state(rcx: u64) -> LongModeState {
    let mut state = mov_byte_state();
    state.gpr[Register::Rsi as usize] = REP_SRC;
    state.gpr[Register::Rdi as usize] = REP_DST;
    state.gpr[Register::Rcx as usize] = rcx;
    state.rflags |= ARITH;
    state
}

/// Code plus 32 user source bytes (`0x41 + i`) and 32 user destination bytes (`0xee`).
fn rep_memory(code: &[u8]) -> DataMemory {
    let mut memory = DataMemory::new(code);
    for index in 0..32 {
        memory = memory
            .with_user(REP_SRC + index, 0x41 + index as u8)
            .with_user(REP_DST + index, 0xee);
    }
    memory
}

fn after_rep(before: &LongModeState, done: u64, total: u64) -> LongModeState {
    let mut state = before.clone();
    state.gpr[Register::Rsi as usize] = before.gpr[Register::Rsi as usize].wrapping_add(done);
    state.gpr[Register::Rdi as usize] = before.gpr[Register::Rdi as usize].wrapping_add(done);
    state.gpr[Register::Rcx as usize] = total - done;
    state.rip = if done == total { RIP + 2 } else { RIP };
    state
}

fn run3_data_stop(
    state: &mut LongModeState,
    memory: &mut DataMemory,
    budget: u64,
    stop: impl FnMut() -> bool,
) -> Cpl0ProfileSetResult {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        stop,
    )
    .unwrap()
}

#[test]
fn rep_movsb_with_rcx_zero_is_a_retired_no_op() {
    let mut state = rep_state(0);
    let mut expected = state.clone();
    expected.rip = RIP + 2;
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn rep_movsb_copies_rcx_bytes_and_updates_only_rsi_rdi_rcx_and_rip() {
    for total in [1u64, 3, 16] {
        for flags in [0, ARITH] {
            let mut state = rep_state(total);
            state.rflags = (state.rflags & !ARITH) | flags;
            let expected = after_rep(&state, total, total);
            let mut memory = rep_memory(&[0xf3, 0xa4]);
            // Budget = rcx: one unit per iteration, the last one completes the instruction.
            let result = run3_data(&mut state, &mut memory, total).unwrap();
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, total);
            assert_eq!(state, expected, "rcx = {total}");
            assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
            assert_eq!(memory.reads, total as usize);
            assert_eq!(memory.writes.len(), total as usize);
            for index in 0..32u64 {
                let want = if index < total {
                    0x41 + index as u8
                } else {
                    0xee
                };
                assert_eq!(memory.user[&(REP_DST + index)], want, "byte {index}");
                assert_eq!(memory.user[&(REP_SRC + index)], 0x41 + index as u8);
            }
        }
    }
}

#[test]
fn rep_movsb_with_df_set_is_refused_without_change() {
    let mut state = rep_state(3);
    state.rflags |= 1 << 10;
    let before = state.clone();
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let result = run3_data(&mut state, &mut memory, 10).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xa4 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn rep_movsb_overlapping_forward_copy_repeats_the_first_byte() {
    // rdi = rsi + 1: each iteration reads the byte the previous one wrote.
    let mut state = rep_state(4);
    state.gpr[Register::Rdi as usize] = REP_SRC + 1;
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    assert_eq!(result.instructions_executed, 4);
    for index in 1..=4 {
        assert_eq!(memory.user[&(REP_SRC + index)], 0x41);
    }
}

#[test]
fn rep_movsb_non_canonical_addresses_are_general_protection_faults_with_partial_state() {
    let canonical_end = 0x0000_7fff_ffff_ffff_u64;
    // (name, rsi, rdi, iterations done before the fault)
    let cases: &[(&str, u64, u64, u64)] = &[
        ("rsi at the start", 0x0000_8000_0000_0000, REP_DST, 0),
        ("rdi at the start", REP_SRC, 0x0000_8000_0000_0000, 0),
        ("rsi after one iteration", canonical_end, REP_DST, 1),
        ("rdi after one iteration", REP_SRC, canonical_end, 1),
    ];
    for (name, rsi, rdi, done) in cases {
        let mut state = rep_state(3);
        state.gpr[Register::Rsi as usize] = *rsi;
        state.gpr[Register::Rdi as usize] = *rdi;
        let expected = after_rep(&state, *done, 3);
        let mut memory = rep_memory(&[0xf3, 0xa4]).with_user(canonical_end, 0x77);
        let result = run3_data(&mut state, &mut memory, 10).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }),
            "{name}"
        );
        assert_eq!(result.instructions_executed, *done, "{name}");
        assert_eq!(state, expected, "{name}");
        assert_eq!(memory.writes.len(), *done as usize, "{name}");
    }
}

#[test]
fn rep_movsb_page_faults_in_the_middle_keep_the_finished_iterations() {
    // The source byte of the second iteration is on a page without P: #PF with P clear, U set.
    let mut state = rep_state(3);
    let expected = after_rep(&state, 1, 3);
    let mut memory = rep_memory(&[0xf3, 0xa4]).with_absent(REP_SRC + 1);
    let result = run3_data(&mut state, &mut memory, 10).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x4),
            linear_address: Some(REP_SRC + 1),
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.writes, [(REP_DST, 0x41)]);

    // The destination byte of the second iteration is on a supervisor page: #PF with P, W, U.
    let mut state = rep_state(3);
    let expected = after_rep(&state, 1, 3);
    let mut memory = rep_memory(&[0xf3, 0xa4]).with_supervisor(REP_DST + 1, 0xcc);
    let result = run3_data(&mut state, &mut memory, 10).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x7),
            linear_address: Some(REP_DST + 1),
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.writes, [(REP_DST, 0x41)]);
    assert_eq!(memory.supervisor[&(REP_DST + 1)], 0xcc);

    // A fault on the very first iteration (supervisor source: P, U) changes nothing.
    let mut state = rep_state(3);
    let before = state.clone();
    let mut memory = rep_memory(&[0xf3, 0xa4]).with_supervisor(REP_SRC, 0xcc);
    let result = run3_data(&mut state, &mut memory, 10).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x5),
            linear_address: Some(REP_SRC),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn rep_movsb_stops_on_the_instruction_budget_in_the_middle_and_resumes() {
    let mut state = rep_state(5);
    let expected = after_rep(&state, 2, 5);
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let result = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state, expected);
    assert_eq!(state.rip, RIP);
    assert_eq!(state.gpr[Register::Rcx as usize], 3);
    assert_eq!(memory.writes.len(), 2);

    // Restarting the instruction with the remaining rcx finishes the copy; the next fetch past
    // the two code bytes is unmapped.
    let mut resumed = state.clone();
    let result = run3_data(&mut resumed, &mut memory, 10).unwrap();
    assert_eq!(result.instructions_executed, 3);
    assert!(matches!(exit(&result), RunExit::MemoryError(_)));
    assert_eq!(resumed, after_rep(&rep_state(5), 5, 5));
    for index in 0..5u64 {
        assert_eq!(memory.user[&(REP_DST + index)], 0x41 + index as u8);
    }
    assert_eq!(memory.user[&(REP_DST + 5)], 0xee);
}

#[test]
fn rep_movsb_stops_on_a_stop_request_before_any_iteration_and_in_the_middle() {
    // The request is polled before every iteration; it wins over the exhausted budget.
    for (polls_allowed, done) in [(0usize, 0u64), (1, 1), (2, 2)] {
        let mut state = rep_state(5);
        let expected = after_rep(&state, done, 5);
        let mut memory = rep_memory(&[0xf3, 0xa4]);
        let mut polls = 0usize;
        let result = run3_data_stop(&mut state, &mut memory, 2, || {
            polls += 1;
            polls > polls_allowed
        });
        assert_eq!(exit(&result), &RunExit::StopRequested, "{polls_allowed}");
        assert_eq!(result.instructions_executed, done);
        assert_eq!(state, expected, "{polls_allowed}");
        assert_eq!(memory.writes.len(), done as usize);
    }
}

#[test]
fn rep_movsb_at_the_stop_address_does_not_start() {
    let mut state = rep_state(3);
    let before = state.clone();
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut stop_config = config(PrivilegeLevel::Ring3, 10);
    stop_config.stop_rip = Some(RIP);
    let result = run_with_cpl3_profile_set(
        &mut state,
        &mut memory,
        stop_config,
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    assert_eq!(exit(&result), &RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn rep_movsb_refusals_and_neighbouring_string_opcodes_stay_outside() {
    let forms: &[(&str, &[u8], u8)] = &[
        ("66 f3 a4 operand-size", &[0x66, 0xf3, 0xa4], 0xa4),
        ("f3 66 a4 operand-size", &[0xf3, 0x66, 0xa4], 0xa4),
        ("67 f3 a4 address-size", &[0x67, 0xf3, 0xa4], 0xa4),
        ("f3 67 a4 address-size", &[0xf3, 0x67, 0xa4], 0xa4),
        ("f3 40 a4 REX", &[0xf3, 0x40, 0xa4], 0xa4),
        ("f3 48 a4 REX.W", &[0xf3, 0x48, 0xa4], 0xa4),
        ("f2 a4 repne", &[0xf2, 0xa4], 0xa4),
        ("f0 f3 a4 lock", &[0xf0, 0xf3, 0xa4], 0xa4),
        ("f3 64 a4 fs", &[0xf3, 0x64, 0xa4], 0xa4),
        ("f3 2e a4 cs", &[0xf3, 0x2e, 0xa4], 0xa4),
        ("a4 movsb without rep", &[0xa4], 0xa4),
        ("f3 66 a5 rep movsw", &[0xf3, 0x66, 0xa5], 0xa5),
        ("f3 48 a5 rep movsq", &[0xf3, 0x48, 0xa5], 0xa5),
        ("a5 movsd without rep", &[0xa5], 0xa5),
        ("f3 66 ab rep stosw", &[0xf3, 0x66, 0xab], 0xab),
        ("aa stosb without rep", &[0xaa], 0xaa),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = rep_state(3);
        let before = state.clone();
        let mut memory = rep_memory(bytes);
        let result = run3_data(&mut state, &mut memory, 10).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.reads, 0, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

#[test]
fn rep_movsb_is_refused_on_a_state_the_step_would_not_admit() {
    // (name, state change, initial rflags_defined)
    type Change = fn(&mut LongModeState);
    let cases: &[(&str, Change, u64)] = &[
        (
            "activity Halted",
            |state| state.activity = Some(CpuActivity::Halted),
            od_isa::INTEGER_FLAGS,
        ),
        (
            "sti_shadow Blocking with IF = 1",
            |state| state.sti_shadow = Some(StiShadow::Blocking),
            od_isa::INTEGER_FLAGS,
        ),
        (
            "cr0 missing",
            |state| state.cr0 = None,
            od_isa::INTEGER_FLAGS,
        ),
        (
            "cr4 missing",
            |state| state.cr4 = None,
            od_isa::INTEGER_FLAGS,
        ),
        (
            "RFLAGS bit 1 clear",
            |state| state.rflags &= !2,
            od_isa::INTEGER_FLAGS,
        ),
        (
            "RFLAGS.VM set",
            |state| state.rflags |= 1 << 17,
            od_isa::INTEGER_FLAGS,
        ),
        (
            "initial rflags_defined outside the integer flags",
            |_| {},
            od_isa::INTEGER_FLAGS | (1 << 9),
        ),
    ];
    for (name, change, defined) in cases {
        let mut state = rep_state(3);
        change(&mut state);
        let before = state.clone();
        let mut memory = rep_memory(&[0xf3, 0xa4]);
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
        let result = run_with_cpl3_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 10),
            *defined,
            &set,
            None,
            || false,
        )
        .unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0xa4 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.reads, 0, "{name}");
        assert!(memory.writes.is_empty(), "{name}");
    }
}

#[test]
fn rep_movsb_with_rcx_zero_stops_on_a_stop_request_before_retiring() {
    let mut state = rep_state(0);
    let before = state.clone();
    let mut memory = rep_memory(&[0xf3, 0xa4]);
    let result = run3_data_stop(&mut state, &mut memory, 10, || true);
    assert_eq!(exit(&result), &RunExit::StopRequested);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(state.rip, RIP);
}

// ---- S4-39f21: `REX.W 29 /r` (SUB r/m64, r64), register form ----

#[test]
fn sub_r64_register_form_writes_the_difference_and_sets_the_sub_flags() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    // (name, bytes, dst index, dst value, src index, src value, literal flags of dst - src). The
    // literals are checked by hand and against `cmp64_flags` (the SUB flag rules) below.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &[u8], usize, u64, usize, u64, u64)] = &[
        // `/init` at 0x4001db: subq %rsi, %rdx (rm = rdx = dst, reg = rsi = src). Equal: 0.
        ("48 29 f2 equal", &[0x48, 0x29, 0xf2], 2, 7, 6, 7, ZF | PF),
        // 3 - 5 = 0xffff_ffff_ffff_fffe: CF, SF, AF (the low nibble borrows); the low byte 0xfe
        // has seven set bits (odd): PF clear.
        (
            "48 29 f2 3-5",
            &[0x48, 0x29, 0xf2],
            2,
            3,
            6,
            5,
            CF | SF | AF,
        ),
        // 5 - 3 = 2 (one set bit: PF clear): no flag.
        ("48 29 f2 5-3", &[0x48, 0x29, 0xf2], 2, 5, 6, 3, 0),
        // The `/init` shape: end pointer minus start pointer, 0x402014 - 0x402000 = 0x14: two set
        // bits, PF; no borrow.
        (
            "48 29 f2 pointers",
            &[0x48, 0x29, 0xf2],
            2,
            0x40_2014,
            6,
            0x40_2000,
            PF,
        ),
        // Low-nibble borrow only: 0x10 - 1 = 0x0f (four set bits): AF, PF.
        (
            "48 29 f2 0x10-1",
            &[0x48, 0x29, 0xf2],
            2,
            0x10,
            6,
            1,
            AF | PF,
        ),
        // Signed overflow: i64::MIN - 1 = i64::MAX (low byte 0xff): OF, PF, AF; no CF, no SF.
        (
            "48 29 f2 min-1",
            &[0x48, 0x29, 0xf2],
            2,
            0x8000_0000_0000_0000,
            6,
            1,
            OF | PF | AF,
        ),
        // 0 - i64::MIN = 0x8000_0000_0000_0000: CF, OF, SF, PF (low byte 0); equal low nibbles.
        (
            "48 29 f2 0-min",
            &[0x48, 0x29, 0xf2],
            2,
            0,
            6,
            0x8000_0000_0000_0000,
            CF | OF | SF | PF,
        ),
        // The high half decides: 0x1_0000_0000 - 1 = 0xffff_ffff (no dword wrap): PF, AF.
        (
            "48 29 f2 high half",
            &[0x48, 0x29, 0xf2],
            2,
            0x1_0000_0000,
            6,
            1,
            PF | AF,
        ),
        // Borrow from the high half only: 0 - 2^32 = 0xffff_ffff_0000_0000: CF, SF, PF.
        (
            "48 29 f2 0-2^32",
            &[0x48, 0x29, 0xf2],
            2,
            0,
            6,
            0x1_0000_0000,
            CF | SF | PF,
        ),
        // REX.W only, other registers: subq %rcx, %rax (48 29 c8): rm = rax = dst, reg = rcx.
        (
            "48 29 c8 REX.W",
            &[0x48, 0x29, 0xc8],
            0,
            1,
            1,
            2,
            CF | SF | PF | AF,
        ),
        // REX.WB: subq %rax, %r15 (49 29 c7): rm = r15 = dst, reg = rax = src.
        ("49 29 c7 REX.WB", &[0x49, 0x29, 0xc7], 15, 4, 0, 4, ZF | PF),
        // REX.WR: subq %r9, %rbx (4c 29 cb): rm = rbx = dst, reg = r9 = src.
        (
            "4c 29 cb REX.WR",
            &[0x4c, 0x29, 0xcb],
            3,
            0x10,
            9,
            0x20,
            CF | SF | PF,
        ),
        // REX.WRB: subq %r9, %r10 (4d 29 ca): rm = r10 = dst, reg = r9 = src.
        (
            "4d 29 ca REX.WRB",
            &[0x4d, 0x29, 0xca],
            10,
            0x50,
            9,
            0x20,
            PF,
        ),
        // REX.WX (X is ignored with mod = 11): subq %rcx, %rax (4a 29 c8).
        ("4a 29 c8 REX.WX", &[0x4a, 0x29, 0xc8], 0, 3, 1, 3, ZF | PF),
        // The same register: subq %rbx, %rbx is zero whatever the value.
        (
            "48 29 db same register",
            &[0x48, 0x29, 0xdb],
            3,
            0x1234,
            3,
            0x1234,
            ZF | PF,
        ),
    ];
    for (name, bytes, dst, dst_value, src, src_value, flags) in cases {
        let mut state = user_state();
        for (index, value) in state.gpr.iter_mut().enumerate() {
            *value = 0x1111_1111_1111_1111_u64.wrapping_mul(index as u64 + 1);
        }
        state.gpr[*dst] = *dst_value;
        state.gpr[*src] = *src_value;
        // Every arithmetic flag set before: SUB recomputes all six.
        state.rflags |= ARITH;
        let mut expected = state.clone();
        expected.rip = RIP + bytes.len() as u64;
        // The whole 64-bit destination is written.
        expected.gpr[*dst] = dst_value.wrapping_sub(*src_value);
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{name}"
        );
        assert_eq!(result.instructions_executed, 1, "{name}");
        // Only the destination register changes; RIP advances by the length.
        assert_eq!(state.gpr, expected.gpr, "{name}");
        assert_eq!(state.rip, expected.rip, "{name}");
        assert_eq!(state.rflags & ARITH, *flags, "{name}");
        assert_eq!(
            state.rflags & ARITH,
            cmp64_flags(*dst_value, *src_value),
            "{name}"
        );
        // The non-arithmetic flags (IF, the reserved bit 1) stay.
        assert_eq!(state.rflags & !ARITH, expected.rflags & !ARITH, "{name}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn sub_r64_then_movl_chain_follows_init_after_the_pointer_difference() {
    // `/init` at 0x4001d8..: `movq %rdi, %rdx; subq %rsi, %rdx; movl $1, %edi; movl $1, %eax`
    // then a `f4` sentinel (outside) instead of the `syscall`: rdx = rdi - rsi, rdi and rax = 1.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = 0x40_2014;
    state.gpr[Register::Rsi as usize] = 0x40_2000;
    let mut memory = Memory::code(&[
        0x48, 0x89, 0xfa, 0x48, 0x29, 0xf2, 0xbf, 1, 0, 0, 0, 0xb8, 1, 0, 0, 0, 0xf4,
    ]);
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state.rip, RIP + 16);
    assert_eq!(state.gpr[Register::Rdx as usize], 0x14);
    assert_eq!(state.gpr[Register::Rdi as usize], 1);
    assert_eq!(state.gpr[Register::Rax as usize], 1);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x40_2000);
    // The flags are those of the `subq` (the `movl`s leave them).
    assert_eq!(state.rflags & ARITH, PF);
}

#[test]
fn sub_outside_the_admitted_forms_stays_refused() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8], u8)] = &[
        // Word register forms.
        ("66 29 f2 subw %si, %dx", &[0x66, 0x29, 0xf2], 0x29),
        (
            "66 41 29 f2 subw %si, %r10w",
            &[0x66, 0x41, 0x29, 0xf2],
            0x29,
        ),
        // Neighbouring opcodes keep their own bytes.
        ("48 2b f2 subq %rdx, %rsi (2B)", &[0x48, 0x2b, 0xf2], 0x2b),
        ("48 2a f2 (2A)", &[0x48, 0x2a, 0xf2], 0x2a),
        ("48 28 f2 (28)", &[0x48, 0x28, 0xf2], 0x28),
        ("48 2c 01 (2C)", &[0x48, 0x2c, 0x01], 0x2c),
        (
            "48 2d 01 00 00 00 subq $1, %rax (2D)",
            &[0x48, 0x2d, 1, 0, 0, 0],
            0x2d,
        ),
        (
            "48 83 2a 01 subq $1, (%rdx) (83 /5)",
            &[0x48, 0x83, 0x2a, 0x01],
            0x83,
        ),
        (
            "48 81 2a 80 00 00 00 subq $0x80, (%rdx) (81 /5)",
            &[0x48, 0x81, 0x2a, 0x80, 0, 0, 0],
            0x81,
        ),
    ];
    for (name, bytes, opcode) in forms {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: *opcode },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(memory.accesses, 0, "{name}");
    }
}

#[test]
fn sub_r64_with_a_prefix_stays_refused() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for prefix in [
        0x66_u8, 0x67, 0xf0, 0xf2, 0xf3, 0x64, 0x65, 0x2e, 0x3e, 0x26, 0x36,
    ] {
        let bytes = [prefix, 0x48, 0x29, 0xf2];
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(&bytes);
        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x29 },
            "prefix {prefix:#x}"
        );
        assert_eq!(result.instructions_executed, 0, "prefix {prefix:#x}");
        assert_eq!(state, before, "prefix {prefix:#x}");
        assert_eq!(memory.accesses, 0, "prefix {prefix:#x}");
    }
}

#[test]
fn cpl3_direct_call_and_leave_restore_the_user_stack() {
    let code = [
        0xe8, 1, 0, 0, 0,    // call RIP+6
        0xf4, // stop at the return address
        0x55, // push rbp
        0x48, 0x89, 0xe5, // mov rbp, rsp
        0x48, 0x81, 0xec, 0xa0, 1, 0, 0, // sub rsp, 0x1a0
        0x48, 0x8d, 0xbd, 0x60, 0xfe, 0xff, 0xff, // lea rdi, [rbp - 0x1a0]
        0xc9, 0xc3, // leave; ret
    ];
    let mut state = user_state();
    state.gpr[Register::Rsp as usize] = DATA + 24;
    state.gpr[Register::Rbp as usize] = 0xffed_cba9_8765_4321;
    let mut expected = state.clone();
    expected.rip += 5;
    expected.gpr[Register::Rdi as usize] = DATA + 8 - 0x1a0;
    let mut memory = DataMemory::new(&code);
    for address in DATA + 8..DATA + 24 {
        memory = memory.with_user(address, 0);
    }

    let result = run3_data(&mut state, &mut memory, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0xf4 });
    assert_eq!(result.instructions_executed, 7);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 2);
    assert_eq!(memory.writes.len(), 16);
}

#[test]
fn cpl3_leave_fault_keeps_both_stack_registers_and_rip() {
    let mut state = user_state();
    state.gpr[Register::Rbp as usize] = DATA;
    state.gpr[Register::Rsp as usize] = DATA - 0x100;
    let expected = state.clone();
    let mut memory = DataMemory::new(&[0xc9]);
    for offset in 0..8 {
        memory = memory.with_supervisor(DATA + offset, 0x55);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, expected);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_rep_movsd_and_narrow_stos_restart_at_the_first_uncommitted_element() {
    let accumulator = 0x0807_0605_0403_0201u64;
    let source = DATA + 0x100;
    for (code, width, move_source) in [
        (&[0xf3, 0xa5][..], 4u64, true),
        (&[0xf3, 0xaa][..], 1, false),
        (&[0xf3, 0xab][..], 4, false),
    ] {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = accumulator;
        state.gpr[Register::Rcx as usize] = 3;
        state.gpr[Register::Rsi as usize] = source;
        state.gpr[Register::Rdi as usize] = DATA;
        state.rflags |= ARITH;
        let mut expected = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..3 * width {
            memory = memory.with_user(source + offset, 0xd0 + offset as u8);
            memory = if offset < 2 * width {
                memory.with_user(DATA + offset, 0)
            } else {
                memory.with_supervisor(DATA + offset, 0)
            };
        }
        let first = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&first), &RunExit::InstructionBudgetExhausted);
        assert_eq!(first.instructions_executed, 1);
        expected.gpr[Register::Rcx as usize] = 2;
        expected.gpr[Register::Rdi as usize] += width;
        if move_source {
            expected.gpr[Register::Rsi as usize] += width;
        }
        assert_eq!(state, expected);
        let last = run3_data(&mut state, &mut memory, 5).unwrap();
        assert_eq!(
            exit(&last),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(DATA + 2 * width),
            })
        );
        assert_eq!(last.instructions_executed, 1);
        expected.gpr[Register::Rcx as usize] = 1;
        expected.gpr[Register::Rdi as usize] += width;
        if move_source {
            expected.gpr[Register::Rsi as usize] += width;
        }
        assert_eq!(state, expected);
        let value = accumulator.to_le_bytes();
        let expected_writes: Vec<_> = (0..2 * width)
            .map(|offset| {
                let byte = if move_source {
                    0xd0 + offset as u8
                } else {
                    value[(offset % width) as usize]
                };
                (DATA + offset, byte)
            })
            .collect();
        assert_eq!(memory.writes, expected_writes);
        assert_eq!(memory.reads, if move_source { 3 } else { 0 });
    }
}
#[test]
fn cpl3_rep_stosq_preserves_completed_iterations_at_budget_and_fault() {
    let value = 0x0807_0605_0403_0201_u64;
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = value;
    state.gpr[Register::Rcx as usize] = 3;
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rsi as usize] = 0xffed_cba9_8765_4321;
    let mut expected = state.clone();
    let mut memory = DataMemory::new(&[0xf3, 0x48, 0xab]);
    for offset in 0..16 {
        memory = memory
            .with_user(DATA + offset, 0)
            .with_supervisor(DATA + 16 + offset, 0);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    expected.gpr[Register::Rcx as usize] = 2;
    expected.gpr[Register::Rdi as usize] += 8;
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);

    let result = run3_data(&mut state, &mut memory, 5).unwrap();
    expected.gpr[Register::Rcx as usize] = 1;
    expected.gpr[Register::Rdi as usize] += 8;
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(DATA + 16),
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 0);
    assert_eq!(
        memory.writes,
        (DATA..DATA + 16)
            .zip(value.to_le_bytes().into_iter().cycle())
            .collect::<Vec<_>>()
    );
}

#[test]
fn cpl3_auxv_word_dword_and_qword_stores_use_their_encoded_widths() {
    let code = [
        0x48, 0xc7, 0x07, 0xfe, 0xff, 0xff, 0xff, // mov qword ptr [rdi], -2
        0x0f, 0xb7, 0x07, // movzx eax, word ptr [rdi]
        0x66, 0x89, 0x47, 0x08, // mov [rdi + 8], ax
        0x89, 0x47, 0x0a, // mov [rdi + 10], eax
    ];
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rax as usize] = u64::MAX;
    state.rflags |= ARITH;
    let mut expected = state.clone();
    expected.rip += code.len() as u64;
    expected.gpr[Register::Rax as usize] = 0xfffe;
    let mut memory = DataMemory::new(&code);
    for offset in 0..16 {
        memory = memory.with_user(DATA + offset, 0xcc);
    }

    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.writes.len(), 14);
    assert_eq!(memory.user[&(DATA + 14)], 0xcc);
    assert_eq!(memory.user[&(DATA + 15)], 0xcc);
    assert_eq!(
        (DATA..DATA + 14)
            .map(|address| memory.user[&address])
            .collect::<Vec<_>>(),
        [
            0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xfe, 0xff, 0, 0
        ]
    );
}

#[test]
fn cpl3_tunables_dword_load_clears_the_upper_half() {
    let code = [0x8b, 0x15, 0xb2, 0x2d, 0x18, 0x00, 0x90];
    let address = RIP + 6 + 0x182db2;
    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = u64::MAX;
    state.rflags |= ARITH;
    let mut expected = state.clone();
    expected.gpr[Register::Rdx as usize] = 0xfedc_ba98;
    expected.rip += code.len() as u64;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 0xfedc_ba98u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(address + offset as u64, byte);
    }

    let result = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());

    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = u64::MAX;
    let initial = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_supervisor(address + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, initial);
}

#[test]
fn cpl3_tunables_cmp_dil_defines_flags_without_a_store() {
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = 0xffee_ddcc_bbaa_993d;
    let mut expected = state.clone();
    expected.rip += 4;
    expected.rflags = (expected.rflags & !ARITH) | ZF | PF;
    let mut memory = DataMemory::new(&[0x40, 0x80, 0xff, 0x3d]);

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_rseq_cmp_eax_imm32_preserves_full_rax_and_sets_sub_flags() {
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x1234_5678_0000_0000;
    state.rflags |= ARITH;
    let mut expected = state.clone();
    expected.rip += 5;
    expected.rflags = (expected.rflags & !ARITH) | CF | PF;
    let mut memory = DataMemory::new(&[0x3d, 0x00, 0xf0, 0xff, 0xff]);

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_rseq_cmp_eax_imm32_drives_unsigned_ja_error_check() {
    let code = [
        0x3d, 0x00, 0xf0, 0xff, 0xff, // cmp eax, 0xfffff000
        0x77, 0x05, // ja error
        0xb8, 0x01, 0x00, 0x00, 0x00, // success continuation (not executed)
    ];
    for (rax, branch_taken) in [
        (0x7654_3210_0000_0000u64, false),
        (0x7654_3210_ffff_fff2u64, true),
    ] {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = rax;
        let mut memory = DataMemory::new(&code);

        let result = run3_data(&mut state, &mut memory, 2).unwrap();

        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.gpr[Register::Rax as usize], rax);
        assert_eq!(state.rip, RIP + if branch_taken { 12 } else { 7 });
        assert_eq!(state.rflags & ZF, 0);
        assert_eq!(state.rflags & CF, if branch_taken { 0 } else { CF });
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_rseq_cmp_word_form_stays_refused_and_qword_form_is_preserved() {
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xfeed_face_1234_5678;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x66, 0x3d, 0x00, 0xf0]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert!(matches!(
        exit(&result),
        RunExit::Unsupported { opcode: 0x3d }
    ));
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);

    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xfeed_face_1234_5678;
    let before = state.clone();
    let mut memory = DataMemory::new(&[0x48, 0x3d, 0x00, 0xf0, 0xff, 0xff]);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.gpr[Register::Rax as usize],
        before.gpr[Register::Rax as usize]
    );
    assert_eq!(state.rip, RIP + 6);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_tunables_add_rsp_imm32_sign_extends() {
    let mut state = user_state();
    state.gpr[Register::Rsp as usize] = 0x2_0000_0000;
    let mut memory = DataMemory::new(&[0x48, 0x81, 0xc4, 0x80, 0xff, 0xff, 0xff]);

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x1_ffff_ff80);
    assert_eq!(state.rip, RIP + 7);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_tunables_add_dword_imm32_and_shr_dword_imm8_execute_register_forms() {
    let code = [
        0x81, 0xc1, 0x40, 0x09, 0x00, 0x00, // add ecx, 0x940
        0xc1, 0xe9, 0x04, // shr ecx, 4
    ];
    let mut state = user_state();
    state.gpr[Register::Rcx as usize] = 0x1000;
    let mut memory = DataMemory::new(&code);

    let result = run3_data(&mut state, &mut memory, 2).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x194);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_glibc_tls_fs_load_and_adjacent_stores_use_live_fs_base() {
    let code = [
        0x64, 0x48, 0x8b, 0x14, 0x25, 0x10, 0x00, 0x00, 0x00, // mov rdx, fs:[0x10]
        0x64, 0x48, 0x89, 0x04, 0x25, 0x10, 0x05, 0x00, 0x00, // mov fs:[0x510], rax
        0x64, 0xc7, 0x04, 0x25, 0x14, 0x06, 0x00, 0x00, 0x02, 0x00, 0x00,
        0x00, // mov dword fs:[0x614], 2
        0x64, 0x88, 0x04, 0x25, 0x11, 0x06, 0x00, 0x00, // mov byte fs:[0x611], al
    ];
    let fs = DATA + 0x100;
    let tls_value = 0x8877_6655_4433_2211u64;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0033;
    state.gpr[Register::Rdx as usize] = 0xdead_beef;
    let mut memory = DataMemory::new(&code).with_user(0x10, 0x99);
    for (offset, byte) in tls_value.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(fs + 0x10 + offset as u64, byte);
    }
    for offset in 0..8 {
        memory = memory.with_user(fs + 0x510 + offset, 0);
    }
    for offset in 0..4 {
        memory = memory.with_user(fs + 0x614 + offset, 0);
    }
    memory = memory.with_user(fs + 0x611, 0);

    let result = run3_data(&mut state, &mut memory, 4).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state.gpr[Register::Rdx as usize], tls_value);
    assert_eq!(state.fs_base, fs);
    assert_eq!(memory.reads, 1);
    let mut expected_writes = (0..8)
        .map(|offset| {
            (
                fs + 0x510 + offset,
                (0xaabb_ccdd_eeff_0033u64 >> (offset * 8)) as u8,
            )
        })
        .collect::<Vec<_>>();
    expected_writes
        .extend((0..4).map(|offset| (fs + 0x614 + offset, if offset == 0 { 2 } else { 0 })));
    expected_writes.push((fs + 0x611, 0x33));
    assert_eq!(memory.writes, expected_writes);
}

#[test]
fn cpl3_glibc_tls_fs_dword_load_uses_live_base_and_zero_extends_rax() {
    let code = [0x64, 0x8b, 0x04, 0x25, 0xd0, 0x02, 0x00, 0x00];
    let fs = DATA + 0x1000;
    let value = 0x89ab_cdefu32;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rax as usize] = 0xfeed_face_aaaa_bbbb;
    state.rflags |= ARITH;
    let flags = state.rflags;
    let mut memory = DataMemory::new(&code).with_user(DATA + 0x2d0, 0x11);
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(fs + 0x2d0 + offset as u64, byte);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rax as usize], u64::from(value));
    assert_eq!(state.fs_base, fs);
    assert_eq!(state.rflags, flags);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_glibc_tls_fs_dword_load_faults_before_register_commit() {
    let code = [0x64, 0x8b, 0x04, 0x25, 0xd0, 0x02, 0x00, 0x00];
    let mut state = user_state();
    state.fs_base = DATA;
    state.gpr[Register::Rax as usize] = 0xfeed_face_aaaa_bbbb;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_supervisor(DATA + 0x2d0 + offset, 0x55);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA + 0x2d0),
        })
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());

    let mut state = user_state();
    state.fs_base = 0x0000_7fff_ffff_fff8;
    state.gpr[Register::Rax as usize] = 0xfeed_face_aaaa_bbbb;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    ));
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn bare_ring3_run_still_refuses_tls_fs_dword_load_before_memory_access() {
    let code = [0x64, 0x8b, 0x04, 0x25, 0xd0, 0x02, 0x00, 0x00];
    let mut state = user_state();
    state.fs_base = DATA;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_user(DATA + 0x2d0 + offset, 0x44);
    }

    let result = od_isa::execute::run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        || false,
    );

    assert!(matches!(result.exit, RunExit::Unsupported { .. }));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_glibc_tls_fs_allowlist_rejects_gs_indexed_and_nonmatching_width_forms() {
    for code in [
        &[0x64, 0x48, 0x8b, 0x14, 0x0d, 0x10, 0x00, 0x00, 0x00][..],
        &[0x65, 0x48, 0x8b, 0x14, 0x25, 0x10, 0x00, 0x00, 0x00][..],
        &[0x64, 0x66, 0x8b, 0x14, 0x25, 0x10, 0x00, 0x00, 0x00][..],
    ] {
        let mut state = user_state();
        state.fs_base = DATA;
        state.gs_base = DATA;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        state.gpr[Register::Rcx as usize] = 0;
        for offset in 0..8 {
            memory = memory.with_user(DATA + 0x10 + offset, 0x11);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x8b });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn cpl3_glibc_tls_fs_direct_forms_use_encoded_registers_offsets_and_immediates() {
    let code = [
        0x64, 0x48, 0x8b, 0x0c, 0x25, 0x28, 0x00, 0x00, 0x00, // mov rcx, fs:[0x28]
        0x64, 0x4c, 0x89, 0x04, 0x25, 0x28, 0x05, 0x00, 0x00, // mov fs:[0x528], r8
        0x64, 0xc7, 0x04, 0x25, 0x24, 0x06, 0x00, 0x00, 0x07, 0x00, 0x00,
        0x00, // mov dword fs:[0x624], 7
        0x64, 0x88, 0x14, 0x25, 0x21, 0x06, 0x00, 0x00, // mov fs:[0x621], dl
    ];
    let fs = DATA + 0x200;
    let value = 0x1020_3040_5060_7080u64;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::R8 as usize] = value;
    state.gpr[Register::Rdx as usize] = 0x5a;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(fs + 0x28 + offset as u64, byte);
    }
    for offset in 0..8 {
        memory = memory.with_user(fs + 0x528 + offset, 0);
    }
    for offset in 0..4 {
        memory = memory.with_user(fs + 0x624 + offset, 0);
    }
    memory = memory.with_user(fs + 0x621, 0);

    let result = run3_data(&mut state, &mut memory, 4).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rcx as usize], value);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.writes.last(), Some(&(fs + 0x621, 0x5a)));
}

#[test]
fn bare_ring3_run_still_refuses_tls_fs_before_memory_access() {
    let code = [0x64, 0x48, 0x8b, 0x14, 0x25, 0x10, 0x00, 0x00, 0x00];
    let mut state = user_state();
    state.fs_base = DATA;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_user(DATA + 0x10 + offset, 0x44);
    }

    let result = od_isa::execute::run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        || false,
    );

    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0x0f });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_glibc_tls_fs_load_and_store_fault_before_cpu_or_memory_mutation() {
    let load = [0x64, 0x48, 0x8b, 0x14, 0x25, 0x10, 0x00, 0x00, 0x00];
    let mut state = user_state();
    state.fs_base = DATA;
    state.gpr[Register::Rdx as usize] = 0x1234;
    let before = state.clone();
    let mut memory = DataMemory::new(&load);
    for offset in 0..8 {
        memory = memory.with_supervisor(DATA + 0x10 + offset, 0x55);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA + 0x10),
        })
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 1);

    let store = [0x64, 0x48, 0x89, 0x04, 0x25, 0x10, 0x05, 0x00, 0x00];
    let mut state = user_state();
    state.fs_base = DATA;
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
    let before = state.clone();
    let mut memory = DataMemory::new(&store);
    for offset in 0..8 {
        memory = memory.with_supervisor(DATA + 0x510 + offset, 0x77);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(DATA + 0x510),
        })
    );
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());

    let mut state = user_state();
    state.fs_base = 0x0000_7fff_ffff_fff8;
    let before = state.clone();
    let mut memory = DataMemory::new(&load);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    ));
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_glibc_rseq_add_qword_from_fs_uses_live_base_flags_and_user_permissions() {
    let code = [0x64, 0x48, 0x03, 0x3c, 0x25, 0x00, 0x00, 0x00, 0x00];
    let fs = DATA + 0x200;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rdi as usize] = 2;
    state.rflags |= ARITH;
    let mut memory = DataMemory::new(&code).with_user(fs, 1);
    for offset in 1..8 {
        memory = memory.with_user(fs + offset, 0);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rdi as usize], 3);
    assert_eq!(state.rflags & ARITH, PF);
    assert_eq!(state.fs_base, fs);
    assert_eq!(memory.reads, 1);

    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rdi as usize] = 2;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_supervisor(fs + offset, 1);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(fs),
        })
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 1);
}

#[test]
fn cpl3_glibc_rseq_fs_add_and_imm_store_keep_forms_bounded() {
    let invalid = [
        &[0x65, 0x48, 0x03, 0x3c, 0x25, 0, 0, 0, 0][..], // GS instead of FS
        &[0x64, 0x48, 0x03, 0x3c, 0x0d, 0, 0, 0, 0][..], // indexed SIB
        &[0x64, 0x49, 0xc7, 0x40, 0x04, 0xff, 0xff, 0xff, 0xff][..], // REX.W changes width
    ];
    for code in invalid {
        let mut state = user_state();
        state.fs_base = DATA;
        state.gpr[Register::Rdi as usize] = 0;
        state.gpr[Register::R8 as usize] = 0x100;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_user(DATA + offset, 1);
            memory = memory.with_user(DATA + 0x104 + offset, 1);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(exit(&result), RunExit::Unsupported { .. }));
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_glibc_rseq_fs_dword_immediate_store_uses_register_base() {
    let code = [0x64, 0x41, 0xc7, 0x40, 0x04, 0xff, 0xff, 0xff, 0xff];
    let fs = DATA + 0x200;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::R8 as usize] = 0x40;
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_user(fs + 0x44 + offset, 0);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(memory.reads, 0);
    assert_eq!(
        memory.writes,
        (0..4)
            .map(|offset| (fs + 0x44 + offset, 0xff))
            .collect::<Vec<_>>()
    );

    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::R8 as usize] = 0x40;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_supervisor(fs + 0x44 + offset, 0x55);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(fs + 0x44),
        })
    );
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());
}

#[test]
fn bare_ring3_run_still_refuses_rseq_fs_add_and_imm_store() {
    let cases = [
        &[0x64, 0x48, 0x03, 0x3c, 0x25, 0, 0, 0, 0][..],
        &[0x64, 0x41, 0xc7, 0x40, 0x04, 0xff, 0xff, 0xff, 0xff][..],
    ];
    for code in cases {
        let mut state = user_state();
        state.fs_base = DATA;
        state.gpr[Register::Rdi as usize] = 1;
        state.gpr[Register::R8 as usize] = 0x40;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_user(DATA + offset, 1);
            memory = memory.with_user(DATA + 0x44 + offset, 0);
        }

        let result = od_isa::execute::run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 1),
            || false,
        );

        assert!(matches!(result.exit, RunExit::Unsupported { .. }));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_lock_add_qword_imm8_executes_one_atomic_and_sets_add_flags() {
    let code = [0xf0, 0x48, 0x83, 0x03, 0x01]; // lock add qword [rbx], 1
    let initial = 0x7fff_ffff_ffff_ffffu64;
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = DATA;
    state.rflags |= ARITH;
    let preserved = state.rflags & !ARITH;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in initial.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + offset as u64, byte);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.rflags, preserved | OF | SF | AF | PF);
    assert_eq!(memory.atomic_rmws, 1);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.writes.len(), 8);
    let actual = (0..8).fold(0u64, |value, offset| {
        value | (u64::from(memory.user[&(DATA + offset)]) << (offset * 8))
    });
    assert_eq!(actual, 0x8000_0000_0000_0000);
}

#[test]
fn cpl3_lock_add_qword_imm8_refuses_other_forms_and_unaligned_operands() {
    for code in [
        &[0xf0, 0x48, 0x81, 0x03, 1, 0, 0, 0][..], // imm32 opcode
        &[0x64, 0xf0, 0x48, 0x83, 0x03, 1][..],    // FS override
        &[0x67, 0xf0, 0x48, 0x83, 0x03, 1][..],    // address-size override
    ] {
        let mut state = user_state();
        state.gpr[Register::Rbx as usize] = DATA;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_user(DATA + offset, 1);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(exit(&result), RunExit::Unsupported { .. }));
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert_eq!(memory.atomic_rmws, 0);
        assert!(memory.writes.is_empty());
    }

    let code = [0xf0, 0x48, 0x83, 0x03, 0x01];
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = DATA + 1;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_user(DATA + 1 + offset, 1);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x83 });
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.atomic_rmws, 0);
    assert!(memory.writes.is_empty());

    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = DATA;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_user(DATA + offset, 1);
    }
    let result = od_isa::execute::run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        || false,
    );
    assert!(matches!(result.exit, RunExit::Unsupported { .. }));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert_eq!(memory.atomic_rmws, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_lock_add_qword_gives_range_fault_priority_over_alignment_refusal() {
    let data_code = [0xf0, 0x48, 0x83, 0x03, 0x01];
    let stack_code = [0xf0, 0x48, 0x83, 0x04, 0x24, 0x01];
    for (code, base, address, vector) in [
        (
            data_code.as_slice(),
            Register::Rbx,
            0x0000_8000_0000_0000,
            13,
        ),
        (
            data_code.as_slice(),
            Register::Rbx,
            0x0000_7fff_ffff_fffc,
            13,
        ),
        (
            stack_code.as_slice(),
            Register::Rsp,
            0x0000_7fff_ffff_fffc,
            12,
        ),
    ] {
        let mut state = user_state();
        state.gpr[base as usize] = address;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert_eq!(memory.atomic_rmws, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_glibc_rseq_sse2_alias_ops_use_the_vcpu_fp_cell() {
    let code = [
        0x66, 0x0f, 0x60, 0xc0, // punpcklbw xmm0, xmm0
        0x66, 0x0f, 0x61, 0xc0, // punpcklwd xmm0, xmm0
        0x66, 0x0f, 0x70, 0xc0, 0x00, // pshufd xmm0, xmm0, 0
    ];
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.rflags |= ARITH;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[0] = std::array::from_fn(|index| 0x10 + index as u8);
    let fp = std::cell::RefCell::new(initial_fp);
    let mut memory = DataMemory::new(&code);

    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 3);

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(fp.borrow().sse.xmm[0], [0x10; 16]);
    assert_eq!(state.rflags & ARITH, ARITH);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_glibc_rseq_sse2_alias_ops_require_fp_state_and_controls() {
    let code = [0x66, 0x0f, 0x60, 0xc0];
    let mut state = user_state();
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert!(matches!(
        exit(&result),
        RunExit::Unsupported { opcode: 0x60 }
    ));
    assert_eq!(state, before);

    let mut bare_state = user_state();
    let bare_before = bare_state.clone();
    let mut bare_memory = DataMemory::new(&code);
    let bare_result = od_isa::execute::run(
        &mut bare_state,
        &mut bare_memory,
        config(PrivilegeLevel::Ring3, 1),
        || false,
    );
    assert!(matches!(bare_result.exit, RunExit::Unsupported { .. }));
    assert_eq!(bare_result.instructions_executed, 0);
    assert_eq!(bare_state, bare_before);
    assert_eq!(bare_memory.reads, 0);
    assert_eq!(bare_memory.reads, 0);

    for (cr0, cr4, vector) in [(0x8005_0033, 0x20, 6), (0x8005_003b, 0x6b0, 7)] {
        let mut state = user_state();
        state.cr0 = Some(cr0);
        state.cr4 = Some(cr4);
        let before = state.clone();
        let fp = std::cell::RefCell::new(od_isa::GuestFpState::initial());
        let mut memory = DataMemory::new(&code);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(state, before);
        assert_eq!(*fp.borrow(), od_isa::GuestFpState::initial());
    }
}

#[test]
fn cpl3_issue575_reference_vector_register_subset_uses_guest_fp_state() {
    let run = |code: &[u8], initial_fp: od_isa::GuestFpState| {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.rflags |= ARITH;
        let original_state = state.clone();
        let fp = std::cell::RefCell::new(initial_fp);
        let mut memory = DataMemory::new(code);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, original_state.rip + code.len() as u64);
        assert_eq!(state.rflags, original_state.rflags);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
        (state, fp.into_inner())
    };

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[0] = std::array::from_fn(|i| i as u8);
    initial.sse.xmm[1] = initial.sse.xmm[0];
    initial.sse.xmm[1][5] ^= 0xff;
    let (state, fp) = run(&[0x66, 0x0f, 0x74, 0xc1], initial); // PCMPEQB xmm0, xmm1
    let expected = std::array::from_fn(|i| if i == 5 { 0 } else { 0xff });
    assert_eq!(fp.sse.xmm[0], expected);
    assert_eq!(state.gpr, user_state().gpr);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[2] = [0; 16];
    initial.sse.xmm[0] = [1; 16];
    let (_, fp) = run(&[0x66, 0x0f, 0xf8, 0xd0], initial); // PSUBB xmm2, xmm0
    assert_eq!(fp.sse.xmm[2], [0xff; 16]);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[4] = std::array::from_fn(|i| if matches!(i, 0 | 2 | 15) { 0x80 } else { 0x7f });
    let (state, _) = run(&[0x66, 0x0f, 0xd7, 0xd4], initial); // PMOVMSKB edx, xmm4
    assert_eq!(state.gpr[Register::Rdx as usize], 0x8005);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[2] = std::array::from_fn(|i| i as u8);
    initial.sse.xmm[3] = [0x80; 16];
    let (_, fp) = run(&[0x66, 0x0f, 0xeb, 0xd3], initial); // POR xmm2, xmm3
    assert_eq!(fp.sse.xmm[2], std::array::from_fn(|i| (i as u8) | 0x80));

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[2] = std::array::from_fn(|i| i as u8);
    let (_, fp) = run(&[0x66, 0x0f, 0x73, 0xfa, 0x02], initial); // PSLLDQ xmm2, 2
    assert_eq!(
        fp.sse.xmm[2],
        [0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
    );

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[3] = std::array::from_fn(|i| i as u8);
    let (_, fp) = run(&[0x66, 0x0f, 0x73, 0xdb, 0x0e], initial); // PSRLDQ xmm3, 14
    assert_eq!(
        fp.sse.xmm[3],
        [14, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[0] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 5, 5, 5, 5, 5, 5, 5, 5,
    ];
    initial.sse.xmm[1] = [1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0];
    let (_, fp) = run(&[0x66, 0x0f, 0xd4, 0xc1], initial); // PADDQ xmm0, xmm1
    assert_eq!(
        fp.sse.xmm[0],
        [0, 0, 0, 0, 0, 0, 0, 0, 12, 5, 5, 5, 5, 5, 5, 5]
    );

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[2] =
        std::array::from_fn(|i| if matches!(i, 0 | 7 | 8 | 15) { 0x80 } else { 0 });
    let (state, _) = run(&[0x66, 0x44, 0x0f, 0xd7, 0xca], initial); // PMOVMSKB r9d, xmm2
    assert_eq!(state.gpr[Register::R9 as usize], 0x8181);
}

#[test]
fn cpl3_s6_22_vector_memory_loads_preserve_lanes_and_validate_the_full_access_before_commit() {
    for (code, width, offset, high, destination) in [
        (&[0x66, 0x0f, 0x74, 0x0f][..], 16usize, 0u64, false, 1usize),
        (&[0x66, 0x0f, 0x12, 0x0f][..], 8, 0, false, 1),
        (&[0x66, 0x0f, 0x16, 0x4f, 0x08][..], 8, 8, true, 1),
        (&[0x0f, 0x16, 0x40, 0x08][..], 8, 8, true, 0),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.gpr[Register::Rdi as usize] = DATA;
        state.gpr[Register::Rax as usize] = DATA;
        let mut initial_fp = od_isa::GuestFpState::initial();
        initial_fp.sse.xmm[destination] = [0xa5; 16];
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(code);
        for i in 0..24 {
            memory = memory.with_user(DATA + i, (0x10 + i) as u8);
        }
        let mut expected = [0xa5; 16];
        if code[2] == 0x74 {
            for i in 0..16 {
                expected[i] = if 0xa5 == (0x10 + i) as u8 { 0xff } else { 0 };
            }
        } else if high {
            expected[8..].copy_from_slice(
                &(0..8)
                    .map(|i| (0x10 + offset + i) as u8)
                    .collect::<Vec<_>>(),
            );
        } else {
            expected[..8].copy_from_slice(
                &(0..8)
                    .map(|i| (0x10 + offset + i) as u8)
                    .collect::<Vec<_>>(),
            );
        }
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(fp.borrow().sse.xmm[destination], expected);
        assert_eq!(memory.read_widths, vec![(DATA + offset, width)]);
        assert!(memory.writes.is_empty());
    }

    // REX.R reaches XMM9, and REX.B reaches R8 as the memory base.
    let code = [0x66, 0x45, 0x0f, 0x12, 0x08]; // MOVLPD xmm9, [r8]
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::R8 as usize] = DATA;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[9] = [0x5a; 16];
    let fp = std::cell::RefCell::new(initial_fp);
    let mut memory = DataMemory::new(&code);
    for i in 0..8 {
        memory = memory.with_user(DATA + i, (0xc0 + i) as u8);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(
        &fp.borrow().sse.xmm[9][..8],
        &[0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7]
    );
    assert_eq!(&fp.borrow().sse.xmm[9][8..], &[0x5a; 8]);

    // Legacy PCMPEQB requires m128 alignment before any paging or data access.
    let code = [0x66, 0x0f, 0x74, 0x0f];
    for (address, mapped, vector) in [
        (DATA + 1, true, 13),
        (DATA + 0xff8, false, 13),
        (DATA + 0x1000, false, 14),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.gpr[Register::Rdi as usize] = address;
        let before = state.clone();
        let mut initial_fp = od_isa::GuestFpState::initial();
        initial_fp.sse.xmm[1] = [0x7c; 16];
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(&code).with_absent(DATA + 0x1000);
        if mapped {
            for offset in 0..16 {
                memory = memory.with_user(address + offset, 0x7c);
            }
        }
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        let page_fault = vector == 14;
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: Some(if page_fault { 4 } else { 0 }),
                linear_address: page_fault.then_some(address),
            })
        );
        assert_eq!(state, before);
        assert_eq!(*fp.borrow(), initial_fp);
        if page_fault {
            assert_eq!(memory.read_widths, [(address, 16)]);
        } else {
            assert!(memory.read_widths.is_empty());
        }
    }

    // Canonical48 validates every byte before the backend can observe a read.
    let code = [0x66, 0x0f, 0x12, 0x0f]; // MOVLPD permits an unaligned m64.
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rdi as usize] = 0x0000_7fff_ffff_fffc;
    let before = state.clone();
    let fp = std::cell::RefCell::new(od_isa::GuestFpState::initial());
    let mut memory = DataMemory::new(&code);
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_issue575_vector_gates_and_neighboring_prefixes_stay_bounded() {
    let code = [0x66, 0x0f, 0x74, 0xc1]; // PCMPEQB register
    for (cr0, cr4, vector) in [
        (0x8005_0033 | (1 << 2), 0x6b0, 6),
        (0x8005_0033 | (1 << 3), 0x6b0, 7),
        (0x8005_0033, 0x6b0 & !(1 << 9), 6),
    ] {
        let mut state = user_state();
        state.cr0 = Some(cr0);
        state.cr4 = Some(cr4);
        let before = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(&code);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
    }

    for (bytes, opcode) in [
        (&[0x0f, 0x74, 0xc1][..], 0x74),
        (&[0xf3, 0x66, 0x0f, 0x74, 0xc1][..], 0x74),
        (&[0x0f, 0xd7, 0xd4][..], 0xd7),
        (&[0xf3, 0x66, 0x0f, 0xd4, 0xc1][..], 0xd4),
        (&[0x66, 0x0f, 0x73, 0xea, 0x02][..], 0x73),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        let before = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(bytes);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
    }
}

#[test]
fn cpl3_tunables_imul_uses_r10_and_flags_signed_overflow() {
    for value in [-3i64, i64::MAX] {
        let mut state = user_state();
        state.gpr[Register::R10 as usize] = value as u64;
        state.rflags |= CF | OF;
        let mut memory = DataMemory::new(&[0x4d, 0x69, 0xd2, 0x88, 0, 0, 0]);
        let (product, overflow) = value.overflowing_mul(0x88);

        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[Register::R10 as usize], product as u64);
        assert_eq!(state.rflags & (CF | OF), if overflow { CF | OF } else { 0 });
        assert_eq!(state.rip, RIP + 7);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_jmp_rel32_uses_signed_displacement_and_preserves_flags() {
    let mut state = user_state();
    state.rflags |= ARITH;
    let expected = state.clone();
    let mut memory = DataMemory::new(&[0xe9, 0xfb, 0xff, 0xff, 0xff]);

    let result = run3_data(&mut state, &mut memory, 4).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_cpuid_reports_the_selected_vcpu_topology() {
    use od_isa::cpuid::CpuidProfile;
    let profile = CpuidProfile::X8664V1Stage4AApic;
    for apic_id in 0..2u8 {
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&profile)
            .with_cpuid_topology(apic_id, 2);
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = 0x1234_5678_0000_0001;
        state.gpr[Register::Rcx as usize] = 0x9876_5432_0000_0000;
        state.rflags |= ARITH;
        let mut expected = state.clone();
        let outputs = profile.query_for_vcpu(1, 0, u32::from(apic_id), 2);
        for (register, value) in [
            (Register::Rax, outputs.eax),
            (Register::Rbx, outputs.ebx),
            (Register::Rcx, outputs.ecx),
            (Register::Rdx, outputs.edx),
        ] {
            expected.gpr[register as usize] = u64::from(value);
        }
        expected.rip += 2;
        let mut memory = Memory::code(&[0x0f, 0xa2]);

        let result = run3(&mut state, &mut memory, &set, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state, expected);
        assert_eq!(memory.accesses, 0);
    }
}

#[test]
fn cpl3_cpu_detection_dword_or_cannot_read_supervisor_memory() {
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0x7654;
    let expected = state.clone();
    let mut memory = DataMemory::new(&[0x0b, 0x07]);
    for offset in 0..4 {
        memory = memory.with_supervisor(DATA + offset, 0);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert!(
        matches!(
            exit(&result),
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(5),
                linear_address: Some(DATA),
            })
        ),
        "{:?}",
        exit(&result)
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, expected);
    assert!(memory.writes.is_empty());
}

fn run3_with_guest_fp(
    state: &mut LongModeState,
    memory: &mut DataMemory,
    fp: &std::cell::RefCell<od_isa::GuestFpState>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    let profile = od_isa::cpuid::CpuidProfile::X8664V1Stage4AApic;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&profile)
        .with_fp_state(fp);
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap()
}

#[test]
fn cpl3_glibc_xmm_transfers_use_the_vcpus_fp_cell() {
    let code = [
        0x66, 0x0f, 0x6e, 0x45, 0x94, // movd xmm0, [rbp - 0x6c]
        0x66, 0x0f, 0x6e, 0xca, // movd xmm1, edx
        0x66, 0x0f, 0x62, 0xc8, // punpckldq xmm1, xmm0
        0x66, 0x0f, 0xd6, 0x0f, // movq [rdi], xmm1
    ];
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rbp as usize] = DATA + 0x6c;
    state.gpr[Register::Rdx as usize] = 0xdead_beef_a1b2_c3d4;
    state.gpr[Register::Rdi as usize] = DATA + 16;
    state.rflags |= ARITH;
    let mut expected = state.clone();
    expected.rip += code.len() as u64;

    let mut original_fp = od_isa::GuestFpState::initial();
    original_fp.sse.xmm[0] = [0xaa; 16];
    original_fp.sse.xmm[1] = [0xbb; 16];
    original_fp.sse.xmm[2] = [0xcc; 16];
    let fp = std::cell::RefCell::new(original_fp.clone());
    let mut expected_fp = original_fp;
    expected_fp.sse.xmm[0] = [0x44, 0x33, 0x22, 0x11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    expected_fp.sse.xmm[1] = [
        0xd4, 0xc3, 0xb2, 0xa1, 0x44, 0x33, 0x22, 0x11, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 0x1122_3344u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + offset as u64, byte);
    }
    for address in DATA + 16..DATA + 24 {
        memory = memory.with_user(address, 0xee);
    }

    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 4);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state, expected);
    assert_eq!(*fp.borrow(), expected_fp);
    assert_eq!(memory.reads, 1);
    assert_eq!(
        memory.writes,
        (DATA + 16..DATA + 24)
            .zip(expected_fp.sse.xmm[1][..8].iter().copied())
            .collect::<Vec<_>>()
    );
}

#[test]
fn cpl3_glibc_xmm_move_faults_preserve_cpu_and_fp_state() {
    for (code, error_code) in [
        (&[0x66, 0x0f, 0x6e, 0x07][..], 5),
        (&[0x66, 0x0f, 0xd6, 0x07][..], 7),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.gpr[Register::Rdi as usize] = DATA;
        let expected = state.clone();
        let mut initial_fp = od_isa::GuestFpState::initial();
        initial_fp.sse.xmm[0] = [0xa5; 16];
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_supervisor(DATA + offset, 0x55);
        }
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
                linear_address: Some(DATA),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, expected);
        assert_eq!(*fp.borrow(), initial_fp);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_glibc_xmm_controls_fault_before_touching_guest_memory() {
    let cr0 = 0x8005_0033;
    let cr4 = 0x6b0;
    for (cr0, cr4, vector) in [
        (cr0 | (1 << 2), cr4, 6),
        (cr0 | (1 << 3), cr4, 7),
        (cr0, cr4 & !(1 << 9), 6),
    ] {
        let mut state = user_state();
        state.cr0 = Some(cr0);
        state.cr4 = Some(cr4);
        state.gpr[Register::Rdi as usize] = DATA;
        let expected = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(&[0x66, 0x0f, 0x6e, 0x07]);

        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, expected);
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_glibc_xmm_keeps_neighbouring_encodings_refused() {
    for (code, opcode) in [
        (&[0x0f, 0x6e, 0xc0][..], 0x6e),
        (&[0xf2, 0x66, 0x48, 0x0f, 0x6e, 0xc0][..], 0x6e),
        (&[0x67, 0x66, 0x0f, 0x6e, 0x07][..], 0x6e),
        (&[0x66, 0x0f, 0xd6, 0xc0][..], 0xd6),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        let expected = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(code);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, expected);
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_cmove_qword_reads_memory_for_both_condition_results() {
    let code = [0x48, 0x0f, 0x44, 0x95, 0x78, 0xff, 0xff, 0xff];
    let value = 0xfedc_ba98_7654_3210u64;
    for taken in [false, true] {
        let mut state = user_state();
        state.gpr[Register::Rbp as usize] = DATA + 0x88;
        state.gpr[Register::Rdx as usize] = 0x1122_3344_5566_7788;
        state.rflags = (state.rflags & !ZF) | if taken { ZF } else { 0 };
        let mut expected = state.clone();
        expected.rip += code.len() as u64;
        if taken {
            expected.gpr[Register::Rdx as usize] = value;
        }
        let mut memory = DataMemory::new(&code);
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            memory = memory.with_user(DATA + offset as u64, byte);
        }

        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(state, expected);
        assert_eq!(memory.reads, 1);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_cmove_false_condition_still_checks_user_page_permissions() {
    let mut state = user_state();
    state.gpr[Register::Rbp as usize] = DATA + 0x88;
    state.gpr[Register::Rdx as usize] = 0x1122_3344_5566_7788;
    state.rflags &= !ZF;
    let expected = state.clone();
    let mut memory = DataMemory::new(&[0x48, 0x0f, 0x44, 0x95, 0x78, 0xff, 0xff, 0xff]);
    for offset in 0..8 {
        memory = memory.with_supervisor(DATA + offset, 0x55);
    }

    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, expected);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_glibc_tls_packed_transfers_preserve_lane_order_and_width() {
    let code = [
        0xf3, 0x0f, 0x7e, 0x06, // movq xmm0, [rsi]
        0x66, 0x48, 0x0f, 0x6e, 0xc8, // movq xmm1, rax
        0x66, 0x0f, 0xdb, 0xc1, // pand xmm0, xmm1
        0x66, 0x0f, 0x6c, 0xc0, // punpcklqdq xmm0, xmm0
        0x0f, 0x11, 0x07, // movups [rdi], xmm0
        0xf3, 0x0f, 0x6f, 0x17, // movdqu xmm2, [rdi]
    ];
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.rflags |= ARITH;
    state.gpr[Register::Rsi as usize] = DATA;
    state.gpr[Register::Rdi as usize] = DATA + 0x40;
    let source = 0xfedc_ba98_7654_3210u64;
    let mask = 0x0f0f_f0f0_55aa_55aau64;
    state.gpr[Register::Rax as usize] = mask;
    let mut expected = state.clone();
    expected.rip += code.len() as u64;

    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[0] = [0xaa; 16];
    initial_fp.sse.xmm[1] = [0xbb; 16];
    let fp = std::cell::RefCell::new(initial_fp.clone());
    let mut expected_fp = initial_fp;
    let lane = (source & mask).to_le_bytes();
    for register in [0, 2] {
        expected_fp.sse.xmm[register][..8].copy_from_slice(&lane);
        expected_fp.sse.xmm[register][8..].copy_from_slice(&lane);
    }
    expected_fp.sse.xmm[1][..8].copy_from_slice(&mask.to_le_bytes());
    expected_fp.sse.xmm[1][8..].fill(0);

    let mut memory = DataMemory::new(&code);
    for (offset, byte) in source.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + offset as u64, byte);
    }
    for address in DATA + 0x40..DATA + 0x50 {
        memory = memory.with_user(address, 0xee);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 6);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 6);
    assert_eq!(state, expected);
    assert_eq!(*fp.borrow(), expected_fp);
    assert_eq!(memory.reads, 2);
    assert_eq!(
        memory.writes,
        (DATA + 0x40..DATA + 0x50)
            .zip(expected_fp.sse.xmm[0])
            .collect::<Vec<_>>()
    );
}

#[test]
fn cpl3_glibc_tls_vector_fault_checks_the_full_sixteen_byte_range() {
    let address = ((DATA + 0x1000) & !0xfff) - 8;
    for (code, error_code) in [
        (&[0xf3, 0x0f, 0x6f, 0x07][..], 5),
        (&[0x0f, 0x11, 0x07][..], 7),
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.gpr[Register::Rdi as usize] = address;
        let expected = state.clone();
        let mut initial_fp = od_isa::GuestFpState::initial();
        initial_fp.sse.xmm[0] = [0xa5; 16];
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory
                .with_user(address + offset, 0x11)
                .with_supervisor(address + 8 + offset, 0x22);
        }

        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
                linear_address: Some(address + 8),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, expected);
        assert_eq!(*fp.borrow(), initial_fp);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_indirect_call_checks_user_memory_before_pushing_return_address() {
    let code = [0x41, 0xff, 0x57, 0x10];
    let stack = DATA + 0x30;
    let mut initial = user_state();
    initial.gpr[Register::R15 as usize] = DATA - 0x10;
    initial.gpr[Register::Rsp as usize] = stack + 8;
    let target = 0x0000_1234_5678_9000u64;

    for supervisor in [false, true] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(&code);
        for (offset, byte) in target.to_le_bytes().into_iter().enumerate() {
            let address = DATA + offset as u64;
            memory = if supervisor {
                memory.with_supervisor(address, byte)
            } else {
                memory.with_user(address, byte)
            };
        }
        for address in stack..stack + 8 {
            memory = memory.with_user(address, 0);
        }

        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        if supervisor {
            assert_eq!(
                exit(&result),
                &RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(DATA),
                })
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, initial);
            assert!(memory.writes.is_empty());
        } else {
            assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, 1);
            let mut expected = initial.clone();
            expected.rip = target;
            expected.gpr[Register::Rsp as usize] = stack;
            assert_eq!(state, expected);
            assert_eq!(memory.reads, 1);
            assert_eq!(
                memory.writes,
                (stack..stack + 8)
                    .zip((RIP + 4).to_le_bytes())
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn cpl3_issue571_scalar_glibc_forms_are_bounded() {
    // CMP dword [rdi+8], eax: one read, no store, dword flags and full RAX preserved.
    let code = [0x39, 0x47, 0x08];
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0xfeed_face_8000_0000;
    state.gpr[Register::Rbx as usize] = 0x1122_3344_5566_7788;
    state.rflags |= ARITH;
    let preserved_flags = state.rflags & !ARITH;
    let mut memory = DataMemory::new(&code);
    for (i, b) in 0x7fff_ffffu32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + 8 + i as u64, b);
    }
    let before_rax = state.gpr[Register::Rax as usize];
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 3);
    assert_eq!(state.gpr[Register::Rax as usize], before_rax);
    assert_eq!(state.gpr[Register::Rbx as usize], 0x1122_3344_5566_7788);
    assert_eq!(state.rflags, preserved_flags | CF | PF | SF | OF);
    assert!(memory.writes.is_empty());
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_widths, [(DATA + 8, 4)]);

    // ADD dword [rdi+12], 1; the upper bytes of RDI are used for addressing.
    let code = [0x83, 0x47, 0x0c, 0x01];
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = 0x0000_7fff_ffff_0000;
    let address = state.gpr[Register::Rdi as usize] + 12;
    let mut memory = DataMemory::new(&code);
    for (i, b) in 0xffff_ffffu32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(address + i as u64, b);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, RIP + 4);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_widths, [(address, 4)]);
    assert_eq!(
        memory.writes,
        (address..address + 4).map(|a| (a, 0)).collect::<Vec<_>>()
    );
    assert_ne!(state.rflags & ZF, 0);

    // SHL RAX, 4: the qword form zeroes no unrelated registers and preserves the source's full width.
    let code = [0x48, 0xc1, 0xe0, 0x04];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x0123_4567_89ab_cdef;
    state.gpr[Register::Rbx as usize] = 0xfedc_ba98_7654_3210;
    let result = run3_data(&mut state, &mut DataMemory::new(&code), 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678_9abc_def0);
    assert_eq!(state.gpr[Register::Rbx as usize], 0xfedc_ba98_7654_3210);
}

#[test]
fn cpl3_issue571_memory_add_uses_write_fault_before_read_or_state_commit() {
    let code = [0x83, 0x47, 0x0c, 0x01]; // ADD dword [rdi+12], 1
    let address = DATA + 12;
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for (i, byte) in 0x1234_5678u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_supervisor(address + i as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7), // present, write, user
            linear_address: Some(address),
        })
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_widths, [(address, 4)]);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_issue571_movdqa_uses_aligned_memory_and_guest_fp_state() {
    // Pinned BusyBox bytes at 0x4398e9: MOVDQA xmm1, [RIP+0x19427f].
    let code = [0x66, 0x0f, 0x6f, 0x0d, 0x7f, 0x42, 0x19, 0x00];
    let target = 0x005c_db70;
    let mut state = user_state();
    state.rip = 0x0043_98e9;
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let mut expected = state.clone();
    expected.rip += code.len() as u64;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[1] = [0xa5; 16];
    let fp = std::cell::RefCell::new(initial_fp.clone());
    let value = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00,
    ];
    let mut memory = DataMemory::new(&code);
    memory.code_base = state.rip;
    for (i, b) in value.into_iter().enumerate() {
        memory = memory.with_user(target + i as u64, b);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state, expected);
    assert_eq!(fp.borrow().sse.xmm[1], value);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_widths, [(target, 16)]);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_issue571_movdqa_alignment_fault_precedes_memory_read() {
    let code = [0x66, 0x0f, 0x6f, 0x0f]; // MOVDQA xmm1, [rdi]
    for address in [DATA + 1, u64::MAX - 7] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        state.gpr[Register::Rdi as usize] = address;
        let before = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(&code);
        if address == DATA + 1 {
            for offset in 0..16 {
                memory = memory.with_user(address + offset, 0x55);
            }
        }
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
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
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_issue571_neighboring_6f_and_7e_spellings_are_typed_unsupported() {
    for code in [
        &[0x0f, 0x6f, 0x07][..],       // unprefixed MOVQ form
        &[0xf2, 0x0f, 0x6f, 0xc0][..], // unsupported register prefix
        &[0x66, 0x0f, 0x7e, 0x07][..], // legacy MOVD/MOVQ store direction
        &[0x0f, 0x7e, 0x07][..],
    ] {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        let before = state.clone();
        let initial_fp = od_isa::GuestFpState::initial();
        let fp = std::cell::RefCell::new(initial_fp.clone());
        let mut memory = DataMemory::new(code);
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported {
                opcode: if code.contains(&0x6f) { 0x6f } else { 0x7e }
            }
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(*fp.borrow(), initial_fp);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_issue571_cmp_dword_memory_fault_preserves_flags_and_rip() {
    let code = [0x39, 0x47, 0x08];
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0xdead_beef_8000_0000;
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..4 {
        memory = memory.with_supervisor(DATA + 8 + offset, 0x55);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA + 8),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.reads, 1);
    assert_eq!(memory.read_widths, [(DATA + 8, 4)]);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_issue573_observed_scalar_forms_use_expected_operands_and_widths() {
    let code = [
        0x48, 0x2b, 0x4f, 0x08, // sub rcx, qword [rdi+8]
        0x80, 0x67, 0x10, 0x0f, // and byte [rdi+0x10], 0x0f
        0x83, 0x6f, 0x14, 0x01, // sub dword [rdi+0x14], 1
        0x30, 0xd8, // xor al, bl
    ];
    let mut state = user_state();
    state.gpr[Register::Rdi as usize] = DATA;
    state.gpr[Register::Rcx as usize] = 0x20;
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_0077;
    state.gpr[Register::Rbx as usize] = 0x5a;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 0x12u64.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + 8 + offset as u64, byte);
    }
    memory = memory.with_user(DATA + 0x10, 0xf3);
    for (offset, byte) in 0x1000u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + 0x14 + offset as u64, byte);
    }

    let result = run3_data(&mut state, &mut memory, 4).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x0e);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_002d);
    assert_eq!(state.gpr[Register::Rbx as usize], 0x5a);
    assert_eq!(
        memory.read_widths,
        [(DATA + 8, 8), (DATA + 0x10, 1), (DATA + 0x14, 4)]
    );
    assert_eq!(
        memory.writes,
        [
            (DATA + 0x10, 0x03),
            (DATA + 0x14, 0xff),
            (DATA + 0x15, 0x0f),
            (DATA + 0x16, 0),
            (DATA + 0x17, 0)
        ]
    );
}

#[test]
fn cpl3_issue573_memory_updates_validate_write_before_read() {
    for (code, offset, width) in [
        (&[0x80, 0x67, 0x10, 0x0f][..], 0x10, 1),
        (&[0x83, 0x6f, 0x14, 0x01][..], 0x14, 4),
    ] {
        let address = DATA + offset;
        let mut state = user_state();
        state.gpr[Register::Rdi as usize] = DATA;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for byte in address..address + width {
            memory = memory.with_supervisor(byte, 0x55);
        }

        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(address),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.read_widths, [(address, width as usize)]);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_issue573_tls_tail_adds_imm32_to_qword_accumulator() {
    // Pinned BusyBox at 0x4f68f9, between the FS load and final FS store.
    let code = [0x48, 0x05, 0x00, 0x02, 0x00, 0x00];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = u64::MAX - 0x1ff;
    state.rflags |= ARITH;
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rax as usize], 0);
    assert_eq!(state.rflags & ARITH, CF | PF | ZF);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_busybox_and_library_rmw_forms_validate_the_write_first() {
    const RIP_ADD_ADDRESS: u64 = RIP + 7 + 0x20_9767;
    for (code, address, width, rax, expected) in [
        (&[0x01, 0x06][..], DATA, 4, 3, 8),
        (
            &[0x48, 0x01, 0x0d, 0x67, 0x97, 0x20, 0x00][..],
            RIP_ADD_ADDRESS,
            8,
            0,
            0x1_0000_0008u64,
        ),
        (&[0x48, 0x83, 0x45, 0x10, 0x01][..], DATA + 0x10, 8, 0, 6),
        (&[0x48, 0x83, 0x20, 0x00][..], DATA, 8, DATA, 0),
        (&[0x48, 0x29, 0x0e][..], DATA, 8, 0, 0xffff_ffff_0000_0002),
        (&[0x29, 0x0e][..], DATA, 4, 0, 2),
        (&[0x66, 0x83, 0x06, 1][..], DATA, 2, 0, 6),
        (&[0xfe, 0x06][..], DATA, 1, 0, 6),
        (&[0xff, 0x06][..], DATA, 4, 0, 6),
        (&[0x48, 0xff, 0x06][..], DATA, 8, 0, 6),
        (&[0xff, 0x0e][..], DATA, 4, 0, 4),
        (&[0x20, 0x0e][..], DATA, 1, 0, 1),
        (&[0x66, 0x81, 0x26, 0, 0xfb][..], DATA, 2, 0, 0),
        (&[0x66, 0x83, 0x26, 0][..], DATA, 2, 0, 0),
        (&[0x81, 0x36, 0, 1, 0, 0][..], DATA, 4, 0, 0x105),
        (&[0xd0, 0x3e][..], DATA, 1, 0, 2),
    ] {
        for protected in [false, true] {
            let mut state = user_state();
            state.gpr[Register::Rax as usize] = rax;
            state.gpr[Register::Rcx as usize] = 0x1_0000_0003;
            state.gpr[Register::Rsi as usize] = DATA;
            state.gpr[Register::Rbp as usize] = DATA;
            let before = state.clone();
            let mut memory = DataMemory::new(code);
            for (offset, &byte) in 5u64.to_le_bytes()[..width].iter().enumerate() {
                let at = address + offset as u64;
                memory = if protected {
                    memory.with_supervisor(at, byte)
                } else {
                    memory.with_user(at, byte)
                };
            }
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(memory.read_widths, [(address, width)]);
            if protected {
                assert_eq!(
                    exit(&result),
                    &RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(7),
                        linear_address: Some(address),
                    })
                );
                assert_eq!(result.instructions_executed, 0);
                assert_eq!(state, before);
                assert!(memory.writes.is_empty());
            } else {
                assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
                assert_eq!(state.rip, RIP + code.len() as u64);
                assert_eq!(state.gpr, before.gpr);
                let writes: Vec<_> = expected.to_le_bytes()[..width]
                    .iter()
                    .enumerate()
                    .map(|(i, &byte)| (address + i as u64, byte))
                    .collect();
                assert_eq!(memory.writes, writes);
            }
        }
    }
}

#[test]
fn cpl3_workload_qword_or_forms_keep_write_intent_for_stack_and_indexed_memory() {
    for (code, address, expected) in [
        (&[0x48, 0x83, 0x0c, 0x24, 0x00][..], DATA, 0x10u64),
        (&[0x48, 0x09, 0x34, 0xc2][..], DATA + 8, 0x13),
    ] {
        let mut initial = user_state();
        initial.gpr[Register::Rsp as usize] = DATA;
        initial.gpr[Register::Rdx as usize] = DATA;
        initial.gpr[Register::Rax as usize] = 1;
        initial.gpr[Register::Rsi as usize] = 3;
        initial.rflags |= ARITH;
        for (protected, expected_exit) in [
            (false, RunExit::InstructionBudgetExhausted),
            (
                true,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    linear_address: Some(address),
                }),
            ),
        ] {
            let mut state = initial.clone();
            let mut memory = DataMemory::new(code);
            for (i, byte) in 0x10u64.to_le_bytes().into_iter().enumerate() {
                let byte_address = address + i as u64;
                memory = if protected {
                    memory.with_supervisor(byte_address, byte)
                } else {
                    memory.with_user(byte_address, byte)
                };
            }
            let result = run3_data(&mut state, &mut memory, 1).unwrap();
            assert_eq!(exit(&result), &expected_exit);
            assert_eq!(memory.read_widths, [(address, 8)]);
            if protected {
                assert_eq!(state, initial);
                assert!(memory.writes.is_empty());
            } else {
                assert_eq!(state.rip, RIP + code.len() as u64);
                assert_eq!(state.gpr, initial.gpr);
                assert_eq!(state.rflags & (CF | OF | ZF), 0);
                assert_eq!(
                    memory.writes,
                    expected
                        .to_le_bytes()
                        .into_iter()
                        .enumerate()
                        .map(|(i, byte)| (address + i as u64, byte))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

#[test]
fn cpl3_busybox_push_imm8_and_sete_use_the_right_stack_widths() {
    let code = [0x6a, 0xfd, 0x0f, 0x94, 0x45, 0x90];
    let mut state = user_state();
    let push_address = DATA;
    let byte_address = DATA + 0x80;
    state.gpr[Register::Rsp as usize] = push_address + 8;
    state.gpr[Register::Rbp as usize] = byte_address + 0x70;
    state.rflags |= ZF;
    let flags = state.rflags;
    let mut memory = DataMemory::new(&code).with_user(byte_address, 0xcc);
    for offset in 0..8 {
        memory = memory.with_user(push_address + offset, 0);
    }
    let done = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rsp as usize], push_address);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.rflags, flags);
    let mut expected = Vec::new();
    for (offset, byte) in (-3i64).to_le_bytes().into_iter().enumerate() {
        expected.push((push_address + offset as u64, byte));
    }
    expected.push((byte_address, 1));
    assert_eq!(memory.writes, expected);
    assert!(memory.read_widths.is_empty());
}

#[test]
fn cpl3_busybox_cqo_and_idiv_use_a_signed_double_width_dividend() {
    let code = [0x48, 0x99, 0x48, 0xf7, 0xf9];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = -13i64 as u64;
    state.gpr[Register::Rcx as usize] = 5;
    let mut memory = DataMemory::new(&code);
    let done = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rax as usize], -2i64 as u64);
    assert_eq!(state.gpr[Register::Rdx as usize], -3i64 as u64);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_reference_implicit_multiply_and_divide_keep_full_results() {
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = u64::MAX;
    state.gpr[Register::Rsi as usize] = 2;
    let mut memory = DataMemory::new(&[0x48, 0xf7, 0xe6]); // MUL RSI
    let done = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rax as usize], u64::MAX - 1);
    assert_eq!(state.gpr[Register::Rdx as usize], 1);
    assert_eq!(state.rflags & (CF | OF), CF | OF);

    for divisor in [0, 5] {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = 0x1111_0000_0000_000d;
        state.gpr[Register::Rdx as usize] = 0x2222_0000_0000_0000;
        state.gpr[Register::Rdi as usize] = divisor;
        let before = state.clone();
        let mut memory = DataMemory::new(&[0xf7, 0xf7]); // DIV EDI
        let done = run3_data(&mut state, &mut memory, 1).unwrap();
        if divisor == 0 {
            assert_eq!(
                exit(&done),
                &RunExit::Fault(ArchitecturalFault {
                    vector: 0,
                    error_code: None,
                    linear_address: None,
                })
            );
            assert_eq!(state, before);
        } else {
            assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
            assert_eq!(state.gpr[Register::Rax as usize], 2);
            assert_eq!(state.gpr[Register::Rdx as usize], 3);
        }
        assert!(memory.read_widths.is_empty());
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_reference_word_compare_and_fs_xor_read_without_writing() {
    let mut state = user_state();
    state.fs_base = DATA;
    state.gpr[Register::Rdx as usize] = DATA;
    let code = [
        0x66, 0x83, 0x7a, 0x06, 0xf1, // CMP word [RDX+6], -15
        0x64, 0x48, 0x33, 0x14, 0x25, 0x30, 0, 0, 0, // XOR RDX, FS:[0x30]
    ];
    let value = 0x1122_3344_5566_7788u64;
    let mut memory = DataMemory::new(&code)
        .with_user(DATA + 6, 0xf1)
        .with_user(DATA + 7, 0xff);
    for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + 0x30 + i as u64, byte);
    }
    let compare = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&compare), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rflags & ARITH, ZF | PF);
    assert_eq!(state.gpr[Register::Rdx as usize], DATA);

    let xor = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&xor), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rdx as usize], DATA ^ value);
    assert_eq!(memory.read_widths, [(DATA + 6, 2), (DATA + 0x30, 8)]);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_reference_fs_compares_use_the_exact_memory_width() {
    for (code, address, width) in [
        (
            &[0x64, 0x80, 0x3c, 0x25, 0xc8, 0xff, 0xff, 0xff, 0][..],
            DATA + 8,
            1,
        ),
        (
            &[0x64, 0x48, 0x83, 0x3c, 0x25, 0xc0, 0xff, 0xff, 0xff, 0][..],
            DATA,
            8,
        ),
    ] {
        let mut state = user_state();
        state.fs_base = DATA + 0x40;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..width {
            memory = memory.with_user(address + offset as u64, 0);
        }
        let done = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.rflags & ARITH, ZF | PF);
        assert_eq!(memory.read_widths, [(address, width)]);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_reference_rdsspq_preserves_rax_without_shadow_stack() {
    let code = [0xf3, 0x48, 0x0f, 0x1e, 0xc8];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xfedc_ba98_7654_3210;
    state.rflags |= ARITH;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    let done = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
    assert_eq!(done.rflags_defined, od_isa::INTEGER_FLAGS);
    assert_eq!(state.rip, RIP + code.len() as u64);
    state.rip = before.rip;
    assert_eq!(state, before);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());

    for code in [
        &[0xf3, 0x0f, 0x1e, 0xc8][..],
        &[0xf3, 0x48, 0x0f, 0x1e, 0xc9][..],
        &[0x48, 0xf3, 0x0f, 0x1e, 0xc8][..],
    ] {
        let mut state = before.clone();
        let mut memory = DataMemory::new(code);
        let done = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&done), &RunExit::Unsupported { opcode: 0x1e });
        assert_eq!(state, before);
    }
}

#[test]
fn cpl3_reference_rdtsc_uses_the_injected_clock() {
    let code = [0x0f, 0x31];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = u64::MAX;
    state.gpr[Register::Rdx as usize] = u64::MAX;
    let before = state.clone();
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_rdtsc();
    let mut memory = DataMemory::new(&code);
    let mut ticks = 0;
    let done = run_with_cpl3_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring3, 1),
        od_isa::INTEGER_FLAGS,
        &set,
        Some(&mut || {
            ticks += 1;
            0x1234_5678_9abc_def0
        }),
        || false,
    )
    .unwrap();
    assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
    assert_eq!(ticks, 1);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.gpr[Register::Rax as usize], 0x9abc_def0);
    assert_eq!(state.gpr[Register::Rdx as usize], 0x1234_5678);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_busybox_notrack_jump_uses_the_full_register_target() {
    for (code, register) in [
        (&[0x3e, 0xff, 0xe0][..], Register::Rax),
        (&[0x3e, 0xff, 0xe2][..], Register::Rdx),
        (&[0x3e, 0x41, 0xff, 0xe2][..], Register::R10),
    ] {
        for target in [0x6000_0000_0010, 0x8000_0000_0010] {
            let mut state = user_state();
            state.gpr[register as usize] = target;
            let before = state.clone();
            let mut memory = DataMemory::new(code);
            let done = run3_data(&mut state, &mut memory, 1).unwrap();
            if target == 0x6000_0000_0010 {
                assert_eq!(exit(&done), &RunExit::InstructionBudgetExhausted);
                assert_eq!(state.rip, target);
                state.rip = before.rip;
            } else {
                assert_eq!(
                    exit(&done),
                    &RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    })
                );
            }
            assert_eq!(state, before);
            assert!(memory.read_widths.is_empty());
            assert!(memory.writes.is_empty());
        }
        let mut state = user_state();
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        let bare = od_isa::execute::run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring3, 1),
            || false,
        );
        assert!(matches!(bare.exit, RunExit::Unsupported { .. }));
        assert_eq!(bare.instructions_executed, 0);
        assert_eq!(state, before);
    }

    for code in [
        &[0x2e, 0xff, 0xe0][..],
        &[0x3e, 0x3e, 0xff, 0xe0][..],
        &[0x41, 0x3e, 0xff, 0xe2][..],
        &[0x3e, 0x67, 0xff, 0xe0][..],
        &[0x3e, 0xff, 0xe1][..],
    ] {
        let mut state = user_state();
        let mut memory = DataMemory::new(code);
        let done = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(exit(&done), &RunExit::Unsupported { opcode: 0xff });
    }
}

#[test]
fn cpl3_issue573_indirect_jump_reads_rip_relative_target_before_rip_commit() {
    let target = 0x0040_1234u64;
    let next_rip = RIP + 6;
    let displacement = (DATA as i64 - next_rip as i64) as i32;
    let mut code = vec![0xff, 0x25]; // jmp qword [rip+disp32]
    code.extend_from_slice(&displacement.to_le_bytes());
    let mut state = user_state();
    let flags = state.rflags | ARITH;
    state.rflags = flags;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in target.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + offset as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, target);
    assert_eq!(state.rflags, flags);
    assert_eq!(memory.read_widths, [(DATA, 8)]);
    assert!(memory.writes.is_empty());

    let mut fault_state = user_state();
    fault_state.rflags |= ARITH;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&code);
    for offset in 0..8 {
        fault_memory = fault_memory.with_supervisor(DATA + offset, 0x55);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(fault_state, before);
    assert_eq!(fault_memory.read_widths, [(DATA, 8)]);
    assert!(fault_memory.writes.is_empty());

    let mut bad_target_state = user_state();
    let before_bad_target = bad_target_state.clone();
    let mut bad_target_memory = DataMemory::new(&code);
    for (offset, byte) in 0x0000_8000_0000_0000u64
        .to_le_bytes()
        .into_iter()
        .enumerate()
    {
        bad_target_memory = bad_target_memory.with_user(DATA + offset as u64, byte);
    }
    let result = run3_data(&mut bad_target_state, &mut bad_target_memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(bad_target_state, before_bad_target);
    assert_eq!(bad_target_memory.read_widths, [(DATA, 8)]);
    assert!(bad_target_memory.writes.is_empty());
}

#[test]
fn cpl3_issue573_fs_qword_base_load_and_stores_use_live_base() {
    let code = [
        0x64, 0x48, 0x8b, 0x00, // mov rax, fs:[rax]
        0x64, 0x48, 0x89, 0x11, // mov fs:[rcx], rdx
        0x64, 0x48, 0x89, 0x02, // mov fs:[rdx], rax
    ];
    let fs = DATA + 0x1000;
    let loaded = 0x8877_6655_4433_2211u64;
    let stored_rdx = 0x0000_3040_5060_7080u64;
    let mut state = user_state();
    state.fs_base = fs;
    state.rflags |= ARITH;
    let flags = state.rflags;
    state.gpr[Register::Rax as usize] = 0x20;
    state.gpr[Register::Rcx as usize] = 0x80;
    state.gpr[Register::Rdx as usize] = stored_rdx;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in loaded.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(fs + 0x20 + offset as u64, byte);
    }
    for offset in 0..8 {
        memory = memory.with_user(fs + 0x80 + offset, 0);
    }
    for offset in 0..8 {
        memory = memory.with_user(fs + stored_rdx + offset, 0);
    }

    let result = run3_data(&mut state, &mut memory, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rax as usize], loaded);
    assert_eq!(state.fs_base, fs);
    assert_eq!(state.rflags, flags);
    assert_eq!(memory.read_widths, [(fs + 0x20, 8)]);
    assert_eq!(memory.writes.len(), 16);
    assert_eq!(
        &memory.writes[..8],
        &(0..8)
            .map(|i| (fs + 0x80 + i, (stored_rdx >> (i * 8)) as u8))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        &memory.writes[8..],
        &(0..8)
            .map(|i| (fs + stored_rdx + i, (loaded >> (i * 8)) as u8))
            .collect::<Vec<_>>()
    );
}

#[test]
fn cpl3_issue573_fs_faults_and_neighboring_forms_stay_outside() {
    let code = [0x64, 0x48, 0x8b, 0x00];
    let fs = DATA + 0x1000;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rax as usize] = 0x20;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_supervisor(fs + 0x20 + offset, 0x55);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(fs + 0x20)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(memory.writes.is_empty());

    // A failed qword store through FS checks user write permission before any byte is committed.
    let store_code = [0x64, 0x48, 0x89, 0x11]; // mov fs:[rcx], rdx
    let mut store_state = user_state();
    store_state.fs_base = fs;
    store_state.gpr[Register::Rcx as usize] = 0x80;
    store_state.gpr[Register::Rdx as usize] = 0x1122_3344_5566_7788;
    let store_before = store_state.clone();
    let mut store_memory = DataMemory::new(&store_code);
    for offset in 0..8 {
        store_memory = store_memory.with_supervisor(fs + 0x80 + offset, 0);
    }
    let result = run3_data(&mut store_state, &mut store_memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(fs + 0x80)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(store_state, store_before);
    assert!(store_memory.writes.is_empty());

    // A non-canonical FS-adjusted store address faults before the backend sees any write.
    let mut canonical_state = user_state();
    canonical_state.fs_base = 0x0000_7fff_ffff_fff0;
    canonical_state.gpr[Register::Rcx as usize] = 0x20;
    let canonical_before = canonical_state.clone();
    let mut canonical_memory = DataMemory::new(&store_code);
    let result = run3_data(&mut canonical_state, &mut canonical_memory, 1).unwrap();
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(canonical_state, canonical_before);
    assert!(canonical_memory.writes.is_empty());

    for code in [
        &[0x65, 0x48, 0x8b, 0x00][..],             // GS is not added
        &[0x64, 0x48, 0x8b, 0x04, 0x08][..],       // indexed FS load is out of scope
        &[0x64, 0x48, 0x89, 0x0c, 0x08][..],       // indexed FS store is out of scope
        &[0x64, 0x48, 0xff, 0x25, 0, 0, 0, 0][..], // FS indirect jump stays refused
        &[0x48, 0xff, 0xe1][..],                   // REX register indirect jump stays refused
    ] {
        let mut state = user_state();
        state.fs_base = fs;
        state.gpr[Register::Rax as usize] = DATA;
        state.gpr[Register::Rcx as usize] = 0x40;
        state.gpr[Register::Rdx as usize] = 0x60;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..16 {
            memory = memory.with_user(DATA + offset, 0);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            matches!(exit(&result), RunExit::Unsupported { .. }),
            "code={code:02x?} exit={:?}",
            exit(&result)
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_s622_owned_scalar_operand_variants_execute() {
    for code in [
        &[0x48, 0xc1, 0xe8, 0x06][..], // pin: SHR RAX,6
        &[0x48, 0x31, 0xf8],           // exchange_packets: XOR RAX,RDI
        &[0x0f, 0xaf, 0xc2],           // exchange_packets: IMUL EAX,EDX
        &[0x49, 0x0f, 0xaf, 0xff],     // exchange_packets: IMUL RDI,R15
        &[0x41, 0x29, 0xfe],           // take_turn: SUB R14D,EDI
    ] {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = 0x119;
        state.gpr[Register::Rdi as usize] = 2;
        state.gpr[Register::Rdx as usize] = 3;
        state.gpr[Register::R14 as usize] = 10;
        state.gpr[Register::R15 as usize] = 5;
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert!(memory.read_widths.is_empty());
        assert!(memory.writes.is_empty());
    }

    let code = [0x88, 0x44, 0x24, 0x0f]; // exchange_packets: MOV [RSP+15],AL
    let mut state = user_state();
    state.gpr[Register::Rsp as usize] = DATA;
    state.gpr[Register::Rax as usize] = 0x1234_569c;
    let flags = state.rflags;
    let mut memory = DataMemory::new(&code).with_user(DATA + 0x0f, 0);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rflags, flags);
    assert_eq!(memory.writes, [(DATA + 0x0f, 0x9c)]);
}

#[test]
fn cpl3_s622_owned_scalar_batch_executes_exact_forms() {
    let code = [
        0x49, 0x11, 0xc6, // adc r14, rax
        0x83, 0xc2, 0x01, // add edx, 1
        0x41, 0x83, 0xc5, 0x11, // add r13d, 17
        0x00, 0xf0, // add al, dh
        0x48, 0x3b, 0x6c, 0x24, 0x10, // cmp rbp, qword [rsp+16]
        0x44, 0x3a, 0x6c, 0x24, 0x0e, // cmp r13b, byte [rsp+14]
        0x0f, 0xaf, 0xc7, // imul eax, edi
        0x49, 0x0f, 0xaf, 0xf7, // imul rsi, r15
        0x48, 0x6b, 0xdb, 0x30, // imul rbx, rbx, 48
        0x88, 0x44, 0x24, 0x0e, // mov byte [rsp+14], al
        0x0f, 0xb6, 0xc0, // movzx eax, al
        0x48, 0xd1, 0xfe, // sar rsi, 1
        0x48, 0xd3, 0xe6, // shl rsi, cl
        0x48, 0xc1, 0xee, 0x06, // shr rsi, 6
        0x83, 0xe8, 0x02, // sub eax, 2
        0x41, 0x83, 0xed, 0x01, // sub r13d, 1
        0x29, 0xfa, // sub edx, edi
        0x44, 0x29, 0xe8, // sub eax, r13d
        0x48, 0x31, 0xf1, // xor rcx, rsi
        0x48, 0x87, 0xdd, // xchg rbp, rbx
        0x48, 0x0f, 0xa3, 0xc6, // bt rsi, rax
        0x66, 0xf7, 0x44, 0x24, 0x04, 0x7f, 0xff, // test word [rsp+4], 0xff7f
        0xff, 0xe0, // jmp rax
    ];
    let mut state = user_state();
    state.gpr[Register::Rsp as usize] = DATA + 0x100;
    state.gpr[Register::Rax as usize] = 0x23;
    state.gpr[Register::Rdx as usize] = 0x0000_0000_0000_3400;
    state.gpr[Register::R13 as usize] = 0x31;
    state.gpr[Register::Rdi as usize] = 7;
    state.gpr[Register::R14 as usize] = 0x10;
    state.gpr[Register::R15 as usize] = 9;
    state.gpr[Register::Rsi as usize] = 0x80;
    state.gpr[Register::Rbx as usize] = 3;
    state.gpr[Register::Rbp as usize] = 0x1122;
    state.gpr[Register::Rcx as usize] = 1;
    let flags = state.rflags | ARITH;
    state.rflags = flags;
    let mut memory = DataMemory::new(&code);
    for (index, byte) in 0x1122u64.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(DATA + 0x110 + index as u64, byte);
    }
    memory = memory.with_user(DATA + 0x10e, 0x31);
    memory = memory
        .with_user(DATA + 0x104, 0x7f)
        .with_user(DATA + 0x105, 0xff);

    let result = run3_data(&mut state, &mut memory, 23).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 23);
    assert_eq!(state.rip, 0x1e);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1e);
    assert_eq!(state.gpr[Register::Rdx as usize] as u32, 0x33fa);
    assert_eq!(memory.writes, [(DATA + 0x10e, 0x61)]);
    assert_eq!(
        memory.read_widths,
        [(DATA + 0x110, 8), (DATA + 0x10e, 1), (DATA + 0x104, 2)]
    );
    assert_ne!(
        state.rflags & (CF | PF | AF | ZF | SF | OF),
        flags & (CF | PF | AF | ZF | SF | OF)
    );
}

#[test]
fn cpl3_issue576_owned_scalar_register_forms_execute() {
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0x0123_4567_89ab_cdef;
    state.gpr[Register::Rcx as usize] = 0x1122_3344_5566_7788;
    state.gpr[Register::Rdx as usize] = 0x0102_0304;
    state.gpr[Register::R9 as usize] = 0x1234_5678;
    state.gpr[Register::R11 as usize] = 0x8000_0001;
    state.gpr[Register::Rsi as usize] = 3;
    let flags = state.rflags;
    let code = [
        0x0f, 0xca, // bswap edx
        0x48, 0x0f, 0xc8, // bswap rax
        0x48, 0x0f, 0xc9, // bswap rcx
        0xc1, 0xc0, 0x0f, // rol eax, 15
        0xc1, 0xca, 0x08, // ror edx, 8
        0x41, 0xc1, 0xc1, 0x0d, // rol r9d, 13
        0x41, 0xc1, 0xcb, 0x02, // ror r11d, 2
        0x0f, 0xa3, 0xc2, // bt edx, eax
        0x0f, 0xba, 0xe0, 0x13, // bt eax, 19
        0x41, 0x0f, 0xba, 0xe4, 0x08, // bt r12d, 8
        0x48, 0x0f, 0xa3, 0xd1, // bt rcx, rdx
        0x48, 0x0f, 0xba, 0xf0, 0x1f, // btr rax, 31
        0x48, 0x0f, 0xab, 0xf2, // bts rdx, rsi
        0x6b, 0xcb, 0x04, // imul ecx, ebx, 4
    ];
    state.gpr[Register::R12 as usize] = 0x101;
    state.gpr[Register::Rbx as usize] = 0xffff_fff0;
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 14).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 14);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1180_b3a2);
    assert_eq!(state.gpr[Register::Rdx as usize] as u32, 0x0104_030a);
    assert_eq!(state.gpr[Register::R9 as usize] as u32, 0x8acf_0246);
    assert_eq!(state.gpr[Register::R11 as usize] as u32, 0x6000_0000);
    assert_eq!(state.gpr[Register::Rax as usize] & (1 << 31), 0);
    assert_eq!(state.gpr[Register::Rdx as usize] & 8, 8);
    assert_eq!(state.gpr[Register::Rcx as usize] as u32, 0xffff_ffc0);
    assert_eq!(state.rflags & (CF | OF), 0);
    assert_eq!(flags & (CF | OF), 0);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_issue576_busybox_ror_ecx_and_bt_esi_execute() {
    let code = [0xc1, 0xc9, 0x07, 0x0f, 0xba, 0xe6, 0x08];
    let mut state = user_state();
    let value = 0x8000_0001_u32;
    state.gpr[Register::Rcx as usize] = 0x1234_5678_0000_0000 | u64::from(value);
    state.gpr[Register::Rsi as usize] = 1 << 8;
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(
        state.gpr[Register::Rcx as usize],
        u64::from(value.rotate_right(7))
    );
    assert_eq!(state.gpr[Register::Rsi as usize], 1 << 8);
    assert_eq!(state.rflags & CF, CF);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_issue576_owned_memory_and_control_forms_preserve_fault_order() {
    // BT's memory form is read-only and leaves the full architectural state unchanged on #PF.
    let bt = [0x0f, 0xba, 0xa5, 0x2c, 0xff, 0xff, 0xff, 0x0a]; // bt dword [rbp-0xd4], 10
    let address = DATA + 0x200 - 0xd4;
    let mut state = user_state();
    state.gpr[Register::Rbp as usize] = DATA + 0x200;
    let mut memory = DataMemory::new(&bt);
    for (offset, byte) in 0x0000_0400u32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(address + offset as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(memory.read_widths, [(address, 4)]);
    assert!(memory.writes.is_empty());
    let mut fault_state = user_state();
    fault_state.gpr[Register::Rbp as usize] = DATA + 0x200;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&bt);
    for offset in 0..4 {
        fault_memory = fault_memory.with_supervisor(address + offset, 0);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(fault_state, before);
    assert!(fault_memory.writes.is_empty());

    // CMOVE reads its m32 source even when ZF is clear; a fault precedes destination/RIP commit.
    let cmove = [0x0f, 0x44, 0x85, 0xfc, 0xfe, 0xff, 0xff];
    let source = DATA + 0x300 - 0x104;
    let mut state = user_state();
    state.gpr[Register::Rbp as usize] = DATA + 0x300;
    state.gpr[Register::Rax as usize] = 0xfeed_face_1234_5678;
    let mut memory = DataMemory::new(&cmove);
    for (offset, byte) in 0x89ab_cdefu32.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(source + offset as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678);
    assert_eq!(memory.read_widths, [(source, 4)]);
    assert!(memory.writes.is_empty());
    let mut fault_state = user_state();
    fault_state.gpr[Register::Rbp as usize] = DATA + 0x300;
    fault_state.gpr[Register::Rax as usize] = 0x1234_5678;
    fault_state.rflags |= ZF;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&cmove);
    for offset in 0..4 {
        fault_memory = fault_memory.with_supervisor(source + offset, 0);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault { vector: 14, .. })
    ));
    assert_eq!(fault_state, before);
    assert!(fault_memory.writes.is_empty());

    // Indirect CALL commits a qword return address only after target fetch; JMP memory is read-only.
    let call = [0x41, 0xff, 0xd2]; // call r10
    let target = RIP + call.len() as u64;
    let mut state = user_state();
    state.gpr[Register::R10 as usize] = target;
    state.gpr[Register::Rsp as usize] = DATA + 0x100;
    let mut memory = DataMemory::new(&call);
    for offset in 0..8 {
        memory = memory.with_user(DATA + 0xf8 + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, target);
    assert_eq!(state.gpr[Register::Rsp as usize], DATA + 0xf8);
    assert_eq!(
        memory.writes,
        (RIP + 3)
            .to_le_bytes()
            .into_iter()
            .enumerate()
            .map(|(offset, byte)| (DATA + 0xf8 + offset as u64, byte))
            .collect::<Vec<_>>()
    );
    assert_eq!(memory.read_widths, []);

    let mut fault_state = user_state();
    fault_state.gpr[Register::R10 as usize] = target;
    fault_state.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&call);
    for offset in 0..8 {
        fault_memory = fault_memory.with_supervisor(DATA + 0xf8 + offset, 0);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(fault_state, before);
    assert!(fault_memory.writes.is_empty());

    let call_rdx = [0xff, 0xd2]; // call rdx
    let rdx_target = RIP + call_rdx.len() as u64;
    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = rdx_target;
    state.gpr[Register::Rsp as usize] = DATA + 0x100;
    let mut memory = DataMemory::new(&call_rdx);
    for offset in 0..8 {
        memory = memory.with_user(DATA + 0xf8 + offset, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, rdx_target);
    assert_eq!(state.gpr[Register::Rsp as usize], DATA + 0xf8);

    let mut bad_target_state = user_state();
    bad_target_state.gpr[Register::R10 as usize] = 0x0000_8000_0000_0000;
    bad_target_state.gpr[Register::Rsp as usize] = DATA + 0x100;
    let before = bad_target_state.clone();
    let mut bad_target_memory = DataMemory::new(&call);
    for offset in 0..8 {
        bad_target_memory = bad_target_memory.with_user(DATA + 0xf8 + offset, 0);
    }
    let result = run3_data(&mut bad_target_state, &mut bad_target_memory, 1).unwrap();
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(bad_target_state, before);
    assert!(bad_target_memory.writes.is_empty());

    let register_jump = [0xff, 0xe2]; // jmp rdx
    let target = RIP + register_jump.len() as u64;
    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = target;
    let mut memory = DataMemory::new(&register_jump);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, target);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());

    let jump = [0xff, 0x60, 0x18]; // jmp qword [rax+0x18]
    let source = DATA + 0x400 + 0x18;
    let target = RIP + jump.len() as u64;
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = DATA + 0x400;
    let mut memory = DataMemory::new(&jump);
    for (offset, byte) in target.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(source + offset as u64, byte);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, target);
    assert_eq!(memory.read_widths, [(source, 8)]);
    assert!(memory.writes.is_empty());

    let mut fault_state = user_state();
    fault_state.gpr[Register::Rax as usize] = DATA + 0x400;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&jump);
    for offset in 0..8 {
        fault_memory = fault_memory.with_supervisor(source + offset, 0);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert_eq!(result.instructions_executed, 0);
    assert!(matches!(
        exit(&result),
        RunExit::Fault(ArchitecturalFault { vector: 14, .. })
    ));
    assert_eq!(fault_state, before);
    assert!(fault_memory.writes.is_empty());
}

#[test]
fn cpl3_issue576_scalar_neighbors_keep_prefix_and_operand_limits() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x66, 0x0f, 0xc8], 0xc8),             // operand override BSWAP
        (&[0xf0, 0x48, 0x0f, 0xab, 0xf2], 0xab), // LOCK BTS register form
        (&[0x48, 0x0f, 0xa3, 0x07], 0xa3),       // unobserved BT memory/register-offset form
        (&[0x49, 0xff, 0xd2], 0xff),             // unused REX.W call alias
        (&[0x66, 0x6b, 0xcb, 0x04], 0x6b),       // word IMUL immediate
        (&[0x41, 0xc1, 0xc0, 0x0d], 0xc1),       // unobserved ROL r8d
    ];
    for (code, expected_opcode) in cases {
        let mut state = user_state();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported {
                opcode: *expected_opcode
            },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
    }
}

#[test]
fn cpl3_s622_fs_canary_compare_is_bounded_and_fault_atomic() {
    let code = [0x64, 0x48, 0x2b, 0x04, 0x25, 0x28, 0, 0, 0];
    let fs = DATA + 0x1000;
    let mut state = user_state();
    state.fs_base = fs;
    state.gpr[Register::Rax as usize] = 0x100;
    state.rflags |= ARITH;
    let before_flags = state.rflags;
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 0x21u64.to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(fs + 0x28 + offset as u64, byte);
    }
    let decoded =
        od_isa::long_decode::decode(&mut memory, RIP, config(PrivilegeLevel::Ring3, 1).context)
            .unwrap();
    println!("decoded FS SUB: {decoded:?}");
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rax as usize], 0xdf);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(memory.read_widths, [(fs + 0x28, 8)]);
    assert!(memory.writes.is_empty());
    assert_ne!(state.rflags, before_flags);

    let mut fault_state = user_state();
    fault_state.fs_base = fs;
    fault_state.gpr[Register::Rax as usize] = 0x100;
    fault_state.rflags |= ARITH;
    let before = fault_state.clone();
    let mut fault_memory = DataMemory::new(&code);
    for offset in 0..8 {
        fault_memory = fault_memory.with_supervisor(fs + 0x28 + offset, 0);
    }
    let result = run3_data(&mut fault_state, &mut fault_memory, 1).unwrap();
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(fs + 0x28)
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(fault_state, before);
    assert_eq!(fault_memory.read_widths, [(fs + 0x28, 8)]);
    assert!(fault_memory.writes.is_empty());
}

#[test]
fn cpl3_s622_scalar_batch_refuses_unobserved_neighbors() {
    let cases: &[(&[u8], u8)] = &[
        (&[0x48, 0x11, 0xc6], 0x11),       // ADC with an unobserved REX encoding
        (&[0x48, 0x3b, 0x04, 0x47], 0x3b), // unobserved scaled-index comparison
        (&[0x48, 0x0f, 0xa3, 0x07], 0xa3), // BT memory destination
        (&[0x6b, 0x03, 0x02], 0x6b),       // IMUL with a memory source
        (&[0x48, 0x88, 0x44, 0x24, 0x0e], 0x88), // REX byte store variant
        (&[0x66, 0x0f, 0xb6, 0xc0], 0xb6), // operand-size override on MOVZX
        (&[0x64, 0x48, 0x2b, 0x04, 0x24], 0x2b), // FS canary compare with base register
    ];
    for (code, expected_opcode) in cases {
        let mut state = user_state();
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported {
                opcode: *expected_opcode
            }
        );
        assert_eq!(result.instructions_executed, 0);
    }
}

#[test]
fn cpl3_s622_read_only_memory_alu_faults_preserve_state() {
    let cases: &[(&[u8], u64, usize)] = &[
        (&[0x48, 0x3b, 0x6c, 0x24, 0x10], DATA + 0x110, 8), // cmp r64, qword [rsp+16]
        (&[0x44, 0x3a, 0x6c, 0x24, 0x0e], DATA + 0x10e, 1), // cmp r8, byte [rsp+14]
        (&[0x66, 0xf7, 0x44, 0x24, 0x04, 0x7f, 0xff], DATA + 0x104, 2), // test word [rsp+4], imm16
    ];
    for (code, address, width) in cases {
        let mut state = user_state();
        state.gpr[Register::Rsp as usize] = DATA + 0x100;
        state.gpr[Register::R13 as usize] = 0x44;
        state.rflags |= ARITH;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..*width {
            memory = memory.with_supervisor(*address + offset as u64, 0x55);
        }
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(5),
                linear_address: Some(*address),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.read_widths, [(*address, *width)]);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_s622_tls_return_rip_byte_store_and_movzx_use_observed_registers() {
    let code = [
        0x88, 0x1d, 0xd7, 0xfe, 0x11, 0x00, // mov byte [rip+0x11fed7], bl
        0x0f, 0xb6, 0xfb, // movzx edi, bl
    ];
    let target = RIP + 6 + 0x11fed7;
    let byte = 0x5a;
    let mut state = user_state();
    state.gpr[Register::Rbx as usize] = 0x1234_5678_0000_005a;
    let mut memory = DataMemory::new(&code).with_user(target, 0);
    let result = run3_data(&mut state, &mut memory, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rdi as usize], u64::from(byte));
    assert_eq!(state.gpr[Register::Rbx as usize], 0x1234_5678_0000_005a);
    assert_eq!(memory.writes, [(target, byte)]);
    assert!(memory.read_widths.is_empty());
}

#[test]
fn cpl3_s622_observed_register_bitscan_forms_reuse_live_semantics() {
    let code = [
        0x0f, 0xbc, 0xc2, // bsf eax, edx
        0x0f, 0xbd, 0xc0, // bsr eax, eax
        0x48, 0x0f, 0xbc, 0xd2, // bsf rdx, rdx
    ];
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = u64::MAX;
    state.gpr[Register::Rdx as usize] = 0x10;
    state.rflags |= ARITH;
    let arithmetic = CF | PF | AF | ZF | SF | OF;
    let non_arithmetic_flags = state.rflags & !arithmetic;
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + code.len() as u64);
    assert_eq!(state.gpr[Register::Rax as usize], 2);
    assert_eq!(state.gpr[Register::Rdx as usize], 4);
    assert_eq!(state.rflags & !arithmetic, non_arithmetic_flags);
    assert_eq!(state.rflags & ZF, 0);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());

    // A zero source leaves the destination unchanged and sets ZF.
    let mut zero_state = user_state();
    zero_state.gpr[Register::Rax as usize] = 0xfeed_beef;
    zero_state.gpr[Register::Rdx as usize] = 0;
    let mut zero_memory = DataMemory::new(&[0x0f, 0xbc, 0xc2]);
    let zero_result = run3_data(&mut zero_state, &mut zero_memory, 1).unwrap();
    assert_eq!(exit(&zero_result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(zero_state.gpr[Register::Rax as usize], 0xfeed_beef);
    assert_ne!(zero_state.rflags & ZF, 0);
    assert!(zero_memory.read_widths.is_empty());
}

#[test]
fn cpl3_s622_busybox_register_form_samples_execute_exactly() {
    let samples: &[&[u8]] = &[
        &[0x01, 0xc2],                         // add edx, eax
        &[0x81, 0xee, 0xbd, 0, 0, 0],          // sub esi, 0xbd
        &[0x45, 0x01, 0xc7],                   // add r15d, r8d
        &[0x48, 0x83, 0xc8, 0x01],             // or rax, 1
        &[0x4c, 0x09, 0xd1],                   // or rcx, r10
        &[0x48, 0x81, 0xe1, 0xff, 0x0f, 0, 0], // and rcx, 0xfff
        &[0x83, 0xf2, 0x01],                   // xor edx, 1
        &[0x4d, 0x6b, 0xe4, 0x70],             // imul r12, r12, 0x70
        &[0x66, 0x85, 0xd2],                   // test dx, dx
        &[0x48, 0xd1, 0xfa],                   // sar rdx, 1
        &[0x48, 0xd1, 0xe8],                   // shr rax, 1
        &[0xd3, 0xe0],                         // shl eax, cl
        &[0x41, 0xd3, 0xe9],                   // shr r9d, cl
        &[0x48, 0xd3, 0xea],                   // shr rdx, cl
        &[0x41, 0xd1, 0xfb],                   // sar r11d, 1
        &[0xd1, 0xf8],                         // sar eax, 1
        &[0x48, 0xc1, 0xc2, 0x11],             // rol rdx, 0x11
        &[0x48, 0xc1, 0xc8, 0x11],             // ror rax, 0x11
        &[0x48, 0x87, 0xf7],                   // xchg rdi, rsi
        &[0x48, 0x93],                         // xchg rax, rbx
        &[0x49, 0x94],                         // xchg rax, r12
        &[0x93],                               // xchg eax, ebx
        &[0x97],                               // xchg eax, edi
        &[0xff, 0xc3],                         // inc ebx
    ];
    for code in samples {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = 0x1234_5678;
        state.gpr[Register::Rcx as usize] = 3;
        state.gpr[Register::Rdx as usize] = 0x80;
        state.gpr[Register::Rsi as usize] = 0x12;
        state.gpr[Register::Rdi as usize] = 0x21;
        state.gpr[Register::R8 as usize] = 4;
        state.gpr[Register::R9 as usize] = 0x80;
        state.gpr[Register::R10 as usize] = 0x40;
        state.gpr[Register::R11 as usize] = 0x80;
        state.gpr[Register::R12 as usize] = 5;
        state.gpr[Register::R15 as usize] = 7;
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1, "{code:02x?}");
        assert_eq!(state.rip, RIP + code.len() as u64, "{code:02x?}");
        assert!(memory.read_widths.is_empty(), "{code:02x?}");
        assert!(memory.writes.is_empty(), "{code:02x?}");
    }
}

#[test]
fn cpl3_s622_observed_call_rax_pushes_before_target_rip_commit() {
    let code = [0xff, 0xd0]; // call rax
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = RIP + 0x1000;
    state.gpr[Register::Rsp as usize] = DATA + 0x100;
    let return_rip = RIP + code.len() as u64;
    let mut memory = DataMemory::new(&code);
    for byte in 0..8 {
        memory = memory.with_user(DATA + 0xf8 + byte, 0);
    }
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 0x1000);
    assert_eq!(state.gpr[Register::Rsp as usize], DATA + 0xf8);
    assert_eq!(memory.writes.len(), 8);
    assert_eq!(memory.writes[0], (DATA + 0xf8, return_rip as u8));
    assert_eq!(memory.writes[7], (DATA + 0xff, (return_rip >> 56) as u8));
}

#[test]
fn cpl3_issue575_pinsrw_and_pshuflw_use_two_byte_read_and_prestate_high_words() {
    let code = [
        0x66, 0x0f, 0xc4, 0x0d, 0x1e, 0x89, 0x20, 0x00,
        0x00, // PINSRW xmm1, [RIP+0x20891e], 0
        0xf2, 0x0f, 0x70, 0xc1, 0x00, // PSHUFLW xmm0, xmm1, 0 (BusyBox)
        0xf2, 0x0f, 0x70, 0xc9, 0x00, // PSHUFLW xmm1, xmm1, 0 (self-alias)
    ];
    let mut state = user_state();
    state.rip = 0x0040_adf1;
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.rflags |= ARITH;
    let target = state.rip + 9 + 0x20_891e;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[1] = std::array::from_fn(|index| 0x20 + index as u8);
    let fp = std::cell::RefCell::new(initial_fp);
    let mut memory = DataMemory::new(&code);
    memory.code_base = state.rip;
    memory = memory.with_user(target, 0xaa).with_user(target + 1, 0x99);

    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 3);

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, 0x0040_adf1 + code.len() as u64);
    assert_eq!(state.rflags & ARITH, ARITH);
    assert_eq!(
        memory.read_widths,
        [(target, 2)],
        "PINSRW reads exactly its m16 source"
    );
    assert_eq!(memory.writes, []);
    assert_eq!(
        fp.borrow().sse.xmm[1],
        [
            0xaa, 0x99, 0xaa, 0x99, 0xaa, 0x99, 0xaa, 0x99, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d,
            0x2e, 0x2f
        ],
        "PSHUFLW duplicates low words and preserves high64 from its source prestate"
    );
}

#[test]
fn cpl3_issue575_pinsrw_read_fault_preserves_xmm_and_rip() {
    let code = [0x66, 0x0f, 0xc4, 0x0d, 0x1e, 0x89, 0x20, 0x00, 0x00];
    let mut state = user_state();
    state.rip = 0x0040_adf1;
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let before_state = state.clone();
    let address = state.rip + code.len() as u64 + 0x20_891e;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[1] = [0xa5; 16];
    let fp = std::cell::RefCell::new(initial_fp.clone());
    let mut memory = DataMemory::new(&code);
    memory.code_base = state.rip;
    for offset in 0..2 {
        memory = memory.with_supervisor(address + offset, 0xff);
    }

    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);

    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(address),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before_state);
    assert_eq!(*fp.borrow(), initial_fp);
    assert_eq!(memory.read_widths, [(address, 2)]);
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_static_pthread_atomic_forms_respect_width_and_alignment() {
    for (code, opcode, width, expected, exchanged) in [
        (&[0xf0, 0x83, 0x03, 0x01][..], 0x83, 4, 6, false), // LOCK ADD [RBX],1
        (
            &[0xf0, 0x48, 0x01, 0x03][..],
            0x01,
            8,
            0x1_0000_0008u64,
            false,
        ), // LOCK ADD [RBX],RAX
        (&[0xf0, 0x83, 0x2b, 0x01][..], 0x83, 4, 4, false), // LOCK SUB [RBX],1
        (&[0xf0, 0x83, 0x0b, 0x10][..], 0x83, 4, 0x15, false), // LOCK OR [RBX],16
        (&[0xf0, 0x0f, 0xc1, 0x03][..], 0xc1, 4, 8, true),  // LOCK XADD [RBX],EAX
        (
            &[0xf0, 0x48, 0x0f, 0xc1, 0x03][..],
            0xc1,
            8,
            0x1_0000_0008u64,
            true,
        ), // XADD qword
        (&[0xf0, 0x0f, 0xba, 0x2b, 0x05][..], 0xba, 4, 0x25, false), // LOCK BTS [RBX],5
    ] {
        for aligned in [true, false] {
            let address = if aligned { DATA } else { DATA + 1 };
            let mut state = user_state();
            state.gpr[Register::Rbx as usize] = address;
            state.gpr[Register::Rax as usize] = 3;
            let before = state.clone();
            let mut memory = DataMemory::new(code);
            let initial = if width == 8 { 0x1_0000_0005u64 } else { 5 };
            for (offset, &byte) in initial.to_le_bytes()[..width].iter().enumerate() {
                memory = memory.with_user(address + offset as u64, byte);
            }
            let done = run3_data(&mut state, &mut memory, 1).unwrap();
            if aligned {
                assert_eq!(
                    exit(&done),
                    &RunExit::InstructionBudgetExhausted,
                    "{code:02x?}"
                );
                assert_eq!(state.rip, RIP + code.len() as u64);
                assert_eq!(
                    state.gpr[Register::Rax as usize],
                    if exchanged { initial } else { 3 }
                );
                let bytes: Vec<_> = expected.to_le_bytes()[..width]
                    .iter()
                    .enumerate()
                    .map(|(offset, &byte)| (address + offset as u64, byte))
                    .collect();
                assert_eq!(memory.writes, bytes);
            } else {
                assert_eq!(exit(&done), &RunExit::Unsupported { opcode }, "{code:02x?}");
                assert_eq!(state, before);
                assert_eq!(done.instructions_executed, 0);
                assert!(memory.read_widths.is_empty());
                assert!(memory.writes.is_empty());
            }
        }
    }
}

#[test]
fn cpl3_static_scalar_fs_samples_are_admitted() {
    let cases: &[(u8, &[u8])] = &[
        (0, &[0x88, 0x1d, 0x90, 0x27, 0x0a, 0x00]),
        (0, &[0x88, 0x42, 0x76]),
        (0, &[0x88, 0x01]),
        (0, &[0x88, 0x85, 0x40, 0xfb, 0xff, 0xff]),
        (1, &[0x48, 0x0f, 0xbc, 0xc2]),
        (1, &[0x48, 0x0f, 0xbc, 0xc0]),
        (2, &[0x41, 0x21, 0xca]),
        (2, &[0x41, 0x21, 0xf3]),
        (2, &[0x44, 0x21, 0xf8]),
        (3, &[0x0d, 0x00, 0x00, 0x00, 0x04]),
        (4, &[0xf7, 0xdb]),
        (4, &[0xf7, 0xd8]),
        (4, &[0xf7, 0xda]),
        (5, &[0x45, 0x0f, 0xb6, 0xff]),
        (5, &[0x41, 0x0f, 0xb6, 0xc6]),
        (5, &[0x45, 0x0f, 0xb6, 0xed]),
        (5, &[0x45, 0x0f, 0xb6, 0xc9]),
        (6, &[0x64, 0x4c, 0x8b, 0x20]),
        (6, &[0x64, 0x49, 0x8b, 0x5d, 0x00]),
        (7, &[0x0f, 0x94, 0x83, 0x00, 0x09, 0x00, 0x00]),
        (7, &[0x0f, 0x94, 0x85, 0x0b, 0xfb, 0xff, 0xff]),
        (8, &[0x64, 0x8b, 0x52, 0x04]),
        (8, &[0x64, 0x8b, 0x00]),
        (9, &[0x0c, 0x80]),
        (12, &[0x44, 0x88, 0xa3, 0x18, 0x06, 0x00, 0x00]),
        (13, &[0x64, 0x44, 0x8b, 0x30]),
        (13, &[0x64, 0x45, 0x8b, 0x3c, 0x24]),
        (14, &[0x64, 0x89, 0x01]),
        (15, &[0x64, 0x83, 0x39, 0x26]),
        (15, &[0x64, 0x83, 0x3c, 0x25, 0x14, 0x06, 0x00, 0x00, 0x02]),
        (16, &[0x64, 0x44, 0x89, 0x30]),
        (17, &[0x22, 0x85, 0x40, 0xfb, 0xff, 0xff]),
        (18, &[0x44, 0x22, 0xad, 0x44, 0xfb, 0xff, 0xff]),
        (19, &[0x0f, 0xbc, 0xc0]),
        (20, &[0x64, 0xf6, 0x04, 0x25, 0x0c, 0x03, 0, 0, 0x80]),
        (21, &[0x64, 0x4c, 0x39, 0x24, 0x25, 0x10, 0, 0, 0]),
        (22, &[0x81, 0xf6, 0x00, 0x01, 0, 0]),
        (23, &[0x40, 0x80, 0xce, 0x89]),
        (24, &[0x68, 0xca, 0, 0, 0]),
        (26, &[0x40, 0x80, 0xf6, 0x80]),
        (26, &[0x40, 0x80, 0xf6, 0x81]),
        (26, &[0x41, 0x80, 0xf1, 0x81]),
        (28, &[0x19, 0xc0]),
        (29, &[0x64, 0x49, 0xc7, 0x45, 0, 0, 0, 0, 0]),
        (29, &[0x64, 0x48, 0xc7, 0x00, 0, 0, 0, 0]),
        (32, &[0x48, 0x0f, 0xa3, 0xd1]),
        (34, &[0xf7, 0x77, 0x08]),
        (35, &[0x80, 0xcc, 0x08]),
        (36, &[0x66, 0xf7, 0x01, 0x02, 0x02]),
        (39, &[0x45, 0x03, 0x5a, 0x10]),
        (41, &[0x64, 0x3b, 0x04, 0x25, 0xd0, 0x02, 0, 0]),
    ];
    for (group, code) in cases {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = DATA + 0x200;
        state.gpr[Register::Rbx as usize] = DATA + 0x300;
        state.gpr[Register::Rcx as usize] = DATA + 0x400;
        state.gpr[Register::Rdx as usize] = DATA + 0x500;
        state.gpr[Register::Rdi as usize] = DATA + 0x600;
        state.gpr[Register::Rbp as usize] = DATA + 0x700;
        state.gpr[Register::R10 as usize] = DATA + 0x800;
        state.gpr[Register::R11 as usize] = 0x1234;
        state.gpr[Register::R12 as usize] = 0x1234;
        state.gpr[Register::R13 as usize] = 0x1234;
        state.gpr[Register::R14 as usize] = 0x1234;
        state.gpr[Register::R15 as usize] = 0x1234;
        state.fs_base = DATA + 0x1000;
        state.gpr[Register::Rsp as usize] = DATA + 0x100;
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            !matches!(exit(&result), RunExit::Unsupported { .. }),
            "static qualification group {group} was refused for bytes {code:02x?}: {:?}",
            exit(&result)
        );
    }
}

#[test]
fn cpl3_issue576_observed_inc_edx_zero_extends_and_keeps_carry() {
    let code = [0xff, 0xc2]; // inc edx
    let mut state = user_state();
    state.gpr[Register::Rdx as usize] = 0xffff_ffff_ffff_ffff;
    state.rflags = (state.rflags & !ARITH) | CF | OF | SF;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);

    let result = run3_data(&mut state, &mut memory, 1).unwrap();

    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 2);
    assert_eq!(state.gpr[Register::Rdx as usize], 0);
    assert_eq!(state.rflags & ARITH, CF | PF | AF | ZF);
    assert!(memory.read_widths.is_empty());
    assert!(memory.writes.is_empty());
}

#[test]
fn cpl3_busybox_byte_store_and_extension_preserve_width_and_flags() {
    let code = [
        0x45, 0x88, 0x75, 0x00, // mov [r13], r14b
        0x0f, 0xbe, 0x0a, // movsx ecx, byte ptr [rdx]
        0x45, 0x0f, 0xb6, 0xc7, // movzx r8d, r15b
    ];
    let mut state = user_state();
    state.gpr[Register::R13 as usize] = DATA;
    state.gpr[Register::Rdx as usize] = DATA;
    state.gpr[Register::R14 as usize] = 0xffff_ffff_ffff_ffa5;
    state.gpr[Register::R15 as usize] = 0xffff_ffff_ffff_ff81;
    state.gpr[Register::Rcx as usize] = u64::MAX;
    state.gpr[Register::R8 as usize] = u64::MAX;
    state.rflags |= ARITH;
    let mut expected = state.clone();
    expected.gpr[Register::Rcx as usize] = 0xffff_ffa5;
    expected.gpr[Register::R8 as usize] = 0x81;
    expected.rip += code.len() as u64;
    let mut memory = DataMemory::new(&code).with_user(DATA, 0);
    let result = run3_data(&mut state, &mut memory, 3).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state, expected);
    assert_eq!(memory.writes, [(DATA, 0xa5)]);
    assert_eq!(memory.read_widths, [(DATA, 1)]);
}

#[test]
fn cpl3_scalar_families_reject_unobserved_legacy_prefixes() {
    let code = [0x41, 0x21, 0xca]; // and r10d, ecx
    let mut initial = user_state();
    initial.gpr[Register::R10 as usize] = u64::MAX;
    initial.gpr[Register::Rcx as usize] = 0x78;
    let mut state = initial.clone();
    let mut memory = DataMemory::new(&code);
    let result = run3_data(&mut state, &mut memory, 1).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::R10 as usize], 0x78);

    for (bytes, opcode) in [
        (&[0x64, 0x41, 0x21, 0xca][..], 0x21),
        (&[0x65, 0x41, 0x21, 0xca][..], 0x21),
        (&[0x67, 0x41, 0x21, 0xca][..], 0x21),
        (&[0x66, 0x41, 0x21, 0xca][..], 0x21),
        (&[0xf2, 0x41, 0x21, 0xca][..], 0x21),
        (&[0x66, 0xb0, 0x02][..], 0xb0),
        (&[0x66, 0x40, 0x80, 0xce, 0x89][..], 0x80),
        (&[0x66, 0x41, 0xb1, 0x01][..], 0xb1),
    ] {
        let mut state = initial.clone();
        let mut memory = DataMemory::new(bytes);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, initial);
        assert!(memory.read_widths.is_empty());
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_full_system_scalar_fs_samples_are_admitted() {
    let cases: &[(&str, &[u8])] = &[
        (
            "busybox group 0: xor\tebx, dword ptr [rbp - 0x28]",
            &[0x33, 0x5d, 0xd8],
        ),
        (
            "busybox group 0: xor\teax, dword ptr [rbp - 0x28]",
            &[0x33, 0x45, 0xd8],
        ),
        (
            "busybox group 3: mov\tword ptr [rax + 0x320], 0x3d",
            &[0x66, 0xc7, 0x80, 0x20, 0x03, 0x00, 0x00, 0x3d, 0x00],
        ),
        (
            "busybox group 3: mov\tword ptr [rbx + 0x10], 0x1",
            &[0x66, 0xc7, 0x43, 0x10, 0x01, 0x00],
        ),
        (
            "busybox group 3: mov\tword ptr [rbp - 0xb4], 0x1",
            &[0x66, 0xc7, 0x85, 0x4c, 0xff, 0xff, 0xff, 0x01, 0x00],
        ),
        ("busybox group 4: mov\tbl, 0x1", &[0xb3, 0x01]),
        ("busybox group 4: mov\tbl, 0x2", &[0xb3, 0x02]),
        ("busybox group 4: mov\tbl, 0x3", &[0xb3, 0x03]),
        ("busybox group 4: mov\tbl, 0x8", &[0xb3, 0x08]),
        ("busybox group 5: mov\tal, 0x2", &[0xb0, 0x02]),
        ("busybox group 5: mov\tal, 0x3", &[0xb0, 0x03]),
        ("busybox group 5: mov\tal, 0xa", &[0xb0, 0x0a]),
        ("busybox group 5: mov\tal, 0x9", &[0xb0, 0x09]),
        ("busybox group 5: mov\tal, 0x6", &[0xb0, 0x06]),
        (
            "busybox group 6: mov\tbyte ptr [r13 + r14 + 0x5a], bl",
            &[0x43, 0x88, 0x5c, 0x35, 0x5a],
        ),
        (
            "busybox group 6: mov\tbyte ptr [rip + 0xb9ab8], dil",
            &[0x40, 0x88, 0x3d, 0xb8, 0x9a, 0x0b, 0x00],
        ),
        (
            "busybox group 6: mov\tbyte ptr [rbp - 0x60], sil",
            &[0x40, 0x88, 0x75, 0xa0],
        ),
        (
            "busybox group 6: mov\tbyte ptr [r14], bl",
            &[0x41, 0x88, 0x1e],
        ),
        (
            "busybox group 6: mov\tbyte ptr [r13], bl",
            &[0x41, 0x88, 0x5d, 0x00],
        ),
        (
            "busybox group 7: or\teax, 0x4000000",
            &[0x0d, 0x00, 0x00, 0x00, 0x04],
        ),
        (
            "busybox group 8: mov\tr9, qword ptr fs:[rax]",
            &[0x64, 0x4c, 0x8b, 0x08],
        ),
        ("busybox group 9: or\tdl, cl", &[0x08, 0xca]),
        ("busybox group 9: or\tal, dl", &[0x08, 0xd0]),
        ("busybox group 9: or\tal, cl", &[0x08, 0xc8]),
        (
            "busybox group 10: movsx\trcx, byte ptr [rax]",
            &[0x48, 0x0f, 0xbe, 0x08],
        ),
        (
            "busybox group 10: movsx\trdx, byte ptr [r14]",
            &[0x49, 0x0f, 0xbe, 0x16],
        ),
        (
            "busybox group 10: movsx\trdx, byte ptr [rdi]",
            &[0x48, 0x0f, 0xbe, 0x17],
        ),
        (
            "busybox group 11: movzx\teax, r12b",
            &[0x41, 0x0f, 0xb6, 0xc4],
        ),
        (
            "busybox group 11: movzx\tedi, dil",
            &[0x40, 0x0f, 0xb6, 0xff],
        ),
        (
            "busybox group 11: movzx\tr15d, bl",
            &[0x44, 0x0f, 0xb6, 0xfb],
        ),
        (
            "busybox group 11: movzx\tecx, sil",
            &[0x40, 0x0f, 0xb6, 0xce],
        ),
        ("busybox group 12: movzx\teax, dl", &[0x0f, 0xb6, 0xc2]),
        ("busybox group 12: movzx\tedx, al", &[0x0f, 0xb6, 0xd0]),
        ("busybox group 12: movzx\teax, dh", &[0x0f, 0xb6, 0xc6]),
        ("busybox group 12: movzx\tebx, bl", &[0x0f, 0xb6, 0xdb]),
        ("busybox group 12: movzx\teax, bl", &[0x0f, 0xb6, 0xc3]),
        ("busybox group 13: inc\tr9d", &[0x41, 0xff, 0xc1]),
        ("busybox group 13: inc\tr12d", &[0x41, 0xff, 0xc4]),
        ("busybox group 13: inc\tr14d", &[0x41, 0xff, 0xc6]),
        (
            "busybox group 14: mov\tbyte ptr [rcx - 0x1], al",
            &[0x88, 0x41, 0xff],
        ),
        ("busybox group 14: mov\tbyte ptr [rdx], al", &[0x88, 0x02]),
        ("busybox group 14: mov\tbyte ptr [rdx], ah", &[0x88, 0x22]),
        (
            "busybox group 14: mov\tbyte ptr [rbx + 0x4d], al",
            &[0x88, 0x43, 0x4d],
        ),
        ("busybox group 17: neg\tecx", &[0xf7, 0xd9]),
        ("busybox group 17: neg\teax", &[0xf7, 0xd8]),
        ("busybox group 17: neg\tedx", &[0xf7, 0xda]),
        (
            "busybox group 18: or\tesi, 0x100",
            &[0x81, 0xce, 0x00, 0x01, 0x00, 0x00],
        ),
        (
            "busybox group 20: mov\tebx, dword ptr fs:[rdi]",
            &[0x64, 0x8b, 0x1f],
        ),
        (
            "busybox group 20: mov\teax, dword ptr fs:[rax]",
            &[0x64, 0x8b, 0x00],
        ),
        (
            "busybox group 20: mov\tebx, dword ptr fs:[rax]",
            &[0x64, 0x8b, 0x18],
        ),
        (
            "busybox group 21: mov\tdword ptr fs:[rax], ebx",
            &[0x64, 0x89, 0x18],
        ),
        (
            "busybox group 21: mov\tdword ptr fs:[rdx], eax",
            &[0x64, 0x89, 0x02],
        ),
        (
            "busybox group 21: mov\tdword ptr fs:[rax], edx",
            &[0x64, 0x89, 0x10],
        ),
        (
            "busybox group 21: mov\tdword ptr fs:[rcx], eax",
            &[0x64, 0x89, 0x01],
        ),
        (
            "busybox group 24: push\t0x101",
            &[0x68, 0x01, 0x01, 0x00, 0x00],
        ),
        (
            "busybox group 24: push\t0xe6",
            &[0x68, 0xe6, 0x00, 0x00, 0x00],
        ),
        (
            "busybox group 25: movsx\teax, byte ptr [rip + 0xbf679]",
            &[0x0f, 0xbe, 0x05, 0x79, 0xf6, 0x0b, 0x00],
        ),
        (
            "busybox group 25: movsx\teax, byte ptr [rip + 0xc01e1]",
            &[0x0f, 0xbe, 0x05, 0xe1, 0x01, 0x0c, 0x00],
        ),
        (
            "busybox group 25: movsx\tedi, byte ptr [rip + 0xbac5a]",
            &[0x0f, 0xbe, 0x3d, 0x5a, 0xac, 0x0b, 0x00],
        ),
        (
            "busybox group 25: movsx\teax, byte ptr [rip + 0xbab51]",
            &[0x0f, 0xbe, 0x05, 0x51, 0xab, 0x0b, 0x00],
        ),
        (
            "busybox group 25: movsx\teax, byte ptr [rip + 0xbac64]",
            &[0x0f, 0xbe, 0x05, 0x64, 0xac, 0x0b, 0x00],
        ),
        ("busybox group 26: dec\tbl", &[0xfe, 0xcb]),
        ("busybox group 26: dec\tal", &[0xfe, 0xc8]),
        ("busybox group 26: dec\tdl", &[0xfe, 0xca]),
        (
            "busybox group 27: movsx\tr12d, byte ptr [rip + 0xbab81]",
            &[0x44, 0x0f, 0xbe, 0x25, 0x81, 0xab, 0x0b, 0x00],
        ),
        (
            "busybox group 27: movsx\teax, byte ptr [rax + r12]",
            &[0x42, 0x0f, 0xbe, 0x04, 0x20],
        ),
        (
            "busybox group 27: movsx\tedx, r12b",
            &[0x41, 0x0f, 0xbe, 0xd4],
        ),
        ("busybox group 28: dec\trbx", &[0x48, 0xff, 0xcb]),
        ("busybox group 28: dec\trax", &[0x48, 0xff, 0xc8]),
        ("busybox group 28: dec\trsi", &[0x48, 0xff, 0xce]),
        (
            "busybox group 31: sete\tbyte ptr [rbp - 0x83]",
            &[0x0f, 0x94, 0x85, 0x7d, 0xff, 0xff, 0xff],
        ),
        (
            "busybox group 31: sete\tbyte ptr [rbp - 0x111]",
            &[0x0f, 0x94, 0x85, 0xef, 0xfe, 0xff, 0xff],
        ),
        (
            "busybox group 31: sete\tbyte ptr [rbp - 0x99]",
            &[0x0f, 0x94, 0x85, 0x67, 0xff, 0xff, 0xff],
        ),
        (
            "busybox group 31: sete\tbyte ptr [rbp - 0x4f5]",
            &[0x0f, 0x94, 0x85, 0x0b, 0xfb, 0xff, 0xff],
        ),
        ("busybox group 34: mov\tdl, 0x3", &[0xb2, 0x03]),
        ("busybox group 34: mov\tdl, 0x2", &[0xb2, 0x02]),
        ("busybox group 34: mov\tdl, -0x7c", &[0xb2, 0x84]),
        ("busybox group 36: and\tr15d, eax", &[0x41, 0x21, 0xc7]),
        ("busybox group 36: and\tr14d, ebx", &[0x41, 0x21, 0xde]),
        ("busybox group 36: and\tecx, r14d", &[0x44, 0x21, 0xf1]),
        ("busybox group 36: and\tr14d, eax", &[0x41, 0x21, 0xc6]),
        ("busybox group 36: and\tr12d, edx", &[0x41, 0x21, 0xd4]),
        (
            "busybox group 37: sub\tr12d, dword ptr [rax + 0x8]",
            &[0x44, 0x2b, 0x60, 0x08],
        ),
        ("busybox group 38: or\tcl, r9b", &[0x44, 0x08, 0xc9]),
        ("busybox group 38: or\tal, sil", &[0x40, 0x08, 0xf0]),
        (
            "busybox group 41: test\tbyte ptr [rbp - 0x111], dl",
            &[0x84, 0x95, 0xef, 0xfe, 0xff, 0xff],
        ),
        ("busybox group 43: adc\trsi, 0x0", &[0x48, 0x83, 0xd6, 0x00]),
        ("busybox group 43: adc\trax, 0x0", &[0x48, 0x83, 0xd0, 0x00]),
        (
            "busybox group 43: adc\trax, -0x1",
            &[0x48, 0x83, 0xd0, 0xff],
        ),
        ("busybox group 44: and\teax, ebx", &[0x21, 0xd8]),
        ("busybox group 44: and\tesi, ecx", &[0x21, 0xce]),
        ("busybox group 44: and\tedi, ecx", &[0x21, 0xcf]),
        ("busybox group 44: and\tedx, ebx", &[0x21, 0xda]),
        ("busybox group 44: and\tedx, esi", &[0x21, 0xf2]),
        (
            "busybox group 46: and\tax, 0xfb00",
            &[0x66, 0x25, 0x00, 0xfb],
        ),
        ("busybox group 47: or\tax, 0x401", &[0x66, 0x0d, 0x01, 0x04]),
        ("busybox group 48: mov\tr13b, 0x1", &[0x41, 0xb5, 0x01]),
        ("busybox group 48: mov\tr13b, 0xa", &[0x41, 0xb5, 0x0a]),
        ("busybox group 49: mov\tr9b, 0x1", &[0x41, 0xb1, 0x01]),
        (
            "busybox group 52: mov\tr15d, dword ptr fs:[r12]",
            &[0x64, 0x45, 0x8b, 0x3c, 0x24],
        ),
        (
            "busybox group 53: or\tdil, -0x80",
            &[0x40, 0x80, 0xcf, 0x80],
        ),
        (
            "busybox group 55: test\trsi, -0x40",
            &[0x48, 0xf7, 0xc6, 0xc0, 0xff, 0xff, 0xff],
        ),
        (
            "busybox group 57: and\trsi, -0x1000",
            &[0x48, 0x81, 0xe6, 0x00, 0xf0, 0xff, 0xff],
        ),
        (
            "busybox group 57: and\trbx, -0x78000001",
            &[0x48, 0x81, 0xe3, 0xff, 0xff, 0xff, 0x87],
        ),
        (
            "busybox group 57: and\trcx, -0x1000",
            &[0x48, 0x81, 0xe1, 0x00, 0xf0, 0xff, 0xff],
        ),
        ("busybox group 58: sub\tcl, dl", &[0x28, 0xd1]),
        (
            "busybox group 59: cmp\tedx, dword ptr [rbx + 0x18]",
            &[0x3b, 0x53, 0x18],
        ),
        (
            "busybox group 59: cmp\tebx, dword ptr [rip + 0xb60d7]",
            &[0x3b, 0x1d, 0xd7, 0x60, 0x0b, 0x00],
        ),
        (
            "busybox group 59: cmp\tebx, dword ptr [rax + 0x20]",
            &[0x3b, 0x58, 0x20],
        ),
        (
            "busybox group 59: cmp\tedx, dword ptr [rip + 0xb5fe3]",
            &[0x3b, 0x15, 0xe3, 0x5f, 0x0b, 0x00],
        ),
        (
            "busybox group 59: cmp\tedx, dword ptr [rax + 0xc]",
            &[0x3b, 0x50, 0x0c],
        ),
        (
            "busybox group 60: sub\tebx, dword ptr [rip + 0xf700f]",
            &[0x2b, 0x1d, 0x0f, 0x70, 0x0f, 0x00],
        ),
        (
            "busybox group 60: sub\teax, dword ptr [rcx + 0x8]",
            &[0x2b, 0x41, 0x08],
        ),
        (
            "busybox group 60: sub\tesi, dword ptr [rax + 0x8]",
            &[0x2b, 0x70, 0x08],
        ),
        (
            "busybox group 60: sub\tedx, dword ptr [rax + 0x8]",
            &[0x2b, 0x50, 0x08],
        ),
        (
            "busybox group 61: movsxd\trax, dword ptr [rip + 0x89308]",
            &[0x48, 0x63, 0x05, 0x08, 0x93, 0x08, 0x00],
        ),
        (
            "busybox group 61: movsxd\trax, dword ptr [rip + 0xa08f0]",
            &[0x48, 0x63, 0x05, 0xf0, 0x08, 0x0a, 0x00],
        ),
        (
            "busybox group 61: movsxd\trax, dword ptr [rip + 0x89205]",
            &[0x48, 0x63, 0x05, 0x05, 0x92, 0x08, 0x00],
        ),
        (
            "busybox group 61: movsxd\trax, dword ptr [rip + 0x85511]",
            &[0x48, 0x63, 0x05, 0x11, 0x55, 0x08, 0x00],
        ),
        (
            "busybox group 61: movsxd\trax, dword ptr [rip + 0x9f129]",
            &[0x48, 0x63, 0x05, 0x29, 0xf1, 0x09, 0x00],
        ),
        ("busybox group 62: xor\tal, 0x1", &[0x34, 0x01]),
        ("busybox group 63: inc\tedx", &[0xff, 0xc2]),
        ("busybox group 63: inc\teax", &[0xff, 0xc0]),
        (
            "busybox group 64: setne\tbyte ptr [rbp - 0x39]",
            &[0x0f, 0x95, 0x45, 0xc7],
        ),
        (
            "busybox group 64: setne\tbyte ptr [rbp - 0x12f]",
            &[0x0f, 0x95, 0x85, 0xd1, 0xfe, 0xff, 0xff],
        ),
        (
            "busybox group 64: setne\tbyte ptr [rbp - 0xbd]",
            &[0x0f, 0x95, 0x85, 0x43, 0xff, 0xff, 0xff],
        ),
        ("busybox group 68: xor\tecx, 0x1", &[0x83, 0xf1, 0x01]),
        ("busybox group 68: xor\tebx, 0x2", &[0x83, 0xf3, 0x02]),
        ("busybox group 68: xor\teax, 0x1", &[0x83, 0xf0, 0x01]),
        (
            "busybox group 69: cmp\tcl, byte ptr [rsi + rax]",
            &[0x3a, 0x0c, 0x06],
        ),
        (
            "busybox group 69: cmp\tdl, byte ptr [rbp - 0x12e]",
            &[0x3a, 0x95, 0xd2, 0xfe, 0xff, 0xff],
        ),
        (
            "busybox group 70: xor\tr12d, 0x1",
            &[0x41, 0x83, 0xf4, 0x01],
        ),
        (
            "busybox group 71: cmp\tedx, dword ptr [r12 + 8*rax + 0x8]",
            &[0x41, 0x3b, 0x54, 0xc4, 0x08],
        ),
        (
            "busybox group 71: cmp\tebx, dword ptr [r15 + 0xc]",
            &[0x41, 0x3b, 0x5f, 0x0c],
        ),
        (
            "busybox group 71: cmp\tebx, dword ptr [r12 + 8*r15 + 0x10]",
            &[0x43, 0x3b, 0x5c, 0xfc, 0x10],
        ),
        ("busybox group 74: neg\tr12d", &[0x41, 0xf7, 0xdc]),
        ("busybox group 77: or\tdh, 0x2", &[0x80, 0xce, 0x02]),
        ("busybox group 77: or\tah, 0x8", &[0x80, 0xcc, 0x08]),
        ("busybox group 78: sbb\tr12d, r12d", &[0x45, 0x19, 0xe4]),
        ("busybox group 79: dec\tr9b", &[0x41, 0xfe, 0xc9]),
        ("busybox group 82: sbb\teax, eax", &[0x19, 0xc0]),
        (
            "busybox group 89: cmp\trax, qword ptr [rdi + rax]",
            &[0x48, 0x3b, 0x04, 0x07],
        ),
        (
            "busybox group 90: and\tal, byte ptr [rbp - 0x4c0]",
            &[0x22, 0x85, 0x40, 0xfb, 0xff, 0xff],
        ),
        (
            "busybox group 91: and\tr13b, byte ptr [rbp - 0x4bc]",
            &[0x44, 0x22, 0xad, 0x44, 0xfb, 0xff, 0xff],
        ),
        ("busybox group 93: and\tal, r8b", &[0x44, 0x20, 0xc0]),
        (
            "busybox group 98: add\tecx, dword ptr [rdx + 0x20]",
            &[0x03, 0x4a, 0x20],
        ),
        (
            "busybox group 99: add\teax, dword ptr [rdx + 8*r10 + 0x4]",
            &[0x42, 0x03, 0x44, 0xd2, 0x04],
        ),
        (
            "busybox group 99: add\teax, dword ptr [rbp + 4*r10 - 0x138]",
            &[0x42, 0x03, 0x84, 0x95, 0xc8, 0xfe, 0xff, 0xff],
        ),
        (
            "busybox group 102: test\tword ptr [rcx], 0x202",
            &[0x66, 0xf7, 0x01, 0x02, 0x02],
        ),
        (
            "busybox group 104: sub\teax, 0x88",
            &[0x2d, 0x88, 0x00, 0x00, 0x00],
        ),
        ("busybox group 105: mov\tcl, 0x61", &[0xb1, 0x61]),
        (
            "busybox group 106: xor\tr8, qword ptr fs:[0x30]",
            &[0x64, 0x4c, 0x33, 0x04, 0x25, 0x30, 0x00, 0x00, 0x00],
        ),
        (
            "busybox group 106: xor\tr9, qword ptr fs:[0x30]",
            &[0x64, 0x4c, 0x33, 0x0c, 0x25, 0x30, 0x00, 0x00, 0x00],
        ),
        (
            "workload group 0: mov\tbyte ptr [rip + 0xa2790], bl",
            &[0x88, 0x1d, 0x90, 0x27, 0x0a, 0x00],
        ),
        (
            "workload group 0: mov\tbyte ptr [rdx + 0x76], al",
            &[0x88, 0x42, 0x76],
        ),
        ("workload group 0: mov\tbyte ptr [rcx], al", &[0x88, 0x01]),
        (
            "workload group 0: mov\tbyte ptr [rbp - 0x4c0], al",
            &[0x88, 0x85, 0x40, 0xfb, 0xff, 0xff],
        ),
        ("workload group 0: mov\tbyte ptr [rdx], bl", &[0x88, 0x1a]),
        ("workload group 8: and\tr10d, ecx", &[0x41, 0x21, 0xca]),
        ("workload group 8: and\tr11d, esi", &[0x41, 0x21, 0xf3]),
        ("workload group 8: and\teax, r15d", &[0x44, 0x21, 0xf8]),
        ("workload group 10: neg\tebx", &[0xf7, 0xdb]),
        (
            "workload group 11: movzx\tr15d, r15b",
            &[0x45, 0x0f, 0xb6, 0xff],
        ),
        (
            "workload group 11: movzx\teax, r14b",
            &[0x41, 0x0f, 0xb6, 0xc6],
        ),
        (
            "workload group 11: movzx\tr13d, r13b",
            &[0x45, 0x0f, 0xb6, 0xed],
        ),
        (
            "workload group 11: movzx\tr9d, r9b",
            &[0x45, 0x0f, 0xb6, 0xc9],
        ),
        (
            "workload group 12: mov\tr12, qword ptr fs:[rax]",
            &[0x64, 0x4c, 0x8b, 0x20],
        ),
        (
            "workload group 12: mov\trbx, qword ptr fs:[r13]",
            &[0x64, 0x49, 0x8b, 0x5d, 0x00],
        ),
        (
            "workload group 13: sete\tbyte ptr [rbx + 0x900]",
            &[0x0f, 0x94, 0x83, 0x00, 0x09, 0x00, 0x00],
        ),
        (
            "workload group 14: mov\tedx, dword ptr fs:[rdx + 0x4]",
            &[0x64, 0x8b, 0x52, 0x04],
        ),
        ("workload group 15: or\tal, -0x80", &[0x0c, 0x80]),
        (
            "workload group 18: mov\tbyte ptr [rbx + 0x618], r12b",
            &[0x44, 0x88, 0xa3, 0x18, 0x06, 0x00, 0x00],
        ),
        (
            "workload group 19: mov\tr14d, dword ptr fs:[rax]",
            &[0x64, 0x44, 0x8b, 0x30],
        ),
        (
            "workload group 21: cmp\tqword ptr fs:[0x10], r12",
            &[0x64, 0x4c, 0x39, 0x24, 0x25, 0x10, 0x00, 0x00, 0x00],
        ),
        (
            "workload group 22: xor\tesi, 0x100",
            &[0x81, 0xf6, 0x00, 0x01, 0x00, 0x00],
        ),
        (
            "workload group 23: or\tsil, -0x77",
            &[0x40, 0x80, 0xce, 0x89],
        ),
        (
            "workload group 24: push\t0xca",
            &[0x68, 0xca, 0x00, 0x00, 0x00],
        ),
        (
            "workload group 28: test\tbyte ptr fs:[0x30c], -0x80",
            &[0x64, 0xf6, 0x04, 0x25, 0x0c, 0x03, 0x00, 0x00, 0x80],
        ),
        (
            "workload group 30: xor\tsil, -0x7f",
            &[0x40, 0x80, 0xf6, 0x81],
        ),
        (
            "workload group 30: xor\tsil, -0x80",
            &[0x40, 0x80, 0xf6, 0x80],
        ),
        (
            "workload group 30: xor\tr9b, -0x7f",
            &[0x41, 0x80, 0xf1, 0x81],
        ),
        ("workload group 32: sbb\tedx, edx", &[0x19, 0xd2]),
        (
            "workload group 33: mov\tqword ptr fs:[r13], 0x0",
            &[0x64, 0x49, 0xc7, 0x45, 0x00, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            "workload group 33: mov\tqword ptr fs:[rax], 0x0",
            &[0x64, 0x48, 0xc7, 0x00, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            "workload group 38: div\tdword ptr [rdi + 0x8]",
            &[0xf7, 0x77, 0x08],
        ),
        (
            "workload group 45: add\tr11d, dword ptr [r10 + 0x10]",
            &[0x45, 0x03, 0x5a, 0x10],
        ),
        (
            "workload group 47: cmp\teax, dword ptr fs:[0x2d0]",
            &[0x64, 0x3b, 0x04, 0x25, 0xd0, 0x02, 0x00, 0x00],
        ),
        (
            "workload group 48: cmp\tdword ptr fs:[0x614], 0x2",
            &[0x64, 0x83, 0x3c, 0x25, 0x14, 0x06, 0x00, 0x00, 0x02],
        ),
    ];
    for (form, code) in cases {
        let mut state = user_state();
        state.gpr[Register::Rax as usize] = DATA + 0x200;
        state.gpr[Register::Rbx as usize] = DATA + 0x300;
        state.gpr[Register::Rcx as usize] = DATA + 0x400;
        state.gpr[Register::Rdx as usize] = DATA + 0x500;
        state.gpr[Register::Rsi as usize] = DATA + 0x600;
        state.gpr[Register::Rdi as usize] = DATA + 0x700;
        state.gpr[Register::Rbp as usize] = DATA + 0x800;
        state.gpr[Register::Rsp as usize] = DATA + 0x100;
        for register in [
            Register::R8,
            Register::R9,
            Register::R10,
            Register::R11,
            Register::R12,
            Register::R13,
            Register::R14,
            Register::R15,
        ] {
            state.gpr[register as usize] = DATA + 0x900;
        }
        state.fs_base = DATA + 0x1000;
        let mut memory = DataMemory::new(code);
        let result = run3_data(&mut state, &mut memory, 1).unwrap();
        assert!(
            !matches!(exit(&result), RunExit::Unsupported { .. }),
            "required sample {form} was refused for bytes {code:02x?}: {:?}",
            exit(&result)
        );
    }
}

#[test]
fn cpl3_issue576_observed_vector_register_forms_use_the_guest_fp_cell() {
    let run = |code: &[u8], mut fp_state: od_isa::GuestFpState| {
        let mut state = user_state();
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        let mut memory = DataMemory::new(code);
        let fp = std::cell::RefCell::new(fp_state.clone());
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert!(memory.writes.is_empty());
        fp_state = fp.into_inner();
        fp_state
    };

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[0] = [0x10; 16];
    let fp = run(&[0x0f, 0x28, 0xc8], initial); // MOVAPS xmm1, xmm0
    assert_eq!(fp.sse.xmm[1], [0x10; 16]);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[8] = [0x80; 16];
    initial.sse.xmm[6] = [0x7f; 16];
    let fp = run(&[0x66, 0x44, 0x0f, 0x64, 0xc6], initial); // PCMPGTB xmm8, xmm6
    assert_eq!(fp.sse.xmm[8], [0; 16]);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[8] = [0xf0; 16];
    initial.sse.xmm[7] = [0x5a; 16];
    let fp = run(&[0x66, 0x44, 0x0f, 0xdf, 0xc7], initial); // PANDN xmm8, xmm7
    assert_eq!(fp.sse.xmm[8], [0x0a; 16]);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[0] = [0x40; 16];
    initial.sse.xmm[1] = [0x80; 16];
    let fp = run(&[0x66, 0x0f, 0xda, 0xc1], initial); // PMINUB xmm0, xmm1
    assert_eq!(fp.sse.xmm[0], [0x40; 16]);

    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[1] = [0x5a; 16];
    initial.sse.xmm[0] = [0xff; 16];
    let fp = run(&[0x0f, 0x57, 0xc1], initial); // XORPS xmm0, xmm1
    assert_eq!(fp.sse.xmm[0], [0xa5; 16]);
}

#[test]
fn cpl3_issue576_observed_scalar_double_forms_reuse_configured_semantics() {
    let run = |code: &[u8], mut state: LongModeState, mut fp_state: od_isa::GuestFpState| {
        state.cr0 = Some(0x8005_0033);
        state.cr4 = Some(0x6b0);
        let mut memory = DataMemory::new(code);
        let fp = std::cell::RefCell::new(fp_state.clone());
        let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
        assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, RIP + code.len() as u64);
        assert!(memory.writes.is_empty());
        fp_state = fp.into_inner();
        (state, fp_state)
    };
    let mut base = user_state();
    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[0][..8].copy_from_slice(&1.5f64.to_bits().to_le_bytes());
    initial.sse.xmm[1][..8].copy_from_slice(&2.0f64.to_bits().to_le_bytes());
    let (_, fp) = run(&[0xf2, 0x0f, 0x58, 0xc1], base.clone(), initial.clone());
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(fp.sse.xmm[0][..8].try_into().unwrap())),
        3.5
    );
    let (_, fp) = run(&[0xf2, 0x0f, 0x59, 0xc1], base.clone(), initial.clone());
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(fp.sse.xmm[0][..8].try_into().unwrap())),
        3.0
    );
    let (_, fp) = run(&[0xf2, 0x0f, 0x5c, 0xc1], base.clone(), initial.clone());
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(fp.sse.xmm[0][..8].try_into().unwrap())),
        -0.5
    );

    // COMISD sets arithmetic flags from the ordered comparison.
    let (state, _) = run(&[0x66, 0x0f, 0x2f, 0xc1], base.clone(), initial.clone());
    assert_eq!(state.rflags & ARITH, 1); // CF: 1.5 < 2.0

    base.gpr[Register::Rax as usize] = 7;
    let (_, fp) = run(
        &[0xf2, 0x48, 0x0f, 0x2a, 0xc0],
        base.clone(),
        initial.clone(),
    );
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(fp.sse.xmm[0][..8].try_into().unwrap())),
        7.0
    );

    let mut to_integer = initial;
    to_integer.sse.xmm[0][..8].copy_from_slice(&(-3.75f64).to_bits().to_le_bytes());
    let (state, _) = run(&[0xf2, 0x48, 0x0f, 0x2c, 0xc0], base, to_integer);
    assert_eq!(state.gpr[Register::Rax as usize] as i64, -3);
}

#[test]
fn cpl3_issue576_observed_vector_memory_forms_validate_width_and_commit_after_reads() {
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rdi as usize] = DATA - 16;
    let movaps = [0x0f, 0x28, 0x57, 0x10]; // MOVAPS xmm2, [rdi+16]
    let mut fp_state = od_isa::GuestFpState::initial();
    fp_state.sse.xmm[2] = [0xa5; 16];
    let fp = std::cell::RefCell::new(fp_state);
    let mut memory = DataMemory::new(&movaps);
    for (i, byte) in [0x11u8; 16].into_iter().enumerate() {
        memory = memory.with_user(DATA + i as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(fp.borrow().sse.xmm[2], [0x11; 16]);
    assert_eq!(memory.read_widths, [(DATA, 16)]);

    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rax as usize] = DATA - 0x50;
    let pminub = [0x66, 0x0f, 0xda, 0x40, 0x50]; // PMINUB xmm0, [rax+0x50] (aligned)
    let mut fp_state = od_isa::GuestFpState::initial();
    fp_state.sse.xmm[0] = [0x7f; 16];
    let fp = std::cell::RefCell::new(fp_state);
    let mut memory = DataMemory::new(&pminub);
    for (i, byte) in [0x20u8; 16].into_iter().enumerate() {
        memory = memory.with_user(DATA + i as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(fp.borrow().sse.xmm[0], [0x20; 16]);
    assert_eq!(memory.read_widths, [(DATA, 16)]);
}

#[test]
fn cpl3_issue576_movaps_alignment_fault_precedes_read_and_xmm_commit() {
    let code = [0x0f, 0x28, 0x57, 0x10]; // MOVAPS xmm2, [rdi+0x10]
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rdi as usize] = DATA + 1 - 0x10;
    let before = state.clone();
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[2] = [0x5a; 16];
    let fp = std::cell::RefCell::new(initial_fp.clone());
    let mut memory = DataMemory::new(&code);
    for offset in 0..16 {
        memory = memory.with_user(DATA + 1 + offset, 0x33);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None
        })
    );
    assert_eq!(state, before);
    assert_eq!(*fp.borrow(), initial_fp);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_busybox_movsd_reads_rip_relative_qword_and_clears_the_upper_lane() {
    let code = [0xf2, 0x0f, 0x10, 0x0d, 0x5b, 0x41, 0x02, 0x00];
    let address = RIP + code.len() as u64 + 0x2415b;
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let mut expected = state.clone();
    expected.rip += code.len() as u64;
    let mut initial_fp = od_isa::GuestFpState::initial();
    initial_fp.sse.xmm[1] = [0xff; 16];
    let fp = std::cell::RefCell::new(initial_fp);
    let mut memory = DataMemory::new(&code);
    let bytes = 0x0123_4567_89ab_cdef_u64.to_le_bytes();
    for (offset, byte) in bytes.into_iter().enumerate() {
        memory = memory.with_user(address + offset as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state, expected);
    assert_eq!(memory.read_widths, [(address, 8)]);
    assert_eq!(&fp.borrow().sse.xmm[1][..8], &bytes);
    assert_eq!(&fp.borrow().sse.xmm[1][8..], &[0; 8]);
}

#[test]
fn cpl3_issue576_mulsd_rip_memory_uses_live_fp_state_and_checks_read_first() {
    let target = DATA;
    let displacement = (target as i64 - (RIP + 8) as i64) as i32;
    let mut code = vec![0xf2, 0x0f, 0x59, 0x05]; // MULSD xmm0, [rip+disp32]
    code.extend_from_slice(&displacement.to_le_bytes());
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let mut fp_state = od_isa::GuestFpState::initial();
    fp_state.sse.xmm[0][..8].copy_from_slice(&1.5f64.to_bits().to_le_bytes());
    let fp = std::cell::RefCell::new(fp_state.clone());
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 2.0f64.to_bits().to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(target + offset as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(memory.read_widths, [(target, 8)]);
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(
            fp.borrow().sse.xmm[0][..8].try_into().unwrap()
        )),
        3.0
    );

    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let before = state.clone();
    let initial_fp = od_isa::GuestFpState::initial();
    let fp = std::cell::RefCell::new(initial_fp.clone());
    let mut protected = DataMemory::new(&code);
    for offset in 0..8 {
        protected = protected.with_supervisor(target + offset, 0);
    }
    let result = run3_with_guest_fp(&mut state, &mut protected, &fp, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(target)
        })
    );
    assert_eq!(state, before);
    assert_eq!(*fp.borrow(), initial_fp);
    assert!(protected.writes.is_empty());
}

#[test]
fn cpl3_issue576_movsd_memory_forms_transfer_only_the_low_qword() {
    let target = DATA;
    let load = [0xf2, 0x0f, 0x10, 0x4d, 0xe8]; // MOVSD xmm1, [rbp-0x18]
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rbp as usize] = target + 0x18;
    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[1] = [0xa5; 16];
    let fp = std::cell::RefCell::new(initial);
    let mut memory = DataMemory::new(&load);
    for (offset, byte) in 0x0123_4567_89ab_cdefu64
        .to_le_bytes()
        .into_iter()
        .enumerate()
    {
        memory = memory.with_user(target + offset as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(
        fp.borrow().sse.xmm[1],
        [
            0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    assert_eq!(memory.read_widths, [(target, 8)]);

    let store = [0xf2, 0x0f, 0x11, 0x4d, 0xe8]; // MOVSD [rbp-0x18], xmm1
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rbp as usize] = target + 0x18;
    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.xmm[1] = [0x5a; 16];
    let fp = std::cell::RefCell::new(initial);
    let mut memory = DataMemory::new(&store);
    for offset in 0..8 {
        memory = memory.with_user(target + offset, 0);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(
        memory.writes,
        (0..8)
            .map(|offset| (target + offset, 0x5a))
            .collect::<Vec<_>>()
    );
    assert_eq!(fp.borrow().sse.xmm[1], [0x5a; 16]);
}

#[test]
fn cpl3_issue576_pminub_alignment_fault_precedes_read() {
    let code = [0x66, 0x0f, 0xda, 0x40, 0x50]; // PMINUB xmm0, [rax+0x50]
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    state.gpr[Register::Rax as usize] = DATA + 1 - 0x50;
    let before = state.clone();
    let mut fp_state = od_isa::GuestFpState::initial();
    fp_state.sse.xmm[0] = [0x77; 16];
    let fp = std::cell::RefCell::new(fp_state.clone());
    let mut memory = DataMemory::new(&code);
    for offset in 0..16 {
        memory = memory.with_user(DATA + 1 + offset, 0x33);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(state, before);
    assert_eq!(*fp.borrow(), fp_state);
    assert_eq!(memory.reads, 0);
}

#[test]
fn cpl3_issue576_mulsd_unmasked_invalid_updates_mxcsr_before_simd_fault() {
    let target = DATA;
    let displacement = (target as i64 - (RIP + 8) as i64) as i32;
    let mut code = vec![0xf2, 0x0f, 0x59, 0x05];
    code.extend_from_slice(&displacement.to_le_bytes());
    let mut state = user_state();
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let before = state.clone();
    let mut initial = od_isa::GuestFpState::initial();
    initial.sse.mxcsr &= !(1 << 7); // unmask invalid-operation
    initial.sse.xmm[0][..8].copy_from_slice(&f64::INFINITY.to_bits().to_le_bytes());
    let fp = std::cell::RefCell::new(initial.clone());
    let mut memory = DataMemory::new(&code);
    for (offset, byte) in 0f64.to_bits().to_le_bytes().into_iter().enumerate() {
        memory = memory.with_user(target + offset as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(
        exit(&result),
        &RunExit::Fault(ArchitecturalFault {
            vector: 19,
            error_code: None,
            linear_address: None,
        })
    );
    assert_eq!(state, before);
    assert_eq!(fp.borrow().sse.xmm[0], initial.sse.xmm[0]);
    assert_eq!(fp.borrow().sse.mxcsr, initial.sse.mxcsr | 1);
    assert_eq!(memory.read_widths, [(target, 8)]);
}

#[test]
fn generalized_movsd_memory_decode_keeps_the_legacy_nonprofile_boundary() {
    let code = [0xf2, 0x0f, 0x10, 0x4d, 0xe8]; // MOVSD xmm1, [rbp-0x18]
    let mut state = user_state();
    state.x87 = Some(od_isa::state::X87State::fixture_seed());
    state.sse = Some(od_isa::state::SseState {
        xmm: [[0xa5; 16]; 16],
        mxcsr: 0x1f80,
    });
    state.gpr[Register::Rbp as usize] = DATA + 0x18;
    let before = state.clone();
    let mut memory = DataMemory::new(&code);
    for offset in 0..8 {
        memory = memory.with_user(DATA + offset, 0x11);
    }
    let result = od_isa::execute::run(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        || false,
    );
    assert_eq!(result.instructions_executed, 0);
    assert!(
        matches!(result.exit, RunExit::Unsupported { .. }),
        "{:?}",
        result.exit
    );
    assert_eq!(state, before);
    assert_eq!(memory.reads, 0);
    assert!(memory.writes.is_empty());
}

#[test]
fn newly_decoded_movsd_rax_zero_displacements_stay_outside_the_legacy_profile() {
    for code in [
        &[0xf2, 0x0f, 0x10, 0x40, 0x00][..],
        &[0xf2, 0x0f, 0x10, 0x80, 0, 0, 0, 0][..],
    ] {
        let mut state = user_state();
        state.x87 = Some(od_isa::state::X87State::fixture_seed());
        state.sse = Some(od_isa::state::SseState {
            xmm: [[0xa5; 16]; 16],
            mxcsr: 0x1f80,
        });
        state.gpr[Register::Rax as usize] = DATA;
        let before = state.clone();
        let mut memory = DataMemory::new(code);
        for offset in 0..8 {
            memory = memory.with_user(DATA + offset, 0x11);
        }
        let result = od_isa::execute::run(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(result.exit, RunExit::Unsupported { .. }));
        assert_eq!(state, before);
        assert_eq!(memory.reads, 0);
        assert!(memory.writes.is_empty());
    }
}

#[test]
fn cpl3_issue576_observed_rip_relative_movsd_load_uses_the_vcpu_fp_cell() {
    let code = [0xf2, 0x0f, 0x10, 0x0d, 0x5b, 0x41, 0x02, 0x00];
    let mut state = user_state();
    state.rip = 0x005a_fb4d;
    state.cr0 = Some(0x8005_0033);
    state.cr4 = Some(0x6b0);
    let target = state.rip + code.len() as u64 + 0x0002_415b;
    let mut fp_state = od_isa::GuestFpState::initial();
    fp_state.sse.xmm[1] = [0xa5; 16];
    let fp = std::cell::RefCell::new(fp_state);
    let mut memory = DataMemory::new(&code);
    memory.code_base = state.rip;
    for (offset, byte) in 0x0123_4567_89ab_cdefu64
        .to_le_bytes()
        .into_iter()
        .enumerate()
    {
        memory = memory.with_user(target + offset as u64, byte);
    }
    let result = run3_with_guest_fp(&mut state, &mut memory, &fp, 1);
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(
        fp.borrow().sse.xmm[1],
        [
            0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    assert_eq!(memory.read_widths, [(target, 8)]);
}
