//! Read-only archive of EFI files linked into the firmware image.

use crate::crc32;

const MAGIC: &[u8; 8] = b"ODFVARC\0";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 24;
const CRC_OFFSET: usize = 20;
const RECORD_HEADER_SIZE: usize = 8;
const MIN_RECORD_SIZE: usize = RECORD_HEADER_SIZE + 2;
pub(crate) const MAX_NAME_SIZE: usize = 63;

#[cfg(target_os = "none")]
#[used]
#[unsafe(link_section = ".od_fv.archive")]
static BUILT_IN_ARCHIVE: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/od-fv.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/od-fv.bin"));

#[cfg(target_os = "none")]
pub fn embedded() -> Result<Archive<'static>, ArchiveError> {
    Archive::parse(&BUILT_IN_ARCHIVE)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    Header,
    Size,
    Checksum,
    Count,
    Record,
    Name,
    Order,
}

/// A validated sequence of records with strictly increasing names.
#[derive(Debug)]
pub struct Archive<'a> {
    records: &'a [u8],
}

impl<'a> Archive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ArchiveError> {
        let (header, mut rest) = bytes
            .split_at_checked(HEADER_SIZE)
            .ok_or(ArchiveError::Header)?;
        if &header[..8] != MAGIC
            || word(&header[8..10]) != VERSION
            || usize::from(word(&header[10..12])) != HEADER_SIZE
        {
            return Err(ArchiveError::Header);
        }
        if dword(&header[12..16]) as usize != bytes.len() {
            return Err(ArchiveError::Size);
        }
        let count = dword(&header[16..20]) as usize;
        if count > rest.len() / MIN_RECORD_SIZE {
            return Err(ArchiveError::Count);
        }
        if crc32::with_zeroed_word(bytes, CRC_OFFSET) != dword(&header[20..24]) {
            return Err(ArchiveError::Checksum);
        }
        let records = rest;
        let mut previous: Option<&[u8]> = None;
        for _ in 0..count {
            let (record, remaining) = read_record(rest).ok_or(ArchiveError::Record)?;
            if record.reserved != 0 || record.data.is_empty() {
                return Err(ArchiveError::Record);
            }
            if !valid_name(record.name) {
                return Err(ArchiveError::Name);
            }
            if previous.is_some_and(|previous| !name_precedes(previous, record.name)) {
                return Err(ArchiveError::Order);
            }
            previous = Some(record.name);
            rest = remaining;
        }
        if !rest.is_empty() {
            return Err(ArchiveError::Count);
        }
        Ok(Self { records })
    }

    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.get_bytes(name.as_bytes())
    }

    pub(crate) fn get_bytes(&self, name: &[u8]) -> Option<&'a [u8]> {
        let mut rest = self.records;
        while let Some((record, remaining)) = read_record(rest) {
            if name == record.name {
                return Some(record.data);
            }
            rest = remaining;
        }
        None
    }
}

struct Record<'a> {
    name: &'a [u8],
    data: &'a [u8],
    reserved: u16,
}

fn read_record(bytes: &[u8]) -> Option<(Record<'_>, &[u8])> {
    let (header, payload) = bytes.split_at_checked(RECORD_HEADER_SIZE)?;
    let (name, payload) = payload.split_at_checked(usize::from(word(&header[..2])))?;
    let (data, rest) = payload.split_at_checked(dword(&header[4..8]) as usize)?;
    Some((
        Record {
            name,
            data,
            reserved: word(&header[2..4]),
        },
        rest,
    ))
}

pub(crate) fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_SIZE
        && name != b"."
        && name != b".."
        && name.iter().all(|&byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn name_precedes(left: &[u8], right: &[u8]) -> bool {
    for (&left, &right) in left.iter().zip(right) {
        if left != right {
            return left < right;
        }
    }
    left.len() < right.len()
}

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().expect("validated word width"))
}

fn dword(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("validated dword width"))
}

#[cfg(test)]
#[path = "fv_tests.rs"]
mod tests;
