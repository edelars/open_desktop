//! S4-39e: `SYSCALL` (`0F 05`) from CPL3 through the set's STAR, LSTAR, SYSCALL_MASK and
//! EFER.SCE cells under the `with_syscall` member. The semantics are from memory of the SDM (no
//! copy in the repository), see CPU_CONTRACT S4-39e.

use std::cell::Cell;

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, RunConfig, RunExit, run_with_cpl0_profile_set, run_with_cpl3_profile_set,
};
use od_isa::long_decode::{DecodeError, Operation, decode};
use od_isa::msr::MsrProfile;
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CodeSegmentDescriptorCache,
    CpuActivity, DataSegmentDescriptorCache, GuestMemory, InstructionMode, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const RIP: u64 = 0x40_00c1;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
/// What the Linux kernel writes: LME | LMA | SCE | NXE.
const EFER_SCE: u64 = 0xd01;
/// STAR[47:32] = 0x10 (kernel CS), STAR[63:48] = 0x23 (user base).
const STAR: u64 = 0x0023_0010_0000_0000;
const LSTAR: u64 = 0xffff_ffff_8100_0080;
/// The FMASK the kernel writes.
const FMASK: u64 = 0x25_7fd5;

#[derive(Clone)]
struct Memory {
    bytes: Vec<u8>,
    accesses: usize,
}

impl Memory {
    fn code(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            accesses: 0,
        }
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
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

struct Cells {
    efer: Cell<u64>,
    star: Cell<u64>,
    lstar: Cell<u64>,
    mask: Cell<u64>,
}

impl Cells {
    fn linux() -> Self {
        Self {
            efer: Cell::new(EFER_SCE),
            star: Cell::new(STAR),
            lstar: Cell::new(LSTAR),
            mask: Cell::new(FMASK),
        }
    }

    fn snapshot(&self) -> [u64; 4] {
        [
            self.efer.get(),
            self.star.get(),
            self.lstar.get(),
            self.mask.get(),
        ]
    }

    fn set(&self) -> Cpl0ProfileSet<'_> {
        self.cells_only().with_syscall()
    }

    /// The same cells without the syscall member.
    fn cells_only(&self) -> Cpl0ProfileSet<'_> {
        Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_msr_efer_write(&self.efer)
            .with_msr_star_write(&self.star)
            .with_msr_lstar_write(&self.lstar)
            .with_msr_syscall_mask_write(&self.mask)
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

/// The state an `IRETQ` to CPL3 leaves (S4-38L9H): CS 0x33 (DPL 3, L = 1), SS 0x2b (DPL 3), with
/// the kernel's user stack and a register file that SYSCALL must not touch.
fn user_state() -> LongModeState {
    let mut state = LongModeState {
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
    };
    for (index, value) in state.gpr.iter_mut().enumerate() {
        *value = 0x1111_0000_0000_0000 + index as u64;
    }
    state.gpr[Register::Rsp as usize] = 0x7ffc_886f_cfa0;
    state
}

fn run3(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
    defined: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
        defined,
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

const SYSCALL: [u8; 2] = [0x0f, 0x05];
const UD: RunExit = RunExit::Fault(ArchitecturalFault {
    vector: 6,
    error_code: None,
    linear_address: None,
});
const GP0: RunExit = RunExit::Fault(ArchitecturalFault {
    vector: 13,
    error_code: Some(0),
    linear_address: None,
});
const SYSRETL: [u8; 2] = [0x0f, 0x07];

#[test]
fn syscall_from_cpl3_enters_the_kernel_through_the_msr_cells() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    // CF, ZF, IF, DF, AC, ID and a bit outside the mask (reserved bit 3) and RF.
    state.rflags = 2 | 1 | 0x40 | (1 << 9) | (1 << 10) | (1 << 18) | (1 << 21) | 0x8 | (1 << 16);
    let before = state.clone();
    let mut memory = Memory::code(&SYSCALL);
    let result = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // RCX = the address after the instruction, R11 = the old RFLAGS.
    assert_eq!(state.gpr[Register::Rcx as usize], RIP + 2);
    assert_eq!(state.gpr[Register::R11 as usize], before.rflags);
    // RFLAGS = old and not FMASK, RF cleared, bit 1 set; IF is among the cleared bits.
    assert_eq!(state.rflags, (before.rflags & !FMASK & !(1 << 16)) | 2);
    assert_eq!(state.rflags, 2 | 0x8);
    assert_eq!(state.rflags & (1 << 9), 0);
    // The flags FMASK cleared are defined now: all of CF, PF, AF, ZF, SF, DF, OF.
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
    // CS = STAR[47:32] with RPL 0, SS = CS + 8, both flat; CPL is 0.
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.ss_selector, Some(0x18));
    assert_eq!(
        state.cs_cache,
        Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        })
    );
    assert_eq!(
        state.ss_cache,
        Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0xc,
        })
    );
    assert!(!state.ss_unusable);
    assert_eq!(state.rip, LSTAR);
    // Nothing else moved: RSP and the other registers, memory, the MSR cells.
    let mut expected = before.gpr;
    expected[Register::Rcx as usize] = RIP + 2;
    expected[Register::R11 as usize] = before.rflags;
    assert_eq!(state.gpr, expected);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x7ffc_886f_cfa0);
    assert_eq!(memory.accesses, 0);
    assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
}

#[test]
fn undefined_flags_stay_undefined_unless_fmask_clears_them() {
    let cells = Cells::linux();
    cells.mask.set(0x1 | (1 << 9));
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    let result = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    // Only CF (and IF, outside the integer flags) are in the mask: CF becomes defined, the rest do not.
    assert_eq!(result.rflags_defined, 2 | 1);
    assert_eq!(state.rflags, 2);
}

#[test]
fn fmask_uses_only_the_low_32_bits_and_keeps_bit_1() {
    let cells = Cells::linux();
    cells.mask.set(0xffff_ffff_0000_0200);
    let set = cells.set();
    let mut state = user_state();
    state.rflags = 2 | (1 << 9) | (1 << 14);
    let mut memory = Memory::code(&SYSCALL);
    run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(state.rflags, 2 | (1 << 14));
    // A mask that names bit 1 cannot clear it.
    let cells = Cells::linux();
    cells.mask.set(0xffff_ffff);
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(state.rflags, 2);
}

#[test]
fn the_selectors_follow_star_with_rpl_zero() {
    let cells = Cells::linux();
    // 0x0038 + RPL bits are not allowed (refused below); 0x38 is a plain base.
    cells.star.set(0x0023_0038_0000_0000);
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(state.cs_selector, Some(0x38));
    assert_eq!(state.ss_selector, Some(0x40));
}

#[test]
fn syscall_with_sce_clear_is_ud_without_a_change() {
    for efer in [0xd00, 0x500, 0] {
        let cells = Cells::linux();
        cells.efer.set(efer);
        let set = cells.set();
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(&SYSCALL);
        let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            }),
            "EFER {efer:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, 2);
        assert_eq!(state, before);
        assert_eq!(memory.accesses, 0);
        assert_eq!(cells.snapshot(), [efer, STAR, LSTAR, FMASK]);
    }
}

#[test]
fn unmodelled_cell_values_refuse_without_a_change() {
    // A non-canonical LSTAR, RPL bits in the STAR kernel selector, a trap flag in RFLAGS.
    let cases: [(&str, fn(&Cells, &mut LongModeState)); 4] = [
        ("non-canonical LSTAR", |cells, _| {
            cells.lstar.set(0x0001_0000_0000_0000)
        }),
        ("non-canonical LSTAR (high half)", |cells, _| {
            cells.lstar.set(0x8000_0000_0000_0000)
        }),
        ("STAR RPL", |cells, _| cells.star.set(0x0023_0013_0000_0000)),
        ("TF", |_, state| state.rflags |= 1 << 8),
    ];
    for (name, mutate) in cases {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = user_state();
        mutate(&cells, &mut state);
        let before = state.clone();
        let snapshot = cells.snapshot();
        let mut memory = Memory::code(&SYSCALL);
        let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x05 },
            "{name}"
        );
        assert_eq!(result.instructions_executed, 0, "{name}");
        assert_eq!(result.rflags_defined, 2, "{name}");
        assert_eq!(state, before, "{name}");
        assert_eq!(cells.snapshot(), snapshot, "{name}");
    }
}

#[test]
fn a_canonical_lstar_in_the_low_half_is_taken() {
    let cells = Cells::linux();
    cells.lstar.set(0x0000_7fff_ffff_f000);
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(state.rip, 0x0000_7fff_ffff_f000);
}

#[test]
fn without_the_member_the_cpl3_entry_keeps_the_allow_list_refusal() {
    let cells = Cells::linux();
    let set = cells.cells_only();
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&SYSCALL);
    let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    // The bare set (no cells at all) refuses the same way.
    let bare = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = user_state();
    let result = run3(&mut state, &mut Memory::code(&SYSCALL), &bare, 5, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x05 });
    assert_eq!(state, before);
}

#[test]
fn the_member_without_one_of_its_cells_is_refused_at_entry() {
    let efer = Cell::new(EFER_SCE);
    let star = Cell::new(STAR);
    let lstar = Cell::new(LSTAR);
    let mask = Cell::new(FMASK);
    let policy = ControlRegisterPolicy::Cpl0BaseLong64V1;
    let sets = [
        Cpl0ProfileSet::new(policy).with_syscall(),
        Cpl0ProfileSet::new(policy)
            .with_msr_star_write(&star)
            .with_msr_lstar_write(&lstar)
            .with_msr_syscall_mask_write(&mask)
            .with_syscall(),
        Cpl0ProfileSet::new(policy)
            .with_msr_efer_write(&efer)
            .with_msr_lstar_write(&lstar)
            .with_msr_syscall_mask_write(&mask)
            .with_syscall(),
        Cpl0ProfileSet::new(policy)
            .with_msr_efer_write(&efer)
            .with_msr_star_write(&star)
            .with_msr_syscall_mask_write(&mask)
            .with_syscall(),
        Cpl0ProfileSet::new(policy)
            .with_msr_efer_write(&efer)
            .with_msr_star_write(&star)
            .with_msr_lstar_write(&lstar)
            .with_syscall(),
    ];
    for (index, set) in sets.iter().enumerate() {
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(&SYSCALL);
        assert_eq!(
            run3(&mut state, &mut memory, set, 5, 2).unwrap_err(),
            Cpl0ProfileSetInputError::SyscallCellsMissing,
            "set {index}"
        );
        assert_eq!(state, before);
    }
    // The CPL0 entry refuses the same sets the same way.
    let mut state = user_state();
    state.cs_selector = Some(0x10);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut Memory::code(&SYSCALL),
        config(PrivilegeLevel::Ring0, 1),
        2,
        &sets[0],
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::SyscallCellsMissing);
}

#[test]
fn syscall_at_cpl0_enters_the_kernel_through_the_msr_cells() {
    // S2-27m: the SDM allows SYSCALL at every CPL of 64-bit mode (a CPL0 caller stays at CPL0).
    let cells = Cells::linux();
    cells.lstar.set(RIP + 2);
    let set = cells.set();
    let mut state = kernel_state();
    state.rflags = 0x3246; // IOPL 3, IF, ZF, PF
    state.gpr[Register::Rcx as usize] = 0xdead;
    state.gpr[Register::R11 as usize] = 0xbeef;
    let rsp = state.gpr[Register::Rsp as usize];
    let mut memory = Memory::code(&SYSCALL);
    let result = run0(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.gpr[Register::Rcx as usize], RIP + 2);
    assert_eq!(state.gpr[Register::R11 as usize], 0x3246);
    assert_eq!(state.rflags, 2);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.ss_selector, Some(0x18));
    assert_eq!(state.gpr[Register::Rsp as usize], rsp);
    assert_eq!(memory.accesses, 0);
    assert_eq!(cells.snapshot(), [EFER_SCE, STAR, RIP + 2, FMASK]);
}

#[test]
fn the_cpl0_entry_refuses_a_ring3_state_even_with_the_member() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    let before = state.clone();
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut Memory::code(&SYSCALL),
        config(PrivilegeLevel::Ring0, 5),
        2,
        &set,
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
    assert_eq!(state, before);
}

#[test]
fn prefixed_forms_are_refused_without_a_change() {
    let cases: [&[u8]; 8] = [
        &[0x48, 0x0f, 0x05], // REX.W: the SYSRET-like spelling, not SYSCALL
        &[0x40, 0x0f, 0x05],
        &[0x66, 0x0f, 0x05],
        &[0x67, 0x0f, 0x05],
        &[0xf2, 0x0f, 0x05],
        &[0xf3, 0x0f, 0x05],
        &[0x64, 0x0f, 0x05],
        &[0x2e, 0x0f, 0x05],
    ];
    for code in cases {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x05 },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
    }
}

#[test]
fn sysenter_sysret_and_neighbours_stay_refused() {
    // SYSENTER/SYSEXIT (0F 34/35), SYSCALL-like 0F 06 (CLTS) and 0F 08 (INVD) are not admitted
    // by the member (SYSRET at CPL3 is the #GP(0) of S2-27m, tested separately).
    let cases: [(&[u8], u8); 4] = [
        (&[0x0f, 0x34], 0x34),
        (&[0x0f, 0x35], 0x35),
        (&[0x0f, 0x06], 0x06),
        (&[0x0f, 0x08], 0x08),
    ];
    for (code, opcode) in cases {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = user_state();
        let before = state.clone();
        let mut memory = Memory::code(code);
        let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
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
fn a_failing_fetch_of_the_second_byte_changes_nothing() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    let before = state.clone();
    let mut memory = Memory::code(&[0x0f]);
    let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
    assert!(
        !matches!(exit(&result), RunExit::InstructionBudgetExhausted),
        "{:?}",
        result.exit
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
}

#[test]
fn the_instruction_commits_without_touching_guest_memory() {
    // Every data access of this memory fails: SYSCALL performs none (no stack switch, no GDT
    // read), so the commit cannot fail halfway.
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    let result = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(memory.accesses, 0);
}

#[test]
fn syscall_follows_the_init_prefix_in_the_same_run() {
    // `mov $0x53, %eax; syscall` from the first `/init` system call, then the budget ends in the
    // kernel at LSTAR (CS RPL 0): the CPL3 entry ran both instructions.
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05]);
    let result = run3(&mut state, &mut memory, &set, 2, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[Register::Rax as usize], 0x53);
    assert_eq!(state.gpr[Register::Rcx as usize], RIP + 7);
    assert_eq!(state.rip, LSTAR);
    assert_eq!(state.cs_selector, Some(0x10));
}

#[test]
fn only_the_bare_form_decodes_as_syscall() {
    let decode_bytes = |bytes: &[u8]| {
        let mut memory = Memory::code(bytes);
        // The decoder reads at RIP.
        decode(&mut memory, RIP, AccessContext::default())
    };
    let instruction = decode_bytes(&SYSCALL).unwrap();
    assert_eq!(instruction.operation, Operation::Syscall);
    assert_eq!(instruction.length, 2);
    for code in [
        &[0x48, 0x0f, 0x05][..],
        &[0x66, 0x0f, 0x05],
        &[0xf2, 0x0f, 0x05],
        &[0x40, 0x0f, 0x05],
    ] {
        assert!(
            matches!(decode_bytes(code), Err(DecodeError::Unsupported { .. })),
            "{code:02x?}"
        );
    }
    // S2-27m: LOCK makes the exact form `#UD` (SDM: "#UD if the LOCK prefix is used").
    assert!(matches!(
        decode_bytes(&[0xf0, 0x0f, 0x05]),
        Err(DecodeError::Invalid { length: 3, .. })
    ));
}

#[test]
fn an_msr_profile_does_not_change_the_instruction() {
    // The EFER cell of the set supersedes the read-only profile for RDMSR only; SYSCALL reads the
    // cell, not the profile's constant (here SCE clear in the profile, set in the cell).
    let cells = Cells::linux();
    let profile = MsrProfile::EferReadOnlyV1(0xd00);
    let set = cells.set().with_msr(&profile);
    let mut state = user_state();
    let mut memory = Memory::code(&SYSCALL);
    let result = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, LSTAR);
}

// ---------------------------------------------------------------------------------------------
// S4-39f: SYSRETQ (`48 0F 07`) from CPL0 back to CPL3, and ISSUE-087 (the CPL3 loop leaves at the
// first CPL change). Semantics from memory of the SDM, see CPU_CONTRACT S4-39f.
// ---------------------------------------------------------------------------------------------

const SYSRETQ: [u8; 3] = [0x48, 0x0f, 0x07];
/// The user return address of the first `/init` system call and its RFLAGS (IF, bit 1).
const USER_RIP: u64 = 0x40_00c3;
const USER_RFLAGS: u64 = 0x202;

/// The kernel-side state at the SYSRET: CS 0x10 / SS 0x18 (DPL 0), the user stack in RSP.
fn kernel_state() -> LongModeState {
    let mut state = user_state();
    state.cs_selector = Some(0x10);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xa,
    });
    state.ss_selector = Some(0x18);
    state.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0xc,
    });
    state.rflags = 2;
    state.gpr[Register::Rcx as usize] = USER_RIP;
    state.gpr[Register::R11 as usize] = USER_RFLAGS;
    state
}

fn run0(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        2,
        set,
        None,
        || false,
    )
}

#[test]
fn sysret_returns_to_cpl3_through_star_without_touching_rsp() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state();
    let before = state.clone();
    let mut memory = Memory::code(&SYSRETQ);
    let result = run0(&mut state, &mut memory, &set, 5).unwrap();
    // The retired SYSRET ends the CPL0 run before any fetch, like a committed IRETQ to CPL3.
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::UserModeEntered(od_isa::IretqUserEntry {
            rip: USER_RIP,
            cs: 0x33,
            rflags: USER_RFLAGS,
            rsp: 0x7ffc_886f_cfa0,
            ss: 0x2b,
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, USER_RIP);
    assert_eq!(state.rflags, USER_RFLAGS);
    // CS = (STAR[63:48] + 16) | 3, SS = (STAR[63:48] + 8) | 3: 0x23 + 16 = 0x33, 0x23 + 8 = 0x2b.
    assert_eq!(state.cs_selector, Some(0x33));
    assert_eq!(state.ss_selector, Some(0x2b));
    assert_eq!(
        state.cs_cache,
        Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xfb,
            flags: 0xa,
        })
    );
    assert_eq!(
        state.ss_cache,
        Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xf3,
            flags: 0xc,
        })
    );
    assert!(!state.ss_unusable);
    // RSP, RCX, R11 and the rest stay.
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x7ffc_886f_cfa0);
    assert_eq!(memory.accesses, 0);
    assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
    // The state is one the CPL3 entry admits.
    let again = run_with_cpl3_profile_set(
        &mut state,
        &mut Memory::code(&[]),
        config(PrivilegeLevel::Ring3, 0),
        2,
        &set,
        None,
        || false,
    );
    assert!(again.is_ok(), "{again:?}");
}

#[test]
fn sysret_loads_rflags_through_the_sdm_mask_from_r11() {
    // The SDM mask from memory: RFLAGS = (R11 and 0x3C7FD7) | 2. TF, VIF and VIP are inside it
    // but not modelled (refused below), so they are cleared in the input here.
    let all_but_unmodelled = u64::MAX & !((1 << 8) | (1 << 19) | (1 << 20));
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state();
    // RF (bit 16) and VM (bit 17): VM is refused, so only RF is set; the high half is dropped.
    state.gpr[Register::R11 as usize] = (all_but_unmodelled & !(1 << 17)) | (1 << 16);
    let r11 = state.gpr[Register::R11 as usize];
    let mut memory = Memory::code(&SYSRETQ);
    let result = run0(&mut state, &mut memory, &set, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rflags, (r11 & 0x3c_7fd7) | 2);
    assert_eq!(state.rflags & (1 << 16), 0);
    assert_eq!(state.rflags & (1 << 17), 0);
    assert_eq!(state.rflags >> 32, 0);
    // IF comes from R11, not from the kernel's value (IF clear before).
    assert_ne!(state.rflags & (1 << 9), 0);
    // The loaded status flags are defined.
    assert_eq!(result.rflags_defined, od_isa::POPFQ_INTEGER_FLAGS | 2);
}

#[test]
fn sysret_refuses_unmodelled_r11_flags_and_selectors_without_a_change() {
    for r11 in [
        USER_RFLAGS | (1 << 8),  // TF
        USER_RFLAGS | (1 << 19), // VIF
        USER_RFLAGS | (1 << 20), // VIP
    ] {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = kernel_state();
        state.gpr[Register::R11 as usize] = r11;
        let before = state.clone();
        let mut memory = Memory::code(&SYSRETQ);
        let result = run0(&mut state, &mut memory, &set, 5).unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x07 }),
            "R11 {r11:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
    // STAR[63:48] + 16 overflows the selector.
    let cells = Cells::linux();
    cells.star.set(0xfff8_0010_0000_0000);
    let set = cells.set();
    let mut state = kernel_state();
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x07 })
    );
    assert_eq!(state, before);
}

#[test]
fn sysret_with_any_other_prefix_is_refused() {
    let cases: [&[u8]; 7] = [
        &[0x40, 0x0f, 0x07],
        &[0x49, 0x0f, 0x07],
        &[0x66, 0x48, 0x0f, 0x07],
        &[0x48, 0x66, 0x0f, 0x07],
        &[0x67, 0x48, 0x0f, 0x07],
        &[0xf3, 0x48, 0x0f, 0x07],
        &[0x2e, 0x48, 0x0f, 0x07],
    ];
    for code in cases {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = kernel_state();
        let before = state.clone();
        let result = run0(&mut state, &mut Memory::code(code), &set, 5).unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x07 }),
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn sysret_without_the_member_keeps_its_refusal() {
    let cells = Cells::linux();
    let set = cells.cells_only();
    let mut state = kernel_state();
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x07 })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn sysret_with_sce_clear_is_ud_without_a_change() {
    for efer in [0xd00, 0x500, 0] {
        let cells = Cells::linux();
        cells.efer.set(efer);
        let set = cells.set();
        let mut state = kernel_state();
        let before = state.clone();
        let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })),
            "EFER {efer:#x}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(cells.snapshot(), [efer, STAR, LSTAR, FMASK]);
    }
}

#[test]
fn sysret_with_a_non_canonical_rcx_is_gp0_without_a_change() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state();
    state.gpr[Register::Rcx as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        }))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    // A canonical high-half RCX is taken.
    let mut state = kernel_state();
    state.gpr[Register::Rcx as usize] = 0xffff_8000_0000_1000;
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 0xffff_8000_0000_1000);
}

#[test]
fn sysret_at_cpl3_is_gp0_without_a_change() {
    // S2-27m: SDM "IF (CPL != 0) THEN #GP(0)" after the #UD gates, for both operand sizes.
    for code in [&SYSRETQ[..], &[0x0f, 0x07]] {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = user_state();
        state.gpr[Register::Rcx as usize] = USER_RIP;
        state.gpr[Register::R11 as usize] = USER_RFLAGS;
        let before = state.clone();
        let result = run3(&mut state, &mut Memory::code(code), &set, 5, 2).unwrap();
        assert_eq!(exit(&result), &GP0, "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
    }
}

#[test]
fn only_the_two_sdm_forms_decode_as_sysret() {
    let decode_bytes = |bytes: &[u8]| {
        let mut memory = Memory::code(bytes);
        decode(&mut memory, RIP, AccessContext::default())
    };
    let instruction = decode_bytes(&SYSRETQ).unwrap();
    assert_eq!(instruction.operation, Operation::Sysret);
    assert_eq!(instruction.length, 3);
    assert_eq!(instruction.operand_size, OperandSize::Qword);
    // S2-27m: the bare `0F 07` is SYSRETL, the return to compatibility mode.
    let instruction = decode_bytes(&[0x0f, 0x07]).unwrap();
    assert_eq!(instruction.operation, Operation::Sysret);
    assert_eq!(instruction.length, 2);
    assert_eq!(instruction.operand_size, OperandSize::Dword);
    for code in [
        &[0x40, 0x0f, 0x07][..],
        &[0x49, 0x0f, 0x07],
        &[0x66, 0x48, 0x0f, 0x07],
        &[0x66, 0x0f, 0x07],
    ] {
        assert!(
            matches!(decode_bytes(code), Err(DecodeError::Unsupported { .. })),
            "{code:02x?}"
        );
    }
    for (code, length) in [(&[0xf0, 0x0f, 0x07][..], 3), (&[0xf0, 0x48, 0x0f, 0x07], 4)] {
        assert!(
            matches!(decode_bytes(code), Err(DecodeError::Invalid { length: l, .. }) if l == length),
            "{code:02x?}"
        );
    }
}

#[test]
fn the_cpl3_loop_stops_at_the_cpl_change_of_syscall() {
    // ISSUE-087: `syscall` enters the kernel at LSTAR (here a low canonical address with a
    // `mov $1, %eax` on it). With budget 3 the second and third instructions must not run in
    // Ring3: the run ends right after the SYSCALL, with a Run exit and no Unsupported.
    let cells = Cells::linux();
    cells.lstar.set(RIP + 2);
    let set = cells.set();
    let mut state = user_state();
    let mut memory = Memory::code(&[0x0f, 0x05, 0xb8, 0x01, 0, 0, 0, 0x90]);
    let result = run3(&mut state, &mut memory, &set, 3, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.gpr[Register::Rcx as usize], RIP + 2);
    // The kernel's `mov $1, %eax` did not execute.
    assert_eq!(
        state.gpr[Register::Rax as usize],
        0x1111_0000_0000_0000 + Register::Rax as u64
    );
    // With the budget spent by the SYSCALL itself the exit stays the budget one (S4-39e).
    let mut state = user_state();
    let result = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(exit(&result), &RunExit::InstructionBudgetExhausted);
}

#[test]
fn syscall_then_sysret_round_trips_to_cpl3_at_rcx() {
    // User: `syscall` at RIP; the kernel: SYSRETQ at RIP + 0x20 (LSTAR); back at RCX = RIP + 2:
    // `mov $7, %eax`.
    let cells = Cells::linux();
    cells.lstar.set(RIP + 0x20);
    let set = cells.set();
    let mut code = vec![0u8; 0x23];
    code[0..2].copy_from_slice(&SYSCALL);
    code[2..7].copy_from_slice(&[0xb8, 7, 0, 0, 0]);
    code[0x20..0x23].copy_from_slice(&SYSRETQ);
    let mut memory = Memory::code(&code);
    let mut state = user_state();
    let user_rflags = state.rflags;
    let rsp = state.gpr[Register::Rsp as usize];
    let first = run3(&mut state, &mut memory, &set, 10, 2).unwrap();
    assert_eq!(exit(&first), &RunExit::StopAddress);
    assert_eq!(state.cs_selector, Some(0x10));
    assert_eq!(state.rip, RIP + 0x20);
    let second = run0(&mut state, &mut memory, &set, 10).unwrap();
    assert!(
        matches!(second.exit, Cpl0ProfileSetExit::UserModeEntered(_)),
        "{second:?}"
    );
    // CPL3 again, RIP = RCX (the instruction after `syscall`), RFLAGS = the saved ones (R11).
    assert_eq!(state.cs_selector, Some(0x33));
    assert_eq!(state.ss_selector, Some(0x2b));
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.rflags, (user_rflags & 0x3c_7fd7) | 2);
    assert_eq!(state.gpr[Register::Rsp as usize], rsp);
    let third = run3(&mut state, &mut memory, &set, 1, 2).unwrap();
    assert_eq!(exit(&third), &RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr[Register::Rax as usize], 7);
    assert_eq!(state.rip, RIP + 7);
}

// ---------------------------------------------------------------------------------------------
// S2-27m: SDM gates of SYSCALL/SYSRET (Intel SDM combined revision 093, Vol. 2B 4-699..4-711).
// There is no oracle: SYSCALL/SYSRET need CPL0 state, and the user-space oracle cannot give it.
// ---------------------------------------------------------------------------------------------

fn kernel_state_with_l_clear() -> LongModeState {
    let mut state = kernel_state();
    // The 32-bit compatibility code segment a long-mode OS keeps in the GDT (D = 1, L = 0).
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 0xc,
    });
    state
}

#[test]
fn syscall_without_long_mode_active_is_ud_before_the_sce_check() {
    // SDM: `IF IA32_EFER.LMA = 0 OR CS.L = 0 THEN #UD` comes first; SCE is checked after it. EFER
    // 0x101 = LME | SCE (LMA clear): SCE alone does not make the instruction valid.
    for efer in [0x101u64, 0x001] {
        for privilege in [PrivilegeLevel::Ring3, PrivilegeLevel::Ring0] {
            let cells = Cells::linux();
            cells.efer.set(efer);
            let set = cells.set();
            let mut memory = Memory::code(&SYSCALL);
            let (result, before, state) = if privilege == PrivilegeLevel::Ring3 {
                let mut state = user_state();
                let before = state.clone();
                let result = run3(&mut state, &mut memory, &set, 5, 2).unwrap();
                (exit(&result).clone(), before, state)
            } else {
                let mut state = kernel_state();
                let before = state.clone();
                let result = run0(&mut state, &mut memory, &set, 5).unwrap();
                match result.exit {
                    Cpl0ProfileSetExit::Run(exit) => (exit, before, state),
                    other => panic!("{other:?}"),
                }
            };
            assert_eq!(result, UD, "EFER {efer:#x} {privilege:?}");
            assert_eq!(state, before, "EFER {efer:#x} {privilege:?}");
            assert_eq!(cells.snapshot(), [efer, STAR, LSTAR, FMASK]);
        }
    }
}

#[test]
fn syscall_in_compatibility_mode_is_ud_and_does_not_read_cstar() {
    // Intel has no SYSCALL in compatibility mode (SDM: "#UD ... not recognized in compatibility
    // mode"); IA32_CSTAR is not an Intel SYSCALL target. A CS cache with L = 0 under EFER.LMA = 1
    // is compatibility mode.
    let cells = Cells::linux();
    let cstar = Cell::new(RIP + 0x40);
    let set = cells.set().with_msr_cstar_write(&cstar);
    let mut state = kernel_state_with_l_clear();
    let before = state.clone();
    let mut memory = Memory::code(&SYSCALL);
    let result = run0(&mut state, &mut memory, &set, 5).unwrap();
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(UD));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(cstar.get(), RIP + 0x40);
}

#[test]
fn lock_prefixed_syscall_and_sysret_are_ud_without_a_change() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = user_state();
    let before = state.clone();
    let result = run3(
        &mut state,
        &mut Memory::code(&[0xf0, 0x0f, 0x05]),
        &set,
        5,
        2,
    )
    .unwrap();
    assert_eq!(exit(&result), &UD);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    for code in [&[0xf0u8, 0x0f, 0x07][..], &[0xf0, 0x48, 0x0f, 0x07]] {
        let mut state = kernel_state();
        let before = state.clone();
        let result = run0(&mut state, &mut Memory::code(code), &set, 5).unwrap();
        assert_eq!(result.exit, Cpl0ProfileSetExit::Run(UD), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

/// S2-27m: the LOCK rule belongs to the decoder, not to the `with_syscall` member: a set without
/// the member also answers `F0 0F 05`, `F0 0F 07` and `F0 48 0F 07` with `#UD` (and without a
/// change), while the exact forms without LOCK keep the member refusal.
#[test]
fn lock_prefixed_forms_are_ud_in_a_set_without_the_member() {
    let cells = Cells::linux();
    let set = cells.cells_only();
    let mut state = user_state();
    let before = state.clone();
    let result = run3(
        &mut state,
        &mut Memory::code(&[0xf0, 0x0f, 0x05]),
        &set,
        5,
        2,
    )
    .unwrap();
    assert_eq!(exit(&result), &UD);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    for code in [
        &[0xf0u8, 0x0f, 0x05][..],
        &[0xf0, 0x0f, 0x07],
        &[0xf0, 0x48, 0x0f, 0x07],
    ] {
        let mut state = kernel_state();
        let before = state.clone();
        let result = run0(&mut state, &mut Memory::code(code), &set, 5).unwrap();
        assert_eq!(result.exit, Cpl0ProfileSetExit::Run(UD), "{code:02x?}");
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn sce_clear_is_ud_before_the_unmodelled_trap_flag_refusal() {
    // TF is a refusal only for a valid SYSCALL; the #UD gates come first.
    let cells = Cells::linux();
    cells.efer.set(0xd00);
    let set = cells.set();
    let mut state = user_state();
    state.rflags |= 1 << 8;
    let before = state.clone();
    let result = run3(&mut state, &mut Memory::code(&SYSCALL), &set, 5, 2).unwrap();
    assert_eq!(exit(&result), &UD);
    assert_eq!(state, before);
}

#[test]
fn sysret_without_long_mode_active_or_sce_is_ud_before_the_gp_checks() {
    // CPL3 (which would be #GP(0)) and a non-canonical RCX do not matter: #UD comes first.
    for efer in [0x101u64, 0xd00, 0] {
        for code in [&SYSRETQ[..], &SYSRETL[..]] {
            let cells = Cells::linux();
            cells.efer.set(efer);
            let set = cells.set();
            let mut state = user_state();
            state.gpr[Register::Rcx as usize] = 0x0000_8000_0000_0000;
            let before = state.clone();
            let result = run3(&mut state, &mut Memory::code(code), &set, 5, 2).unwrap();
            assert_eq!(exit(&result), &UD, "EFER {efer:#x} {code:02x?}");
            assert_eq!(state, before);
        }
    }
    // The same at CPL0 for the LMA gate (EFER 0x101: SCE set, LMA clear).
    let cells = Cells::linux();
    cells.efer.set(0x101);
    let set = cells.set();
    let mut state = kernel_state();
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(UD));
    assert_eq!(state, before);
}

#[test]
fn sysret_in_compatibility_mode_is_ud() {
    // SDM: "#UD: The SYSRET instruction is not recognized in compatibility mode."
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state_with_l_clear();
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(UD));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn sysretl_returns_to_compatibility_mode_with_ecx() {
    // SDM: without REX.W "RIP := ECX", CS.Selector := STAR[63:48] (no +16) | 3, CS.L := 0,
    // CS.D := 1; the upper half of RCX is ignored and no canonical check applies.
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state();
    state.gpr[Register::Rcx as usize] = 0xdead_beef_0040_1234;
    let before = state.clone();
    let mut memory = Memory::code(&SYSRETL);
    let result = run0(&mut state, &mut memory, &set, 5).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::UserModeEntered(od_isa::IretqUserEntry {
            rip: 0x40_1234,
            cs: 0x23,
            rflags: USER_RFLAGS,
            rsp: 0x7ffc_886f_cfa0,
            ss: 0x2b,
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 0x40_1234);
    assert_eq!(state.rflags, USER_RFLAGS);
    assert_eq!(state.cs_selector, Some(0x23));
    assert_eq!(state.ss_selector, Some(0x2b));
    assert_eq!(
        state.cs_cache,
        Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xfb,
            flags: 0xc,
        })
    );
    assert_eq!(
        state.ss_cache,
        Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0xf3,
            flags: 0xc,
        })
    );
    // RSP, RCX (full 64 bits), R11 and the rest stay.
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(memory.accesses, 0);
    assert_eq!(cells.snapshot(), [EFER_SCE, STAR, LSTAR, FMASK]);
    // The long-mode interpreter does not run compatibility code: the CPL3 entry refuses it.
    let error = run3(&mut state, &mut Memory::code(&[]), &set, 0, 2).unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WrongPrivilege);
}

#[test]
fn sysretl_takes_a_rcx_that_sysretq_rejects_as_non_canonical() {
    let cells = Cells::linux();
    let set = cells.set();
    let mut state = kernel_state();
    state.gpr[Register::Rcx as usize] = 0x0000_8000_0000_1000;
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(GP0));
    assert_eq!(state, before);
    let result = run0(&mut state, &mut Memory::code(&SYSRETL), &set, 5).unwrap();
    assert!(matches!(
        result.exit,
        Cpl0ProfileSetExit::UserModeEntered(_)
    ));
    assert_eq!(state.rip, 0x1000);
}

#[test]
fn sysret_selector_overflow_differs_by_operand_size() {
    // STAR[63:48] = 0xfff0: the 64-bit form needs +16 (0x10000, overflow, refused); the
    // compatibility form needs only the base (0xfff0) and +8 (0xfff8): both fit.
    let cells = Cells::linux();
    cells.star.set(0xfff0_0010_0000_0000);
    let set = cells.set();
    let mut state = kernel_state();
    let before = state.clone();
    let result = run0(&mut state, &mut Memory::code(&SYSRETQ), &set, 5).unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x07 })
    );
    assert_eq!(state, before);
    let result = run0(&mut state, &mut Memory::code(&SYSRETL), &set, 5).unwrap();
    assert!(matches!(
        result.exit,
        Cpl0ProfileSetExit::UserModeEntered(_)
    ));
    assert_eq!(state.cs_selector, Some(0xfff3));
    assert_eq!(state.ss_selector, Some(0xfffb));
}

#[test]
fn sysret_clears_vm_and_rf_of_r11_instead_of_refusing() {
    // SDM: "Regardless of the value of R11, the RF and VM flags are always 0 in RFLAGS after
    // execution of SYSRET." RFLAGS := (R11 & 3C7FD7H) | 2 contains neither bit 16 nor bit 17.
    for code in [&SYSRETQ[..], &SYSRETL[..]] {
        let cells = Cells::linux();
        let set = cells.set();
        let mut state = kernel_state();
        state.gpr[Register::R11 as usize] = USER_RFLAGS | (1 << 16) | (1 << 17);
        let result = run0(&mut state, &mut Memory::code(code), &set, 5).unwrap();
        assert!(
            matches!(result.exit, Cpl0ProfileSetExit::UserModeEntered(_)),
            "{code:02x?}"
        );
        assert_eq!(state.rflags, USER_RFLAGS, "{code:02x?}");
    }
}

#[test]
fn syscall_sysret_round_trip_through_the_compatibility_return() {
    // User `syscall` (CPL3) -> kernel SYSRETL at LSTAR -> CPL3 compatibility mode at ECX.
    let cells = Cells::linux();
    cells.lstar.set(RIP + 0x20);
    let set = cells.set();
    let mut code = vec![0u8; 0x22];
    code[0..2].copy_from_slice(&SYSCALL);
    code[0x20..0x22].copy_from_slice(&SYSRETL);
    let mut memory = Memory::code(&code);
    let mut state = user_state();
    let first = run3(&mut state, &mut memory, &set, 10, 2).unwrap();
    assert_eq!(exit(&first), &RunExit::StopAddress);
    let second = run0(&mut state, &mut memory, &set, 10).unwrap();
    assert!(matches!(
        second.exit,
        Cpl0ProfileSetExit::UserModeEntered(_)
    ));
    assert_eq!(state.rip, RIP + 2);
    assert_eq!(state.cs_selector, Some(0x23));
    assert_eq!(state.cs_cache.map(|cache| cache.flags), Some(0xc));
}
