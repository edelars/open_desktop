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
        // Ordinary forms that `/init` runs after its first syscall stay outside this slice.
        ("xor eax, eax", &[0x31, 0xc0], 0x31),
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
