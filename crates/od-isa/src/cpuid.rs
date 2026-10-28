//! Caller-selected CPUID profiles supported by the local reference interpreter.
//!
//! `DiagnosticV1` is a deliberately non-bootable diagnostic identity. Its
//! feature masks are zero, so it is not an `x86-64-v1` CPU and is not an
//! Ubuntu boot contract.

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
}

impl CpuidProfile {
    /// Return a CPUID tuple. This initial slice ignores ECX for every leaf.
    pub const fn query(self, leaf: u32, _subleaf: u32) -> CpuidResult {
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
                edx: 0,
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
}
