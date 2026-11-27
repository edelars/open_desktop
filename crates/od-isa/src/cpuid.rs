//! Caller-selected CPUID profiles supported by the local reference interpreter.
//!
//! `DiagnosticV1` is a deliberately non-bootable diagnostic identity. Its
//! feature masks are zero, so it is not an `x86-64-v1` CPU and is not an
//! Ubuntu boot contract.
//!
//! `X8664V1Stage4A` (S4-38L9a) is the first named subset of the Linux board target
//! `od-x86-64-v1-neutral-v0.1` (`docs/specs/CPU_CONTRACT.md`): it declares only the features whose
//! behaviour the board's CPU set already implements (TSC, through `RDTSC`; since S4-38L9d the FPU
//! and FXSR bits are declared, but only `FNINIT` and the plain `FXSAVE` of the set's FP member are
//! implemented: every other x87/FXSR form, e.g. `FXRSTOR`, stays `Unsupported`). It is not the
//! full target table and not an Ubuntu boot contract.
//!
//! `X8664V1Stage4AApic` (S4-38L8d) is `X8664V1Stage4A` plus the APIC bit. `X8664V1Stage4AExt1`
//! (S7-31h) is `X8664V1Stage4A` plus the extended leaf 0x80000001 EDX of the target (SYSCALL, NX,
//! LM); `X8664V1Stage4AExt2` (S7-31i) is `Ext1` plus SSE and SSE2 in leaf 1 EDX. Ext1 and Ext2
//! are selected only by the unified UEFI+Linux loop; `X8664V1Stage4A` itself is unchanged.
//!
//! `OdX8664V1Neutral` (S6-14h) is the full CPUID table of that target (leaves 0, 1, 0x80000000,
//! 0x80000001 and 0x80000008). It is selected only explicitly and declares feature bits whose
//! behaviour the engine does not implement yet, so no existing path uses it.
//!
//! `OdX8664V2Windows` (S2-57) is the Windows profile `od-x86-64-v2-windows-v0.1` (ADR-0013,
//! `CPU_CONTRACT.md`): the identity of an Intel Celeron N5105 and, in leaves 1 and 0x80000001,
//! only the feature bits whose implementing tasks are done (not the table of
//! `OdX8664V1Neutral`, which is never inherited). It is an intermediate profile, not yet a Windows
//! boot contract: SSE/SSE2, FXSR, FPU, MMX, TSC, MSR, SYSCALL and LM stay clear until their tasks
//! are accepted (owner decision of 2026-11-21: FPU, TSC, MSR, SYSCALL and LM are not declared until
//! a separate owner decision). S2-57 (slice C) declares CX8 (EDX[8]) and CMOV (EDX[15]), whose forms
//! are implemented and tested and whose tasks S2-21 and S2-16 the owner accepted (confirmed
//! 2026-11-21; `CPU_CONTRACT.md`). S2-58 declares PAE (EDX[6]) and APIC (EDX[9]) before the owner
//! accepted S2-25 and S4-38L8d (deviation, confirmed by the owner 2026-11-21; `CPU_CONTRACT.md`). NX
//! (80000001H EDX[20]) is NOT declared: the owner deferred it until S4-10 is accepted (decision of
//! 2026-11-21). The interpreter does not gate a form on these bits except one: `WRMSR` setting
//! `EFER.NXE` needs NX in the set's own CPUID member (S2-25f), so with this profile it is #GP(0).
//! S7-32 slice O raises its maximum basic leaf from 1 to 6 for ARAT (leaf 6 EAX[2]) and defines
//! the leaves that become valid with it: 2 (descriptor FFH), 3 (zero), 4 (the caches of the
//! identity, by sub-leaf) and 5 (zero, MONITOR is not declared).

/// The four architecturally 32-bit outputs of CPUID.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// An immutable CPUID profile explicitly selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuidProfile {
    /// Local diagnostic identity with all CPUID feature masks disabled.
    DiagnosticV1,
    /// Subset of the Linux board target: `DiagnosticV1` plus the TSC bit (CPUID.01H:EDX[4], S4-38L9a),
    /// the FPU bit (EDX[0]) and the FXSR bit (EDX[24]) (S4-38L9d), and nothing else. Every other
    /// feature bit of the target table (APIC, PAT, SEP, SSE, SSE2, ...) stays clear until the slice
    /// that implements its behaviour declares it.
    X8664V1Stage4A,
    /// `X8664V1Stage4A` plus the APIC bit (CPUID.01H:EDX[9], S4-38L8d) and nothing else: the board
    /// then provides a xAPIC-mode local APIC and an IOAPIC (MMIO windows opened by the runner flag of
    /// the same profile). x2APIC (ECX[21]), TSC-deadline (ECX[24]) and the hypervisor bit (ECX[31])
    /// stay clear; leaves 6, 0xB, 0x15 and 0x16 stay zero.
    X8664V1Stage4AApic,
    /// S7-31h: `X8664V1Stage4A` plus CPUID.80000001H:EDX of the target table (SYSCALL bit 11, NX bit
    /// 20, LM bit 29: `0x20100800`) and nothing else. Selected only by the unified UEFI+Linux loop:
    /// the kernel of that run reads NX from leaf 0x80000001 and, without it, prints "NX (Execute
    /// Disable) protection missing in CPU!", strips `_PAGE_NX` from its page-table mask and fails
    /// `efi_map_region` (ISSUE-634). The board already runs with EFER.NXE and LME/LMA, honors NX in
    /// the walker and has `SYSCALL` in the set (`with_syscall`); `X8664V1Stage4A` itself stays as it
    /// was, so the evidence of the linux-direct runs is unchanged.
    X8664V1Stage4AExt1,
    /// S7-31i: `X8664V1Stage4AExt1` plus SSE (CPUID.01H:EDX bit 25) and SSE2 (bit 26) and nothing else.
    /// Selected only by the unified UEFI+Linux loop: the kernel executes `LDMXCSR` in
    /// `kernel_fpu_begin_mask` (`efi_set_virtual_address_map`; XMM is in its required-feature mask),
    /// which the FP member of the CPL0 set implements and admits only over a profile that declares
    /// SSE (ISSUE-636). `X8664V1Stage4A` and `X8664V1Stage4AExt1` stay as they were.
    X8664V1Stage4AExt2,
    /// S6-14h: the complete table of `od-x86-64-v1-neutral-v0.1` (`CPU_CONTRACT.md`): leaf 1 EDX
    /// `0x0781A379` (FPU, PSE, TSC, MSR, PAE, CX8, APIC, PGE, CMOV, PAT, MMX, FXSR, SSE, SSE2),
    /// leaf 1 ECX 0, and 0x80000001 EDX `0x20100800` (SYSCALL, NX, LM). Never the default; the
    /// Stage 4 board gates (`is_stage4a_family`) do not treat it as a member of that family.
    OdX8664V1Neutral,
    /// S2-57: the Windows profile `od-x86-64-v2-windows-v0.1` (ADR-0013, `CPU_CONTRACT.md`). Never the
    /// default and not a member of the Stage 4 family. Leaf 1 ECX declares SSE3, SSSE3, CMPXCHG16B,
    /// SSE4.1, SSE4.2 and POPCNT, leaf 0x80000001 ECX declares LAHF/SAHF and PREFETCHW; leaf 1 EDX
    /// declares PAE, CX8, APIC and CMOV (`0x8340`; S2-58 and S2-57 slice C) and leaf 0x80000001 EDX stays zero
    /// (NX deferred until S4-10). The Linux target `OdX8664V1Neutral` is unchanged. Since S7-32
    /// (2026-11-25) leaf 1 EDX is the target table plus, since slice I, DE, MCE, MTRR, MCA and
    /// CLFSH (`0x0789F3FD`, see `WINDOWS_LEAF1_EDX`), and leaf 0x80000001 EDX is SYSCALL, NX, LM.
    /// Since S7-32 slice O the maximum basic leaf is 6: leaves 2..=6 are defined (`windows_query`),
    /// leaf 6 declares ARAT only.
    OdX8664V2Windows,
}

/// CPUID.01H:EDX bit 9 (APIC), declared by `X8664V1Stage4AApic` since S4-38L8d. From the Intel SDM,
/// from memory.
const LEAF1_EDX_APIC: u32 = 1 << 9;
/// CPUID.01H:EDX bit 4 (TSC); the bit position is from the Intel SDM, from memory.
const LEAF1_EDX_TSC: u32 = 1 << 4;
/// CPUID.01H:EDX bit 0 (FPU), declared since S4-38L9d: `FNINIT` is implemented in the CPL0 set's FP
/// member (the kernel's `fpu__init_system` gives up without this bit). From the Intel SDM, from memory.
const LEAF1_EDX_FPU: u32 = 1 << 0;
/// CPUID.01H:EDX bit 24 (FXSR), declared since S4-38L9d: the plain `FXSAVE` is implemented in the
/// same member. SSE (25) and SSE2 (26) are NOT declared by the profiles up to `X8664V1Stage4AExt1`;
/// `X8664V1Stage4AExt2` declares them (S7-31i, the `LDMXCSR` of the FP member). From the Intel SDM,
/// from memory.
const LEAF1_EDX_FXSR: u32 = 1 << 24;
/// CPUID.01H:EDX bit 25 (SSE) and bit 26 (SSE2), declared by `X8664V1Stage4AExt2` (S7-31i); the
/// positions are those of the target table's `0x0781A379`. From the Intel SDM, from memory.
const LEAF1_EDX_SSE: u32 = 1 << 25;
const LEAF1_EDX_SSE2: u32 = 1 << 26;
/// CPUID.01H:EDX of the full target table (S6-14h), the contract's `0x0781A379`: bits 0 FPU, 3 PSE,
/// 4 TSC, 5 MSR, 6 PAE, 8 CX8, 9 APIC, 13 PGE, 15 CMOV, 16 PAT, 23 MMX, 24 FXSR, 25 SSE, 26 SSE2.
const NEUTRAL_LEAF1_EDX: u32 = 0x0781_A379;
/// CPUID.80000001H:EDX of the full target table (S6-14h), the contract's `0x20100800`: bits 11
/// SYSCALL, 20 NX, 29 LM.
const NEUTRAL_EXT1_EDX: u32 = 0x2010_0800;

/// CPUID.01H:EAX of the Windows profile (ADR-0013, decision 2): the signature of Intel family 6,
/// model 0x9C, stepping 0 (extended model 9, model 0xC, family 6, processor type 0, extended family
/// 0). Layout from the Intel SDM (Vol. 1, "Leaf 01H Version and Features Returned in EAX").
const WINDOWS_LEAF1_EAX: u32 = 0x0009_06C0;
/// CPUID.01H:ECX of the Windows profile: SSE3 [0] (S2-54), SSSE3 [9] (S2-54), CMPXCHG16B [13]
/// (S2-51), SSE4_1 [19] (S2-55), SSE4_2 [20] (S2-56) and POPCNT [23] (S2-56); bit numbers from the
/// Intel SDM (Vol. 1, "Leaf 01H Version and Features Returned in ECX"). Not declared: MONITOR [3],
/// VMX [5], AES [25], XSAVE [26], OSXSAVE [27], AVX [28], the hypervisor bit [31] and every other
/// bit (ADR-0013, decisions 3 and 4).
const WINDOWS_LEAF1_ECX: u32 = (1 << 0) | (1 << 9) | (1 << 13) | (1 << 19) | (1 << 20) | (1 << 23);
/// CPUID.01H:EDX of the Windows profile: the bits of the full target table `0x0781A379` (FPU [0],
/// PSE [3], TSC [4], MSR [5], PAE [6], CX8 [8], APIC [9], PGE [13], CMOV [15], PAT [16], MMX [23],
/// FXSR [24], SSE [25], SSE2 [26]; bit numbers from the Intel SDM rev. 093, Vol. 1, Table 21-10).
/// Until 2026-11-25 only PAE, CX8, APIC and CMOV (`0x8340`) were declared and the rest was held for
/// separate owner decisions (S2-57). S7-32 decision of 2026-11-25 (architectural, under the owner's
/// instruction for S7-32 to decide the architecture): declare the whole set. The Windows kernel needs
/// them (PAE, NX, SSE2 and CMPXCHG16B are required by M2 §3.5; the CPL0 FP member refuses a profile
/// without FPU/FXSR/SSE, the first stop of the Windows profile run, `FpFeaturesNotDeclared`), and the
/// interpreter implements their core forms (the FP member and SSE/MMX cores, RDTSC, RDMSR/WRMSR of
/// the modelled MSRs, 2 MiB pages). Known gaps are stops of a run, not wrong results: x87 forms
/// outside the bounded profile, FXSAVE forms other than the plain ones, and MSRs the unified set
/// does not bind (IA32_PAT, IA32_MISC_ENABLE, IA32_APIC_BASE reads and writes; the Windows MSR file
/// `with_msr_windows` exists in od-isa but no board binds it yet).
/// S7-32 slice I (architectural decision, ADR-0013 "S7-32, slice I"): DE [2], MCE [7], MTRR [12],
/// MCA [14] and CLFSH [19] are declared too, `0x0789F3FD`. The kernel of the Windows media requires
/// `(CPUID.01H:EDX & 0x0789F3FD) == 0x0789F3FD` and otherwise stops with bug check 0x5D
/// UNSUPPORTED_PROCESSOR (observed: `KeBugCheckEx(0x5D, 0x0781A379, 0x20100800, 0x101, 0)`); the
/// Celeron N5105 of the identity has all five. What backs them: CR4.DE and CR4.MCE are stored bits
/// of the CPL0 set (DR4/DR5 are #UD with CR4.DE = 1; no #MC is ever raised), the MTRR and MCA MSRs
/// are modelled (`crate::msr::MtrrState`, `crate::msr::McaState`; memory types have no effect: the
/// model has no caches), `CLFLUSH` is the byte-read access check of the set and leaf 1 EBX[15:8]
/// gives its line size.
/// Not declared: VME [1], SEP [11], PSE-36 [17], HTT [28] and the rest.
const WINDOWS_LEAF1_EDX: u32 = 0x0789_F3FD;
/// CPUID.01H:EBX of the Windows profile for one vCPU (S7-32 slice I): bits 15:8 = 8, the CLFLUSH
/// line size in 8-byte units (64 bytes, the line size of the N5105 and of every x86-64 core the
/// identity could stand for; SDM Vol. 2A CPUID, Table 3-8 "EBX bits 15-08"), which is valid because
/// CLFSH is declared. Bits 7:0 (brand index) are 0: the brand string of leaves 0x80000002..4 is
/// used. Bits 23:16 (addressable logical processor IDs, valid only with HTT, which is not declared)
/// and bits 31:24 (initial APIC ID) are 0 here and set per vCPU by [`CpuidProfile::query_for_vcpu`]
/// (S6-18a) when the board has more than one vCPU. Until slice I the profile kept the Linux
/// target's `0x100` (a line size field of 1, meaningless while CLFSH was clear).
const WINDOWS_LEAF1_EBX: u32 = 8 << 8;
/// CPUID.80000001H:ECX of the Windows profile: LAHF/SAHF in 64-bit mode [0] (S2-52) and PREFETCHW
/// [8] (S2-53), from the Intel SDM (Vol. 1, "Leaf 80000001H"). LZCNT [5] is not declared.
const WINDOWS_EXT1_ECX: u32 = (1 << 0) | (1 << 8);
/// CPUID.80000001H:EDX of the Windows profile: SYSCALL [11], NX [20] and LM [29] (`0x20100800`, the
/// target table's; bit numbers from the Intel SDM rev. 093, Vol. 1, Table 21-90). NX was deferred by
/// the owner until S4-10 was accepted (2026-11-21), and S4-10 was accepted by the owner on
/// 2026-11-21; SYSCALL and LM wait no longer (S7-32 decision of 2026-11-25, see the leaf 1 EDX note).
/// Page1GB [26] and RDTSCP [27] are not declared.
const WINDOWS_EXT1_EDX: u32 = 0x2010_0800;
/// CPUID.80000002H..80000004H of the Windows profile: the brand string of an Intel Celeron N5105
/// (ADR-0013, decision 2), NUL-padded to 48 bytes (Intel SDM, Vol. 1, "Methods for Returning
/// Branding Information Using CPUID").
const WINDOWS_BRAND: [u8; 48] = {
    let text = b"Intel(R) Celeron(R) N5105 @ 2.00GHz";
    let mut out = [0u8; 48];
    let mut index = 0;
    while index < text.len() {
        out[index] = text[index];
        index += 1;
    }
    out
};

/// One little-endian 32-bit word of [`WINDOWS_BRAND`].
const fn windows_brand_word(index: usize) -> u32 {
    u32::from_le_bytes([
        WINDOWS_BRAND[4 * index],
        WINDOWS_BRAND[4 * index + 1],
        WINDOWS_BRAND[4 * index + 2],
        WINDOWS_BRAND[4 * index + 3],
    ])
}

/// The brand-string leaf `0x8000_0002 + part` (`part` 0..=2): four consecutive words.
const fn windows_brand_leaf(part: usize) -> CpuidResult {
    CpuidResult {
        eax: windows_brand_word(4 * part),
        ebx: windows_brand_word(4 * part + 1),
        ecx: windows_brand_word(4 * part + 2),
        edx: windows_brand_word(4 * part + 3),
    }
}

/// CPUID.00H:EAX of the Windows profile, the maximum basic leaf. S7-32 slice O (ISSUE-703,
/// ADR-0013 "S7-32, slice O"): 6, the lowest value that makes leaf 6 (ARAT) valid; until slice O
/// it was 1. Leaves 2..=5 become valid with it and are defined below. The leaves above it (7,
/// 0xB, 0xD, 0x15, ...) stay four zeros, which is not the SDM's "data for the highest basic
/// information leaf" (Vol. 1, 21.1.3): see [`windows_query`].
const WINDOWS_MAX_BASIC_LEAF: u32 = 6;
/// CPUID.02H of the Windows profile (S7-32 slice O; SDM Vol. 1 Table 21-11 and Table 21-12):
/// EAX[7:0] is the reserved value 01H, EAX[15:8] is descriptor FFH ("CPUID leaf 2 does not report
/// cache descriptor information, use CPUID leaf 4 to query cache parameters"); every other byte
/// is the null descriptor 00H and bit 31 of each register is 0 (valid). A Jasper Lake part of the
/// identity reports `0x00FEFF01` (descriptor FEH: TLBs through leaf 18H); FEH is left out because
/// leaf 18H is above the maximum basic leaf of the profile.
const WINDOWS_LEAF2_EAX: u32 = 0x0000_FF01;
/// CPUID.04H sub-leaves 0..=3 of the Windows profile for one vCPU (S7-32 slice O; field layout
/// SDM Vol. 1 Table 21-14): the caches of a Jasper Lake part of the identity (family 6, model 0x9C,
/// stepping 0; values of the InstLatx64 CPUID dumps of the N4505 and N5095A, the same die as the
/// N5105 of ADR-0013; the L3 size agrees with the "4 MB" of Intel ARK for the N5105). The board has
/// no caches, so the values are information only: no instruction behaves by them, and the only
/// field a guest can observe in behaviour, the 64-byte line, is the CLFLUSH line of leaf 1
/// EBX[15:8]. EAX[25:14] (logical processors sharing the cache, minus 1) and EAX[31:26] (cores
/// in the package, minus 1) are 0 here, one logical processor; [`CpuidProfile::query_for_vcpu`]
/// sets them for more vCPUs. Sub-leaf 4 and above are four zeros (type 0, the end).
/// - 0: L1 data, type 1, level 1, self-initialising; 64-byte lines, 1 partition, 8 ways, 64 sets
///   (32 KiB).
/// - 1: L1 instruction, type 2, otherwise as sub-leaf 0 (32 KiB).
/// - 2: L2 unified, type 3, level 2; 12 ways, 2048 sets (1.5 MiB).
/// - 3: L3 unified, type 3, level 3; 16 ways, 4096 sets (4 MiB); EDX: inclusive [1] and complex
///   indexing [2], as the part reports (information only, like the rest).
const WINDOWS_CACHES: [CpuidResult; 4] = [
    CpuidResult {
        eax: 0x0000_0121,
        ebx: 0x01C0_003F,
        ecx: 0x3F,
        edx: 0,
    },
    CpuidResult {
        eax: 0x0000_0122,
        ebx: 0x01C0_003F,
        ecx: 0x3F,
        edx: 0,
    },
    CpuidResult {
        eax: 0x0000_0143,
        ebx: 0x02C0_003F,
        ecx: 0x7FF,
        edx: 0,
    },
    CpuidResult {
        eax: 0x0000_0163,
        ebx: 0x03C0_003F,
        ecx: 0xFFF,
        edx: 0x6,
    },
];
/// The lowest cache level of [`WINDOWS_CACHES`] that every vCPU of the board shares (L2 and L3,
/// as on the identity, whose four cores share both; L1 is per core).
const WINDOWS_SHARED_CACHE_LEVEL: u32 = 2;
/// CPUID.06H:EAX of the Windows profile (S7-32 slice O, ISSUE-703; SDM Vol. 1 Table 21-16): only
/// ALWAYS_RUNNING_APIC_TIMER [2] (ARAT). SDM Vol. 3A 13.5.4: with ARAT "the processor's APIC timer
/// runs at a constant rate regardless of P-state transitions and it continues to run at the same
/// rate in deep C-states"; the board's LAPIC timer counts on the board clock, which has no
/// P-states and keeps running in `HLT`. Not declared (the model has none of their MSRs or
/// behaviour): DTS [0], Turbo Boost [1], PLN [4], ECMD [5], PTM [6], HWP [7..=11, 15..=18, 20, 22],
/// HDC [13], Turbo Boost Max [14], HW feedback [19], Thread Director [23]; EBX (DTS thresholds),
/// ECX (MPERF/APERF [0], ENERGY_PERF_BIAS [3], Thread Director classes) and EDX are 0. The part of
/// the identity reports EAX `0x00178FF7`, EBX 2, ECX 9.
const WINDOWS_LEAF6_EAX: u32 = 1 << 2;

/// CPUID of `OdX8664V2Windows`: leaves 0..=6, 0x80000000, 0x80000001, 0x80000002..=0x80000004
/// and 0x80000008; every other leaf is four zeros. The maximum basic leaf is
/// [`WINDOWS_MAX_BASIC_LEAF`] (S7-32 slice O; before it 1), the maximum extended leaf is
/// 0x80000008. Leaf 1 EBX is [`WINDOWS_LEAF1_EBX`] (topology per vCPU by
/// [`CpuidProfile::query_for_vcpu`], S6-18); leaf 2 is [`WINDOWS_LEAF2_EAX`]; leaf 3 is zero (no
/// processor serial number: SDM Vol. 1 Table 21-13, Pentium III only); leaf 4 is
/// [`WINDOWS_CACHES`] by `subleaf` (the only leaf that reads ECX); leaf 5 is zero because MONITOR
/// (leaf 1 ECX[3]) is not declared; leaf 6 is [`WINDOWS_LEAF6_EAX`]; leaf 0x80000008 EAX is the
/// Linux target's `0x3024` (48-bit linear, 36-bit physical).
///
/// A leaf above the maximum basic or extended leaf returns four zeros, as every leaf of every
/// profile that is not listed. The SDM (Vol. 1, 21.1.3) says a processor returns "the data for the
/// highest basic information leaf" there and that software "should not rely on the values"; zeros
/// are kept so that a guest that reads leaf 7, 0xB, 0xD, 0x15, ... without checking the maximum
/// sees no feature (leaf 6 data in leaf 7 would be EAX = 4, four sub-leaves), exactly as before the
/// maximum was raised.
const fn windows_query(leaf: u32, subleaf: u32) -> CpuidResult {
    match leaf {
        0 => CpuidResult {
            eax: WINDOWS_MAX_BASIC_LEAF,
            ebx: u32::from_le_bytes(*b"Genu"),
            ecx: u32::from_le_bytes(*b"ntel"),
            edx: u32::from_le_bytes(*b"ineI"),
        },
        1 => CpuidResult {
            eax: WINDOWS_LEAF1_EAX,
            ebx: WINDOWS_LEAF1_EBX,
            ecx: WINDOWS_LEAF1_ECX,
            edx: WINDOWS_LEAF1_EDX,
        },
        2 => CpuidResult {
            eax: WINDOWS_LEAF2_EAX,
            ebx: 0,
            ecx: 0,
            edx: 0,
        },
        4 if (subleaf as usize) < WINDOWS_CACHES.len() => WINDOWS_CACHES[subleaf as usize],
        6 => CpuidResult {
            eax: WINDOWS_LEAF6_EAX,
            ebx: 0,
            ecx: 0,
            edx: 0,
        },
        0x8000_0000 => CpuidResult {
            eax: 0x8000_0008,
            ebx: 0,
            ecx: 0,
            edx: 0,
        },
        0x8000_0001 => CpuidResult {
            eax: 0,
            ebx: 0,
            ecx: WINDOWS_EXT1_ECX,
            edx: WINDOWS_EXT1_EDX,
        },
        0x8000_0002 => windows_brand_leaf(0),
        0x8000_0003 => windows_brand_leaf(1),
        0x8000_0004 => windows_brand_leaf(2),
        0x8000_0008 => CpuidResult {
            eax: 0x3024,
            ebx: 0,
            ecx: 0,
            edx: 0,
        },
        _ => CpuidResult {
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
        },
    }
}

impl CpuidProfile {
    /// Whether the profile is `X8664V1Stage4A` or a variant of it (`X8664V1Stage4AApic`): the family
    /// the Linux board's stage 4 members (MSR 0x87, FP state, `INT3`) are turned on for.
    pub const fn is_stage4a_family(self) -> bool {
        matches!(self, Self::X8664V1Stage4A | Self::X8664V1Stage4AApic)
    }

    /// CPUID of one vCPU of a board with `cpu_count` vCPUs (S6-18a, `SMP_CONTRACT.md` section 2).
    /// Equal to [`Self::query`] except CPUID.01H:EBX when `cpu_count > 1` (and, since S7-32 slice
    /// O, CPUID.04H EAX of the Windows profile, below): bits 23:16 (logical
    /// processor count) become `cpu_count` and bits 31:24 (initial APIC ID) become `apic_id`
    /// (APIC ID = vCPU index). With `cpu_count <= 1` the result is `query`'s, byte for byte. The
    /// topology leaves `0xB`, `0x1F` and `0x80000026` stay absent (four zeros) at every count, as
    /// the contract says; CPUID.01H:EDX (HTT, bit 28) is not changed. Only the low 8 bits of
    /// `apic_id` and `cpu_count` are used (the fields are 8 bits wide).
    ///
    /// S7-32 slice O: the Windows profile's CPUID.04H (the only profile with that leaf) counts
    /// every vCPU as a core of one package without SMT (the identity has four cores and four
    /// threads): for each valid sub-leaf EAX[31:26] (cores in the package, minus 1) is
    /// `cpu_count - 1` and EAX[25:14] (logical processors sharing the cache, minus 1) is
    /// `cpu_count - 1` for L2 and L3 and 0 for the per-core L1 (SDM Vol. 1 Table 21-14). The
    /// 6-bit core field is capped at 63 only so that a larger count cannot spill into the other
    /// fields; a board has at most 4 vCPUs (`SMP_CONTRACT.md` 2), so the cap is not reached, and the
    /// SDM's saturated value belongs to processors that enumerate leaf 0xB or 0x1F, which this
    /// profile does not. Sub-leaves past the last cache stay four zeros. With N > 1 this declares
    /// N cores while HTT (CPUID.01H:EDX[28]) is 0, which by SDM Vol. 1 Table 21-10 means a single
    /// logical processor in the package: a deliberate extension of the open S6-18a topology (the
    /// Windows kernel, `ntkrnlmp` + `0x5c9714`, takes the core count from leaf 4 whatever HTT
    /// says); a guest that follows the SDM sees the contradiction. Not checked with a guest.
    pub const fn query_for_vcpu(
        self,
        leaf: u32,
        subleaf: u32,
        apic_id: u32,
        cpu_count: u32,
    ) -> CpuidResult {
        let mut result = self.query(leaf, subleaf);
        if leaf == 1 && cpu_count > 1 {
            result.ebx =
                (result.ebx & 0x0000_FFFF) | ((cpu_count & 0xFF) << 16) | ((apic_id & 0xFF) << 24);
        }
        let others = (cpu_count & 0xFF).saturating_sub(1);
        if matches!(self, Self::OdX8664V2Windows) && leaf == 4 && result.eax & 0x1F != 0 {
            let cores = if others > 0x3F { 0x3F } else { others };
            let level = (result.eax >> 5) & 0x7;
            let sharing = if level >= WINDOWS_SHARED_CACHE_LEVEL {
                others
            } else {
                0
            };
            result.eax |= (cores << 26) | (sharing << 14);
        }
        result
    }

    /// Return a CPUID tuple. ECX (`subleaf`) is read only by CPUID.04H of the Windows profile
    /// (S7-32 slice O); every other leaf of every profile ignores it.
    pub const fn query(self, leaf: u32, subleaf: u32) -> CpuidResult {
        if matches!(self, Self::OdX8664V2Windows) {
            return windows_query(leaf, subleaf);
        }
        let leaf1_edx = match self {
            Self::DiagnosticV1 => 0,
            Self::X8664V1Stage4A | Self::X8664V1Stage4AExt1 => {
                LEAF1_EDX_FPU | LEAF1_EDX_TSC | LEAF1_EDX_FXSR
            }
            Self::X8664V1Stage4AExt2 => {
                LEAF1_EDX_FPU | LEAF1_EDX_TSC | LEAF1_EDX_FXSR | LEAF1_EDX_SSE | LEAF1_EDX_SSE2
            }
            Self::X8664V1Stage4AApic => {
                LEAF1_EDX_FPU | LEAF1_EDX_TSC | LEAF1_EDX_APIC | LEAF1_EDX_FXSR
            }
            Self::OdX8664V1Neutral => NEUTRAL_LEAF1_EDX,
            Self::OdX8664V2Windows => WINDOWS_LEAF1_EDX,
        };
        match leaf {
            0 => CpuidResult {
                eax: 1,
                ebx: u32::from_le_bytes(*b"Open"),
                ecx: u32::from_le_bytes(*b"top "),
                edx: u32::from_le_bytes(*b"Desk"),
            },
            1 => CpuidResult {
                eax: 0x600,
                ebx: 0x100,
                ecx: 0,
                edx: leaf1_edx,
            },
            0x8000_0000 => CpuidResult {
                eax: 0x8000_0008,
                ebx: 0,
                ecx: 0,
                edx: 0,
            },
            0x8000_0001
                if matches!(
                    self,
                    Self::OdX8664V1Neutral | Self::X8664V1Stage4AExt1 | Self::X8664V1Stage4AExt2
                ) =>
            {
                CpuidResult {
                    eax: 0,
                    ebx: 0,
                    ecx: 0,
                    edx: NEUTRAL_EXT1_EDX,
                }
            }
            0x8000_0008 => CpuidResult {
                eax: 0x3024,
                ebx: 0,
                ecx: 0,
                edx: 0,
            },
            _ => CpuidResult {
                eax: 0,
                ebx: 0,
                ecx: 0,
                edx: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S7-31h: `X8664V1Stage4AExt1` is `X8664V1Stage4A` plus the extended leaf 0x80000001 EDX of
    /// the target table (SYSCALL, NX, LM) and nothing else; `X8664V1Stage4A` still declares none.
    #[test]
    fn stage4a_ext1_adds_only_syscall_nx_and_lm_to_leaf_0x80000001() {
        let base = CpuidProfile::X8664V1Stage4A;
        let ext = CpuidProfile::X8664V1Stage4AExt1;
        for leaf in [
            0,
            1,
            2,
            4,
            6,
            7,
            0xb,
            0x15,
            0x16,
            0x8000_0000,
            0x8000_0008,
            0x8000_0009,
        ] {
            assert_eq!(ext.query(leaf, 0), base.query(leaf, 0), "leaf {leaf:#x}");
        }
        assert_eq!(base.query(0x8000_0001, 0), CpuidResult::default());
        let result = ext.query(0x8000_0001, 0);
        assert_eq!((result.eax, result.ebx, result.ecx), (0, 0, 0));
        // SYSCALL (bit 11), NX (bit 20), LM (bit 29) and nothing else.
        assert_eq!(result.edx, (1 << 11) | (1 << 20) | (1 << 29));
        assert_eq!(result.edx, 0x2010_0800);
        // The Stage 4 board gates still treat only the two Stage4A profiles as that family.
        assert!(!ext.is_stage4a_family());
    }

    /// S7-31i: `X8664V1Stage4AExt2` is `Ext1` plus SSE (bit 25) and SSE2 (bit 26) in leaf 1 EDX and
    /// nothing else; `Ext1` and `Stage4A` keep their values (the S7-31h record and the linux-direct
    /// runs stay reproducible).
    #[test]
    fn stage4a_ext2_adds_only_sse_and_sse2_to_ext1() {
        let ext1 = CpuidProfile::X8664V1Stage4AExt1;
        let ext2 = CpuidProfile::X8664V1Stage4AExt2;
        for leaf in [
            0,
            2,
            4,
            6,
            7,
            0xb,
            0x15,
            0x16,
            0x8000_0000,
            0x8000_0001,
            0x8000_0008,
        ] {
            assert_eq!(ext2.query(leaf, 0), ext1.query(leaf, 0), "leaf {leaf:#x}");
        }
        let one = ext2.query(1, 0);
        assert_eq!(
            one,
            CpuidResult {
                edx: ext1.query(1, 0).edx | (1 << 25) | (1 << 26),
                ..ext1.query(1, 0)
            }
        );
        // SSE and SSE2 are both in the target table's leaf 1 word.
        assert_eq!(one.edx & NEUTRAL_LEAF1_EDX, one.edx);
        // The earlier profiles are as they were.
        assert_eq!(CpuidProfile::X8664V1Stage4A.query(1, 0).edx, 0x0100_0011);
        assert_eq!(ext1.query(1, 0).edx, 0x0100_0011);
        assert_eq!(ext1.query(0x8000_0001, 0).edx, 0x2010_0800);
        assert!(!ext2.is_stage4a_family());
    }

    /// S4-28b: the APIC timer has no TSC-deadline mode, so the profile must
    /// not declare it (CPUID.01H:ECX[24]) nor the APIC/TSC feature bits.
    #[test]
    fn diagnostic_v1_does_not_declare_tsc_deadline() {
        let leaf1 = CpuidProfile::DiagnosticV1.query(1, 0);
        assert_eq!(leaf1.ecx & (1 << 24), 0);
        assert_eq!(leaf1.edx & ((1 << 4) | (1 << 9)), 0);
    }

    #[test]
    fn diagnostic_v1_returns_fixed_identity_and_zero_feature_masks() {
        let profile = CpuidProfile::DiagnosticV1;
        let tuples = [
            (
                0,
                CpuidResult {
                    eax: 1,
                    ebx: u32::from_le_bytes(*b"Open"),
                    ecx: u32::from_le_bytes(*b"top "),
                    edx: u32::from_le_bytes(*b"Desk"),
                },
            ),
            (
                1,
                CpuidResult {
                    eax: 0x600,
                    ebx: 0x100,
                    ecx: 0,
                    edx: 0,
                },
            ),
            (
                0x8000_0000,
                CpuidResult {
                    eax: 0x8000_0008,
                    ebx: 0,
                    ecx: 0,
                    edx: 0,
                },
            ),
            (0x8000_0001, CpuidResult::default()),
            (0x8000_0002, CpuidResult::default()),
            (0x8000_0003, CpuidResult::default()),
            (0x8000_0004, CpuidResult::default()),
            (0x8000_0005, CpuidResult::default()),
            (0x8000_0006, CpuidResult::default()),
            (0x8000_0007, CpuidResult::default()),
            (
                0x8000_0008,
                CpuidResult {
                    eax: 0x3024,
                    ebx: 0,
                    ecx: 0,
                    edx: 0,
                },
            ),
            (2, CpuidResult::default()),
            (0x8000_0009, CpuidResult::default()),
        ];
        for (leaf, expected) in tuples {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(
                    profile.query(leaf, subleaf),
                    expected,
                    "leaf={leaf:#x}, ecx={subleaf:#x}"
                );
            }
        }
    }

    /// S4-38L9a, extended by S4-38L9d. Oracles written by hand from the contract table
    /// (`CPU_CONTRACT.md`, row `CPUID(1)`: EAX 0x600, EBX 0x100, ECX 0) and the SDM bit positions
    /// (FPU bit 0, TSC bit 4, FXSR bit 24), not from the implementation.
    #[test]
    fn stage4a_leaf_1_declares_only_fpu_tsc_and_fxsr() {
        let leaf1 = CpuidProfile::X8664V1Stage4A.query(1, 0);
        assert_eq!(leaf1.eax, 0x0000_0600);
        assert_eq!(leaf1.ebx, 0x0000_0100);
        assert_eq!(leaf1.ecx, 0);
        // 1 (FPU) | 1 << 4 (TSC) | 1 << 24 (FXSR).
        assert_eq!(leaf1.edx, 0x0100_0011, "only EDX bits 0, 4 and 24");
        // Declared bits.
        for (bit, name) in [(0, "FPU"), (4, "TSC"), (24, "FXSR")] {
            assert_ne!(leaf1.edx & (1 << bit), 0, "{name} must be declared");
        }
        // Feature bits of the target table that this subset must not declare. SEP (bit 11) is
        // not in the target table either (SEP=0 by the contract) and stays clear; the table's
        // 0x0781A379 is not asserted bit by bit here. SSE and SSE2 stay clear: the set has no SSE
        // instruction, only the FXSAVE image of the (zero) XMM registers and MXCSR.
        for (bit, name) in [
            (3, "PSE"),
            (5, "MSR"),
            (6, "PAE"),
            (8, "CX8"),
            (9, "APIC"),
            (11, "SEP"),
            (13, "PGE"),
            (15, "CMOV"),
            (16, "PAT"),
            (23, "MMX"),
            (25, "SSE"),
            (26, "SSE2"),
        ] {
            assert_eq!(leaf1.edx & (1 << bit), 0, "{name} must not be declared");
        }
        // TSC-deadline (ECX bit 24) stays clear too.
        assert_eq!(leaf1.ecx & (1 << 24), 0);
    }

    #[test]
    fn stage4a_leaf_0_keeps_max_leaf_1_and_the_vendor() {
        let leaf0 = CpuidProfile::X8664V1Stage4A.query(0, 0);
        assert_eq!(leaf0.eax, 1);
        assert_eq!(leaf0.ebx, u32::from_le_bytes(*b"Open"));
        assert_eq!(leaf0.ecx, u32::from_le_bytes(*b"top "));
        assert_eq!(leaf0.edx, u32::from_le_bytes(*b"Desk"));
    }

    #[test]
    fn stage4a_unclaimed_leaves_are_zero_and_ecx_is_ignored() {
        let profile = CpuidProfile::X8664V1Stage4A;
        // 0x80000001 (SYSCALL/NX/LM of the target) is not declared by L9a: all zero.
        let zero_leaves = [
            2,
            3,
            4,
            7,
            0xB,
            0x0D,
            0x8000_0001,
            0x8000_0002,
            0x8000_0003,
            0x8000_0004,
            0x8000_0005,
            0x8000_0006,
            0x8000_0007,
            0x8000_0009,
            0x4000_0000,
            u32::MAX,
        ];
        for leaf in zero_leaves {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(
                    profile.query(leaf, subleaf),
                    CpuidResult::default(),
                    "leaf={leaf:#x}, ecx={subleaf:#x}"
                );
            }
        }
        // The two extended leaves of the table are the same as in DiagnosticV1.
        let max_ext = profile.query(0x8000_0000, 0);
        assert_eq!(max_ext.eax, 0x8000_0008);
        assert_eq!((max_ext.ebx, max_ext.ecx, max_ext.edx), (0, 0, 0));
        let widths = profile.query(0x8000_0008, 0);
        assert_eq!(widths.eax, 0x0000_3024);
        assert_eq!((widths.ebx, widths.ecx, widths.edx), (0, 0, 0));
        // ECX never changes any listed leaf.
        for leaf in [0, 1, 0x8000_0000, 0x8000_0008] {
            for subleaf in [1, 7, 0x1234_5678, u32::MAX] {
                assert_eq!(
                    profile.query(leaf, subleaf),
                    profile.query(leaf, 0),
                    "leaf={leaf:#x}, ecx={subleaf:#x}"
                );
            }
        }
    }

    /// The new profile differs from `DiagnosticV1` in leaf 1 EDX only.
    #[test]
    fn stage4a_differs_from_diagnostic_v1_in_leaf_1_edx_only() {
        for leaf in [0, 1, 2, 0x8000_0000, 0x8000_0001, 0x8000_0008, 0x8000_0009] {
            let old = CpuidProfile::DiagnosticV1.query(leaf, 0);
            let new = CpuidProfile::X8664V1Stage4A.query(leaf, 0);
            if leaf == 1 {
                assert_eq!((new.eax, new.ebx, new.ecx), (old.eax, old.ebx, old.ecx));
                assert_eq!((old.edx, new.edx), (0, 0x0100_0011));
            } else {
                assert_eq!(new, old, "leaf={leaf:#x}");
            }
        }
    }

    /// S4-38L8d. Oracle by hand: FPU (bit 0) | TSC (bit 4) | APIC (bit 9) | FXSR (bit 24).
    #[test]
    fn stage4a_apic_leaf_1_adds_only_the_apic_bit() {
        let leaf1 = CpuidProfile::X8664V1Stage4AApic.query(1, 0);
        assert_eq!(leaf1.eax, 0x0000_0600);
        assert_eq!(leaf1.ebx, 0x0000_0100, "initial APIC ID 0");
        assert_eq!(leaf1.ecx, 0);
        assert_eq!(leaf1.edx, 0x0100_0211);
        assert_eq!(
            leaf1.edx,
            CpuidProfile::X8664V1Stage4A.query(1, 0).edx | (1 << 9)
        );
        // x2APIC (ECX[21]), TSC-deadline (ECX[24]) and hypervisor (ECX[31]) stay clear.
        for bit in [21, 24, 31] {
            assert_eq!(leaf1.ecx & (1 << bit), 0, "ECX bit {bit}");
        }
    }

    #[test]
    fn stage4a_apic_zero_leaves_and_all_other_leaves_match_stage4a() {
        let apic = CpuidProfile::X8664V1Stage4AApic;
        let base = CpuidProfile::X8664V1Stage4A;
        for leaf in [6, 0xB, 0x15, 0x16] {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(
                    apic.query(leaf, subleaf),
                    CpuidResult::default(),
                    "leaf={leaf:#x}, ecx={subleaf:#x}"
                );
            }
        }
        for leaf in [
            0,
            2,
            3,
            4,
            7,
            0x0D,
            0x8000_0000,
            0x8000_0001,
            0x8000_0007,
            0x8000_0008,
            0x8000_0009,
            0x4000_0000,
            u32::MAX,
        ] {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(
                    apic.query(leaf, subleaf),
                    base.query(leaf, subleaf),
                    "leaf={leaf:#x}, ecx={subleaf:#x}"
                );
            }
        }
    }

    /// S6-14h. Oracles by hand from the contract table: leaf 1 EDX bits 0 (1), 3 (0x8), 4 (0x10),
    /// 5 (0x20), 6 (0x40), 8 (0x100), 9 (0x200), 13 (0x2000), 15 (0x8000), 16 (0x1_0000),
    /// 23 (0x80_0000), 24/25/26 (0x700_0000); 0x80000001 EDX bits 11 (0x800), 20 (0x10_0000),
    /// 29 (0x2000_0000).
    #[test]
    fn neutral_declares_the_full_target_table() {
        let p = CpuidProfile::OdX8664V1Neutral;
        let leaf0 = p.query(0, 0);
        assert_eq!(leaf0.eax, 1);
        assert_eq!(leaf0.ebx, 0x6E65_704F, "ASCII Open");
        assert_eq!(leaf0.ecx, 0x2070_6F74, "ASCII top ");
        assert_eq!(leaf0.edx, 0x6B73_6544, "ASCII Desk");
        let leaf1 = p.query(1, 0);
        assert_eq!(leaf1.eax, 0x0000_0600);
        assert_eq!(leaf1.ebx, 0x0000_0100);
        assert_eq!(leaf1.ecx, 0, "no XSAVE, OSXSAVE, AVX, hypervisor");
        let want = 0x1
            + 0x8
            + 0x10
            + 0x20
            + 0x40
            + 0x100
            + 0x200
            + 0x2000
            + 0x8000
            + 0x1_0000
            + 0x80_0000
            + 0x700_0000;
        assert_eq!(want, 0x0781_A379);
        assert_eq!(leaf1.edx, want);
        assert_eq!(leaf1.edx & (1 << 11), 0, "SEP stays clear");
        let ext = p.query(0x8000_0000, 0);
        assert_eq!(ext.eax, 0x8000_0008);
        assert_eq!((ext.ebx, ext.ecx, ext.edx), (0, 0, 0));
        let ext1 = p.query(0x8000_0001, 0);
        assert_eq!((ext1.eax, ext1.ebx, ext1.ecx), (0, 0, 0));
        assert_eq!(ext1.edx, 0x800 + 0x10_0000 + 0x2000_0000);
        assert_eq!(ext1.edx, 0x2010_0800);
        let widths = p.query(0x8000_0008, 0);
        assert_eq!(widths.eax, 0x0000_3024);
        assert_eq!((widths.ebx, widths.ecx, widths.edx), (0, 0, 0));
    }

    #[test]
    fn neutral_unlisted_leaves_are_zero_and_ecx_is_ignored() {
        let p = CpuidProfile::OdX8664V1Neutral;
        for leaf in [
            2,
            4,
            7,
            0xB,
            0x0D,
            0x8000_0002,
            0x8000_0007,
            0x8000_0009,
            0x4000_0000,
            u32::MAX,
        ] {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(p.query(leaf, subleaf), CpuidResult::default(), "{leaf:#x}");
            }
        }
        for leaf in [0, 1, 0x8000_0000, 0x8000_0001, 0x8000_0008] {
            for subleaf in [1, 7, u32::MAX] {
                assert_eq!(p.query(leaf, subleaf), p.query(leaf, 0), "{leaf:#x}");
            }
        }
    }

    /// S6-14h: the new profile is chosen only explicitly; the earlier profiles are unchanged and
    /// it is not a member of the Stage 4 family.
    #[test]
    fn neutral_is_explicit_and_leaves_earlier_profiles_unchanged() {
        assert_eq!(CpuidProfile::DiagnosticV1.query(1, 0).edx, 0);
        assert_eq!(
            CpuidProfile::DiagnosticV1.query(0x8000_0001, 0),
            CpuidResult::default()
        );
        assert_eq!(CpuidProfile::X8664V1Stage4A.query(1, 0).edx, 0x0100_0011);
        assert_eq!(
            CpuidProfile::X8664V1Stage4AApic.query(1, 0).edx,
            0x0100_0211
        );
        for p in [
            CpuidProfile::DiagnosticV1,
            CpuidProfile::X8664V1Stage4A,
            CpuidProfile::X8664V1Stage4AApic,
        ] {
            assert_eq!(p.query(0x8000_0001, 0), CpuidResult::default());
            assert_ne!(p, CpuidProfile::OdX8664V1Neutral);
        }
        assert!(!CpuidProfile::OdX8664V1Neutral.is_stage4a_family());
    }

    const ALL_PROFILES: [CpuidProfile; 5] = [
        CpuidProfile::DiagnosticV1,
        CpuidProfile::X8664V1Stage4A,
        CpuidProfile::X8664V1Stage4AApic,
        CpuidProfile::OdX8664V1Neutral,
        CpuidProfile::OdX8664V2Windows,
    ];

    /// S6-18a: for one vCPU the per-vCPU query is `query`, byte for byte, on every leaf (the
    /// contract's leaf 1 EBX `0x100`: one logical CPU, APIC ID 0), for every profile.
    #[test]
    fn s6_18a_one_vcpu_is_the_plain_query_byte_for_byte() {
        for p in ALL_PROFILES {
            for leaf in [
                0,
                1,
                2,
                4,
                0xB,
                0x1F,
                0x8000_0000,
                0x8000_0001,
                0x8000_0008,
                u32::MAX,
            ] {
                for subleaf in [0, 1] {
                    assert_eq!(
                        p.query_for_vcpu(leaf, subleaf, 0, 1),
                        p.query(leaf, subleaf)
                    );
                    // Count 0 is not a processor count; it is treated as one.
                    assert_eq!(
                        p.query_for_vcpu(leaf, subleaf, 0, 0),
                        p.query(leaf, subleaf)
                    );
                }
            }
            // The CLFLUSH line size field: the contract's 0x100 (size field 1) for the Linux
            // profiles, 8 (64 bytes) for the Windows profile, which declares CLFSH (S7-32 slice I).
            let ebx = if p == CpuidProfile::OdX8664V2Windows {
                0x0000_0800
            } else {
                0x0000_0100
            };
            assert_eq!(p.query_for_vcpu(1, 0, 0, 1).ebx, ebx);
        }
    }

    /// S6-18a: with N > 1, CPUID.1 EBX[23:16] is N and EBX[31:24] is the vCPU's APIC ID (oracle
    /// written by hand from the SDM field positions); everything else, including EAX, ECX, EDX
    /// and the topology leaves 0xB/0x1F/0x80000026 (absent), is as for one vCPU.
    #[test]
    fn s6_18a_two_and_four_vcpus_differ_in_leaf_1_ebx_only() {
        for p in ALL_PROFILES {
            for (count, apic_id) in [(2u32, 0u32), (2, 1), (4, 0), (4, 1), (4, 2), (4, 3)] {
                let one = p.query(1, 0);
                let got = p.query_for_vcpu(1, 0, apic_id, count);
                let line = if p == CpuidProfile::OdX8664V2Windows {
                    0x0000_0800
                } else {
                    0x0000_0100
                };
                assert_eq!(got.ebx, line | (count << 16) | (apic_id << 24));
                assert_eq!(got.ebx >> 24, apic_id, "initial APIC ID");
                assert_eq!((got.ebx >> 16) & 0xFF, count, "logical processor count");
                assert_eq!(got.ebx & 0xFFFF, one.ebx, "brand index and CLFLUSH size");
                assert_eq!((got.eax, got.ecx, got.edx), (one.eax, one.ecx, one.edx));
                assert_eq!(got.edx & (1 << 28), 0, "HTT is not declared");
                for leaf in [
                    0,
                    2,
                    4,
                    6,
                    0xB,
                    0x1F,
                    0x8000_0000,
                    0x8000_0001,
                    0x8000_0008,
                    0x8000_0026,
                ] {
                    // S7-32 slice O: the Windows profile's leaf 4 follows the topology too
                    // (`windows_leaf_4_follows_the_vcpu_topology`).
                    if p == CpuidProfile::OdX8664V2Windows && leaf == 4 {
                        continue;
                    }
                    assert_eq!(p.query_for_vcpu(leaf, 0, apic_id, count), p.query(leaf, 0));
                }
                for leaf in [0xB, 0x1F, 0x8000_0026] {
                    assert_eq!(
                        p.query_for_vcpu(leaf, 0, apic_id, count),
                        CpuidResult::default()
                    );
                }
            }
        }
    }

    /// S2-57. Oracles written by hand from the Intel SDM tables (Vol. 1, chapter "Processor
    /// identification and feature determination"): GenuineIntel in EBX, EDX, ECX order; the
    /// signature of family 6 model 0x9C stepping 0; ECX bits 0, 9, 13, 19, 20, 23 (0x982201);
    /// EDX the target table `0x0781A379` (S7-32 decision); 80000001H ECX bits 0 and 8 (0x101) and
    /// EDX bits 11, 20, 29 (0x20100800).
    #[test]
    fn windows_declares_the_identity_and_only_the_accepted_bits() {
        let p = CpuidProfile::OdX8664V2Windows;
        let leaf0 = p.query(0, 0);
        assert_eq!(
            leaf0.eax, 6,
            "maximum basic leaf (S7-32 slice O; 1 before it)"
        );
        assert_eq!(leaf0.ebx, 0x756E_6547, "ASCII Genu");
        assert_eq!(leaf0.edx, 0x4965_6E69, "ASCII ineI");
        assert_eq!(leaf0.ecx, 0x6C65_746E, "ASCII ntel");
        let leaf1 = p.query(1, 0);
        assert_eq!(leaf1.eax, 0x0009_06C0);
        // S7-32 slice I: EBX[15:8] = 8 (CLFLUSH line size 8 * 8 = 64 bytes), everything else 0.
        assert_eq!(leaf1.ebx, 0x0000_0800);
        assert_eq!(
            leaf1.ecx,
            0x1 + 0x200 + 0x2000 + 0x8_0000 + 0x10_0000 + 0x80_0000
        );
        assert_eq!(leaf1.ecx, 0x0098_2201);
        // FPU 0, DE 2, PSE 3, TSC 4, MSR 5, PAE 6, MCE 7, CX8 8, APIC 9, MTRR 12, PGE 13, MCA 14,
        // CMOV 15, PAT 16, CLFSH 19, MMX 23, FXSR 24, SSE 25, SSE2 26, by hand from the Intel SDM.
        assert_eq!(
            leaf1.edx,
            0x1 + 0x4
                + 0x8
                + 0x10
                + 0x20
                + 0x40
                + 0x80
                + 0x100
                + 0x200
                + 0x1000
                + 0x2000
                + 0x4000
                + 0x8000
                + 0x1_0000
                + 0x8_0000
                + 0x80_0000
                + 0x100_0000
                + 0x200_0000
                + 0x400_0000
        );
        assert_eq!(leaf1.edx, 0x0789_F3FD);
        let ext = p.query(0x8000_0000, 0);
        assert_eq!(ext.eax, 0x8000_0008);
        assert_eq!((ext.ebx, ext.ecx, ext.edx), (0, 0, 0));
        let ext1 = p.query(0x8000_0001, 0);
        assert_eq!((ext1.eax, ext1.ebx), (0, 0));
        assert_eq!(
            ext1.edx,
            0x800 + 0x10_0000 + 0x2000_0000,
            "SYSCALL, NX and LM"
        );
        assert_eq!(ext1.ecx, 0x1 + 0x100);
        let widths = p.query(0x8000_0008, 0);
        assert_eq!(widths.eax, 0x0000_3024);
        assert_eq!((widths.ebx, widths.ecx, widths.edx), (0, 0, 0));
    }

    #[test]
    fn windows_brand_string_is_the_celeron_n5105_text_nul_padded() {
        let p = CpuidProfile::OdX8664V2Windows;
        assert_eq!(p.query(0x8000_0002, 0).eax, 0x6574_6E49, "ASCII Inte");
        let mut bytes = Vec::new();
        for leaf in [0x8000_0002, 0x8000_0003, 0x8000_0004] {
            let r = p.query(leaf, 0);
            for word in [r.eax, r.ebx, r.ecx, r.edx] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        let mut expected = b"Intel(R) Celeron(R) N5105 @ 2.00GHz".to_vec();
        assert_eq!(expected.len(), 35);
        expected.resize(48, 0);
        assert_eq!(bytes, expected);
    }

    /// S2-57 negative: the bits that ADR-0013 forbids stay clear. Since the S7-32 decision of
    /// 2026-11-25 leaf 1 EDX and leaf 0x80000001 EDX contain the Linux target table; since slice I
    /// leaf 1 EDX adds DE, MCE, MTRR, MCA and CLFSH (the kernel's 0x0789F3FD); the other bits stay
    /// clear.
    #[test]
    fn windows_declares_the_target_edx_words_and_no_forbidden_bits() {
        let p = CpuidProfile::OdX8664V2Windows;
        let linux = CpuidProfile::OdX8664V1Neutral;
        let leaf1_edx = p.query(1, 0).edx;
        assert_eq!(leaf1_edx & linux.query(1, 0).edx, linux.query(1, 0).edx);
        assert_eq!(
            leaf1_edx & !linux.query(1, 0).edx,
            (1 << 2) | (1 << 7) | (1 << 12) | (1 << 14) | (1 << 19),
            "DE, MCE, MTRR, MCA, CLFSH (slice I)"
        );
        for (bit, name) in [(1, "VME"), (11, "SEP"), (17, "PSE-36"), (28, "HTT")] {
            assert_eq!(leaf1_edx & (1 << bit), 0, "{name}");
        }
        let ext1_edx = p.query(0x8000_0001, 0).edx;
        assert_eq!(ext1_edx, linux.query(0x8000_0001, 0).edx);
        assert_eq!(ext1_edx & (1 << 26), 0, "Page1GB");
        assert_eq!(ext1_edx & (1 << 27), 0, "RDTSCP");
        let ecx = p.query(1, 0).ecx;
        for (bit, name) in [
            (3, "MONITOR"),
            (5, "VMX"),
            (6, "SMX"),
            (12, "FMA"),
            (21, "x2APIC"),
            (24, "TSC-deadline"),
            (25, "AES"),
            (26, "XSAVE"),
            (27, "OSXSAVE"),
            (28, "AVX"),
            (31, "hypervisor"),
        ] {
            assert_eq!(ecx & (1 << bit), 0, "{name}");
        }
        assert_eq!(
            p.query(0x8000_0007, 0),
            CpuidResult::default(),
            "no invariant TSC"
        );
        assert_eq!(p.query(0x4000_0000, 0), CpuidResult::default());
        assert_eq!(p.query(0x8000_0001, 0).ecx & (1 << 5), 0, "no LZCNT");
    }

    #[test]
    fn windows_unlisted_leaves_are_zero_and_ecx_is_ignored() {
        let p = CpuidProfile::OdX8664V2Windows;
        for leaf in [
            3,
            5,
            7,
            0xB,
            0x0D,
            0x1F,
            0x8000_0005,
            0x8000_0006,
            0x8000_0007,
            0x8000_0009,
            0x8000_0026,
            0x4000_0000,
            u32::MAX,
        ] {
            for subleaf in [0, 1, u32::MAX] {
                assert_eq!(p.query(leaf, subleaf), CpuidResult::default(), "{leaf:#x}");
            }
        }
        for leaf in [
            0,
            1,
            2,
            6,
            0x8000_0000,
            0x8000_0001,
            0x8000_0002,
            0x8000_0003,
            0x8000_0004,
            0x8000_0008,
        ] {
            for subleaf in [1, 7, u32::MAX] {
                assert_eq!(p.query(leaf, subleaf), p.query(leaf, 0), "{leaf:#x}");
            }
        }
    }

    /// S7-32 slice O (ISSUE-703). Oracles by hand from SDM 325462-093US Vol. 1: Table 21-11 and
    /// 21-12 (leaf 2: AL = 01H, descriptor FFH "use CPUID leaf 4", null descriptors, bit 31 of
    /// each register 0), Table 21-13 (leaf 3: reserved, no serial number after the Pentium III),
    /// Table 21-15 (leaf 5: MONITOR/MWAIT, zero because leaf 1 ECX[3] is clear) and Table 21-16
    /// (leaf 6: EAX[2] ARAT only).
    #[test]
    fn windows_leaves_2_3_5_and_6_are_the_slice_o_values() {
        let p = CpuidProfile::OdX8664V2Windows;
        let two = p.query(2, 0);
        assert_eq!(
            two,
            CpuidResult {
                eax: 0x01 | (0xFF << 8),
                ebx: 0,
                ecx: 0,
                edx: 0
            }
        );
        for word in [two.eax, two.ebx, two.ecx, two.edx] {
            assert_eq!(
                word & (1 << 31),
                0,
                "every register holds valid descriptors"
            );
        }
        let descriptors: Vec<u8> = [two.eax, two.ebx, two.ecx, two.edx]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .skip(1)
            .filter(|&byte| byte != 0)
            .collect();
        assert_eq!(
            descriptors,
            [0xFF],
            "no FEH: leaf 0x18 is above the maximum"
        );
        assert_eq!(p.query(3, 0), CpuidResult::default());
        assert_eq!(p.query(1, 0).ecx & (1 << 3), 0, "MONITOR is not declared");
        assert_eq!(p.query(5, 0), CpuidResult::default());
        assert_eq!(
            p.query(6, 0),
            CpuidResult {
                eax: 1 << 2,
                ebx: 0,
                ecx: 0,
                edx: 0
            }
        );
    }

    /// S7-32 slice O. Oracles by hand from SDM Vol. 1 Table 21-14 (cache type 4:0, level 7:5,
    /// self-initialising 8, line size EBX[11:0] + 1, partitions EBX[21:12] + 1, ways EBX[31:22]
    /// + 1, sets ECX + 1; size = ways * partitions * line * sets) and the caches of the identity:
    /// L1D and L1I 32 KiB 8-way, L2 1.5 MiB 12-way, L3 4 MiB 16-way (inclusive, complex index),
    /// 64-byte lines everywhere (the CLFLUSH line of leaf 1). The enumeration ends with type 0
    /// at sub-leaf 4; an invalid sub-leaf returns four zeros.
    #[test]
    fn windows_leaf_4_enumerates_the_caches_of_the_identity_and_ends() {
        let p = CpuidProfile::OdX8664V2Windows;
        // (type, level, ways, sets, size in bytes, EDX)
        let expected = [
            (1u32, 1u32, 8u32, 64u32, 32 * 1024u32, 0u32),
            (2, 1, 8, 64, 32 * 1024, 0),
            (3, 2, 12, 2048, 1536 * 1024, 0),
            (3, 3, 16, 4096, 4 * 1024 * 1024, 0b110),
        ];
        for (subleaf, &(kind, level, ways, sets, size, edx)) in expected.iter().enumerate() {
            let r = p.query(4, subleaf as u32);
            assert_eq!(r.eax & 0x1F, kind, "sub-leaf {subleaf}");
            assert_eq!((r.eax >> 5) & 0x7, level, "sub-leaf {subleaf}");
            assert_eq!(r.eax & (1 << 8), 1 << 8, "self-initialising");
            assert_eq!(r.eax & (1 << 9), 0, "not fully associative");
            assert_eq!(r.eax >> 14, 0, "one logical processor, one core");
            assert_eq!((r.ebx & 0xFFF) + 1, 64, "line size");
            assert_eq!((r.ebx & 0xFFF) + 1, 8 * ((p.query(1, 0).ebx >> 8) & 0xFF));
            assert_eq!(((r.ebx >> 12) & 0x3FF) + 1, 1, "partitions");
            assert_eq!((r.ebx >> 22) + 1, ways, "sub-leaf {subleaf}");
            assert_eq!(r.ecx + 1, sets, "sub-leaf {subleaf}");
            assert_eq!(ways * 64 * sets, size, "sub-leaf {subleaf}");
            assert_eq!(r.edx, edx, "sub-leaf {subleaf}");
        }
        for subleaf in [4, 5, 0x100, u32::MAX] {
            assert_eq!(p.query(4, subleaf), CpuidResult::default(), "{subleaf:#x}");
        }
    }

    /// S7-32 slice O: the Windows profile's leaf 4 with N vCPUs (all cores of one package, no
    /// SMT, as leaf 1 EBX[23:16] = N): EAX[31:26] = N - 1 on every valid sub-leaf, EAX[25:14] =
    /// N - 1 on L2 and L3 and 0 on L1; the rest of the sub-leaf and the end of the enumeration
    /// do not change. Oracles by hand from SDM Vol. 1 Table 21-14. The counts above 4 (beyond
    /// the board's vCPU limit) only pin the cap of the 6-bit core field: no field spills.
    #[test]
    fn windows_leaf_4_follows_the_vcpu_topology() {
        let p = CpuidProfile::OdX8664V2Windows;
        for (count, apic_id) in [(2u32, 0u32), (2, 1), (4, 3), (64, 5), (65, 0), (255, 7)] {
            for subleaf in 0..4u32 {
                let one = p.query(4, subleaf);
                let got = p.query_for_vcpu(4, subleaf, apic_id, count);
                let cores = (count - 1).min(0x3F);
                let sharing = if subleaf >= 2 { count - 1 } else { 0 };
                assert_eq!(
                    got.eax,
                    one.eax | (cores << 26) | (sharing << 14),
                    "{count}"
                );
                assert_eq!((got.ebx, got.ecx, got.edx), (one.ebx, one.ecx, one.edx));
            }
            assert_eq!(
                p.query_for_vcpu(4, 4, apic_id, count),
                CpuidResult::default()
            );
        }
        // The hand-computed words for two vCPUs: L1D 0x04000121, L2 0x04004143.
        assert_eq!(p.query_for_vcpu(4, 0, 1, 2).eax, 0x0400_0121);
        assert_eq!(p.query_for_vcpu(4, 2, 1, 2).eax, 0x0400_4143);
        assert_eq!(p.query_for_vcpu(4, 3, 0, 4).eax, 0x0C00_C163);
        // No other profile has leaf 4.
        for other in ALL_PROFILES {
            if other != p {
                assert_eq!(other.query_for_vcpu(4, 0, 1, 2), CpuidResult::default());
            }
        }
    }

    /// S2-57: the Linux target and the earlier profiles are unchanged, and the Windows profile is
    /// explicit and not a member of the Stage 4 family.
    #[test]
    fn windows_is_explicit_and_leaves_the_other_profiles_unchanged() {
        assert_eq!(CpuidProfile::OdX8664V1Neutral.query(1, 0).edx, 0x0781_A379);
        assert_eq!(CpuidProfile::OdX8664V1Neutral.query(1, 0).ecx, 0);
        assert_eq!(CpuidProfile::OdX8664V1Neutral.query(1, 0).eax, 0x600);
        assert_eq!(
            CpuidProfile::OdX8664V1Neutral.query(0x8000_0001, 0).edx,
            0x2010_0800
        );
        assert_eq!(CpuidProfile::OdX8664V1Neutral.query(0x8000_0001, 0).ecx, 0);
        assert_eq!(
            CpuidProfile::OdX8664V1Neutral.query(0x8000_0002, 0),
            CpuidResult::default()
        );
        assert_eq!(CpuidProfile::X8664V1Stage4A.query(1, 0).edx, 0x0100_0011);
        assert_eq!(
            CpuidProfile::X8664V1Stage4AApic.query(1, 0).edx,
            0x0100_0211
        );
        assert_eq!(CpuidProfile::DiagnosticV1.query(1, 0).ecx, 0);
        for p in [
            CpuidProfile::DiagnosticV1,
            CpuidProfile::X8664V1Stage4A,
            CpuidProfile::X8664V1Stage4AApic,
            CpuidProfile::OdX8664V1Neutral,
        ] {
            assert_ne!(p, CpuidProfile::OdX8664V2Windows);
            assert_eq!(p.query(0x8000_0002, 0), CpuidResult::default());
        }
        assert!(!CpuidProfile::OdX8664V2Windows.is_stage4a_family());
    }
}
