//! Checked ARM64 code-buffer encoding used by the existing fragment translator.

pub mod atomic_sequence;
mod encoder;
pub mod memory_order;

pub use encoder::{
    AccessWidth, AddressBase, AtomicOrder, BarrierOption, Condition, EncodeError, EncodedCode,
    Encoder, Label, PairIndexMode, Shift, WReg, XReg, XRegOrSp, XRegOrZero,
};
