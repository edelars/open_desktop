//! Checked ARM64 code-buffer encoding used by the existing fragment translator.

mod encoder;
pub mod memory_order;

pub use encoder::{
    AccessWidth, AddressBase, AtomicOrder, BarrierOption, Condition, EncodeError, EncodedCode,
    Encoder, Label, PairIndexMode, WReg, XReg, XRegOrSp, XRegOrZero,
};
