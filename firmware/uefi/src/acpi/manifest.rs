//! Board manifest: the device table of the handoff block v1.0 (FIRMWARE_BOOT.md, section 6.3).
//!
//! Only fields defined there exist here. The region table, header fields and the parsing of a
//! whole handoff block belong to the firmware core (S7-04); this module covers the 48-byte
//! device entries the ACPI tables are built from.

use super::bytes::copy_at;

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
    /// NVMe, including the optional PCI aperture and INTx route in handoff v1.3.
    pub const NVME: u32 = 8;
    /// ACPI fixed hardware: PM block and SCI route (handoff v1.2, S4-31c).
    pub const ACPI_PM: u32 = 9;
    /// CMOS RTC at PIO 0x70/0x71 with a deterministic UTC base time (handoff v1.4, S7-14).
    pub const RTC_CMOS: u32 = 10;
    pub const MAX_RTC_UNIX_SECONDS: u64 = 253_402_300_799;
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
    /// An ACPI PM entry (code 9) that cannot be described by the FADT; the text names the field.
    BadAcpiPm(&'static str),
    /// A handoff RTC entry (code 10) that does not match its fixed PIO/time contract.
    BadRtc(&'static str),
    BadPciNvme,
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

/// An all-zero entry, the filler of unused slots (`DeviceEntry::default()` is not const).
const ABSENT_ENTRY: DeviceEntry = DeviceEntry {
    device_type: 0,
    flags: 0,
    base: 0,
    length: 0,
    gsi: 0,
    irq_flags: 0,
    param0: 0,
    param1: 0,
};

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

    /// Encodes the entry into the 48 bytes of FIRMWARE_BOOT.md 6.3 (the inverse of
    /// [`DeviceEntry::parse`]); the manifest hash of the ACPI blob (S4-31d) is taken over them.
    pub const fn to_bytes(&self) -> [u8; DEVICE_ENTRY_SIZE] {
        let mut raw = [0u8; DEVICE_ENTRY_SIZE];
        copy_at(&mut raw, 0x00, &self.device_type.to_le_bytes());
        copy_at(&mut raw, 0x04, &self.flags.to_le_bytes());
        copy_at(&mut raw, 0x08, &self.base.to_le_bytes());
        copy_at(&mut raw, 0x10, &self.length.to_le_bytes());
        copy_at(&mut raw, 0x18, &self.gsi.to_le_bytes());
        copy_at(&mut raw, 0x1C, &self.irq_flags.to_le_bytes());
        copy_at(&mut raw, 0x20, &self.param0.to_le_bytes());
        copy_at(&mut raw, 0x28, &self.param1.to_le_bytes());
        raw
    }

    pub const fn is_present(&self) -> bool {
        self.flags & FLAG_PRESENT != 0
    }

    pub const fn is_pio(&self) -> bool {
        self.flags & FLAG_PIO != 0
    }
}

/// The seven base handoff entries and validated optional devices used by ACPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardManifest {
    devices: [DeviceEntry; DEVICE_COUNT],
    pci_nvme: Option<PciNvmeRoute>,
    rtc: Option<DeviceEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PciNvmeRoute {
    pub min: u32,
    pub max: u32,
    pub length: u32,
    pub slot: u8,
    pub gsi: u32,
}

impl BoardManifest {
    /// Accepts the entries in any order and checks FIRMWARE_BOOT.md 6.3/6.5 for devices: each
    /// type 1..=7 exactly once, reserved bits zero, no address for an absent device.
    pub const fn from_entries(entries: [DeviceEntry; DEVICE_COUNT]) -> Result<Self, ManifestError> {
        let mut slots = [ABSENT_ENTRY; DEVICE_COUNT];
        let mut seen = [false; DEVICE_COUNT];
        let mut i = 0;
        while i < DEVICE_COUNT {
            let entry = entries[i];
            i += 1;
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
        Ok(BoardManifest {
            devices: slots,
            pci_nvme: None,
            rtc: None,
        })
    }

    pub(super) const fn with_nvme(mut self, entry: &DeviceEntry) -> Result<Self, ManifestError> {
        // Handoff v1.1/v1.2 NVMe is polled at a fixed MMIO address.
        if entry.gsi == GSI_NONE && entry.irq_flags == 0 && entry.param0 == 0 && entry.param1 == 0 {
            return Ok(self);
        }
        let Some(ecam) = self.device(device_type::PCIE_ECAM) else {
            return Err(ManifestError::BadPciNvme);
        };
        let Some(ioapic) = self.device(device_type::IOAPIC) else {
            return Err(ManifestError::BadPciNvme);
        };
        let Some(end) = entry.base.checked_add(entry.length) else {
            return Err(ManifestError::BadPciNvme);
        };
        let Some(ecam_end) = ecam.base.checked_add(ecam.length) else {
            return Err(ManifestError::BadPciNvme);
        };
        if entry.flags != FLAG_PRESENT
            || entry.base == 0
            || entry.length > u32::MAX as u64
            || !entry.length.is_power_of_two()
            || entry.base & (entry.length - 1) != 0
            || end > (1u64 << 32)
            || (entry.base < ecam_end && ecam.base < end)
            || entry.gsi < 16
            || entry.gsi >= 24
            || entry.irq_flags != (IRQ_LEVEL | IRQ_ACTIVE_LOW)
            || entry.param0 > 31
            || entry.param1 != 0
            || !ecam.is_present()
            || !ioapic.is_present()
        {
            return Err(ManifestError::BadPciNvme);
        }
        self.pci_nvme = Some(PciNvmeRoute {
            min: entry.base as u32,
            max: (end - 1) as u32,
            length: entry.length as u32,
            slot: entry.param0 as u8,
            gsi: entry.gsi,
        });
        Ok(self)
    }

    pub(super) const fn pci_nvme(&self) -> Option<PciNvmeRoute> {
        self.pci_nvme
    }

    pub(super) const fn with_rtc(mut self, entry: DeviceEntry) -> Result<Self, ManifestError> {
        if entry.flags != (FLAG_PRESENT | FLAG_PIO) {
            return Err(ManifestError::BadRtc("flags"));
        }
        if entry.base != 0x70 || entry.length != 2 {
            return Err(ManifestError::BadRtc("ports"));
        }
        if entry.gsi != GSI_NONE || entry.irq_flags != 0 || entry.param1 != 0 {
            return Err(ManifestError::BadRtc("reserved fields"));
        }
        if entry.param0 > device_type::MAX_RTC_UNIX_SECONDS {
            return Err(ManifestError::BadRtc("base UTC time"));
        }
        self.rtc = Some(entry);
        Ok(self)
    }

    pub(super) const fn rtc(&self) -> Option<&DeviceEntry> {
        self.rtc.as_ref()
    }

    /// The entry of a device type (1..=7).
    pub const fn device(&self, device_type: u32) -> Option<&DeviceEntry> {
        if device_type == 0 || device_type as usize > DEVICE_COUNT {
            return None;
        }
        Some(&self.devices[device_type as usize - 1])
    }
}

/// Length of `PM1a_EVT_BLK` (PM1_STS + PM1_EN), `PM1a_CNT_BLK`, `PM_TMR_BLK` and `RESET_REG`
/// in bytes. They are constants of the board's PM block (`ACPI_PM_DESCRIPTOR`), not handoff fields.
pub(super) const PM1_EVT_LEN: u32 = 4;
pub(super) const PM1_CNT_LEN: u32 = 2;
pub(super) const PM_TMR_LEN: u32 = 4;
pub(super) const RESET_REG_LEN: u32 = 1;

/// The ACPI fixed hardware entry (device code 9, handoff v1.2): the PM block ports and the SCI
/// route. Decoded from `param0` (four `u16` lanes: `PM1a_EVT_BLK`, `PM1a_CNT_BLK`, `PM_TMR_BLK`,
/// `RESET_REG`) and `param1` (`RESET_VALUE`, S5 `SLP_TYP`). The FADT serializer (S4-31e) takes
/// this value; the whole-block parser of the handoff is S7-04.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcpiPmEntry {
    /// First port and number of ports of the whole PM block.
    pub base_port: u16,
    pub port_count: u16,
    pub pm1a_evt_blk: u16,
    pub pm1a_cnt_blk: u16,
    pub pm_tmr_blk: u16,
    pub reset_reg: u16,
    pub reset_value: u8,
    pub slp_typ_s5: u8,
    /// SCI global system interrupt.
    pub sci_gsi: u32,
    pub sci_level: bool,
    pub sci_active_low: bool,
}

impl AcpiPmEntry {
    /// Validates a code-9 entry: `PRESENT | PIO` only, block inside the 16-bit port space, the
    /// four registers inside the block, in order and without overlap, a real SCI GSI, `param1`
    /// bits above 16 and `SLP_TYP` above 7 are zero.
    pub const fn from_entry(entry: &DeviceEntry) -> Result<Self, ManifestError> {
        if entry.device_type != device_type::ACPI_PM {
            return Err(ManifestError::BadAcpiPm("device type is not 9"));
        }
        if entry.flags != FLAG_PRESENT | FLAG_PIO {
            return Err(ManifestError::BadAcpiPm("flags must be PRESENT | PIO"));
        }
        if entry.irq_flags & !(IRQ_LEVEL | IRQ_ACTIVE_LOW) != 0 {
            return Err(ManifestError::ReservedIrqFlags(entry.irq_flags));
        }
        if entry.length == 0 || entry.base.saturating_add(entry.length) > 0x1_0000 {
            return Err(ManifestError::BadAcpiPm(
                "block outside the 16-bit port space",
            ));
        }
        if entry.gsi == GSI_NONE {
            return Err(ManifestError::BadAcpiPm("SCI needs a GSI"));
        }
        if entry.param1 >> 16 != 0 || (entry.param1 >> 8) & 0xFF > 7 {
            return Err(ManifestError::BadAcpiPm(
                "param1 reserved bits or SLP_TYP above 7",
            ));
        }
        let evt = (entry.param0 & 0xFFFF) as u32;
        let cnt = ((entry.param0 >> 16) & 0xFFFF) as u32;
        let tmr = ((entry.param0 >> 32) & 0xFFFF) as u32;
        let reset = ((entry.param0 >> 48) & 0xFFFF) as u32;
        let (start, end) = (entry.base as u32, (entry.base + entry.length) as u32);
        if evt < start
            || evt + PM1_EVT_LEN > cnt
            || cnt + PM1_CNT_LEN > tmr
            || tmr + PM_TMR_LEN > reset
            || reset + RESET_REG_LEN > end
        {
            return Err(ManifestError::BadAcpiPm(
                "registers must lie in the block in the order EVT, CNT, TMR, RESET",
            ));
        }
        Ok(AcpiPmEntry {
            base_port: entry.base as u16,
            port_count: entry.length as u16,
            pm1a_evt_blk: evt as u16,
            pm1a_cnt_blk: cnt as u16,
            pm_tmr_blk: tmr as u16,
            reset_reg: reset as u16,
            reset_value: (entry.param1 & 0xFF) as u8,
            slp_typ_s5: ((entry.param1 >> 8) & 0xFF) as u8,
            sci_gsi: entry.gsi,
            sci_level: entry.irq_flags & IRQ_LEVEL != 0,
            sci_active_low: entry.irq_flags & IRQ_ACTIVE_LOW != 0,
        })
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
    fn to_bytes_is_the_inverse_of_parse() {
        let entry = DeviceEntry {
            device_type: 9,
            flags: FLAG_PRESENT | FLAG_PIO,
            base: 0x600,
            length: 0x10,
            gsi: 9,
            irq_flags: 0b11,
            param0: 0x060C_0608_0604_0600,
            param1: 0x0506,
        };
        let raw = entry.to_bytes();
        assert_eq!(DeviceEntry::parse(&raw), entry);
        assert_eq!(&raw[0x20..0x28], &0x060C_0608_0604_0600u64.to_le_bytes());
        const CONST_RAW: [u8; DEVICE_ENTRY_SIZE] = ABSENT_ENTRY.to_bytes();
        assert_eq!(CONST_RAW, [0u8; DEVICE_ENTRY_SIZE]);
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

    /// The code-9 entry as `od-machine` emits it with `acpi_tables` (FIRMWARE_BOOT.md 6.3),
    /// bytes written out by hand in little-endian order (the same 48 bytes as the machine's
    /// own test).
    const PM_ENTRY_RAW: [u8; DEVICE_ENTRY_SIZE] = [
        0x09, 0x00, 0x00, 0x00, // type
        0x03, 0x00, 0x00, 0x00, // flags: PRESENT | PIO
        0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // base 0x0600
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // length 0x10
        0x09, 0x00, 0x00, 0x00, // gsi 9
        0x03, 0x00, 0x00, 0x00, // level, active-low
        0x00, 0x06, 0x04, 0x06, 0x08, 0x06, 0x0C, 0x06, // param0
        0x06, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // param1
    ];

    #[test]
    fn code_9_entry_is_decoded_into_the_pm_block_and_the_sci_route() {
        let entry = DeviceEntry::parse(&PM_ENTRY_RAW);
        assert_eq!(entry.device_type, device_type::ACPI_PM);
        let pm = AcpiPmEntry::from_entry(&entry).unwrap();
        assert_eq!(
            pm,
            AcpiPmEntry {
                base_port: 0x600,
                port_count: 0x10,
                pm1a_evt_blk: 0x600,
                pm1a_cnt_blk: 0x604,
                pm_tmr_blk: 0x608,
                reset_reg: 0x60C,
                reset_value: 0x06,
                slp_typ_s5: 5,
                sci_gsi: 9,
                sci_level: true,
                sci_active_low: true,
            }
        );
        // const fn: usable in a const item.
        const PM: Result<AcpiPmEntry, ManifestError> = AcpiPmEntry::from_entry(&DeviceEntry {
            device_type: 9,
            flags: FLAG_PRESENT | FLAG_PIO,
            base: 0x600,
            length: 0x10,
            gsi: 9,
            irq_flags: IRQ_LEVEL | IRQ_ACTIVE_LOW,
            param0: 0x060C_0608_0604_0600,
            param1: 0x0506,
        });
        assert_eq!(PM, Ok(pm));
    }

    #[test]
    fn code_9_entries_that_the_fadt_cannot_use_are_rejected() {
        let good = DeviceEntry::parse(&PM_ENTRY_RAW);
        let bad = |edit: fn(&mut DeviceEntry)| {
            let mut e = good;
            edit(&mut e);
            AcpiPmEntry::from_entry(&e)
        };
        assert!(matches!(
            bad(|e| e.device_type = 7),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.flags = FLAG_PRESENT),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.flags = 0),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.length = 0),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.base = 0xFFF8),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.gsi = GSI_NONE),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert_eq!(
            bad(|e| e.irq_flags = 4),
            Err(ManifestError::ReservedIrqFlags(4))
        );
        assert!(matches!(
            bad(|e| e.param1 |= 1 << 16),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.param1 = 0x0806),
            Err(ManifestError::BadAcpiPm(_))
        ));
        // A register outside the block, out of order or overlapping.
        assert!(matches!(
            bad(|e| e.length = 0x0C),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.param0 = 0x060C_0608_0602_0600),
            Err(ManifestError::BadAcpiPm(_))
        ));
        assert!(matches!(
            bad(|e| e.param0 = 0x060C_0604_0608_0600),
            Err(ManifestError::BadAcpiPm(_))
        ));
        // The v1.0 manifest still has exactly seven devices and does not accept code 9.
        let mut entries = all_absent();
        entries[6] = good;
        assert_eq!(
            BoardManifest::from_entries(entries),
            Err(ManifestError::UnknownDeviceType(9))
        );
    }
}
