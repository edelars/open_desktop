//! S4-31d/g: the stage-4 blob. The header is read here by hard-coded offsets (not through
//! constants of the builder), the tables are compared with the run-time builders, and the build
//! is shown to depend on the device table only.

use super::*;
use crate::acpi::dsdt::{build_dsdt, validate_dsdt};
use crate::acpi::fadt::{build_facs, validate_facs, validate_fadt};
use crate::acpi::madt::validate_madt;
use crate::acpi::manifest::{AcpiPmEntry, BoardManifest, DeviceEntry};
use crate::acpi::mcfg::validate_mcfg;
use crate::acpi::rsdp::{validate_rsdp, validate_xsdt};
use crate::acpi::table::byte_sum;

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
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
    assert_eq!(&CONST_BLOB[..], build(&STAGE4_DEVICES, 0).as_slice());
}

/// Directory entry `i` as (signature, payload offset, length).
fn dir_entry(blob: &[u8], i: usize) -> ([u8; 4], usize, usize) {
    let at = 0x60 + i * 16;
    (
        blob[at..at + 4].try_into().unwrap(),
        le32(blob, at + 4) as usize,
        le32(blob, at + 8) as usize,
    )
}

/// The bytes of the table with `signature` inside the payload.
fn table<'a>(blob: &'a [u8], signature: &[u8; 4]) -> &'a [u8] {
    let payload = le32(blob, 0x38) as usize;
    for i in 0..le32(blob, 0x48) as usize {
        let (sig, offset, len) = dir_entry(blob, i);
        if &sig == signature {
            return &blob[payload + offset..payload + offset + len];
        }
    }
    panic!("no table {signature:?}");
}

#[test]
fn linux_nvme_dsdt_describes_the_bar_and_hardwired_inta() {
    let blob = build(&LINUX_NVME_DEVICES, 0);
    assert_eq!(blob, linux_nvme_blob_bytes::<LINUX_NVME_BLOB_LEN>());
    assert_nvme_resources(table(&blob, b"DSDT"));

    let mut devices = LINUX_NVME_DEVICES;
    devices[7].base = devices[6].base;
    let mut out = [0; STAGE4_BLOB_CAPACITY];
    assert_eq!(
        build_blob(&devices, &mut out),
        Err(BlobError::Manifest(ManifestError::BadPciNvme))
    );
}

fn assert_nvme_resources(dsdt: &[u8]) {
    let window = [
        0x87, 0x17, 0, 0, 0x0c, 1, // DWordMemory, producer, positive decode, fixed, RW.
        0, 0, 0, 0, // Granularity.
        0, 0x40, 0, 0xfc, // Minimum.
        0xff, 0x7f, 0, 0xfc, // Maximum.
        0, 0, 0, 0, // Translation.
        0, 0x40, 0, 0, // Length.
    ];
    let route = [
        0x08, b'_', b'P', b'R', b'T', 0x12, 0x0e, 1, 0x12, 0x0b, 4, 0x0c, 0xff, 0xff, 4, 0, 0, 0,
        0x0a, 16,
    ];
    assert!(dsdt.windows(window.len()).any(|bytes| bytes == window));
    assert!(dsdt.windows(route.len()).any(|bytes| bytes == route));
}

fn table_address(blob: &[u8], signature: &[u8; 4]) -> u64 {
    for i in 0..le32(blob, 0x48) as usize {
        let (sig, offset, _) = dir_entry(blob, i);
        if &sig == signature {
            return le64(blob, 0x30) + offset as u64;
        }
    }
    panic!("no table {signature:?}");
}

#[test]
fn tables_in_the_blob_are_the_runtime_ones() {
    let blob = build(&STAGE4_DEVICES, 0);
    let manifest = board_manifest();
    let pm = AcpiPmEntry::from_entry(&STAGE4_DEVICES[7]).unwrap();
    let mut dsdt = [0u8; 512];
    let dsdt_len = build_dsdt_with_s5(&manifest, pm.slp_typ_s5, &mut dsdt).unwrap();
    let mut madt = [0u8; 128];
    let madt_len = build_madt(&manifest, &pm, &mut madt).unwrap();
    let mut mcfg = [0u8; 128];
    let mcfg_len = build_mcfg(&manifest, &mut mcfg).unwrap();

    assert_eq!(le32(&blob, 0x48), 5);
    let order: Vec<[u8; 4]> = (0..5).map(|i| dir_entry(&blob, i).0).collect();
    assert_eq!(
        order,
        [*b"DSDT", *b"APIC", *b"FACP", *b"MCFG", *b"XSDT"],
        "DSDT first, then MADT, FADT, MCFG; the XSDT, which needs every address, last"
    );
    assert_eq!(table(&blob, b"DSDT"), &dsdt[..dsdt_len]);
    assert_eq!(table(&blob, b"APIC"), &madt[..madt_len]);
    assert_eq!(table(&blob, b"MCFG"), &mcfg[..mcfg_len]);
    assert_eq!(validate_dsdt(&dsdt[..dsdt_len]), Ok(()));
    assert_eq!(validate_madt(&madt[..madt_len]), Ok(()));
    assert_eq!(validate_mcfg(&mcfg[..mcfg_len]), Ok(()));
    // The DSDT of the blob is the variant with `\_S5` (15 bytes longer than build_dsdt's).
    let mut plain = [0u8; 512];
    assert_eq!(
        build_dsdt(&manifest, &mut plain).unwrap() + 15,
        dsdt_len,
        "the blob carries the DSDT with the \\_S5 package"
    );
    // The FADT is the run-time FADT for the addresses the layout gives.
    let addresses = FadtAddresses {
        facs: 0xE_F000,
        dsdt: table_address(&blob, b"DSDT"),
    };
    let mut fadt = [0u8; 300];
    let fadt_len = build_fadt(&pm, &addresses, &mut fadt).unwrap();
    assert_eq!(table(&blob, b"FACP"), &fadt[..fadt_len]);
    assert_eq!(validate_fadt(table(&blob, b"FACP")), Ok(()));
}

#[test]
fn rtc_handoff_advertises_the_same_rtc_in_dsdt_and_fadt() {
    let mut devices = LINUX_NVME_DEVICES.to_vec();
    devices.push(DeviceEntry {
        device_type: device_type::RTC_CMOS,
        flags: FLAG_PRESENT | FLAG_PIO,
        base: 0x70,
        length: 2,
        gsi: GSI_NONE,
        irq_flags: 0,
        param0: 1_704_067_200,
        param1: 0,
    });
    let blob = build(&devices, 0);
    let fadt = table(&blob, b"FACP");
    assert_eq!(fadt[108], 0x32, "FADT CENTURY points at CMOS register 0x32");
    assert_eq!(le16(fadt, 109) & (1 << 5), 0, "RTC is present");
    assert_eq!(validate_fadt(fadt), Ok(()));

    // Device(RTC0), _HID = EISA PNP0B00, _CRS = two Decode16 ports, EndTag.
    // ACPI 6.6 §9.14.1, §6.4.2.6 and §20.2.5.4.
    let rtc = [
        0x5b, 0x82, 0x22, b'R', b'T', b'C', b'0', 0x08, b'_', b'H', b'I', b'D', 0x0c, 0x41, 0xd0,
        0x0b, 0x00, 0x08, b'_', b'C', b'R', b'S', 0x11, 0x0d, 0x0a, 0x0a, 0x47, 0x01, 0x70, 0x00,
        0x70, 0x00, 0x01, 0x02, 0x79, 0x00,
    ];
    let dsdt = table(&blob, b"DSDT");
    assert_eq!(validate_dsdt(dsdt), Ok(()));
    assert_nvme_resources(dsdt);
    assert_eq!(
        dsdt.windows(rtc.len())
            .filter(|bytes| *bytes == rtc)
            .count(),
        1
    );
    let without_rtc = build(&LINUX_NVME_DEVICES, 0);
    assert!(
        !table(&without_rtc, b"DSDT")
            .windows(4)
            .any(|bytes| bytes == b"RTC0")
    );
    assert_ne!(le16(table(&without_rtc, b"FACP"), 109) & (1 << 5), 0);
}

#[test]
fn invalid_rtc_handoff_entry_is_rejected_before_table_build() {
    let mut devices = [DeviceEntry::default(); 9];
    devices[..STAGE4_DEVICES.len()].copy_from_slice(&STAGE4_DEVICES);
    devices[8] = DeviceEntry {
        device_type: device_type::RTC_CMOS,
        flags: FLAG_PRESENT | FLAG_PIO,
        base: 0x70,
        length: 2,
        gsi: GSI_NONE,
        irq_flags: 0,
        param0: device_type::MAX_RTC_UNIX_SECONDS + 1,
        param1: 0,
    };
    let mut out = [0xA5; STAGE4_BLOB_CAPACITY];
    assert!(matches!(
        build_blob(&devices, &mut out),
        Err(BlobError::Manifest(ManifestError::BadRtc("base UTC time")))
    ));
    assert!(out.iter().all(|byte| *byte == 0xA5));
}

#[test]
fn header_layout_is_the_documented_one() {
    let blob = build(&STAGE4_DEVICES, 0);
    assert_eq!(&blob[..8], b"ODACPIB1");
    assert_eq!(le32(&blob, 0x08), 2);
    assert_eq!(le32(&blob, 0x0C), 0x60);
    assert_eq!(&blob[0x10..0x30], &manifest_hash(&STAGE4_DEVICES).unwrap());
    assert_eq!(le64(&blob, 0x30), 0xE_0000);
    // Header 0x60 + five directory entries = 0xB0, already 16-aligned.
    assert_eq!(le32(&blob, 0x38), 0xB0);
    assert_eq!(le64(&blob, 0x40), 0xE_0000, "RSDP is at payload offset 0");
    assert_eq!(le32(&blob, 0x4C) as usize, blob.len());
    // The payload ends where the FACS segment starts; the FACS ends the blob.
    assert_eq!(le32(&blob, 0x38) + le32(&blob, 0x3C), le32(&blob, 0x50));
    assert_eq!(le32(&blob, 0x3C) % 16, 0);
    assert_eq!(le32(&blob, 0x54), 64);
    assert_eq!(le32(&blob, 0x50) + le32(&blob, 0x54), blob.len() as u32);
    assert_eq!(le64(&blob, 0x58), 0xE_F000);
    // Directory reserved words are zero.
    for i in 0..5 {
        assert_eq!(le32(&blob, 0x60 + i * 16 + 12), 0);
    }
}

#[test]
fn payload_layout_has_the_rsdp_in_the_slot_aligned_tables_and_zero_gaps() {
    let blob = build(&STAGE4_DEVICES, 0xFF);
    let payload_start = le32(&blob, 0x38) as usize;
    let payload = &blob[payload_start..payload_start + le32(&blob, 0x3C) as usize];
    // The RSDP occupies 36 bytes of the 0x40-byte slot; the rest of the slot is zero.
    assert_eq!(&payload[..8], b"RSD PTR ");
    assert!(payload[36..RSDP_SLOT_LEN].iter().all(|&b| b == 0));
    let mut covered = vec![false; payload.len()];
    for slot in covered.iter_mut().take(36) {
        *slot = true;
    }
    for i in 0..le32(&blob, 0x48) as usize {
        let (_, offset, len) = dir_entry(&blob, i);
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
    // Zero between the directory and the payload, and between the payload and the FACS.
    assert!(blob[0x60 + 5 * 16..payload_start].iter().all(|&b| b == 0));
    // Everything the load covers lies in the reclaim part of the region.
    let end = 0xE_0000 + le32(&blob, 0x3C) as u64;
    assert!(end <= 0xE_F000);
    // DSDT first, right after the slot: the address the FADT tests pin.
    assert_eq!(table_address(&blob, b"DSDT"), 0xE_0040);
}

#[test]
fn rsdp_xsdt_fadt_and_facs_point_at_the_real_offsets() {
    let blob = build(&STAGE4_DEVICES, 0);
    let base = le64(&blob, 0x30);
    let payload = le32(&blob, 0x38) as usize;
    // RSDP: at load_base, XsdtAddress = the XSDT in the payload, RsdtAddress 0.
    assert_eq!(le64(&blob, 0x40), base);
    let rsdp = &blob[payload..payload + 36];
    assert_eq!(validate_rsdp(rsdp), Ok(()));
    assert_eq!(le32(rsdp, 16), 0);
    assert_eq!(le64(rsdp, 24), table_address(&blob, b"XSDT"));
    // XSDT: FADT, MADT, MCFG, in this order; DSDT and FACS are not listed.
    let xsdt = table(&blob, b"XSDT");
    assert_eq!(validate_xsdt(xsdt), Ok(()));
    assert_eq!(xsdt.len(), 36 + 3 * 8);
    assert_eq!(le64(xsdt, 36), table_address(&blob, b"FACP"));
    assert_eq!(le64(xsdt, 44), table_address(&blob, b"APIC"));
    assert_eq!(le64(xsdt, 52), table_address(&blob, b"MCFG"));
    // FADT: FIRMWARE_CTRL/X_FIRMWARE_CTRL at 0xE_F000, DSDT/X_DSDT at the DSDT of the payload.
    let fadt = table(&blob, b"FACP");
    assert_eq!(le32(fadt, 36), 0xE_F000);
    assert_eq!(le64(fadt, 132), 0xE_F000);
    assert_eq!(le32(fadt, 40) as u64, table_address(&blob, b"DSDT"));
    assert_eq!(le64(fadt, 140), table_address(&blob, b"DSDT"));
    // FACS: a separate segment after the payload, loaded at 0xE_F000.
    let facs_at = le32(&blob, 0x50) as usize;
    let facs = &blob[facs_at..facs_at + 64];
    assert_eq!(validate_facs(facs), Ok(()));
    assert_eq!(le64(&blob, 0x58), 0xE_F000);
    let mut expected = [0u8; 64];
    build_facs(&mut expected).unwrap();
    assert_eq!(facs, expected.as_slice());
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
fn without_an_ecam_window_the_blob_has_no_mcfg_and_a_two_entry_xsdt() {
    let mut devices = STAGE4_DEVICES;
    devices[6] = absent(device_type::PCIE_ECAM);
    let blob = build(&devices, 0);
    assert_eq!(le32(&blob, 0x48), 4);
    let order: Vec<[u8; 4]> = (0..4).map(|i| dir_entry(&blob, i).0).collect();
    assert_eq!(order, [*b"DSDT", *b"APIC", *b"FACP", *b"XSDT"]);
    assert_eq!(le32(&blob, 0x4C) as usize, blob.len());
    // Header 0x60 + four entries = 0xA0.
    assert_eq!(le32(&blob, 0x38), 0xA0);
    let xsdt = table(&blob, b"XSDT");
    assert_eq!(xsdt.len(), 36 + 2 * 8);
    assert_eq!(le64(xsdt, 36), table_address(&blob, b"FACP"));
    assert_eq!(le64(xsdt, 44), table_address(&blob, b"APIC"));
}

#[test]
fn a_buffer_that_is_too_small_is_refused_at_every_step_without_panicking() {
    let full = build(&STAGE4_DEVICES, 0).len();
    for len in 0..full {
        let mut out = vec![0u8; len];
        assert!(
            build_blob(&STAGE4_DEVICES, &mut out).is_err(),
            "{len} bytes are not enough"
        );
    }
    let mut out = vec![0u8; full];
    assert_eq!(build_blob(&STAGE4_DEVICES, &mut out), Ok(full));
}

/// The committed fixture (`tests/fixtures/guest/stage4/acpi_blob/`) is what the freestanding
/// build puts in `.acpi_blob`; the `od-machine` validator test reads the same file.
#[test]
fn fixture_equals_the_runtime_blob() {
    const FIXTURE: &[u8] =
        include_bytes!("../../../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
    assert_eq!(FIXTURE, build(&STAGE4_DEVICES, 0).as_slice());
}
