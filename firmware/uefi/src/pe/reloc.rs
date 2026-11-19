//! PE32+ base relocation blocks, read from the immutable source file.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationError {
    MisalignedBlock,
    InvalidBlockSize,
    InvalidPageRva,
    UnsupportedType(u16),
    TargetOutsideImage,
    OverlappingTargets,
}

/// An immutable fixup record captured after the initial physical relocation.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RuntimeFixup {
    rva: usize,
    physical_value: u64,
}

impl RuntimeFixup {
    pub(crate) fn capture(rva: usize, mapped: &[u8]) -> Self {
        Self {
            rva,
            physical_value: u64::from_le_bytes(mapped[rva..rva + 8].try_into().unwrap()),
        }
    }

    pub(crate) fn validate_targets(records: &mut [Self]) -> Result<(), RelocationError> {
        records.sort_unstable_by_key(|fixup| fixup.rva);
        if records.windows(2).any(|pair| pair[0].rva + 8 > pair[1].rva) {
            return Err(RelocationError::OverlappingTargets);
        }
        Ok(())
    }

    /// # Safety
    /// The image is live at its physical base and this fixup target permits
    /// reads and writes. Caller serializes the transition on the owning CPU.
    pub unsafe fn apply(self, physical_base: usize, delta: u64) {
        let word = core::ptr::with_exposed_provenance_mut::<u64>(physical_base + self.rva);
        // SAFETY: PE permits unaligned DIR64 targets, validated at load time.
        unsafe {
            if word.read_unaligned() == self.physical_value {
                word.write_unaligned(self.physical_value.wrapping_add(delta));
            }
        }
    }
}

/// Visits DIR64 target RVAs in a PE32+ relocation directory.
pub fn for_each_dir64(
    directory_rva: u32,
    data: &[u8],
    image_size: u32,
    mut visit: impl FnMut(usize),
) -> Result<(), RelocationError> {
    const BLOCK_HEADER_BYTES: usize = 8;
    const PAGE_MASK: u32 = 0xfff;
    const ABSOLUTE: u16 = 0;
    const DIR64: u16 = 10;

    let mut offset = 0;
    while offset < data.len() {
        let block_rva = u64::from(directory_rva) + offset as u64;
        if block_rva & 3 != 0 {
            return Err(RelocationError::MisalignedBlock);
        }
        let remaining = &data[offset..];
        let header = remaining
            .first_chunk::<BLOCK_HEADER_BYTES>()
            .ok_or(RelocationError::InvalidBlockSize)?;
        let page_rva = u32::from_le_bytes(header[..4].try_into().unwrap());
        let size = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if size < BLOCK_HEADER_BYTES || size & 1 != 0 || size > remaining.len() {
            return Err(RelocationError::InvalidBlockSize);
        }
        if page_rva & PAGE_MASK != 0 || page_rva >= image_size {
            return Err(RelocationError::InvalidPageRva);
        }
        let (entries, _) = remaining[BLOCK_HEADER_BYTES..size].as_chunks::<2>();
        for entry in entries {
            let entry = u16::from_le_bytes(*entry);
            match entry >> 12 {
                ABSOLUTE => {}
                DIR64 => {
                    let target = page_rva
                        .checked_add(u32::from(entry) & PAGE_MASK)
                        .filter(|target| u64::from(*target) + 8 <= u64::from(image_size))
                        .ok_or(RelocationError::TargetOutsideImage)?;
                    visit(target as usize);
                }
                kind => return Err(RelocationError::UnsupportedType(kind)),
            }
        }
        offset += size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_relocation_only_patches_unchanged_words() {
        let mut image = [0; 24];
        image[1..9].copy_from_slice(&0x1000u64.to_le_bytes());
        image[12..20].copy_from_slice(&0x2000u64.to_le_bytes());
        let mut records = [
            RuntimeFixup::capture(12, &image),
            RuntimeFixup::capture(1, &image),
        ];
        RuntimeFixup::validate_targets(&mut records).unwrap();
        image[12..20].copy_from_slice(&0x1234u64.to_le_bytes());
        let base = image.as_mut_ptr().addr();
        for record in records {
            // SAFETY: both target words are inside the exclusively writable image.
            unsafe { record.apply(base, 0x8000) };
        }
        assert_eq!(u64::from_le_bytes(image[1..9].try_into().unwrap()), 0x9000);
        assert_eq!(
            u64::from_le_bytes(image[12..20].try_into().unwrap()),
            0x1234
        );
        let same = RuntimeFixup::capture(1, &image);
        assert_eq!(
            RuntimeFixup::validate_targets(&mut [same, same]),
            Err(RelocationError::OverlappingTargets)
        );
    }
}
