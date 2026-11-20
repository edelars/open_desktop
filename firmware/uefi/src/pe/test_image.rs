//! Minimal PE32+ file built directly from the on-disk layout.

pub(super) const PE_START: usize = 0x80;
pub(super) const OPTIONAL_START: usize = PE_START + 24;
pub(super) const OPTIONAL_PREFIX_BYTES: usize = 112;
pub(super) const DIRECTORY_BYTES: usize = 8;
pub(super) const SECTION_HEADER_BYTES: usize = 40;
pub(super) const OPTIONAL_BYTES: usize = OPTIONAL_PREFIX_BYTES + 16 * DIRECTORY_BYTES;
pub(super) const SECTIONS: usize = OPTIONAL_START + OPTIONAL_BYTES;
pub(super) const RELOCATIONS: usize = OPTIONAL_START + OPTIONAL_PREFIX_BYTES + 5 * DIRECTORY_BYTES;
pub(super) const SECURITY: usize = OPTIONAL_START + OPTIONAL_PREFIX_BYTES + 4 * DIRECTORY_BYTES;

pub(super) fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..][..2].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..][..4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn fixture(subsystem: u16) -> Vec<u8> {
    let mut file = vec![0u8; 0x600];
    file[..2].copy_from_slice(b"MZ");
    put_u32(&mut file, 0x3c, PE_START as u32);
    file[PE_START..][..4].copy_from_slice(b"PE\0\0");
    put_u16(&mut file, PE_START + 4, 0x8664);
    put_u16(&mut file, PE_START + 6, 3);
    put_u16(&mut file, PE_START + 20, OPTIONAL_BYTES as u16);
    put_u16(&mut file, PE_START + 22, 0x0002);
    put_u16(&mut file, OPTIONAL_START, 0x020b);
    put_u32(&mut file, OPTIONAL_START + 16, 0x1000);
    file[OPTIONAL_START + 24..][..8].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    put_u32(&mut file, OPTIONAL_START + 32, 0x1000);
    put_u32(&mut file, OPTIONAL_START + 36, 0x200);
    put_u32(&mut file, OPTIONAL_START + 56, 0x4000);
    put_u32(&mut file, OPTIONAL_START + 60, 0x200);
    put_u16(&mut file, OPTIONAL_START + 68, subsystem);
    put_u32(&mut file, OPTIONAL_START + 108, 16);
    put_u32(&mut file, RELOCATIONS, 0x2000);
    put_u32(&mut file, RELOCATIONS + 4, 0x18);
    for (index, (virtual_size, rva, raw_size, raw_offset, flags)) in [
        (0x180, 0x1000, 0x200, 0x200, 0x6000_0020),
        (0x18, 0x2000, 0x200, 0x400, 0x4200_0040),
        (0x5d0, 0x3000, 0, 0, 0xc000_0080),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = SECTIONS + index * 40;
        put_u32(&mut file, offset + 8, virtual_size);
        put_u32(&mut file, offset + 12, rva);
        put_u32(&mut file, offset + 16, raw_size);
        put_u32(&mut file, offset + 20, raw_offset);
        put_u32(&mut file, offset + 36, flags);
    }
    file[0x200] = 0xc3;
    file
}
