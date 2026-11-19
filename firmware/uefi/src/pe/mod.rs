//! Parsing and loading of PE32+ images for the x64 UEFI profile.

mod parser;

pub use parser::{Error, PeImage, Section, Subsystem};
