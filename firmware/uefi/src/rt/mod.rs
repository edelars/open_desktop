//! Implementations of EFI Runtime Services entry points.

#[cfg(target_os = "uefi")]
pub mod driver;
pub mod reset;
pub mod time;
pub mod variable;
pub mod virt;
