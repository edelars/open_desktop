//! Reflected IEEE CRC-32 shared by firmware wire formats and UEFI tables.

pub fn crc32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(u32::MAX, |state, &byte| update_byte(state, byte))
        ^ u32::MAX
}

/// Counts the bytes at the four-byte field offset as zero.
pub(crate) const fn with_zeroed_word(bytes: &[u8], offset: usize) -> u32 {
    let mut state = u32::MAX;
    let mut index = 0;
    while index < bytes.len() {
        let byte = if index >= offset && index - offset < 4 {
            0
        } else {
            bytes[index]
        };
        state = update_byte(state, byte);
        index += 1;
    }
    state ^ u32::MAX
}

#[inline(always)]
const fn update_byte(mut state: u32, byte: u8) -> u32 {
    state ^= byte as u32;
    let mut bit = 0;
    while bit < 8 {
        state = if state & 1 != 0 {
            (state >> 1) ^ 0xedb8_8320
        } else {
            state >> 1
        };
        bit += 1;
    }
    state
}
