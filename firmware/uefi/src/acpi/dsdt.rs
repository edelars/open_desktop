//! DSDT built from the board manifest (S7-12).
//!
//! The namespace is `\_SB` with one child per device that the manifest marks `PRESENT`:
//! `PCI0` (PCIe root, from the PCIe ECAM entry, type 7) and `COM1` (type 1). Devices that are
//! not described here (RTC, HPET, TPM CRB, `_S5_`) belong to S7-14 and S7-22 and are not
//! emitted; see `docs/specs/ACPI_SMBIOS_TABLES.md`, section "AML and DSDT".

use super::aml::AmlError;
use super::aml::builder::AmlBuilder;
use super::aml::names::{AmlName, NameSeg};
use super::aml::resource::{AddressRange, InterruptFlags, ResourceTemplate, SpaceFlags};
use super::aml::validate::validate_definition_block;
use super::bytes::{head_mut, sub, tail_mut, tri};
use super::manifest::{
    BoardManifest, DeviceEntry, GSI_NONE, IRQ_ACTIVE_LOW, IRQ_LEVEL, device_type,
};
use super::table::{SDT_HEADER_LEN, TableError, finish_checksum, validate_table, write_header};

/// DSDT signature (ACPI Table 5.17).
pub const DSDT_SIGNATURE: [u8; 4] = *b"DSDT";
/// DSDT revision 2: 64-bit integer width (ACPI Table 5.17).
pub const DSDT_REVISION: u8 = 2;
/// OEM Table ID of the DSDT: an own string, 8 bytes.
pub const DSDT_OEM_TABLE_ID: [u8; 8] = *b"ODFWDSDT";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DsdtError {
    Aml(AmlError),
    Table(TableError),
    /// The COM1 entry is `PRESENT` but cannot be described: not a PIO range, a range outside
    /// the 16-bit port space or longer than 255 ports, or `param0` (COM number) outside 1..=9.
    BadCom1,
    /// The PCIe ECAM entry is `PRESENT` but cannot be described: a PIO range, a segment above
    /// 0xFFFF, reserved bits in `param1`, or `start_bus > end_bus`.
    BadPcieEcam,
}

impl From<AmlError> for DsdtError {
    fn from(error: AmlError) -> Self {
        DsdtError::Aml(error)
    }
}

impl From<TableError> for DsdtError {
    fn from(error: TableError) -> Self {
        DsdtError::Table(error)
    }
}

/// Builds the DSDT into `out` and returns its length. The manifest entries are checked first;
/// the result is checked by [`validate_dsdt`] before it is returned.
///
/// A `const fn` (S4-31b0): evaluated at compile time it gives the table as static bytes, called
/// at run time it gives the same bytes.
pub const fn build_dsdt(manifest: &BoardManifest, out: &mut [u8]) -> Result<usize, DsdtError> {
    if out.len() < SDT_HEADER_LEN {
        return Err(DsdtError::Table(TableError::TooShort));
    }
    let pci = match manifest.device(device_type::PCIE_ECAM) {
        Some(ecam) if ecam.is_present() => Some(tri!(PciRoot::from_entry(ecam))),
        _ => None,
    };
    let com = match manifest.device(device_type::COM1) {
        Some(entry) if entry.is_present() => Some(tri!(ComPort::from_entry(entry))),
        _ => None,
    };
    let aml_len = {
        let mut builder = AmlBuilder::new(tail_mut(out, SDT_HEADER_LEN));
        let scope = tri!(builder.begin_scope("\\_SB"), DsdtError::Aml);
        // Order is fixed (PCI0, then COM1) so the output is deterministic.
        if let Some(pci) = &pci {
            tri!(pci.emit(&mut builder), DsdtError::Aml);
        }
        if let Some(com) = &com {
            tri!(com.emit(&mut builder), DsdtError::Aml);
        }
        tri!(builder.end(scope), DsdtError::Aml);
        builder.len()
    };
    let total = SDT_HEADER_LEN + aml_len;
    tri!(
        write_header(
            out,
            DSDT_SIGNATURE,
            DSDT_REVISION,
            DSDT_OEM_TABLE_ID,
            total as u32,
        ),
        DsdtError::Table
    );
    tri!(finish_checksum(head_mut(out, total)), DsdtError::Table);
    tri!(validate_dsdt(head_mut(out, total)));
    Ok(total)
}

/// Validates a DSDT: header signature, `Length` field, checksum, then the AML structure
/// (package bounds, no repeated name within one scope).
pub const fn validate_dsdt(table: &[u8]) -> Result<(), DsdtError> {
    tri!(validate_table(table, &DSDT_SIGNATURE), DsdtError::Table);
    tri!(
        validate_definition_block(sub(table, SDT_HEADER_LEN, table.len())),
        DsdtError::Aml
    );
    Ok(())
}

/// PCIe root bridge `\_SB.PCI0` from the PCIe ECAM entry (FIRMWARE_BOOT.md 6.3, type 7):
/// `param0` = segment, `param1` = `start_bus | end_bus << 8`.
///
/// The manifest v1.0 has no I/O or MMIO window fields for the root bridge and no interrupt
/// routes, so `_CRS` carries only the bus number range and `_PRT` is empty. This is a gap of
/// the contract, recorded in `docs/KNOWN_ISSUES.md`; nothing is invented here.
struct PciRoot {
    segment: u16,
    start_bus: u16,
    crs: ResourceTemplate,
}

impl PciRoot {
    const fn from_entry(ecam: &DeviceEntry) -> Result<Self, DsdtError> {
        if ecam.param0 > u16::MAX as u64 {
            return Err(DsdtError::BadPcieEcam);
        }
        let segment = ecam.param0 as u16;
        if ecam.is_pio() || ecam.param1 >> 16 != 0 {
            return Err(DsdtError::BadPcieEcam);
        }
        let start_bus = (ecam.param1 & 0xFF) as u16;
        let end_bus = ((ecam.param1 >> 8) & 0xFF) as u16;
        if start_bus > end_bus {
            return Err(DsdtError::BadPcieEcam);
        }
        let mut template = ResourceTemplate::new();
        tri!(
            template.word_bus_number(
                SpaceFlags::FIXED,
                AddressRange {
                    granularity: 0,
                    min: start_bus,
                    max: end_bus,
                    translation: 0,
                    length: end_bus - start_bus + 1,
                },
            ),
            DsdtError::Aml
        );
        let crs = tri!(template.finish(), DsdtError::Aml);
        Ok(PciRoot {
            segment,
            start_bus,
            crs,
        })
    }

    const fn emit(&self, b: &mut AmlBuilder<'_>) -> Result<(), AmlError> {
        let device = tri!(b.begin_device("PCI0"));
        tri!(b.name_eisa_id("_HID", "PNP0A08"));
        tri!(b.name_eisa_id("_CID", "PNP0A03"));
        tri!(b.name_integer("_UID", 0));
        tri!(b.name_integer("_SEG", self.segment as u64));
        tri!(b.name_integer("_BBN", self.start_bus as u64));
        tri!(b.name_resource_template("_CRS", &self.crs));
        // Empty: the manifest has no PCI interrupt routes (ACPI 6.2.13 допускает пустой _PRT,
        // маршрутов нет; текст 6.2.13 не сверен). No `_ADR`: a device has `_HID` or `_ADR`.
        tri!(b.begin_name("_PRT"));
        let prt = tri!(b.begin_package());
        tri!(b.end_package(prt));
        b.end(device)
    }
}

/// `COM<n>` from the COM1 entry (type 1): `PNP0501`, `_UID = param0`, `_CRS` with the I/O
/// range and, if `gsi` is not `GSI_NONE`, one Extended Interrupt.
struct ComPort {
    number: u8,
    crs: ResourceTemplate,
}

impl ComPort {
    const fn from_entry(com: &DeviceEntry) -> Result<Self, DsdtError> {
        if com.param0 < 1 || com.param0 > 9 {
            return Err(DsdtError::BadCom1);
        }
        let number = com.param0 as u8;
        if com.base > u16::MAX as u64 {
            return Err(DsdtError::BadCom1);
        }
        let base = com.base as u16;
        if com.length == 0 || com.length > u8::MAX as u64 {
            return Err(DsdtError::BadCom1);
        }
        let length = com.length as u8;
        if !com.is_pio() || base as u32 + length as u32 > 0x1_0000 {
            return Err(DsdtError::BadCom1);
        }
        let interrupt = InterruptFlags {
            edge: com.irq_flags & IRQ_LEVEL == 0,
            active_low: com.irq_flags & IRQ_ACTIVE_LOW != 0,
            shared: false,
            wake: false,
        };
        let mut template = ResourceTemplate::new();
        tri!(template.io(true, base, base, 1, length), DsdtError::Aml);
        if com.gsi != GSI_NONE {
            tri!(
                template.extended_interrupt(true, interrupt, &[com.gsi]),
                DsdtError::Aml
            );
        }
        let crs = tri!(template.finish(), DsdtError::Aml);
        Ok(ComPort { number, crs })
    }

    const fn emit(&self, b: &mut AmlBuilder<'_>) -> Result<(), AmlError> {
        let seg = tri!(NameSeg::from_bytes([b'C', b'O', b'M', b'0' + self.number]));
        let device = tri!(b.begin_device_at(&AmlName::from_seg(seg)));
        tri!(b.name_eisa_id("_HID", "PNP0501"));
        tri!(b.name_integer("_UID", self.number as u64));
        tri!(b.name_resource_template("_CRS", &self.crs));
        b.end(device)
    }
}

#[cfg(test)]
mod const_tests;
#[cfg(test)]
mod tests;
