//! MADT (Multiple APIC Description Table, signature `APIC`) revision 5 for the one-CPU stage-4
//! board (S4-31f).
//!
//! A `const fn` serializer (method A of S4-31): the bytes depend only on the Local APIC and
//! IOAPIC entries of the board manifest (device codes 2 and 3) and on the ACPI PM entry (code 9,
//! the SCI route); nothing is read from the machine at run time.
//!
//! Sources. Read from the ACPI **6.4** HTML edition through Context7 (the 6.6 pages answered
//! HTTP 403, so the 6.6 wording is NOT VERIFIED): the MADT header (Table 5.19: `Revision` = 5,
//! `Local Interrupt Controller Address` at offset 36, `Flags` at 40, structures from 44), the
//! `PCAT_COMPAT` bit 0, the Interrupt Source Override structure (Table 5.25: type 2, bus, source,
//! GSI, flags) and the MPS INTI flags (Table 5.26: polarity in bits 0..=1 with 01 = active high,
//! 11 = active low, 00 = conforms to the bus; trigger mode in bits 2..=3 with 01 = edge,
//! 11 = level, 00 = conforms to the bus; 10 is reserved in both). NOT returned by the lookups,
//! and therefore taken from the ACPI/ACPICA structure as known and pinned only by the
//! hand-assembled bytes in `madt/tests.rs`: the exact layout of the Processor Local APIC
//! structure (type 0, length 8: UID, APIC ID, flags with `Enabled` = bit 0) and of the I/O APIC
//! structure (type 1, length 12: ID, reserved, address, GSI base).
//!
//! Contents (MACHINE_CONTRACT.md, "Fixed-edge Local APIC register core (S4-25a)", "Fixed-edge
//! IOAPIC route (S4-26a)", "Расширение manifest ... (S4-31c)"; FIRMWARE_BOOT.md 6.3):
//! one Processor Local APIC (UID 0, APIC ID 0, enabled), one I/O APIC (address and GSI base from
//! the manifest, ID [`IOAPIC_ID`]), an override of ISA IRQ 0 to GSI 2 (the PIT stays, S4-29) and an
//! override of the SCI (source IRQ = SCI GSI, same GSI, flags from the PM entry). No NMI
//! sources, no x2APIC, no Local APIC address override.

use super::bytes::{copy_at, fill_range, head_mut, sub, tri};
use super::manifest::{AcpiPmEntry, BoardManifest, DeviceEntry, device_type};
use super::table::{SDT_HEADER_LEN, TableError, finish_checksum, validate_table, write_header};

/// MADT signature (Table 5.19).
pub const MADT_SIGNATURE: [u8; 4] = *b"APIC";
/// MADT revision: 5, the value of Table 5.19 in ACPI 6.4, the edition that was read. Revision 6
/// belongs to ACPI 6.5 (not read); revision 5 is accepted, and Linux handles 5 and 6 the same way for this table.
pub const MADT_REVISION: u8 = 5;
/// OEM Table ID of the MADT: an own string, 8 bytes.
pub const MADT_OEM_TABLE_ID: [u8; 8] = *b"ODFWAPIC";
/// Header (36) plus Local Interrupt Controller Address (4) plus Flags (4).
pub const MADT_FIXED_LEN: usize = SDT_HEADER_LEN + 8;
/// Multiple APIC Flags: `PCAT_COMPAT` (bit 0), the board has a (minimal) 8259 pair (S4-29).
pub const MADT_FLAG_PCAT_COMPAT: u32 = 1;

/// Interrupt controller structure types (Table 5.20) and lengths.
pub const TYPE_LOCAL_APIC: u8 = 0;
pub const TYPE_IO_APIC: u8 = 1;
pub const TYPE_INT_SRC_OVERRIDE: u8 = 2;
pub const LOCAL_APIC_ENTRY_LEN: usize = 8;
pub const IO_APIC_ENTRY_LEN: usize = 12;
pub const INT_SRC_OVERRIDE_LEN: usize = 10;
/// Length of the MADT of this board: header, three fixed-size groups and two overrides.
pub const MADT_LEN: usize =
    MADT_FIXED_LEN + LOCAL_APIC_ENTRY_LEN + IO_APIC_ENTRY_LEN + 2 * INT_SRC_OVERRIDE_LEN;

/// Processor Local APIC `Flags`: `Enabled` (bit 0).
pub const LOCAL_APIC_ENABLED: u32 = 1;
/// The one CPU of the stage-4 board: ACPI processor UID 0 and APIC ID 0 (S4-25a: "one-CPU
/// ... xAPIC ... with APIC ID 0").
pub const CPU_UID: u8 = 0;
pub const CPU_APIC_ID: u8 = 0;
/// I/O APIC ID in the MADT. The manifest carries no ID (the handoff entry holds address, length
/// and GSI base only, and the `IoApic` model has no readable ID register), so the firmware picks
/// one: 1, distinct from the Local APIC ID 0 so that the two never collide in an OS that keeps
/// one table of APIC IDs. NOT checked against a guest read of the device.
pub const IOAPIC_ID: u8 = 1;
/// Number of inputs of the I/O APIC: 24, the size of an Intel 82093AA-compatible IOAPIC. The
/// manifest and the `IoApic` model do not expose it (S4-26a gives one route, INTIN4, and no
/// Version value), so this is an assumption of the firmware, used to check the GSIs of the
/// overrides: they must lie in `[gsi_base, gsi_base + 24)`.
pub const IOAPIC_PIN_COUNT: u32 = 24;
/// Size and alignment of the APIC register windows (4 KiB, FIRMWARE_BOOT.md 6.3).
pub const APIC_WINDOW_LEN: u64 = 0x1000;

/// ISA timer: the PIT channel 0 line (IRQ 0) is routed to GSI 2 of the IOAPIC, as on every PC
/// (the 8259 IRQ 0 is wired to IOAPIC INTIN2). PIT stays (S4-29).
pub const ISA_TIMER_IRQ: u8 = 0;
pub const ISA_TIMER_GSI: u32 = 2;
/// The only bus an override may name: 0 = ISA (Table 5.25).
pub const BUS_ISA: u8 = 0;
/// Highest ISA IRQ (8259 input) an override may name as its source.
pub const MAX_ISA_IRQ: u8 = 15;

/// MPS INTI flags (Table 5.26). Polarity bits 0..=1, trigger bits 2..=3.
pub const INTI_POLARITY_CONFORMS: u16 = 0b00;
pub const INTI_POLARITY_HIGH: u16 = 0b01;
pub const INTI_POLARITY_LOW: u16 = 0b11;
pub const INTI_TRIGGER_CONFORMS: u16 = 0b00 << 2;
pub const INTI_TRIGGER_EDGE: u16 = 0b01 << 2;
pub const INTI_TRIGGER_LEVEL: u16 = 0b11 << 2;
/// Flags of the IRQ 0 override: "conforms to the bus" for both fields, which for ISA means
/// edge-triggered, active high (the decision of S4-31f).
pub const TIMER_OVERRIDE_FLAGS: u16 = INTI_POLARITY_CONFORMS | INTI_TRIGGER_CONFORMS;
/// Flags of the SCI override of this board: level-triggered, active low = `0b11 << 2 | 0b11` =
/// 0x000F. (The value 0x000D that the task text named is level / active HIGH: its low two bits
/// are 01.)
pub const SCI_OVERRIDE_FLAGS_LEVEL_LOW: u16 = INTI_TRIGGER_LEVEL | INTI_POLARITY_LOW;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MadtError {
    Table(TableError),
    /// The manifest has no present Local APIC entry (code 2).
    MissingLocalApic,
    /// The manifest has no present I/O APIC entry (code 3).
    MissingIoApic,
    /// An APIC entry of the manifest is a PIO range; APIC windows are memory-mapped.
    PioApic(u32),
    /// An APIC window that is not 4 KiB long, not 4 KiB aligned, zero or above 4 GiB.
    BadApicWindow {
        device_type: u32,
        base: u64,
        length: u64,
    },
    /// The I/O APIC GSI base (`param0`) does not fit 32 bits or leaves no room for 24 inputs.
    BadGsiBase(u64),
    /// The SCI GSI is above 15: it cannot be an ISA IRQ source of an override (and FADT
    /// `SCI_INT` is 8259-mode, 0..=15).
    SciOutOfRange(u32),
    /// The SCI is on IRQ 0 or on GSI 2, which the timer override already uses.
    SciConflictsWithTimer(u32),
    /// A GSI the table routes lies outside `[gsi_base, gsi_base + IOAPIC_PIN_COUNT)`.
    GsiOutsideIoApic {
        gsi: u32,
    },
    /// A finished table that breaks a rule of this board's MADT; the text names it.
    BadField(&'static str),
    /// The entry area is cut off, has an entry of length zero or of an unknown type.
    BadEntryArea,
    /// An override names a bus other than 0 (ISA).
    BadIsoBus(u8),
    /// An override names a source above IRQ 15.
    BadIsoIrq(u8),
    /// Two overrides name the same source IRQ.
    DuplicateIso(u8),
    /// Reserved or invalid MPS INTI flags (a reserved bit, or the value 0b10 in a field).
    BadIsoFlags(u16),
    /// The override of IRQ 0 is missing.
    TimerOverrideMissing,
    /// The override of IRQ 0 names a GSI other than 2.
    TimerOverrideWrongGsi(u32),
}

impl From<TableError> for MadtError {
    fn from(error: TableError) -> Self {
        MadtError::Table(error)
    }
}

/// The checked address window of one APIC entry of the manifest.
const fn apic_window(entry: &DeviceEntry, missing: MadtError) -> Result<u32, MadtError> {
    if !entry.is_present() {
        return Err(missing);
    }
    if entry.is_pio() {
        return Err(MadtError::PioApic(entry.device_type));
    }
    if entry.base == 0
        || entry.base > u32::MAX as u64
        || entry.base % APIC_WINDOW_LEN != 0
        || entry.length != APIC_WINDOW_LEN
    {
        return Err(MadtError::BadApicWindow {
            device_type: entry.device_type,
            base: entry.base,
            length: entry.length,
        });
    }
    Ok(entry.base as u32)
}

/// MPS INTI flags of an override from the polarity and trigger of the manifest.
const fn inti_flags(level: bool, active_low: bool) -> u16 {
    let trigger = if level {
        INTI_TRIGGER_LEVEL
    } else {
        INTI_TRIGGER_EDGE
    };
    let polarity = if active_low {
        INTI_POLARITY_LOW
    } else {
        INTI_POLARITY_HIGH
    };
    trigger | polarity
}

const fn in_ioapic_range(gsi_base: u32, gsi: u32) -> bool {
    gsi >= gsi_base && gsi - gsi_base < IOAPIC_PIN_COUNT
}

const fn write_int_src_override(table: &mut [u8], at: usize, source: u8, gsi: u32, flags: u16) {
    table[at] = TYPE_INT_SRC_OVERRIDE;
    table[at + 1] = INT_SRC_OVERRIDE_LEN as u8;
    table[at + 2] = BUS_ISA;
    table[at + 3] = source;
    copy_at(table, at + 4, &gsi.to_le_bytes());
    copy_at(table, at + 8, &flags.to_le_bytes());
}

/// Builds the MADT into `out[..84]` and returns the length.
///
/// A `const fn`: evaluated at compile time it gives static bytes, called at run time the same
/// bytes. The inputs are checked first (on an error before any byte is written `out` is
/// untouched); the finished table is checked by [`validate_madt`] before the length is returned.
pub const fn build_madt(
    manifest: &BoardManifest,
    pm: &AcpiPmEntry,
    out: &mut [u8],
) -> Result<usize, MadtError> {
    let lapic = match manifest.device(device_type::LOCAL_APIC) {
        Some(entry) => tri!(apic_window(entry, MadtError::MissingLocalApic)),
        None => return Err(MadtError::MissingLocalApic),
    };
    let (ioapic, gsi_base) = match manifest.device(device_type::IOAPIC) {
        Some(entry) => {
            let base = tri!(apic_window(entry, MadtError::MissingIoApic));
            // `param0` is the GSI base (FIRMWARE_BOOT.md 6.3, code 3).
            if entry.param0 > u32::MAX as u64 - IOAPIC_PIN_COUNT as u64 {
                return Err(MadtError::BadGsiBase(entry.param0));
            }
            (base, entry.param0 as u32)
        }
        None => return Err(MadtError::MissingIoApic),
    };
    if pm.sci_gsi > MAX_ISA_IRQ as u32 {
        return Err(MadtError::SciOutOfRange(pm.sci_gsi));
    }
    if pm.sci_gsi == ISA_TIMER_IRQ as u32 || pm.sci_gsi == ISA_TIMER_GSI {
        return Err(MadtError::SciConflictsWithTimer(pm.sci_gsi));
    }
    if !in_ioapic_range(gsi_base, ISA_TIMER_GSI) {
        return Err(MadtError::GsiOutsideIoApic { gsi: ISA_TIMER_GSI });
    }
    if !in_ioapic_range(gsi_base, pm.sci_gsi) {
        return Err(MadtError::GsiOutsideIoApic { gsi: pm.sci_gsi });
    }
    if out.len() < MADT_LEN {
        return Err(MadtError::Table(TableError::TooShort));
    }
    let table = head_mut(out, MADT_LEN);
    tri!(
        write_header(
            table,
            MADT_SIGNATURE,
            MADT_REVISION,
            MADT_OEM_TABLE_ID,
            MADT_LEN as u32,
        ),
        MadtError::Table
    );
    fill_range(table, SDT_HEADER_LEN, MADT_LEN, 0);
    copy_at(table, 36, &lapic.to_le_bytes());
    copy_at(table, 40, &MADT_FLAG_PCAT_COMPAT.to_le_bytes());

    let mut at = MADT_FIXED_LEN;
    table[at] = TYPE_LOCAL_APIC;
    table[at + 1] = LOCAL_APIC_ENTRY_LEN as u8;
    table[at + 2] = CPU_UID;
    table[at + 3] = CPU_APIC_ID;
    copy_at(table, at + 4, &LOCAL_APIC_ENABLED.to_le_bytes());
    at += LOCAL_APIC_ENTRY_LEN;

    table[at] = TYPE_IO_APIC;
    table[at + 1] = IO_APIC_ENTRY_LEN as u8;
    table[at + 2] = IOAPIC_ID;
    // at + 3: reserved, zero.
    copy_at(table, at + 4, &ioapic.to_le_bytes());
    copy_at(table, at + 8, &gsi_base.to_le_bytes());
    at += IO_APIC_ENTRY_LEN;

    write_int_src_override(
        table,
        at,
        ISA_TIMER_IRQ,
        ISA_TIMER_GSI,
        TIMER_OVERRIDE_FLAGS,
    );
    at += INT_SRC_OVERRIDE_LEN;
    write_int_src_override(
        table,
        at,
        pm.sci_gsi as u8,
        pm.sci_gsi,
        inti_flags(pm.sci_level, pm.sci_active_low),
    );

    tri!(finish_checksum(table), MadtError::Table);
    tri!(validate_madt(table));
    Ok(MADT_LEN)
}

const fn le16(table: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([table[at], table[at + 1]])
}

const fn le32(table: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([table[at], table[at + 1], table[at + 2], table[at + 3]])
}

/// Checks a finished MADT: signature, `Length`, checksum, revision, flags, the Local APIC
/// address, and the structure list (exactly one enabled Local APIC with APIC ID 0 and one I/O
/// APIC; overrides on bus 0, source IRQ 0..=15, one per source, valid INTI flags, GSI inside the
/// IOAPIC range; IRQ 0 overridden to GSI 2). Unknown structure types are refused.
pub const fn validate_madt(table: &[u8]) -> Result<(), MadtError> {
    tri!(validate_table(table, &MADT_SIGNATURE), MadtError::Table);
    if table[8] != MADT_REVISION {
        return Err(MadtError::BadField("revision"));
    }
    if table.len() < MADT_FIXED_LEN {
        return Err(MadtError::BadEntryArea);
    }
    let lapic_address = le32(table, 36);
    if lapic_address == 0 || lapic_address as u64 % APIC_WINDOW_LEN != 0 {
        return Err(MadtError::BadField("Local APIC address"));
    }
    if le32(table, 40) != MADT_FLAG_PCAT_COMPAT {
        return Err(MadtError::BadField(
            "flags: PCAT_COMPAT must be the only bit",
        ));
    }

    let mut lapics = 0u32;
    let mut ioapics = 0u32;
    let mut gsi_base = 0u32;
    // Source IRQ -> GSI of each override, checked against the IOAPIC range after the walk (an
    // override may precede the I/O APIC structure).
    let mut iso_gsi = [0u32; MAX_ISA_IRQ as usize + 1];
    let mut iso_seen = 0u32;
    let mut at = MADT_FIXED_LEN;
    while at < table.len() {
        if table.len() - at < 2 {
            return Err(MadtError::BadEntryArea);
        }
        let kind = table[at];
        let len = table[at + 1] as usize;
        if len < 2 || len > table.len() - at {
            return Err(MadtError::BadEntryArea);
        }
        let entry = sub(table, at, at + len);
        match kind {
            TYPE_LOCAL_APIC => {
                if len != LOCAL_APIC_ENTRY_LEN {
                    return Err(MadtError::BadEntryArea);
                }
                lapics += 1;
                if entry[2] != CPU_UID || entry[3] != CPU_APIC_ID {
                    return Err(MadtError::BadField("Local APIC: UID and APIC ID must be 0"));
                }
                // This one-CPU profile requires Enabled=1 and Online Capable=0 (ACPI 6.4
                // Table 5.23); the validator therefore accepts exactly the Enabled bit.
                let flags = le32(entry, 4);
                if flags != LOCAL_APIC_ENABLED {
                    return Err(MadtError::BadField("Local APIC: flags"));
                }
            }
            TYPE_IO_APIC => {
                if len != IO_APIC_ENTRY_LEN {
                    return Err(MadtError::BadEntryArea);
                }
                ioapics += 1;
                let address = le32(entry, 4);
                if entry[3] != 0
                    || address == 0
                    || address as u64 % APIC_WINDOW_LEN != 0
                    || address == lapic_address
                {
                    return Err(MadtError::BadField("I/O APIC: reserved byte or address"));
                }
                gsi_base = le32(entry, 8);
                if gsi_base > u32::MAX - IOAPIC_PIN_COUNT {
                    return Err(MadtError::BadGsiBase(gsi_base as u64));
                }
            }
            TYPE_INT_SRC_OVERRIDE => {
                if len != INT_SRC_OVERRIDE_LEN {
                    return Err(MadtError::BadEntryArea);
                }
                if entry[2] != BUS_ISA {
                    return Err(MadtError::BadIsoBus(entry[2]));
                }
                let source = entry[3];
                if source > MAX_ISA_IRQ {
                    return Err(MadtError::BadIsoIrq(source));
                }
                if iso_seen >> source & 1 != 0 {
                    return Err(MadtError::DuplicateIso(source));
                }
                iso_seen |= 1 << source;
                iso_gsi[source as usize] = le32(entry, 4);
                let flags = le16(entry, 8);
                // Bits above 3 are reserved; the value 0b10 is reserved in both fields.
                if flags >> 4 != 0 || flags & 0b11 == 0b10 || flags >> 2 & 0b11 == 0b10 {
                    return Err(MadtError::BadIsoFlags(flags));
                }
            }
            _ => return Err(MadtError::BadEntryArea),
        }
        at += len;
    }
    if lapics != 1 {
        return Err(MadtError::BadField("exactly one Local APIC is required"));
    }
    if ioapics != 1 {
        return Err(MadtError::BadField("exactly one I/O APIC is required"));
    }
    if iso_seen & 1 == 0 {
        return Err(MadtError::TimerOverrideMissing);
    }
    if iso_gsi[ISA_TIMER_IRQ as usize] != ISA_TIMER_GSI {
        return Err(MadtError::TimerOverrideWrongGsi(
            iso_gsi[ISA_TIMER_IRQ as usize],
        ));
    }
    let mut source = 0;
    while source <= MAX_ISA_IRQ as usize {
        if iso_seen >> source & 1 != 0 && !in_ioapic_range(gsi_base, iso_gsi[source]) {
            return Err(MadtError::GsiOutsideIoApic {
                gsi: iso_gsi[source],
            });
        }
        source += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
