//! Tests for the full-VM container codec (S9-23). Field offsets are written
//! out as literals on purpose: they are the layout under test, not derived from
//! the codec.

use crate::crc32::crc32;
use crate::vm_container::*;

const ID: [u8; 16] = [0xA1; 16];
const CKPT: u64 = 7;
const EPOCH: u64 = 5;
const PROFILE: u32 = 3;
const HDR: Header = Header {
    vm_identity: ID,
    checkpoint_id: CKPT,
    epoch: EPOCH,
    profile: PROFILE,
};
const REQ: [(u32, u32); 4] = [(1, 0), (2, 0), (3, 0), (3, 1)];
const CPU: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8];
const DEV0: &[u8] = &[0xD0, 0xD1, 0xD2, 0xD3];
const DEV1: &[u8] = &[0xE0, 0xE1, 0xE2, 0xE3, 0xE4];
const EXTRA: &[u8] = &[0x55; 10];

fn refid() -> ReferenceId {
    ReferenceId {
        file_uid: [9; 16],
        generation: 3,
        byte_len: 4096,
        crc32: 0xDEAD_BEEF,
    }
}

fn inline(kind: u32, instance: u32, mandatory: bool, body: &[u8]) -> SectionInput<'_> {
    SectionInput {
        kind,
        instance,
        schema_id: 10 + kind,
        schema_version: 2,
        mandatory,
        body: SectionBody::Inline(body),
    }
}

fn reference(kind: u32, instance: u32, r: ReferenceId) -> SectionInput<'static> {
    SectionInput {
        kind,
        instance,
        schema_id: 10 + kind,
        schema_version: 1,
        mandatory: true,
        body: SectionBody::Reference(r),
    }
}

/// Sorted layout: (1,0) cpu, (2,0) ram ref, (3,0), (3,1), (100,0) optional.
fn sample_sections() -> Vec<SectionInput<'static>> {
    vec![
        inline(1, 0, true, CPU),
        reference(2, 0, refid()),
        inline(3, 0, true, DEV0),
        inline(3, 1, true, DEV1),
        inline(100, 0, false, EXTRA),
    ]
}

fn sample() -> Vec<u8> {
    write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &sample_sections(),
    )
    .unwrap()
}

fn expect<'a>(required: &'a [(u32, u32)], known: &'a [u32]) -> Expect<'a> {
    Expect {
        vm_identity: ID,
        checkpoint_id: CKPT,
        profile: PROFILE,
        required,
        known_kinds: known,
    }
}

fn read(b: &[u8]) -> Result<Container<'_>, ContainerError> {
    read_container(b, &expect(&REQ, &[100]))
}

fn read_err(b: &[u8]) -> ContainerError {
    read(b).unwrap_err()
}

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    b[off..off + v.len()].copy_from_slice(v);
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn e(i: usize) -> usize {
    128 + 64 * i
}

fn refresh_header(b: &mut [u8]) {
    let c = crc32(&b[0..124]);
    put(b, 124, &c.to_le_bytes());
}

fn refresh_table(b: &mut [u8]) {
    let n = u32_at(b, 68) as usize;
    let c = crc32(&b[128..128 + 64 * n]);
    put(b, 80, &c.to_le_bytes());
    refresh_header(b);
}

/// Payload byte range of entry i, read from the table.
fn payload_range(b: &[u8], i: usize) -> (usize, usize) {
    let off = u64_at(b, e(i) + 20) as usize;
    (off, off + u64_at(b, e(i) + 28) as usize)
}

// ---------- roundtrip, layout, determinism ----------

#[test]
fn roundtrip_opaque_payloads() {
    let b = sample();
    let c = read(&b).unwrap();
    assert_eq!(c.header, HDR);
    assert!(c.skipped_optional.is_empty());
    assert_eq!(c.sections.len(), 5);
    let s = |k, i| *c.section(k, i).unwrap();
    assert_eq!(s(1, 0).data, SectionData::Inline(CPU));
    assert_eq!((s(1, 0).schema_id, s(1, 0).schema_version), (11, 2));
    assert!(s(1, 0).mandatory);
    assert_eq!(s(2, 0).data, SectionData::Reference(refid()));
    assert_eq!(s(3, 0).data, SectionData::Inline(DEV0));
    assert_eq!(s(3, 1).data, SectionData::Inline(DEV1));
    assert_eq!(s(100, 0).data, SectionData::Inline(EXTRA));
    assert!(!s(100, 0).mandatory);
}

#[test]
fn layout_matches_the_specification() {
    let b = sample();
    assert_eq!(&b[0..8], b"ODVMCK\0\x01");
    assert_eq!(u32_at(&b, 8), 1);
    assert_eq!(u32_at(&b, 12), 128);
    assert_eq!(u64_at(&b, 16), 0);
    assert_eq!(&b[32..48], &ID);
    assert_eq!(u64_at(&b, 48), CKPT);
    assert_eq!(u64_at(&b, 56), EPOCH);
    assert_eq!(u32_at(&b, 64), PROFILE);
    assert_eq!(u32_at(&b, 68), 5);
    assert_eq!(u64_at(&b, 72), b.len() as u64);
    assert!(b[84..124].iter().all(|&x| x == 0));
    assert_eq!(u32_at(&b, 80), crc32(&b[128..128 + 5 * 64]));
    assert_eq!(u32_at(&b, 124), crc32(&b[0..124]));
    // entry 0: cpu, mandatory inline, first payload right after the table
    assert_eq!(u32_at(&b, e(0)), 1);
    assert_eq!(u32_at(&b, e(0) + 4), 1);
    assert_eq!(u32_at(&b, e(0) + 8), 0);
    assert_eq!(u64_at(&b, e(0) + 20), 128 + 5 * 64);
    assert_eq!(u64_at(&b, e(0) + 28), 8);
    assert_eq!(u64_at(&b, e(0) + 36), EPOCH);
    assert_eq!(u32_at(&b, e(0) + 44), crc32(CPU));
    assert!(b[e(0) + 48..e(0) + 64].iter().all(|&x| x == 0));
    // entry 1: reference, mandatory, exactly 40 bytes, no path anywhere
    assert_eq!(u32_at(&b, e(1) + 4), 3);
    assert_eq!(u64_at(&b, e(1) + 28), 40);
    // entry 4: optional inline
    assert_eq!(u32_at(&b, e(4) + 4), 0);
    let (s, t) = payload_range(&b, 1);
    assert_eq!(&b[s..s + 16], &[9u8; 16]);
    assert_eq!(u64_at(&b, s + 16), 3);
    assert_eq!(u64_at(&b, s + 24), 4096);
    assert_eq!(u32_at(&b, s + 32), 0xDEAD_BEEF);
    assert_eq!(&b[s + 36..t], &[0u8; 4]);
    assert_eq!(b.len(), 128 + 5 * 64 + 8 + 40 + 4 + 5 + 10);
}

#[test]
fn output_is_deterministic_and_independent_of_input_order() {
    let a = sample();
    assert_eq!(a, sample());
    let mut rev = sample_sections();
    rev.reverse();
    let b = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &rev,
    )
    .unwrap();
    assert_eq!(a, b);
    // A non-trivial permutation of the same set gives the same bytes too.
    let s = sample_sections();
    let perm = [s[3], s[0], s[4], s[2], s[1]];
    let c = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &perm,
    )
    .unwrap();
    assert_eq!(a, c);
    // The reader returns the sections in (kind, instance) order.
    let keys: Vec<(u32, u32)> = read(&c)
        .unwrap()
        .sections
        .iter()
        .map(|x| (x.kind, x.instance))
        .collect();
    assert_eq!(keys, [(1, 0), (2, 0), (3, 0), (3, 1), (100, 0)]);
}

#[test]
fn reader_accepts_table_entries_in_any_order() {
    let req = [(1, 0), (3, 0)];
    let secs = [inline(1, 0, true, CPU), inline(3, 0, true, DEV0)];
    let canonical = write_container(
        &WriteSpec {
            header: HDR,
            required: &req,
        },
        &secs,
    )
    .unwrap();
    // Swap the two table entries together with their payloads: the table lists
    // (3,0) first and its payload comes first; offsets are fixed up.
    let table_end = 128 + 2 * 64;
    let mut b = canonical[0..table_end].to_vec();
    let entry0 = canonical[e(0)..e(0) + 64].to_vec();
    let entry1 = canonical[e(1)..e(1) + 64].to_vec();
    put(&mut b, e(0), &entry1);
    put(&mut b, e(1), &entry0);
    put(&mut b, e(0) + 20, &(table_end as u64).to_le_bytes());
    put(
        &mut b,
        e(1) + 20,
        &((table_end + DEV0.len()) as u64).to_le_bytes(),
    );
    b.extend_from_slice(DEV0);
    b.extend_from_slice(CPU);
    refresh_table(&mut b);
    assert_ne!(b, canonical);
    assert_eq!(u32_at(&b, e(0)), 3);
    assert_eq!(u32_at(&b, e(1)), 1);

    let c = read_container(&b, &expect(&req, &[])).unwrap();
    let keys: Vec<(u32, u32)> = c.sections.iter().map(|s| (s.kind, s.instance)).collect();
    assert_eq!(keys, [(1, 0), (3, 0)]);
    assert_eq!(c.sections[0].data, SectionData::Inline(CPU));
    assert_eq!(c.sections[1].data, SectionData::Inline(DEV0));
    assert!(c.skipped_optional.is_empty());
}

#[test]
fn writer_adds_no_sections_and_no_zero_fill() {
    // Only what the caller supplied is present; the container has exactly the
    // supplied bytes and nothing else (checked by total length above too).
    let only = [inline(1, 0, true, CPU)];
    let b = write_container(
        &WriteSpec {
            header: HDR,
            required: &[(1, 0)],
        },
        &only,
    )
    .unwrap();
    assert_eq!(u32_at(&b, 68), 1);
    assert_eq!(b.len(), 128 + 64 + CPU.len());
    assert_eq!(&b[b.len() - CPU.len()..], CPU);
}

// ---------- required, duplicate, unknown ----------

#[test]
fn writer_rejects_missing_required_section() {
    let mut s = sample_sections();
    s.remove(3); // (3,1)
    let err = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &s,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ContainerError::MissingRequiredSection {
            kind: 3,
            instance: 1
        }
    );
}

#[test]
fn writer_rejects_required_section_that_is_not_mandatory() {
    let mut s = sample_sections();
    s[2] = inline(3, 0, false, DEV0); // (3,0) is required but optional
    let err = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &s,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ContainerError::RequiredNotMandatory {
            kind: 3,
            instance: 0
        }
    );
    // A non-required optional section is fine.
    assert!(!sample_sections()[4].mandatory);
    sample();
}

#[test]
fn reader_requires_the_declared_sections() {
    let mut s = sample_sections();
    s.remove(3);
    let b = write_container(
        &WriteSpec {
            header: HDR,
            required: &[],
        },
        &s,
    )
    .unwrap();
    assert_eq!(
        read_err(&b),
        ContainerError::MissingRequiredSection {
            kind: 3,
            instance: 1
        }
    );
    // A whole required kind absent.
    let only = [inline(1, 0, true, CPU)];
    let b = write_container(
        &WriteSpec {
            header: HDR,
            required: &[],
        },
        &only,
    )
    .unwrap();
    assert_eq!(
        read_err(&b),
        ContainerError::MissingRequiredSection {
            kind: 2,
            instance: 0
        }
    );
}

#[test]
fn duplicate_kind_instance_is_rejected_by_writer_and_reader() {
    let mut s = sample_sections();
    s.push(inline(3, 0, true, DEV1));
    let err = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &s,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ContainerError::DuplicateSection {
            kind: 3,
            instance: 0
        }
    );
    // Same kind with a different instance is not a duplicate (sample has it);
    // forge a duplicate on disk: entry 3 (3,1) becomes (3,0).
    let mut b = sample();
    put(&mut b, e(3) + 8, &0u32.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::DuplicateSection {
            kind: 3,
            instance: 0
        }
    );
}

#[test]
fn unknown_mandatory_section_is_rejected() {
    let mut s = sample_sections();
    s.push(inline(200, 0, true, EXTRA));
    let b = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &s,
    )
    .unwrap();
    assert_eq!(
        read_err(&b),
        ContainerError::UnknownMandatory {
            kind: 200,
            instance: 0
        }
    );
    // Once the caller knows the kind it is accepted.
    let c = read_container(&b, &expect(&REQ, &[100, 200])).unwrap();
    assert_eq!(c.section(200, 0).unwrap().data, SectionData::Inline(EXTRA));
}

#[test]
fn unknown_optional_section_is_skipped_and_reported() {
    let b = sample();
    let c = read_container(&b, &expect(&REQ, &[])).unwrap();
    assert_eq!(c.skipped_optional, vec![(100, 0)]);
    assert_eq!(c.sections.len(), 4);
    assert!(c.section(100, 0).is_none());
}

#[test]
fn unknown_optional_section_is_still_integrity_checked() {
    let mut b = sample();
    let (_, end) = payload_range(&b, 4);
    b[end - 1] ^= 0xFF;
    let err = read_container(&b, &expect(&REQ, &[])).unwrap_err();
    assert_eq!(
        err,
        ContainerError::SectionCrc {
            kind: 100,
            instance: 0
        }
    );
}

// ---------- header: magic, version, CRCs ----------

#[test]
fn bad_magic() {
    let mut b = sample();
    b[0] ^= 1;
    assert_eq!(read_err(&b), ContainerError::BadMagic);
    let mut b = sample();
    b[7] = 2; // format byte
    assert_eq!(read_err(&b), ContainerError::BadMagic);
}

#[test]
fn bad_version_and_header_len() {
    let mut b = sample();
    put(&mut b, 8, &2u32.to_le_bytes());
    assert_eq!(read_err(&b), ContainerError::BadVersion { found: 2 });
    refresh_header(&mut b);
    assert_eq!(read_err(&b), ContainerError::BadVersion { found: 2 });
    let mut b = sample();
    put(&mut b, 12, &256u32.to_le_bytes());
    refresh_header(&mut b);
    assert_eq!(read_err(&b), ContainerError::BadHeaderLen { found: 256 });
}

#[test]
fn header_crc_detects_corruption() {
    let mut b = sample();
    b[48] ^= 1; // checkpoint_id, CRC not refreshed
    assert_eq!(read_err(&b), ContainerError::HeaderCrc);
    let mut b = sample();
    b[124] ^= 1; // the CRC itself
    assert_eq!(read_err(&b), ContainerError::HeaderCrc);
}

#[test]
fn table_crc_detects_corruption() {
    let mut b = sample();
    b[e(2) + 12] ^= 1; // schema_id of an entry, CRC not refreshed
    assert_eq!(read_err(&b), ContainerError::TableCrc);
}

#[test]
fn section_crc_detects_corruption() {
    let mut b = sample();
    let (s, _) = payload_range(&b, 0);
    b[s] ^= 0x80;
    assert_eq!(
        read_err(&b),
        ContainerError::SectionCrc {
            kind: 1,
            instance: 0
        }
    );
    let mut b = sample();
    let (_, t) = payload_range(&b, 3);
    b[t - 1] ^= 1;
    assert_eq!(
        read_err(&b),
        ContainerError::SectionCrc {
            kind: 3,
            instance: 1
        }
    );
}

// ---------- reserved and flags ----------

#[test]
fn reserved_bytes_must_be_zero() {
    let mut b = sample();
    b[100] = 1;
    refresh_header(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::ReservedNonZero {
            place: Place::Header
        }
    );
    let mut b = sample();
    b[e(2) + 63] = 1;
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::ReservedNonZero {
            place: Place::Entry(2)
        }
    );
    let mut b = sample();
    put(&mut b, e(1) + 4, &(3u32 | 4).to_le_bytes()); // unknown flag bit
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::ReservedNonZero {
            place: Place::Entry(1)
        }
    );
    // Reserved tail of a reference payload; entry CRC kept consistent.
    let mut b = sample();
    let (s, t) = payload_range(&b, 1);
    b[t - 1] = 1;
    let c = crc32(&b[s..t]);
    put(&mut b, e(1) + 44, &c.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::ReservedNonZero {
            place: Place::Reference {
                kind: 2,
                instance: 0
            }
        }
    );
}

#[test]
fn nonzero_mandatory_header_flags_are_unsupported_optional_are_ignored() {
    let mut b = sample();
    put(&mut b, 16, &1u64.to_le_bytes());
    refresh_header(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::UnsupportedHeaderFlags { mandatory: 1 }
    );
    let mut b = sample();
    put(&mut b, 24, &0xFFu64.to_le_bytes());
    refresh_header(&mut b);
    assert!(read(&b).is_ok());
}

// ---------- layout: overlap, bounds, overflow, gaps ----------

#[test]
fn overlapping_sections_are_rejected() {
    let mut b = sample();
    let off0 = b[e(0) + 20..e(0) + 28].to_vec();
    put(&mut b, e(1) + 20, &off0); // ram ref starts where cpu starts
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::Overlap {
            kind: 2,
            instance: 0
        }
    );
    // Payload pointing into the table itself.
    let mut b = sample();
    put(&mut b, e(0) + 20, &130u64.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::Overlap {
            kind: 1,
            instance: 0
        }
    );
}

#[test]
fn section_past_the_end_is_out_of_bounds() {
    let mut b = sample();
    let len = u64_at(&b, e(4) + 28);
    put(&mut b, e(4) + 28, &(len + 1).to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::OutOfBounds { what: "section" }
    );
    let mut b = sample();
    put(&mut b, e(4) + 20, &(1u64 << 40).to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::OutOfBounds { what: "section" }
    );
}

#[test]
fn offset_plus_len_overflow_is_rejected() {
    let mut b = sample();
    put(&mut b, e(2) + 20, &u64::MAX.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::Overflow {
            kind: 3,
            instance: 0
        }
    );
}

#[test]
fn uncovered_bytes_are_rejected() {
    // Trailing bytes inside total_len that no section covers.
    let mut b = sample();
    let len = u64_at(&b, e(4) + 28);
    put(&mut b, e(4) + 28, &(len - 1).to_le_bytes());
    refresh_table(&mut b);
    let end = b.len() as u64 - 1;
    assert_eq!(read_err(&b), ContainerError::Gap { offset: end });
    // Hole between two sections.
    let mut b = sample();
    let off = u64_at(&b, e(3) + 20);
    put(&mut b, e(3) + 20, &(off + 1).to_le_bytes());
    refresh_table(&mut b);
    // Entry 3 now also ends one byte too late; the hole is found first, at the
    // end of entry 2 (the old start of entry 3).
    assert_eq!(read_err(&b), ContainerError::Gap { offset: off });
}

#[test]
fn file_length_must_equal_total_len() {
    let b = sample();
    assert_eq!(
        read_err(&b[..b.len() - 1]),
        ContainerError::OutOfBounds { what: "total_len" }
    );
    let mut longer = b.clone();
    longer.push(0);
    assert_eq!(
        read_err(&longer),
        ContainerError::OutOfBounds { what: "total_len" }
    );
    assert_eq!(
        read_err(&b[..100]),
        ContainerError::OutOfBounds { what: "header" }
    );
    // Header claims more sections than the file holds.
    let mut b = sample();
    let total = b.len() as u64;
    put(&mut b, 68, &200u32.to_le_bytes());
    refresh_header(&mut b);
    assert_eq!(total, u64_at(&b, 72));
    assert_eq!(read_err(&b), ContainerError::OutOfBounds { what: "table" });
}

#[test]
fn empty_section_is_rejected() {
    let mut b = sample();
    put(&mut b, e(4) + 28, &0u64.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::EmptySection {
            kind: 100,
            instance: 0
        }
    );
    let err = write_container(
        &WriteSpec {
            header: HDR,
            required: &[],
        },
        &[inline(1, 0, true, &[])],
    )
    .unwrap_err();
    assert_eq!(
        err,
        ContainerError::EmptySection {
            kind: 1,
            instance: 0
        }
    );
}

// ---------- limits ----------

#[test]
fn reader_limits_are_checked_before_any_table_or_payload_is_used() {
    // Only 128 header bytes exist: if the count were used to size or read a
    // table, the result would be OutOfBounds, not LimitExceeded.
    let mut h = sample()[..128].to_vec();
    put(&mut h, 68, &u32::MAX.to_le_bytes());
    refresh_header(&mut h);
    assert_eq!(
        read_err(&h),
        ContainerError::LimitExceeded {
            limit: Limit::Sections,
            value: u64::from(u32::MAX),
            max: 256
        }
    );
    let mut h = sample()[..128].to_vec();
    put(&mut h, 68, &(MAX_SECTIONS + 1).to_le_bytes());
    refresh_header(&mut h);
    assert!(matches!(
        read_err(&h),
        ContainerError::LimitExceeded {
            limit: Limit::Sections,
            ..
        }
    ));
    let mut h = sample()[..128].to_vec();
    put(&mut h, 72, &(MAX_TOTAL_BYTES + 1).to_le_bytes());
    refresh_header(&mut h);
    assert_eq!(
        read_err(&h),
        ContainerError::LimitExceeded {
            limit: Limit::TotalBytes,
            value: MAX_TOTAL_BYTES + 1,
            max: MAX_TOTAL_BYTES
        }
    );
    // A section length above the limit, with the payload far out of bounds.
    let mut b = sample();
    put(&mut b, e(4) + 28, &(MAX_SECTION_BYTES + 1).to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::LimitExceeded {
            limit: Limit::SectionBytes,
            value: MAX_SECTION_BYTES + 1,
            max: MAX_SECTION_BYTES
        }
    );
}

#[test]
fn writer_enforces_its_limits_on_the_inputs() {
    let spec = WriteSpec {
        header: HDR,
        required: &[],
    };
    let many: Vec<SectionInput<'_>> = (0..=MAX_SECTIONS)
        .map(|k| inline(k, 0, true, &[1]))
        .collect();
    assert_eq!(many.len(), 257);
    assert!(matches!(
        write_container(&spec, &many).unwrap_err(),
        ContainerError::LimitExceeded {
            limit: Limit::Sections,
            value: 257,
            max: 256
        }
    ));
    // The test only observes that the writer refuses with LimitExceeded; it does
    // not (and cannot) observe when the output buffer would be allocated.
    let big = vec![0u8; MAX_SECTION_BYTES as usize + 1];
    assert!(matches!(
        write_container(&spec, &[inline(1, 0, true, &big)]).unwrap_err(),
        ContainerError::LimitExceeded {
            limit: Limit::SectionBytes,
            ..
        }
    ));
    // Four maximal sections (one shared buffer) exceed the total limit.
    let max = &big[..MAX_SECTION_BYTES as usize];
    let four: Vec<SectionInput<'_>> = (0..4).map(|k| inline(k, 0, true, max)).collect();
    assert!(matches!(
        write_container(&spec, &four).unwrap_err(),
        ContainerError::LimitExceeded {
            limit: Limit::TotalBytes,
            ..
        }
    ));
}

// ---------- epoch and identity ----------

#[test]
fn section_epoch_must_equal_header_epoch() {
    let mut b = sample();
    put(&mut b, e(1) + 36, &6u64.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::EpochMismatch {
            kind: 2,
            instance: 0,
            found: 6,
            expected: 5
        }
    );
}

#[test]
fn header_epoch_zero_and_max_are_invalid() {
    for bad in [0u64, u64::MAX] {
        let mut h = HDR;
        h.epoch = bad;
        let err = write_container(
            &WriteSpec {
                header: h,
                required: &[],
            },
            &[inline(1, 0, true, CPU)],
        )
        .unwrap_err();
        assert_eq!(err, ContainerError::BadEpoch { epoch: bad });
        let mut b = sample();
        put(&mut b, 56, &bad.to_le_bytes());
        refresh_header(&mut b);
        assert_eq!(read_err(&b), ContainerError::BadEpoch { epoch: bad });
    }
}

#[test]
fn identity_checkpoint_and_profile_must_match() {
    let b = sample();
    let mut x = expect(&REQ, &[100]);
    x.vm_identity = [0xA2; 16];
    assert_eq!(
        read_container(&b, &x).unwrap_err(),
        ContainerError::IdentityMismatch {
            field: IdentityField::VmIdentity
        }
    );
    let mut x = expect(&REQ, &[100]);
    x.checkpoint_id = CKPT + 1;
    assert_eq!(
        read_container(&b, &x).unwrap_err(),
        ContainerError::IdentityMismatch {
            field: IdentityField::CheckpointId
        }
    );
    let mut x = expect(&REQ, &[100]);
    x.profile = PROFILE + 1;
    assert_eq!(
        read_container(&b, &x).unwrap_err(),
        ContainerError::IdentityMismatch {
            field: IdentityField::Profile
        }
    );
}

// ---------- references ----------

#[test]
fn reference_with_wrong_size_is_rejected() {
    // An inline section of 10 bytes relabelled as a reference.
    let mut s = sample_sections();
    s.push(inline(50, 0, false, EXTRA));
    let mut b = write_container(
        &WriteSpec {
            header: HDR,
            required: &REQ,
        },
        &s,
    )
    .unwrap();
    // Sorted position of (50,0): after (3,1), before (100,0).
    assert_eq!(u32_at(&b, e(4)), 50);
    put(&mut b, e(4) + 4, &FLAG_REFERENCE.to_le_bytes());
    refresh_table(&mut b);
    let err = read_container(&b, &expect(&REQ, &[50, 100])).unwrap_err();
    assert_eq!(
        err,
        ContainerError::BadReferenceLen {
            kind: 50,
            instance: 0,
            len: 10
        }
    );
}

#[test]
fn reference_without_identity_is_rejected() {
    let mut zero = refid();
    zero.file_uid = [0; 16];
    let err = write_container(
        &WriteSpec {
            header: HDR,
            required: &[],
        },
        &[reference(2, 0, zero)],
    )
    .unwrap_err();
    assert_eq!(
        err,
        ContainerError::BadReference {
            kind: 2,
            instance: 0
        }
    );
    let mut empty = refid();
    empty.byte_len = 0;
    assert!(matches!(
        write_container(
            &WriteSpec {
                header: HDR,
                required: &[]
            },
            &[reference(2, 0, empty)]
        )
        .unwrap_err(),
        ContainerError::BadReference { .. }
    ));
    // Forged on disk: zero the file_uid, keep CRCs consistent.
    let mut b = sample();
    let (s, t) = payload_range(&b, 1);
    b[s..s + 16].fill(0);
    let c = crc32(&b[s..t]);
    put(&mut b, e(1) + 44, &c.to_le_bytes());
    refresh_table(&mut b);
    assert_eq!(
        read_err(&b),
        ContainerError::BadReference {
            kind: 2,
            instance: 0
        }
    );
}

#[test]
fn reference_payload_is_exactly_40_bytes() {
    // The reference record is identity-only and fixed-size: file_uid, generation,
    // byte_len, crc32 and 4 reserved bytes make up exactly REFERENCE_LEN bytes.
    // The writer takes no path or handle input, so none can be stored here.
    let b = sample();
    let (s, t) = payload_range(&b, 1);
    assert_eq!(t - s, REFERENCE_LEN);
    assert_eq!(REFERENCE_LEN, 16 + 8 + 8 + 4 + 4);
}
