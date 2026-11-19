//! Versioned, little-endian variable store (docs/specs/NVRAM_DEVICE.md).

use crate::crc32::with_zeroed_word;
use crate::uefi::guid::Guid;

pub const MAX_STORE_BYTES: usize = 256 * 1024;
pub const MAX_NAME_UNITS: usize = 127;
pub const MAX_VALUE_BYTES: usize = 16 * 1024;
pub const HEADER_BYTES: usize = 24;
pub(super) const RECORD_HEADER_BYTES: usize = 40;
const MAGIC: &[u8; 8] = b"ODVARSTR";
const VERSION: u16 = 1;
const HEADER_CRC: usize = 16;
const RECORD_CRC: usize = 36;
const LIVE: u8 = 1;

/// A nonempty, bounded UCS-2 name represented as little-endian bytes without NUL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Name<'a>(&'a [u8]);

impl<'a> Name<'a> {
    pub fn new(bytes: &'a [u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() % 2 != 0 || bytes.len() > MAX_NAME_UNITS * 2 {
            return None;
        }
        let name = Self(bytes);
        name.units().all(valid_unit).then_some(name)
    }

    pub fn units(self) -> impl Iterator<Item = u16> + 'a {
        self.0
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
}

pub const fn valid_unit(unit: u16) -> bool {
    unit != 0 && !(unit >= 0xd800 && unit <= 0xdfff)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record<'a> {
    pub guid: Guid,
    pub name: Name<'a>,
    pub attributes: u32,
    pub value: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Header,
    CorruptRecord { offset: usize },
    TruncatedRecord { offset: usize },
}

pub struct Decoder<'a> {
    bytes: &'a [u8],
    declared_end: usize,
    position: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let header = bytes.get(..HEADER_BYTES).ok_or(DecodeError::Header)?;
        let declared_end = get_u32(header, 12) as usize;
        if &header[..8] != MAGIC
            || get_u16(header, 8) != VERSION
            || get_u16(header, 10) as usize != HEADER_BYTES
            || !(HEADER_BYTES..=MAX_STORE_BYTES).contains(&declared_end)
            || get_u32(header, 20) != 0
            || get_u32(header, HEADER_CRC) != with_zeroed_word(header, HEADER_CRC)
        {
            return Err(DecodeError::Header);
        }
        Ok(Self {
            bytes: &bytes[..bytes.len().min(declared_end)],
            declared_end,
            position: HEADER_BYTES,
        })
    }
}

impl<'a> Iterator for Decoder<'a> {
    type Item = Result<Record<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position == self.declared_end {
            return None;
        }
        let offset = self.position;
        let pending = &self.bytes[offset..];
        let Some(bytes) = framed_record(pending) else {
            self.position = self.declared_end;
            return Some(Err(DecodeError::TruncatedRecord { offset }));
        };
        self.position += bytes.len();
        let record = decode_record(bytes).filter(|record| {
            super::services::persistent_attributes(record.attributes, record.guid, record.name)
                && super::services::valid_write(record.guid, record.name, record.value, false)
                && get_u32(bytes, RECORD_CRC) == with_zeroed_word(bytes, RECORD_CRC)
        });
        Some(record.ok_or(DecodeError::CorruptRecord { offset }))
    }
}

pub(super) fn write_header(out: &mut [u8], total: usize) {
    let header = &mut out[..HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
    header[12..16].copy_from_slice(&(total as u32).to_le_bytes());
    header[20..24].fill(0);
    let crc = with_zeroed_word(header, HEADER_CRC);
    header[HEADER_CRC..HEADER_CRC + 4].copy_from_slice(&crc.to_le_bytes());
}

pub(super) const fn record_size(name: Name<'_>, value_size: usize) -> usize {
    RECORD_HEADER_BYTES + name.0.len() + value_size
}

pub(super) fn write_record(out: &mut [u8], record: Record<'_>) {
    let name_end = RECORD_HEADER_BYTES + record.name.0.len();
    out[8] = LIVE;
    out[9] = 0;
    out[10..12].copy_from_slice(&((record.name.0.len() / 2) as u16).to_le_bytes());
    out[12..16].copy_from_slice(&record.attributes.to_le_bytes());
    out[20..24].copy_from_slice(&record.guid.data1.to_le_bytes());
    out[24..26].copy_from_slice(&record.guid.data2.to_le_bytes());
    out[26..28].copy_from_slice(&record.guid.data3.to_le_bytes());
    out[28..36].copy_from_slice(&record.guid.data4);
    out[RECORD_HEADER_BYTES..name_end].copy_from_slice(record.name.0);
    copy_bytes(&mut out[name_end..], record.value);
    seal_record(out, record.value.len());
}

/// Refresh the framing and checksum after extending a live record's value.
pub(super) fn seal_record(out: &mut [u8], value_size: usize) {
    let size = out.len() as u32;
    out[..4].copy_from_slice(&size.to_le_bytes());
    out[4..8].copy_from_slice(&(!size).to_le_bytes());
    out[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
    let crc = with_zeroed_word(out, RECORD_CRC);
    out[RECORD_CRC..RECORD_CRC + 4].copy_from_slice(&crc.to_le_bytes());
}

pub(super) fn framed_record(pending: &[u8]) -> Option<&[u8]> {
    let header = pending.get(..RECORD_HEADER_BYTES)?;
    let size = get_u32(header, 0);
    if get_u32(header, 4) != !size || size < RECORD_HEADER_BYTES as u32 {
        return None;
    }
    pending.get(..size as usize)
}

pub(super) fn decode_record(bytes: &[u8]) -> Option<Record<'_>> {
    let name_end = RECORD_HEADER_BYTES + 2 * get_u16(bytes, 10) as usize;
    let value_size = get_u32(bytes, 16) as usize;
    if bytes[8] != LIVE
        || bytes[9] != 0
        || !(1..=MAX_VALUE_BYTES).contains(&value_size)
        || name_end + value_size != bytes.len()
    {
        return None;
    }
    Some(Record {
        guid: Guid {
            data1: get_u32(bytes, 20),
            data2: get_u16(bytes, 24),
            data3: get_u16(bytes, 26),
            data4: bytes[28..36].try_into().ok()?,
        },
        name: Name::new(&bytes[RECORD_HEADER_BYTES..name_end])?,
        attributes: get_u32(bytes, 12),
        value: &bytes[name_end..],
    })
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub(super) fn copy_bytes(out: &mut [u8], bytes: &[u8]) {
    assert_eq!(out.len(), bytes.len());
    // SAFETY: both slices cover the same length. The helper bounds REP work.
    unsafe {
        crate::bs::misc::copy_mem(out.as_mut_ptr().cast(), bytes.as_ptr().cast(), bytes.len())
    };
}
