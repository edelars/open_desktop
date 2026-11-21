//! Parsing and loading of PE32+ images for the x64 UEFI profile.

#[cfg(test)]
mod authenticode_tests;
mod loader;
mod pages;
#[cfg(test)]
mod parser_tests;
mod reloc;
#[cfg(test)]
mod test_image;

pub use loader::{LoadError, MappedImage, load_into};
pub(crate) use loader::{capture_runtime_fixups, is_relocatable, runtime_fixup_count};
pub use od_uefi_security::pe::{
    DigestAlgorithm, DigestError, Error, PeImage, Section, Subsystem, authenticode_digest,
};
pub(crate) use pages::{MAX_REGIONS, PageLayout};
pub use reloc::{RelocationError, RuntimeFixup, for_each_dir64};
