//! Parsing and loading of PE32+ images for the x64 UEFI profile.

mod authenticode;
mod loader;
mod parser;
mod reloc;
#[cfg(test)]
mod test_image;

pub use authenticode::{DigestAlgorithm, DigestError, authenticode_digest};
pub use loader::{LoadError, MappedImage, load_into};
pub use parser::{Error, PeImage, Section, Subsystem};
pub use reloc::RelocationError;
