//! S2-57: the Windows CPUID profile `od-x86-64-v2-windows-v0.1` (`CpuidProfile::OdX8664V2Windows`).
//!
//! Every expected value is written by hand from the Intel SDM (Vol. 1, "Processor identification and
//! feature determination": leaf 01H ECX/EDX tables, leaf 80000001H, brand string; Vol. 2 instruction
//! pages) or from a published check value (CRC-32C of "123456789" is 0xE3069283, RFC 3720), never
//! from the implementation under test.
//!
//! Slice C of S2-57 (CX8 and CMOV declared; FPU, TSC, MSR, SYSCALL and LM kept clear until a separate
//! owner decision, 2026-11-21, see CPU_CONTRACT.md) adds CX8 (Vol. 1 Table 21-10, EDX[8]) and CMOV
//! (EDX[15]) to the declared set below.
//!
//! Three groups:
//!
//! 1. The table: each declared bit is set and each bit of an owner task that is not accepted (or has
//!    no task) is clear; registers are compared whole.
//! 2. Positive: for each declared bit, one representative form of its owner task executes in the
//!    interpreter and gives the SDM result. The interpreter does not gate a form on its CPUID bit
//!    (the SDM's #UD for a clear bit is an open decision, see CPU_CONTRACT.md S2-57), so the
//!    negative side is the contract side: a clear bit means that the profile does not declare the
//!    form, which group 1 checks.
//! 3. `CPUID` executed in Long64 with the profile returns the table, byte for byte.

use od_isa::cpuid::{CpuidProfile, CpuidResult};
use od_isa::execute::{self, RdtscControls, RunConfig, RunExit, StepOutcome};
use od_isa::state::{SseState, X87State, X87Tag};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, GuestMemory, LongModeState, MemoryError,
    PrivilegeLevel, Register,
};

const WINDOWS: CpuidProfile = CpuidProfile::OdX8664V2Windows;

// ---- group 1: the table ----

#[derive(Clone, Copy)]
enum Reg {
    Edx1,
    Ecx1,
    Ecx80000001,
    Edx80000001,
}

fn register(reg: Reg) -> u32 {
    match reg {
        Reg::Edx1 => WINDOWS.query(1, 0).edx,
        Reg::Ecx1 => WINDOWS.query(1, 0).ecx,
        Reg::Ecx80000001 => WINDOWS.query(0x8000_0001, 0).ecx,
        Reg::Edx80000001 => WINDOWS.query(0x8000_0001, 0).edx,
    }
}

/// (register, bit, name, owner task): declared because the owner task is implemented and merged.
const DECLARED: [(Reg, u32, &str, &str); 12] = [
    // S2-57 slice C (S2-16 and S2-21 accepted, confirmed by the owner 2026-11-21). TSC [4] is NOT
    // here: the owner holds it until a separate decision. Bit numbers: Intel SDM rev. 093, Vol. 1,
    // Table 21-10 (pp. 21-12 and 21-13).
    (Reg::Edx1, 8, "CX8 (CMPXCHG8B)", "S2-21i"),
    (Reg::Edx1, 15, "CMOV", "S2-16c"),
    // S2-58 (deviation: declared before S2-25 and S4-38L8d are accepted, confirmed by the owner
    // 2026-11-21). NX [80000001H EDX 20] is NOT here: the owner deferred it until S4-10 is accepted.
    (Reg::Edx1, 6, "PAE", "S2-58"),
    (Reg::Edx1, 9, "APIC", "S2-58"),
    (Reg::Ecx1, 0, "SSE3", "S2-54"),
    (Reg::Ecx1, 9, "SSSE3", "S2-54"),
    (Reg::Ecx1, 13, "CX16 (CMPXCHG16B)", "S2-51"),
    (Reg::Ecx1, 19, "SSE4_1", "S2-55"),
    (Reg::Ecx1, 20, "SSE4_2", "S2-56"),
    (Reg::Ecx1, 23, "POPCNT", "S2-56"),
    (Reg::Ecx80000001, 0, "LAHF_SAHF_64", "S2-52"),
    (Reg::Ecx80000001, 8, "PREFETCHW", "S2-53"),
];

/// (register, bit, name, why it is clear). The profile is built from the owner-task table of S2-57,
/// not from the Linux table: a bit whose owner is not accepted, or has no task, stays clear.
const UNDECLARED: [(Reg, u32, &str, &str); 30] = [
    (
        Reg::Edx1,
        0,
        "FPU",
        "x87 is not complete for CPL0: FSIN, FCOS, FSINCOS, FPTAN, FPATAN, F2XM1, FYL2X, FYL2XP1, FSCALE, FXTRACT, FBLD, FBSTP, FNSTENV, FNSAVE, FRSTOR exist only in the bounded profile of S2-32a/S2-32b/S2-32c and are not connected to the CPL0 set (x87 state is None there), FLDENV and the FWAIT spellings FSTENV/FSAVE are not implemented in the set (S2-30, S2-31, S2-32 open)",
    ),
    (Reg::Edx1, 3, "PSE", "no task"),
    (
        Reg::Edx1,
        4,
        "TSC",
        "owner decision 2026-11-21: not declared until a separate owner decision",
    ),
    (
        Reg::Edx1,
        5,
        "MSR",
        "only single MSRs (EFER, FS/GS base, STAR, LSTAR, FMASK, SYSENTER) are modelled, not the Windows set (S2-25 open)",
    ),
    (Reg::Edx1, 13, "PGE", "no task"),
    (Reg::Edx1, 16, "PAT", "no task"),
    (Reg::Edx1, 19, "CLFSH", "no task"),
    (Reg::Edx1, 23, "MMX", "S2-33 minimal only"),
    (Reg::Edx1, 24, "FXSR", "S2-39 not complete"),
    (Reg::Edx1, 25, "SSE", "S2-34..S2-39 not complete"),
    (Reg::Edx1, 26, "SSE2", "S2-35..S2-39 not complete"),
    (Reg::Edx1, 28, "HTT", "ADR-0013, S6-18 (topology)"),
    (Reg::Ecx1, 1, "PCLMULQDQ", "no task"),
    (Reg::Ecx1, 3, "MONITOR", "separate bit, not in S2-54"),
    (Reg::Ecx1, 5, "VMX", "ADR-0013, decision 3"),
    (Reg::Ecx1, 12, "FMA", "no task"),
    (Reg::Ecx1, 21, "x2APIC", "no task"),
    (Reg::Ecx1, 22, "MOVBE", "no task"),
    (Reg::Ecx1, 24, "TSC-deadline", "no task"),
    (Reg::Ecx1, 25, "AES", "no task"),
    (Reg::Ecx1, 26, "XSAVE", "ADR-0013, decision 4"),
    (Reg::Ecx1, 27, "OSXSAVE", "ADR-0013, decision 4"),
    (Reg::Ecx1, 28, "AVX", "ADR-0013, decision 4"),
    (Reg::Ecx1, 31, "hypervisor", "ADR-0013, decision 3"),
    (Reg::Ecx80000001, 5, "LZCNT", "no task"),
    (
        Reg::Edx80000001,
        11,
        "SYSCALL",
        "SYSCALL/SYSRET only in the Linux board sets, semantics from memory of the SDM, no compatibility mode (S2-27, S2-24 open)",
    ),
    (
        Reg::Edx80000001,
        20,
        "NX",
        "S4-10 not accepted (owner decision 2026-11-21: NX deferred)",
    ),
    (Reg::Edx80000001, 26, "1 GiB pages", "no task"),
    (Reg::Edx80000001, 27, "RDTSCP", "no task"),
    (
        Reg::Edx80000001,
        29,
        "LM",
        "EFER.LME/LMA changes and compatibility mode are not modelled (S2-24, S2-25 open)",
    ),
];

#[test]
fn every_declared_bit_is_set() {
    for (reg, bit, name, owner) in DECLARED {
        assert_ne!(register(reg) & (1 << bit), 0, "{name} ({owner}), bit {bit}");
    }
}

#[test]
fn every_bit_of_a_task_that_is_not_accepted_or_has_no_task_is_clear() {
    for (reg, bit, name, why) in UNDECLARED {
        assert_eq!(register(reg) & (1 << bit), 0, "{name}: {why}");
    }
}

#[test]
fn feature_registers_are_exactly_the_declared_bits() {
    // ECX of leaf 1: bits 0, 9, 13, 19, 20, 23. 80000001H ECX: bits 0 and 8.
    assert_eq!(
        register(Reg::Ecx1),
        0x0000_0001 | 0x0000_0200 | 0x0000_2000 | 0x0008_0000 | 0x0010_0000 | 0x0080_0000
    );
    assert_eq!(register(Reg::Ecx1), 0x0098_2201);
    assert_eq!(register(Reg::Ecx80000001), 0x0000_0101);
    // EDX of leaf 1: CX8 [8], CMOV [15] (S2-57 slice C), PAE [6] and APIC [9] (S2-58); TSC [4] is
    // held by the owner. 80000001H EDX: zero (NX [20] is deferred until S4-10, owner decision
    // 2026-11-21). No bit is left over that the lists above do not name.
    assert_eq!(register(Reg::Edx1), 0x40 | 0x100 | 0x200 | 0x8000);
    assert_eq!(register(Reg::Edx1), 0x8340);
    assert_eq!(register(Reg::Edx80000001), 0);
    for reg in [Reg::Edx1, Reg::Ecx1, Reg::Ecx80000001, Reg::Edx80000001] {
        let mut declared = 0u32;
        for (r, bit, _, _) in DECLARED {
            if std::mem::discriminant(&r) == std::mem::discriminant(&reg) {
                declared |= 1 << bit;
            }
        }
        assert_eq!(declared, register(reg));
    }
}

#[test]
fn the_linux_table_is_not_inherited_and_the_linux_profile_is_unchanged() {
    let linux = CpuidProfile::OdX8664V1Neutral;
    // The contract's Linux values (S6-14h).
    assert_eq!(linux.query(0, 0).ebx, u32::from_le_bytes(*b"Open"));
    assert_eq!(linux.query(1, 0).eax, 0x0000_0600);
    assert_eq!(linux.query(1, 0).ecx, 0);
    assert_eq!(linux.query(1, 0).edx, 0x0781_A379);
    assert_eq!(linux.query(0x8000_0001, 0).ecx, 0);
    assert_eq!(linux.query(0x8000_0001, 0).edx, 0x2010_0800);
    assert_eq!(linux.query(0x8000_0002, 0), CpuidResult::default());
    // The Windows profile has its own identity and takes from the Linux EDX bits only those it
    // declares: PAE, CX8, APIC and CMOV in leaf 1; NX in 80000001H is not taken (the other Linux
    // bits stay clear, TSC included).
    assert_ne!(WINDOWS.query(0, 0), linux.query(0, 0));
    assert_eq!(WINDOWS.query(1, 0).edx & linux.query(1, 0).edx, 0x8340);
    assert_eq!(
        WINDOWS.query(0x8000_0001, 0).edx & linux.query(0x8000_0001, 0).edx,
        0
    );
}

#[test]
fn the_identity_is_the_celeron_n5105_of_adr_0013() {
    let leaf0 = WINDOWS.query(0, 0);
    let mut vendor = Vec::new();
    for word in [leaf0.ebx, leaf0.edx, leaf0.ecx] {
        vendor.extend_from_slice(&word.to_le_bytes());
    }
    assert_eq!(vendor, b"GenuineIntel");
    assert_eq!(leaf0.eax, 1);
    // Signature: stepping 0, model 0xC, family 6, type 0, extended model 9, extended family 0.
    let eax = WINDOWS.query(1, 0).eax;
    assert_eq!(eax, 0x0009_06C0);
    assert_eq!(eax & 0xF, 0, "stepping");
    assert_eq!((eax >> 4) & 0xF, 0xC, "model");
    assert_eq!((eax >> 8) & 0xF, 6, "family");
    assert_eq!((eax >> 12) & 0x3, 0, "processor type");
    assert_eq!((eax >> 16) & 0xF, 9, "extended model");
    assert_eq!((eax >> 20) & 0xFF, 0, "extended family");
    assert_eq!(WINDOWS.query(0x8000_0000, 0).eax, 0x8000_0008);
}

// ---- group 3: CPUID executed in Long64 ----

const CODE: u64 = 0x2000;
const DATA: u64 = 0x4000;
const RAM: usize = 0x8000;
const ALL_FLAGS: u64 = 0x2 | 0xcd5;

struct Ram {
    bytes: Vec<u8>,
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut bytes = vec![0; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self { bytes }
    }

    fn range(&self, address: u64, length: usize, access: AccessKind) -> Result<usize, MemoryError> {
        usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM))
            .ok_or(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let start = self.range(address, 1, AccessKind::Fetch)?;
        Ok(self.bytes[start])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let start = self.range(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let start = self.range(address, data.len(), AccessKind::Write)?;
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// `mov eax, leaf; xor ecx, ecx; cpuid`.
fn cpuid_leaf(leaf: u32) -> [u32; 4] {
    let mut code = vec![0xb8];
    code.extend_from_slice(&leaf.to_le_bytes());
    code.extend_from_slice(&[0x31, 0xc9, 0x0f, 0xa2]);
    let mut state = LongModeState {
        rip: CODE,
        rflags: 0x2,
        ..LongModeState::default()
    };
    let mut ram = Ram::with_code(&code);
    let result = execute::run_with_cpuid_profile(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 3,
            stop_rip: None,
            context: ring0(),
        },
        &WINDOWS,
        || false,
    );
    assert_eq!(
        result.instructions_executed, 3,
        "{leaf:#x}: {:?}",
        result.exit
    );
    assert_eq!(
        result.exit,
        RunExit::InstructionBudgetExhausted,
        "{leaf:#x}"
    );
    [
        state.gpr[Register::Rax as usize] as u32,
        state.gpr[Register::Rbx as usize] as u32,
        state.gpr[Register::Rcx as usize] as u32,
        state.gpr[Register::Rdx as usize] as u32,
    ]
}

#[test]
fn cpuid_in_long64_returns_the_table_byte_for_byte() {
    // Order of the result: EAX, EBX, ECX, EDX.
    assert_eq!(cpuid_leaf(0), [1, 0x756e_6547, 0x6c65_746e, 0x4965_6e69]);
    // EDX: PAE | CX8 | APIC | CMOV (S2-57 slice C, S2-58); TSC is clear.
    assert_eq!(cpuid_leaf(1), [0x0009_06c0, 0x100, 0x0098_2201, 0x8340]);
    assert_eq!(cpuid_leaf(0x8000_0000), [0x8000_0008, 0, 0, 0]);
    // EDX: zero (NX deferred until S4-10).
    assert_eq!(cpuid_leaf(0x8000_0001), [0, 0, 0x101, 0]);
    // "Inte" "l(R)" " Cel" "eron" | "(R) " "N510" "5 @ " "2.00" | "GHz" NUL and zeros.
    assert_eq!(
        cpuid_leaf(0x8000_0002),
        [0x6574_6e49, 0x2952_286c, 0x6c65_4320, 0x6e6f_7265]
    );
    assert_eq!(
        cpuid_leaf(0x8000_0003),
        [0x2029_5228, 0x3031_354e, 0x2040_2035, 0x3030_2e32]
    );
    assert_eq!(cpuid_leaf(0x8000_0004), [0x007a_4847, 0, 0, 0]);
    assert_eq!(cpuid_leaf(0x8000_0008), [0x3024, 0, 0, 0]);
    // Unlisted leaves: four zeros (no leaf 7, no hypervisor leaves, no invariant TSC).
    for leaf in [2, 4, 7, 0xb, 0xd, 0x4000_0000, 0x8000_0007] {
        assert_eq!(cpuid_leaf(leaf), [0, 0, 0, 0], "{leaf:#x}");
    }
}

// ---- group 2: one representative form per declared bit ----

fn cpu() -> LongModeState {
    let mut cpu = LongModeState {
        rip: CODE,
        rflags: ALL_FLAGS,
        x87: Some(X87State::fixture_seed()),
        sse: Some(SseState {
            xmm: core::array::from_fn(|index| {
                core::array::from_fn(|lane| (index as u8).wrapping_mul(0x11) ^ (lane as u8) ^ 0x5a)
            }),
            mxcsr: 0x1f80,
        }),
        ..LongModeState::default()
    };
    for (index, slot) in cpu.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    cpu
}

fn step(code: &[u8], cpu: &mut LongModeState, ram: &mut Ram) {
    ram.bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
    let before_rip = cpu.rip;
    let outcome = execute::step(cpu, ram, ring0());
    assert_eq!(outcome, Ok(StepOutcome::Advanced), "{code:02x?}");
    assert_eq!(cpu.rip, before_rip + code.len() as u64, "{code:02x?}");
}

fn xmm(cpu: &LongModeState, index: usize) -> [u8; 16] {
    cpu.sse.as_ref().unwrap().xmm[index]
}

fn set_xmm(cpu: &mut LongModeState, index: usize, value: [u8; 16]) {
    cpu.sse.as_mut().unwrap().xmm[index] = value;
}

fn dwords(values: [u32; 4]) -> [u8; 16] {
    let mut out = [0; 16];
    for (lane, value) in values.iter().enumerate() {
        out[4 * lane..4 * lane + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

fn qwords(values: [u64; 2]) -> [u8; 16] {
    let mut out = [0; 16];
    out[..8].copy_from_slice(&values[0].to_le_bytes());
    out[8..].copy_from_slice(&values[1].to_le_bytes());
    out
}

/// CX16: `REX.W 0F C7 /1` m128 at [rdi]. Equal: ZF = 1 and RCX:RBX is stored; not equal: ZF = 0 and
/// the operand is loaded into RDX:RAX (Intel SDM Vol. 2A, CMPXCHG8B/CMPXCHG16B).
#[test]
fn cx16_cmpxchg16b_both_outcomes() {
    let code = [0x48, 0x0f, 0xc7, 0x0f];
    let (lo, hi) = (0x1111_2222_3333_4444u64, 0x5555_6666_7777_8888u64);
    let (new_lo, new_hi) = (0xaaaa_bbbb_cccc_ddddu64, 0x0123_4567_89ab_cdefu64);
    for equal in [true, false] {
        let mut cpu = cpu();
        let mut ram = Ram::with_code(&code);
        ram.bytes[DATA as usize..DATA as usize + 8].copy_from_slice(&lo.to_le_bytes());
        ram.bytes[DATA as usize + 8..DATA as usize + 16].copy_from_slice(&hi.to_le_bytes());
        cpu.gpr[Register::Rdi as usize] = DATA;
        cpu.gpr[Register::Rax as usize] = if equal { lo } else { lo ^ 1 };
        cpu.gpr[Register::Rdx as usize] = hi;
        cpu.gpr[Register::Rbx as usize] = new_lo;
        cpu.gpr[Register::Rcx as usize] = new_hi;
        step(&code, &mut cpu, &mut ram);
        let stored_lo = u64::from_le_bytes(
            ram.bytes[DATA as usize..DATA as usize + 8]
                .try_into()
                .unwrap(),
        );
        let stored_hi = u64::from_le_bytes(
            ram.bytes[DATA as usize + 8..DATA as usize + 16]
                .try_into()
                .unwrap(),
        );
        if equal {
            assert_eq!((stored_lo, stored_hi), (new_lo, new_hi));
            assert_ne!(cpu.rflags & 0x40, 0, "ZF = 1");
            assert_eq!(cpu.gpr[Register::Rax as usize], lo);
            assert_eq!(cpu.gpr[Register::Rdx as usize], hi);
        } else {
            assert_eq!((stored_lo, stored_hi), (lo, hi));
            assert_eq!(cpu.rflags & 0x40, 0, "ZF = 0");
            assert_eq!(cpu.gpr[Register::Rax as usize], lo);
            assert_eq!(cpu.gpr[Register::Rdx as usize], hi);
        }
        // Only ZF changes among the flags (SDM: the other flags are unaffected).
        assert_eq!(cpu.rflags & !0x40, ALL_FLAGS & !0x40);
    }
}

/// LAHF_SAHF_64: `9F` LAHF loads AH with SF:ZF:0:AF:0:PF:1:CF; `9E` SAHF stores AH into those flags.
#[test]
fn lahf_sahf_in_long64() {
    let mut cpu = cpu();
    let mut ram = Ram::with_code(&[]);
    // CF, PF, AF, ZF, SF set (0xd5 | 0x2 = 0xd7 is the AH image); DF and OF set and untouched.
    cpu.rflags = 0x2 | 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x400 | 0x800;
    let rax = cpu.gpr[Register::Rax as usize];
    step(&[0x9f], &mut cpu, &mut ram);
    assert_eq!(cpu.gpr[Register::Rax as usize], (rax & !0xff00) | 0xd700);
    assert_eq!(cpu.rflags, 0x2 | 0xd5 | 0xc00, "LAHF changes no flag");
    cpu.rip = CODE;
    // SAHF with AH = 0x00: CF, PF, AF, ZF, SF cleared; bit 1 stays 1; DF and OF unchanged.
    cpu.gpr[Register::Rax as usize] = 0x1234_5678_9abc_00de;
    step(&[0x9e], &mut cpu, &mut ram);
    assert_eq!(cpu.rflags, 0x2 | 0x400 | 0x800);
    assert_eq!(
        cpu.gpr[Register::Rax as usize],
        0x1234_5678_9abc_00de,
        "SAHF writes no register"
    );
}

/// PREFETCHW: `0F 0D /1` m8 is a hint: no fault for any address, no register or flag effect.
#[test]
fn prefetchw_is_a_hint_without_fault() {
    for address in [DATA, 0, 0x1234_5678_1234_5678, 0xffff_ffff_ffff_ffff] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rdi as usize] = address;
        let before = cpu.clone();
        let mut ram = Ram::with_code(&[]);
        step(&[0x0f, 0x0d, 0x0f], &mut cpu, &mut ram);
        let mut expected = before;
        expected.rip += 3;
        assert_eq!(cpu, expected, "{address:#x}");
    }
}

/// POPCNT: `F3 REX.W 0F B8 /r` counts the set bits; OF, SF, AF, CF and PF are cleared and ZF is 1
/// only for a zero source (Intel SDM Vol. 2B, POPCNT).
#[test]
fn popcnt_counts_bits_and_sets_only_zf() {
    for (source, count, flags) in [
        (0xf0f0_0000_0000_0001u64, 9u64, 0x2 | 0x400),
        (u64::MAX, 64, 0x2 | 0x400),
        (0, 0, 0x2 | 0x400 | 0x40),
    ] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rcx as usize] = source;
        let mut ram = Ram::with_code(&[]);
        step(&[0xf3, 0x48, 0x0f, 0xb8, 0xc1], &mut cpu, &mut ram);
        assert_eq!(cpu.gpr[Register::Rax as usize], count, "{source:#x}");
        assert_eq!(cpu.rflags, flags, "{source:#x}");
    }
}

/// SSE3: `F2 0F 12 /r` MOVDDUP xmm0, xmm1 copies the low quadword of the source into both halves.
#[test]
fn sse3_movddup() {
    let mut cpu = cpu();
    set_xmm(
        &mut cpu,
        1,
        qwords([0x1122_3344_5566_7788, 0xdead_beef_dead_beef]),
    );
    let mut ram = Ram::with_code(&[]);
    step(&[0xf2, 0x0f, 0x12, 0xc1], &mut cpu, &mut ram);
    assert_eq!(xmm(&cpu, 0), qwords([0x1122_3344_5566_7788; 2]));
}

/// SSSE3: `66 0F 38 00 /r` PSHUFB: destination byte i = 0 if source byte i has bit 7 set, else
/// destination byte (source byte i & 15) (Intel SDM Vol. 2B, PSHUFB).
#[test]
fn ssse3_pshufb() {
    let mut cpu = cpu();
    let destination: [u8; 16] = core::array::from_fn(|lane| 0xa0 + lane as u8);
    let mut selector: [u8; 16] = core::array::from_fn(|lane| 15 - lane as u8);
    selector[3] = 0x80;
    selector[9] = 0x8f;
    set_xmm(&mut cpu, 0, destination);
    set_xmm(&mut cpu, 1, selector);
    let mut ram = Ram::with_code(&[]);
    step(&[0x66, 0x0f, 0x38, 0x00, 0xc1], &mut cpu, &mut ram);
    let mut expected = [0u8; 16];
    for lane in 0..16 {
        expected[lane] = if selector[lane] & 0x80 != 0 {
            0
        } else {
            destination[(selector[lane] & 15) as usize]
        };
    }
    assert_eq!(xmm(&cpu, 0), expected);
    assert_eq!(expected[3], 0);
    assert_eq!(expected[0], 0xaf);
}

/// SSE4_1: `66 0F 38 40 /r` PMULLD keeps the low 32 bits of each signed dword product.
#[test]
fn sse4_1_pmulld() {
    let mut cpu = cpu();
    set_xmm(&mut cpu, 0, dwords([3, 0x1_0001, 0xffff_ffff, 0x8000_0000]));
    set_xmm(&mut cpu, 1, dwords([7, 0x1_0001, 5, 2]));
    let mut ram = Ram::with_code(&[]);
    step(&[0x66, 0x0f, 0x38, 0x40, 0xc1], &mut cpu, &mut ram);
    // 3*7 = 21; 0x10001^2 = 0x1_0002_0001 -> 0x0002_0001; (-1)*5 = -5; 0x8000_0000*2 -> 0.
    assert_eq!(xmm(&cpu, 0), dwords([21, 0x0002_0001, 0xffff_fffb, 0]));
}

/// SSE4_2: `66 0F 38 37 /r` PCMPGTQ compares signed qwords; `F2 0F 38 F0 /r` CRC32 r32, r/m8
/// accumulates CRC-32C (reflected polynomial 0x82F63B78).
#[test]
fn sse4_2_pcmpgtq_and_crc32() {
    let mut cpu = cpu();
    set_xmm(&mut cpu, 0, qwords([5, 0x8000_0000_0000_0000]));
    set_xmm(&mut cpu, 1, qwords([4, 1]));
    let mut ram = Ram::with_code(&[]);
    step(&[0x66, 0x0f, 0x38, 0x37, 0xc1], &mut cpu, &mut ram);
    // 5 > 4 is true; the most negative value > 1 is false (signed compare).
    assert_eq!(xmm(&cpu, 0), qwords([u64::MAX, 0]));

    // CRC-32C check value: init 0xFFFFFFFF, bytes "123456789", final XOR 0xFFFFFFFF = 0xE3069283.
    let mut cpu = cpu_for_crc();
    for byte in b"123456789" {
        cpu.gpr[Register::Rcx as usize] = u64::from(*byte);
        cpu.rip = CODE;
        step(&[0xf2, 0x0f, 0x38, 0xf0, 0xc1], &mut cpu, &mut ram);
    }
    assert_eq!(
        cpu.gpr[Register::Rax as usize] as u32 ^ 0xffff_ffff,
        0xe306_9283
    );
}

fn cpu_for_crc() -> LongModeState {
    let mut cpu = cpu();
    cpu.gpr[Register::Rax as usize] = 0xffff_ffff;
    cpu
}

// ---- slice C: CX8 and CMOV declared, TSC form (RDTSC) tested but its bit not declared ----

fn step_outcome(
    code: &[u8],
    cpu: &mut LongModeState,
    ram: &mut Ram,
) -> Result<StepOutcome, MemoryError> {
    ram.bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
    execute::step(cpu, ram, ring0())
}

fn ud() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    })
}

/// CX8: `0F C7 /1` m64 (CMPXCHG8B) compares EDX:EAX with the operand. Equal: ZF = 1 and ECX:EBX is
/// stored. Not equal: ZF = 0, the operand is loaded into EDX:EAX and memory keeps its value (Intel SDM
/// Vol. 2A, CMPXCHG8B, pp. 3-196 to 3-198). Only ZF changes among the flags. The register form is #UD.
#[test]
fn cx8_cmpxchg8b_both_outcomes_and_the_register_form_is_ud() {
    let code = [0x0f, 0xc7, 0x0f];
    let operand = 0x1111_2222_3333_4444u64;
    let replacement = 0xaaaa_bbbb_cccc_ddddu64;
    for equal in [true, false] {
        let mut cpu = cpu();
        let mut ram = Ram::with_code(&code);
        ram.bytes[DATA as usize..DATA as usize + 8].copy_from_slice(&operand.to_le_bytes());
        cpu.gpr[Register::Rdi as usize] = DATA;
        let expected = if equal { operand } else { operand ^ 1 };
        cpu.gpr[Register::Rax as usize] = 0xdead_beef_0000_0000 | (expected & 0xffff_ffff);
        cpu.gpr[Register::Rdx as usize] = 0xdead_beef_0000_0000 | (expected >> 32);
        cpu.gpr[Register::Rbx as usize] = 0x5555_5555_0000_0000 | (replacement & 0xffff_ffff);
        cpu.gpr[Register::Rcx as usize] = 0x5555_5555_0000_0000 | (replacement >> 32);
        step(&code, &mut cpu, &mut ram);
        let stored = u64::from_le_bytes(
            ram.bytes[DATA as usize..DATA as usize + 8]
                .try_into()
                .unwrap(),
        );
        if equal {
            assert_eq!(stored, replacement);
            assert_ne!(cpu.rflags & 0x40, 0, "ZF = 1");
            // EDX:EAX is not modified when the operands are equal.
            assert_eq!(cpu.gpr[Register::Rax as usize] as u32, expected as u32);
            assert_eq!(
                cpu.gpr[Register::Rdx as usize] as u32,
                (expected >> 32) as u32
            );
        } else {
            assert_eq!(stored, operand, "memory keeps its value");
            assert_eq!(cpu.rflags & 0x40, 0, "ZF = 0");
            // 32-bit writes in 64-bit mode zero the upper halves of RAX and RDX.
            assert_eq!(cpu.gpr[Register::Rax as usize], operand & 0xffff_ffff);
            assert_eq!(cpu.gpr[Register::Rdx as usize], operand >> 32);
        }
        assert_eq!(cpu.rflags & !0x40, ALL_FLAGS & !0x40, "only ZF changes");
    }
    // Register form (mod = 3): #UD, nothing changes.
    let reg_code = [0x0f, 0xc7, 0xcb];
    let mut cpu = cpu();
    let before = cpu.clone();
    let mut ram = Ram::with_code(&reg_code);
    assert_eq!(step_outcome(&reg_code, &mut cpu, &mut ram), Ok(ud()));
    assert_eq!(cpu, before);
}

/// CMOV: `0F 44 /r` CMOVE moves the source when ZF = 1 and leaves the destination when ZF = 0; no flag
/// changes (Intel SDM Vol. 2A, CMOVcc, pp. 3-157 to 3-160). In 64-bit mode the 32-bit form clears the
/// upper half of the destination even when the condition is false.
#[test]
fn cmov_moves_only_when_the_condition_holds() {
    let code = [0x48, 0x0f, 0x44, 0xc1];
    for zf in [true, false] {
        let mut cpu = cpu_with_zf(zf);
        let before = cpu.clone();
        let mut ram = Ram::with_code(&code);
        step(&code, &mut cpu, &mut ram);
        let (rax, rcx) = (
            before.gpr[Register::Rax as usize],
            before.gpr[Register::Rcx as usize],
        );
        assert_eq!(cpu.gpr[Register::Rax as usize], if zf { rcx } else { rax });
        assert_eq!(cpu.gpr[Register::Rcx as usize], rcx);
        assert_eq!(cpu.rflags, before.rflags, "CMOVcc changes no flag");
    }
    // 32-bit form with a false condition: EAX is kept, the upper half of RAX is cleared.
    let code = [0x0f, 0x44, 0xc1];
    let mut cpu = cpu();
    cpu.rflags = ALL_FLAGS & !0x40;
    cpu.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    cpu.gpr[Register::Rcx as usize] = 0x0fed_cba9_8765_4321;
    let mut ram = Ram::with_code(&code);
    step(&code, &mut cpu, &mut ram);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x9abc_def0);
    // 32-bit form with a true condition: the low half of RCX, zero-extended.
    cpu.rflags = ALL_FLAGS | 0x40;
    cpu.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    cpu.rip = CODE;
    step(&code, &mut cpu, &mut ram);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x8765_4321);
    // The memory source is read even when the condition is false (Vol. 2A, CMOVcc).
    let code = [0x48, 0x0f, 0x44, 0x07];
    let mut cpu = cpu_with_zf(false);
    cpu.gpr[Register::Rdi as usize] = DATA;
    let before = cpu.clone();
    let mut ram = Ram::with_code(&code);
    step(&code, &mut cpu, &mut ram);
    let mut expected = before;
    expected.rip += 4;
    assert_eq!(cpu, expected);
    let mut cpu = cpu_with_zf(false);
    cpu.gpr[Register::Rdi as usize] = 0x1_0000_0000;
    let mut ram = Ram::with_code(&code);
    assert!(matches!(
        step_outcome(&code, &mut cpu, &mut ram),
        Err(MemoryError::Unmapped { .. })
    ));
}

fn cpu_with_zf(zf: bool) -> LongModeState {
    let mut cpu = cpu();
    cpu.rflags = if zf {
        ALL_FLAGS | 0x40
    } else {
        ALL_FLAGS & !0x40
    };
    cpu
}

/// TSC: `0F 31` RDTSC puts the low 32 bits of the injected counter in EAX and the high 32 bits in EDX,
/// clearing the upper halves of RAX and RDX; with CR4.TSD = 1 at CPL > 0 it is #GP(0) and the source is
/// not read (Intel SDM Vol. 2B, RDTSC, pp. 4-559 to 4-560).
fn rdtsc(privilege: PrivilegeLevel, cr4_tsd: bool, value: u64) -> (LongModeState, RunExit, u32) {
    let mut cpu = cpu();
    let mut ram = Ram::with_code(&[0x0f, 0x31]);
    let mut calls = 0;
    let mut source = || {
        calls += 1;
        value
    };
    let result = execute::run_with_rdtsc_source(
        &mut cpu,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(CODE + 2),
            context: AccessContext {
                privilege,
                ..AccessContext::default()
            },
        },
        RdtscControls { cr4_tsd },
        &mut source,
        || false,
    );
    (cpu, result.exit, calls)
}

#[test]
fn tsc_rdtsc_splits_the_injected_counter_into_edx_eax() {
    let before = cpu();
    let (cpu, exit, calls) = rdtsc(PrivilegeLevel::Ring0, false, 0xfedc_ba98_7654_3210);
    assert_eq!(exit, RunExit::StopAddress);
    assert_eq!(calls, 1, "the counter is read once");
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x7654_3210);
    assert_eq!(cpu.gpr[Register::Rdx as usize], 0xfedc_ba98);
    assert_eq!(cpu.rip, CODE + 2);
    // Every other register and flag is preserved.
    let mut expected = before;
    expected.gpr[Register::Rax as usize] = 0x7654_3210;
    expected.gpr[Register::Rdx as usize] = 0xfedc_ba98;
    expected.rip = CODE + 2;
    assert_eq!(cpu, expected);
    // CR4.TSD = 1 does not restrict CPL 0.
    let (cpu, exit, calls) = rdtsc(PrivilegeLevel::Ring0, true, 1);
    assert_eq!(
        (exit, calls, cpu.gpr[Register::Rax as usize]),
        (RunExit::StopAddress, 1, 1)
    );
}

#[test]
fn tsc_rdtsc_with_cr4_tsd_is_gp0_outside_cpl0_and_does_not_read_the_source() {
    for privilege in [
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        let before = cpu();
        let (cpu, exit, calls) = rdtsc(privilege, true, 0x1234);
        assert_eq!(
            exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }),
            "{privilege:?}"
        );
        assert_eq!(calls, 0, "{privilege:?}: the counter is not read");
        assert_eq!(cpu, before, "{privilege:?}: no state change");
        // With TSD clear the same CPL reads the counter.
        let (_, exit, calls) = rdtsc(privilege, false, 0x1234);
        assert_eq!((exit, calls), (RunExit::StopAddress, 1), "{privilege:?}");
    }
}

// ---- slice C: the bits that stay clear have a live blocker ----

/// Each of these forms is what keeps a bit clear. When a task implements one, the matching
/// `UNDECLARED` row has to be re-examined, and this test is the reminder.
#[test]
fn the_forms_that_block_fpu_msr_and_syscall_are_still_unsupported() {
    let unsupported = |code: &[u8], setup: &dyn Fn(&mut LongModeState)| {
        let mut cpu = cpu();
        setup(&mut cpu);
        let mut ram = Ram::with_code(code);
        let before = cpu.clone();
        let outcome = step_outcome(code, &mut cpu, &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{code:02x?}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{code:02x?}: refused without a state change");
    };
    // FPU, blocker 1: the CPL0 set keeps `LongModeState.x87` None between instructions; the x87 half of the FP
    // cell is lent to the MMX forms (S2-39o) and, since S2-32d, to the forms of the bounded x87 profile of
    // S2-32a/S2-32b/S2-32c (s2_32d_cpl0_x87.rs): FNSTENV/FNSAVE/FRSTOR run in the set, FSIN..FXTRACT and
    // FBLD/FBSTP are gated and then refused while a selector is represented (the set admits no cell without
    // them: no FCS/FDS bypass), and every other x87 arithmetic form stays refused. The FPU bit remains
    // undeclared (S2-57). This test checks the refusal of the forms on a state without x87: FSIN (D9 FE),
    // FPATAN (D9 F3), F2XM1 (D9 F0), FSCALE (D9 FD), FBLD m80 [RAX] (DF 20), FBSTP m80 [RAX] (DF 30),
    // FNSTENV (D9 30), FNSAVE (DD 30) and FRSTOR (DD 20).
    for code in [
        &[0xd9u8, 0xfe][..],
        &[0xd9, 0xf3],
        &[0xd9, 0xf0],
        &[0xd9, 0xfd],
        &[0xdf, 0x20],
        &[0xdf, 0x30],
        &[0xd9, 0x30],
        &[0xdd, 0x30],
        &[0xdd, 0x20],
    ] {
        unsupported(code, &|cpu| {
            cpu.x87 = None;
            cpu.gpr[Register::Rax as usize] = DATA;
        });
    }
    // Control for the above: the same FSIN completes when x87 state with a valid ST(0) is present, so the
    // refusal comes from the missing x87 state and not from the operand or the status word.
    {
        let mut cpu = cpu();
        let mut x87 = X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x1000; // TOP = 2, no exception flags
        x87.tag_word = 0xffff;
        x87.registers[2] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]; // 1.0
        x87.set_tag(2, X87Tag::Valid);
        cpu.x87 = Some(x87);
        let mut ram = Ram::with_code(&[0xd9, 0xfe]);
        let outcome = step_outcome(&[0xd9, 0xfe], &mut cpu, &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Advanced)),
            "FSIN with x87 state: {outcome:?}"
        );
    }
    // FPU, blocker 2 (S2-32c): FNSTENV (D9 /6), FNSAVE (DD /6) and FRSTOR (DD /4) at [RAX] exist in the bounded
    // profile as well (the FSTENV/FSAVE spellings add FWAIT, which only the set owns). With clean x87 state the
    // bounded profile completes FNSTENV.
    {
        let mut cpu = cpu();
        let mut x87 = X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x0000;
        x87.tag_word = 0xffff;
        cpu.x87 = Some(x87);
        cpu.gpr[Register::Rax as usize] = DATA;
        let mut ram = Ram::with_code(&[0xd9, 0x30]);
        let outcome = step_outcome(&[0xd9, 0x30], &mut cpu, &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Advanced)),
            "FNSTENV with x87 state: {outcome:?}"
        );
    }
    // The same three forms on a state without x87 (x87 = None): refused with no state change and with the
    // 108-byte image area untouched (FNSTENV/FNSAVE write nothing, FRSTOR loads nothing).
    for code in [&[0xd9u8, 0x30][..], &[0xdd, 0x30], &[0xdd, 0x20]] {
        let mut cpu = cpu();
        cpu.x87 = None;
        cpu.gpr[Register::Rax as usize] = DATA;
        let mut ram = Ram::with_code(code);
        ram.bytes[DATA as usize..DATA as usize + 108].fill(0xa5);
        let before = cpu.clone();
        let image = ram.bytes[DATA as usize..DATA as usize + 108].to_vec();
        let outcome = step_outcome(code, &mut cpu, &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{code:02x?} with x87 = None: {outcome:?}"
        );
        assert_eq!(cpu, before, "{code:02x?}: refused without a state change");
        assert_eq!(
            ram.bytes[DATA as usize..DATA as usize + 108],
            image[..],
            "{code:02x?}: the image area is untouched"
        );
    }
    // MSR: RDMSR of IA32_APIC_BASE (0x1B) and WRMSR of IA32_PAT (0x277) in the bare run.
    unsupported(&[0x0f, 0x32], &|cpu| cpu.gpr[Register::Rcx as usize] = 0x1b);
    unsupported(&[0x0f, 0x30], &|cpu| {
        cpu.gpr[Register::Rcx as usize] = 0x277
    });
    // SYSCALL: bare run, no board member.
    unsupported(&[0x0f, 0x05], &|_| {});
}
