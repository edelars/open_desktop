//! Board manifest: the device table of the handoff block v1.0 (FIRMWARE_BOOT.md, section 6.3).
//!
//! Only fields defined there exist here. The region table, header fields and the parsing of a
//! whole handoff block belong to the firmware core (S7-04); this module covers the 48-byte
//! device entries the ACPI tables are built from.

/// Size of one device table entry (FIRMWARE_BOOT.md, 6.3).
pub const DEVICE_ENTRY_SIZE: usize = 48;
/// Number of device entries in v1.0: types 1..=7, each exactly once.
pub const DEVICE_COUNT: usize = 7;

/// `flags` bit 0: the device exists on the board.
pub const FLAG_PRESENT: u32 = 1 << 0;
/// `flags` bit 1: `base`/`length` are I/O ports (otherwise MMIO).
pub const FLAG_PIO: u32 = 1 << 1;
/// `gsi` value meaning "no interrupt".
pub const GSI_NONE: u32 = 0xFFFF_FFFF;

/// `irq_flags` bit 0: 1 = level, 0 = edge.
pub const IRQ_LEVEL: u32 = 1 << 0;
/// `irq_flags` bit 1: 1 = active-low, 0 = active-high.
pub const IRQ_ACTIVE_LOW: u32 = 1 << 1;

/// Device type codes of v1.0 (FIRMWARE_BOOT.md, 6.3).
pub mod device_type {
    pub const COM1: u32 = 1;
    pub const LOCAL_APIC: u32 = 2;
    pub const IOAPIC: u32 = 3;
    pub const DISPLAY: u32 = 4;
    pub const NVRAM: u32 = 5;
    pub const TPM_CRB: u32 = 6;
    pub const PCIE_ECAM: u32 = 7;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// A type code outside 1..=7.
    UnknownDeviceType(u32),
    /// A type code appears twice, or one of 1..=7 is missing.
    DeviceTypeNotUnique(u32),
    /// A flag bit other than PRESENT/PIO is set.
    ReservedFlags(u32),
    /// `PRESENT = 0` but `base`/`length` are not zero.
    AbsentDeviceHasAddress(u32),
    /// `irq_flags` has bits above bit 1.
    ReservedIrqFlags(u32),
}

/// One 48-byte device entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceEntry {
    pub device_type: u32,
    pub flags: u32,
    pub base: u64,
    pub length: u64,
    pub gsi: u32,
    pub irq_flags: u32,
    pub param0: u64,
    pub param1: u64,
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

impl DeviceEntry {
    /// Decodes an entry (offsets from FIRMWARE_BOOT.md 6.3: `type` 0x00, `flags` 0x04, `base`
    /// 0x08, `length` 0x10, `gsi` 0x18, `irq_flags` 0x1C, `param0` 0x20, `param1` 0x28).
    pub fn parse(raw: &[u8; DEVICE_ENTRY_SIZE]) -> Self {
        DeviceEntry {
            device_type: le32(raw, 0x00),
            flags: le32(raw, 0x04),
            base: le64(raw, 0x08),
            length: le64(raw, 0x10),
            gsi: le32(raw, 0x18),
            irq_flags: le32(raw, 0x1C),
            param0: le64(raw, 0x20),
            param1: le64(raw, 0x28),
        }
    }

    pub fn is_present(&self) -> bool {
        self.flags & FLAG_PRESENT != 0
    }

    pub fn is_pio(&self) -> bool {
        self.flags & FLAG_PIO != 0
    }
}

/// The seven devices of the handoff block, indexed by type code minus one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardManifest {
    devices: [DeviceEntry; DEVICE_COUNT],
}

impl BoardManifest {
    /// Accepts the entries in any order and checks FIRMWARE_BOOT.md 6.3/6.5 for devices: each
    /// type 1..=7 exactly once, reserved bits zero, no address for an absent device.
    pub fn from_entries(entries: [DeviceEntry; DEVICE_COUNT]) -> Result<Self, ManifestError> {
        let mut slots = [DeviceEntry::default(); DEVICE_COUNT];
        let mut seen = [false; DEVICE_COUNT];
        for entry in entries {
            let index = match entry.device_type {
                1..=7 => (entry.device_type - 1) as usize,
                other => return Err(ManifestError::UnknownDeviceType(other)),
            };
            if seen[index] {
                return Err(ManifestError::DeviceTypeNotUnique(entry.device_type));
            }
            if entry.flags & !(FLAG_PRESENT | FLAG_PIO) != 0 {
                return Err(ManifestError::ReservedFlags(entry.flags));
            }
            if !entry.is_present() && (entry.base != 0 || entry.length != 0) {
                return Err(ManifestError::AbsentDeviceHasAddress(entry.device_type));
            }
            if entry.irq_flags & !(IRQ_LEVEL | IRQ_ACTIVE_LOW) != 0 {
                return Err(ManifestError::ReservedIrqFlags(entry.irq_flags));
            }
            seen[index] = true;
            slots[index] = entry;
        }
        // Seven entries, no repeats and all in range: every slot is filled.
        Ok(BoardManifest { devices: slots })
    }

    /// The entry of a device type (1..=7).
    pub fn device(&self, device_type: u32) -> Option<&DeviceEntry> {
        let index = usize::try_from(device_type.checked_sub(1)?).ok()?;
        self.devices.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(device_type: u32, flags: u32, base: u64) -> DeviceEntry {
        DeviceEntry {
            device_type,
            flags,
            base,
            length: if flags & FLAG_PRESENT != 0 { 8 } else { 0 },
            gsi: GSI_NONE,
            ..DeviceEntry::default()
        }
    }

    fn all_absent() -> [DeviceEntry; DEVICE_COUNT] {
        core::array::from_fn(|i| entry(i as u32 + 1, 0, 0))
    }

    #[test]
    fn parse_reads_the_documented_offsets() {
        // FIRMWARE_BOOT.md 6.3, COM1 row: PRESENT|PIO, base 0x3F8, length 8, gsi 4, edge/high,
        // param0 = 1. Bytes written out by hand in little-endian order.
        let raw: [u8; DEVICE_ENTRY_SIZE] = [
            0x01, 0x00, 0x00, 0x00, // type
            0x03, 0x00, 0x00, 0x00, // flags
            0xF8, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // base
            0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // length
            0x04, 0x00, 0x00, 0x00, // gsi
            0x00, 0x00, 0x00, 0x00, // irq_flags
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // param0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // param1
        ];
        let e = DeviceEntry::parse(&raw);
        assert_eq!(
            (e.device_type, e.flags, e.base, e.length, e.gsi, e.param0),
            (1, 3, 0x3F8, 8, 4, 1)
        );
        assert!(e.is_present() && e.is_pio());
    }

    #[test]
    fn entries_may_come_in_any_order_and_are_indexed_by_type() {
        let mut entries = all_absent();
        entries.reverse();
        let m = BoardManifest::from_entries(entries).unwrap();
        for t in 1..=7 {
            assert_eq!(m.device(t).unwrap().device_type, t);
        }
        assert!(m.device(0).is_none() && m.device(8).is_none());
    }

    #[test]
    fn duplicate_or_unknown_types_are_rejected() {
        let mut dup = all_absent();
        dup[1].device_type = 1;
        assert_eq!(
            BoardManifest::from_entries(dup),
            Err(ManifestError::DeviceTypeNotUnique(1))
        );
        let mut unknown = all_absent();
        unknown[3].device_type = 9;
        assert_eq!(
            BoardManifest::from_entries(unknown),
            Err(ManifestError::UnknownDeviceType(9))
        );
    }

    #[test]
    fn reserved_bits_and_addresses_of_absent_devices_are_rejected() {
        let mut flags = all_absent();
        flags[0].flags = 1 << 2;
        assert_eq!(
            BoardManifest::from_entries(flags),
            Err(ManifestError::ReservedFlags(1 << 2))
        );
        let mut addr = all_absent();
        addr[4].base = 0xFF00_0000;
        assert_eq!(
            BoardManifest::from_entries(addr),
            Err(ManifestError::AbsentDeviceHasAddress(5))
        );
        let mut irq = all_absent();
        irq[0].irq_flags = 4;
        assert_eq!(
            BoardManifest::from_entries(irq),
            Err(ManifestError::ReservedIrqFlags(4))
        );
    }
}
