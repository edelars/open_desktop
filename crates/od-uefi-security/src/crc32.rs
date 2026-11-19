//! Shared IEEE checksum for NVRAM and other machine wire formats.

/// CRC-32 IEEE 802.3 over contiguous parts, with reflected input and output.
pub fn crc32_ieee(parts: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 != 0 {
                    (value >> 1) ^ 0xEDB8_8320
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &byte in *part {
            crc = TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
        }
    }
    !crc
}

pub(crate) fn with_zeroed_word(bytes: &[u8], offset: usize) -> u32 {
    crc32_ieee(&[&bytes[..offset], &[0; 4], &bytes[offset + 4..]])
}
