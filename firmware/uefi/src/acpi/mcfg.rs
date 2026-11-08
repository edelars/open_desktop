//! MCFG (PCI Express memory-mapped configuration space base address description table),
//! built from the PCIe ECAM entry of the board manifest (S4-31a).
//!
//! Layout (ACPI table header, Table 5.4, plus the PCI Firmware Specification MCFG layout):
//! 36-byte header, 8 reserved bytes, then one 16-byte allocation structure per ECAM window:
//! `base u64`, `segment u16`, `start_bus u8`, `end_bus u8`, 4 reserved bytes. MCFG revision 1.
//! One window gives a table of 60 bytes. The PCI Firmware Specification text is NOT verified
//! here (no copy was available); the layout follows the long-standing format that Linux's
//! `pci_parse_mcfg` reads (`header->length - sizeof(struct acpi_table_mcfg)` divided by the
//! allocation size, entries at `&mcfg[1]`; `arch/x86/pci/mmconfig-shared.c`), and the
//! 44-byte/16-byte sizes are confirmed only by the reference bytes written by hand in the
//! tests, not by `include/acpi/actbl1.h` (not extracted in the source tree used for S4-31a).
//!
//! The serializer accepts exactly the stage-4 board contract (MACHINE_CONTRACT.md, "ECAM and
//! conf1 on the board (S4-30c)"): segment 0, buses 0..=0, a 1 MiB window below 4 GiB. Anything
//! else is an error, nothing is rounded or defaulted.

use super::bytes::{copy_at, fill_range, head_mut, tri};
use super::manifest::{BoardManifest, DeviceEntry, device_type};
use super::table::{SDT_HEADER_LEN, TableError, finish_checksum, validate_table, write_header};

/// MCFG signature.
pub const MCFG_SIGNATURE: [u8; 4] = *b"MCFG";
/// MCFG revision: 1 (Linux `acpi_mcfg_valid_entry` reads `revision >= 1` for windows above
/// 4 GiB; the window of this board is below 4 GiB, so the value is informational).
pub const MCFG_REVISION: u8 = 1;
/// OEM Table ID of the MCFG: an own string, 8 bytes.
pub const MCFG_OEM_TABLE_ID: [u8; 8] = *b"ODFWMCFG";
/// Header (36) plus the 8 reserved bytes.
pub const MCFG_FIXED_LEN: usize = SDT_HEADER_LEN + 8;
/// One allocation structure.
pub const MCFG_ENTRY_LEN: usize = 16;
/// Length of a table with one allocation structure.
pub const MCFG_LEN_ONE_ENTRY: usize = MCFG_FIXED_LEN + MCFG_ENTRY_LEN;

/// Bytes of ECAM per bus (256 devices x 8 functions x 4 KiB).
pub const ECAM_BYTES_PER_BUS: u64 = 1 << 20;
/// Stage-4 board contract: the only supported segment and bus range (MACHINE_CONTRACT.md).
pub const STAGE4_SEGMENT: u16 = 0;
pub const STAGE4_START_BUS: u8 = 0;
pub const STAGE4_END_BUS: u8 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McfgError {
    Table(TableError),
    /// The entry is not a PCIe ECAM entry (`device_type != 7`).
    NotEcamEntry(u32),
    /// The ECAM entry has `PRESENT = 0`: there is no window to describe.
    MissingEcam,
    /// The ECAM entry is a PIO range; ECAM is memory-mapped.
    PioWindow,
    /// `param1` has bits above bit 15 (only `start_bus | end_bus << 8` is defined).
    ReservedParamBits(u64),
    /// `param0` (segment) does not fit 16 bits.
    SegmentOutOfRange(u64),
    /// `start_bus > end_bus`.
    BusRangeInverted {
        start: u8,
        end: u8,
    },
    /// `length != (end_bus - start_bus + 1) << 20` (also covers a zero length).
    LengthMismatch {
        expected: u64,
        actual: u64,
    },
    ZeroBase,
    /// `base` is not a multiple of 1 MiB (project rule; one bus is 1 MiB).
    BaseUnaligned(u64),
    /// The window ends above 4 GiB. Linux (`acpi_mcfg_valid_entry`) ignores a window with
    /// `address >= 0xFFFF_FFFF` unless the OEM ID is "SGI" or SMBIOS says the BIOS year is
    /// 2010 or later; this board has no SMBIOS (S4-31a), so such a window would be dropped.
    WindowAbove4Gib {
        end: u64,
    },
    /// Valid ECAM, but not the stage-4 contract (segment 0, buses 0..=0).
    OutsideStage4Contract {
        segment: u16,
        start: u8,
        end: u8,
    },
    /// The entry area of a finished table is empty or not a whole number of 16-byte
    /// allocation structures. Signature, length and checksum defects are `Table(..)`.
    BadEntryArea,
}

impl From<TableError> for McfgError {
    fn from(error: TableError) -> Self {
        McfgError::Table(error)
    }
}

/// Builds the MCFG from the PCIe ECAM entry of the manifest.
///
/// A `const fn` (S4-31b0): evaluated at compile time it gives the table as static bytes, called
/// at run time it gives the same bytes.
pub const fn build_mcfg(manifest: &BoardManifest, out: &mut [u8]) -> Result<usize, McfgError> {
    match manifest.device(device_type::PCIE_ECAM) {
        Some(entry) => build_mcfg_from_entry(entry, out),
        None => Err(McfgError::MissingEcam),
    }
}

/// Builds the MCFG from one device entry, which must be the PCIe ECAM entry (type 7). The
/// entry is checked first, the table is written into `out` (all 60 bytes, reserved ones as
/// zero) and checked by [`validate_mcfg`] before the length is returned.
pub const fn build_mcfg_from_entry(
    entry: &DeviceEntry,
    out: &mut [u8],
) -> Result<usize, McfgError> {
    let window = tri!(EcamWindow::from_entry(entry));
    if out.len() < MCFG_LEN_ONE_ENTRY {
        return Err(McfgError::Table(TableError::TooShort));
    }
    let table = head_mut(out, MCFG_LEN_ONE_ENTRY);
    tri!(
        write_header(
            table,
            MCFG_SIGNATURE,
            MCFG_REVISION,
            MCFG_OEM_TABLE_ID,
            MCFG_LEN_ONE_ENTRY as u32,
        ),
        McfgError::Table
    );
    fill_range(table, SDT_HEADER_LEN, MCFG_FIXED_LEN, 0);
    let at = MCFG_FIXED_LEN;
    // Future note: in MCFG the base field is the address of bus 0 of the segment, not the
    // start of the window; Linux adds PCI_MMCFG_BUS_OFFSET(bus) to cfg->address
    // (arch/x86/pci/mmconfig_64.c:105, `start = cfg->address + BUS_OFFSET(start_bus)`). If start_bus != 0 is ever allowed, write
    // `base - (start_bus << 20)` here. Today the range is 0..=0 (OutsideStage4Contract), so
    // the two are equal.
    copy_at(table, at, &window.base.to_le_bytes());
    copy_at(table, at + 8, &window.segment.to_le_bytes());
    table[at + 10] = window.start_bus;
    table[at + 11] = window.end_bus;
    fill_range(table, at + 12, at + 16, 0);
    tri!(finish_checksum(table), McfgError::Table);
    tri!(validate_mcfg(table));
    Ok(MCFG_LEN_ONE_ENTRY)
}

/// Checks signature, `Length`, checksum and that the entry area is a non-empty whole number
/// of 16-byte allocation structures.
pub const fn validate_mcfg(table: &[u8]) -> Result<(), McfgError> {
    tri!(validate_table(table, &MCFG_SIGNATURE), McfgError::Table);
    let entries = match table.len().checked_sub(MCFG_FIXED_LEN) {
        Some(entries) => entries,
        None => return Err(McfgError::BadEntryArea),
    };
    if entries == 0 || !entries.is_multiple_of(MCFG_ENTRY_LEN) {
        return Err(McfgError::BadEntryArea);
    }
    Ok(())
}

/// The checked window: the only values that reach the table.
struct EcamWindow {
    base: u64,
    segment: u16,
    start_bus: u8,
    end_bus: u8,
}

impl EcamWindow {
    const fn from_entry(entry: &DeviceEntry) -> Result<Self, McfgError> {
        if entry.device_type != device_type::PCIE_ECAM {
            return Err(McfgError::NotEcamEntry(entry.device_type));
        }
        if !entry.is_present() {
            return Err(McfgError::MissingEcam);
        }
        if entry.is_pio() {
            return Err(McfgError::PioWindow);
        }
        if entry.param1 >> 16 != 0 {
            return Err(McfgError::ReservedParamBits(entry.param1));
        }
        if entry.param0 > u16::MAX as u64 {
            return Err(McfgError::SegmentOutOfRange(entry.param0));
        }
        let segment = entry.param0 as u16;
        let start_bus = (entry.param1 & 0xFF) as u8;
        let end_bus = ((entry.param1 >> 8) & 0xFF) as u8;
        if start_bus > end_bus {
            return Err(McfgError::BusRangeInverted {
                start: start_bus,
                end: end_bus,
            });
        }
        let expected = (end_bus as u64 - start_bus as u64 + 1) * ECAM_BYTES_PER_BUS;
        if entry.length != expected {
            return Err(McfgError::LengthMismatch {
                expected,
                actual: entry.length,
            });
        }
        if entry.base == 0 {
            return Err(McfgError::ZeroBase);
        }
        if !entry.base.is_multiple_of(ECAM_BYTES_PER_BUS) {
            return Err(McfgError::BaseUnaligned(entry.base));
        }
        // An overflowing sum is reported as the largest end.
        let end = match entry.base.checked_add(entry.length) {
            Some(end) => end,
            None => return Err(McfgError::WindowAbove4Gib { end: u64::MAX }),
        };
        if end > 1 << 32 {
            return Err(McfgError::WindowAbove4Gib { end });
        }
        if segment != STAGE4_SEGMENT || start_bus != STAGE4_START_BUS || end_bus != STAGE4_END_BUS {
            return Err(McfgError::OutsideStage4Contract {
                segment,
                start: start_bus,
                end: end_bus,
            });
        }
        Ok(EcamWindow {
            base: entry.base,
            segment,
            start_bus,
            end_bus,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acpi::manifest::{DEVICE_COUNT, FLAG_PIO, FLAG_PRESENT, GSI_NONE};
    use crate::acpi::table::byte_sum;

    const ECAM_BASE: u64 = 0xFC10_0000;

    /// The entry the machine emits for the board with `pci_ecam` (MACHINE_CONTRACT.md S4-30c).
    fn board_entry() -> DeviceEntry {
        DeviceEntry {
            device_type: 7,
            flags: FLAG_PRESENT,
            base: ECAM_BASE,
            length: 0x10_0000,
            gsi: GSI_NONE,
            irq_flags: 0,
            param0: 0,
            param1: 0,
        }
    }

    /// Reference table written out by hand from the layout, field by field. The checksum
    /// byte 0x4C was computed independently: the other 59 bytes add to 0x7B4, i.e. 0xB4 mod
    /// 256, and 0x100 - 0xB4 = 0x4C. The code under test is not used to produce any byte.
    const REFERENCE: [u8; 60] = [
        // Signature "MCFG"
        0x4D, 0x43, 0x46, 0x47, //
        // Length 60
        0x3C, 0x00, 0x00, 0x00, //
        // Revision 1, checksum
        0x01, 0x4C, //
        // OEM ID "OPNDSK"
        0x4F, 0x50, 0x4E, 0x44, 0x53, 0x4B, //
        // OEM Table ID "ODFWMCFG"
        0x4F, 0x44, 0x46, 0x57, 0x4D, 0x43, 0x46, 0x47, //
        // OEM Revision 1
        0x01, 0x00, 0x00, 0x00, //
        // Creator ID "ODFW"
        0x4F, 0x44, 0x46, 0x57, //
        // Creator Revision 1
        0x01, 0x00, 0x00, 0x00, //
        // Reserved (8 bytes)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        // Base 0x0000_0000_FC10_0000
        0x00, 0x00, 0x10, 0xFC, 0x00, 0x00, 0x00, 0x00, //
        // Segment 0, start bus 0, end bus 0
        0x00, 0x00, 0x00, 0x00, //
        // Reserved
        0x00, 0x00, 0x00, 0x00,
    ];

    fn build(entry: &DeviceEntry) -> Result<[u8; 60], McfgError> {
        let mut out = [0xAAu8; 64]; // dirty buffer larger than the table
        let n = build_mcfg_from_entry(entry, &mut out)?;
        assert_eq!(n, 60);
        assert_eq!(&out[60..], &[0xAA; 4], "bytes past the table are untouched");
        let mut table = [0u8; 60];
        table.copy_from_slice(&out[..60]);
        Ok(table)
    }

    fn manifest_with(ecam: DeviceEntry) -> BoardManifest {
        let entries: [DeviceEntry; DEVICE_COUNT] = core::array::from_fn(|i| {
            let t = i as u32 + 1;
            if t == 7 {
                ecam
            } else {
                DeviceEntry {
                    device_type: t,
                    gsi: GSI_NONE,
                    ..DeviceEntry::default()
                }
            }
        });
        BoardManifest::from_entries(entries).unwrap()
    }

    #[test]
    fn board_ecam_entry_gives_the_reference_bytes() {
        let table = build(&board_entry()).unwrap();
        assert_eq!(table, REFERENCE);
    }

    #[test]
    fn fields_sit_at_the_documented_offsets_and_sum_to_zero() {
        let t = build(&board_entry()).unwrap();
        assert_eq!(t.len(), 60);
        assert_eq!(&t[0..4], b"MCFG");
        assert_eq!(&t[4..8], &60u32.to_le_bytes());
        assert_eq!(t[8], 1, "revision");
        assert_eq!(&t[36..44], &[0u8; 8], "reserved");
        assert_eq!(&t[44..52], &0xFC10_0000u64.to_le_bytes(), "base");
        assert_eq!(&t[52..54], &0u16.to_le_bytes(), "segment");
        assert_eq!((t[54], t[55]), (0, 0), "start/end bus");
        assert_eq!(&t[56..60], &[0u8; 4], "entry reserved");
        // Sum of all bytes including the checksum byte is 0 mod 256, summed by a plain loop
        // here and not through the crate helper.
        let mut sum = 0u32;
        for b in t {
            sum += u32::from(b);
        }
        assert_eq!(sum % 256, 0);
        assert_eq!(byte_sum(&t), 0);
    }

    #[test]
    fn manifest_wrapper_gives_the_same_bytes() {
        let m = manifest_with(board_entry());
        let mut out = [0u8; 60];
        assert_eq!(build_mcfg(&m, &mut out), Ok(60));
        assert_eq!(out, REFERENCE);
    }

    #[test]
    fn two_calls_give_identical_bytes() {
        let a = build(&board_entry()).unwrap();
        let b = build(&board_entry()).unwrap();
        assert_eq!(a, b);
        // A different buffer history does not matter either.
        let mut clean = [0u8; 60];
        build_mcfg_from_entry(&board_entry(), &mut clean).unwrap();
        assert_eq!(clean, a);
    }

    #[test]
    fn a_different_base_changes_only_base_and_checksum() {
        let mut e = board_entry();
        e.base = 0xE000_0000;
        let t = build(&e).unwrap();
        let mut expected = REFERENCE;
        expected[44..52].copy_from_slice(&0xE000_0000u64.to_le_bytes());
        // Base byte 0x10 + 0xFC (sum 0x10C) is replaced by 0xE0; the checksum moves by the
        // difference: old sum of the table was 0 mod 256.
        let old = 0x10u32 + 0xFC;
        let new = 0xE0u32;
        let checksum = (0x4Cu32 + old - new) % 256;
        expected[9] = checksum as u8;
        assert_eq!(t, expected);
    }

    #[test]
    fn absent_ecam_is_rejected() {
        let mut absent = board_entry();
        absent.flags = 0;
        absent.base = 0;
        absent.length = 0;
        assert_eq!(build(&absent), Err(McfgError::MissingEcam));
        // The default board manifest has code 7 PRESENT = 0.
        let m = manifest_with(absent);
        assert_eq!(build_mcfg(&m, &mut [0u8; 60]), Err(McfgError::MissingEcam));
    }

    #[test]
    fn inverted_bus_range_is_rejected() {
        let mut e = board_entry();
        e.param1 = 1; // start 1, end 0
        assert_eq!(
            build(&e),
            Err(McfgError::BusRangeInverted { start: 1, end: 0 })
        );
        e.param1 = 0xFF | (0x10 << 8);
        assert_eq!(
            build(&e),
            Err(McfgError::BusRangeInverted {
                start: 0xFF,
                end: 0x10
            })
        );
    }

    #[test]
    fn entries_that_are_not_a_memory_ecam_window_are_rejected() {
        // Another device type.
        let mut com = board_entry();
        com.device_type = 1;
        assert_eq!(build(&com), Err(McfgError::NotEcamEntry(1)));
        // A PIO base.
        let mut pio = board_entry();
        pio.flags = FLAG_PRESENT | FLAG_PIO;
        assert_eq!(build(&pio), Err(McfgError::PioWindow));
        // Zero length.
        let mut zero = board_entry();
        zero.length = 0;
        assert_eq!(
            build(&zero),
            Err(McfgError::LengthMismatch {
                expected: 0x10_0000,
                actual: 0
            })
        );
        // Not 1 MiB on buses 0..=0.
        for bad in [0xF_FFFFu64, 0x10_0001, 0x20_0000, 0x1000_0000] {
            let mut e = board_entry();
            e.length = bad;
            assert_eq!(
                build(&e),
                Err(McfgError::LengthMismatch {
                    expected: 0x10_0000,
                    actual: bad
                })
            );
        }
        // Reserved bits of param1 and a segment above 16 bits.
        let mut reserved = board_entry();
        reserved.param1 = 1 << 16;
        assert_eq!(build(&reserved), Err(McfgError::ReservedParamBits(1 << 16)));
        let mut seg = board_entry();
        seg.param0 = 0x1_0000;
        assert_eq!(build(&seg), Err(McfgError::SegmentOutOfRange(0x1_0000)));
    }

    #[test]
    fn base_must_be_nonzero_aligned_and_below_4_gib() {
        let mut e = board_entry();
        e.base = 0;
        assert_eq!(build(&e), Err(McfgError::ZeroBase));
        e.base = ECAM_BASE + 0x1000;
        assert_eq!(build(&e), Err(McfgError::BaseUnaligned(ECAM_BASE + 0x1000)));
        // Window ending exactly at 4 GiB is allowed (Linux checks the start address); one MiB
        // later is not.
        e.base = 0xFFF0_0000;
        assert!(build(&e).is_ok());
        e.base = 0x1_0000_0000;
        assert_eq!(
            build(&e),
            Err(McfgError::WindowAbove4Gib { end: 0x1_0010_0000 })
        );
        e.base = 0xFFFF_FFFF_FFF0_0000;
        assert_eq!(build(&e), Err(McfgError::WindowAbove4Gib { end: u64::MAX }));
    }

    #[test]
    fn valid_ecam_outside_the_stage4_contract_is_rejected() {
        // Segment 1.
        let mut seg = board_entry();
        seg.param0 = 1;
        assert_eq!(
            build(&seg),
            Err(McfgError::OutsideStage4Contract {
                segment: 1,
                start: 0,
                end: 0
            })
        );
        // Buses 0..=1 with a consistent 2 MiB window.
        let mut buses = board_entry();
        buses.param1 = 1 << 8;
        buses.length = 0x20_0000;
        assert_eq!(
            build(&buses),
            Err(McfgError::OutsideStage4Contract {
                segment: 0,
                start: 0,
                end: 1
            })
        );
    }

    #[test]
    fn short_output_buffer_is_rejected_without_writing() {
        let mut out = [0xAAu8; 59];
        assert_eq!(
            build_mcfg_from_entry(&board_entry(), &mut out),
            Err(McfgError::Table(TableError::TooShort))
        );
        assert_eq!(out, [0xAA; 59]);
    }

    #[test]
    fn validator_reports_each_defect() {
        let mut t = REFERENCE;
        assert_eq!(validate_mcfg(&t), Ok(()));
        assert_eq!(
            validate_mcfg(&t[..59]),
            Err(McfgError::Table(TableError::LengthMismatch {
                declared: 60,
                actual: 59
            }))
        );
        t[45] ^= 1;
        assert_eq!(
            validate_mcfg(&t),
            Err(McfgError::Table(TableError::BadChecksum { sum: 1 }))
        );
        // A table without any allocation structure (44 bytes), and one with a partial one.
        for len in [44usize, 52] {
            let mut short = [0u8; 60];
            short[..len].copy_from_slice(&REFERENCE[..len]);
            short[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            finish_checksum(&mut short[..len]).unwrap();
            assert_eq!(
                validate_mcfg(&short[..len]),
                Err(McfgError::BadEntryArea),
                "len {len}"
            );
        }
        let mut other = REFERENCE;
        other[0] = b'X';
        assert_eq!(
            validate_mcfg(&other),
            Err(McfgError::Table(TableError::SignatureMismatch))
        );
    }

    // S4-31b0: the same serializer evaluated at compile time. The manifest and both tables are
    // built in const items; the hand-written REFERENCE above is the golden for them.
    const BOARD_ENTRY: DeviceEntry = DeviceEntry {
        device_type: 7,
        flags: FLAG_PRESENT,
        base: ECAM_BASE,
        length: 0x10_0000,
        gsi: GSI_NONE,
        irq_flags: 0,
        param0: 0,
        param1: 0,
    };

    const MCFG_FROM_ENTRY: [u8; 60] = {
        let mut out = [0u8; 60];
        match build_mcfg_from_entry(&BOARD_ENTRY, &mut out) {
            Ok(60) => {}
            _ => panic!("build_mcfg_from_entry failed in a const context"),
        }
        out
    };

    const BOARD_MANIFEST: BoardManifest = {
        let mut entries = [BOARD_ENTRY; DEVICE_COUNT];
        let mut i = 0;
        while i < DEVICE_COUNT {
            if i != 6 {
                entries[i] = DeviceEntry {
                    device_type: i as u32 + 1,
                    flags: 0,
                    base: 0,
                    length: 0,
                    gsi: GSI_NONE,
                    irq_flags: 0,
                    param0: 0,
                    param1: 0,
                };
            }
            i += 1;
        }
        match BoardManifest::from_entries(entries) {
            Ok(manifest) => manifest,
            Err(_) => panic!("from_entries failed in a const context"),
        }
    };

    /// Static bytes of the stage-4 table: a `static` filled by the const `build_mcfg`.
    static MCFG_STATIC: [u8; 60] = {
        let mut out = [0u8; 60];
        match build_mcfg(&BOARD_MANIFEST, &mut out) {
            Ok(60) => {}
            _ => panic!("build_mcfg failed in a const context"),
        }
        out
    };

    #[test]
    fn const_evaluated_tables_equal_the_hand_written_reference() {
        assert_eq!(MCFG_FROM_ENTRY, REFERENCE);
        assert_eq!(MCFG_STATIC, REFERENCE);
    }

    #[test]
    fn const_and_runtime_calls_agree() {
        let runtime = build(&board_entry()).unwrap();
        assert_eq!(MCFG_STATIC, runtime);
        // The const validator accepts the const table and still rejects a damaged one.
        const _: () = assert!(validate_mcfg(&MCFG_FROM_ENTRY).is_ok());
        let mut damaged = MCFG_STATIC;
        damaged[50] ^= 0x80;
        assert!(matches!(
            validate_mcfg(&damaged),
            Err(McfgError::Table(TableError::BadChecksum { .. }))
        ));
    }
}
