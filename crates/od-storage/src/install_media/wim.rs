//! Streaming split of the WIM 1.13 install-image profile into SWM parts.
//! Resource headers and split layout follow Microsoft's WIM format v1.1.

use std::collections::BTreeSet;

use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;
use super::tree::{Contents, Entry, File, FileData, Name};

const HEADER_BYTES: usize = 208;
const ROW_BYTES: usize = 50;
const PART_LIMIT: u64 = 3_800 * 1024 * 1024;
const MAX_PARTS: usize = 64;
const MAX_TABLE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
const SIZE_MASK: u64 = (1 << 56) - 1;
const HEADER_COMPRESSED: u32 = 0x02;
const HEADER_SPANNED: u32 = 0x08;
const HEADER_READONLY: u32 = 0x04;
const HEADER_REPARSE_FIXUP: u32 = 0x80;
const XPRESS: u32 = HEADER_COMPRESSED | 0x20_000;
const LZX: u32 = HEADER_COMPRESSED | 0x40_000;
const METADATA: u8 = 0x02;
const COMPRESSED: u8 = 0x04;

#[derive(Clone, Copy)]
struct Resource {
    stored_bytes: u64,
    offset: u64,
    original_bytes: u64,
    flags: u8,
}

impl Resource {
    fn parse(bytes: &[u8]) -> Self {
        let size_and_flags = le64(bytes, 0);
        Self {
            stored_bytes: size_and_flags & SIZE_MASK,
            offset: le64(bytes, 8),
            original_bytes: le64(bytes, 16),
            flags: (size_and_flags >> 56) as u8,
        }
    }

    fn marshal(self, offset: u64, bytes: &mut [u8]) {
        let size_and_flags = self.stored_bytes | (u64::from(self.flags) << 56);
        bytes[..8].copy_from_slice(&size_and_flags.to_le_bytes());
        bytes[8..16].copy_from_slice(&offset.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.original_bytes.to_le_bytes());
    }

    fn bounds(self, file_bytes: u64) -> Result<(u64, u64), ImportError> {
        let end = self
            .offset
            .checked_add(self.stored_bytes)
            .ok_or(ImportError::Corrupt("wim_resource_bounds"))?;
        if self.stored_bytes == 0 || self.offset < HEADER_BYTES as u64 || end > file_bytes {
            return Err(ImportError::Corrupt("wim_resource_bounds"));
        }
        Ok((self.offset, end))
    }

    fn table_or_xml(&self, limit: u64) -> Result<(), ImportError> {
        if self.stored_bytes > limit {
            return Err(ImportError::Unsupported("wim_metadata_size"));
        }
        if self.flags & !METADATA != 0 || self.stored_bytes != self.original_bytes {
            return Err(ImportError::Unsupported("wim_metadata_encoding"));
        }
        Ok(())
    }
}

struct Row {
    resource: Resource,
    ref_count: u32,
    sha1: [u8; 20],
}

pub(super) struct Part {
    header: [u8; HEADER_BYTES],
    payload: File,
    lookup: Vec<u8>,
    xml: File,
}

impl Part {
    pub(super) fn length(&self) -> u64 {
        HEADER_BYTES as u64 + self.payload.length + self.lookup.len() as u64 + self.xml.length
    }

    pub(super) fn emit<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        identity: u64,
        buffer: &mut [u8],
        mut put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        let mut header = self.header;
        // Source GUID plus this output's non-zero image identity. All parts
        // receive the same distinct GUID on both production and verification.
        for (dest, salt) in header[32..40].iter_mut().zip(identity.max(1).to_le_bytes()) {
            *dest ^= salt;
        }
        put(0, &header)?;
        let payload_at = HEADER_BYTES as u64;
        self.payload
            .emit(source, buffer, |at, bytes| put(payload_at + at, bytes))?;
        let lookup_at = payload_at + self.payload.length;
        put(lookup_at, &self.lookup)?;
        let xml_at = lookup_at + self.lookup.len() as u64;
        self.xml
            .emit(source, buffer, |at, bytes| put(xml_at + at, bytes))
    }
}

pub(super) fn split_install_wim<B: StorageBackend>(
    source: &CheckedStorage<B>,
    entries: &mut [Entry],
) -> Result<(), ImportError> {
    let Some(directory) = entries
        .iter_mut()
        .find(|entry| entry.name.eq_ignore_ascii_case("sources"))
    else {
        return Ok(());
    };
    let Contents::Directory(children) = &mut directory.contents else {
        return Ok(());
    };
    let Some(index) = children
        .iter()
        .position(|entry| entry.name.eq_ignore_ascii_case("install.wim"))
    else {
        return Ok(());
    };
    let Contents::File(FileData::Source(file)) = &children[index].contents else {
        return Ok(());
    };
    if file.length <= u64::from(u32::MAX) {
        return Ok(());
    }
    let parts = split(source, file, PART_LIMIT)?;
    let mut output = Vec::with_capacity(parts.len());
    for (part_index, part) in parts.into_iter().enumerate() {
        let name = if part_index == 0 {
            String::from("install.swm")
        } else {
            format!("install{}.swm", part_index + 1)
        };
        if children
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(&name))
        {
            return Err(ImportError::Unsupported("wim_swm_conflict"));
        }
        output.push(Entry {
            name: Name::Long(name),
            contents: Contents::File(FileData::SplitWim(part)),
        });
    }
    children.splice(index..=index, output);
    Ok(())
}

fn split<B: StorageBackend>(
    source: &CheckedStorage<B>,
    file: &File,
    limit: u64,
) -> Result<Vec<Part>, ImportError> {
    let mut header = [0; HEADER_BYTES];
    file.read_exact_at(source, 0, &mut header)?;
    let header_flags = le32(&header, 16);
    let compression = header_flags & !(HEADER_READONLY | HEADER_REPARSE_FIXUP);
    let chunk_bytes = le32(&header, 20);
    let image_count = le32(&header, 44);
    if &header[..8] != b"MSWIM\0\0\0"
        || le32(&header, 8) != HEADER_BYTES as u32
        || le32(&header, 12) != 0x10d00
        || !matches!((compression, chunk_bytes), (0, 0) | (XPRESS | LZX, 32_768))
        || header[40..44] != [1, 0, 1, 0]
        || image_count == 0
        || header[96..].iter().any(|&byte| byte != 0)
    {
        return Err(ImportError::Unsupported("wim_profile"));
    }
    let lookup = Resource::parse(&header[48..72]);
    let xml = Resource::parse(&header[72..96]);
    lookup.table_or_xml(MAX_TABLE_BYTES)?;
    xml.table_or_xml(MAX_XML_BYTES)?;
    let mut intervals = vec![lookup.bounds(file.length)?, xml.bounds(file.length)?];
    if !lookup.stored_bytes.is_multiple_of(ROW_BYTES as u64)
        || !xml.stored_bytes.is_multiple_of(2)
        || xml.stored_bytes < 2
    {
        return Err(ImportError::Corrupt("wim_metadata_length"));
    }
    let mut bom = [0; 2];
    file.read_exact_at(source, xml.offset, &mut bom)?;
    if bom != [0xff, 0xfe] {
        return Err(ImportError::Unsupported("wim_xml_encoding"));
    }

    let mut table = vec![0; lookup.stored_bytes as usize];
    file.read_exact_at(source, lookup.offset, &mut table)?;
    let mut metadata = Vec::new();
    let mut files = Vec::new();
    let mut hashes = BTreeSet::new();
    for bytes in table.chunks_exact(ROW_BYTES) {
        let resource = Resource::parse(bytes);
        let ref_count = le32(bytes, 26);
        let sha1 = bytes[30..50].try_into().expect("WIM hash size");
        if resource.flags & !(METADATA | COMPRESSED) != 0
            || (compression == 0 && resource.flags & COMPRESSED != 0)
        {
            return Err(ImportError::Unsupported("wim_resource_flags"));
        }
        if bytes[24..26] != [1, 0]
            || ref_count == 0
            || resource.original_bytes == 0
            || resource.flags & COMPRESSED == 0 && resource.original_bytes != resource.stored_bytes
            || !hashes.insert(sha1)
        {
            return Err(ImportError::Corrupt("wim_lookup_row"));
        }
        intervals.push(resource.bounds(file.length)?);
        let row = Row {
            resource,
            sha1,
            ref_count,
        };
        if resource.flags & METADATA == 0 {
            files.push(row);
        } else {
            metadata.push(row);
        }
    }
    if metadata.len() != image_count as usize {
        return Err(ImportError::Corrupt("wim_image_count"));
    }
    intervals.sort_unstable();
    if intervals.windows(2).any(|rows| rows[0].1 > rows[1].0) {
        return Err(ImportError::Corrupt("wim_resource_overlap"));
    }

    let groups = pack(metadata, files, xml.stored_bytes, limit)?;
    let parts_total = groups.len() as u16;
    let flags = header_flags | HEADER_SPANNED;
    header[16..20].copy_from_slice(&flags.to_le_bytes());
    header[42..44].copy_from_slice(&parts_total.to_le_bytes());
    groups
        .into_iter()
        .enumerate()
        .map(|(index, rows)| build_part(file, header, index as u16 + 1, rows, lookup.flags, xml))
        .collect()
}

fn pack(
    metadata: Vec<Row>,
    files: Vec<Row>,
    xml_bytes: u64,
    limit: u64,
) -> Result<Vec<Vec<Row>>, ImportError> {
    let fixed = HEADER_BYTES as u64 + xml_bytes;
    let row_bytes = |row: &Row| row.resource.stored_bytes + ROW_BYTES as u64;
    let mut used = fixed + metadata.iter().map(row_bytes).sum::<u64>();
    if used > limit {
        return Err(ImportError::Unsupported("wim_part_size"));
    }
    let mut current = metadata;
    let mut parts = Vec::new();
    for row in files {
        let bytes = row_bytes(&row);
        if fixed + bytes > limit {
            return Err(ImportError::Unsupported("wim_part_size"));
        }
        if used + bytes > limit {
            parts.push(std::mem::take(&mut current));
            if parts.len() == MAX_PARTS {
                return Err(ImportError::Unsupported("wim_part_count"));
            }
            used = fixed;
        }
        used += bytes;
        current.push(row);
    }
    parts.push(current);
    Ok(parts)
}

fn build_part(
    file: &File,
    mut header: [u8; HEADER_BYTES],
    number: u16,
    rows: Vec<Row>,
    lookup_flags: u8,
    xml: Resource,
) -> Result<Part, ImportError> {
    let mut payload = Vec::new();
    let mut table = vec![0; rows.len() * ROW_BYTES];
    let mut offset = HEADER_BYTES as u64;
    for (row, bytes) in rows.into_iter().zip(table.chunks_exact_mut(ROW_BYTES)) {
        row.resource.marshal(offset, bytes);
        bytes[24..26].copy_from_slice(&number.to_le_bytes());
        bytes[26..30].copy_from_slice(&row.ref_count.to_le_bytes());
        bytes[30..50].copy_from_slice(&row.sha1);
        file.append_ranges(row.resource.offset, row.resource.stored_bytes, &mut payload)?;
        offset += row.resource.stored_bytes;
    }
    let table_length = table.len() as u64;
    let table_resource = Resource {
        stored_bytes: table_length,
        original_bytes: table_length,
        offset,
        flags: lookup_flags,
    };
    let mut xml_ranges = Vec::new();
    file.append_ranges(xml.offset, xml.stored_bytes, &mut xml_ranges)?;
    header[40..42].copy_from_slice(&number.to_le_bytes());
    table_resource.marshal(offset, &mut header[48..72]);
    xml.marshal(offset + table_length, &mut header[72..96]);
    Ok(Part {
        header,
        payload: File::new(offset - HEADER_BYTES as u64, payload)?,
        lookup: table,
        xml: File::new(xml.stored_bytes, xml_ranges)?,
    })
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("WIM field bounds"))
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("WIM field bounds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_media::tree::Extent;
    use crate::testing::memory::MemoryStorage;

    const XML: &[u8] = b"\xff\xfe<\0W\0/\0>\0";
    const GUID: [u8; 16] = [0x5a; 16];
    const ROWS: [(u8, usize, bool, u32); 5] = [
        (0x11, 100, false, 2),
        (0x22, 15, true, 1),
        (0x33, 110, false, 1),
        (0x44, 25, true, 1),
        (0x55, 60, false, 2),
    ];

    fn sample() -> Vec<u8> {
        let mut bytes = vec![0; HEADER_BYTES];
        bytes[..8].copy_from_slice(b"MSWIM\0\0\0");
        bytes[8..12].copy_from_slice(&208u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&0x10d00u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&HEADER_REPARSE_FIXUP.to_le_bytes());
        bytes[24..40].copy_from_slice(&GUID);
        bytes[40..48].copy_from_slice(&[1, 0, 1, 0, 2, 0, 0, 0]);
        let mut table = [0; ROWS.len() * ROW_BYTES];
        for ((value, length, metadata, refs), row) in
            ROWS.into_iter().zip(table.chunks_exact_mut(ROW_BYTES))
        {
            let flags = if metadata { 0x02 } else { 0 };
            let stored = length as u64 | (flags << 56);
            row[..8].copy_from_slice(&stored.to_le_bytes());
            row[8..16].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
            row[16..24].copy_from_slice(&(length as u64).to_le_bytes());
            row[24] = 1;
            row[26..30].copy_from_slice(&refs.to_le_bytes());
            row[30..50].fill(value);
            bytes.resize(bytes.len() + length, value);
        }
        let table_at = bytes.len() as u64;
        bytes.extend_from_slice(&table);
        let xml_at = bytes.len() as u64;
        bytes.extend_from_slice(XML);
        for (at, offset, length) in [(48, table_at, table.len()), (72, xml_at, XML.len())] {
            let len = length as u64;
            bytes[at..at + 8].copy_from_slice(&(len | (2u64 << 56)).to_le_bytes());
            bytes[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            bytes[at + 16..at + 24].copy_from_slice(&len.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn split_uses_local_lookup_entries_and_copies_whole_resources_across_extents() {
        let bytes = sample();
        let split_at = 223;
        let mut backing = bytes[..split_at].to_vec();
        backing.extend_from_slice(b"not WIM data");
        let second = backing.len();
        backing.extend_from_slice(&bytes[split_at..]);
        let source = CheckedStorage::new(MemoryStorage::new_read_only(&backing)).unwrap();
        let file = File::new(
            bytes.len() as u64,
            vec![
                Extent {
                    offset: 0,
                    length: split_at as u64,
                },
                Extent {
                    offset: second as u64,
                    length: (bytes.len() - split_at) as u64,
                },
            ],
        )
        .unwrap();
        let parts = split(&source, &file, 520).unwrap();
        assert_eq!(parts.len(), 2);
        let mut guid = Vec::new();
        for (index, (part, expected)) in parts.iter().zip([&[1, 3, 0][..], &[2, 4]]).enumerate() {
            assert!(part.length() <= 520);
            let mut bytes = Vec::new();
            part.emit(&source, 471, &mut [0; 37], |at, range| {
                assert_eq!(at as usize, bytes.len());
                bytes.extend_from_slice(range);
                Ok(())
            })
            .unwrap();
            assert_eq!(bytes.len() as u64, part.length());
            assert_eq!(le32(&bytes, 16), 0x88);
            if index == 0 {
                guid = bytes[24..40].to_vec();
                assert_ne!(guid, GUID);
            }
            assert_eq!(bytes[24..40], guid);
            assert_eq!(bytes[40..48], [index as u8 + 1, 0, 2, 0, 2, 0, 0, 0]);
            let table_at = le64(&bytes, 56) as usize;
            let table_length = (le64(&bytes, 48) & SIZE_MASK) as usize;
            let xml_at = le64(&bytes, 80) as usize;
            assert_eq!(xml_at, table_at + table_length);
            assert_eq!(&bytes[xml_at..], XML);
            assert_eq!(table_length, expected.len() * 50);
            let mut resource_at = HEADER_BYTES;
            for (&which, row) in expected
                .iter()
                .zip(bytes[table_at..xml_at].chunks_exact(50))
            {
                let (value, length, metadata, refs) = ROWS[which];
                assert_eq!(le64(row, 8) as usize, resource_at);
                assert_eq!(le64(row, 0) & SIZE_MASK, length as u64);
                assert_eq!(row[7], u8::from(metadata) * 2);
                assert_eq!(row[24..26], [index as u8 + 1, 0]);
                assert_eq!(le32(row, 26), refs);
                assert_eq!(row[30..50], [value; 20]);
                assert_eq!(
                    bytes[resource_at..resource_at + length],
                    vec![value; length]
                );
                resource_at += length;
            }
            assert_eq!(resource_at, table_at);
        }
    }

    #[test]
    fn split_rejects_overlap_unknown_encodings_and_a_resource_that_cannot_fit() {
        let bytes = sample();
        let open = |bytes: &[u8]| {
            (
                CheckedStorage::new(MemoryStorage::new_read_only(bytes)).unwrap(),
                File::contiguous(Extent {
                    offset: 0,
                    length: bytes.len() as u64,
                }),
            )
        };
        let (source, file) = open(&bytes);
        assert!(matches!(
            split(&source, &file, 370),
            Err(ImportError::Unsupported("wim_part_size"))
        ));
        let table_at = le64(&bytes, 56) as usize;
        let mut overlap = bytes.clone();
        overlap[table_at + 58..table_at + 66].copy_from_slice(&208u64.to_le_bytes());
        let (source, file) = open(&overlap);
        assert!(matches!(
            split(&source, &file, 520),
            Err(ImportError::Corrupt("wim_resource_overlap"))
        ));
        let mut solid = bytes;
        solid[table_at + 7] = 0x10;
        let (source, file) = open(&solid);
        assert!(matches!(
            split(&source, &file, 520),
            Err(ImportError::Unsupported("wim_resource_flags"))
        ));
    }
}
