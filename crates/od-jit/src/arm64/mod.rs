//! Checked ARM64 code-buffer encoding used by the existing fragment translator.

mod encoder;

pub use encoder::{
    AddressBase, Condition, EncodeError, EncodedCode, Encoder, Label, PairIndexMode, WReg, XReg,
    XRegOrSp, XRegOrZero,
};
