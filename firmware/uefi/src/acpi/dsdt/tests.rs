//! DSDT tests (S7-12).
//!
//! The reference tables below were generated once by an independent Python program written from
//! the ACPI 6.4 text (AML grammar chapter 20, resource descriptors 6.4, table header 5.2.6); the
//! generator was not saved and shares no code with the Rust encoder. `ReturnOp` is not used by
//! the DSDT. Board values are the FIRMWARE_BOOT.md v1.0 defaults for COM1.
//!
//! S4-31b: the PCIe ECAM entry is the stage-4 board window (MACHINE_CONTRACT.md, S4-30c: memory
//! `[0xFC10_0000, +1 MiB)`, segment 0, buses 0..=0). The earlier S7-12 test configuration (base
//! 0xC000_0000, 256 MiB, buses 0..=0xFF) is outside the board contract and no longer builds; the
//! golden `reference_both` was replaced by a table for the board window (see its comment).

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

/// ECAM entry at the board base `0xFC10_0000`: buses `start..=end`, length
/// `(end - start + 1) << 20` for a consistent range (1 MiB otherwise). `param1 = start | end << 8`.
fn ecam(start: u64, end: u64) -> DeviceEntry {
    let buses = if end >= start { end - start + 1 } else { 1 };
    entry(
        device_type::PCIE_ECAM,
        FLAG_PRESENT,
        0xFC10_0000,
        buses << 20,
        GSI_NONE,
        0,
        0,
        start | (end << 8),
    )
}

/// The stage-4 board ECAM entry (MACHINE_CONTRACT.md, S4-30c): memory window
/// `[0xFC10_0000, +1 MiB)`, segment 0, buses 0..=0.
fn board_ecam() -> DeviceEntry {
    ecam(0, 0)
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

/// `\_SB` with `PCI0` (segment 0, buses 0..=0), `ECAM` (PNP0C02, window 0xFC10_0000 + 1 MiB)
/// and `COM1` (0x3F8, 8 ports, GSI 4, edge, active-high), for the board ECAM window.
///
/// Assembled by hand from the AML grammar (DefDevice 20.2.5.2: `5B 82` PkgLength NameString body;
/// DefName `08`; PkgLength 20.2.4: one byte below 64, else lead byte `0x40 | low nibble` and
/// `value >> 4`; Buffer `11` PkgLength size; Memory32Fixed 6.4.3.4: `86 09 00` info base length;
/// IO 6.4.2.5: `47` decode min max align length; WordBusNumber 6.4.3.5.3; EISA id `PNP0C02` =
/// `41 D0 0C 02`) and cross-checked by a separate Python script that computes every PkgLength
/// from the pieces (not saved, no code shared with the Rust encoder).
///
/// Layout: header 36 bytes (Length 0xE6); `10 41 0C` Scope(`\_SB_`, PkgLength 193 = 5 + 89 + 46 +
/// 51 + 2); `5B 82 47 05` Device(`PCI0`, PkgLength 87 = 2 + 4 + 81) with `_HID` PNP0A08, `_CID`
/// PNP0A03, `_UID` 0, `_SEG` 0, `_BBN` 0, `_CRS` (Buffer, PkgLength 0x1D = 1 + 2 + 26, size
/// `0A 1A`: WordBusNumber 0..0 length 1 (16 bytes), IO(Decode16, 0xCF8, 0xCF8, 1, 8) (8 bytes),
/// End Tag (2 bytes)), `_PRT` `12 02 00`: 89 bytes; `5B 82 2C` Device(`ECAM`, PkgLength 44 = 1 +
/// 4 + 39) with `_HID` PNP0C02, `_UID` 0, `_CRS` (Buffer, PkgLength 0x11 = 1 + 2 + 14, size
/// `0A 0E`: Memory32Fixed ReadWrite base `00 00 10 FC` length `00 00 10 00`, End Tag): 46 bytes;
/// `5B 82 31` Device(`COM1`) with `_HID` PNP0501, `_UID` 1, `_CRS` (Buffer 0x16: IO, Extended
/// Interrupt GSI 4, End Tag): 51 bytes. Total 36 + 1 + 2 + 191 = 230 = 0xE6 bytes, checksum 0xFE.
///
/// Replaces the S7-12 golden (PCI0 for buses 0..=0xFF, base 0xC000_0000, 256 MiB; Length 0xB0,
/// checksum 0x0E): that entry is outside the board contract (S4-30c) and MCFG-style checks reject
/// it. The COM1-only, COM2-only and empty tables did not change.
pub(super) fn reference_both() -> Vec<u8> {
    hex("44 53 44 54 E6 00 00 00 02 FE 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 41 0C 5C 5F 53 42 5F 5B 82 47 05 \
         50 43 49 30 08 5F 48 49 44 0C 41 D0 0A 08 08 5F \
         43 49 44 0C 41 D0 0A 03 08 5F 55 49 44 00 08 5F \
         53 45 47 00 08 5F 42 42 4E 00 08 5F 43 52 53 11 \
         1D 0A 1A 88 0D 00 02 0C 00 00 00 00 00 00 00 00 \
         00 01 00 47 01 F8 0C F8 0C 01 08 79 00 08 5F 50 \
         52 54 12 02 00 5B 82 2C 45 43 41 4D 08 5F 48 49 \
         44 0C 41 D0 0C 02 08 5F 55 49 44 00 08 5F 43 52 \
         53 11 11 0A 0E 86 09 00 01 00 00 10 FC 00 00 10 \
         00 79 00 5B 82 31 43 4F 4D 31 08 5F 48 49 44 0C \
         41 D0 05 01 08 5F 55 49 44 01 08 5F 43 52 53 11 \
         16 0A 13 47 01 F8 03 F8 03 01 08 89 06 00 03 01 \
         04 00 00 00 79 00")
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
    let m = manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam());
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
    let only_pci = build(&manifest(absent(device_type::COM1), board_ecam()));
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
        &build(&manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam())),
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
fn board_bus_range_reaches_seg_bbn_and_crs() {
    // Buses 0..=0 (the only range the board contract allows): `_SEG` 0, `_BBN` 0 (`00`),
    // `_CRS` WordBusNumber min 0 max 0 length 1.
    let table = build(&manifest(absent(device_type::COM1), board_ecam()));
    for needle in [
        hex("08 5F 53 45 47 00"),
        hex("08 5F 42 42 4E 00"),
        hex("88 0D 00 02 0C 00 00 00 00 00 00 00 00 00 01 00"),
    ] {
        assert!(table.windows(needle.len()).any(|w| w == needle.as_slice()));
    }
}

#[test]
fn built_table_has_consistent_length_checksum_and_header() {
    let table = build(&manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam()));
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
fn small_output_buffers_are_reported() {
    let m = manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam());
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
        raw(7, 1, 0xFC10_0000, 0x10_0000, none, 0, 0, 0),
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

#[test]
fn ecam_entry_emitted_by_the_board_with_the_pci_flag_is_accepted() {
    // The 48 raw bytes `od-machine` writes for device code 7 when the board is built with
    // `pci_ecam` (S4-30c): PRESENT, MMIO, base 0xFC10_0000, length 0x10_0000, no IRQ,
    // segment 0, buses 0..=0 (`firmware_board.rs` test `pci_ecam_flag_makes_code_7_present_...`).
    let mut raw = [0u8; DEVICE_ENTRY_SIZE];
    raw[0x00..0x04].copy_from_slice(&7u32.to_le_bytes());
    raw[0x04..0x08].copy_from_slice(&1u32.to_le_bytes());
    raw[0x08..0x10].copy_from_slice(&0xFC10_0000u64.to_le_bytes());
    raw[0x10..0x18].copy_from_slice(&0x10_0000u64.to_le_bytes());
    raw[0x18..0x1C].copy_from_slice(&GSI_NONE.to_le_bytes());
    let pci = DeviceEntry::parse(&raw);
    assert!(pci.is_present() && !pci.is_pio());
    let table = build(&manifest(com1(0x3F8, 8, 4, 0, 1), pci));
    // `_BBN` 0 and `_CRS` WordBusNumber min 0 max 0 length 1.
    let bbn = hex("08 5F 42 42 4E 00");
    assert!(table.windows(bbn.len()).any(|w| w == bbn.as_slice()));
    let word = hex("88 0D 00 02 0C 00 00 00 00 00 00 00 00 00 01 00");
    assert!(table.windows(word.len()).any(|w| w == word.as_slice()));
    assert!(table.windows(4).any(|w| w == *b"PCI0"));
    validate_dsdt(&table).unwrap();
}

// ---------------------------------------------------------------------------------------------
// S4-31b: PNP0C02 reservation and the conf1 ports in `build_dsdt`. `reference_both` above and
// `reference_ecam_only` below were assembled by hand from the ACPI AML grammar and cross-checked
// by a separate script (see the comment of `reference_both`); they share no code with the encoder.
// ---------------------------------------------------------------------------------------------

use crate::acpi::mcfg::McfgError;

/// The same as `reference_both` without `COM1`: Scope body 5 + 89 + 46 = 140, PkgLength 142 =
/// `4E 08`, table 36 + 1 + 2 + 140 = 179 = 0xB3 bytes, checksum 0xD9.
fn reference_ecam_only() -> Vec<u8> {
    hex("44 53 44 54 B3 00 00 00 02 D9 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 4E 08 5C 5F 53 42 5F 5B 82 47 05 \
         50 43 49 30 08 5F 48 49 44 0C 41 D0 0A 08 08 5F \
         43 49 44 0C 41 D0 0A 03 08 5F 55 49 44 00 08 5F \
         53 45 47 00 08 5F 42 42 4E 00 08 5F 43 52 53 11 \
         1D 0A 1A 88 0D 00 02 0C 00 00 00 00 00 00 00 00 \
         00 01 00 47 01 F8 0C F8 0C 01 08 79 00 08 5F 50 \
         52 54 12 02 00 5B 82 2C 45 43 41 4D 08 5F 48 49 \
         44 0C 41 D0 0C 02 08 5F 55 49 44 00 08 5F 43 52 \
         53 11 11 0A 0E 86 09 00 01 00 00 10 FC 00 00 10 \
         00 79 00")
}

fn contains(table: &[u8], needle: &[u8]) -> bool {
    table.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn board_ecam_without_com1_matches_the_hand_assembled_reference() {
    let table = build(&manifest(absent(device_type::COM1), board_ecam()));
    assert_eq!(table, reference_ecam_only());
    // Length and checksum separately, not through the encoder.
    assert_eq!(table.len(), 0xB3);
    assert_eq!(&table[4..8], &0xB3u32.to_le_bytes());
    assert_eq!(table[9], 0xD9);
    assert_eq!(table.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
    validate_dsdt(&table).unwrap();
}

#[test]
fn board_ecam_with_com1_has_the_golden_length_and_checksum() {
    let table = build(&manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam()));
    assert_eq!(table, reference_both());
    assert_eq!(table.len(), 0xE6);
    assert_eq!(&table[4..8], &0xE6u32.to_le_bytes());
    assert_eq!(table[9], 0xFE);
    validate_dsdt(&table).unwrap();
    // COM1 is the same 51 bytes as in the COM-only tables: the ECAM-only table plus COM1.
    assert!(table.ends_with(&reference_both()[0xE6 - 51..]));
}

#[test]
fn pnp0c02_device_carries_the_ecam_window_as_memory32_fixed() {
    let table = build(&manifest(absent(device_type::COM1), board_ecam()));
    let device = hex(
        "5B 82 2C 45 43 41 4D 08 5F 48 49 44 0C 41 D0 0C 02 08 5F 55 49 44 00 \
                      08 5F 43 52 53 11 11 0A 0E 86 09 00 01 00 00 10 FC 00 00 10 00 79 00",
    );
    assert!(contains(&table, &device));
    // Another base moves only the base bytes of the descriptor.
    let mut moved = board_ecam();
    moved.base = 0xFC20_0000;
    let table = build(&manifest(absent(device_type::COM1), moved));
    assert!(contains(
        &table,
        &hex("86 09 00 01 00 00 20 FC 00 00 10 00")
    ));
    assert!(!contains(&table, &hex("86 09 00 01 00 00 10 FC")));
    validate_dsdt(&table).unwrap();
    // A window that ends exactly at 4 GiB is valid (Memory32Fixed base 0xFFF00000).
    let mut top = board_ecam();
    top.base = 0xFFF0_0000;
    let table = build(&manifest(absent(device_type::COM1), top));
    assert!(contains(
        &table,
        &hex("86 09 00 01 00 00 F0 FF 00 00 10 00")
    ));
    validate_dsdt(&table).unwrap();
}

#[test]
fn pci0_crs_has_the_conf1_io_range_after_the_bus_number() {
    let table = build(&manifest(absent(device_type::COM1), board_ecam()));
    // `_CRS` of PCI0 whole: WordBusNumber 0..0, IO 0xCF8..0xCF8 align 1 length 8, End Tag.
    let crs = hex(
        "08 5F 43 52 53 11 1D 0A 1A 88 0D 00 02 0C 00 00 00 00 00 00 00 00 00 01 00 \
                   47 01 F8 0C F8 0C 01 08 79 00",
    );
    assert!(contains(&table, &crs));
    // Exactly one conf1 descriptor in the table.
    let io = hex("47 01 F8 0C F8 0C 01 08");
    assert_eq!(
        table
            .windows(io.len())
            .filter(|w| *w == io.as_slice())
            .count(),
        1
    );
}

#[test]
fn without_an_ecam_entry_there_is_no_pci0_no_conf1_and_no_pnp0c02() {
    let none = absent(device_type::PCIE_ECAM);
    for m in [
        manifest(com1(0x2F8, 8, 3, IRQ_LEVEL | IRQ_ACTIVE_LOW, 2), none),
        manifest(absent(device_type::COM1), none),
        manifest(com1(0x3F8, 8, 4, 0, 1), none),
    ] {
        let table = build(&m);
        assert!(!contains(&table, b"PCI0"));
        assert!(!contains(&table, b"ECAM"));
        assert!(!contains(&table, &hex("47 01 F8 0C F8 0C 01 08")));
        assert!(!contains(&table, &[0x41, 0xD0, 0x0C, 0x02]));
    }
}

#[test]
fn ecam_entries_that_mcfg_rejects_are_rejected_with_the_mcfg_reason() {
    let none = absent(device_type::COM1);
    let bad = |e: DeviceEntry| build_err(&manifest(none, e));
    let mut pio = board_ecam();
    pio.flags |= FLAG_PIO;
    assert_eq!(bad(pio), DsdtError::Ecam(McfgError::PioWindow));
    let mut short = board_ecam();
    short.length = 0x1000;
    assert_eq!(
        bad(short),
        DsdtError::Ecam(McfgError::LengthMismatch {
            expected: 0x10_0000,
            actual: 0x1000,
        })
    );
    let mut zero = board_ecam();
    zero.length = 0;
    assert!(matches!(
        bad(zero),
        DsdtError::Ecam(McfgError::LengthMismatch { .. })
    ));
    let mut high = board_ecam();
    high.base = 0x1_0000_0000;
    assert_eq!(
        bad(high),
        DsdtError::Ecam(McfgError::WindowAbove4Gib { end: 0x1_0010_0000 })
    );
    let mut zero_base = board_ecam();
    zero_base.base = 0;
    assert_eq!(bad(zero_base), DsdtError::Ecam(McfgError::ZeroBase));
    let mut unaligned = board_ecam();
    unaligned.base = 0xFC10_1000;
    assert_eq!(
        bad(unaligned),
        DsdtError::Ecam(McfgError::BaseUnaligned(0xFC10_1000))
    );
    // Segment: above 16 bits, and a valid segment outside the board contract.
    let mut huge_segment = board_ecam();
    huge_segment.param0 = 0x1_0000;
    assert_eq!(
        bad(huge_segment),
        DsdtError::Ecam(McfgError::SegmentOutOfRange(0x1_0000))
    );
    let mut segment = board_ecam();
    segment.param0 = 1;
    assert_eq!(
        bad(segment),
        DsdtError::Ecam(McfgError::OutsideStage4Contract {
            segment: 1,
            start: 0,
            end: 0,
        })
    );
    // Bus order and range: inverted, and a consistent range outside buses 0..=0 .
    assert_eq!(
        bad(ecam(5, 4)),
        DsdtError::Ecam(McfgError::BusRangeInverted { start: 5, end: 4 })
    );
    assert!(matches!(
        bad(ecam(4, 7)),
        DsdtError::Ecam(McfgError::OutsideStage4Contract {
            start: 4,
            end: 7,
            ..
        })
    ));
    // The former S7-12 range 0..=0xFF needs 256 MiB: at the board base it passes the length
    // check and ends above 4 GiB; at a low base it is outside the board contract.
    assert!(matches!(
        bad(ecam(0, 0xFF)),
        DsdtError::Ecam(McfgError::WindowAbove4Gib { .. })
    ));
    let mut wide = ecam(0, 0xFF);
    wide.base = 0xC000_0000;
    assert!(matches!(
        bad(wide),
        DsdtError::Ecam(McfgError::OutsideStage4Contract { end: 0xFF, .. })
    ));
    // `param1` bits above bit 15.
    let mut reserved = board_ecam();
    reserved.param1 = 1 << 16;
    assert_eq!(
        bad(reserved),
        DsdtError::Ecam(McfgError::ReservedParamBits(1 << 16))
    );
}

// ---------------------------------------------------------------------------------------------
// S4-31e: `\_S5`. `build_dsdt` (and every golden above) is unchanged; `build_dsdt_with_s5` is a
// separate variant. The reference below is `reference_ecam_only` with the 15 bytes of
// `Name(\_S5, Package() { 5, 5, 0, 0 })` appended after the `\_SB` scope, assembled by hand:
// NameOp 08, RootChar 5C + NameSeg `_S5_` (5F 53 35 5F), PackageOp 12, PkgLength 08 (1 + 1 + 2 + 2
// + 1 + 1), NumElements 04, BytePrefix 0A 05 twice (SLP_TYPa, SLP_TYPb), ZeroOp 00 twice. The
// table grows from 0xB3 to 0xC2 bytes, the checksum from 0xD9 to 0xE4 (the sum of the 15 new
// bytes is 486 and the length byte grows by 15: 217 - 501 = 228 mod 256 = 0xE4, worked out by
// hand and confirmed by a separate script).
// ---------------------------------------------------------------------------------------------

const S5_BYTES: [u8; 15] = [
    0x08, 0x5C, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x08, 0x04, 0x0A, 0x05, 0x0A, 0x05, 0x00, 0x00,
];

fn reference_ecam_only_with_s5() -> Vec<u8> {
    hex("44 53 44 54 C2 00 00 00 02 E4 4F 50 4E 44 53 4B \
         4F 44 46 57 44 53 44 54 01 00 00 00 4F 44 46 57 \
         01 00 00 00 10 4E 08 5C 5F 53 42 5F 5B 82 47 05 \
         50 43 49 30 08 5F 48 49 44 0C 41 D0 0A 08 08 5F \
         43 49 44 0C 41 D0 0A 03 08 5F 55 49 44 00 08 5F \
         53 45 47 00 08 5F 42 42 4E 00 08 5F 43 52 53 11 \
         1D 0A 1A 88 0D 00 02 0C 00 00 00 00 00 00 00 00 \
         00 01 00 47 01 F8 0C F8 0C 01 08 79 00 08 5F 50 \
         52 54 12 02 00 5B 82 2C 45 43 41 4D 08 5F 48 49 \
         44 0C 41 D0 0C 02 08 5F 55 49 44 00 08 5F 43 52 \
         53 11 11 0A 0E 86 09 00 01 00 00 10 FC 00 00 10 \
         00 79 00 08 5C 5F 53 35 5F 12 08 04 0A 05 0A 05 \
         00 00")
}

fn build_s5(m: &BoardManifest, typ: u8) -> Vec<u8> {
    let mut out = [0u8; 1024];
    let len = build_dsdt_with_s5(m, typ, &mut out).unwrap();
    out[..len].to_vec()
}

#[test]
fn dsdt_with_s5_matches_the_hand_assembled_reference() {
    let table = build_s5(&manifest(absent(device_type::COM1), board_ecam()), 5);
    assert_eq!(table, reference_ecam_only_with_s5());
    assert_eq!(table.len(), 0xC2);
    assert_eq!(table[9], 0xE4);
    assert_eq!(&table[table.len() - 15..], &S5_BYTES);
    validate_dsdt(&table).unwrap();
}

#[test]
fn building_without_s5_still_gives_the_old_golden_bytes() {
    // The `_S5`-less builder is untouched: both goldens of S4-31b still match, with and
    // without COM1.
    let no_com = manifest(absent(device_type::COM1), board_ecam());
    assert_eq!(build(&no_com), reference_ecam_only());
    let with_com = manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam());
    assert_eq!(build(&with_com), reference_both());
}

#[test]
fn s5_follows_the_scope_and_leaves_the_rest_of_the_table_alone() {
    // With COM1 too: the variant is the old table, three header bytes changed (Length, checksum)
    // and the 15 `_S5` bytes appended.
    let m = manifest(com1(0x3F8, 8, 4, 0, 1), board_ecam());
    let plain = build(&m);
    let with_s5 = build_s5(&m, 5);
    assert_eq!(with_s5.len(), plain.len() + 15);
    assert_eq!(&with_s5[36..plain.len()], &plain[36..]);
    assert_eq!(&with_s5[plain.len()..], &S5_BYTES);
    assert_eq!(&with_s5[..4], b"DSDT");
    assert_eq!(&with_s5[4..8], &(with_s5.len() as u32).to_le_bytes());
    assert_eq!(with_s5.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
    validate_dsdt(&with_s5).unwrap();
}

#[test]
fn s5_value_is_written_for_both_pm1a_and_pm1b_and_is_checked() {
    let m = manifest(absent(device_type::COM1), board_ecam());
    let table = build_s5(&m, 7);
    assert_eq!(
        &table[table.len() - 7..],
        &[0x04, 0x0A, 0x07, 0x0A, 0x07, 0, 0][..]
    );
    let mut out = [0u8; 1024];
    assert_eq!(
        build_dsdt_with_s5(&m, 8, &mut out),
        Err(DsdtError::SlpTypOutOfRange(8))
    );
    // An output buffer too small for the extra bytes is an error, not a truncated table.
    let mut small = [0u8; 0xC1];
    assert!(build_dsdt_with_s5(&m, 5, &mut small).is_err());
}

const fn const_entry(device_type: u32, flags: u32, base: u64, length: u64) -> DeviceEntry {
    DeviceEntry {
        device_type,
        flags,
        base,
        length,
        gsi: GSI_NONE,
        irq_flags: 0,
        param0: 0,
        param1: 0,
    }
}

#[test]
fn const_dsdt_with_s5_equals_the_runtime_one() {
    const BUILT: ([u8; 256], usize) = {
        let manifest = match BoardManifest::from_entries([
            const_entry(1, 0, 0, 0),
            const_entry(2, 0, 0, 0),
            const_entry(3, 0, 0, 0),
            const_entry(4, 0, 0, 0),
            const_entry(5, 0, 0, 0),
            const_entry(6, 0, 0, 0),
            const_entry(7, FLAG_PRESENT, 0xFC10_0000, 0x10_0000),
        ]) {
            Ok(m) => m,
            Err(_) => panic!("manifest"),
        };
        let mut out = [0u8; 256];
        match build_dsdt_with_s5(&manifest, 5, &mut out) {
            Ok(len) => (out, len),
            Err(_) => panic!("build_dsdt_with_s5 failed in a const context"),
        }
    };
    assert_eq!(
        &BUILT.0[..BUILT.1],
        reference_ecam_only_with_s5().as_slice()
    );
}
