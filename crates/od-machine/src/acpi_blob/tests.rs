//! S4-31d/g: machine-side validator of the ACPI table blob.
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
    assert_eq!(info.version, 2);
    assert_eq!(info.manifest_hash, board_hash());
    assert_eq!(
        info.load_range(),
        (0xE_0000, 0xE_0000 + info.payload_len as u64)
    );
    assert_eq!(info.load_base, ACPI_REGION_BASE);
    assert!(info.load_range().1 <= ACPI_RECLAIM_END);
    let signatures: Vec<&[u8; 4]> = info.tables.iter().map(|t| &t.signature).collect();
    assert_eq!(signatures, [b"DSDT", b"APIC", b"FACP", b"MCFG", b"XSDT"]);
    assert_eq!(info.payload(FIXTURE).len(), info.payload_len);
    // RSDP at the load base (the first payload slot), FACS in the NVS part.
    assert_eq!(info.rsdp_address, 0xE_0000);
    assert_eq!(info.facs_range(), (0xE_F000, 0xE_F040));
    assert_eq!(&info.payload(FIXTURE)[..8], b"RSD PTR ");
    assert_eq!(&info.facs_bytes(FIXTURE)[..4], b"FACS");
}

#[test]
fn the_fixture_layout_is_the_documented_one() {
    // Offsets written out here deliberately: a drift of the builder fails this test.
    let info = validate_acpi_blob(FIXTURE, &board_hash()).unwrap();
    let at = |signature: &[u8; 4]| info.table_address(signature).unwrap();
    assert_eq!(
        at(b"DSDT"),
        0xE_0040,
        "DSDT first, right after the RSDP slot"
    );
    assert_eq!(at(b"APIC"), 0xE_0160);
    assert_eq!(at(b"FACP"), 0xE_01C0);
    assert_eq!(at(b"MCFG"), 0xE_02E0);
    assert_eq!(at(b"XSDT"), 0xE_0320);
    assert_eq!(info.payload_offset, 0xB0);
    assert_eq!(info.payload_len, 0x360);
    assert_eq!(info.load_range(), (0xE_0000, 0xE_0360));
    assert_eq!(info.facs_offset, 0x410);
    assert_eq!(FIXTURE.len(), 0x450);
    // RSDP -> XSDT, XSDT -> FADT, MADT, MCFG, FADT -> DSDT and FACS.
    let payload = info.payload(FIXTURE);
    assert_eq!(le64(payload, 24), at(b"XSDT"));
    let xsdt = &payload[0x320..0x35C];
    assert_eq!(le64(xsdt, 36), at(b"FACP"));
    assert_eq!(le64(xsdt, 44), at(b"APIC"));
    assert_eq!(le64(xsdt, 52), at(b"MCFG"));
    let fadt = &payload[0x1C0..0x1C0 + 276];
    assert_eq!(le64(fadt, 140), at(b"DSDT"));
    assert_eq!(le64(fadt, 132), 0xE_F000);
}

#[test]
fn the_dsdt_in_the_blob_has_the_s5_package() {
    let info = validate_acpi_blob(FIXTURE, &board_hash()).unwrap();
    let dsdt = info
        .tables
        .iter()
        .find(|t| &t.signature == b"DSDT")
        .unwrap();
    let bytes = info.table_bytes(FIXTURE, dsdt);
    // Name(\_S5, Package() { 5, 5, 0, 0 }), SLP_TYP 5 from the PM entry.
    let s5 = [
        0x08u8, 0x5C, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x08, 0x04, 0x0A, 0x05, 0x0A, 0x05, 0x00, 0x00,
    ];
    assert!(bytes.windows(s5.len()).any(|w| w == s5));
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
//
// `Blob` assembles a complete, coherent blob byte by byte (RSDP, XSDT, FADT with real addresses,
// FACS); a test then breaks exactly one thing in the built bytes and, where a checksum would
// hide the defect, repairs the checksum with `seal`/`seal_rsdp` so that only the rule under test
// fails.

const HASH: AcpiManifestHash = [0x5A; 32];
const NO_PAD: usize = 0;

/// A System Description Table of `len` bytes with a correct length field and checksum.
fn sdt(signature: &[u8; 4], len: usize) -> Vec<u8> {
    let mut table = vec![0x11u8; len];
    table[..4].copy_from_slice(signature);
    put32(&mut table, 4, len as u32);
    seal(&mut table);
    table
}

/// Recomputes the checksum byte (offset 9) of an SDT.
fn seal(table: &mut [u8]) {
    table[9] = 0;
    let sum = table.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    table[9] = sum.wrapping_neg();
}

/// Recomputes both checksums of an RSDP.
fn seal_rsdp(rsdp: &mut [u8]) {
    rsdp[8] = 0;
    rsdp[32] = 0;
    rsdp[8] = rsdp[..20]
        .iter()
        .fold(0u8, |s, &b| s.wrapping_add(b))
        .wrapping_neg();
    rsdp[32] = rsdp
        .iter()
        .fold(0u8, |s, &b| s.wrapping_add(b))
        .wrapping_neg();
}

/// Layout of the hand-made payload (offsets in the payload): RSDP 0, DSDT 0x40 (0x40 bytes),
/// MADT 0x80 (0x40), FADT 0xC0 (276, to 0x1D4), MCFG 0x1E0 (0x3C), XSDT 0x220 (with MCFG 60
/// bytes, without 52), the payload ends 16-aligned.
struct Blob {
    with_mcfg: bool,
    /// Extra zero bytes between the directory and the payload.
    pad: usize,
}

const DSDT_AT: u32 = 0x40;
const MADT_AT: u32 = 0x80;
const FADT_AT: u32 = 0xC0;
const FADT_LEN: usize = 276;
const MCFG_AT: u32 = 0x1E0;
const XSDT_AT: u32 = 0x220;

impl Blob {
    fn good() -> Self {
        Blob {
            with_mcfg: true,
            pad: NO_PAD,
        }
    }

    fn count(&self) -> usize {
        if self.with_mcfg { 5 } else { 4 }
    }

    fn payload_offset(&self) -> usize {
        (0x60 + self.count() * 16).div_ceil(16) * 16 + self.pad
    }

    fn build(&self) -> Vec<u8> {
        let base = 0xE_0000u64;
        let xsdt_len = if self.with_mcfg { 60 } else { 52 };
        let xsdt_at = if self.with_mcfg { XSDT_AT } else { MCFG_AT };
        let payload_len = (xsdt_at as usize + xsdt_len).div_ceil(16) * 16;
        let payload_offset = self.payload_offset();
        let facs_offset = payload_offset + payload_len;
        let mut blob = vec![0u8; facs_offset + 64];

        let mut tables: Vec<([u8; 4], u32, Vec<u8>)> = vec![
            (*b"DSDT", DSDT_AT, sdt(b"DSDT", 0x40)),
            (*b"APIC", MADT_AT, sdt(b"APIC", 0x40)),
        ];
        let mut fadt = sdt(b"FACP", FADT_LEN);
        put32(&mut fadt, 36, 0xE_F000);
        put64(&mut fadt, 132, 0xE_F000);
        put32(&mut fadt, 40, base as u32 + DSDT_AT);
        put64(&mut fadt, 140, base + u64::from(DSDT_AT));
        seal(&mut fadt);
        tables.push((*b"FACP", FADT_AT, fadt));
        let mut listed = vec![base + u64::from(FADT_AT), base + u64::from(MADT_AT)];
        if self.with_mcfg {
            tables.push((*b"MCFG", MCFG_AT, sdt(b"MCFG", 0x3C)));
            listed.push(base + u64::from(MCFG_AT));
        }
        let mut xsdt = vec![0u8; 36 + 8 * listed.len()];
        xsdt[..4].copy_from_slice(b"XSDT");
        let xsdt_total = xsdt.len() as u32;
        put32(&mut xsdt, 4, xsdt_total);
        xsdt[8] = 1;
        for (i, address) in listed.iter().enumerate() {
            put64(&mut xsdt, 36 + 8 * i, *address);
        }
        seal(&mut xsdt);
        tables.push((*b"XSDT", xsdt_at, xsdt));

        blob[..8].copy_from_slice(b"ODACPIB1");
        put32(&mut blob, 0x08, 2);
        put32(&mut blob, 0x0C, 0x60);
        blob[0x10..0x30].copy_from_slice(&HASH);
        put64(&mut blob, 0x30, base);
        put32(&mut blob, 0x38, payload_offset as u32);
        put32(&mut blob, 0x3C, payload_len as u32);
        put64(&mut blob, 0x40, base);
        put32(&mut blob, 0x48, tables.len() as u32);
        let total = blob.len() as u32;
        put32(&mut blob, 0x4C, total);
        put32(&mut blob, 0x50, facs_offset as u32);
        put32(&mut blob, 0x54, 64);
        put64(&mut blob, 0x58, 0xE_F000);
        for (index, (signature, offset, bytes)) in tables.iter().enumerate() {
            let at = 0x60 + index * 16;
            blob[at..at + 4].copy_from_slice(signature);
            put32(&mut blob, at + 4, *offset);
            put32(&mut blob, at + 8, bytes.len() as u32);
            let start = payload_offset + *offset as usize;
            blob[start..start + bytes.len()].copy_from_slice(bytes);
        }
        // RSDP rev 2 in the slot: XSDT address, no RSDT.
        let mut rsdp = vec![0u8; 36];
        rsdp[..8].copy_from_slice(b"RSD PTR ");
        rsdp[9..15].copy_from_slice(b"OPNDSK");
        rsdp[15] = 2;
        put32(&mut rsdp, 20, 36);
        put64(&mut rsdp, 24, base + u64::from(xsdt_at));
        seal_rsdp(&mut rsdp);
        blob[payload_offset..payload_offset + 36].copy_from_slice(&rsdp);
        // FACS: signature, Length 64, version 2.
        blob[facs_offset..facs_offset + 4].copy_from_slice(b"FACS");
        put32(&mut blob, facs_offset + 4, 64);
        blob[facs_offset + 32] = 2;
        blob
    }
}

fn good() -> Vec<u8> {
    Blob::good().build()
}

fn check_blob(blob: &[u8]) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    validate_acpi_blob(blob, &HASH)
}

/// Applies `patch` to a copy of the good blob and validates it.
fn with(patch: &dyn Fn(&mut Vec<u8>)) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    let mut blob = good();
    patch(&mut blob);
    check_blob(&blob)
}

/// Payload start of the good blob (header 0x60 + five directory entries).
const PAYLOAD: usize = 0xB0;

/// Applies `patch` to the bytes of one table of the good blob, re-seals its checksum and validates.
fn with_table(
    at: u32,
    len: usize,
    patch: &dyn Fn(&mut [u8]),
) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    let mut blob = good();
    let start = PAYLOAD + at as usize;
    patch(&mut blob[start..start + len]);
    seal(&mut blob[start..start + len]);
    check_blob(&blob)
}

/// Shortens the table at payload offset `at` from `old` to `new` bytes (new Length, directory
/// entry `index`, zeroed tail so no stray byte is left, checksum re-sealed).
fn shrink_table(blob: &mut [u8], index: usize, at: u32, old: usize, new: usize) {
    let start = PAYLOAD + at as usize;
    put32(blob, start + 4, new as u32);
    blob[start + new..start + old].fill(0);
    seal(&mut blob[start..start + new]);
    put32(blob, 0x60 + index * 16 + 8, new as u32);
}

/// Inserts `extra` zero bytes at the end of the payload (before the FACS) and fixes the header,
/// to reach payload lengths the hand-made layout does not have.
fn grow_payload(blob: &mut Vec<u8>, extra: usize) {
    let facs_offset = le32(blob, 0x50) as usize;
    blob.splice(facs_offset..facs_offset, vec![0u8; extra]);
    let payload_len = le32(blob, 0x3C) + extra as u32;
    put32(blob, 0x3C, payload_len);
    put32(blob, 0x50, (facs_offset + extra) as u32);
    let total = blob.len() as u32;
    put32(blob, 0x4C, total);
}

#[test]
fn a_correct_blob_is_accepted() {
    let info = check_blob(&good()).unwrap();
    assert_eq!(info.load_range(), (0xE_0000, 0xE_0000 + 0x260));
    assert_eq!(info.facs_range(), (0xE_F000, 0xE_F040));
    assert_eq!(info.rsdp_address, 0xE_0000);
    assert_eq!(info.tables.len(), 5);
    assert_eq!(info.table_address(b"XSDT"), Some(0xE_0220));
    // No ECAM board: no MCFG, a two-entry XSDT.
    let no_mcfg = Blob {
        with_mcfg: false,
        pad: NO_PAD,
    }
    .build();
    let info = check_blob(&no_mcfg).unwrap();
    assert_eq!(info.tables.len(), 4);
    assert!(info.table_address(b"MCFG").is_none());
}

#[test]
fn tables_that_overlap_are_refused() {
    // MCFG (directory index 3) moved to 0x1D0 runs into the FADT [0xC0, 0x1D4).
    assert_eq!(
        with(&|b| put32(b, 0x60 + 3 * 16 + 4, 0x1D0)),
        Err(AcpiBlobError::TableOverlap {
            first: 2,
            second: 3
        })
    );
    // The MADT moved onto the DSDT.
    assert_eq!(
        with(&|b| put32(b, 0x60 + 16 + 4, 0x40)),
        Err(AcpiBlobError::TableOverlap {
            first: 0,
            second: 1
        })
    );
    // Reported in directory indices even when the directory is not in offset order.
    assert_eq!(
        with(&|b| {
            // Swap the directory entries of DSDT (0) and MADT (1), then overlap them.
            let (a, c) = (0x60, 0x70);
            for i in 0..16 {
                b.swap(a + i, c + i);
            }
            put32(b, 0x60 + 4, 0x70); // the MADT entry (now index 0) at 0x70 overlaps the DSDT
        }),
        Err(AcpiBlobError::TableOverlap {
            first: 1,
            second: 0
        })
    );
}

#[test]
fn only_the_region_base_is_accepted_as_load_base() {
    // Debt of S4-31d: any 16-aligned base inside the region used to pass.
    for load_base in [
        0xE_0010u64,
        0xE_EF80, // a 16-byte aligned base inside the region, accepted before the v2 layout
        0xD_FFF0,
        0x0,
        0xF_0000,
        0x10_0000,
        0xE_0008,
        !0xF_u64,
    ] {
        assert_eq!(
            with(&|b| put64(b, 0x30, load_base)),
            Err(AcpiBlobError::UnexpectedLoadBase(load_base)),
            "load base {load_base:#x}"
        );
    }
    check_blob(&good()).unwrap();
}

#[test]
fn the_rsdp_address_must_be_the_load_base() {
    // 0 was the value of the S4-31d blob; it no longer passes.
    for rsdp in [0u64, 0xE_0010, 0xE_0040, 0xF_0000] {
        assert_eq!(
            with(&|b| put64(b, 0x40, rsdp)),
            Err(AcpiBlobError::RsdpAddressMismatch { found: rsdp }),
            "rsdp address {rsdp:#x}"
        );
    }
}

#[test]
fn a_payload_that_leaves_the_reclaim_part_is_refused() {
    // Payload of exactly 0xF000 bytes ends at 0xE_F000: legal. 0x10 more bytes reach into the
    // NVS part (FACS only), and a payload past 0xF_0000 leaves the region altogether.
    let blob = good();
    let payload_len = le32(&blob, 0x3C) as usize;
    let mut full = blob.clone();
    grow_payload(&mut full, 0xF000 - payload_len);
    assert_eq!(le32(&full, 0x3C), 0xF000);
    assert!(check_blob(&full).is_ok());

    let mut nvs = blob.clone();
    grow_payload(&mut nvs, 0xF000 - payload_len + 0x10);
    assert_eq!(
        check_blob(&nvs),
        Err(AcpiBlobError::PayloadIntoNvs { end: 0xE_F010 })
    );
    let mut out = blob.clone();
    grow_payload(&mut out, 0x1_0000 - payload_len + 0x10);
    assert_eq!(
        check_blob(&out),
        Err(AcpiBlobError::OutOfRegion {
            start: 0xE_0000,
            end: Some(0xF_0010)
        })
    );
}

#[test]
fn a_bad_checksum_is_refused() {
    // One bit flipped in a byte that was 0x11: the sum is off by -1.
    assert_eq!(
        with(&|b| b[PAYLOAD + DSDT_AT as usize + 0x20] ^= 0x01),
        Err(AcpiBlobError::BadChecksum {
            index: 0,
            sum: 0xFF
        })
    );
    assert!(matches!(
        with(&|b| {
            let at = PAYLOAD + MCFG_AT as usize + 9;
            b[at] = b[at].wrapping_add(7);
        }),
        Err(AcpiBlobError::BadChecksum { index: 3, .. })
    ));
}

#[test]
fn a_wrong_length_is_refused_wherever_it_is_written() {
    // The table header says 0x44, the directory says 0x40 (checksum made right for 0x44).
    assert_eq!(
        with_table(DSDT_AT, 0x40, &|t| put32(t, 4, 0x44)),
        Err(AcpiBlobError::TableLengthMismatch {
            index: 0,
            directory: 0x40,
            header: 0x44
        })
    );
    // A directory length that runs past the payload.
    assert_eq!(
        with(&|b| put32(b, 0x60 + 3 * 16 + 8, 0x1000)),
        Err(AcpiBlobError::TableOutsidePayload { index: 3 })
    );
    assert_eq!(
        with(&|b| put32(b, 0x60 + 3 * 16 + 8, 0xFFFF_FF00)),
        Err(AcpiBlobError::TableOutsidePayload { index: 3 })
    );
    // A table shorter than an SDT header.
    assert_eq!(
        with(&|b| put32(b, 0x60 + 8, 0x20)),
        Err(AcpiBlobError::TableTooShort {
            index: 0,
            len: 0x20
        })
    );
    // The total length of the blob and the payload length.
    let blob = good();
    assert_eq!(
        with(&|b| put32(b, 0x4C, blob.len() as u32 + 1)),
        Err(AcpiBlobError::LengthMismatch {
            declared: blob.len() as u32 + 1,
            actual: blob.len()
        })
    );
    assert!(matches!(
        with(&|b| put32(b, 0x3C, 0x230)),
        Err(AcpiBlobError::BadPayload { .. })
    ));
    // A payload length that is not 16-aligned (the FACS offset moved with it).
    assert!(matches!(
        with(&|b| {
            put32(b, 0x3C, 0x241);
            put32(b, 0x50, 0xB0 + 0x241);
        }),
        Err(AcpiBlobError::BadPayload { .. })
    ));
    // A short blob is truncated, not a panic.
    for cut in [0, 8, 0x5F, 0x60, 0xB0, blob.len() - 1] {
        assert!(check_blob(&blob[..cut]).is_err(), "cut {cut}");
    }
}

#[test]
fn header_and_directory_defects_are_refused() {
    assert_eq!(with(&|b| b[0] = b'X'), Err(AcpiBlobError::BadMagic));
    for version in [0u32, 1, 3] {
        assert_eq!(
            with(&|b| put32(b, 0x08, version)),
            Err(AcpiBlobError::UnsupportedVersion(version))
        );
    }
    assert_eq!(
        with(&|b| put32(b, 0x0C, 0x70)),
        Err(AcpiBlobError::BadHeaderLength(0x70))
    );
    assert_eq!(
        with(&|b| put32(b, 0x6C, 1)),
        Err(AcpiBlobError::ReservedNotZero)
    );
    assert_eq!(
        with(&|b| put32(b, 0x48, 0)),
        Err(AcpiBlobError::BadTableCount(0))
    );
    assert_eq!(
        with(&|b| put32(b, 0x48, 9)),
        Err(AcpiBlobError::BadTableCount(9))
    );
    // A signature outside the directory's set, and a repeated one.
    assert_eq!(
        with(&|b| b[0x60..0x64].copy_from_slice(b"SSDT")),
        Err(AcpiBlobError::UnexpectedTable {
            signature: *b"SSDT"
        })
    );
    assert_eq!(
        with(&|b| b[0x70..0x74].copy_from_slice(b"DSDT")),
        Err(AcpiBlobError::DuplicateTable {
            signature: *b"DSDT"
        })
    );
    // The signature inside the table must be the one of the directory.
    assert_eq!(
        with_table(DSDT_AT, 0x40, &|t| t[..4].copy_from_slice(b"APIC")),
        Err(AcpiBlobError::SignatureMismatch { index: 0 })
    );
    // Alignment, RSDP slot.
    assert_eq!(
        with(&|b| put32(b, 0x60 + 3 * 16 + 4, 0x1E4)),
        Err(AcpiBlobError::MisalignedTable {
            index: 3,
            offset: 0x1E4
        })
    );
    assert_eq!(
        with(&|b| put32(b, 0x60 + 4, 0x30)),
        Err(AcpiBlobError::TableInRsdpSlot {
            index: 0,
            offset: 0x30
        })
    );
}

#[test]
fn a_blob_must_have_dsdt_fadt_madt_and_xsdt() {
    // Drop the last directory entry (the XSDT; its bytes stay in the payload).
    assert_eq!(
        with(&|b| {
            put32(b, 0x48, 4);
            b[0xA0..0xB0].fill(0);
        }),
        Err(AcpiBlobError::MissingTable {
            signature: *b"XSDT"
        })
    );
    // Rename the MADT entry to a second MCFG: duplicate; to the XSDT: duplicate as well.
    assert_eq!(
        with(&|b| b[0x70..0x74].copy_from_slice(b"MCFG")),
        Err(AcpiBlobError::DuplicateTable {
            signature: *b"MCFG"
        })
    );
}

#[test]
fn bytes_that_no_table_owns_must_be_zero() {
    let payload = PAYLOAD;
    // The rest of the RSDP slot after the 36 RSDP bytes.
    assert_eq!(
        with(&|b| b[payload + 0x30] = 1),
        Err(AcpiBlobError::NonZeroGap {
            payload_offset: 0x30
        })
    );
    // The alignment gap after the FADT [0x1D4, 0x1E0) and the tail of the payload [0x25C, 0x260).
    assert_eq!(
        with(&|b| b[payload + 0x1D8] = 0xCC),
        Err(AcpiBlobError::NonZeroGap {
            payload_offset: 0x1D8
        })
    );
    assert_eq!(
        with(&|b| b[payload + 0x25D] = 0xCC),
        Err(AcpiBlobError::NonZeroGap {
            payload_offset: 0x25D
        })
    );
}

#[test]
fn bytes_between_the_directory_and_the_payload_must_be_zero() {
    // Debt of S4-31d: this gap was never looked at. The hand-made blob has 0x10 pad bytes.
    let padded = Blob {
        with_mcfg: true,
        pad: 0x10,
    }
    .build();
    assert!(check_blob(&padded).is_ok());
    assert_eq!(le32(&padded, 0x38), 0xC0);
    for at in [0xB0usize, 0xB7, 0xBF] {
        let mut blob = padded.clone();
        blob[at] = 0x01;
        assert_eq!(
            check_blob(&blob),
            Err(AcpiBlobError::NonZeroHeaderGap { blob_offset: at })
        );
    }
}

// ---- RSDP, XSDT, FADT, FACS ---------------------------------------------------------------

/// Patches the RSDP of the good blob and re-seals both checksums.
fn with_rsdp(patch: &dyn Fn(&mut [u8])) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    let mut blob = good();
    patch(&mut blob[PAYLOAD..PAYLOAD + 36]);
    seal_rsdp(&mut blob[PAYLOAD..PAYLOAD + 36]);
    check_blob(&blob)
}

#[test]
fn the_rsdp_is_checked_field_by_field() {
    assert!(with_rsdp(&|_| {}).is_ok());
    assert_eq!(
        with_rsdp(&|r| r[0] = b'X'),
        Err(AcpiBlobError::BadRsdpSignature)
    );
    assert_eq!(
        with_rsdp(&|r| r[15] = 0),
        Err(AcpiBlobError::BadRsdp("revision is not 2"))
    );
    assert_eq!(
        with_rsdp(&|r| r[16] = 1),
        Err(AcpiBlobError::BadRsdp("RsdtAddress is not 0"))
    );
    assert_eq!(
        with_rsdp(&|r| r[20] = 20),
        Err(AcpiBlobError::BadRsdp("Length is not 36"))
    );
    assert_eq!(
        with_rsdp(&|r| r[34] = 1),
        Err(AcpiBlobError::BadRsdp("reserved bytes are not zero"))
    );
    // XsdtAddress: inside the blob but not the XSDT (the DSDT), outside the blob, zero.
    for address in [0xE_0040u64, 0xE_0221, 0x1_0000_0000, 0] {
        assert_eq!(
            with_rsdp(&|r| put64(r, 24, address)),
            Err(AcpiBlobError::RsdpXsdtAddress {
                found: address,
                expected: 0xE_0220
            })
        );
    }
}

#[test]
fn the_two_rsdp_checksums_are_independent() {
    // First checksum: a change in bytes 0..20 that is not repaired.
    let mut blob = good();
    blob[PAYLOAD + 10] ^= 0x01; // OEM ID
    assert_eq!(
        check_blob(&blob),
        Err(AcpiBlobError::BadRsdpChecksum { sum: 0x01 })
    );
    // Extended checksum: a change after byte 20 that leaves the first sum intact.
    let mut blob = good();
    blob[PAYLOAD + 26] ^= 0x01; // XsdtAddress byte
    assert!(matches!(
        check_blob(&blob),
        Err(AcpiBlobError::BadRsdpExtendedChecksum { .. })
    ));
    // The extended checksum byte itself.
    let mut blob = good();
    blob[PAYLOAD + 32] ^= 0x10;
    assert_eq!(
        check_blob(&blob),
        Err(AcpiBlobError::BadRsdpExtendedChecksum { sum: 0x10 })
    );
    // A repaired extended checksum with a broken first one: still the first one is reported.
    let mut blob = good();
    blob[PAYLOAD + 8] ^= 0x02;
    blob[PAYLOAD + 32] = blob[PAYLOAD + 32].wrapping_add(0x02);
    assert_eq!(
        check_blob(&blob),
        Err(AcpiBlobError::BadRsdpChecksum { sum: 0x02 })
    );
}

#[test]
fn the_xsdt_must_list_exactly_the_fadt_madt_and_mcfg_of_the_blob() {
    let entry_at = |i: usize| 36 + 8 * i;
    // An entry outside the blob, one inside the region but not at a table, zero.
    for address in [0xF_0000u64, 0xE_0041, 0x1_0000_0000, 0] {
        assert_eq!(
            with_table(XSDT_AT, 60, &|t| put64(t, entry_at(2), address)),
            Err(AcpiBlobError::XsdtEntryOutsideBlob { index: 2, address })
        );
    }
    // An entry at the FACS (not a table of the directory).
    assert_eq!(
        with_table(XSDT_AT, 60, &|t| put64(t, entry_at(0), 0xE_F000)),
        Err(AcpiBlobError::XsdtEntryOutsideBlob {
            index: 0,
            address: 0xE_F000
        })
    );
    // An entry at a table that must not be listed (DSDT, the XSDT itself).
    assert_eq!(
        with_table(XSDT_AT, 60, &|t| put64(t, entry_at(1), 0xE_0040)),
        Err(AcpiBlobError::XsdtEntryWrongTable {
            index: 1,
            signature: *b"DSDT"
        })
    );
    assert_eq!(
        with_table(XSDT_AT, 60, &|t| put64(t, entry_at(1), 0xE_0220)),
        Err(AcpiBlobError::XsdtEntryWrongTable {
            index: 1,
            signature: *b"XSDT"
        })
    );
    // The same table twice, which also leaves the MCFG unlisted: the duplicate is reported first.
    assert_eq!(
        with_table(XSDT_AT, 60, &|t| put64(t, entry_at(2), 0xE_00C0)),
        Err(AcpiBlobError::XsdtEntryWrongTable {
            index: 2,
            signature: *b"FACP"
        })
    );
    // A table of the blob that the XSDT leaves out: shorten the XSDT to two entries (Length 52).
    assert_eq!(
        with(&|b| shrink_table(b, 4, XSDT_AT, 60, 52)),
        Err(AcpiBlobError::XsdtMissingTable {
            signature: *b"MCFG"
        })
    );
    // Revision and body shape.
    assert_eq!(
        with_table(XSDT_AT, 60, &|t| t[8] = 2),
        Err(AcpiBlobError::BadXsdt("revision is not 1"))
    );
    assert_eq!(
        with(&|b| shrink_table(b, 4, XSDT_AT, 60, 36)),
        Err(AcpiBlobError::BadXsdt(
            "body is not a whole number of entries"
        ))
    );
    assert_eq!(
        with(&|b| shrink_table(b, 4, XSDT_AT, 60, 48)),
        Err(AcpiBlobError::BadXsdt(
            "body is not a whole number of entries"
        ))
    );
    // The XSDT checksum itself.
    assert!(matches!(
        with(&|b| b[PAYLOAD + XSDT_AT as usize + 40] ^= 1),
        Err(AcpiBlobError::BadChecksum { index: 4, .. })
    ));
}

#[test]
fn the_fadt_must_point_at_the_dsdt_of_the_blob_and_the_facs_at_0xe_f000() {
    let fadt = |patch: &dyn Fn(&mut [u8])| with_table(FADT_AT, FADT_LEN, patch);
    // X_DSDT elsewhere: the MADT, outside the blob, zero.
    for address in [0xE_0080u64, 0xE_0041, 0x1000, 0] {
        assert_eq!(
            fadt(&|t| put64(t, 140, address)),
            Err(AcpiBlobError::FadtAddressMismatch {
                field: "X_DSDT",
                found: address,
                expected: 0xE_0040
            })
        );
    }
    // The 32-bit DSDT field disagrees.
    assert_eq!(
        fadt(&|t| put32(t, 40, 0xE_0080)),
        Err(AcpiBlobError::FadtAddressMismatch {
            field: "DSDT",
            found: 0xE_0080,
            expected: 0xE_0040
        })
    );
    // The FACS is not at 0xE_F000 in either form.
    assert_eq!(
        fadt(&|t| put64(t, 132, 0xE_F040)),
        Err(AcpiBlobError::FadtAddressMismatch {
            field: "X_FIRMWARE_CTRL",
            found: 0xE_F040,
            expected: 0xE_F000
        })
    );
    assert_eq!(
        fadt(&|t| put32(t, 36, 0xE_0000)),
        Err(AcpiBlobError::FadtAddressMismatch {
            field: "FIRMWARE_CTRL",
            found: 0xE_0000,
            expected: 0xE_F000
        })
    );
    // A FADT too short to carry X_DSDT.
    assert_eq!(
        with(&|b| shrink_table(b, 2, FADT_AT, FADT_LEN, 100)),
        Err(AcpiBlobError::FadtTooShort { len: 100 })
    );
}

#[test]
fn the_facs_segment_is_checked() {
    // FACS address: the old payload-relative position, an unaligned one, zero.
    for address in [0xE_0340u64, 0xE_F040, 0xE_F001, 0] {
        assert_eq!(
            with(&|b| put64(b, 0x58, address)),
            Err(AcpiBlobError::FacsAddressMismatch { found: address })
        );
    }
    assert_eq!(with(&|b| put32(b, 0x54, 128)), Err(AcpiBlobError::BadFacs));
    let facs = le32(&good(), 0x50) as usize;
    assert_eq!(with(&|b| b[facs] = b'X'), Err(AcpiBlobError::BadFacs));
    // The version byte (offset 32) and a reserved byte (offset 33) are checked too.
    assert_eq!(with(&|b| b[facs + 32] = 1), Err(AcpiBlobError::BadFacs));
    assert_eq!(with(&|b| b[facs + 33] = 1), Err(AcpiBlobError::BadFacs));
    assert_eq!(
        with(&|b| put32(b, facs + 4, 128)),
        Err(AcpiBlobError::BadFacs)
    );
    // The FACS offset must be the end of the payload, and the FACS ends the blob.
    assert!(matches!(
        with(&|b| put32(b, 0x50, facs as u32 - 16)),
        Err(AcpiBlobError::BadPayload { .. })
    ));
    let mut longer = good();
    longer.extend_from_slice(&[0u8; 16]);
    let total = longer.len() as u32;
    put32(&mut longer, 0x4C, total);
    assert!(matches!(
        check_blob(&longer),
        Err(AcpiBlobError::BadPayload { .. })
    ));
}

#[test]
fn every_corrupted_header_field_and_byte_of_the_fixture_is_refused() {
    // Flip one bit in every byte of the header and the directory of the real blob: all refused
    // (no reserved field is left in the v2 header).
    let expected = board_hash();
    let info = validate_acpi_blob(FIXTURE, &expected).unwrap();
    for at in 0..info.payload_offset {
        let mut blob = FIXTURE.to_vec();
        blob[at] ^= 0x01;
        assert!(
            validate_acpi_blob(&blob, &expected).is_err(),
            "header or directory byte {at:#x}"
        );
    }
    // Every byte of the payload is an RSDP byte (summed), a table byte (summed) or a gap byte
    // (zero), so every flip is refused.
    for i in 0..info.payload_len {
        let mut blob = FIXTURE.to_vec();
        blob[info.payload_offset + i] ^= 0x01;
        assert!(
            validate_acpi_blob(&blob, &expected).is_err(),
            "payload byte {i:#x}"
        );
    }
    // The FACS has no checksum; every byte is pinned (signature, length, version 2, the rest zero).
    for i in 0..64 {
        let mut blob = FIXTURE.to_vec();
        blob[info.facs_offset + i] ^= 0x01;
        assert!(
            validate_acpi_blob(&blob, &expected).is_err(),
            "FACS byte {i}"
        );
    }
}
