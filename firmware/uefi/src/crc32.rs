//! Reflected IEEE CRC-32 shared by firmware wire formats and UEFI tables.

pub fn crc32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(u32::MAX, |state, &byte| update_byte(state, byte))
        ^ u32::MAX
}

/// Counts the bytes at the four-byte field offset as zero.
pub(crate) const fn with_zeroed_word(bytes: &[u8], offset: usize) -> u32 {
    let prefix_len = if offset < bytes.len() {
        offset
    } else {
        bytes.len()
    };
    let (prefix, rest) = bytes.split_at(prefix_len);
    let word_len = if rest.len() < 4 { rest.len() } else { 4 };
    let (_, suffix) = rest.split_at(word_len);
    let mut state = update_bytes(u32::MAX, prefix);
    let mut remaining = word_len;
    while remaining != 0 {
        state = update_byte(state, 0);
        remaining -= 1;
    }
    update_bytes(state, suffix) ^ u32::MAX
}

const fn update_bytes(mut state: u32, bytes: &[u8]) -> u32 {
    let mut index = 0;
    while index < bytes.len() {
        state = update_byte(state, bytes[index]);
        index += 1;
    }
    state
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_word_matches_zeroed_input_even_at_a_short_tail() {
        let input = *b"123456789";
        for offset in [0, 2, 5, 7, 8, 9, usize::MAX] {
            let mut expected = input;
            if let Some(remaining) = expected.get_mut(offset..) {
                let word_len = remaining.len().min(4);
                remaining[..word_len].fill(0);
            }
            assert_eq!(with_zeroed_word(&input, offset), crc32(&expected));
        }
    }
}
