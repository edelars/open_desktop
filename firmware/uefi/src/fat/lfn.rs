//! UCS-2 long name records and their binding to a following short entry.

const LAST: u8 = 0x40;
const UNITS_PER_RECORD: usize = 13;
const MAX_RECORDS: u8 = 20;
const MAX_NAME_UNITS: usize = 255;
const UNIT_OFFSETS: [usize; UNITS_PER_RECORD] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

pub(super) struct LongName {
    name: [u16; MAX_RECORDS as usize * UNITS_PER_RECORD],
    total: u8,
    next: u8,
    checksum: u8,
}

impl LongName {
    pub(super) const fn new() -> Self {
        Self {
            name: [0xffff; 260],
            total: 0,
            next: 0,
            checksum: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.total = 0;
        self.next = 0;
    }

    pub(super) fn push(&mut self, record: &[u8; 32]) {
        let ordinal = record[0];
        let slot = ordinal & 0x1f;
        if ordinal & !0x5f != 0
            || !(1..=MAX_RECORDS).contains(&slot)
            || record[11] != 0x0f
            || record[12] != 0
            || record[26..28] != [0, 0]
        {
            self.reset();
            return;
        }
        if ordinal & LAST != 0 {
            self.total = slot;
            self.next = slot;
            self.checksum = record[13];
        }
        if self.next != slot || self.checksum != record[13] {
            self.reset();
            return;
        }
        let start = (usize::from(slot) - 1) * UNITS_PER_RECORD;
        for (index, at) in UNIT_OFFSETS.into_iter().enumerate() {
            self.name[start + index] = u16::from_le_bytes([record[at], record[at + 1]]);
        }
        self.next -= 1;
    }

    pub(super) fn take(&mut self, short: &[u8; 11]) -> Option<&[u16]> {
        let count = usize::from(self.total) * UNITS_PER_RECORD;
        let complete = self.next == 0 && count != 0 && self.checksum == checksum(short);
        self.reset();
        if !complete {
            return None;
        }
        let units = &self.name[..count];
        let end = units
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(units.len());
        let name = &units[..end];
        if name.is_empty()
            || name.len() > MAX_NAME_UNITS
            || name.iter().any(|&unit| !valid_name_unit(unit))
            || (end < count && units[end + 1..].iter().any(|&unit| unit != 0xffff))
        {
            return None;
        }
        Some(name)
    }
}

fn valid_name_unit(unit: u16) -> bool {
    if let Some(ch) = char::from_u32(u32::from(unit)) {
        return unit >= 0x20
            && unit != 0xffff
            && !matches!(ch, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|');
    }
    false
}

fn checksum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte))
}
