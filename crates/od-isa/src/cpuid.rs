//! Caller-selected CPUID profiles supported by the local reference interpreter.
//!
//! `DiagnosticV1` is a deliberately non-bootable diagnostic identity. Its
//! feature masks are zero, so it is not an `x86-64-v1` CPU and is not an
//! Ubuntu boot contract.
//!
//! `X8664V1Stage4A` (S4-38L9a) is the first named subset of the Linux board target
//! `od-x86-64-v1-neutral-v0.1` (`docs/specs/CPU_CONTRACT.md`): it declares only the features whose
//! behaviour the board's CPU set already implements (TSC, through `RDTSC`; since S4-38L9d FPU and
//! FXSR, through `FNINIT` and the plain `FXSAVE` of the set's FP member). It is not the full
//! target table and not an Ubuntu boot contract.

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
}

/// CPUID.01H:EDX bit 4 (TSC); the bit position is from the Intel SDM, from memory.
const LEAF1_EDX_TSC: u32 = 1 << 4;
/// CPUID.01H:EDX bit 0 (FPU), declared since S4-38L9d: `FNINIT` is implemented in the CPL0 set's FP
/// member (the kernel's `fpu__init_system` gives up without this bit). From the Intel SDM, from memory.
const LEAF1_EDX_FPU: u32 = 1 << 0;
/// CPUID.01H:EDX bit 24 (FXSR), declared since S4-38L9d: the plain `FXSAVE` is implemented in the
/// same member. SSE (25) and SSE2 (26) are NOT declared: no SSE instruction is implemented. From the
/// Intel SDM, from memory.
const LEAF1_EDX_FXSR: u32 = 1 << 24;

impl CpuidProfile {
    /// Return a CPUID tuple. This initial slice ignores ECX for every leaf.
    pub const fn query(self, leaf: u32, _subleaf: u32) -> CpuidResult {
        let leaf1_edx = match self {
            Self::DiagnosticV1 => 0,
            Self::X8664V1Stage4A => LEAF1_EDX_FPU | LEAF1_EDX_TSC | LEAF1_EDX_FXSR,
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
}
