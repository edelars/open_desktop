//! S2-57: the Windows CPUID profile `od-x86-64-v2-windows-v0.1` (`CpuidProfile::OdX8664V2Windows`).
//!
//! Every expected value is written by hand from the Intel SDM (Vol. 1, "Processor identification and
//! feature determination": leaf 01H ECX/EDX tables, leaf 80000001H, brand string; Vol. 2 instruction
//! pages) or from a published check value (CRC-32C of "123456789" is 0xE3069283, RFC 3720), never
//! from the implementation under test.
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
use od_isa::execute::{self, RunConfig, RunExit, StepOutcome};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, GuestMemory, LongModeState, MemoryError, PrivilegeLevel, Register,
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
const DECLARED: [(Reg, u32, &str, &str); 8] = [
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
const UNDECLARED: [(Reg, u32, &str, &str); 34] = [
    (Reg::Edx1, 0, "FPU", "x87 S2-28..S2-32 not complete"),
    (Reg::Edx1, 3, "PSE", "no task"),
    (Reg::Edx1, 4, "TSC", "S2-26 not accepted"),
    (Reg::Edx1, 5, "MSR", "S2-25/S2-26 not accepted"),
    (Reg::Edx1, 6, "PAE", "S2-58 not accepted"),
    (Reg::Edx1, 8, "CX8", "CMPXCHG8B not implemented (S2-21)"),
    (Reg::Edx1, 9, "APIC", "S2-58 not accepted"),
    (Reg::Edx1, 13, "PGE", "no task"),
    (
        Reg::Edx1,
        15,
        "CMOV",
        "S2-16 not accepted (physical check pending)",
    ),
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
    (Reg::Edx80000001, 11, "SYSCALL", "S2-27 not accepted"),
    (Reg::Edx80000001, 20, "NX", "S2-58 not accepted"),
    (Reg::Edx80000001, 26, "1 GiB pages", "no task"),
    (Reg::Edx80000001, 27, "RDTSCP", "no task"),
    (Reg::Edx80000001, 29, "LM", "S2-24/S2-25 not accepted"),
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
    // Nothing is declared in either EDX: no bit is left over that the lists above do not name.
    assert_eq!(register(Reg::Edx1), 0);
    assert_eq!(register(Reg::Edx80000001), 0);
    let mut declared_ecx1 = 0u32;
    for (reg, bit, _, _) in DECLARED {
        if matches!(reg, Reg::Ecx1) {
            declared_ecx1 |= 1 << bit;
        }
    }
    assert_eq!(declared_ecx1, register(Reg::Ecx1));
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
    // The Windows profile shares none of its EDX bits and has its own identity.
    assert_ne!(WINDOWS.query(0, 0), linux.query(0, 0));
    assert_eq!(WINDOWS.query(1, 0).edx & linux.query(1, 0).edx, 0);
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
    assert_eq!(cpuid_leaf(1), [0x0009_06c0, 0x100, 0x0098_2201, 0]);
    assert_eq!(cpuid_leaf(0x8000_0000), [0x8000_0008, 0, 0, 0]);
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
