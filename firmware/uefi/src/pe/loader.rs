//! Materialize a parsed image in caller-provided memory.

use super::{
    PeImage, Subsystem,
    reloc::{RelocationError, RuntimeFixup, for_each_dir64},
};
use crate::{bs::misc::MEMORY_CHUNK, paging::PAGE_BYTES};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    DestinationTooSmall,
    InvalidAddress,
    RelocationsRequired,
    ConflictingRelocationFlag,
    Relocation(RelocationError),
}

/// Whether an image may be placed at an address other than its ImageBase.
///
/// An image with a base relocation directory can always move. An image without
/// one moves only when it declares itself position independent: ImageBase 0
/// (the Linux EFI stub layout) and RELOCS_STRIPPED clear. Such an image needs no
/// fixups at any aligned address. RELOCS_STRIPPED, or a non-zero ImageBase
/// without a directory, pins the image to its ImageBase.
pub(crate) fn is_relocatable(image: &PeImage<'_>) -> bool {
    image.relocation_data().is_some() || (image.image_base() == 0 && !image.relocations_stripped())
}

pub(crate) fn runtime_fixup_count(image: &PeImage<'_>) -> Result<usize, LoadError> {
    let (rva, data) = image
        .relocation_data()
        .ok_or(LoadError::RelocationsRequired)?;
    let mut count = 0;
    for_each_dir64(rva, data, image.image_size(), |_| count += 1).map_err(LoadError::Relocation)?;
    Ok(count)
}

pub(crate) fn capture_runtime_fixups(
    image: &PeImage<'_>,
    mapped: &[u8],
    records: &mut [RuntimeFixup],
) -> Result<(), LoadError> {
    let (rva, data) = image
        .relocation_data()
        .ok_or(LoadError::RelocationsRequired)?;
    let mut output = records.iter_mut();
    for_each_dir64(rva, data, image.image_size(), |target| {
        *output.next().expect("preflighted fixup count") = RuntimeFixup::capture(target, mapped);
    })
    .map_err(LoadError::Relocation)?;
    RuntimeFixup::validate_targets(records).map_err(LoadError::Relocation)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappedImage {
    pub base: u64,
    pub size: u32,
    pub entry_address: u64,
    pub subsystem: Subsystem,
}

/// Copy an image, including headers and raw section padding, and apply DIR64
/// relocations. An error leaves the entire destination intact.
pub fn load_into(
    image: &PeImage<'_>,
    actual_base: u64,
    destination: &mut [u8],
) -> Result<MappedImage, LoadError> {
    let image_size = image.image_size();
    let mapped = destination
        .get_mut(..image_size as usize)
        .ok_or(LoadError::DestinationTooSmall)?;
    let alignment = u64::from(image.section_alignment()).max(PAGE_BYTES);
    if actual_base & (alignment - 1) != 0
        || actual_base.checked_add(u64::from(image_size)).is_none()
    {
        return Err(LoadError::InvalidAddress);
    }
    let relocation_data = image.relocation_data();
    let delta = actual_base.wrapping_sub(image.image_base());
    if image.relocations_stripped() && relocation_data.is_some() {
        return Err(LoadError::ConflictingRelocationFlag);
    }
    if delta != 0 && !is_relocatable(image) {
        return Err(LoadError::RelocationsRequired);
    }
    if let Some((rva, bytes)) = relocation_data {
        for_each_dir64(rva, bytes, image_size, |_| {}).map_err(LoadError::Relocation)?;
    }

    for chunk in mapped.chunks_mut(MEMORY_CHUNK) {
        chunk.fill(0);
    }
    copy_image_bytes(mapped, 0, image.headers());
    for section in image.sections() {
        copy_image_bytes(mapped, section.virtual_address as usize, section.file_data);
    }
    if let Some((rva, bytes)) = relocation_data
        && delta != 0
    {
        // The immutable relocation data was validated before the first write.
        for_each_dir64(rva, bytes, image_size, |target| {
            let field = &mut mapped[target..target + 8];
            let original = u64::from_le_bytes(field.try_into().unwrap());
            field.copy_from_slice(&original.wrapping_add(delta).to_le_bytes());
        })
        .expect("validated immutable relocation table");
    }

    Ok(MappedImage {
        base: actual_base,
        size: image_size,
        entry_address: actual_base + u64::from(image.entry_rva()),
        subsystem: image.subsystem(),
    })
}

fn copy_image_bytes(image: &mut [u8], offset: usize, bytes: &[u8]) {
    let region = &mut image[offset..offset + bytes.len()];
    for (target, source) in region
        .chunks_mut(MEMORY_CHUNK)
        .zip(bytes.chunks(MEMORY_CHUNK))
    {
        target.copy_from_slice(source);
    }
}

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;
