//! Caller-selected local MSR probe policies for the reference interpreter.
//!
//! `SysenterProbeV1` models a fault-only safe SYSENTER probe, while the FS/GS
//! profiles model one explicit base read/write each. None enables general MSR
//! support.

/// Immutable MSR access policy explicitly selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsrProfile {
    /// Fault RDMSR/WRMSR to IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`).
    SysenterProbeV1,
    /// CPL0-only access to IA32_GS_BASE (`0xc000_0101`).
    GsBaseV1,
    /// CPL0-only access to IA32_FS_BASE (`0xc000_0100`).
    FsBaseV1,
    /// CPL0-only access to IA32_KERNEL_GS_BASE (`0xc000_0102`).
    KernelGsBaseV1,
}

impl MsrProfile {
    /// Whether this profile handles `index`.
    pub const fn handles_index(self, index: u32) -> bool {
        match self {
            Self::SysenterProbeV1 => matches!(index, 0x174..=0x176),
            Self::GsBaseV1 => index == 0xc000_0101,
            Self::FsBaseV1 => index == 0xc000_0100,
            Self::KernelGsBaseV1 => index == 0xc000_0102,
        }
    }
}
