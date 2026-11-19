//! PE32+ base relocation blocks, read from the immutable source file.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationError {
    MisalignedBlock,
    InvalidBlockSize,
    InvalidPageRva,
    UnsupportedType(u16),
    TargetOutsideImage,
}

pub(super) fn for_each_dir64(
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
