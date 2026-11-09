//! S4-31f: MADT. The reference table was assembled field by field from the MADT layout (offsets
//! in the comments of `reference_madt`) and written out by a separate Python script
//! (`python3 -I`) that shares no code with the serializer; the checksum 0x4D was also summed
//! by hand (sum of all other bytes = 2739 = 0xAB3 -> 0xB3, and 0x100 - 0xB3 = 0x4D). Offsets are used
//! as literals here, not through the constants of the serializer.

use super::*;
use crate::acpi::blob::STAGE4_DEVICES;
use crate::acpi::manifest::{FLAG_PIO, GSI_NONE};
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

fn entries() -> [DeviceEntry; 7] {
    STAGE4_DEVICES[..7].try_into().unwrap()
}

fn board_manifest() -> BoardManifest {
    BoardManifest::from_entries(entries()).unwrap()
}

/// The PM entry of the stage-4 board (SCI GSI 9, level, active low).
fn board_pm() -> AcpiPmEntry {
    AcpiPmEntry::from_entry(&STAGE4_DEVICES[7]).unwrap()
}

fn build(manifest: &BoardManifest, pm: &AcpiPmEntry, fill: u8) -> Vec<u8> {
    let mut out = [fill; 100];
    let len = build_madt(manifest, pm, &mut out).unwrap();
    assert_eq!(len, 84);
    out[..len].to_vec()
}

fn board_madt() -> Vec<u8> {
    build(&board_manifest(), &board_pm(), 0)
}

/// Recomputes the checksum after a test changed a byte.
fn fix_checksum(table: &mut [u8]) {
    table[9] = 0;
    table[9] = byte_sum(table).wrapping_neg();
}

/// Sets `Length` to the vector length and re-seals the checksum.
fn resize(table: &mut Vec<u8>) {
    let len = table.len() as u32;
    table[4..8].copy_from_slice(&len.to_le_bytes());
    fix_checksum(table);
}

/// The board table with `extra` appended after the last structure.
fn appended(extra: &[u8]) -> Vec<u8> {
    let mut t = board_madt();
    t.extend_from_slice(extra);
    resize(&mut t);
    t
}

/// Patches a byte and re-seals the table, so that only the structural defect is reported.
fn patched(at: usize, value: u8) -> Vec<u8> {
    let mut t = board_madt();
    t[at] = value;
    fix_checksum(&mut t);
    t
}

/// MADT revision 5 for the board, 84 bytes. Offsets: header 0..36 (signature `APIC`, Length
/// 0x54, Revision 5, checksum 0x4D, OEM ID `OPNDSK`, OEM table ID `ODFWAPIC`, OEM rev 1, creator
/// `ODFW`, creator rev 1); 36 Local APIC address 0xFEE00000; 40 Flags 1 (PCAT_COMPAT); 44
/// Processor Local APIC (type 0, length 8, UID 0, APIC ID 0, flags 1 = Enabled); 52 I/O APIC
/// (type 1, length 12, ID 1, reserved 0, address 0xFEC00000, GSI base 0); 64 Interrupt Source
/// Override (type 2, length 10, bus 0, source 0, GSI 2, flags 0x0000 = conforms to ISA: edge,
/// active high); 74 Interrupt Source Override (type 2, length 10, bus 0, source 9, GSI 9, flags
/// 0x000F = trigger level 0b11 << 2 | polarity active low 0b11).
fn reference_madt() -> Vec<u8> {
    hex("41 50 49 43 54 00 00 00 05 4D 4F 50 4E 44 53 4B \
         4F 44 46 57 41 50 49 43 01 00 00 00 4F 44 46 57 \
         01 00 00 00 00 00 E0 FE 01 00 00 00 00 08 00 00 \
         01 00 00 00 01 0C 01 00 00 00 C0 FE 00 00 00 00 \
         02 0A 00 00 02 00 00 00 00 00 02 0A 00 09 09 00 \
         00 00 0F 00")
}

#[test]
fn madt_matches_the_hand_assembled_reference() {
    let table = board_madt();
    assert_eq!(table, reference_madt());
    assert_eq!(table.len(), 84);
    assert_eq!(&table[4..8], &84u32.to_le_bytes());
    assert_eq!(table[9], 0x4D);
    assert_eq!(table.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
    assert_eq!(table.len(), MADT_LEN);
    validate_madt(&table).unwrap();
}

#[test]
fn madt_fields_equal_the_board_decisions() {
    let t = board_madt();
    assert_eq!(&t[0..4], b"APIC");
    assert_eq!(t[8], 5, "ACPI 6.4 Table 5.19 revision");
    assert_eq!(&t[10..16], b"OPNDSK");
    assert_eq!(&t[16..24], b"ODFWAPIC");
    // Local APIC address of S4-25a, PCAT_COMPAT = 1 and no other flag.
    assert_eq!(le32(&t, 36), 0xFEE0_0000);
    assert_eq!(le32(&t, 40), 1);
    // One processor: type 0, length 8, UID 0, APIC ID 0, Enabled.
    assert_eq!(&t[44..48], &[0, 8, 0, 0][..]);
    assert_eq!(le32(&t, 48), 1);
    // I/O APIC: address of S4-26a, GSI base 0 (FIRMWARE_BOOT.md 6.3).
    assert_eq!(&t[52..56], &[1, 12, 1, 0][..]);
    assert_eq!(le32(&t, 56), 0xFEC0_0000);
    assert_eq!(le32(&t, 60), 0);
    // IRQ 0 -> GSI 2, SCI IRQ 9 -> GSI 9.
    assert_eq!(&t[64..68], &[2, 10, 0, 0][..]);
    assert_eq!(le32(&t, 68), 2);
    assert_eq!(le16(&t, 72), 0x0000);
    assert_eq!(&t[74..78], &[2, 10, 0, 9][..]);
    assert_eq!(le32(&t, 78), 9);
    assert_eq!(le16(&t, 82), 0x000F);
    // The SCI of the FADT (S4-31e) is the GSI of the override.
    assert_eq!(board_pm().sci_gsi, 9);
}

#[test]
fn only_the_four_wanted_structures_exist() {
    // No NMI source (types 3, 4), no x2APIC (9), no address override (5): the structure list is
    // exactly Local APIC, I/O APIC, two overrides.
    let t = board_madt();
    let mut at = 44;
    let mut kinds = Vec::new();
    while at < t.len() {
        kinds.push(t[at]);
        at += t[at + 1] as usize;
    }
    assert_eq!(at, t.len());
    assert_eq!(kinds, [0, 1, 2, 2]);
}

#[test]
fn inti_flags_encoding_follows_table_5_26() {
    // ACPI 6.4 Table 5.26 (= MPS 1.4 Table 4-10): polarity bits 0..=1 (00 bus default, 01 high,
    // 11 low), trigger bits 2..=3 (00 bus default, 01 edge, 11 level).
    let level = 0b11u16 << 2;
    let active_low = 0b11u16;
    assert_eq!(level | active_low, 0x000F);
    assert_eq!(SCI_OVERRIDE_FLAGS_LEVEL_LOW, 0x000F);
    // 0x000D, the value named in the task text, decodes as level / ACTIVE HIGH.
    assert_eq!(0x000Du16 & 0b11, 0b01, "polarity of 0x000D is active high");
    assert_eq!(0x000Du16 >> 2 & 0b11, 0b11, "trigger of 0x000D is level");
    // The timer override uses "conforms" for both fields (ISA: edge, active high).
    assert_eq!(TIMER_OVERRIDE_FLAGS, 0x0000);
    // The derived flags for the four manifest combinations.
    assert_eq!(inti_flags(true, true), 0x000F);
    assert_eq!(inti_flags(true, false), 0x000D);
    assert_eq!(inti_flags(false, true), 0x0007);
    assert_eq!(inti_flags(false, false), 0x0005);
}

#[test]
fn madt_follows_other_manifest_and_pm_values() {
    let mut e = entries();
    e[1].base = 0xFEE0_1000; // Local APIC
    e[2].base = 0xFEC0_1000; // I/O APIC
    e[2].param0 = 0; // GSI base
    let manifest = BoardManifest::from_entries(e).unwrap();
    let mut pm_entry = STAGE4_DEVICES[7];
    pm_entry.gsi = 10;
    pm_entry.irq_flags = 0; // edge, active high
    let pm = AcpiPmEntry::from_entry(&pm_entry).unwrap();
    let t = build(&manifest, &pm, 0);
    assert_eq!(le32(&t, 36), 0xFEE0_1000);
    assert_eq!(le32(&t, 56), 0xFEC0_1000);
    assert_eq!(t[77], 10);
    assert_eq!(le32(&t, 78), 10);
    assert_eq!(le16(&t, 82), 0x0005);
    validate_madt(&t).unwrap();
}

#[test]
fn madt_follows_a_nonzero_gsi_base_that_covers_the_gsis() {
    // The inputs are 24 wide; a base of 0 is the board, a base above 2 uncovers GSI 2.
    let mut e = entries();
    e[2].param0 = 1; // covers 1..=24: GSI 2 and 9 both inside
    let t = build(&BoardManifest::from_entries(e).unwrap(), &board_pm(), 0);
    assert_eq!(le32(&t, 60), 1);
    validate_madt(&t).unwrap();
}

#[test]
fn madt_is_deterministic_and_does_not_depend_on_the_buffer() {
    let a = board_madt();
    let b = board_madt();
    assert_eq!(a, b);
    for fill in [0x00u8, 0xA5, 0xFF] {
        assert_eq!(
            build(&board_manifest(), &board_pm(), fill),
            a,
            "fill {fill:#x}"
        );
    }
    // A buffer exactly as long as the table is enough, a shorter one is not.
    let mut exact = [0u8; 84];
    assert_eq!(
        build_madt(&board_manifest(), &board_pm(), &mut exact),
        Ok(84)
    );
    let mut short = [0x77u8; 83];
    assert_eq!(
        build_madt(&board_manifest(), &board_pm(), &mut short),
        Err(MadtError::Table(TableError::TooShort))
    );
    assert!(
        short.iter().all(|&b| b == 0x77),
        "buffer untouched on error"
    );
}

#[test]
fn const_evaluation_gives_the_runtime_bytes() {
    const MANIFEST: BoardManifest = match BoardManifest::from_entries([
        STAGE4_DEVICES[0],
        STAGE4_DEVICES[1],
        STAGE4_DEVICES[2],
        STAGE4_DEVICES[3],
        STAGE4_DEVICES[4],
        STAGE4_DEVICES[5],
        STAGE4_DEVICES[6],
    ]) {
        Ok(m) => m,
        Err(_) => panic!("manifest"),
    };
    const PM: AcpiPmEntry = match AcpiPmEntry::from_entry(&STAGE4_DEVICES[7]) {
        Ok(p) => p,
        Err(_) => panic!("pm"),
    };
    const BYTES: [u8; MADT_LEN] = {
        let mut out = [0u8; MADT_LEN];
        match build_madt(&MANIFEST, &PM, &mut out) {
            Ok(MADT_LEN) => {}
            _ => panic!("madt"),
        }
        out
    };
    assert_eq!(BYTES.as_slice(), reference_madt().as_slice());
}

#[test]
fn validation_rejects_a_bad_header() {
    let t = board_madt();
    let mut sig = t.clone();
    sig[0] = b'X';
    assert_eq!(
        validate_madt(&sig),
        Err(MadtError::Table(TableError::SignatureMismatch))
    );
    assert_eq!(
        validate_madt(&t[..83]),
        Err(MadtError::Table(TableError::LengthMismatch {
            declared: 84,
            actual: 83
        }))
    );
    let mut sum = t.clone();
    sum[40] ^= 0x10;
    assert_eq!(
        validate_madt(&sum),
        Err(MadtError::Table(TableError::BadChecksum { sum: 0x10 }))
    );
    for revision in [0u8, 3, 4, 6] {
        assert_eq!(
            validate_madt(&patched(8, revision)),
            Err(MadtError::BadField("revision")),
            "revision {revision}"
        );
    }
    assert_eq!(
        validate_madt(&patched(40, 0)),
        Err(MadtError::BadField(
            "flags: PCAT_COMPAT must be the only bit"
        ))
    );
    assert_eq!(
        validate_madt(&patched(41, 1)),
        Err(MadtError::BadField(
            "flags: PCAT_COMPAT must be the only bit"
        ))
    );
    let mut zero_address = board_madt();
    zero_address[36..40].copy_from_slice(&[0; 4]);
    fix_checksum(&mut zero_address);
    assert_eq!(
        validate_madt(&zero_address),
        Err(MadtError::BadField("Local APIC address"))
    );
    assert_eq!(
        validate_madt(&patched(36, 0x10)),
        Err(MadtError::BadField("Local APIC address"))
    );
}

#[test]
fn validation_rejects_an_ioapic_gsi_range_that_does_not_cover_the_overrides() {
    // GSI base 3: IRQ 0 -> GSI 2 lies below the range.
    assert_eq!(
        validate_madt(&patched(60, 3)),
        Err(MadtError::GsiOutsideIoApic { gsi: 2 })
    );
    // GSI base 1: 1..=24 covers 2 and 9, so the table stays valid.
    validate_madt(&patched(60, 1)).unwrap();
    // The SCI GSI above the 24 inputs: GSI base 0, SCI GSI 24 (also source IRQ 9 -> GSI 24).
    assert_eq!(
        validate_madt(&patched(78, 24)),
        Err(MadtError::GsiOutsideIoApic { gsi: 24 })
    );
    validate_madt(&patched(78, 23)).unwrap();
    // A base so high that base + 24 overflows 32 bits.
    let mut t = board_madt();
    t[60..64].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    fix_checksum(&mut t);
    assert_eq!(validate_madt(&t), Err(MadtError::BadGsiBase(0xFFFF_FFF0)));
}

#[test]
fn validation_rejects_a_bad_override() {
    assert_eq!(validate_madt(&patched(66, 1)), Err(MadtError::BadIsoBus(1)));
    assert_eq!(
        validate_madt(&patched(76, 0xFF)),
        Err(MadtError::BadIsoBus(0xFF))
    );
    assert_eq!(
        validate_madt(&patched(77, 16)),
        Err(MadtError::BadIsoIrq(16))
    );
    assert_eq!(
        validate_madt(&patched(77, 0)),
        Err(MadtError::DuplicateIso(0))
    );
    // The override of IRQ 0 must stay on GSI 2.
    assert_eq!(
        validate_madt(&patched(68, 1)),
        Err(MadtError::TimerOverrideWrongGsi(1))
    );
    // No override of IRQ 0 at all: the first override becomes IRQ 1.
    assert_eq!(
        validate_madt(&patched(67, 1)),
        Err(MadtError::TimerOverrideMissing)
    );
    // Flags: reserved polarity 0b10, reserved trigger 0b10, reserved bit 4.
    assert_eq!(
        validate_madt(&patched(82, 0x02)),
        Err(MadtError::BadIsoFlags(0x02))
    );
    assert_eq!(
        validate_madt(&patched(82, 0x08)),
        Err(MadtError::BadIsoFlags(0x08))
    );
    assert_eq!(
        validate_madt(&patched(82, 0x1F)),
        Err(MadtError::BadIsoFlags(0x1F))
    );
    assert_eq!(
        validate_madt(&patched(83, 1)),
        Err(MadtError::BadIsoFlags(0x010F))
    );
}

#[test]
fn validation_rejects_a_bad_structure_list() {
    // Unknown type (an NMI source, type 3), x2APIC (9) and a zero length.
    assert_eq!(validate_madt(&patched(64, 3)), Err(MadtError::BadEntryArea));
    assert_eq!(validate_madt(&patched(64, 9)), Err(MadtError::BadEntryArea));
    assert_eq!(validate_madt(&patched(65, 0)), Err(MadtError::BadEntryArea));
    // A wrong length for the type, and one that runs past the table.
    assert_eq!(
        validate_madt(&patched(45, 12)),
        Err(MadtError::BadEntryArea)
    );
    assert_eq!(
        validate_madt(&patched(75, 11)),
        Err(MadtError::BadEntryArea)
    );
    // Local APIC: wrong UID, wrong APIC ID.
    for at in [46, 47] {
        assert_eq!(
            validate_madt(&patched(at, 1)),
            Err(MadtError::BadField("Local APIC: UID and APIC ID must be 0")),
            "at {at}"
        );
    }
    // Local APIC flags: not enabled; a reserved bit (bit 2) set.
    for value in [0u8, 5] {
        assert_eq!(
            validate_madt(&patched(48, value)),
            Err(MadtError::BadField("Local APIC: flags")),
            "flags {value}"
        );
    }
    // I/O APIC: reserved byte, zero address, address equal to the Local APIC's.
    assert_eq!(
        validate_madt(&patched(55, 1)),
        Err(MadtError::BadField("I/O APIC: reserved byte or address"))
    );
    let mut zero = board_madt();
    zero[56..60].copy_from_slice(&[0; 4]);
    fix_checksum(&mut zero);
    assert_eq!(
        validate_madt(&zero),
        Err(MadtError::BadField("I/O APIC: reserved byte or address"))
    );
    let mut same = board_madt();
    same[56..60].copy_from_slice(&0xFEE0_0000u32.to_le_bytes());
    fix_checksum(&mut same);
    assert_eq!(
        validate_madt(&same),
        Err(MadtError::BadField("I/O APIC: reserved byte or address"))
    );
    // A second Local APIC, a second I/O APIC: one CPU and one IOAPIC only.
    let second_cpu = appended(&[0, 8, 0, 0, 1, 0, 0, 0]);
    assert_eq!(
        validate_madt(&second_cpu),
        Err(MadtError::BadField("exactly one Local APIC is required"))
    );
    let second_ioapic = appended(&[1, 12, 2, 0, 0x00, 0x10, 0xC0, 0xFE, 0, 0, 0, 0]);
    assert_eq!(
        validate_madt(&second_ioapic),
        Err(MadtError::BadField("exactly one I/O APIC is required"))
    );
    // No I/O APIC / no Local APIC: the structure is cut out of the table.
    let mut no_ioapic = board_madt();
    no_ioapic.drain(52..64);
    resize(&mut no_ioapic);
    assert_eq!(
        validate_madt(&no_ioapic),
        Err(MadtError::BadField("exactly one I/O APIC is required"))
    );
    let mut no_cpu = board_madt();
    no_cpu.drain(44..52);
    resize(&mut no_cpu);
    assert_eq!(
        validate_madt(&no_cpu),
        Err(MadtError::BadField("exactly one Local APIC is required"))
    );
    // A structure cut off in the middle.
    let mut cut = board_madt();
    cut.truncate(80);
    resize(&mut cut);
    assert_eq!(validate_madt(&cut), Err(MadtError::BadEntryArea));
}

#[test]
fn builder_rejects_bad_manifest_entries() {
    let pm = board_pm();
    let mut out = [0x5Au8; 100];
    let mut run = |e: [DeviceEntry; 7], pm: &AcpiPmEntry| {
        let manifest = BoardManifest::from_entries(e).unwrap();
        let result = build_madt(&manifest, pm, &mut out);
        assert!(
            out.iter().all(|&b| b == 0x5A),
            "output untouched on {result:?}"
        );
        result
    };

    let mut e = entries();
    e[1].flags = 0;
    e[1].base = 0;
    e[1].length = 0;
    assert_eq!(run(e, &pm), Err(MadtError::MissingLocalApic));

    let mut e = entries();
    e[2].flags = 0;
    e[2].base = 0;
    e[2].length = 0;
    assert_eq!(run(e, &pm), Err(MadtError::MissingIoApic));

    let mut e = entries();
    e[2].flags |= FLAG_PIO;
    assert_eq!(run(e, &pm), Err(MadtError::PioApic(3)));

    for (index, base, length) in [
        (1usize, 0xFEE0_0010u64, 0x1000u64), // unaligned
        (1, 0xFEE0_0000, 0x800),             // wrong length
        (2, 0x1_FEC0_0000, 0x1000),          // above 4 GiB
        (2, 0xFEC0_0000, 0),
    ] {
        let mut e = entries();
        e[index].base = base;
        e[index].length = length;
        assert_eq!(
            run(e, &pm),
            Err(MadtError::BadApicWindow {
                device_type: index as u32 + 1,
                base,
                length
            })
        );
    }

    // GSI base: 3 uncovers GSI 2; a huge one overflows.
    let mut e = entries();
    e[2].param0 = 3;
    assert_eq!(run(e, &pm), Err(MadtError::GsiOutsideIoApic { gsi: 2 }));
    let mut e = entries();
    e[2].param0 = 0x1_0000_0000;
    assert_eq!(run(e, &pm), Err(MadtError::BadGsiBase(0x1_0000_0000)));
}

#[test]
fn builder_rejects_bad_sci_routes() {
    let manifest = board_manifest();
    for (gsi, expected) in [
        (16, MadtError::SciOutOfRange(16)),
        (GSI_NONE, MadtError::SciOutOfRange(GSI_NONE)),
        (0, MadtError::SciConflictsWithTimer(0)),
        (2, MadtError::SciConflictsWithTimer(2)),
    ] {
        let pm = AcpiPmEntry {
            sci_gsi: gsi,
            ..board_pm()
        };
        let mut out = [0u8; 100];
        assert_eq!(
            build_madt(&manifest, &pm, &mut out),
            Err(expected),
            "gsi {gsi}"
        );
    }
}
