//! Pure logic of the Open Desktop firmware (S7-01 skeleton).
//!
//! Rule: everything in this library is testable on the host with `cargo test` and does not
//! depend on MMIO, port I/O or the firmware allocator. Code that needs them lives in the binary
//! (`main.rs`) and is checked at level 3 of `docs/testing/FIRMWARE.md` (freestanding run in the
//! machine). `std` is available only under `cfg(test)`.

#![cfg_attr(not(test), no_std)]

// `alloc` is deliberately not linked yet: an `extern crate alloc` without a `#[global_allocator]`
// does not link. S7-04 adds the allocator together with `extern crate alloc`.

pub mod acpi;
pub mod handoff;
pub mod image;
pub mod paging;
pub mod stop;

/// GPA of the handoff block passed in `RDI` at firmware entry (FIRMWARE_BOOT.md, section 5).
pub const HANDOFF_GPA: u64 = 0x0010_0000;
