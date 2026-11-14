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
}

impl Memory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            fetches: 0,
            accesses: 0,
            supervisor_only_code: false,
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
        // Forms outside the allow-list. `xor eax, eax` / `xor r10d, r10d` / `xor r8d, r8d` of
        // `/init` (`31 c0`, `45 31 d2`, `45 31 c0`) are allowed since S4-39f2 (register form,
        // dword) and `mov $imm32, %rdi` (`48 c7 c7`) since S4-39f3; the variants below stay
        // outside: XOR with REX.W, a memory operand and the other XOR opcodes.
        ("xor rax, rax (REX.W)", &[0x48, 0x31, 0xc0], 0x31),
        ("xor r10, r10 (REX.W+R+B)", &[0x4d, 0x31, 0xd2], 0x31),
        ("xor [rax], eax (memory)", &[0x31, 0x00], 0x31),
        (
            "xor [rip+0], r10d (memory)",
            &[0x44, 0x31, 0x15, 0, 0, 0, 0],
            0x31,
        ),
        ("xor ax, ax (0x66)", &[0x66, 0x31, 0xc0], 0x31),
        ("lock xor [rax], eax", &[0xf0, 0x31, 0x00], 0x31),
        ("xor al, bl (30 /r)", &[0x30, 0xd8], 0x30),
        ("xor eax, ebx (33 /r)", &[0x33, 0xc3], 0x33),
        ("xor eax, imm32 (35)", &[0x35, 1, 0, 0, 0], 0x35),
        ("xor eax, imm8 (83 /6)", &[0x83, 0xf0, 0x01], 0x83),
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
            "mov qword [rax], imm32 (C7 /0, REX.W memory)",
            &[0x48, 0xc7, 0x00, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov dword [rax], imm32 (C7 /0, memory, no REX.W)",
            &[0xc7, 0x00, 0x01, 0x00, 0x00, 0x00],
            0xc7,
        ),
        (
            "mov qword [rip+0], imm32 (C7 /0, REX.W RIP-relative)",
            &[0x48, 0xc7, 0x05, 0, 0, 0, 0, 0x01, 0x00, 0x00, 0x00],
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
        ("66 test ax, ax (66 85 /r)", &[0x66, 0x85, 0xc0], 0x85),
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
        ("test al, al (84 /r)", &[0x84, 0xc0], 0x84),
        ("test eax, imm32 (A9)", &[0xa9, 1, 0, 0, 0], 0xa9),
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
        ("test al, imm8 (A8)", &[0xa8, 1], 0xa8),
        // S4-39f5: `[REX.W] 89 /r` in the register form is allowed; the variants below stay outside.
        ("mov ebx, eax (89 /r, no REX.W)", &[0x89, 0xc3], 0x89),
        (
            "mov r12d, r8d (89 /r, REX.RB without W)",
            &[0x45, 0x89, 0xc4],
            0x89,
        ),
        (
            "mov [rbx], rax (REX.W 89 /r, memory)",
            &[0x48, 0x89, 0x03],
            0x89,
        ),
        (
            "mov [rip+0], rax (REX.W 89 /r, RIP-relative)",
            &[0x48, 0x89, 0x05, 0, 0, 0, 0],
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
        // S4-39f7: the short Jcc is allowed without REX/prefix; JMP/LOOP rel8 and prefixed Jcc stay.
        ("jmp rel8 (EB)", &[0xeb, 0x00], 0xeb),
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
        // S4-39f7: CMP r/m64, r64 is allowed only as `[REX.W] 39 /r` with mod = 11 and no prefix.
        ("cmp ebx, r13d (REX.R 39 /r)", &[0x44, 0x39, 0xeb], 0x39),
        ("cmp ebx, ecx (39 /r)", &[0x39, 0xcb], 0x39),
        (
            "cmp [rbx], rcx (REX.W 39 /r memory)",
            &[0x48, 0x39, 0x0b],
            0x39,
        ),
        ("cmp [rbx], ecx (39 /r memory)", &[0x39, 0x0b], 0x39),
        ("operand-size cmp rbx, rcx", &[0x66, 0x48, 0x39, 0xcb], 0x39),
        ("lock cmp rbx, rcx", &[0xf0, 0x48, 0x39, 0xcb], 0x39),
        ("rep cmp rbx, rcx", &[0xf3, 0x48, 0x39, 0xcb], 0x39),
        ("fs cmp rbx, rcx", &[0x64, 0x48, 0x39, 0xcb], 0x39),
        ("addr32 cmp rbx, rcx", &[0x67, 0x48, 0x39, 0xcb], 0x39),
        ("cmp rcx, rbx (REX.W 3B /r)", &[0x48, 0x3b, 0xcb], 0x3b),
        ("cmp bl, cl (38 /r)", &[0x38, 0xcb], 0x38),
        (
            "cmpq $imm8, rbx (REX.W 83 /7)",
            &[0x48, 0x83, 0xfb, 0x01],
            0x83,
        ),
        ("cmpb $imm8, [rbx] (80 /7)", &[0x80, 0x3b, 0x20], 0x80),
        ("jmp rel32 (E9)", &[0xe9, 0, 0, 0, 0], 0xe9),
        ("call rel32 (E8)", &[0xe8, 0, 0, 0, 0], 0xe8),
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
            "mov rax, imm64",
            &[0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0],
            0xb8,
        ),
        ("mov ax, imm16", &[0x66, 0xb8, 1, 0], 0xb8),
        ("lea rax, [rbx]", &[0x48, 0x8d, 0x03], 0x8d),
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
            "lea edi, [rip+0x10] (no REX.W)",
            [&[0x8d, 0x3d][..], &disp].concat(),
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
fn lea_sib_variants_outside_the_allowed_form_stop_without_change() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let forms: &[(&str, &[u8])] = &[
        (
            "no REX.W (lea edi, [r14+rbx] needs REX.B only)",
            &[0x41, 0x8d, 0x3c, 0x1e],
        ),
        ("no REX at all", &[0x8d, 0x3c, 0x1e]),
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
        // Two REX bytes: the decoder keeps the last one only, so the byte at rip + 2 is not a ModRM.
        (
            "two REX prefixes, [rbx] without SIB",
            &[0x40, 0x40, 0x4c, 0x8d, 0x03],
        ),
        // Register-base forms without a SIB byte stay outside (S4-39f8 allows the SIB form only).
        ("[rbx] without SIB", &[0x48, 0x8d, 0x03]),
        ("[r14] without SIB", &[0x49, 0x8d, 0x3e]),
        ("[rbx+0x8] without SIB", &[0x48, 0x8d, 0x43, 0x08]),
        (
            "[rbx+0x100] without SIB",
            &[0x48, 0x8d, 0x83, 0x00, 0x01, 0x00, 0x00],
        ),
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
    // The allowed C7 form retires, the memory form that follows stops the run before any write.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let mut memory = Memory::code(&[
        0x48, 0xc7, 0xc3, 0x05, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x00, 0x01, 0x00, 0x00, 0x00,
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
    // on `mov [rbx], rax` (memory form, still outside), which stops the run.
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    state.gpr[Register::Rax as usize] = 0xffff_ffff_ffff_fff2;
    let mut memory = Memory::code(&[
        0x48, 0x85, 0xc0, 0x0f, 0x88, 0x01, 0x00, 0x00, 0x00, 0x90, 0x48, 0x89, 0x03,
    ]);
    let result = run3(&mut state, &mut memory, &set, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x89 });
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
    // `/init` at 0x40016b: `85 c9` (testl %ecx, %ecx), `74 01` (je, taken on a zero low dword),
    // a skipped `90`, then `8a 06` (movb (%rsi), %al, memory form; executes since S4-39f10) and
    // `38 07` (cmpb %al, (%rdi), still outside) stops the run.
    let mut state = user_state();
    state.gpr[Register::Rcx as usize] = 0xffff_ffff_0000_0000;
    state.gpr[Register::Rsi as usize] = DATA;
    let mut memory = DataMemory::new(&[0x85, 0xc9, 0x74, 0x01, 0x90, 0x8a, 0x06, 0x38, 0x07])
        .with_user(DATA, 0x5a);
    let result = run3_data(&mut state, &mut memory, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x38 });
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, RIP + 7);
    assert_eq!(state.gpr[Register::Rcx as usize], 0xffff_ffff_0000_0000);
    assert_eq!(state.gpr[Register::Rax as usize], 0x5a);
    assert_eq!(state.rflags & (ZF | CF | OF), ZF);
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
    user: std::collections::BTreeMap<u64, u8>,
    supervisor: std::collections::BTreeMap<u64, u8>,
    /// Addresses of a page without P: any access is a #PF with the P bit clear.
    absent: std::collections::BTreeSet<u64>,
    writes: Vec<(u64, u8)>,
    reads: usize,
}

impl DataMemory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            user: std::collections::BTreeMap::new(),
            supervisor: std::collections::BTreeMap::new(),
            absent: std::collections::BTreeSet::new(),
            writes: Vec::new(),
            reads: 0,
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
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
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
fn mov_r64_with_a_hint_or_segment_prefix_or_a_memory_operand_is_refused() {
    // S4-39f5 review item 6: `2e`/`3e` (branch hints, CS/DS override) before the memory form and
    // before the register form of `89` never reach the CPL0 `Operation::Mov`.
    let forms: &[(&str, &[u8])] = &[
        ("2e 89 03", &[0x2e, 0x89, 0x03]),
        ("3e 89 03", &[0x3e, 0x89, 0x03]),
        ("2e 48 89 03", &[0x2e, 0x48, 0x89, 0x03]),
        ("3e 48 89 03", &[0x3e, 0x48, 0x89, 0x03]),
        ("2e 48 89 c3", &[0x2e, 0x48, 0x89, 0xc3]),
        ("3e 48 89 c3", &[0x3e, 0x48, 0x89, 0xc3]),
        ("48 89 03 memory form", &[0x48, 0x89, 0x03]),
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
        assert_eq!(result.instructions_executed, 2, "{rbx}");
        if taken {
            assert_eq!(
                exit(&result),
                &RunExit::Unsupported { opcode: 0x90 },
                "{rbx}"
            );
            assert_eq!(state.rip, RIP + 15, "{rbx}");
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
            assert_eq!(result.instructions_executed, 1, "cc {cc} flags {bits:#x}");
            if taken {
                assert_eq!(state.rip, RIP + 4, "cc {cc} flags {bits:#x}");
                assert_eq!(
                    exit(&result),
                    &RunExit::Unsupported { opcode: 0x90 },
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
    // `88` (MOV r/m8, r8), `8b` (MOV r32, m32), `38` (CMP r/m8, r8), `ff /0` and `ff /1`
    // (INC/DEC r/m) and `eb` (JMP rel8) are not part of S4-39f10.
    let forms: &[(&str, &[u8], u8)] = &[
        ("88 03 movb %al, (%rbx)", &[0x88, 0x03], 0x88),
        ("8b 03 movl (%rbx), %eax", &[0x8b, 0x03], 0x8b),
        ("38 07 cmpb %al, (%rdi)", &[0x38, 0x07], 0x38),
        ("48 ff c6 incq %rsi", &[0x48, 0xff, 0xc6], 0xff),
        ("ff c9 decl %ecx", &[0xff, 0xc9], 0xff),
        ("eb 00 jmp +0", &[0xeb, 0x00], 0xeb),
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
fn init_movb_load_then_cmpb_boundary_keeps_the_retired_state() {
    // `/init` at 0x40016f: `8a 06` (movb (%rsi), %al) retires, then `38 07` (cmpb %al, (%rdi)) is
    // the next boundary (opcode 0x38).
    let mut state = mov_byte_state();
    state.gpr[Register::Rsi as usize] = DATA;
    state.gpr[Register::Rdi as usize] = DATA + 0x40;
    state.rflags |= ARITH;
    let mut expected = with_byte(&state, Register::Rax, 0, b'a');
    expected.rip = RIP + 2;
    let mut memory = DataMemory::new(&[0x8a, 0x06, 0x38, 0x07])
        .with_user(DATA, b'a')
        .with_user(DATA + 0x40, b'b');
    let result = run3_data(&mut state, &mut memory, 8).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x38 });
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state, expected);
    assert_eq!(memory.reads, 1);
    assert!(memory.writes.is_empty());
}
