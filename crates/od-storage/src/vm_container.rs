//! Versioned full-VM checkpoint container codec (S9-23).
//!
//! A pure byte-level codec: it neither knows what a section means nor
//! produces participant data. CPU, RAM, device, UEFI and vTPM state arrive from
//! the caller as opaque bytes (inline sections) or as identity-only references
//! to separate files (reference sections). The caller also names which
//! sections its profile requires; a missing participant is an error on both
//! the write and the read side, never a placeholder or zero fill. The layout is
//! specified in STORAGE_FORMAT.md ("Контейнер VM v1"); the format is not yet
//! accepted as normative.
//!
//! # Layout (all integers little endian)
//!
//! ```text
//!   header (128 B)            section table (count * 64 B)     payloads
//!   [0..128]                  [128 .. 128 + 64 * count]        packed, in table order
//!
//! header:
//!    0.. 8  magic "ODVMCK\0\x01"      64..68  profile u32
//!    8..12  version u32 = 1           68..72  section_count u32
//!   12..16  header_len u32 = 128      72..80  total_len u64 (whole file)
//!   16..24  mandatory flags u64 = 0   80..84  CRC-32 of the section table
//!   24..32  optional flags u64        84..124 reserved, zero
//!   32..48  vm_identity [u8; 16]     124..128 CRC-32 of bytes 0..124
//!   48..56  checkpoint_id u64
//!   56..64  epoch u64 (not 0, not u64::MAX)
//!
//! section entry (64 B):
//!    0.. 4  kind u32          20..28  offset u64 (from file start)
//!    4.. 8  flags u32         28..36  len u64
//!    8..12  instance u32      36..44  epoch u64 (== header epoch)
//!   12..16  schema_id u32     44..48  CRC-32 of the payload
//!   16..20  schema_version    48..64  reserved, zero
//! ```
//!
//! Section flags: bit 0 mandatory, bit 1 reference (otherwise inline). Other
//! bits are reserved and must be zero.
//!
//! A reference payload is exactly [`REFERENCE_LEN`] bytes: file_uid (16 B),
//! generation u64, byte_len u64, CRC-32 of the referenced bytes u32, 4 reserved
//! zero bytes. It carries no path and no host handle.
//!
//! # Limits
//!
//! [`MAX_SECTIONS`], [`MAX_SECTION_BYTES`] and [`MAX_TOTAL_BYTES`] are checked
//! on the values read from the file before any table or payload is touched or
//! allocated. The reader borrows payloads from the input; its only allocations
//! are bounded by [`MAX_SECTIONS`].
//!
//! # Not covered
//!
//! CRC-32 detects corruption; it does not authenticate the container or the
//! referenced files. The reader cannot verify the referenced files themselves
//! (it never opens them). Choosing required kinds, the schema of each section
//! and the cross-section consistency rules belong to the participants.

use crate::crc32::crc32;
use std::fmt;

/// File magic: `ODVMCK`, NUL, format byte 1.
pub const MAGIC: [u8; 8] = *b"ODVMCK\0\x01";
/// Container version written and accepted by this codec.
pub const VERSION: u32 = 1;
/// Fixed header length.
pub const HEADER_LEN: usize = 128;
/// Fixed section table entry length.
pub const ENTRY_LEN: usize = 64;
/// Exact payload length of a reference section.
pub const REFERENCE_LEN: usize = 40;
/// Most sections one container may declare.
pub const MAX_SECTIONS: u32 = 256;
/// Largest single payload (inline bytes) in a container.
pub const MAX_SECTION_BYTES: u64 = 1 << 26;
/// Largest whole container.
pub const MAX_TOTAL_BYTES: u64 = 1 << 28;
/// Section flag: the section is mandatory (a reader that does not know its
/// kind must reject the container).
pub const FLAG_MANDATORY: u32 = 1;
/// Section flag: the payload is a reference record, not inline bytes.
pub const FLAG_REFERENCE: u32 = 2;

const KNOWN_FLAGS: u32 = FLAG_MANDATORY | FLAG_REFERENCE;

/// Which header field of the container disagreed with the expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityField {
    VmIdentity,
    CheckpointId,
    Profile,
}

/// A limit of the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Sections,
    SectionBytes,
    TotalBytes,
}

/// Where a non-zero reserved field was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Header,
    /// Table entry by index (also used for unknown entry flag bits).
    Entry(u32),
    /// Reserved tail of a reference payload.
    Reference {
        kind: u32,
        instance: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerError {
    BadMagic,
    BadVersion {
        found: u32,
    },
    BadHeaderLen {
        found: u32,
    },
    HeaderCrc,
    TableCrc,
    SectionCrc {
        kind: u32,
        instance: u32,
    },
    ReservedNonZero {
        place: Place,
    },
    /// Non-zero mandatory header feature bits: nothing is defined in v1.
    UnsupportedHeaderFlags {
        mandatory: u64,
    },
    /// Header epoch is 0 or u64::MAX.
    BadEpoch {
        epoch: u64,
    },
    MissingRequiredSection {
        kind: u32,
        instance: u32,
    },
    DuplicateSection {
        kind: u32,
        instance: u32,
    },
    UnknownMandatory {
        kind: u32,
        instance: u32,
    },
    Overlap {
        kind: u32,
        instance: u32,
    },
    /// Bytes between sections, or after the last one, that no section covers.
    Gap {
        offset: u64,
    },
    OutOfBounds {
        what: &'static str,
    },
    Overflow {
        kind: u32,
        instance: u32,
    },
    LimitExceeded {
        limit: Limit,
        value: u64,
        max: u64,
    },
    EpochMismatch {
        kind: u32,
        instance: u32,
        found: u64,
        expected: u64,
    },
    IdentityMismatch {
        field: IdentityField,
    },
    /// A section the profile requires was supplied with mandatory = false.
    RequiredNotMandatory {
        kind: u32,
        instance: u32,
    },
    /// Zero-length payload: would be a placeholder for an absent participant.
    EmptySection {
        kind: u32,
        instance: u32,
    },
    /// Reference section whose payload is not exactly [`REFERENCE_LEN`] bytes.
    BadReferenceLen {
        kind: u32,
        instance: u32,
        len: u64,
    },
    /// Reference with an all-zero file_uid or zero byte_len.
    BadReference {
        kind: u32,
        instance: u32,
    },
}

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vm container: {self:?}")
    }
}

impl std::error::Error for ContainerError {}

/// Identity-only description of a separate file (RAM, disk, UEFI, vTPM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceId {
    pub file_uid: [u8; 16],
    pub generation: u64,
    /// Length of the referenced bytes.
    pub byte_len: u64,
    /// CRC-32 of the referenced bytes.
    pub crc32: u32,
}

impl ReferenceId {
    fn encode(&self) -> [u8; REFERENCE_LEN] {
        let mut b = [0u8; REFERENCE_LEN];
        b[0..16].copy_from_slice(&self.file_uid);
        b[16..24].copy_from_slice(&self.generation.to_le_bytes());
        b[24..32].copy_from_slice(&self.byte_len.to_le_bytes());
        b[32..36].copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    fn validate(&self, kind: u32, instance: u32) -> Result<(), ContainerError> {
        if self.file_uid == [0u8; 16] || self.byte_len == 0 {
            return Err(ContainerError::BadReference { kind, instance });
        }
        Ok(())
    }
}

/// Payload of one section as the caller supplies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionBody<'a> {
    Inline(&'a [u8]),
    Reference(ReferenceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionInput<'a> {
    pub kind: u32,
    pub instance: u32,
    pub schema_id: u32,
    pub schema_version: u32,
    pub mandatory: bool,
    pub body: SectionBody<'a>,
}

/// Header values the caller owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub vm_identity: [u8; 16],
    pub checkpoint_id: u64,
    pub epoch: u64,
    pub profile: u32,
}

/// What the writer needs besides the sections.
#[derive(Debug, Clone, Copy)]
pub struct WriteSpec<'a> {
    pub header: Header,
    /// (kind, instance) pairs the profile requires; absent ones fail the write.
    pub required: &'a [(u32, u32)],
}

/// What the reader checks the container against.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    pub vm_identity: [u8; 16],
    pub checkpoint_id: u64,
    pub profile: u32,
    /// (kind, instance) pairs the profile requires.
    pub required: &'a [(u32, u32)],
    /// Kinds the caller understands besides the required ones. A section of
    /// any other kind is rejected when mandatory and skipped when optional.
    pub known_kinds: &'a [u32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionData<'a> {
    Inline(&'a [u8]),
    Reference(ReferenceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section<'a> {
    pub kind: u32,
    pub instance: u32,
    pub schema_id: u32,
    pub schema_version: u32,
    pub mandatory: bool,
    pub data: SectionData<'a>,
}

/// A fully verified container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container<'a> {
    pub header: Header,
    /// Known sections, in (kind, instance) order.
    pub sections: Vec<Section<'a>>,
    /// (kind, instance) of unknown optional sections that were skipped. They
    /// passed every structural and CRC check.
    pub skipped_optional: Vec<(u32, u32)>,
}

impl<'a> Container<'a> {
    pub fn section(&self, kind: u32, instance: u32) -> Option<&Section<'a>> {
        self.sections
            .iter()
            .find(|s| s.kind == kind && s.instance == instance)
    }
}

fn valid_epoch(epoch: u64) -> bool {
    epoch != 0 && epoch != u64::MAX
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

fn all_zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

fn check_required(
    required: &[(u32, u32)],
    present: impl Fn(u32, u32) -> bool,
) -> Result<(), ContainerError> {
    for &(kind, instance) in required {
        if !present(kind, instance) {
            return Err(ContainerError::MissingRequiredSection { kind, instance });
        }
    }
    Ok(())
}

/// Builds a container. Sections are laid out sorted by (kind, instance) in the
/// table and in the payloads, packed directly after the table, so the same set
/// of sections gives the same bytes whatever order the caller passes them in
/// (the reader accepts any table order). Every limit is checked before the
/// output buffer is filled. Nothing is invented: an empty inline payload, a
/// missing required section, a required section that is not mandatory and a
/// zero reference are errors.
pub fn write_container(
    spec: &WriteSpec<'_>,
    sections: &[SectionInput<'_>],
) -> Result<Vec<u8>, ContainerError> {
    let h = &spec.header;
    if !valid_epoch(h.epoch) {
        return Err(ContainerError::BadEpoch { epoch: h.epoch });
    }
    if sections.len() as u64 > u64::from(MAX_SECTIONS) {
        return Err(ContainerError::LimitExceeded {
            limit: Limit::Sections,
            value: sections.len() as u64,
            max: u64::from(MAX_SECTIONS),
        });
    }
    let mut order: Vec<usize> = (0..sections.len()).collect();
    order.sort_by_key(|&i| (sections[i].kind, sections[i].instance));
    for w in order.windows(2) {
        let (a, b) = (&sections[w[0]], &sections[w[1]]);
        if (a.kind, a.instance) == (b.kind, b.instance) {
            return Err(ContainerError::DuplicateSection {
                kind: a.kind,
                instance: a.instance,
            });
        }
    }
    check_required(spec.required, |k, i| {
        sections.iter().any(|s| s.kind == k && s.instance == i)
    })?;
    // A required participant must be mandatory: otherwise a reader that does
    // not know its kind would skip it instead of rejecting the container.
    for &(kind, instance) in spec.required {
        let optional = sections
            .iter()
            .any(|s| s.kind == kind && s.instance == instance && !s.mandatory);
        if optional {
            return Err(ContainerError::RequiredNotMandatory { kind, instance });
        }
    }

    let table_end = (HEADER_LEN + ENTRY_LEN * sections.len()) as u64;
    let mut total = table_end;
    for s in sections {
        let len = match &s.body {
            SectionBody::Inline(b) => {
                if b.is_empty() {
                    return Err(ContainerError::EmptySection {
                        kind: s.kind,
                        instance: s.instance,
                    });
                }
                b.len() as u64
            }
            SectionBody::Reference(r) => {
                r.validate(s.kind, s.instance)?;
                REFERENCE_LEN as u64
            }
        };
        if len > MAX_SECTION_BYTES {
            return Err(ContainerError::LimitExceeded {
                limit: Limit::SectionBytes,
                value: len,
                max: MAX_SECTION_BYTES,
            });
        }
        total = total.checked_add(len).ok_or(ContainerError::Overflow {
            kind: s.kind,
            instance: s.instance,
        })?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(ContainerError::LimitExceeded {
            limit: Limit::TotalBytes,
            value: total,
            max: MAX_TOTAL_BYTES,
        });
    }

    let mut out = vec![0u8; HEADER_LEN + ENTRY_LEN * sections.len()];
    out.reserve((total - table_end) as usize);
    let mut offset = table_end;
    for (n, &i) in order.iter().enumerate() {
        let s = &sections[i];
        let (len, mut flags) = match &s.body {
            SectionBody::Inline(b) => (b.len(), 0),
            SectionBody::Reference(_) => (REFERENCE_LEN, FLAG_REFERENCE),
        };
        let crc = match &s.body {
            SectionBody::Inline(b) => crc32(b),
            SectionBody::Reference(r) => crc32(&r.encode()),
        };
        if s.mandatory {
            flags |= FLAG_MANDATORY;
        }
        let e = HEADER_LEN + n * ENTRY_LEN;
        out[e..e + 4].copy_from_slice(&s.kind.to_le_bytes());
        out[e + 4..e + 8].copy_from_slice(&flags.to_le_bytes());
        out[e + 8..e + 12].copy_from_slice(&s.instance.to_le_bytes());
        out[e + 12..e + 16].copy_from_slice(&s.schema_id.to_le_bytes());
        out[e + 16..e + 20].copy_from_slice(&s.schema_version.to_le_bytes());
        out[e + 20..e + 28].copy_from_slice(&offset.to_le_bytes());
        out[e + 28..e + 36].copy_from_slice(&(len as u64).to_le_bytes());
        out[e + 36..e + 44].copy_from_slice(&h.epoch.to_le_bytes());
        out[e + 44..e + 48].copy_from_slice(&crc.to_le_bytes());
        offset += len as u64;
    }
    for &i in &order {
        match &sections[i].body {
            SectionBody::Inline(b) => out.extend_from_slice(b),
            SectionBody::Reference(r) => out.extend_from_slice(&r.encode()),
        }
    }
    debug_assert_eq!(out.len() as u64, total);

    out[0..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    out[32..48].copy_from_slice(&h.vm_identity);
    out[48..56].copy_from_slice(&h.checkpoint_id.to_le_bytes());
    out[56..64].copy_from_slice(&h.epoch.to_le_bytes());
    out[64..68].copy_from_slice(&h.profile.to_le_bytes());
    out[68..72].copy_from_slice(&(sections.len() as u32).to_le_bytes());
    out[72..80].copy_from_slice(&total.to_le_bytes());
    let table_crc = crc32(&out[HEADER_LEN..HEADER_LEN + ENTRY_LEN * sections.len()]);
    out[80..84].copy_from_slice(&table_crc.to_le_bytes());
    let header_crc = crc32(&out[0..124]);
    out[124..128].copy_from_slice(&header_crc.to_le_bytes());
    Ok(out)
}

struct Entry {
    kind: u32,
    flags: u32,
    instance: u32,
    schema_id: u32,
    schema_version: u32,
    offset: u64,
    len: u64,
}

/// Parses and verifies a container. Order of checks: header (magic, version,
/// length, CRC, reserved, flags, epoch), limits, identity, total length, table
/// CRC, entries (reserved, limits, bounds, epoch, duplicates), layout (no
/// overlap, no gaps), payload CRCs and reference shape, unknown kinds, required
/// sections. Nothing is skipped silently except unknown optional sections,
/// which are listed in [`Container::skipped_optional`].
pub fn read_container<'a>(
    bytes: &'a [u8],
    expect: &Expect<'_>,
) -> Result<Container<'a>, ContainerError> {
    if bytes.len() < HEADER_LEN {
        return Err(ContainerError::OutOfBounds { what: "header" });
    }
    if bytes[0..8] != MAGIC {
        return Err(ContainerError::BadMagic);
    }
    let version = rd_u32(bytes, 8);
    if version != VERSION {
        return Err(ContainerError::BadVersion { found: version });
    }
    let header_len = rd_u32(bytes, 12);
    if header_len as usize != HEADER_LEN {
        return Err(ContainerError::BadHeaderLen { found: header_len });
    }
    if crc32(&bytes[0..124]) != rd_u32(bytes, 124) {
        return Err(ContainerError::HeaderCrc);
    }
    if !all_zero(&bytes[84..124]) {
        return Err(ContainerError::ReservedNonZero {
            place: Place::Header,
        });
    }
    let mandatory_flags = rd_u64(bytes, 16);
    if mandatory_flags != 0 {
        return Err(ContainerError::UnsupportedHeaderFlags {
            mandatory: mandatory_flags,
        });
    }
    // Optional header flags (bytes 24..32) are not defined in v1 and ignored.
    let mut vm_identity = [0u8; 16];
    vm_identity.copy_from_slice(&bytes[32..48]);
    let header = Header {
        vm_identity,
        checkpoint_id: rd_u64(bytes, 48),
        epoch: rd_u64(bytes, 56),
        profile: rd_u32(bytes, 64),
    };
    if !valid_epoch(header.epoch) {
        return Err(ContainerError::BadEpoch {
            epoch: header.epoch,
        });
    }
    let count = rd_u32(bytes, 68);
    let total_len = rd_u64(bytes, 72);
    if count > MAX_SECTIONS {
        return Err(ContainerError::LimitExceeded {
            limit: Limit::Sections,
            value: u64::from(count),
            max: u64::from(MAX_SECTIONS),
        });
    }
    if total_len > MAX_TOTAL_BYTES {
        return Err(ContainerError::LimitExceeded {
            limit: Limit::TotalBytes,
            value: total_len,
            max: MAX_TOTAL_BYTES,
        });
    }
    if header.vm_identity != expect.vm_identity {
        return Err(ContainerError::IdentityMismatch {
            field: IdentityField::VmIdentity,
        });
    }
    if header.checkpoint_id != expect.checkpoint_id {
        return Err(ContainerError::IdentityMismatch {
            field: IdentityField::CheckpointId,
        });
    }
    if header.profile != expect.profile {
        return Err(ContainerError::IdentityMismatch {
            field: IdentityField::Profile,
        });
    }
    if total_len != bytes.len() as u64 {
        return Err(ContainerError::OutOfBounds { what: "total_len" });
    }
    let count = count as usize;
    let table_end = HEADER_LEN + ENTRY_LEN * count;
    if (table_end as u64) > total_len {
        return Err(ContainerError::OutOfBounds { what: "table" });
    }
    let table = &bytes[HEADER_LEN..table_end];
    if crc32(table) != rd_u32(bytes, 80) {
        return Err(ContainerError::TableCrc);
    }

    let mut entries = Vec::with_capacity(count);
    let mut crcs = Vec::with_capacity(count);
    for i in 0..count {
        let e = &table[i * ENTRY_LEN..(i + 1) * ENTRY_LEN];
        let kind = rd_u32(e, 0);
        let flags = rd_u32(e, 4);
        let instance = rd_u32(e, 8);
        let place = Place::Entry(i as u32);
        if flags & !KNOWN_FLAGS != 0 || !all_zero(&e[48..64]) {
            return Err(ContainerError::ReservedNonZero { place });
        }
        let offset = rd_u64(e, 20);
        let len = rd_u64(e, 28);
        if len > MAX_SECTION_BYTES {
            return Err(ContainerError::LimitExceeded {
                limit: Limit::SectionBytes,
                value: len,
                max: MAX_SECTION_BYTES,
            });
        }
        if len == 0 {
            return Err(ContainerError::EmptySection { kind, instance });
        }
        let end = offset
            .checked_add(len)
            .ok_or(ContainerError::Overflow { kind, instance })?;
        if end > total_len {
            return Err(ContainerError::OutOfBounds { what: "section" });
        }
        let epoch = rd_u64(e, 36);
        if epoch != header.epoch {
            return Err(ContainerError::EpochMismatch {
                kind,
                instance,
                found: epoch,
                expected: header.epoch,
            });
        }
        entries.push(Entry {
            kind,
            flags,
            instance,
            schema_id: rd_u32(e, 12),
            schema_version: rd_u32(e, 16),
            offset,
            len,
        });
        crcs.push(rd_u32(e, 44));
    }

    let mut by_key: Vec<usize> = (0..count).collect();
    by_key.sort_by_key(|&i| (entries[i].kind, entries[i].instance));
    for w in by_key.windows(2) {
        let (a, b) = (&entries[w[0]], &entries[w[1]]);
        if (a.kind, a.instance) == (b.kind, b.instance) {
            return Err(ContainerError::DuplicateSection {
                kind: a.kind,
                instance: a.instance,
            });
        }
    }

    let mut by_offset: Vec<usize> = (0..count).collect();
    by_offset.sort_by_key(|&i| entries[i].offset);
    let mut next = table_end as u64;
    for &i in &by_offset {
        let e = &entries[i];
        if e.offset < next {
            return Err(ContainerError::Overlap {
                kind: e.kind,
                instance: e.instance,
            });
        }
        if e.offset > next {
            return Err(ContainerError::Gap { offset: next });
        }
        next = e.offset + e.len;
    }
    if next != total_len {
        return Err(ContainerError::Gap { offset: next });
    }

    let mut datas: Vec<SectionData<'a>> = Vec::with_capacity(count);
    for (e, &crc) in entries.iter().zip(&crcs) {
        let payload = &bytes[e.offset as usize..(e.offset + e.len) as usize];
        if crc32(payload) != crc {
            return Err(ContainerError::SectionCrc {
                kind: e.kind,
                instance: e.instance,
            });
        }
        if e.flags & FLAG_REFERENCE == 0 {
            datas.push(SectionData::Inline(payload));
            continue;
        }
        if payload.len() != REFERENCE_LEN {
            return Err(ContainerError::BadReferenceLen {
                kind: e.kind,
                instance: e.instance,
                len: e.len,
            });
        }
        if !all_zero(&payload[36..40]) {
            return Err(ContainerError::ReservedNonZero {
                place: Place::Reference {
                    kind: e.kind,
                    instance: e.instance,
                },
            });
        }
        let mut file_uid = [0u8; 16];
        file_uid.copy_from_slice(&payload[0..16]);
        let r = ReferenceId {
            file_uid,
            generation: rd_u64(payload, 16),
            byte_len: rd_u64(payload, 24),
            crc32: rd_u32(payload, 32),
        };
        r.validate(e.kind, e.instance)?;
        datas.push(SectionData::Reference(r));
    }

    let known = |kind: u32| {
        expect.known_kinds.contains(&kind) || expect.required.iter().any(|&(k, _)| k == kind)
    };
    let mut sections = Vec::new();
    let mut skipped_optional = Vec::new();
    for &i in &by_key {
        let e = &entries[i];
        let mandatory = e.flags & FLAG_MANDATORY != 0;
        if !known(e.kind) {
            if mandatory {
                return Err(ContainerError::UnknownMandatory {
                    kind: e.kind,
                    instance: e.instance,
                });
            }
            skipped_optional.push((e.kind, e.instance));
            continue;
        }
        sections.push(Section {
            kind: e.kind,
            instance: e.instance,
            schema_id: e.schema_id,
            schema_version: e.schema_version,
            mandatory,
            data: datas[i],
        });
    }
    check_required(expect.required, |k, i| {
        sections.iter().any(|s| s.kind == k && s.instance == i)
    })?;
    Ok(Container {
        header,
        sections,
        skipped_optional,
    })
}
