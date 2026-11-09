//! ACPI tables built from the handoff block (S7-12: AML encoder and DSDT; the other tables
//! are S7-13). Pure logic, testable on the host.

// Declared first: the `tri!` macro it exports is used by the modules below.
mod bytes;

pub mod aml;
pub mod blob;
pub mod dsdt;
pub mod fadt;
pub mod madt;
pub mod manifest;
pub mod mcfg;
pub mod sha256;
pub mod table;
