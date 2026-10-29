//! Caller-selected local policy for a synthetic target with SEP disabled.

/// Immutable CPU policy for the selected `SEP=0` target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SepProfile {
    /// Exact bare Long64 SYSENTER/SYSEXIT forms raise CPU-side `#UD`.
    SepDisabledV1,
}
