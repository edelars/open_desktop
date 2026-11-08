//! S4-31d: the stage-4 blob. The header is read here by hard-coded offsets (not through
//! constants of the builder), the tables are compared with the run-time builders, and the build
//! is shown to depend on the device table only.

use super::*;
use crate::acpi::dsdt::validate_dsdt;
use crate::acpi::manifest::{BoardManifest, DeviceEntry};
use crate::acpi::mcfg::validate_mcfg;
use crate::acpi::table::byte_sum;

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn build(devices: &[DeviceEntry], fill: u8) -> Vec<u8> {
    let mut out = vec![fill; STAGE4_BLOB_CAPACITY];
    let len = build_blob(devices, &mut out).unwrap();
    out.truncate(len);
    out
}

fn board_manifest() -> BoardManifest {
    let entries: [DeviceEntry; 7] = STAGE4_DEVICES[..7].try_into().unwrap();
    BoardManifest::from_entries(entries).unwrap()
}

#[test]
fn const_blob_equals_the_runtime_blob_byte_for_byte() {
    const CONST_BLOB: [u8; STAGE4_BLOB_LEN] = stage4_blob_bytes::<STAGE4_BLOB_LEN>();
    assert_eq!(CONST_BLOB.as_slice(), build(&STAGE4_DEVICES, 0).as_slice());
}

#[test]
fn tables_in_the_blob_are_the_runtime_dsdt_and_mcfg() {
    let blob = build(&STAGE4_DEVICES, 0);
    let manifest = board_manifest();
    let mut dsdt = [0u8; 512];
    let dsdt_len = build_dsdt(&manifest, &mut dsdt).unwrap();
    let mut mcfg = [0u8; 128];
    let mcfg_len = build_mcfg(&manifest, &mut mcfg).unwrap();

    let payload = le32(&blob, 0x38) as usize;
    assert_eq!(le32(&blob, 0x48), 2);
    let entry = |i: usize| {
        let at = 0x60 + i * 16;
        (
            blob[at..at + 4].to_vec(),
            le32(&blob, at + 4) as usize,
            le32(&blob, at + 8) as usize,
        )
    };
    let (sig0, off0, len0) = entry(0);
    let (sig1, off1, len1) = entry(1);
    assert_eq!((sig0.as_slice(), len0), (b"DSDT".as_slice(), dsdt_len));
    assert_eq!((sig1.as_slice(), len1), (b"MCFG".as_slice(), mcfg_len));
    assert_eq!(
        &blob[payload + off0..payload + off0 + len0],
        &dsdt[..dsdt_len]
    );
    assert_eq!(
        &blob[payload + off1..payload + off1 + len1],
        &mcfg[..mcfg_len]
    );
    assert_eq!(validate_dsdt(&dsdt[..dsdt_len]), Ok(()));
    assert_eq!(validate_mcfg(&mcfg[..mcfg_len]), Ok(()));
}

#[test]
fn header_layout_is_the_documented_one() {
    let blob = build(&STAGE4_DEVICES, 0);
    assert_eq!(&blob[..8], b"ODACPIB1");
    assert_eq!(le32(&blob, 0x08), 1);
    assert_eq!(le32(&blob, 0x0C), 0x60);
    assert_eq!(&blob[0x10..0x30], &manifest_hash(&STAGE4_DEVICES).unwrap());
    assert_eq!(le64(&blob, 0x30), 0xE_0000);
    // Header 0x60 + two directory entries = 0x80, already 16-aligned.
    assert_eq!(le32(&blob, 0x38), 0x80);
    assert_eq!(le64(&blob, 0x40), 0, "RSDP is S4-31g");
    assert_eq!(le32(&blob, 0x4C) as usize, blob.len());
    assert_eq!(
        le32(&blob, 0x38) + le32(&blob, 0x3C),
        blob.len() as u32,
        "the payload ends at the end of the blob"
    );
    assert!(blob[0x50..0x60].iter().all(|&b| b == 0));
    // Directory reserved words are zero.
    assert_eq!(le32(&blob, 0x60 + 12), 0);
    assert_eq!(le32(&blob, 0x70 + 12), 0);
}

#[test]
fn payload_layout_has_an_empty_rsdp_slot_aligned_tables_and_zero_gaps() {
    let blob = build(&STAGE4_DEVICES, 0xFF);
    let payload_start = le32(&blob, 0x38) as usize;
    let payload = &blob[payload_start..];
    assert!(payload[..RSDP_SLOT_LEN].iter().all(|&b| b == 0));
    let mut covered = vec![false; payload.len()];
    for slot in covered.iter_mut().take(RSDP_SLOT_LEN) {
        *slot = true;
    }
    for i in 0..le32(&blob, 0x48) as usize {
        let at = 0x60 + i * 16;
        let (offset, len) = (le32(&blob, at + 4) as usize, le32(&blob, at + 8) as usize);
        assert_eq!(offset % 16, 0, "tables are 16-aligned");
        assert!(offset >= RSDP_SLOT_LEN);
        let table = &payload[offset..offset + len];
        assert_eq!(le32(table, 4) as usize, len, "length field");
        assert_eq!(byte_sum(table), 0, "checksum");
        for slot in &mut covered[offset..offset + len] {
            assert!(!*slot, "tables do not overlap");
            *slot = true;
        }
    }
    for (i, byte) in payload.iter().enumerate() {
        assert!(covered[i] || *byte == 0, "gap byte {i} is zero");
    }
    // Everything the load covers lies in the ACPI table region (reclaim part).
    let end = 0xE_0000 + le32(&blob, 0x3C) as u64;
    assert!(end <= 0xE_F000);
}

#[test]
fn the_result_does_not_depend_on_what_the_buffer_held() {
    let zero = build(&STAGE4_DEVICES, 0x00);
    let ones = build(&STAGE4_DEVICES, 0xFF);
    let pattern = build(&STAGE4_DEVICES, 0xA5);
    assert_eq!(zero, ones);
    assert_eq!(zero, pattern);
    // Two builds from the same table give the same hash of the blob bytes.
    assert_eq!(sha256(&zero), sha256(&build(&STAGE4_DEVICES, 0x5A)));
}

#[test]
fn manifest_hash_is_sha256_of_the_tag_and_the_entries_in_type_order() {
    let mut raw = Vec::from(MANIFEST_HASH_TAG);
    for t in [1u32, 2, 3, 4, 5, 6, 7, 9] {
        let entry = STAGE4_DEVICES.iter().find(|e| e.device_type == t).unwrap();
        raw.extend_from_slice(&entry.to_bytes());
    }
    assert_eq!(raw.len(), 16 + 8 * 48);
    assert_eq!(manifest_hash(&STAGE4_DEVICES).unwrap(), sha256(&raw));
    // The order of the table is irrelevant.
    let mut reversed = STAGE4_DEVICES;
    reversed.reverse();
    assert_eq!(
        manifest_hash(&reversed).unwrap(),
        manifest_hash(&STAGE4_DEVICES).unwrap()
    );
}

#[test]
fn any_change_of_the_device_table_changes_the_manifest_hash_and_the_blob() {
    let base = manifest_hash(&STAGE4_DEVICES).unwrap();
    let blob = build(&STAGE4_DEVICES, 0);
    for (index, tweak) in [
        |e: &mut DeviceEntry| e.base ^= 0x1000,
        |e: &mut DeviceEntry| e.length += 8,
        |e: &mut DeviceEntry| e.gsi ^= 1,
        |e: &mut DeviceEntry| e.irq_flags ^= 1,
        |e: &mut DeviceEntry| e.param0 ^= 1,
        |e: &mut DeviceEntry| e.param1 ^= 1,
    ]
    .into_iter()
    .enumerate()
    {
        // COM1 (index 0) and the PM entry (index 7) both feed the hash.
        for victim in [0usize, 7] {
            let mut devices = STAGE4_DEVICES;
            tweak(&mut devices[victim]);
            assert_ne!(
                manifest_hash(&devices).unwrap(),
                base,
                "tweak {index} of entry {victim}"
            );
        }
    }
    // A moved ECAM window changes the hash and the MCFG bytes (the blob is foreign).
    let mut moved = STAGE4_DEVICES;
    moved[6].base = 0xFC20_0000;
    assert_ne!(manifest_hash(&moved).unwrap(), base);
    assert_ne!(build(&moved, 0), blob);
}

#[test]
fn bad_device_tables_are_rejected() {
    // Unknown and duplicate types.
    let mut devices = STAGE4_DEVICES;
    devices[0].device_type = 16;
    assert_eq!(
        manifest_hash(&devices),
        Err(ManifestError::UnknownDeviceType(16))
    );
    let mut devices = STAGE4_DEVICES;
    devices[1].device_type = 1;
    assert_eq!(
        manifest_hash(&devices),
        Err(ManifestError::DeviceTypeNotUnique(1))
    );
    // No PM entry: the FADT of S4-31e could not be built.
    let mut out = [0u8; STAGE4_BLOB_CAPACITY];
    assert_eq!(
        build_blob(&STAGE4_DEVICES[..7], &mut out),
        Err(BlobError::MissingAcpiPm)
    );
    // A buffer too small for header and RSDP slot.
    assert_eq!(
        build_blob(&STAGE4_DEVICES, &mut out[..0x80]),
        Err(BlobError::BufferTooSmall)
    );
    // A broken PM entry is refused with the manifest error.
    let mut devices = STAGE4_DEVICES;
    devices[7].gsi = GSI_NONE;
    assert!(matches!(
        build_blob(&devices, &mut out),
        Err(BlobError::Manifest(ManifestError::BadAcpiPm(_)))
    ));
}

#[test]
fn without_an_ecam_window_the_blob_has_the_dsdt_only() {
    let mut devices = STAGE4_DEVICES;
    devices[6] = absent(device_type::PCIE_ECAM);
    let blob = build(&devices, 0);
    assert_eq!(le32(&blob, 0x48), 1);
    assert_eq!(&blob[0x60..0x64], b"DSDT");
    assert_eq!(le32(&blob, 0x4C) as usize, blob.len());
    // Header 0x60 + one entry = 0x70.
    assert_eq!(le32(&blob, 0x38), 0x70);
}

/// The committed fixture (`tests/fixtures/guest/stage4/acpi_blob/`) is what the freestanding
/// build puts in `.acpi_blob`; the `od-machine` validator test reads the same file.
#[test]
fn fixture_equals_the_runtime_blob() {
    const FIXTURE: &[u8] =
        include_bytes!("../../../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
    assert_eq!(FIXTURE, build(&STAGE4_DEVICES, 0).as_slice());
}
