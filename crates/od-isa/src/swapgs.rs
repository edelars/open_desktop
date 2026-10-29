/// Explicit admission for the bounded Long64 SWAPGS state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapgsProfile {
    /// Long64, CPL0/3 execution, FRED disabled, canonical48 GS bases.
    SwapgsV1,
}
