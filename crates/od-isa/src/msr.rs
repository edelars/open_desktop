//! Caller-selected local MSR probe policies for the reference interpreter.
//!
//! `SysenterProbeV1` models only the synthetic fault decisions required for
//! Linux's safe SYSENTER MSR probes. It is not general MSR support.

/// Immutable MSR access policy explicitly selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsrProfile {
    /// Fault RDMSR/WRMSR to IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`).
    SysenterProbeV1,
}

impl MsrProfile {
    /// Whether this profile models a fault decision for `index`.
    pub const fn handles_index(self, index: u32) -> bool {
        match self {
            Self::SysenterProbeV1 => matches!(index, 0x174..=0x176),
        }
    }
}
