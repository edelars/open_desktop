//! S2-58: PAE, APIC and NX in the Windows CPUID profile (`CpuidProfile::OdX8664V2Windows`).
//!
//! Every expected value is written by hand from the Intel SDM rev. 093 (Vol. 1, Table 21-10 leaf 01H
//! EDX and Table 21-90 leaf 80000001H EDX; Vol. 3A, sections 5.1.4, 5.4.1, 5.4.2, 12.8.5 and Table 5-7), never from the
//! implementation under test. PAE and APIC are declared before the owner accepted S2-25 and S4-38L8d
//! (deviation of S2-58, confirmed by the owner 2026-11-21; `CPU_CONTRACT.md`). NX is declared, and
//! both EDX registers are the whole target table (leaf 1 EDX `0x0781A379`, 80000001H EDX
//! `0x20100800`), by the S7-32 decision of 2026-11-25 (NX was deferred until S4-10 was accepted,
//! and the owner accepted S4-10 on 2026-11-21).
//!
//! Groups:
//!
//! 1. The table: PAE (leaf 1 EDX[6]), APIC (leaf 1 EDX[9]) and NX (80000001H EDX[20]) are set, the
//!    registers are exactly the target table (S7-32 decision of 2026-11-25), every neighbouring bit
//!    that the Windows profile does not declare is clear.
//! 2. PAE rests on: the PAE walk (PDPTE registers, 4 KiB and 2 MiB pages, reserved bits) and the
//!    CR4.PAE policy of the Long64 CPL0 profile (IA-32e active: clearing PAE is #GP(0)).
//! 3. NX rests on: `WRMSR IA32_EFER` setting NXE is admitted with a member that declares NX (the
//!    Windows profile and the Linux target profile) and is #GP(0) with a member that does not (the
//!    Stage-4A subsets, NX = 0), and XD in a 2 MiB leaf follows NXE.
//! 4. APIC rests on: the initial APIC ID of leaf 1 EBX (APIC ID = vCPU index). The LAPIC device
//!    itself (xAPIC mode, base 0xFEE00000) is not an `od-isa` object (S2-27).

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetResult,
    MovCr3Long64V1, RunConfig, RunExit, run_with_cpl0_profile_set,
};
use od_isa::msr::MsrProfile;
use od_isa::paging::{
    AccessKind as WalkAccess, PaeParams, PageRefusal, PageSize, PagingParams, PdpteRefusal,
    load_pdptes, translate, translate_pae,
};
use od_isa::real_mode_exec::RealModeMemory;
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CliProfile, CpuActivity,
    GuestMemory, InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel,
    StiShadow, TranslationContextChange,
};
use std::cell::Cell;

static WINDOWS: CpuidProfile = CpuidProfile::OdX8664V2Windows;
static STAGE4A: CpuidProfile = CpuidProfile::X8664V1Stage4A;

// ---- group 1: the table ----

#[test]
fn pae_apic_and_nx_are_declared_at_their_sdm_positions() {
    let leaf1 = WINDOWS.query(1, 0);
    let ext1 = WINDOWS.query(0x8000_0001, 0);
    // CPUID.01H:EDX[6] PAE, EDX[9] APIC; CPUID.80000001H:EDX[20] Execute Disable Bit (NX).
    assert_ne!(leaf1.edx & (1 << 6), 0, "PAE");
    assert_ne!(leaf1.edx & (1 << 9), 0, "APIC");
    // S7-32 decision of 2026-11-25: NX is declared (S4-10 was accepted by the owner 2026-11-21).
    assert_ne!(ext1.edx & (1 << 20), 0, "NX");
    // Leaf 1 EDX is the target table (S7-32 decision of 2026-11-25): FPU 0x1, PSE 0x8, TSC 0x10,
    // MSR 0x20, PAE 0x40, CX8 0x100, APIC 0x200, PGE 0x2000, CMOV 0x8000, PAT 0x1_0000, MMX
    // 0x80_0000, FXSR 0x100_0000, SSE 0x200_0000, SSE2 0x400_0000. 80000001H EDX is SYSCALL 0x800,
    // NX 0x10_0000 and LM 0x2000_0000.
    assert_eq!(leaf1.edx & (0x40 | 0x200), 0x0000_0240);
    assert_eq!(leaf1.edx, 0x0781_A379);
    assert_eq!(ext1.edx, 0x2010_0800);
    // The rest of both leaves is the S2-57 table.
    assert_eq!(leaf1.eax, 0x0009_06C0);
    assert_eq!(leaf1.ecx, 0x0098_2201);
    assert_eq!((ext1.eax, ext1.ebx, ext1.ecx), (0, 0, 0x0000_0101));
}

/// S7-32 decision of 2026-11-25: NX [80000001H EDX 20] is declared by the Windows profile, for any
/// sub-leaf and any vCPU, as it is by the Linux target profile.
#[test]
fn nx_is_declared_in_the_windows_profile() {
    for subleaf in [0, 1] {
        assert_ne!(WINDOWS.query(0x8000_0001, subleaf).edx & (1 << 20), 0);
    }
    for (index, count) in [(0, 1), (0, 2), (1, 2)] {
        let edx = WINDOWS.query_for_vcpu(0x8000_0001, 0, index, count).edx;
        assert_ne!(edx & (1 << 20), 0, "vCPU {index} of {count}");
    }
    assert_ne!(
        CpuidProfile::OdX8664V1Neutral.query(0x8000_0001, 0).edx & (1 << 20),
        0
    );
}

/// The neighbouring bits that the Windows profile does not declare are clear: after the S7-32
/// decision of 2026-11-25 (the whole target table in both EDX registers) these are the leaf 1 EDX
/// bits outside `0x0781A379`, the x2APIC/TSC-deadline/hypervisor bits of leaf 1 ECX and Page1GB and
/// RDTSCP of 80000001H EDX.
#[test]
fn every_neighbouring_bit_that_the_windows_profile_does_not_declare_is_clear() {
    let leaf1 = WINDOWS.query(1, 0);
    let ext1 = WINDOWS.query(0x8000_0001, 0);
    for (bit, name) in [
        (1, "VME"),
        (2, "DE"),
        (7, "MCE"),
        (11, "SEP"),
        (12, "MTRR"),
        (14, "MCA"),
        (17, "PSE-36"),
        (19, "CLFSH"),
        (28, "HTT"),
    ] {
        assert_eq!(leaf1.edx & (1 << bit), 0, "leaf 1 EDX[{bit}] {name}");
    }
    for (bit, name) in [(21, "x2APIC"), (24, "TSC-deadline"), (31, "hypervisor")] {
        assert_eq!(leaf1.ecx & (1 << bit), 0, "leaf 1 ECX[{bit}] {name}");
    }
    for (bit, name) in [(26, "Page1GB"), (27, "RDTSCP")] {
        assert_eq!(ext1.edx & (1 << bit), 0, "80000001H EDX[{bit}] {name}");
    }
    // ADR-0013: no APIC-timer TSC-deadline, no x2APIC MSR range, no 1 GiB page.
    assert_eq!(WINDOWS.query(0x8000_0007, 0), Default::default());
}

#[test]
fn the_other_profiles_are_unchanged() {
    // Linux target (S6-14h): contract values. Stage 4A and its APIC variant keep their subsets.
    let linux = CpuidProfile::OdX8664V1Neutral;
    assert_eq!(linux.query(1, 0).edx, 0x0781_A379);
    assert_eq!(linux.query(0x8000_0001, 0).edx, 0x2010_0800);
    assert_eq!(STAGE4A.query(1, 0).edx, 0x0100_0011);
    assert_eq!(STAGE4A.query(0x8000_0001, 0).edx, 0);
    assert_eq!(
        CpuidProfile::X8664V1Stage4AApic.query(1, 0).edx,
        0x0100_0211
    );
    assert_eq!(
        CpuidProfile::X8664V1Stage4AApic.query(0x8000_0001, 0).edx,
        0
    );
    assert_eq!(CpuidProfile::DiagnosticV1.query(1, 0).edx, 0);
}

// ---- group 4: APIC rests on the initial APIC ID of leaf 1 EBX ----

#[test]
fn apic_id_of_leaf1_ebx_is_the_vcpu_index() {
    // Leaf 1 EBX: bits 31:24 initial APIC ID, 23:16 logical processor count, 15:8 CLFLUSH line
    // size in 8-byte units (0x100 is the table's value), 7:0 brand index.
    assert_eq!(WINDOWS.query(1, 0).ebx, 0x0000_0100);
    assert_eq!(WINDOWS.query_for_vcpu(1, 0, 0, 1).ebx, 0x0000_0100);
    assert_eq!(WINDOWS.query_for_vcpu(1, 0, 0, 2).ebx, 0x0002_0100);
    assert_eq!(WINDOWS.query_for_vcpu(1, 0, 1, 2).ebx, 0x0102_0100);
    // EDX does not change with the vCPU (the target table, S7-32 decision of 2026-11-25), HTT
    // (bit 28) stays clear.
    assert_eq!(WINDOWS.query_for_vcpu(1, 0, 1, 2).edx, 0x0781_A379);
}

// ---- executing guest code on the Windows member ----

const RIP: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
/// CR4 of the Long64 CPL0 profile: PAE (bit 5) only.
const CR4_BASE: u64 = 0x20;
const IF: u64 = 1 << 9;
const WRMSR: [u8; 2] = [0x0f, 0x30];
const MOV_CR4_RAX: [u8; 3] = [0x0f, 0x22, 0xe0];
const EFER_MSR: u64 = 0xc000_0080;

#[derive(Clone)]
struct Code(Vec<u8>);

impl GuestMemory for Code {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.checked_sub(RIP).unwrap_or(u64::MAX) as usize;
        self.0.get(offset).copied().ok_or(MemoryError::Unmapped {
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
    code: &[u8],
    set: &Cpl0ProfileSet<'_>,
    budget: u64,
) -> Cpl0ProfileSetResult {
    let mut memory = Code(code.to_vec());
    run_with_cpl0_profile_set(
        state,
        &mut memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip: None,
            context: AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
        },
        od_isa::INTEGER_FLAGS,
        set,
        None,
        || false,
    )
    .unwrap()
}

fn gp0() -> RunExit {
    RunExit::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

fn movabs_rax(value: u64) -> Vec<u8> {
    let mut bytes = vec![0x48, 0xb8];
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

/// `CPUID` executed in a set whose CPUID member is the Windows profile returns the declared
/// registers (S7-32 decision of 2026-11-25: leaf 1 EDX = `0x0781A379`, 80000001H EDX =
/// `0x20100800` = SYSCALL | NX | LM).
#[test]
fn cpuid_in_the_cpl0_set_with_the_windows_member_returns_the_declared_bits() {
    static CLI: CliProfile = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&CLI)
        .with_cpuid(&WINDOWS);
    // mov eax, leaf; cpuid
    for (leaf, edx, ecx) in [
        (1u32, 0x0781_A379u64, 0x0098_2201u64),
        (0x8000_0001, 0x2010_0800, 0x101),
    ] {
        let mut code = vec![0xb8];
        code.extend_from_slice(&leaf.to_le_bytes());
        code.extend_from_slice(&[0x0f, 0xa2]);
        let mut state = board_state();
        let result = run_set(&mut state, &code, &set, 2);
        assert_eq!(result.instructions_executed, 2, "leaf {leaf:#x}");
        assert_eq!(state.gpr[2], edx, "leaf {leaf:#x} EDX");
        assert_eq!(state.gpr[1], ecx, "leaf {leaf:#x} ECX");
    }
}

// ---- group 2: PAE ----

/// 0x4000 bytes of tables from physical 0: no data page is ever read, only entries.
struct Tables(Vec<u8>);

impl Tables {
    fn new() -> Self {
        Self(vec![0; 0x4000])
    }
    fn set(&mut self, at: usize, entry: u64) {
        self.0[at..at + 8].copy_from_slice(&entry.to_le_bytes());
    }
}

impl RealModeMemory for Tables {
    fn read_u8(&self, linear: u32) -> Option<u8> {
        self.0.get(linear as usize).copied()
    }
    fn can_write(&self, linear: u32) -> bool {
        (linear as usize) < self.0.len()
    }
    fn write_u8(&mut self, linear: u32, value: u8) -> bool {
        match self.0.get_mut(linear as usize) {
            Some(byte) => {
                *byte = value;
                true
            }
            None => false,
        }
    }
}

const P: u64 = 1;
const RW: u64 = 2;
const A: u64 = 0x20;
const PS: u64 = 0x80;
const XD: u64 = 1 << 63;

/// PAE paging (CR0.PG=1, CR4.PAE=1, EFER.LME=0; SDM Vol. 3A 5.4.2): PDPTE[0] -> PD at 0x2000, PDE[0]
/// -> PT at 0x3000, PTE[1] -> 4 KiB page 0x5000, PDE[1] = 2 MiB page at 0x40_0000.
fn pae_tables() -> Tables {
    let mut t = Tables::new();
    t.set(0x1000, 0x2000 | P);
    t.set(0x2000, 0x3000 | P | RW | A);
    t.set(0x2008, 0x40_0000 | P | RW | A | PS);
    t.set(0x3008, 0x5000 | P | RW | A);
    t
}

#[test]
fn pae_walk_maps_4k_and_2m_pages_through_the_pdpte_registers() {
    let t = pae_tables();
    let pdptes = load_pdptes(&t, 0x1000).unwrap();
    assert_eq!(pdptes, [0x2000 | P, 0, 0, 0]);
    let params = PaeParams {
        pdptes,
        wp: true,
        nxe: true,
    };
    let walk = translate_pae(&t, params, 0x1abc, WalkAccess::Read).unwrap();
    assert_eq!((walk.phys, walk.size), (0x5abc, PageSize::Size4K));
    assert!(walk.touches.is_empty(), "A is set at every level");
    let walk = translate_pae(&t, params, 0x20_0123, WalkAccess::Read).unwrap();
    assert_eq!((walk.phys, walk.size), (0x40_0123, PageSize::Size2M));
    // A linear address through a PDPTE with P = 0 is not present at level 3.
    assert_eq!(
        translate_pae(&t, params, 0x4000_0000, WalkAccess::Read),
        Err(PageRefusal::NotPresent { level: 3, entry: 0 })
    );
}

#[test]
fn pae_pdpte_with_a_reserved_bit_is_refused_when_present_and_ignores_cr3_low_bits() {
    let mut t = pae_tables();
    // CR3 bits 4:0 are ignored: 0x101f loads the table at 0x1000.
    assert_eq!(load_pdptes(&t, 0x101f).unwrap(), [0x2000 | P, 0, 0, 0]);
    // PDPTE bits 2:1 are reserved (SDM Vol. 3A Table 5-7) when P = 1.
    t.set(0x1008, 0x2000 | P | RW);
    assert_eq!(
        load_pdptes(&t, 0x1000),
        Err(PdpteRefusal::Reserved {
            index: 1,
            pdpte: 0x2000 | P | RW
        })
    );
}

#[test]
fn pae_entries_use_the_extended_format_xd_is_reserved_without_nxe() {
    let mut t = pae_tables();
    // Bit 63 of the PTE: XD with NXE=1 (fetch is refused, data is not), reserved with NXE=0.
    let pte = 0x5000 | P | RW | A | XD;
    t.set(0x3008, pte);
    let pdptes = load_pdptes(&t, 0x1000).unwrap();
    let nx = PaeParams {
        pdptes,
        wp: true,
        nxe: true,
    };
    assert_eq!(
        translate_pae(&t, nx, 0x1000, WalkAccess::Fetch),
        Err(PageRefusal::NoExecute { entry: pte })
    );
    assert_eq!(
        translate_pae(&t, nx, 0x1000, WalkAccess::Read)
            .unwrap()
            .phys,
        0x5000
    );
    let no_nx = PaeParams { nxe: false, ..nx };
    assert_eq!(
        translate_pae(&t, no_nx, 0x1000, WalkAccess::Read),
        Err(PageRefusal::ReservedBits {
            level: 1,
            entry: pte
        })
    );
}

/// CR4.PAE policy of the Long64 CPL0 profile with the Windows CPUID member (SDM Vol. 3A 12.8.5):
/// IA-32e is active, so a `MOV CR4` that clears PAE is #GP(0) with no state change, and one
/// that keeps PAE is accepted.
#[test]
fn cr4_pae_policy_holds_with_the_windows_cpuid_member() {
    static CLI: CliProfile = CliProfile::NonPviLong64V1;
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&CLI)
        .with_cpuid(&WINDOWS);
    for value in [0u64, 0x600, 0x10, 1 << 40] {
        let mut code = movabs_rax(value);
        code.extend_from_slice(&MOV_CR4_RAX);
        let mut state = board_state();
        let result = run_set(&mut state, &code, &set, 64);
        assert_eq!(result.exit, Cpl0ProfileSetExit::Run(gp0()), "{value:#x}");
        assert_eq!(result.instructions_executed, 1, "{value:#x}");
        assert_eq!(state.cr4, Some(CR4_BASE), "{value:#x}: CR4 unchanged");
        assert_eq!(state.gpr[0], value);
    }
    // PAE kept, OSFXSR (bit 9) set: accepted.
    let mut code = movabs_rax(CR4_BASE | 0x200);
    code.extend_from_slice(&MOV_CR4_RAX);
    let mut state = board_state();
    let result = run_set(&mut state, &code, &set, 2);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.cr4, Some(CR4_BASE | 0x200));
}

// ---- group 3: NX ----

/// `WRMSR IA32_EFER` with the cpu-owned CR3 sink and the given CPUID member.
fn wrmsr_efer(
    cell: &Cell<u64>,
    cpuid: &CpuidProfile,
    value: u64,
) -> (LongModeState, LongModeState, Cpl0ProfileSetResult) {
    let profile = MsrProfile::EferReadOnlyV1(cell.get());
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_msr(&profile)
        .with_msr_efer_write(cell)
        .with_cpu_owned_cr3(MovCr3Long64V1)
        .with_cpuid(cpuid);
    let mut state = board_state();
    state.cr4 = Some(CR4_BASE | (1 << 4) | (1 << 7));
    state.cr3 = Some(0x2000);
    state.gpr[1] = EFER_MSR;
    state.gpr[0] = value;
    state.gpr[2] = 0;
    let before = state.clone();
    let result = run_set(&mut state, &WRMSR, &set, 1);
    (state, before, result)
}

#[test]
fn wrmsr_efer_setting_nxe_is_admitted_with_a_member_that_declares_nx() {
    // SDM Vol. 3A 5.1.4: NXE may be set to 1 only if CPUID.80000001H:EDX[20] = 1. The Linux target
    // profile declares NX (80000001H EDX = 0x20100800), and so does the Windows profile (S7-32
    // decision of 2026-11-25).
    let neutral = CpuidProfile::OdX8664V1Neutral;
    for member in [&neutral, &WINDOWS] {
        assert_ne!(
            member.query(0x8000_0001, 0).edx & (1 << 20),
            0,
            "{member:?}"
        );
        for (old, new) in [(0x500u64, 0xd00u64), (0x501, 0xd01)] {
            let cell = Cell::new(old);
            let (state, before, result) = wrmsr_efer(&cell, member, new);
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::TranslationContextChange(TranslationContextChange::EferWrite {
                    old_efer: old,
                    new_efer: new
                }),
                "{member:?} {old:#x}->{new:#x}"
            );
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(cell.get(), new);
            let mut expected = before;
            expected.rip += 2;
            assert_eq!(state, expected, "only RIP changes");
        }
    }
}

#[test]
fn wrmsr_efer_setting_nxe_is_gp0_with_a_member_that_does_not_declare_nx() {
    // The Stage-4A subset members do not declare NX (80000001H EDX = 0), so NXE=1 is #GP(0), clear
    // or set alike for the other bits. The Windows member declares NX since the S7-32 decision of
    // 2026-11-25 and is covered by the admitted case above.
    let stage4a_apic = CpuidProfile::X8664V1Stage4AApic;
    for member in [&STAGE4A, &stage4a_apic] {
        assert_eq!(
            member.query(0x8000_0001, 0).edx & (1 << 20),
            0,
            "{member:?}"
        );
        for (old, new) in [(0x500u64, 0xd00u64), (0x501, 0xd01)] {
            let cell = Cell::new(old);
            let (state, before, result) = wrmsr_efer(&cell, member, new);
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::Run(gp0()),
                "{old:#x}->{new:#x}"
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
            assert_eq!(cell.get(), old);
        }
    }
    // A reserved EFER bit (bit 1) is #GP(0) with the Windows member, which declares NX: the fault
    // comes from the reserved bit alone, not from the NXE in the same value.
    let cell = Cell::new(0x500);
    let (state, before, result) = wrmsr_efer(&cell, &WINDOWS, 0xd02);
    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(gp0()));
    assert_eq!(state, before);
    assert_eq!(cell.get(), 0x500);
}

/// XD in a 2 MiB leaf of the 4-level walk (SDM Vol. 3A 5.6, 4.6.1): with NXE=1 a fetch is refused
/// and a data read is not; with NXE=0 bit 63 is reserved.
#[test]
fn xd_in_a_2m_leaf_follows_nxe() {
    let mut t = Tables::new();
    t.set(0x1000, 0x2000 | P | RW | A); // PML4E[0]
    t.set(0x2000, 0x3000 | P | RW | A); // PDPTE[0]
    let pde = 0x40_0000 | P | RW | A | PS | XD;
    t.set(0x3008, pde); // PDE[1]: 2 MiB page at 0x40_0000, XD
    let nx = PagingParams {
        cr3: 0x1000,
        wp: true,
        nxe: true,
    };
    let walk = translate(&t, nx, 0x20_0123, WalkAccess::Read).unwrap();
    assert_eq!((walk.phys, walk.size), (0x40_0123, PageSize::Size2M));
    assert_eq!(
        translate(&t, nx, 0x20_0123, WalkAccess::Fetch),
        Err(PageRefusal::NoExecute { entry: pde })
    );
    let no_nx = PagingParams { nxe: false, ..nx };
    assert_eq!(
        translate(&t, no_nx, 0x20_0123, WalkAccess::Read),
        Err(PageRefusal::ReservedBits {
            level: 2,
            entry: pde
        })
    );
    // Without XD the same leaf is executable.
    t.set(0x3008, pde & !XD);
    let walk = translate(&t, nx, 0x20_0123, WalkAccess::Fetch).unwrap();
    assert_eq!(walk.phys, 0x40_0123);
}
