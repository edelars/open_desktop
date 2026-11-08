//! S4-31d: machine-side validator of the ACPI table blob.
//!
//! Two kinds of input: the committed fixture, which is the real output of the freestanding
//! firmware build (`tests/fixtures/guest/stage4/acpi_blob/`), and blobs assembled here byte by
//! byte so that every defect can be introduced alone.

use super::*;
use crate::{FirmwareBoardConfig, NvmeConfig, PCI_ECAM_DESCRIPTOR};

const FIXTURE: &[u8] =
    include_bytes!("../../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");

/// The board the fixture was built for: ECAM, PM block and the table manifest.
fn board() -> FirmwareBoardConfig {
    FirmwareBoardConfig {
        pci_ecam: true,
        acpi_pm: true,
        acpi_tables: true,
        ..FirmwareBoardConfig::default()
    }
}

fn board_hash() -> AcpiManifestHash {
    board().acpi_manifest_hash().unwrap()
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- the real blob -------------------------------------------------------------------------

#[test]
fn stage4_blob_is_built_for_this_boards_manifest() {
    // The header hash was computed by the firmware's const evaluation from its own copy of the
    // device table; the machine recomputes it from the real board configuration.
    let info = validate_acpi_blob(FIXTURE, &board_hash()).expect("fixture is a valid blob");
    assert_eq!(info.version, 1);
    assert_eq!(info.manifest_hash, board_hash());
    assert_eq!(
        info.load_range(),
        (0xE_0000, 0xE_0000 + info.payload_len as u64)
    );
    assert_eq!(info.load_base, ACPI_REGION_BASE);
    assert!(info.load_range().1 <= ACPI_RECLAIM_END);
    let signatures: Vec<&[u8; 4]> = info.tables.iter().map(|t| &t.signature).collect();
    assert_eq!(signatures, [b"DSDT", b"MCFG"]);
    assert_eq!(info.payload(FIXTURE).len(), info.payload_len);
}

#[test]
fn the_mcfg_in_the_blob_describes_the_boards_ecam_window() {
    let info = validate_acpi_blob(FIXTURE, &board_hash()).unwrap();
    let mcfg = info
        .tables
        .iter()
        .find(|t| &t.signature == b"MCFG")
        .unwrap();
    let bytes = info.table_bytes(FIXTURE, mcfg);
    // One allocation structure after the 44-byte MCFG header: base, segment, bus range.
    assert_eq!(bytes.len(), 60);
    assert_eq!(
        u64::from_le_bytes(bytes[44..52].try_into().unwrap()),
        PCI_ECAM_DESCRIPTOR.base
    );
    assert_eq!(
        u16::from_le_bytes(bytes[52..54].try_into().unwrap()),
        PCI_ECAM_DESCRIPTOR.segment
    );
    assert_eq!(
        (bytes[54], bytes[55]),
        (PCI_ECAM_DESCRIPTOR.start_bus, PCI_ECAM_DESCRIPTOR.end_bus)
    );
}

#[test]
fn a_blob_built_for_another_manifest_is_refused() {
    // Any other resolved device table has another hash, so a blob built earlier is foreign.
    let other_boards = [
        FirmwareBoardConfig {
            pci_ecam: false,
            ..board()
        },
        FirmwareBoardConfig {
            nvme: Some(NvmeConfig::default()),
            ..board()
        },
    ];
    for other in other_boards {
        let hash = other.acpi_manifest_hash().unwrap();
        assert_ne!(hash, board_hash());
        match validate_acpi_blob(FIXTURE, &hash) {
            Err(AcpiBlobError::ManifestMismatch { expected, found }) => {
                assert_eq!(expected, hash);
                assert_eq!(found, board_hash());
            }
            other => panic!("expected ManifestMismatch, got {other:?}"),
        }
    }
}

#[test]
fn changing_the_manifest_without_rebuilding_the_blob_is_a_hash_mismatch() {
    // The header carries the hash of the manifest it was built for. Editing the header to claim
    // another manifest (one bit) is the same defect seen from the other side.
    let mut tampered = FIXTURE.to_vec();
    tampered[0x10] ^= 1;
    assert!(matches!(
        validate_acpi_blob(&tampered, &board_hash()),
        Err(AcpiBlobError::ManifestMismatch { .. })
    ));
    // Hash and tables agree only for the manifest they were built from: the hash is a function
    // of the whole device table, hence of the ECAM window the MCFG describes.
    let mut moved = board().acpi_manifest_hash().unwrap();
    moved[31] ^= 0x80;
    assert!(validate_acpi_blob(FIXTURE, &moved).is_err());
}

#[test]
fn the_manifest_hash_follows_the_documented_construction() {
    // SHA-256(tag || entries ordered by type), over the 48-byte handoff encodings.
    let entry = |t: u32, flags: u32| {
        let mut raw = [0u8; 48];
        raw[0..4].copy_from_slice(&t.to_le_bytes());
        raw[4..8].copy_from_slice(&flags.to_le_bytes());
        raw
    };
    let (a, b) = (entry(3, 1), entry(7, 0));
    let mut message = ACPI_MANIFEST_HASH_TAG.to_vec();
    message.extend_from_slice(&a);
    message.extend_from_slice(&b);
    let expected = sha256(&message);
    assert_eq!(acpi_manifest_hash(&[a, b]).unwrap(), expected);
    assert_eq!(
        acpi_manifest_hash(&[b, a]).unwrap(),
        expected,
        "order is irrelevant"
    );
    assert_eq!(
        acpi_manifest_hash(&[a, a]),
        Err(AcpiBlobError::DuplicateDeviceType(3))
    );
    assert_eq!(
        acpi_manifest_hash(&[entry(16, 0)]),
        Err(AcpiBlobError::BadDeviceType(16))
    );
    assert_eq!(
        acpi_manifest_hash(&[entry(0, 0)]),
        Err(AcpiBlobError::BadDeviceType(0))
    );
}

#[test]
fn the_default_board_has_a_hash_too_and_flags_change_it() {
    let default = FirmwareBoardConfig::default().acpi_manifest_hash().unwrap();
    assert_ne!(default, board_hash());
    // An invalid configuration has no hash (the same check as the board loader).
    let invalid = FirmwareBoardConfig {
        acpi_tables: true,
        acpi_pm: false,
        ..FirmwareBoardConfig::default()
    };
    assert!(invalid.acpi_manifest_hash().is_err());
}

#[test]
fn sha256_matches_the_fips_vectors() {
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        )),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        hex(&sha256(
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
        )),
        "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
    );
    assert_eq!(
        hex(&sha256(&vec![b'a'; 1_000_000])),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

// ---- hand-made blobs -----------------------------------------------------------------------

const HASH: AcpiManifestHash = [0x5A; 32];

/// A System Description Table of `len` bytes with a correct length field and checksum.
fn sdt(signature: &[u8; 4], len: usize) -> Vec<u8> {
    let mut table = vec![0x11u8; len];
    table[..4].copy_from_slice(signature);
    put32(&mut table, 4, len as u32);
    table[9] = 0;
    let sum = table.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    table[9] = sum.wrapping_neg();
    table
}

#[derive(Clone)]
struct Spec {
    load_base: u64,
    /// (directory signature, payload offset, directory length, bytes)
    tables: Vec<([u8; 4], u32, u32, Vec<u8>)>,
}

impl Spec {
    fn good() -> Self {
        let dsdt = sdt(b"DSDT", 0x40);
        let mcfg = sdt(b"MCFG", 0x3C);
        Spec {
            load_base: 0xE_0000,
            tables: vec![(*b"DSDT", 0x40, 0x40, dsdt), (*b"MCFG", 0x80, 0x3C, mcfg)],
        }
    }

    fn build(&self) -> Vec<u8> {
        let count = self.tables.len();
        let payload_offset = (0x60 + count * 16).div_ceil(16) * 16;
        let payload_len = self
            .tables
            .iter()
            .map(|(_, offset, _, bytes)| *offset as usize + bytes.len())
            .max()
            .unwrap_or(0)
            .max(0x40);
        let mut blob = vec![0u8; payload_offset + payload_len];
        blob[..8].copy_from_slice(b"ODACPIB1");
        put32(&mut blob, 0x08, 1);
        put32(&mut blob, 0x0C, 0x60);
        blob[0x10..0x30].copy_from_slice(&HASH);
        put64(&mut blob, 0x30, self.load_base);
        put32(&mut blob, 0x38, payload_offset as u32);
        put32(&mut blob, 0x3C, payload_len as u32);
        put32(&mut blob, 0x48, count as u32);
        let total = blob.len() as u32;
        put32(&mut blob, 0x4C, total);
        for (index, (signature, offset, length, bytes)) in self.tables.iter().enumerate() {
            let at = 0x60 + index * 16;
            blob[at..at + 4].copy_from_slice(signature);
            put32(&mut blob, at + 4, *offset);
            put32(&mut blob, at + 8, *length);
            let start = payload_offset + *offset as usize;
            blob[start..start + bytes.len()].copy_from_slice(bytes);
        }
        blob
    }
}

fn check(spec: &Spec) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    validate_acpi_blob(&spec.build(), &HASH)
}

#[test]
fn a_correct_blob_is_accepted() {
    let info = check(&Spec::good()).unwrap();
    assert_eq!(info.load_range(), (0xE_0000, 0xE_0000 + 0xBC));
    assert_eq!(info.tables.len(), 2);
    // A single table (no ECAM board) and the smallest legal load base are fine too.
    let mut one = Spec::good();
    one.tables.truncate(1);
    one.load_base = 0xE_EF80;
    assert!(check(&one).is_ok());
}

#[test]
fn tables_that_overlap_are_refused() {
    let mut spec = Spec::good();
    spec.tables[1].1 = 0x70; // MCFG at 0x70 runs into the DSDT [0x40, 0x80)
    assert_eq!(
        check(&spec),
        Err(AcpiBlobError::TableOverlap {
            first: 0,
            second: 1
        })
    );
    // The same table twice at one offset overlaps too (signatures differ here).
    let mut same = Spec::good();
    same.tables[1].1 = 0x40;
    assert_eq!(
        check(&same),
        Err(AcpiBlobError::TableOverlap {
            first: 0,
            second: 1
        })
    );
    // Reported in directory indices even when the directory is not in offset order.
    let mut swapped = Spec::good();
    swapped.tables.reverse();
    swapped.tables[0].1 = 0x70;
    assert_eq!(
        check(&swapped),
        Err(AcpiBlobError::TableOverlap {
            first: 1,
            second: 0
        })
    );
}

#[test]
fn a_payload_outside_the_acpi_region_is_refused() {
    let len = 0xBCu64;
    for load_base in [
        0xD_FFF0u64,     // starts below the region
        0x0,             // far below
        0x10_0000,       // above the region
        0xF_0000 - 0x10, // starts inside, ends past 0xF_0000
        !0xF_u64,        // overflows when the length is added
    ] {
        let mut spec = Spec::good();
        spec.load_base = load_base;
        match check(&spec) {
            Err(AcpiBlobError::OutOfRegion { start, end }) => {
                assert_eq!(start, load_base);
                assert_eq!(end, load_base.checked_add(len));
            }
            other => panic!("load base {load_base:#x}: {other:?}"),
        }
    }
    // Inside the region but into the NVS part (FACS only): its own error.
    let mut nvs = Spec::good();
    nvs.load_base = 0xE_F000 - 0x50;
    assert_eq!(
        check(&nvs),
        Err(AcpiBlobError::PayloadIntoNvs {
            end: 0xE_F000 - 0x50 + len
        })
    );
    // The last byte below 0xE_F000 is allowed; one more byte is not.
    let mut edge = Spec::good();
    edge.load_base = 0xE_F000 - 0xC0;
    assert!(check(&edge).is_ok());
    edge.load_base += 0x10;
    assert!(matches!(
        check(&edge),
        Err(AcpiBlobError::PayloadIntoNvs { .. })
    ));
    // A misaligned base is refused as such.
    let mut odd = Spec::good();
    odd.load_base = 0xE_0008;
    assert_eq!(
        check(&odd),
        Err(AcpiBlobError::MisalignedLoadBase(0xE_0008))
    );
}

#[test]
fn a_bad_checksum_is_refused() {
    let mut spec = Spec::good();
    spec.tables[0].3[0x20] ^= 0x01;
    // One bit flipped in a byte that was 0x11: the sum is off by -1.
    assert_eq!(
        check(&spec),
        Err(AcpiBlobError::BadChecksum {
            index: 0,
            sum: 0xFF
        })
    );
    let mut mcfg = Spec::good();
    mcfg.tables[1].3[9] = mcfg.tables[1].3[9].wrapping_add(7);
    assert!(matches!(
        check(&mcfg),
        Err(AcpiBlobError::BadChecksum { index: 1, .. })
    ));
}

#[test]
fn a_wrong_length_is_refused_wherever_it_is_written() {
    // The table header says 0x44, the directory says 0x40 (checksum made right for 0x44).
    let mut header = Spec::good();
    let mut table = header.tables[0].3.clone();
    put32(&mut table, 4, 0x44);
    table[9] = 0;
    let sum = table.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    table[9] = sum.wrapping_neg();
    header.tables[0].3 = table;
    assert_eq!(
        check(&header),
        Err(AcpiBlobError::TableLengthMismatch {
            index: 0,
            directory: 0x40,
            header: 0x44
        })
    );
    // A directory length that runs past the payload.
    let mut long = Spec::good();
    long.tables[1].2 = 0x1000;
    assert!(check(&long).is_err());
    let mut payload = Spec::good();
    payload.tables[1].2 = 0xFFFF_FF00;
    assert_eq!(
        check(&payload),
        Err(AcpiBlobError::TableOutsidePayload { index: 1 })
    );
    // A table shorter than an SDT header.
    let mut short = Spec::good();
    short.tables[0].2 = 0x20;
    assert_eq!(
        check(&short),
        Err(AcpiBlobError::TableTooShort {
            index: 0,
            len: 0x20
        })
    );
    // The total length of the blob and the payload length.
    let blob = Spec::good().build();
    let mut total = blob.clone();
    put32(&mut total, 0x4C, blob.len() as u32 + 1);
    assert_eq!(
        validate_acpi_blob(&total, &HASH),
        Err(AcpiBlobError::LengthMismatch {
            declared: blob.len() as u32 + 1,
            actual: blob.len()
        })
    );
    let mut payload_len = blob.clone();
    put32(&mut payload_len, 0x3C, 0xB0);
    assert!(matches!(
        validate_acpi_blob(&payload_len, &HASH),
        Err(AcpiBlobError::BadPayload { .. })
    ));
    // A short blob is truncated, not a panic.
    for cut in [0, 8, 0x5F, 0x60] {
        assert!(
            validate_acpi_blob(&blob[..cut], &HASH).is_err(),
            "cut {cut}"
        );
    }
}

#[test]
fn header_and_directory_defects_are_refused() {
    let good = Spec::good().build();
    let with = |patch: &dyn Fn(&mut Vec<u8>)| {
        let mut blob = good.clone();
        patch(&mut blob);
        validate_acpi_blob(&blob, &HASH)
    };
    assert_eq!(with(&|b| b[0] = b'X'), Err(AcpiBlobError::BadMagic));
    assert_eq!(
        with(&|b| put32(b, 0x08, 2)),
        Err(AcpiBlobError::UnsupportedVersion(2))
    );
    assert_eq!(
        with(&|b| put32(b, 0x0C, 0x70)),
        Err(AcpiBlobError::BadHeaderLength(0x70))
    );
    assert_eq!(with(&|b| b[0x58] = 1), Err(AcpiBlobError::ReservedNotZero));
    assert_eq!(
        with(&|b| put32(b, 0x6C, 1)),
        Err(AcpiBlobError::ReservedNotZero)
    );
    assert_eq!(
        with(&|b| put64(b, 0x40, 0xE_0000)),
        Err(AcpiBlobError::RsdpNotSupported(0xE_0000))
    );
    assert_eq!(
        with(&|b| put32(b, 0x48, 0)),
        Err(AcpiBlobError::BadTableCount(0))
    );
    assert_eq!(
        with(&|b| put32(b, 0x48, 9)),
        Err(AcpiBlobError::BadTableCount(9))
    );
    // A table signature outside this slice (FADT arrives with S4-31e) and a repeated one.
    assert_eq!(
        with(&|b| b[0x60..0x64].copy_from_slice(b"FACP")),
        Err(AcpiBlobError::UnexpectedTable {
            signature: *b"FACP"
        })
    );
    assert_eq!(
        with(&|b| b[0x70..0x74].copy_from_slice(b"DSDT")),
        Err(AcpiBlobError::DuplicateTable {
            signature: *b"DSDT"
        })
    );
    // The signature inside the table must be the one of the directory.
    let mut renamed = Spec::good();
    renamed.tables[0].3[..4].copy_from_slice(b"SSDT");
    assert_eq!(
        check(&renamed),
        Err(AcpiBlobError::SignatureMismatch { index: 0 })
    );
    // Alignment, RSDP slot.
    let mut odd = Spec::good();
    odd.tables[1].1 = 0x84;
    assert_eq!(
        check(&odd),
        Err(AcpiBlobError::MisalignedTable {
            index: 1,
            offset: 0x84
        })
    );
    let mut slot = Spec::good();
    slot.tables[0].1 = 0x30;
    assert_eq!(
        check(&slot),
        Err(AcpiBlobError::TableInRsdpSlot {
            index: 0,
            offset: 0x30
        })
    );
}

#[test]
fn bytes_that_no_table_owns_must_be_zero() {
    let mut blob = Spec::good().build();
    let payload = le32(&blob, 0x38) as usize;
    // A byte in the empty RSDP slot (rsdp_address is 0, the slot stays empty in this slice).
    blob[payload + 0x10] = 1;
    assert_eq!(
        validate_acpi_blob(&blob, &HASH),
        Err(AcpiBlobError::NonZeroGap {
            payload_offset: 0x10
        })
    );
    let mut gap = Spec::good();
    gap.tables[1].1 = 0xA0; // gap [0x80, 0xA0)
    let mut blob = gap.build();
    let payload = le32(&blob, 0x38) as usize;
    assert!(validate_acpi_blob(&blob, &HASH).is_ok());
    blob[payload + 0x90] = 0xCC;
    assert_eq!(
        validate_acpi_blob(&blob, &HASH),
        Err(AcpiBlobError::NonZeroGap {
            payload_offset: 0x90
        })
    );
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn every_corrupted_header_field_and_table_byte_of_the_fixture_is_refused() {
    // Flip one bit in each field area of the real blob: every flip is refused (the hash check
    // fires first for the header hash, the others by their own rule).
    let expected = board_hash();
    for at in [
        0usize, 8, 12, 0x10, 0x30, 0x38, 0x3C, 0x40, 0x48, 0x4C, 0x50, 0x60, 0x64, 0x68, 0x6C,
    ] {
        let mut blob = FIXTURE.to_vec();
        blob[at] ^= 0x01;
        assert!(
            validate_acpi_blob(&blob, &expected).is_err(),
            "header byte {at:#x}"
        );
    }
    // Every byte of every table is covered by its checksum.
    let info = validate_acpi_blob(FIXTURE, &expected).unwrap();
    for table in &info.tables {
        for i in 0..table.length as usize {
            let mut blob = FIXTURE.to_vec();
            blob[info.payload_offset + table.payload_offset as usize + i] ^= 0x01;
            assert!(
                validate_acpi_blob(&blob, &expected).is_err(),
                "{:?} byte {i}",
                table.signature
            );
        }
    }
}
