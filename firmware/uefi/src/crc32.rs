//! Reflected IEEE CRC-32 shared by firmware wire formats and UEFI tables.

pub fn crc32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(u32::MAX, |state, &byte| update_byte(state, byte))
        ^ u32::MAX
}

/// Counts the bytes at the four-byte field offset as zero.
pub(crate) fn with_zeroed_word(bytes: &[u8], offset: usize) -> u32 {
    let mut state = u32::MAX;
    for (index, &byte) in bytes.iter().enumerate() {
        let byte = if (offset..offset + 4).contains(&index) {
            0
        } else {
            byte
        };
        state = update_byte(state, byte);
    }
    state ^ u32::MAX
}

#[inline(always)]
fn update_byte(mut state: u32, byte: u8) -> u32 {
    state ^= u32::from(byte);
    for _ in 0..8 {
        state = if state & 1 != 0 {
            (state >> 1) ^ 0xedb8_8320
        } else {
            state >> 1
        };
    }
    state
}
