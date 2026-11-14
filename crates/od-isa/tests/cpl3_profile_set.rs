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
        ("mov rbx, rax", &[0x48, 0x89, 0xc3], 0x89),
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
