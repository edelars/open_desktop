//! FAT names are compared with Unicode 17 simple uppercase mappings.

pub(crate) const MAX_NAME: usize = 255;

#[derive(Clone)]
pub(crate) struct Name {
    units: [u16; MAX_NAME],
    len: u8,
}

impl Name {
    pub(super) fn from_units(units: &[u16]) -> Self {
        let mut data = [0; MAX_NAME];
        data[..units.len()].copy_from_slice(units);
        Self {
            units: data,
            len: units.len() as u8,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u16] {
        &self.units[..usize::from(self.len)]
    }

    pub(super) fn label(bytes: &[u8]) -> Self {
        let mut label = [0; 11];
        let len = trim_field(bytes).len();
        for (dest, &source) in label.iter_mut().zip(&bytes[..len]) {
            *dest = oem_unit(source);
        }
        Self::from_units(&label[..len])
    }
}

#[derive(Clone)]
pub(super) struct ShortName {
    units: [u16; 12],
    len: u8,
}

impl ShortName {
    pub(super) fn parse(bytes: &[u8; 11], case: u8) -> Self {
        let mut units = [0; 12];
        let base = trim_field(&bytes[..8]);
        let extension = trim_field(&bytes[8..]);
        for (index, &byte) in base.iter().enumerate() {
            let byte = if index == 0 && byte == 5 { 0xe5 } else { byte };
            units[index] = oem_unit(if case & 8 != 0 {
                byte.to_ascii_lowercase()
            } else {
                byte
            });
        }
        let mut len = base.len();
        if !extension.is_empty() {
            units[len] = b'.' as u16;
            len += 1;
            for &byte in extension {
                units[len] = oem_unit(if case & 16 != 0 {
                    byte.to_ascii_lowercase()
                } else {
                    byte
                });
                len += 1;
            }
        }
        Self {
            units,
            len: len as u8,
        }
    }

    pub(super) fn as_slice(&self) -> &[u16] {
        &self.units[..usize::from(self.len)]
    }
}

fn trim_field(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != b' ')
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

pub(super) fn equivalent(left: &[u16], right: &[u16]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(&a, &b)| simple_upper(a) == simple_upper(b))
}

fn simple_upper(unit: u16) -> u16 {
    // UCD 17 Simple_Uppercase_Mapping has these nonidentity BMP cases when
    // the full uppercase mapping used by char::to_uppercase expands.
    match unit {
        0x1f80..=0x1f87 | 0x1f90..=0x1f97 | 0x1fa0..=0x1fa7 => unit + 8,
        0x1fb3 | 0x1fc3 | 0x1ff3 => unit + 9,
        _ => {
            let Some(character) = char::from_u32(u32::from(unit)) else {
                return unit;
            };
            let mut upper = character.to_uppercase();
            let first = upper.next().unwrap() as u32;
            if upper.next().is_none() && first <= u32::from(u16::MAX) {
                first as u16
            } else {
                unit
            }
        }
    }
}

fn oem_unit(byte: u8) -> u16 {
    if byte < 128 {
        u16::from(byte)
    } else {
        CP437[usize::from(byte - 128)]
    }
}

// IBM Code Page 437, high half, in byte order. The low half uses ASCII.
const CP437: [u16; 128] = [
    0x00c7, 0x00fc, 0x00e9, 0x00e2, 0x00e4, 0x00e0, 0x00e5, 0x00e7, 0x00ea, 0x00eb, 0x00e8, 0x00ef,
    0x00ee, 0x00ec, 0x00c4, 0x00c5, 0x00c9, 0x00e6, 0x00c6, 0x00f4, 0x00f6, 0x00f2, 0x00fb, 0x00f9,
    0x00ff, 0x00d6, 0x00dc, 0x00a2, 0x00a3, 0x00a5, 0x20a7, 0x0192, 0x00e1, 0x00ed, 0x00f3, 0x00fa,
    0x00f1, 0x00d1, 0x00aa, 0x00ba, 0x00bf, 0x2310, 0x00ac, 0x00bd, 0x00bc, 0x00a1, 0x00ab, 0x00bb,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255d, 0x255c, 0x255b, 0x2510, 0x2514, 0x2534, 0x252c, 0x251c, 0x2500, 0x253c, 0x255e, 0x255f,
    0x255a, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256c, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256b, 0x256a, 0x2518, 0x250c, 0x2588, 0x2584, 0x258c, 0x2590, 0x2580,
    0x03b1, 0x00df, 0x0393, 0x03c0, 0x03a3, 0x03c3, 0x00b5, 0x03c4, 0x03a6, 0x0398, 0x03a9, 0x03b4,
    0x221e, 0x03c6, 0x03b5, 0x2229, 0x2261, 0x00b1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00f7, 0x2248,
    0x00b0, 0x2219, 0x00b7, 0x221a, 0x207f, 0x00b2, 0x25a0, 0x00a0,
];
