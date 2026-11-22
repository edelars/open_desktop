//! S2-26h: the Windows-boot MSR file of the CPL0 profile set (`Cpl0ProfileSet::with_msr_windows`).
//!
//! Every expectation is a model of the SDM (Vol. 2B RDMSR, Vol. 2D WRMSR, Vol. 3A 14.12.4 for
//! IA32_PAT, Vol. 3A Figure 6-14 for IA32_STAR/IA32_FMASK, Vol. 4 Table 2-2 for IA32_SYSENTER_CS and
//! IA32_MISC_ENABLE); RDMSR/WRMSR cannot run in user space (only the CPL3 `#GP(0)` of a bare RDMSR
//! is observable there, and no oracle count was taken for it), so there is no hardware oracle count
//! for this slice (all rows are modelled).

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, RunConfig, RunExit, run_with_cpl0_profile_set, run_with_cpl3_profile_set,
};
use od_isa::msr::{
    IA32_MISC_ENABLE_RESET, IA32_PAT_RESET, MsrProfile, WindowsMsrWrite,
    windows_msr_cell_value_valid, windows_msr_is_known, windows_msr_stored_value,
    windows_msr_write_decision,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CodeSegmentDescriptorCache,
    CpuActivity, DataSegmentDescriptorCache, GuestMemory, InstructionMode, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};
use std::cell::Cell;

const RIP: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const RDMSR: [u8; 2] = [0x0f, 0x32];
const WRMSR: [u8; 2] = [0x0f, 0x30];

const IA32_APIC_BASE: u32 = 0x1b;
const IA32_SYSENTER_CS: u32 = 0x174;
const IA32_SYSENTER_ESP: u32 = 0x175;
const IA32_SYSENTER_EIP: u32 = 0x176;
const IA32_MISC_ENABLE: u32 = 0x1a0;
const IA32_PAT: u32 = 0x277;
const IA32_EFER: u32 = 0xc000_0080;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_CSTAR: u32 = 0xc000_0083;
const IA32_FMASK: u32 = 0xc000_0084;
const IA32_FS_BASE: u32 = 0xc000_0100;
const IA32_GS_BASE: u32 = 0xc000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;

/// The ten MSRs whose value lives in a cell.
const CELL_OWNED: [u32; 10] = [
    IA32_SYSENTER_CS,
    IA32_SYSENTER_ESP,
    IA32_SYSENTER_EIP,
    IA32_MISC_ENABLE,
    IA32_PAT,
    IA32_STAR,
    IA32_LSTAR,
    IA32_CSTAR,
    IA32_FMASK,
    IA32_KERNEL_GS_BASE,
];

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

fn board_state() -> LongModeState {
    LongModeState {
        rip: RIP,
        rflags: 2 | (1 << 9),
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    }
}

fn user_state() -> LongModeState {
    LongModeState {
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
        ..board_state()
    }
}

fn exit(result: &Cpl0ProfileSetResult) -> &RunExit {
    match &result.exit {
        Cpl0ProfileSetExit::Run(exit) => exit,
        other => panic!("expected a Run exit, got {other:?}"),
    }
}

fn gp0() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

static EFER_PROFILE: MsrProfile = MsrProfile::EferReadOnlyV1(0xd00);

/// The caller-owned MSR cells of one Windows set, at their power-up values.
struct Cells {
    efer: Cell<u64>,
    star: Cell<u64>,
    lstar: Cell<u64>,
    cstar: Cell<u64>,
    mask: Cell<u64>,
    sysenter_cs: Cell<u64>,
    sysenter_esp: Cell<u64>,
    sysenter_eip: Cell<u64>,
    kernel_gs_base: Cell<u64>,
    pat: Cell<u64>,
    misc_enable: Cell<u64>,
}

impl Cells {
    fn new() -> Self {
        Self {
            efer: Cell::new(0xd00),
            star: Cell::new(0),
            lstar: Cell::new(0),
            cstar: Cell::new(0),
            mask: Cell::new(0),
            sysenter_cs: Cell::new(0),
            sysenter_esp: Cell::new(0),
            sysenter_eip: Cell::new(0),
            kernel_gs_base: Cell::new(0),
            pat: Cell::new(IA32_PAT_RESET),
            misc_enable: Cell::new(IA32_MISC_ENABLE_RESET),
        }
    }

    /// The cell of a cell-owned index.
    fn cell(&self, index: u32) -> &Cell<u64> {
        match index {
            IA32_SYSENTER_CS => &self.sysenter_cs,
            IA32_SYSENTER_ESP => &self.sysenter_esp,
            IA32_SYSENTER_EIP => &self.sysenter_eip,
            IA32_MISC_ENABLE => &self.misc_enable,
            IA32_PAT => &self.pat,
            IA32_STAR => &self.star,
            IA32_LSTAR => &self.lstar,
            IA32_CSTAR => &self.cstar,
            IA32_FMASK => &self.mask,
            IA32_KERNEL_GS_BASE => &self.kernel_gs_base,
            _ => panic!("{index:#x} is not cell owned"),
        }
    }

    /// The members the Windows member builds on, without the Windows member itself.
    fn members(&self) -> Cpl0ProfileSet<'_> {
        Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_msr(&EFER_PROFILE)
            .with_msr_efer_write(&self.efer)
            .with_msr_fs_gs_base()
            .with_msr_apic_base()
            .with_msr_star_write(&self.star)
            .with_msr_lstar_write(&self.lstar)
            .with_msr_cstar_write(&self.cstar)
            .with_msr_syscall_mask_write(&self.mask)
            .with_msr_sysenter_cs_write(&self.sysenter_cs)
            .with_msr_sysenter_stack_write(&self.sysenter_esp, &self.sysenter_eip)
            .with_msr_kernel_gs_base_write(&self.kernel_gs_base)
    }

    fn windows(&self) -> Cpl0ProfileSet<'_> {
        self.members()
            .with_msr_windows(&self.pat, &self.misc_enable)
    }
}

fn run(
    state: &mut LongModeState,
    code: &[u8],
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Result<Cpl0ProfileSetResult, Cpl0ProfileSetInputError> {
    let mut memory = Memory::code(code);
    run_with_cpl0_profile_set(
        state,
        &mut memory,
        config(PrivilegeLevel::Ring0, budget),
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
}

/// One bare `RDMSR`/`WRMSR` of `index` with EDX:EAX = `value` and distinct high register halves.
fn access(
    set: &Cpl0ProfileSet<'_>,
    code: [u8; 2],
    index: u32,
    value: u64,
) -> (Cpl0ProfileSetResult, LongModeState, LongModeState) {
    let mut state = board_state();
    state.gpr[Register::Rcx as usize] = 0xdead_beef_0000_0000 | u64::from(index);
    state.gpr[Register::Rax as usize] = 0x1111_1111_0000_0000 | (value & 0xffff_ffff);
    state.gpr[Register::Rdx as usize] = 0x2222_2222_0000_0000 | (value >> 32);
    state.gpr[Register::Rbx as usize] = 0x5555;
    let before = state.clone();
    let result = run(&mut state, &code, set, 1).unwrap();
    (result, before, state)
}

fn assert_gp0_unchanged(
    result: &Cpl0ProfileSetResult,
    before: &LongModeState,
    after: &LongModeState,
    label: &str,
) {
    assert_eq!(exit(result), &gp0(), "{label}");
    assert_eq!(result.instructions_executed, 0, "{label}");
    assert_eq!(after, before, "{label}");
}

#[test]
fn rdmsr_returns_every_cell_owned_msr_in_edx_eax() {
    let values: [(u32, u64); 10] = [
        (IA32_SYSENTER_CS, 0x0000_0000_0000_0010),
        (IA32_SYSENTER_ESP, 0xffff_8000_dead_b000),
        (IA32_SYSENTER_EIP, 0xffff_f800_1234_5678),
        (IA32_MISC_ENABLE, 0x1801),
        (IA32_PAT, 0x0007_0406_0007_0406),
        (IA32_STAR, 0x0023_0010_0000_0000),
        (IA32_LSTAR, 0xffff_f800_0000_1000),
        (IA32_CSTAR, 0xffff_f800_0000_2000),
        (IA32_FMASK, 0x0000_0000_0004_4700),
        (IA32_KERNEL_GS_BASE, 0xffff_9000_0000_0040),
    ];
    for (index, value) in values {
        let cells = Cells::new();
        cells.cell(index).set(value);
        let set = cells.windows();
        let (result, before, after) = access(&set, RDMSR, index, 0);
        let label = format!("rdmsr {index:#x}");
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{label}"
        );
        assert_eq!(result.instructions_executed, 1, "{label}");
        // EAX and EDX are zero-extended, RCX high half ignored, nothing else touched.
        let mut expected = before.clone();
        expected.gpr[Register::Rax as usize] = value & 0xffff_ffff;
        expected.gpr[Register::Rdx as usize] = value >> 32;
        expected.rip = RIP + 2;
        assert_eq!(after, expected, "{label}");
        assert_eq!(cells.cell(index).get(), value, "{label}");
    }
}

#[test]
fn power_up_values_are_the_documented_ones() {
    let cells = Cells::new();
    let set = cells.windows();
    let (_, _, state) = access(&set, RDMSR, IA32_PAT, 0);
    // PA0 WB, PA1 WT, PA2 UC-, PA3 UC, repeated (SDM Vol. 3A 14.12.4).
    assert_eq!(state.gpr[Register::Rax as usize], 0x0007_0406);
    assert_eq!(state.gpr[Register::Rdx as usize], 0x0007_0406);
    let (_, _, state) = access(&set, RDMSR, IA32_MISC_ENABLE, 0);
    // Bit 0 fast strings, bit 11 BTS unavailable, bit 12 PEBS unavailable (SDM Vol. 4 Table 2-2).
    assert_eq!(IA32_MISC_ENABLE_RESET, 0x1801);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1801);
    assert_eq!(state.gpr[Register::Rdx as usize], 0);
}

#[test]
fn wrmsr_stores_valid_values_and_changes_nothing_else() {
    let valid: [(u32, u64); 13] = [
        (IA32_SYSENTER_CS, 0x10),
        (IA32_SYSENTER_CS, 0xffff_ffff),
        // Bits 63:32 are ignored on write and read as zero (SDM Vol. 4 Table 2-2, 174H).
        (IA32_SYSENTER_CS, 0x1_0000_0010),
        (IA32_SYSENTER_CS, u64::MAX),
        (IA32_SYSENTER_ESP, 0xffff_8000_dead_b000),
        (IA32_SYSENTER_EIP, 0x0000_7fff_ffff_ffff),
        (IA32_PAT, 0x0007_0106_0405_0006),
        (IA32_STAR, 0xffff_ffff_0000_0000),
        (IA32_LSTAR, 0xffff_f800_0000_1000),
        (IA32_CSTAR, 0x8000_0000_0000_0000),
        (IA32_FMASK, 0x0000_0000_0004_4700),
        (IA32_KERNEL_GS_BASE, 0xffff_8000_0000_0000),
        // Only bit 0 changes; the read-only bits 11 and 12 stay 1.
        (IA32_MISC_ENABLE, 0x1800),
    ];
    for (index, value) in valid {
        let cells = Cells::new();
        let set = cells.windows();
        let (result, before, after) = access(&set, WRMSR, index, value);
        let label = format!("wrmsr {index:#x} {value:#x}");
        assert_eq!(
            exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "{label}"
        );
        assert_eq!(result.instructions_executed, 1, "{label}");
        let mut expected = before.clone();
        expected.rip = RIP + 2;
        assert_eq!(after, expected, "{label}");
        let stored = windows_msr_stored_value(index, value);
        assert_eq!(cells.cell(index).get(), stored, "{label}");
        // The stored value reads back through the same set.
        let (_, _, read) = access(&set, RDMSR, index, 0);
        assert_eq!(
            read.gpr[Register::Rax as usize],
            stored & 0xffff_ffff,
            "{label}"
        );
        assert_eq!(read.gpr[Register::Rdx as usize], stored >> 32, "{label}");
        // Every other cell is untouched.
        for other in CELL_OWNED {
            if other != index {
                let fresh = Cells::new();
                assert_eq!(
                    cells.cell(other).get(),
                    fresh.cell(other).get(),
                    "{label} {other:#x}"
                );
            }
        }
    }
}

#[test]
fn wrmsr_reserved_bits_and_non_canonical_addresses_are_gp0_without_change() {
    const NON_CANONICAL: [u64; 3] = [
        0x0000_8000_0000_0000,
        0xffff_7fff_ffff_ffff,
        0x1234_5678_9abc_def0,
    ];
    let mut bad: Vec<(u32, u64)> = Vec::new();
    // FMASK: bits 63:32 reserved (SYSENTER_CS is not here: its upper half is ignored, not reserved).
    for value in [1u64 << 32, 1 << 63, 0x1_0000_0010] {
        bad.push((IA32_FMASK, value));
    }
    // STAR: bits 31:0 reserved (SDM Vol. 3A Figure 6-14).
    for value in [1u64, 1 << 31, 0x0023_0010_0000_0001, 0xffff_ffff_ffff_ffff] {
        bad.push((IA32_STAR, value));
    }
    // Canonical-address MSRs of the WRMSR page: ESP, EIP, LSTAR, KERNEL_GS_BASE.
    for index in [
        IA32_SYSENTER_ESP,
        IA32_SYSENTER_EIP,
        IA32_LSTAR,
        IA32_KERNEL_GS_BASE,
    ] {
        for value in NON_CANONICAL {
            bad.push((index, value));
        }
    }
    // PAT: reserved memory types (2, 3, 8..=0xff) in any of the eight entries.
    for entry in 0..8 {
        for byte in [2u64, 3, 8, 0x10, 0x80, 0xff] {
            bad.push((IA32_PAT, byte << (entry * 8)));
        }
    }
    for (index, value) in bad {
        let cells = Cells::new();
        let set = cells.windows();
        let before_cell = cells.cell(index).get();
        let (result, before, after) = access(&set, WRMSR, index, value);
        let label = format!("wrmsr {index:#x} {value:#x}");
        assert_gp0_unchanged(&result, &before, &after, &label);
        assert_eq!(cells.cell(index).get(), before_cell, "{label}");
    }
}

#[test]
fn star_takes_any_high_half_and_cstar_any_value() {
    // STAR[63:32] are the SYSCALL/SYSRET selectors and take any value; STAR[31:0] are reserved
    // (Figure 6-14, see the #GP(0) test). CSTAR is not in the WRMSR canonical list.
    for (index, value) in [
        (IA32_STAR, 0u64),
        (IA32_STAR, 0x0023_0010_0000_0000),
        (IA32_STAR, 0xdead_beef_0000_0000),
        (IA32_CSTAR, 0),
        (IA32_CSTAR, 0x0000_8000_0000_0000),
        (IA32_CSTAR, 0xdead_beef_dead_beef),
    ] {
        let cells = Cells::new();
        let set = cells.windows();
        let (result, _, _) = access(&set, WRMSR, index, value);
        assert_eq!(result.instructions_executed, 1, "{index:#x} {value:#x}");
        assert_eq!(cells.cell(index).get(), value, "{index:#x} {value:#x}");
    }
}

#[test]
fn sysenter_cs_upper_half_is_ignored_on_write_and_reads_zero() {
    // SDM Vol. 4 Table 2-2, 174H: bits 63:32 "not used, writes ignored, reads return zero"; bits 31:16
    // can be read and written.
    for (value, stored) in [
        (0x1_0000_0010u64, 0x10u64),
        (0xffff_ffff_ffff_ffff, 0xffff_ffff),
        (0x8000_0000_dead_0008, 0xdead_0008),
    ] {
        let cells = Cells::new();
        let set = cells.windows();
        let (result, _, _) = access(&set, WRMSR, IA32_SYSENTER_CS, value);
        assert_eq!(result.instructions_executed, 1, "{value:#x}");
        assert_eq!(cells.sysenter_cs.get(), stored, "{value:#x}");
        let (_, _, read) = access(&set, RDMSR, IA32_SYSENTER_CS, 0);
        assert_eq!(read.gpr[Register::Rax as usize], stored, "{value:#x}");
        assert_eq!(read.gpr[Register::Rdx as usize], 0, "{value:#x}");
    }
}

#[test]
fn misc_enable_models_only_the_fast_string_bit() {
    let cells = Cells::new();
    let set = cells.windows();
    // Bit 0 toggles; the read-only bits 11 (BTS unavailable) and 12 (PEBS unavailable) stay 1.
    let (result, _, _) = access(&set, WRMSR, IA32_MISC_ENABLE, 0x1800);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cells.misc_enable.get(), 0x1800);
    let (result, _, _) = access(&set, WRMSR, IA32_MISC_ENABLE, 0x1801);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cells.misc_enable.get(), 0x1801);
    // Any other change (here: monitor FSM 18, limit CPUID maxval 22, XD disable 34, reserved bit 1,
    // bit 63, and clearing the read-only bit 11 or 12 or both) is a bit the model lacks: a deliberate
    // fail-closed decision, no state change, not an architectural #GP.
    for value in [
        0x1801u64 | 1 << 18,
        0x1801 | 1 << 22,
        0x1801 | 1 << 34,
        0x1801 | 2,
        0x1801 | 1 << 63,
        0x1001,
        0x0801,
        0x0001,
        1,
        0,
    ] {
        let (result, before, after) = access(&set, WRMSR, IA32_MISC_ENABLE, value);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{value:#x}"
        );
        assert_eq!(result.instructions_executed, 0, "{value:#x}");
        assert_eq!(after, before, "{value:#x}");
        assert_eq!(cells.misc_enable.get(), 0x1801, "{value:#x}");
    }
}

#[test]
fn unknown_indexes_are_gp0_for_rdmsr_and_wrmsr() {
    let unknown: [u32; 24] = [
        0x0,
        0x10, // IA32_TSC: withheld with the TSC policy (S2-57)
        0x17,
        0x1c,
        0x8b,
        0xfe,
        0x3a,
        0x87,
        0x173,
        0x177,
        0x179,
        0x19a,
        0x1a1,
        0x1b0,
        0x200,
        0x201,
        0x276,
        0x278,
        0x2ff,
        0x6e0,
        0x802,
        0xc000_0085,
        0xc000_0103,
        0xdead_beef,
    ];
    for index in unknown {
        let cells = Cells::new();
        let set = cells.windows();
        for (code, name) in [(RDMSR, "rdmsr"), (WRMSR, "wrmsr")] {
            let (result, before, after) = access(&set, code, index, 0);
            assert_gp0_unchanged(&result, &before, &after, &format!("{name} {index:#x}"));
            assert!(!windows_msr_is_known(index));
        }
    }
    // The upper half of RCX is ignored: 0x1_0000_0277 is IA32_PAT, not an unknown index.
    let cells = Cells::new();
    let set = cells.windows();
    let mut state = board_state();
    state.gpr[Register::Rcx as usize] = 0x1_0000_0000 | u64::from(IA32_PAT);
    let result = run(&mut state, &RDMSR, &set, 1).unwrap();
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0007_0406);
}

#[test]
fn the_other_members_keep_their_own_msrs_in_the_windows_set() {
    let cells = Cells::new();
    let set = cells.windows();
    // APIC_BASE: fixed 0xFEE00900.
    let (result, _, state) = access(&set, RDMSR, IA32_APIC_BASE, 0);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rax as usize], 0xfee0_0900);
    assert_eq!(state.gpr[Register::Rdx as usize], 0);
    let (result, _, _) = access(&set, WRMSR, IA32_APIC_BASE, 0xfee0_0900);
    assert_eq!(result.instructions_executed, 1);
    let (result, before, after) = access(&set, WRMSR, IA32_APIC_BASE, 0xfee0_0800);
    assert_eq!(exit(&result), &RunExit::Unsupported { opcode: 0x30 });
    assert_eq!(after, before);
    // EFER: read from the cell, SCE may change.
    let (_, _, state) = access(&set, RDMSR, IA32_EFER, 0);
    assert_eq!(state.gpr[Register::Rax as usize], 0xd00);
    let (result, _, _) = access(&set, WRMSR, IA32_EFER, 0xd01);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cells.efer.get(), 0xd01);
    // FS_BASE / GS_BASE: read and canonical write through the state.
    for (index, base) in [(IA32_FS_BASE, 0), (IA32_GS_BASE, 1)] {
        let (result, _, after) = access(&set, WRMSR, index, 0xffff_8800_0000_1000);
        assert_eq!(result.instructions_executed, 1);
        let stored = if base == 0 {
            after.fs_base
        } else {
            after.gs_base
        };
        assert_eq!(stored, 0xffff_8800_0000_1000);
        let (result, before, after) = access(&set, WRMSR, index, 0x0000_8000_0000_0000);
        assert_gp0_unchanged(&result, &before, &after, &format!("{index:#x}"));
    }
    let mut state = board_state();
    state.fs_base = 0xffff_8000_aaaa_0000;
    state.gpr[Register::Rcx as usize] = u64::from(IA32_FS_BASE);
    run(&mut state, &RDMSR, &set, 1).unwrap();
    assert_eq!(state.gpr[Register::Rax as usize], 0xaaaa_0000);
    assert_eq!(state.gpr[Register::Rdx as usize], 0xffff_8000);
}

#[test]
fn the_tdx_read_member_keeps_its_index_and_a_set_without_it_faults() {
    let cells = Cells::new();
    let with = cells.windows().with_msr_tdx_keyid_partitioning_read();
    let (result, _, state) = access(&with, RDMSR, 0x87, 0);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rax as usize], 0);
    assert_eq!(state.gpr[Register::Rdx as usize], 0);
    // WRMSR of it stays #GP(0): the Windows list has no writable 0x87.
    let (result, before, after) = access(&with, WRMSR, 0x87, 0);
    assert_gp0_unchanged(&result, &before, &after, "wrmsr 0x87");
    let without = cells.windows();
    let (result, before, after) = access(&without, RDMSR, 0x87, 0);
    assert_gp0_unchanged(&result, &before, &after, "rdmsr 0x87 without the member");
}

#[test]
fn prefixed_and_other_forms_keep_their_refusal() {
    let cells = Cells::new();
    let set = cells.windows();
    for bytes in [
        &[0x66, 0x0f, 0x32][..],
        &[0x66, 0x0f, 0x30][..],
        &[0x48, 0x0f, 0x32][..],
        &[0xf3, 0x0f, 0x30][..],
    ] {
        for index in [IA32_PAT, 0x10] {
            let mut state = board_state();
            state.gpr[Register::Rcx as usize] = u64::from(index);
            let before = state.clone();
            let result = run(&mut state, bytes, &set, 1).unwrap();
            assert!(
                matches!(exit(&result), RunExit::Unsupported { .. }),
                "{bytes:02x?} {index:#x}: {:?}",
                result.exit
            );
            assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
            assert_eq!(state, before, "{bytes:02x?}");
        }
    }
}

#[test]
fn every_access_at_cpl3_is_gp0_without_change() {
    let cells = Cells::new();
    let set = cells.windows();
    for index in [IA32_PAT, IA32_LSTAR, IA32_EFER, IA32_APIC_BASE, 0x10] {
        for code in [RDMSR, WRMSR] {
            let mut state = user_state();
            state.gpr[Register::Rcx as usize] = u64::from(index);
            let before = state.clone();
            let mut memory = Memory::code(&code);
            let result = run_with_cpl3_profile_set(
                &mut state,
                &mut memory,
                config(PrivilegeLevel::Ring3, 1),
                od_isa::INTEGER_FLAGS,
                &set,
                None,
                || false,
            )
            .unwrap();
            assert_gp0_unchanged(&result, &before, &state, &format!("{code:02x?} {index:#x}"));
        }
    }
    assert_eq!(cells.pat.get(), IA32_PAT_RESET);
}

#[test]
fn admission_needs_every_member_and_valid_cells_before_any_fetch() {
    let cells = Cells::new();
    let base = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let full = |skip: usize| {
        let mut set = base.with_msr_windows(&cells.pat, &cells.misc_enable);
        if skip != 0 {
            set = set.with_msr(&EFER_PROFILE);
        }
        if skip != 1 {
            set = set.with_msr_efer_write(&cells.efer);
        }
        if skip != 2 {
            set = set.with_msr_fs_gs_base();
        }
        if skip != 3 {
            set = set.with_msr_apic_base();
        }
        if skip != 4 {
            set = set.with_msr_star_write(&cells.star);
        }
        if skip != 5 {
            set = set.with_msr_lstar_write(&cells.lstar);
        }
        if skip != 6 {
            set = set.with_msr_cstar_write(&cells.cstar);
        }
        if skip != 7 {
            set = set.with_msr_syscall_mask_write(&cells.mask);
        }
        if skip != 8 {
            set = set.with_msr_sysenter_cs_write(&cells.sysenter_cs);
        }
        if skip != 9 {
            set = set.with_msr_sysenter_stack_write(&cells.sysenter_esp, &cells.sysenter_eip);
        }
        if skip != 10 {
            set = set.with_msr_kernel_gs_base_write(&cells.kernel_gs_base);
        }
        set
    };
    for skip in 0..=10 {
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(&RDMSR);
        let error = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            od_isa::INTEGER_FLAGS,
            &full(skip),
            None,
            || false,
        )
        .unwrap_err();
        assert_eq!(
            error,
            Cpl0ProfileSetInputError::WindowsMsrMembersMissing,
            "skip {skip}"
        );
        assert_eq!((memory.fetches, &state), (0, &before), "skip {skip}");
    }
    // A set with the whole list is admitted (the skip index 11 drops nothing).
    let mut state = board_state();
    state.gpr[Register::Rcx as usize] = u64::from(IA32_PAT);
    assert!(run(&mut state, &RDMSR, &full(11), 1).is_ok());

    // A cell that its MSR could not hold is refused at entry, one cell at a time.
    for (index, value) in [
        (IA32_SYSENTER_CS, 1u64 << 32),
        (IA32_SYSENTER_ESP, 0x0000_8000_0000_0000),
        (IA32_SYSENTER_EIP, 0x0000_8000_0000_0000),
        (IA32_LSTAR, 0x0000_8000_0000_0000),
        (IA32_KERNEL_GS_BASE, 0x0000_8000_0000_0000),
        (IA32_FMASK, 1 << 32),
        (IA32_STAR, 1),
        (IA32_PAT, 0x02),
        (IA32_MISC_ENABLE, 1 << 34),
        (IA32_MISC_ENABLE, 0x1801 | 1 << 34),
        // The read-only bits 11 and 12 always read 1.
        (IA32_MISC_ENABLE, 1),
        (IA32_MISC_ENABLE, 0x1000),
    ] {
        let cells = Cells::new();
        cells.cell(index).set(value);
        let mut state = board_state();
        let before = state.clone();
        let mut memory = Memory::code(&RDMSR);
        let error = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(PrivilegeLevel::Ring0, 1),
            od_isa::INTEGER_FLAGS,
            &cells.windows(),
            None,
            || false,
        )
        .unwrap_err();
        assert_eq!(
            error,
            Cpl0ProfileSetInputError::WindowsMsrCellInvalid,
            "{index:#x}"
        );
        assert_eq!((memory.fetches, &state), (0, &before), "{index:#x}");
    }
}

#[test]
fn without_the_windows_member_the_existing_answers_are_unchanged() {
    let cells = Cells::new();
    let set = cells.members();
    // RDMSR of a write-only cell index stays Unsupported (the previous slices' refusal).
    for index in [IA32_STAR, IA32_LSTAR, IA32_SYSENTER_CS, IA32_KERNEL_GS_BASE] {
        let (result, before, after) = access(&set, RDMSR, index, 0);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x32 },
            "{index:#x}"
        );
        assert_eq!(after, before);
    }
    // PAT and MISC_ENABLE are unmodelled indexes: RDMSR is #GP(0) (S4-38L9v), WRMSR Unsupported.
    for index in [IA32_PAT, IA32_MISC_ENABLE] {
        let (result, before, after) = access(&set, RDMSR, index, 0);
        assert_gp0_unchanged(&result, &before, &after, &format!("{index:#x}"));
        let (result, before, after) = access(&set, WRMSR, index, 0);
        assert_eq!(
            exit(&result),
            &RunExit::Unsupported { opcode: 0x30 },
            "{index:#x}"
        );
        assert_eq!(after, before);
    }
    // The legacy write cell accepts a value the Windows rules would refuse.
    let (result, _, _) = access(&set, WRMSR, IA32_LSTAR, 0x0000_8000_0000_0000);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(cells.lstar.get(), 0x0000_8000_0000_0000);
}

#[test]
fn a_replaced_efer_profile_is_refused_at_entry() {
    // SysenterProbeV1 handles 0x174..=0x176; replacing the EFER profile with it drops what the
    // Windows member builds on, so the set is refused before any fetch.
    let cells = Cells::new();
    let probe = MsrProfile::SysenterProbeV1;
    let mut state = board_state();
    let mut memory = Memory::code(&RDMSR);
    let error = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        config(PrivilegeLevel::Ring0, 1),
        od_isa::INTEGER_FLAGS,
        &cells.windows().with_msr(&probe),
        None,
        || false,
    )
    .unwrap_err();
    assert_eq!(error, Cpl0ProfileSetInputError::WindowsMsrMembersMissing);
}

#[test]
fn the_decision_table_matches_the_documented_rules() {
    use WindowsMsrWrite::{Accept, GeneralProtection, Unsupported};
    assert_eq!(
        windows_msr_write_decision(IA32_SYSENTER_CS, 0, 0xffff_ffff),
        Accept
    );
    // The upper half is ignored on write (Vol. 4 Table 2-2), not reserved; the cell keeps bits 31:0.
    assert_eq!(
        windows_msr_write_decision(IA32_SYSENTER_CS, 0, 1 << 32),
        Accept
    );
    assert_eq!(
        windows_msr_stored_value(IA32_SYSENTER_CS, u64::MAX),
        0xffff_ffff
    );
    assert_eq!(windows_msr_stored_value(IA32_LSTAR, u64::MAX), u64::MAX);
    assert!(windows_msr_cell_value_valid(IA32_SYSENTER_CS, 0xffff_ffff));
    assert!(!windows_msr_cell_value_valid(IA32_SYSENTER_CS, 1 << 32));
    assert_eq!(
        windows_msr_write_decision(IA32_STAR, 0, 0x0023_0010_0000_0000),
        Accept
    );
    assert_eq!(
        windows_msr_write_decision(IA32_STAR, 0, 1),
        GeneralProtection
    );
    assert_eq!(
        windows_msr_write_decision(IA32_LSTAR, 0, 0xffff_8000_0000_0000),
        Accept
    );
    assert_eq!(
        windows_msr_write_decision(IA32_LSTAR, 0, 0x0001_0000_0000_0000),
        GeneralProtection
    );
    assert_eq!(
        windows_msr_write_decision(IA32_PAT, 0, 0x0707_0707_0707_0707),
        Accept
    );
    assert_eq!(
        windows_msr_write_decision(IA32_PAT, 0, 0x0302),
        GeneralProtection
    );
    assert_eq!(
        windows_msr_write_decision(IA32_MISC_ENABLE, 0x1801, 0x1800),
        Accept
    );
    assert_eq!(
        windows_msr_write_decision(IA32_MISC_ENABLE, 0x1801, 0x1803),
        Unsupported
    );
    assert_eq!(
        windows_msr_write_decision(IA32_MISC_ENABLE, 0x1801, 0x0801),
        Unsupported
    );
    // Indexes outside the cell-owned list have no decision here.
    assert_eq!(windows_msr_write_decision(IA32_EFER, 0, 0), Unsupported);
}
