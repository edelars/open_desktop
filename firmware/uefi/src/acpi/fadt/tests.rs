//! S4-31e: FADT and FACS.
//!
//! The reference tables were assembled field by field from the ACPI FADT/FACS layouts (offsets in
//! the comments of `reference_fadt`) and written out by a separate Python script that shares no
//! code with the serializer; the checksum 0x6B was also summed by hand (sum of all other bytes =
//! 3221 = 0xC95 -> 0x95, and 0x100 - 0x95 = 0x6B). Offsets are used as literals here, not
//! through the constants of the serializer.

use super::*;
use crate::acpi::blob::{ACPI_REGION_BASE, STAGE4_BLOB_CAPACITY, STAGE4_DEVICES, build_blob};
use crate::acpi::manifest::AcpiPmEntry;
use crate::acpi::table::byte_sum;

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// The PM entry of the stage-4 board (code 9 of the fixture device table, equal to
/// `ACPI_PM_DESCRIPTOR` of S7-14a).
fn board_pm() -> AcpiPmEntry {
    AcpiPmEntry::from_entry(&STAGE4_DEVICES[7]).unwrap()
}

/// FACS at `0xE_F000` (contract of S4-31c), DSDT at `0xE_0040`: the place of the DSDT in the
/// blob of S4-31d (RSDP slot of 0x40 bytes first), see `fixture_layout_gives_the_dsdt_address`.
const ADDRESSES: FadtAddresses = FadtAddresses {
    facs: 0xE_F000,
    dsdt: 0xE_0040,
};

fn build(pm: &AcpiPmEntry, addresses: &FadtAddresses, fill: u8) -> Vec<u8> {
    let mut out = [fill; 300];
    let len = build_fadt(pm, addresses, &mut out).unwrap();
    assert_eq!(len, 276);
    out[..len].to_vec()
}

fn board_fadt() -> Vec<u8> {
    build(&board_pm(), &ADDRESSES, 0)
}

/// Recomputes the checksum after a test changed a byte.
fn fix_checksum(table: &mut [u8]) {
    table[9] = 0;
    table[9] = byte_sum(table).wrapping_neg();
}

/// FADT revision 6 for the board, 276 bytes. Offsets: header 0..36 (signature `FACP`, Length
/// 0x114, Revision 6, checksum 0x6B, OEM ID `OPNDSK`, OEM table ID `ODFWFACP`, OEM rev 1,
/// creator `ODFW`, creator rev 1); 36 FIRMWARE_CTRL 0xE_F000; 40 DSDT 0xE_0040; 44 INT_MODEL 0;
/// 45 Preferred_PM_Profile 1; 46 SCI_INT 9; 48..56 SMI_CMD, ACPI_ENABLE/DISABLE, S4BIOS_REQ,
/// PSTATE_CNT 0; 56 PM1a_EVT_BLK 0x600; 60 PM1b_EVT 0; 64 PM1a_CNT 0x604; 68 PM1b_CNT 0; 72 PM2_CNT
/// 0; 76 PM_TMR 0x608; 80/84 GPE0/GPE1 0; 88 PM1_EVT_LEN 4; 89 PM1_CNT_LEN 2; 90 PM2_CNT_LEN 0; 91
/// PM_TMR_LEN 4; 92..96 GPE lens, GPE1_BASE, CST_CNT 0; 96 P_LVL2_LAT 0x0FFF; 98 P_LVL3_LAT 0x0FFF;
/// 100..109 FLUSH_*, DUTY_*, alarms, CENTURY 0; 109 IAPC_BOOT_ARCH 0x0024 (VGA_NOT_PRESENT bit 2,
/// CMOS_RTC_NOT_PRESENT bit 5); 111 reserved; 112 Flags 0x470 (bits 4, 5, 6, 10); 116 RESET_REG
/// (SystemIO 1, width 8, offset 0, access byte 1, address 0x60C); 128 RESET_VALUE 6; 129
/// ARM_BOOT_ARCH 0; 131 minor version 4; 132 X_FIRMWARE_CTRL; 140 X_DSDT; 148 X_PM1a_EVT_BLK (1,
/// 32, 0, 0, 0x600); 160 X_PM1b_EVT 0; 172 X_PM1a_CNT_BLK (1, 16, 0, 0, 0x604); 184/196 X_PM1b_CNT,
/// X_PM2_CNT 0; 208 X_PM_TMR_BLK (1, 32, 0, 0, 0x608); 220..276 GPE, sleep registers and the
/// hypervisor vendor identity 0.
fn reference_fadt() -> Vec<u8> {
    hex("46 41 43 50 14 01 00 00 06 6B 4F 50 4E 44 53 4B \
         4F 44 46 57 46 41 43 50 01 00 00 00 4F 44 46 57 \
         01 00 00 00 00 F0 0E 00 40 00 0E 00 00 01 09 00 \
         00 00 00 00 00 00 00 00 00 06 00 00 00 00 00 00 \
         04 06 00 00 00 00 00 00 00 00 00 00 08 06 00 00 \
         00 00 00 00 00 00 00 00 04 02 00 04 00 00 00 00 \
         FF 0F FF 0F 00 00 00 00 00 00 00 00 00 24 00 00 \
         70 04 00 00 01 08 00 01 0C 06 00 00 00 00 00 00 \
         06 00 00 04 00 F0 0E 00 00 00 00 00 40 00 0E 00 \
         00 00 00 00 01 20 00 00 00 06 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 01 10 00 00 \
         04 06 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         01 20 00 00 08 06 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00")
}

/// FACS: `FACS`, Length 64, version 2 at offset 32, everything else 0.
fn reference_facs() -> Vec<u8> {
    hex("46 41 43 53 40 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         02 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00")
}

#[test]
fn fadt_matches_the_hand_assembled_reference() {
    let table = board_fadt();
    assert_eq!(table, reference_fadt());
    assert_eq!(table.len(), 276);
    assert_eq!(&table[4..8], &276u32.to_le_bytes());
    assert_eq!(table[9], 0x6B);
    assert_eq!(table.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
}

#[test]
fn fadt_fields_equal_the_pm_descriptor_and_the_decisions() {
    let t = board_fadt();
    // ACPI_PM_DESCRIPTOR (S7-14a): EVT 0x600/4, CNT 0x604/2, TMR 0x608/4, RESET 0x60C, value 6.
    assert_eq!(le32(&t, 56), 0x600);
    assert_eq!(t[88], 4);
    assert_eq!(le32(&t, 64), 0x604);
    assert_eq!(t[89], 2);
    assert_eq!(le32(&t, 76), 0x608);
    assert_eq!(t[91], 4);
    assert_eq!(
        &t[116..120],
        &[1, 8, 0, 1][..],
        "SystemIO, 8 bit, offset 0, byte access"
    );
    assert_eq!(le64(&t, 120), 0x60C);
    assert_eq!(t[128], 0x06);
    // Not implemented: SMI, PM1b, PM2, GPE.
    assert!(
        t[48..56].iter().all(|&b| b == 0),
        "SMI_CMD, ACPI_ENABLE/DISABLE, ..."
    );
    for at in [60, 68, 72, 80, 84] {
        assert_eq!(le32(&t, at), 0, "PM1b/PM2/GPE block at {at}");
    }
    assert_eq!(t[90], 0);
    assert!(t[92..96].iter().all(|&b| b == 0));
    // SCI GSI 9 (S4-31c), PM profile Desktop.
    assert_eq!(le16(&t, 46), 9);
    assert_eq!(t[45], 1);
    // Flags: PWR_BUTTON, SLP_BUTTON, FIX_RTC, RESET_REG_SUP set; TMR_VAL_EXT, HW_REDUCED clear.
    let flags = le32(&t, 112);
    assert_eq!(flags, 0x470);
    for (bit, name, expected) in [
        (4, "PWR_BUTTON", true),
        (5, "SLP_BUTTON", true),
        (6, "FIX_RTC", true),
        (8, "TMR_VAL_EXT", false),
        (10, "RESET_REG_SUP", true),
        (20, "HW_REDUCED_ACPI", false),
    ] {
        assert_eq!(flags >> bit & 1 == 1, expected, "{name}");
    }
    // IAPC_BOOT_ARCH: VGA_NOT_PRESENT and CMOS_RTC_NOT_PRESENT only (8042 = 0, LEGACY_DEVICES = 0).
    assert_eq!(le16(&t, 109), 0x24);
    // The extended copies of the PM registers and the tables.
    assert_eq!(le64(&t, 132), 0xE_F000);
    assert_eq!(le64(&t, 140), 0xE_0040);
    assert_eq!(le32(&t, 36) as u64, le64(&t, 132));
    assert_eq!(le32(&t, 40) as u64, le64(&t, 140));
    assert_eq!(&t[148..152], &[1, 32, 0, 0][..]);
    assert_eq!(le64(&t, 152), 0x600);
    assert_eq!(&t[172..176], &[1, 16, 0, 0][..]);
    assert_eq!(le64(&t, 176), 0x604);
    assert_eq!(&t[208..212], &[1, 32, 0, 0][..]);
    assert_eq!(le64(&t, 212), 0x608);
    // Header identification and revision.
    assert_eq!(&t[0..4], b"FACP");
    assert_eq!(t[8], 6);
    assert_eq!(&t[10..16], b"OPNDSK");
    assert_eq!(&t[16..24], b"ODFWFACP");
    assert_eq!(t[131], 4);
}

#[test]
fn fadt_follows_other_pm_entry_values() {
    // The serializer reads the PM entry: a different (valid) block moves the fields.
    let mut entry = STAGE4_DEVICES[7];
    entry.base = 0x700;
    entry.param0 = 0x070C_0708_0704_0700;
    entry.param1 = 0x0301;
    entry.gsi = 10;
    let pm = AcpiPmEntry::from_entry(&entry).unwrap();
    let t = build(&pm, &ADDRESSES, 0);
    assert_eq!(le32(&t, 56), 0x700);
    assert_eq!(le32(&t, 64), 0x704);
    assert_eq!(le32(&t, 76), 0x708);
    assert_eq!(le64(&t, 120), 0x70C);
    assert_eq!(t[128], 1);
    assert_eq!(le16(&t, 46), 10);
    assert_eq!(le64(&t, 152), 0x700);
    validate_fadt(&t).unwrap();
}

#[test]
fn fadt_is_deterministic_and_does_not_depend_on_the_buffer() {
    let a = build(&board_pm(), &ADDRESSES, 0x00);
    let b = build(&board_pm(), &ADDRESSES, 0xFF);
    assert_eq!(a, b);
    assert_eq!(a, board_fadt());
}

#[test]
fn const_fadt_and_facs_equal_the_runtime_ones() {
    const PM: AcpiPmEntry = match AcpiPmEntry::from_entry(&STAGE4_DEVICES[7]) {
        Ok(pm) => pm,
        Err(_) => panic!("pm entry"),
    };
    const FADT: ([u8; 276], usize) = {
        let mut out = [0u8; 276];
        match build_fadt(&PM, &ADDRESSES, &mut out) {
            Ok(len) => (out, len),
            Err(_) => panic!("build_fadt failed in a const context"),
        }
    };
    const FACS: ([u8; 64], usize) = {
        let mut out = [0u8; 64];
        match build_facs(&mut out) {
            Ok(len) => (out, len),
            Err(_) => panic!("build_facs failed in a const context"),
        }
    };
    assert_eq!(&FADT.0[..FADT.1], reference_fadt().as_slice());
    assert_eq!(&FACS.0[..FACS.1], reference_facs().as_slice());
}

#[test]
fn fixture_layout_gives_the_dsdt_address() {
    // The DSDT address of this test is not invented: in the blob of S4-31d the payload starts at
    // `load_base` (0xE_0000) and the DSDT is the first table after the 0x40-byte RSDP slot.
    // S4-31g may change the layout; then this test names what moved.
    let mut blob = vec![0u8; STAGE4_BLOB_CAPACITY];
    build_blob(&STAGE4_DEVICES, &mut blob).unwrap();
    assert_eq!(le64(&blob, 0x30), ACPI_REGION_BASE);
    assert_eq!(&blob[0x60..0x64], b"DSDT");
    let dsdt_offset = le32(&blob, 0x60 + 4) as u64;
    assert_eq!(ACPI_REGION_BASE + dsdt_offset, ADDRESSES.dsdt);
    assert_eq!(ADDRESSES.dsdt, 0xE_0040);
}

#[test]
fn fadt_validation_accepts_the_result_and_reports_each_defect() {
    let good = board_fadt();
    validate_fadt(&good).unwrap();

    // Signature, length and checksum come from the shared table check.
    let mut t = good.clone();
    t[0] = b'X';
    fix_checksum(&mut t);
    assert_eq!(
        validate_fadt(&t),
        Err(FadtError::Table(TableError::SignatureMismatch))
    );
    assert_eq!(
        validate_fadt(&good[..275]),
        Err(FadtError::Table(TableError::LengthMismatch {
            declared: 276,
            actual: 275
        }))
    );
    let mut t = good.clone();
    t[200] ^= 0x01;
    assert_eq!(
        validate_fadt(&t),
        Err(FadtError::Table(TableError::BadChecksum { sum: 0x01 }))
    );
    // A table with the right header length field but an older/other length is rejected:
    // 244 bytes is the revision-5 FADT, a Length of 244 passes the shared check but not ours.
    let mut short = good[..244].to_vec();
    short[4..8].copy_from_slice(&244u32.to_le_bytes());
    fix_checksum(&mut short);
    assert!(matches!(
        validate_fadt(&short),
        Err(FadtError::Table(TableError::LengthMismatch { .. }))
    ));

    // Rules of this board, each with a repaired checksum so only the rule fails.
    let mutate = |at: usize, value: u8| {
        let mut t = good.clone();
        t[at] = value;
        fix_checksum(&mut t);
        validate_fadt(&t)
    };
    assert_eq!(mutate(8, 5), Err(FadtError::BadField("revision")));
    // HW_REDUCED_ACPI = bit 20 = bit 4 of byte 114.
    assert_eq!(
        mutate(114, 0x10),
        Err(FadtError::BadField("flags: HW_REDUCED_ACPI is set"))
    );
    // RESET_REG_SUP = bit 10 = bit 2 of byte 113.
    assert_eq!(
        mutate(113, 0),
        Err(FadtError::BadField("flags: RESET_REG_SUP is clear"))
    );
    assert_eq!(mutate(40, 0x41), Err(FadtError::BadField("DSDT / X_DSDT")));
    assert_eq!(
        mutate(133, 0),
        Err(FadtError::BadField("FIRMWARE_CTRL / X_FIRMWARE_CTRL"))
    );
    assert_eq!(
        mutate(57, 0x07),
        Err(FadtError::BadField("PM1a_EVT_BLK / X_PM1a_EVT_BLK"))
    );
    assert_eq!(
        mutate(89, 4),
        Err(FadtError::BadField("PM1a_CNT_BLK / X_PM1a_CNT_BLK"))
    );
    assert_eq!(
        mutate(209, 24),
        Err(FadtError::BadField("PM_TMR_BLK / X_PM_TMR_BLK"))
    );
    assert_eq!(mutate(116, 0), Err(FadtError::BadField("RESET_REG")));
}

#[test]
fn fadt_build_rejects_bad_inputs_and_leaves_the_buffer_alone() {
    let mut entry = STAGE4_DEVICES[7];
    entry.gsi = 16;
    let pm = AcpiPmEntry::from_entry(&entry).unwrap();
    let mut out = [0xAAu8; 300];
    assert_eq!(
        build_fadt(&pm, &ADDRESSES, &mut out),
        Err(FadtError::SciOutOfRange(16))
    );
    assert!(out.iter().all(|&b| b == 0xAA));

    let pm = board_pm();
    let bad = |facs: u64, dsdt: u64| {
        let mut out = [0xAAu8; 300];
        let result = build_fadt(&pm, &FadtAddresses { facs, dsdt }, &mut out);
        assert!(
            out.iter().all(|&b| b == 0xAA),
            "no byte written on {result:?}"
        );
        result
    };
    // DSDT outside the table part: below the region, at the NVS part, above the region.
    assert_eq!(
        bad(0xE_F000, 0xD_FFC0),
        Err(FadtError::DsdtOutsideRegion(0xD_FFC0))
    );
    assert_eq!(
        bad(0xE_F000, 0xE_F000),
        Err(FadtError::DsdtOutsideRegion(0xE_F000))
    );
    assert_eq!(bad(0xE_F000, 0), Err(FadtError::DsdtOutsideRegion(0)));
    // FACS: in the table part, unaligned, past the end (the 64 bytes must fit), above 4 GiB.
    assert_eq!(
        bad(0xE_EFC0, 0xE_0040),
        Err(FadtError::FacsOutsideRegion(0xE_EFC0))
    );
    assert_eq!(
        bad(0xE_F010, 0xE_0040),
        Err(FadtError::FacsOutsideRegion(0xE_F010))
    );
    assert_eq!(
        bad(0xF_0000, 0xE_0040),
        Err(FadtError::FacsOutsideRegion(0xF_0000))
    );
    assert_eq!(
        bad(0xE_F000 + (1 << 32), 0xE_0040),
        Err(FadtError::FacsOutsideRegion(0xE_F000 + (1 << 32)))
    );
    // The last 64-byte slot of the region is valid.
    let mut out = [0u8; 276];
    assert_eq!(
        build_fadt(
            &pm,
            &FadtAddresses {
                facs: 0xE_FFC0,
                dsdt: 0xE_0040
            },
            &mut out
        ),
        Ok(276)
    );
    // Short buffer.
    let mut small = [0u8; 275];
    assert_eq!(
        build_fadt(&pm, &ADDRESSES, &mut small),
        Err(FadtError::Table(TableError::TooShort))
    );
}

#[test]
fn facs_matches_the_hand_assembled_reference() {
    let mut out = [0xFFu8; 100];
    let len = build_facs(&mut out).unwrap();
    assert_eq!(len, 64);
    assert_eq!(&out[..64], reference_facs().as_slice());
    assert_eq!(out[64], 0xFF, "nothing past 64 bytes is touched");
    // Signature, Length, version 2, every other field zero (no checksum in the FACS).
    assert_eq!(&out[0..4], b"FACS");
    assert_eq!(le32(&out, 4), 64);
    assert_eq!(out[32], 2);
    for (name, at, len) in [
        ("HARDWARE_SIGNATURE", 8, 4),
        ("FIRMWARE_WAKING_VECTOR", 12, 4),
        ("GLOBAL_LOCK", 16, 4),
        ("FLAGS", 20, 4),
        ("X_FIRMWARE_WAKING_VECTOR", 24, 8),
        ("reserved 33..36", 33, 3),
        ("OSPM_FLAGS", 36, 4),
        ("reserved 40..64", 40, 24),
    ] {
        assert!(out[at..at + len].iter().all(|&b| b == 0), "{name}");
    }
    assert_ne!(
        byte_sum(&out[..64]),
        0,
        "the FACS is not checksummed: its sum is not 0"
    );
    validate_facs(&out[..64]).unwrap();
}

#[test]
fn facs_validation_reports_each_defect() {
    let good = reference_facs();
    assert_eq!(validate_facs(&good[..63]), Err(FacsError::TooShort));
    let mut t = good.clone();
    t[3] = b'X';
    assert_eq!(validate_facs(&t), Err(FacsError::SignatureMismatch));
    let mut t = good.clone();
    t[4] = 0x80;
    assert_eq!(
        validate_facs(&t),
        Err(FacsError::LengthMismatch {
            declared: 128,
            actual: 64
        })
    );
    let mut long = good.clone();
    long.push(0);
    assert_eq!(
        validate_facs(&long),
        Err(FacsError::LengthMismatch {
            declared: 64,
            actual: 65
        })
    );
    let mut t = good.clone();
    t[32] = 1;
    assert_eq!(validate_facs(&t), Err(FacsError::BadVersion(1)));
    let mut t = good.clone();
    t[34] = 1;
    assert_eq!(validate_facs(&t), Err(FacsError::ReservedNotZero(34)));
    let mut t = good.clone();
    t[63] = 1;
    assert_eq!(validate_facs(&t), Err(FacsError::ReservedNotZero(63)));
    // FLAGS bits 0 and 1 are defined, the rest is reserved.
    let mut t = good.clone();
    t[20] = 0x04;
    assert_eq!(validate_facs(&t), Err(FacsError::ReservedFlags(4)));
    let mut t = good.clone();
    t[20] = 0x03;
    assert_eq!(validate_facs(&t), Ok(()));
    let mut small = [0u8; 63];
    assert_eq!(build_facs(&mut small), Err(FacsError::TooShort));
}
