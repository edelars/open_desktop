//! ACPI serialization and publication for the handoff-described board.

// Declared first: the `tri!` macro it exports is used by the modules below.
mod bytes;

pub mod aml;
pub mod blob;
pub mod dsdt;
pub mod fadt;
pub mod install;
pub mod madt;
pub mod manifest;
pub mod mcfg;
pub mod rsdp;
pub mod sha256;
pub mod table;
