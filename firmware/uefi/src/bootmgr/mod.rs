//! UEFI boot options and their selection on the firmware board.

pub mod options;

#[cfg(target_os = "none")]
mod loader;
#[cfg(target_os = "none")]
mod policy;

#[cfg(target_os = "none")]
pub use policy::{run, storage_present};

#[cfg(target_os = "none")]
const fn char16<const N: usize>(ascii: &[u8; N]) -> [u16; N] {
    let mut units = [0; N];
    let mut offset = 0;
    while offset < N {
        units[offset] = ascii[offset] as u16;
        offset += 1;
    }
    units
}
