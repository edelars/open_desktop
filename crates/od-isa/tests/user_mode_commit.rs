//! S4-39b: the `IRETQ` to CPL3 commits under `Cpl0ProfileSet::with_user_mode` and the CPL3 entry
//! continues from the committed state. Without the member the boundary (`UserEntry`) is unchanged.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetResult, RunConfig,
    RunExit, run_with_cpl0_profile_set, run_with_cpl0_profile_set_and_pio,
    run_with_cpl3_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CodeSegmentDescriptorCache,
    CpuActivity, DataSegmentDescriptorCache, FsDescriptorCache, GuestMemory, InstructionMode,
    IretqUserEntry, LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register,
    StiShadow,
};

const KERNEL_RIP: u64 = 0x1000;
const USER_RIP: u64 = 0x0000_7fff_0000_1000;
const USER_RSP: u64 = 0x0000_7fff_ffff_e000;
const RAM_BASE: u64 = 0x1_0000;
const RAM_SIZE: usize = 0x4000;
const STACK: u64 = RAM_BASE + 0x1000;
const GDT: u64 = RAM_BASE + 0x2000;
const CS_SLOT: u64 = GDT + 0x10;
const SS_SLOT: u64 = GDT + 0x18;
const USER_CODE64_A0: u64 = 0x00af_fa00_0000_ffff;
const USER_DATA_A0: u64 = 0x00cf_f200_0000_ffff;
const IF: u64 = 1 << 9;
const RF: u64 = 1 << 16;
const IRETQ: [u8; 2] = [0x48, 0xcf];

struct Memory {
    kernel: Vec<u8>,
    user: Vec<u8>,
    ram: Vec<u8>,
    fetches: Vec<u64>,
    reads: Vec<(u64, usize, AccessOrigin)>,
    writes: usize,
    fault_pages: Vec<u64>,
    /// The write with this one-based number faults.
    fail_write_number: Option<usize>,
}

impl Memory {
    fn new() -> Self {
        Self {
            kernel: IRETQ.to_vec(),
            user: Vec::new(),
            ram: vec![0; RAM_SIZE],
            fetches: Vec::new(),
            reads: Vec::new(),
            writes: 0,
            fault_pages: Vec::new(),
            fail_write_number: None,
        }
    }

    fn put(&mut self, address: u64, value: u64) {
        let offset = (address - RAM_BASE) as usize;
        self.ram[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn get(&self, address: u64) -> u64 {
        let offset = (address - RAM_BASE) as usize;
        u64::from_le_bytes(self.ram[offset..offset + 8].try_into().unwrap())
    }

    fn page_fault(&self, address: u64, length: usize) -> Option<MemoryError> {
        let first = address & !0xfff;
        let last = (address + length as u64 - 1) & !0xfff;
        self.fault_pages
            .iter()
            .find(|page| (first..=last).contains(page))
            .map(|_| {
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(address),
                })
            })
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.fetches.push(address);
        let (base, bytes) = if address >= USER_RIP {
            (USER_RIP, &self.user)
        } else {
            (KERNEL_RIP, &self.kernel)
        };
        bytes
            .get(address.wrapping_sub(base) as usize)
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

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        self.writes += 1;
        if self.fail_write_number == Some(self.writes) {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(address),
            }));
        }
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

struct NoPort;

impl PortIo for NoPort {
    type Error = ();

    fn read_u8(&mut self, _: u16) -> Result<u8, ()> {
        panic!("no port access expected")
    }

    fn write_u8(&mut self, _: u16, _: u8) -> Result<(), ()> {
        panic!("no port access expected")
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

fn set() -> Cpl0ProfileSet<'static> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
}

/// CPL0 state about to execute the `IRETQ` at `KERNEL_RIP`; the frame is in `user_frame`.
fn kernel_state() -> LongModeState {
    let mut state = LongModeState {
        rip: KERNEL_RIP,
        rflags: 2 | IF,
        cr0: Some(0x8001_0033),
        cr4: Some(0x20),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ss_selector: Some(0x10),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0x1234,
            limit: 0xfff,
            access: 0x93,
            flags: 0x4,
        }),
        ss_unusable: true,
        gdtr: Some(od_isa::state::Gdtr {
            base: GDT,
            limit: 0x1f,
        }),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = STACK;
    state.gpr[0] = 0x1111_2222_3333_4444;
    state
}

/// Frame `[RIP][CS 0x13][RFLAGS][RSP][SS 0x1b]` (GDT slots 2 and 3, RPL 3, DPL 3, A = 0).
fn user_frame() -> Memory {
    let mut memory = Memory::new();
    memory.put(STACK, USER_RIP);
    memory.put(STACK + 8, 0x13);
    memory.put(STACK + 16, 0x202);
    memory.put(STACK + 24, USER_RSP);
    memory.put(STACK + 32, 0x1b);
    memory.put(CS_SLOT, USER_CODE64_A0);
    memory.put(SS_SLOT, USER_DATA_A0);
    memory
}

fn run0(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    run_with_cpl0_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring0, budget),
        0,
        set,
        None,
        || false,
    )
    .unwrap()
}

fn run3(
    state: &mut LongModeState,
    memory: &mut Memory,
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    run_with_cpl3_profile_set(
        state,
        memory,
        config(PrivilegeLevel::Ring3, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap()
}

fn entered(rflags: u64) -> Cpl0ProfileSetExit {
    Cpl0ProfileSetExit::UserModeEntered(IretqUserEntry {
        rip: USER_RIP,
        cs: 0x13,
        rflags,
        rsp: USER_RSP,
        ss: 0x1b,
    })
}

fn unsupported_cf() -> Cpl0ProfileSetExit {
    Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0xcf })
}

#[test]
fn without_the_member_the_return_to_cpl3_stays_the_user_entry_boundary() {
    let mut state = kernel_state();
    let mut memory = user_frame();
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run0(&mut state, &mut memory, &set(), 1);
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::UserEntry(IretqUserEntry {
            rip: USER_RIP,
            cs: 0x13,
            rflags: 0x202,
            rsp: USER_RSP,
            ss: 0x1b,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);
    assert_eq!(memory.writes, 0);
}

#[test]
fn with_the_member_the_return_to_cpl3_commits_and_the_run_stops_before_a_fetch() {
    let mut state = kernel_state();
    let mut memory = user_frame();
    // A budget of 5 and user code that would run: the run still stops right after the `IRETQ`.
    memory.user = vec![0xb8, 1, 0, 0, 0];
    let mut expected = state.clone();
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 5);
    assert_eq!(result.exit, entered(0x202));
    assert_eq!(result.instructions_executed, 1);
    // Same flag definition as the retired CPL0 return (the loaded flags become defined).
    assert_eq!(result.rflags_defined, od_isa::POPFQ_INTEGER_FLAGS);
    expected.rip = USER_RIP;
    expected.cs_selector = Some(0x13);
    expected.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0xfb,
        flags: 0xa,
    });
    expected.ss_selector = Some(0x1b);
    expected.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0xf3,
        flags: 0xc,
    });
    expected.ss_unusable = false;
    expected.gpr[Register::Rsp as usize] = USER_RSP;
    expected.rflags = 0x202;
    assert_eq!(state, expected);
    // Accessed bits written, one qword each; nothing else.
    assert_eq!(memory.writes, 2);
    assert_eq!(memory.get(CS_SLOT), USER_CODE64_A0 | (1 << 40));
    assert_eq!(memory.get(SS_SLOT), USER_DATA_A0 | (1 << 40));
    // Only the `IRETQ` bytes were fetched: no fetch at the user RIP.
    assert!(memory.fetches.iter().all(|address| *address < USER_RIP));
    // Frame (5 qwords) then CS and SS descriptors as descriptor-table reads.
    assert_eq!(
        memory.reads,
        vec![
            (STACK, 8, AccessOrigin::Explicit),
            (STACK + 8, 8, AccessOrigin::Explicit),
            (STACK + 16, 8, AccessOrigin::Explicit),
            (STACK + 24, 8, AccessOrigin::Explicit),
            (STACK + 32, 8, AccessOrigin::Explicit),
            (CS_SLOT, 8, AccessOrigin::DescriptorTable),
            (SS_SLOT, 8, AccessOrigin::DescriptorTable),
        ]
    );
}

#[test]
fn the_pio_entry_commits_too() {
    let mut state = kernel_state();
    let mut memory = user_frame();
    let result = run_with_cpl0_profile_set_and_pio(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 5),
        0,
        &set().with_user_mode(),
        None,
        &mut NoPort,
        || false,
    )
    .unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::<()>::UserModeEntered(IretqUserEntry {
            rip: USER_RIP,
            cs: 0x13,
            rflags: 0x202,
            rsp: USER_RSP,
            ss: 0x1b,
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, USER_RIP);
}

#[test]
fn rflags_follow_the_mask_rf_cleared_bit_one_set_iopl_if_and_nt_loaded() {
    const STATUS_DF: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x400 | 0x800;
    const LOADED: u64 = STATUS_DF | IF | (3 << 12) | (1 << 14) | (1 << 18) | (1 << 21);
    // Bits IRET never loads: reserved bits 3, 5, 15, 22; RF is loaded by the CPL0 return only.
    const NEVER_LOADED: u64 = (1 << 3) | (1 << 5) | (1 << 15) | (1 << 22);
    let mut state = kernel_state();
    // Current RF and a current reserved bit: RF is cleared, the reserved bit is kept.
    state.rflags = 2 | RF | (1 << 3);
    let mut memory = user_frame();
    memory.put(STACK + 16, LOADED | RF | NEVER_LOADED);
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    let loaded = 2 | LOADED | (1 << 3);
    assert_eq!(state.rflags, loaded);
    assert_eq!(result.exit, entered(loaded));

    // The frame clears what it clears: current CF/IF/IOPL are replaced, bit 1 stays one.
    let mut state = kernel_state();
    state.rflags = 2 | 1 | IF | (3 << 12);
    let mut memory = user_frame();
    memory.put(STACK + 16, 0);
    run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(state.rflags, 2);
}

#[test]
fn after_the_commit_the_cpl3_entry_runs_an_allowed_form_then_stops_on_a_forbidden_one() {
    let mut state = kernel_state();
    let mut memory = user_frame();
    // `mov $0x53, %eax; syscall`, then `iretq` as the other forbidden form.
    memory.user = vec![0xb8, 0x53, 0, 0, 0, 0x0f, 0x05];
    let set = set().with_user_mode();
    let first = run0(&mut state, &mut memory, &set, 5);
    assert_eq!(first.exit, entered(0x202));
    let reads_after_commit = memory.reads.len();
    let second = run3(&mut state, &mut memory, &set, 5);
    assert_eq!(
        second.exit,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x05 })
    );
    assert_eq!(second.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rax as usize], 0x53);
    assert_eq!(state.rip, USER_RIP + 5);
    // Nothing of the CPL3 steps touched data memory.
    assert_eq!(memory.reads.len(), reads_after_commit);
    assert_eq!(memory.writes, 2);

    // The forbidden form right after the commit: no execution, no mutation.
    let mut state = kernel_state();
    let mut memory = user_frame();
    memory.user = vec![0x0f, 0x05];
    run0(&mut state, &mut memory, &set, 5);
    let before = state.clone();
    let result = run3(&mut state, &mut memory, &set, 5);
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x05 })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    // `IRETQ` is not on the allow-list, committed set or not.
    let mut state = kernel_state();
    let mut memory = user_frame();
    memory.user = IRETQ.to_vec();
    run0(&mut state, &mut memory, &set, 5);
    let before = state.clone();
    let result = run3(&mut state, &mut memory, &set, 5);
    assert_eq!(result.exit, unsupported_cf());
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn a_committed_state_is_admitted_by_the_cpl3_entry() {
    let mut state = kernel_state();
    let mut memory = user_frame();
    run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    // Budget 0 at the boundary: admission passes, nothing runs.
    let result = run3(&mut state, &mut memory, &set().with_user_mode(), 0);
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    );
    assert_eq!(result.instructions_executed, 0);
}

/// Run the committing return after `tweak` and expect a refusal: `Unsupported { 0xcf }`, nothing
/// retired, state, RAM and write count unchanged.
fn assert_commit_refused(tweak: impl FnOnce(&mut LongModeState, &mut Memory), label: &str) {
    let mut state = kernel_state();
    let mut memory = user_frame();
    tweak(&mut state, &mut memory);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(result.exit, unsupported_cf(), "{label}");
    assert_eq!(result.instructions_executed, 0, "{label}");
    assert_eq!(state, before, "{label}");
    assert_eq!(memory.ram, ram, "{label}");
    assert_eq!(memory.writes, 0, "{label}");
}

fn data_cache(access: u8) -> DataSegmentDescriptorCache {
    DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access,
        flags: 0xc,
    }
}

#[test]
fn a_data_segment_register_with_a_dpl_below_three_refuses_the_commit() {
    assert_commit_refused(
        |state, _| {
            state.ds_selector = 0x18;
            state.ds_cache = Some(data_cache(0x93));
        },
        "DS DPL 0",
    );
    assert_commit_refused(
        |state, _| {
            state.es_selector = 0x2a;
            state.es_cache = Some(data_cache(0xd3));
        },
        "ES DPL 2",
    );
    assert_commit_refused(
        |state, _| {
            state.fs_selector = 0x18;
            state.fs_cache = Some(FsDescriptorCache {
                limit: u32::MAX,
                access: 0x93,
                flags: 0xc,
            });
        },
        "FS DPL 0",
    );
    assert_commit_refused(
        |state, _| {
            state.gs_selector = 0x19;
            state.gs_cache = Some(FsDescriptorCache {
                limit: u32::MAX,
                access: 0xb3,
                flags: 0xc,
            });
        },
        "GS DPL 1",
    );
    // A non-null selector whose cache is not modeled cannot be shown to be DPL 3.
    assert_commit_refused(|state, _| state.ds_selector = 0x2b, "DS without a cache");
}

#[test]
fn null_and_dpl3_data_segment_registers_do_not_stop_the_commit() {
    let mut state = kernel_state();
    state.ds_selector = 0x2b;
    state.ds_cache = Some(data_cache(0xf3));
    state.es_selector = 0x3; // null with RPL bits, cache not modeled
    state.fs_selector = 0;
    let mut memory = user_frame();
    let ds = (state.ds_selector, state.ds_cache);
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(result.exit, entered(0x202));
    // DS/ES/FS/GS are left as they were.
    assert_eq!((state.ds_selector, state.ds_cache), ds);
    assert_eq!(state.es_selector, 3);
}

#[test]
fn the_frame_refusals_of_the_return_to_user_mode_still_refuse_with_the_member() {
    // Null SS (with and without RPL bits): not modeled.
    assert_commit_refused(|_, memory| memory.put(STACK + 32, 0), "null SS");
    assert_commit_refused(|_, memory| memory.put(STACK + 32, 3), "null SS RPL 3");
    // RPL 1 and 2 returns, a compatibility-mode CS, TF in the frame: not modeled.
    assert_commit_refused(|_, memory| memory.put(STACK + 8, 0x11), "CS RPL 1");
    assert_commit_refused(|_, memory| memory.put(STACK + 8, 0x12), "CS RPL 2");
    assert_commit_refused(
        |_, memory| memory.put(CS_SLOT, 0x00cf_fa00_0000_ffff),
        "compatibility CS",
    );
    assert_commit_refused(|_, memory| memory.put(STACK + 16, 0x302), "TF in the frame");
    // No GDTR, shadow active: not modeled.
    assert_commit_refused(|state, _| state.gdtr = None, "no GDTR");
}

#[test]
fn guest_faults_of_the_frame_checks_are_the_same_with_the_member_and_change_nothing() {
    // CS of DPL 0 behind an RPL 3 selector: #GP(0x13); SS with a mismatching RPL: #GP(0x1a).
    for (tweak, vector, code) in [
        (
            Box::new(|memory: &mut Memory| memory.put(CS_SLOT, 0x00af_9a00_0000_ffff))
                as Box<dyn Fn(&mut Memory)>,
            13_u8,
            0x10_u32,
        ),
        (
            Box::new(|memory: &mut Memory| memory.put(STACK + 32, 0x1a)),
            13,
            0x18,
        ),
        // Non-canonical RIP.
        (
            Box::new(|memory: &mut Memory| memory.put(STACK, 0x0000_8000_0000_0000)),
            13,
            0,
        ),
    ] {
        let mut state = kernel_state();
        let mut memory = user_frame();
        tweak(&mut memory);
        let before = state.clone();
        let ram = memory.ram.clone();
        let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault {
                vector,
                error_code: Some(code),
                linear_address: None,
            }))
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.ram, ram);
        assert_eq!(memory.writes, 0);
    }
}

#[test]
fn a_read_failure_changes_nothing_and_writes_nothing() {
    for (page, label) in [(STACK, "frame page"), (GDT, "descriptor page")] {
        let mut state = kernel_state();
        let mut memory = user_frame();
        memory.fault_pages = vec![page];
        let before = state.clone();
        let ram = memory.ram.clone();
        let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
        assert!(
            matches!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault { vector: 14, .. }))
            ),
            "{label}: {:?}",
            result.exit
        );
        assert_eq!(result.instructions_executed, 0, "{label}");
        assert_eq!(state, before, "{label}");
        assert_eq!(memory.ram, ram, "{label}");
        assert_eq!(memory.writes, 0, "{label}");
    }
}

#[test]
fn an_accessed_bit_write_failure_leaves_the_cpu_untouched() {
    // First write (CS accessed bit) fails: nothing at all changed.
    let mut state = kernel_state();
    let mut memory = user_frame();
    memory.fail_write_number = Some(1);
    let before = state.clone();
    let ram = memory.ram.clone();
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert!(matches!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault { vector: 14, .. }))
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.ram, ram);

    // Second write (SS accessed bit) fails: the CPU is untouched; the CS accessed bit written
    // before stays set (as for the CPL0 return, see CPU_CONTRACT S4-38L5b).
    let mut state = kernel_state();
    let mut memory = user_frame();
    memory.fail_write_number = Some(2);
    let before = state.clone();
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert!(matches!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault { vector: 14, .. }))
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.get(CS_SLOT), USER_CODE64_A0 | (1 << 40));
    assert_eq!(memory.get(SS_SLOT), USER_DATA_A0);
}

#[test]
fn a_same_privilege_iretq_is_unaffected_by_the_member() {
    // Frame back to CPL0: CS 0x10 (code, DPL 0), SS 0x18 (data, DPL 0), as the plain return.
    for member in [false, true] {
        let mut state = kernel_state();
        let mut memory = user_frame();
        memory.put(STACK, KERNEL_RIP + 2);
        memory.put(STACK + 8, 0x10);
        memory.put(STACK + 24, STACK + 0x800);
        memory.put(STACK + 32, 0x18);
        memory.put(CS_SLOT, 0x00af_9b00_0000_ffff);
        memory.put(SS_SLOT, 0x00cf_9300_0000_ffff);
        let set = if member {
            set().with_user_mode()
        } else {
            set()
        };
        let result = run0(&mut state, &mut memory, &set, 1);
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted),
            "member {member}"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, KERNEL_RIP + 2);
        assert_eq!(state.cs_selector, Some(0x10));
    }
}

#[test]
fn a_conforming_cs_with_a_dpl_below_three_refuses_the_commit_and_keeps_the_boundary() {
    // S4-39c (review S4-39b, note 2): the return passes the outer-level checks (conforming,
    // DPL <= RPL) but would leave a CS cache the CPL3 entry does not admit.
    for dpl in 0..3u64 {
        let descriptor = 0x00af_0000_0000_ffff | ((0x9c | (dpl << 5)) << 40);
        assert_commit_refused(
            |_, memory| memory.put(CS_SLOT, descriptor),
            &format!("conforming CS DPL {dpl}"),
        );
        // Without the member the same frame is still the validated boundary, as in S4-38L9H.
        let mut state = kernel_state();
        let mut memory = user_frame();
        memory.put(CS_SLOT, descriptor);
        let before = state.clone();
        let result = run0(&mut state, &mut memory, &set(), 1);
        assert!(
            matches!(result.exit, Cpl0ProfileSetExit::UserEntry(_)),
            "DPL {dpl}: {:?}",
            result.exit
        );
        assert_eq!(state, before);
    }
    // A conforming CS with DPL 3 commits.
    let mut state = kernel_state();
    let mut memory = user_frame();
    memory.put(CS_SLOT, 0x00af_0000_0000_ffff | (0xfc << 40));
    let result = run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(result.exit, entered(0x202));
}

#[test]
fn the_cpl0_entries_refuse_a_state_whose_cs_has_rpl_3() {
    // S4-39c (review S4-39b, note 3): the state the commit leaves is for the CPL3 entry only.
    let mut state = kernel_state();
    let mut memory = user_frame();
    run0(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(state.cs_selector, Some(0x13));
    memory.user = vec![0xb8, 1, 0, 0, 0];
    let before = state.clone();
    let fetches = memory.fetches.len();
    for member in [false, true] {
        let set = if member {
            set().with_user_mode()
        } else {
            set()
        };
        let plain = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        );
        assert_eq!(
            plain,
            Err(od_isa::execute::Cpl0ProfileSetInputError::WrongPrivilege)
        );
        let with_port = run_with_cpl0_profile_set_and_pio(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            &mut NoPort,
            || false,
        );
        assert_eq!(
            with_port,
            Err(od_isa::execute::Cpl0ProfileSetInputError::WrongPrivilege)
        );
    }
    assert_eq!(state, before);
    assert_eq!(memory.fetches.len(), fetches, "nothing was fetched");
    // The CPL3 entry still runs it.
    let result = run3(&mut state, &mut memory, &set().with_user_mode(), 1);
    assert_eq!(result.instructions_executed, 1);
}
