/// Explicit admission for the bounded Long64 non-PVI CLI transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliProfile {
    /// Long64, CR0.PE=1, RFLAGS.VM=0, CR4.PVI=0, without VMX or SMM.
    NonPviLong64V1,
}
