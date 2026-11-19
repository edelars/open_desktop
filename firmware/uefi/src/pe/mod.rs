//! Parsing and loading of PE32+ images for the x64 UEFI profile.

mod authenticode;
mod parser;
#[cfg(test)]
mod test_image;

pub use authenticode::{DigestAlgorithm, DigestError, authenticode_digest};
pub use parser::{Error, PeImage, Section, Subsystem};
