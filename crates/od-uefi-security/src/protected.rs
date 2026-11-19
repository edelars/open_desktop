//! The protected ODVARSTR v2 image shared by the firmware and the NVRAM device.

use crate::{Guid, Name, authvar::Timestamp, crc32::with_zeroed_word};

pub const IMAGE_CAPACITY: usize = 256 * 1024;
pub const ORDINARY_VALUE_LIMIT: usize = 16 * 1024;
pub const PROTECTED_VALUE_LIMIT: usize = 64 * 1024;
pub const HEADER_BYTES: usize = 24;
const RECORD_HEADER_BYTES: usize = 40;
const TIMESTAMP_BYTES: usize = 16;
const HEADER_CRC: usize = 16;
const RECORD_CRC: usize = 36;
const LIVE: u8 = 1;
const TOMBSTONE: u8 = 2;
const AUTHENTICATED: u8 = 1;
const VERSION: u16 = 2;
const MAGIC: &[u8; 8] = b"ODVARSTR";

pub const GLOBAL_VARIABLE: Guid = Guid {
    data1: 0x8be4_df61,
    data2: 0x93ca,
    data3: 0x11d2,
    data4: [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c],
};
pub const IMAGE_SECURITY_DATABASE: Guid = Guid {
    data1: 0xd719_b2cb,
    data2: 0x3d3a,
    data3: 0x4596,
    data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
};

const ORDINARY_REQUIRED_ATTRIBUTES: u32 = 0x03;
const ORDINARY_ALLOWED_ATTRIBUTES: u32 = 0x07;
pub const AUTHENTICATED_ATTRIBUTES: u32 = 0x27;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PolicyVariable {
    Pk,
    Kek,
    Db,
    Dbx,
    Dbt,
    Dbr,
}

impl PolicyVariable {
    const COUNT: usize = 6;

    pub fn from_key(guid: Guid, name: Name<'_>) -> Option<Self> {
        let bytes = name.as_bytes();
        match (guid, bytes) {
            (GLOBAL_VARIABLE, b"P\0K\0") => Some(Self::Pk),
            (GLOBAL_VARIABLE, b"K\0E\0K\0") => Some(Self::Kek),
            (IMAGE_SECURITY_DATABASE, b"d\0b\0") => Some(Self::Db),
            (IMAGE_SECURITY_DATABASE, b"d\0b\0x\0") => Some(Self::Dbx),
            (IMAGE_SECURITY_DATABASE, b"d\0b\0t\0") => Some(Self::Dbt),
            (IMAGE_SECURITY_DATABASE, b"d\0b\0r\0") => Some(Self::Dbr),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flags {
    pub secure_boot_disabled: bool,
    pub vendor_keys_modified: bool,
}

impl Flags {
    fn from_bits(bits: u32) -> Option<Self> {
        (bits <= 3).then_some(Self {
            secure_boot_disabled: bits & 1 != 0,
            vendor_keys_modified: bits & 2 != 0,
        })
    }

    fn bits(self) -> u32 {
        u32::from(self.secure_boot_disabled) | (u32::from(self.vendor_keys_modified) << 1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Header,
    Record,
    DuplicatePolicyVariable,
    Capacity,
}

#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub guid: Guid,
    pub name: Name<'a>,
    pub attributes: u32,
    pub value: &'a [u8],
    pub watermark: Option<Timestamp>,
}

impl Record<'_> {
    pub fn policy_variable(self) -> Option<PolicyVariable> {
        PolicyVariable::from_key(self.guid, self.name)
    }

    fn validate(self, kind: Option<PolicyVariable>) -> Result<(), Error> {
        if kind.is_some() {
            if self.attributes != AUTHENTICATED_ATTRIBUTES
                || (self.value.is_empty() && self.watermark.is_none())
            {
                return Err(Error::Record);
            }
            if self.value.len() > PROTECTED_VALUE_LIMIT {
                return Err(Error::Capacity);
            }
        } else {
            if self.attributes & ORDINARY_REQUIRED_ATTRIBUTES != ORDINARY_REQUIRED_ATTRIBUTES
                || self.attributes & !ORDINARY_ALLOWED_ATTRIBUTES != 0
                || self.value.is_empty()
                || self.watermark.is_some()
                || security_mode_variable(self.guid, self.name)
            {
                return Err(Error::Record);
            }
            if self.value.len() > ORDINARY_VALUE_LIMIT {
                return Err(Error::Capacity);
            }
        }
        Ok(())
    }

    /// Encodes one complete record and leaves the remainder of `out` available to append records.
    pub fn write(self, out: &mut [u8]) -> Result<usize, Error> {
        let kind = self.policy_variable();
        self.validate(kind)?;
        let name = self.name.as_bytes();
        let name_end = RECORD_HEADER_BYTES + name.len();
        let data_start = name_end + if kind.is_some() { TIMESTAMP_BYTES } else { 0 };
        let record_size = data_start + self.value.len();
        let out = out.get_mut(..record_size).ok_or(Error::Capacity)?;
        let size = record_size as u32;
        out[..4].copy_from_slice(&size.to_le_bytes());
        out[4..8].copy_from_slice(&(!size).to_le_bytes());
        out[8] = if self.value.is_empty() {
            TOMBSTONE
        } else {
            LIVE
        };
        out[9] = if kind.is_some() { AUTHENTICATED } else { 0 };
        out[10..12].copy_from_slice(&((name.len() / 2) as u16).to_le_bytes());
        out[12..16].copy_from_slice(&self.attributes.to_le_bytes());
        out[16..20].copy_from_slice(&(self.value.len() as u32).to_le_bytes());
        out[20..24].copy_from_slice(&self.guid.data1.to_le_bytes());
        out[24..26].copy_from_slice(&self.guid.data2.to_le_bytes());
        out[26..28].copy_from_slice(&self.guid.data3.to_le_bytes());
        out[28..36].copy_from_slice(&self.guid.data4);
        out[RECORD_HEADER_BYTES..name_end].copy_from_slice(name);
        if kind.is_some() {
            let watermark = self
                .watermark
                .map_or([0; TIMESTAMP_BYTES], Timestamp::to_bytes);
            out[name_end..data_start].copy_from_slice(&watermark);
        }
        out[data_start..].copy_from_slice(self.value);
        let crc = with_zeroed_word(out, RECORD_CRC);
        out[RECORD_CRC..RECORD_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        Ok(record_size)
    }
}

pub struct Image<'a> {
    records: &'a [u8],
    pub flags: Flags,
}

impl<'a> Image<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, Error> {
        let header: &[u8; HEADER_BYTES] = bytes.first_chunk().ok_or(Error::Header)?;
        if header[..8] != *MAGIC
            || u16_at(header, 8) != VERSION
            || usize::from(u16_at(header, 10)) != HEADER_BYTES
            || bytes.len() > IMAGE_CAPACITY
            || u32_at(header, 12) as usize != bytes.len()
            || u32_at(header, HEADER_CRC) != with_zeroed_word(header, HEADER_CRC)
        {
            return Err(Error::Header);
        }
        let flags = Flags::from_bits(u32_at(header, 20)).ok_or(Error::Header)?;
        Ok(Self {
            records: &bytes[HEADER_BYTES..],
            flags,
        })
    }

    pub fn records(&self) -> impl Iterator<Item = Result<(Record<'a>, &'a [u8]), Error>> {
        let mut records = self.records;
        core::iter::from_fn(move || {
            if records.is_empty() {
                return None;
            }
            let decoded = read_record(records);
            match decoded {
                Ok((record, encoded)) => {
                    records = &records[encoded.len()..];
                    Some(Ok((record, encoded)))
                }
                Err(err) => {
                    records = &[];
                    Some(Err(err))
                }
            }
        })
    }

    /// Compares protected records by key, even if ordinary records move during compaction.
    pub fn policy_bytes(&self) -> Result<[Option<&'a [u8]>; PolicyVariable::COUNT], Error> {
        let mut keys = [None; PolicyVariable::COUNT];
        for decoded in self.records() {
            let (record, encoded) = decoded?;
            if let Some(key) = record.policy_variable() {
                if keys[key as usize].replace(encoded).is_some() {
                    return Err(Error::DuplicatePolicyVariable);
                }
            }
        }
        Ok(keys)
    }

    pub fn write_header(out: &mut [u8], flags: Flags) -> Result<(), Error> {
        if out.len() > IMAGE_CAPACITY {
            return Err(Error::Capacity);
        }
        let length = out.len() as u32;
        let header = out.get_mut(..HEADER_BYTES).ok_or(Error::Capacity)?;
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_le_bytes());
        header[10..12].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
        header[12..16].copy_from_slice(&length.to_le_bytes());
        header[20..24].copy_from_slice(&flags.bits().to_le_bytes());
        let crc = with_zeroed_word(header, HEADER_CRC);
        header[HEADER_CRC..HEADER_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        Ok(())
    }
}

fn read_record(bytes: &[u8]) -> Result<(Record<'_>, &[u8]), Error> {
    let head: &[u8; RECORD_HEADER_BYTES] = bytes.first_chunk().ok_or(Error::Record)?;
    let size = u32_at(head, 0);
    if u32_at(head, 4) != !size || (size as usize) < RECORD_HEADER_BYTES {
        return Err(Error::Record);
    }
    let encoded = bytes.get(..size as usize).ok_or(Error::Record)?;
    let name_end = RECORD_HEADER_BYTES + 2 * usize::from(u16_at(head, 10));
    let name = encoded
        .get(RECORD_HEADER_BYTES..name_end)
        .and_then(Name::new)
        .ok_or(Error::Record)?;
    let guid = Guid {
        data1: u32_at(head, 20),
        data2: u16_at(head, 24),
        data3: u16_at(head, 26),
        data4: head[28..36].try_into().unwrap(),
    };
    let kind = PolicyVariable::from_key(guid, name);
    let protected_flag = if kind.is_some() { AUTHENTICATED } else { 0 };
    if head[9] != protected_flag {
        return Err(Error::Record);
    }
    let mut body = &encoded[name_end..];
    let watermark = if kind.is_some() {
        let (raw, value) = body
            .split_first_chunk::<TIMESTAMP_BYTES>()
            .ok_or(Error::Record)?;
        body = value;
        if *raw == [0; TIMESTAMP_BYTES] {
            None
        } else {
            Some(Timestamp::from_bytes(raw).ok_or(Error::Record)?)
        }
    } else {
        None
    };
    let state = if body.is_empty() { TOMBSTONE } else { LIVE };
    if head[8] != state
        || body.len() != u32_at(head, 16) as usize
        || u32_at(head, RECORD_CRC) != with_zeroed_word(encoded, RECORD_CRC)
    {
        return Err(Error::Record);
    }
    let record = Record {
        guid,
        name,
        attributes: u32_at(head, 12),
        value: body,
        watermark,
    };
    record.validate(kind)?;
    Ok((record, encoded))
}

fn security_mode_variable(guid: Guid, name: Name<'_>) -> bool {
    guid == GLOBAL_VARIABLE
        && matches!(
            name.as_bytes(),
            b"S\0e\0c\0u\0r\0e\0B\0o\0o\0t\0"
                | b"S\0e\0t\0u\0p\0M\0o\0d\0e\0"
                | b"A\0u\0d\0i\0t\0M\0o\0d\0e\0"
                | b"D\0e\0p\0l\0o\0y\0e\0d\0M\0o\0d\0e\0"
                | b"V\0e\0n\0d\0o\0r\0K\0e\0y\0s\0"
                | b"S\0i\0g\0n\0a\0t\0u\0r\0e\0S\0u\0p\0p\0o\0r\0t\0"
        )
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_FLAGS: Flags = Flags {
        secure_boot_disabled: false,
        vendor_keys_modified: false,
    };
    const DBX: &[u8] = include_bytes!("../../../tests/fixtures/firmware/certs/dbx-x64-v1.7.2.bin");

    fn dbx(value: &[u8], watermark: Option<Timestamp>) -> Record<'_> {
        Record {
            guid: IMAGE_SECURITY_DATABASE,
            name: Name::new(b"d\0b\0x\0").unwrap(),
            attributes: AUTHENTICATED_ATTRIBUTES,
            value,
            watermark,
        }
    }

    fn encode(record: Record<'_>) -> Vec<u8> {
        let len = HEADER_BYTES
            + RECORD_HEADER_BYTES
            + record.name.as_bytes().len()
            + TIMESTAMP_BYTES
            + record.value.len();
        let mut bytes = vec![0; len];
        let length = record.write(&mut bytes[HEADER_BYTES..]).unwrap();
        bytes.truncate(HEADER_BYTES + length);
        Image::write_header(&mut bytes, DEFAULT_FLAGS).unwrap();
        bytes
    }

    #[test]
    fn official_dbx_fits_without_trimming_and_tombstones_keep_the_timestamp() {
        let original = encode(dbx(DBX, None));
        let image = Image::new(&original).unwrap();
        let policy = image.policy_bytes().unwrap();
        assert_eq!(policy.iter().flatten().count(), 1);
        assert!(policy[PolicyVariable::Dbx as usize].is_some());
        let record = image.records().next().unwrap().unwrap().0;
        assert_eq!(record.value, DBX);
        assert_eq!(record.watermark, None);

        let stamp = Timestamp::from_bytes(&[0xea, 7, 11, 19, 22, 12, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            .unwrap();
        let removed = encode(dbx(b"", Some(stamp)));
        assert_eq!(removed[HEADER_BYTES + 8], TOMBSTONE);
        let image = Image::new(&removed).unwrap();
        let record = image.records().next().unwrap().unwrap().0;
        assert!(record.value.is_empty());
        assert_eq!(record.watermark, Some(stamp));
    }

    #[test]
    fn rejects_downgrade_truncation_invalid_time_and_read_only_overrides() {
        let valid = encode(dbx(DBX, None));
        let broken_header = &valid[..valid.len() - 1];
        assert_eq!(Image::new(broken_header).err(), Some(Error::Header));

        let mut downgrade = valid.clone();
        downgrade[8..10].copy_from_slice(&1u16.to_le_bytes());
        let crc = with_zeroed_word(&downgrade[..HEADER_BYTES], HEADER_CRC);
        downgrade[HEADER_CRC..HEADER_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Image::new(&downgrade).err(), Some(Error::Header));

        let mut bad_time = valid;
        let offset = HEADER_BYTES + RECORD_HEADER_BYTES + b"d\0b\0x\0".len();
        bad_time[offset + 2] = 13;
        let crc = with_zeroed_word(&bad_time[HEADER_BYTES..], RECORD_CRC);
        bad_time[HEADER_BYTES + RECORD_CRC..HEADER_BYTES + RECORD_CRC + 4]
            .copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            Image::new(&bad_time).unwrap().policy_bytes(),
            Err(Error::Record)
        );

        let forged = Record {
            guid: GLOBAL_VARIABLE,
            name: Name::new(b"S\0e\0t\0u\0p\0M\0o\0d\0e\0").unwrap(),
            attributes: 7,
            value: &[1],
            watermark: None,
        };
        assert_eq!(forged.write(&mut [0; 128]), Err(Error::Record));
    }
}
