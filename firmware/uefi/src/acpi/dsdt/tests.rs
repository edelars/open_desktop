//! DSDT tests (S7-12).
//!
//! The reference tables below were generated once by an independent Python program written from
//! the ACPI 6.4 text (AML grammar chapter 20, resource descriptors 6.4, table header 5.2.6); the
//! generator was not saved and shares no code with the Rust encoder. `ReturnOp` is not used by
//! the DSDT. Board values are the FIRMWARE_BOOT.md v1.0 defaults for COM1; the PCIe ECAM entry
//! is a *test configuration* (its base and length are not a board map; only segment and bus
//! range reach the DSDT).

use super::*;
use crate::acpi::manifest::{
    DEVICE_ENTRY_SIZE, DeviceEntry, FLAG_PIO, FLAG_PRESENT, GSI_NONE, IRQ_ACTIVE_LOW, IRQ_LEVEL,
};

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn entry(
    t: u32,
    flags: u32,
    base: u64,
    length: u64,
    gsi: u32,
    irq: u32,
    p0: u64,
    p1: u64,
) -> DeviceEntry {
    DeviceEntry {
        device_type: t,
        flags,
        base,
        length,
        gsi,
        irq_flags: irq,
        param0: p0,
        param1: p1,
    }
}

fn absent(t: u32) -> DeviceEntry {
    entry(t, 0, 0, 0, GSI_NONE, 0, 0, 0)
}

fn com1(base: u64, length: u64, gsi: u32, irq: u32, number: u64) -> DeviceEntry {
    entry(
        device_type::COM1,
        FLAG_PRESENT | FLAG_PIO,
        base,
        length,
        gsi,
        irq,
        number,
        0,
    )
}

/// Test configuration: one segment, buses 0..=0xFF (`param1 = start | end << 8`).
fn ecam(start: u64, end: u64) -> DeviceEntry {
    entry(
        device_type::PCIE_ECAM,
        FLAG_PRESENT,
        0xC000_0000,
        0x1000_0000,
        GSI_NONE,
        0,
        0,
        start | (end << 8),
    )
}

fn manifest(com: DeviceEntry, pci: DeviceEntry) -> BoardManifest {
    BoardManifest::from_entries([
        com,
        entry(
            device_type::LOCAL_APIC,
            FLAG_PRESENT,
            0xFEE0_0000,
            0x1000,
            GSI_NONE,
            0,
            0,
            0,
        ),
        entry(
            device_type::IOAPIC,
            FLAG_PRESENT,
            0xFEC0_0000,
            0x1000,
            GSI_NONE,
            0,
            0,
            0,
        ),
        entry(
            device_type::DISPLAY,
            FLAG_PRESENT,
            0xFC00_0000,
            0x1000,
            GSI_NONE,
            0,
            0,
            0,
        ),
        absent(device_type::NVRAM),
        absent(device_type::TPM_CRB),
        pci,
    ])
    .unwrap()
}

fn build(m: &BoardManifest) -> Vec<u8> {
    let mut out = [0u8; 1024];
    let len = build_dsdt(m, &mut out).unwrap();
    out[..len].to_vec()
}

fn build_err(m: &BoardManifest) -> DsdtError {
    let mut out = [0u8; 1024];
    build_dsdt(m, &mut out).unwrap_err()
}

/// `\_SB` with `PCI0` (segment 0, buses 0..=0xFF) and `COM1` (0x3F8, 8 ports, GSI 4, edge,
/// active-high). Layout: header 36 bytes (Length 0xB0); `10 4B 08` Scope(`\_SB_`, PkgLength 139);
/// `5B 82 4F 04` Device(`PCI0`, PkgLength 79) with `_HID` PNP0A08, `_CID` PNP0A03, `_UID` 0,
/// `_SEG` 0, `_BBN` 0, `_CRS` (Buffer 0x15: WordBusNumber 0..0xFF + End Tag), `_PRT` `12 02 00`;
/// `5B 82 31` Device(`COM1`) with `_HID` PNP0501, `_UID` 1, `_CRS` (Buffer 0x16: IO, Extended
/// Interrupt GSI 4, End Tag). Checksum 0x0E.
///
/// Пересчитано после удаления `_ADR` у PCI0 (устройство имеет `_HID` или `_ADR`, не оба): 6 bytes
/// (`08 5F 41 44 52 00`) dropped, so Length 0xB6 -> 0xB0, Scope PkgLength 145 -> 139
/// (`41 09` -> `4B 08`), Device PkgLength 85 -> 79 (`45 05` -> `4F 04`), checksum 0xDC -> 0x0E.
fn reference_both() -> Vec<u8> {
    hex("44 53 44 54 B0 00 00 00 02 0E 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 4B 08 5C 5F 53 42 5F 5B 82 4F 04 \
         50 43 49 30 08 5F 48 49 44 0C 41 D0 0A 08 08 5F \
         43 49 44 0C 41 D0 0A 03 08 5F 55 49 44 00 08 5F \
         53 45 47 00 08 5F 42 42 4E 00 08 5F 43 52 53 11 \
         15 0A 12 88 0D 00 02 0C 00 00 00 00 00 FF 00 00 \
         00 00 01 79 00 08 5F 50 52 54 12 02 00 5B 82 31 \
         43 4F 4D 31 08 5F 48 49 44 0C 41 D0 05 01 08 5F \
         55 49 44 01 08 5F 43 52 53 11 16 0A 13 47 01 F8 \
         03 F8 03 01 08 89 06 00 03 01 04 00 00 00 79 00")
}

/// `\_SB` with only `COM2` (0x2F8, GSI 3, level, active-low). Checksum 0x5A.
fn reference_com2_only() -> Vec<u8> {
    hex("44 53 44 54 5F 00 00 00 02 5A 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 3A 5C 5F 53 42 5F 5B 82 32 43 4F \
         4D 32 08 5F 48 49 44 0C 41 D0 05 01 08 5F 55 49 \
         44 0A 02 08 5F 43 52 53 11 16 0A 13 47 01 F8 02 \
         F8 02 01 08 89 06 00 05 01 03 00 00 00 79 00")
}

/// `\_SB` with no devices. Checksum 0x7F.
fn reference_empty() -> Vec<u8> {
    hex("44 53 44 54 2B 00 00 00 02 7F 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 06 5C 5F 53 42 5F")
}

#[test]
fn dsdt_for_pci_root_and_com1_matches_the_independent_reference() {
    let m = manifest(com1(0x3F8, 8, 4, 0, 1), ecam(0, 0xFF));
    assert_eq!(build(&m), reference_both());
}

#[test]
fn dsdt_for_com2_with_level_active_low_irq_matches_the_reference() {
    let m = manifest(
        com1(0x2F8, 8, 3, IRQ_LEVEL | IRQ_ACTIVE_LOW, 2),
        absent(device_type::PCIE_ECAM),
    );
    assert_eq!(build(&m), reference_com2_only());
}

#[test]
fn dsdt_without_devices_is_an_empty_system_bus_scope() {
    let m = manifest(absent(device_type::COM1), absent(device_type::PCIE_ECAM));
    assert_eq!(build(&m), reference_empty());
}

#[test]
fn absent_devices_are_not_emitted() {
    let only_pci = build(&manifest(absent(device_type::COM1), ecam(0, 0xFF)));
    let pnp0501 = [0x41, 0xD0, 0x05, 0x01];
    assert!(!only_pci.windows(4).any(|w| w == pnp0501));
    assert!(!only_pci.windows(4).any(|w| w == *b"COM1"));
    let only_com = build(&manifest(
        com1(0x3F8, 8, 4, 0, 1),
        absent(device_type::PCIE_ECAM),
    ));
    assert!(!only_com.windows(4).any(|w| w == *b"PCI0"));
    // Devices of S7-14 / S7-22 are never emitted: no RTC (PNP0B00), HPET (PNP0103), TPM.
    for table in [
        &only_pci,
        &only_com,
        &build(&manifest(com1(0x3F8, 8, 4, 0, 1), ecam(0, 0xFF))),
    ] {
        assert!(!table.windows(4).any(|w| w == [0x41, 0xD0, 0x0B, 0x00]));
        assert!(!table.windows(4).any(|w| w == [0x41, 0xD0, 0x01, 0x03]));
        assert!(!table.windows(4).any(|w| w == *b"MSFT"));
        assert!(!table.windows(4).any(|w| w == *b"_S5_"));
    }
}

#[test]
fn com1_without_an_interrupt_has_only_the_io_range() {
    let m = manifest(
        com1(0x3F8, 8, GSI_NONE, 0, 1),
        absent(device_type::PCIE_ECAM),
    );
    let table = build(&m);
    // `_CRS` buffer: IO (8 bytes) + End Tag (2) = 10 -> BufferOp 11, PkgLength 0D, 0A 0A.
    let crs = hex("08 5F 43 52 53 11 0D 0A 0A 47 01 F8 03 F8 03 01 08 79 00");
    assert!(table.windows(crs.len()).any(|w| w == crs.as_slice()));
    validate_dsdt(&table).unwrap();
}

#[test]
fn bus_range_reaches_bbn_seg_and_crs() {
    // Buses 4..=7: `_BBN` 4 (ByteConst 0A 04), `_CRS` WordBusNumber min 4 max 7 length 4.
    let m = manifest(absent(device_type::COM1), ecam(4, 7));
    let table = build(&m);
    let bbn = hex("08 5F 42 42 4E 0A 04");
    assert!(table.windows(bbn.len()).any(|w| w == bbn.as_slice()));
    let word = hex("88 0D 00 02 0C 00 00 00 04 00 07 00 00 00 04 00");
    assert!(table.windows(word.len()).any(|w| w == word.as_slice()));
}

#[test]
fn built_table_has_consistent_length_checksum_and_header() {
    let table = build(&manifest(com1(0x3F8, 8, 4, 0, 1), ecam(0, 0xFF)));
    assert_eq!(&table[..4], b"DSDT");
    assert_eq!(
        u32::from_le_bytes(table[4..8].try_into().unwrap()) as usize,
        table.len()
    );
    assert_eq!(table[8], 2);
    assert_eq!(table.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
    assert_eq!(&table[10..16], b"OPNDSK");
    assert_eq!(&table[16..24], b"ODFWDSDT");
    validate_dsdt(&table).unwrap();
}

#[test]
fn validator_rejects_a_corrupted_table() {
    let table = reference_both();
    let mut flipped = table.clone();
    flipped[50] ^= 0x01;
    assert!(matches!(
        validate_dsdt(&flipped),
        Err(DsdtError::Table(TableError::BadChecksum { .. }))
    ));
    assert!(matches!(
        validate_dsdt(&table[..table.len() - 1]),
        Err(DsdtError::Table(TableError::LengthMismatch { .. }))
    ));
    let mut wrong_signature = table.clone();
    wrong_signature[0] = b'X';
    assert_eq!(
        validate_dsdt(&wrong_signature),
        Err(DsdtError::Table(TableError::SignatureMismatch))
    );
    assert_eq!(
        validate_dsdt(&table[..10]),
        Err(DsdtError::Table(TableError::TooShort))
    );
}

#[test]
fn validator_rejects_a_repeated_name_even_with_a_correct_checksum() {
    // Scope(\_SB_) { Name(FOO_, 0) Name(FOO_, 0) } wrapped in a valid header.
    let aml = hex("10 12 5C 5F 53 42 5F 08 46 4F 4F 5F 00 08 46 4F 4F 5F 00");
    let mut table = vec![0u8; SDT_HEADER_LEN];
    write_header(
        &mut table,
        DSDT_SIGNATURE,
        DSDT_REVISION,
        DSDT_OEM_TABLE_ID,
        (SDT_HEADER_LEN + aml.len()) as u32,
    )
    .unwrap();
    table.extend_from_slice(&aml);
    finish_checksum(&mut table).unwrap();
    assert_eq!(
        validate_dsdt(&table),
        Err(DsdtError::Aml(AmlError::DuplicateName))
    );
}

#[test]
fn com1_entries_that_cannot_be_described_are_rejected() {
    let none = absent(device_type::PCIE_ECAM);
    let bad = |e: DeviceEntry| build_err(&manifest(e, none));
    let mut not_pio = com1(0x3F8, 8, 4, 0, 1);
    not_pio.flags = FLAG_PRESENT;
    assert_eq!(bad(not_pio), DsdtError::BadCom1);
    assert_eq!(bad(com1(0x3F8, 8, 4, 0, 0)), DsdtError::BadCom1);
    assert_eq!(bad(com1(0x3F8, 8, 4, 0, 10)), DsdtError::BadCom1);
    assert_eq!(bad(com1(0x3F8, 0, 4, 0, 1)), DsdtError::BadCom1);
    assert_eq!(bad(com1(0x3F8, 256, 4, 0, 1)), DsdtError::BadCom1);
    assert_eq!(bad(com1(0x1_0000, 8, 4, 0, 1)), DsdtError::BadCom1);
    assert_eq!(bad(com1(0xFFFC, 8, 4, 0, 1)), DsdtError::BadCom1);
}

#[test]
fn pci_entries_that_cannot_be_described_are_rejected() {
    let no_com = absent(device_type::COM1);
    let bad = |e: DeviceEntry| build_err(&manifest(no_com, e));
    let mut pio = ecam(0, 0xFF);
    pio.flags |= FLAG_PIO;
    assert_eq!(bad(pio), DsdtError::BadPcieEcam);
    let mut segment = ecam(0, 0xFF);
    segment.param0 = 0x1_0000;
    assert_eq!(bad(segment), DsdtError::BadPcieEcam);
    let mut reserved = ecam(0, 0xFF);
    reserved.param1 |= 1 << 16;
    assert_eq!(bad(reserved), DsdtError::BadPcieEcam);
    assert_eq!(bad(ecam(5, 4)), DsdtError::BadPcieEcam);
}

#[test]
fn small_output_buffers_are_reported() {
    let m = manifest(com1(0x3F8, 8, 4, 0, 1), ecam(0, 0xFF));
    let mut tiny = [0u8; 20];
    assert_eq!(
        build_dsdt(&m, &mut tiny),
        Err(DsdtError::Table(TableError::TooShort))
    );
    let mut short = [0u8; 60];
    assert_eq!(
        build_dsdt(&m, &mut short),
        Err(DsdtError::Aml(AmlError::BufferFull))
    );
}

#[test]
fn dsdt_built_from_raw_device_entries_matches_the_reference() {
    // Device table bytes laid out by hand from FIRMWARE_BOOT.md 6.3: type 0x00, flags 0x04, base
    // 0x08, length 0x10, gsi 0x18, irq_flags 0x1C, param0 0x20, param1 0x28 (little-endian).
    let raw = |t: u32, flags: u32, base: u64, len: u64, gsi: u32, irq: u32, p0: u64, p1: u64| {
        let mut b = [0u8; DEVICE_ENTRY_SIZE];
        b[0x00..0x04].copy_from_slice(&t.to_le_bytes());
        b[0x04..0x08].copy_from_slice(&flags.to_le_bytes());
        b[0x08..0x10].copy_from_slice(&base.to_le_bytes());
        b[0x10..0x18].copy_from_slice(&len.to_le_bytes());
        b[0x18..0x1C].copy_from_slice(&gsi.to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&irq.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&p0.to_le_bytes());
        b[0x28..0x30].copy_from_slice(&p1.to_le_bytes());
        DeviceEntry::parse(&b)
    };
    let none = GSI_NONE;
    let m = BoardManifest::from_entries([
        raw(7, 1, 0xC000_0000, 0x1000_0000, none, 0, 0, 0xFF00),
        raw(1, 3, 0x3F8, 8, 4, 0, 1, 0),
        raw(2, 1, 0xFEE0_0000, 0x1000, none, 0, 0, 0),
        raw(3, 1, 0xFEC0_0000, 0x1000, none, 0, 0, 0),
        raw(4, 1, 0xFC00_0000, 0x1000, none, 0, 0, 0),
        raw(5, 0, 0, 0, none, 0, 0, 0),
        raw(6, 0, 0, 0, none, 0, 0, 0),
    ])
    .unwrap();
    assert_eq!(build(&m), reference_both());
}
