//! DSDT built from the board manifest (S7-12).
//!
//! The namespace is `\_SB` with the fixed one-CPU profile device `CPU0` (`_HID` `ACPI0007`,
//! integer `_UID` 0) followed by devices that the manifest marks `PRESENT`: `PCI0` (PCIe root,
//! from the PCIe ECAM entry, type 7), `ECAM` (`PNP0C02`, the same entry) and `COM1` (type 1).
//! `CPU0` matches the UID 0 Local APIC record emitted by MADT. Devices that are not described
//! here (RTC, HPET, TPM CRB) belong to S7-14 and S7-22 and are not emitted; `_S5` is added only by
//! `build_dsdt_with_s5` (S4-31e); see `docs/specs/ACPI_SMBIOS_TABLES.md`, section "AML and DSDT".
//!
//! The PCIe ECAM entry (S4-31b) is checked by the same rules as the MCFG serializer
//! (`mcfg::EcamWindow::from_entry`, S4-31a: a memory window of `(end_bus - start_bus + 1) << 20`
//! bytes, non-zero 1 MiB-aligned base, end at most 4 GiB, segment 0, buses 0..=0); a failure is
//! `DsdtError::Ecam`. A `PRESENT` entry gives two devices: `PCI0` with the conf1 ports
//! `0xCF8..=0xCFF` in `_CRS`, and a motherboard-resources device `ECAM` (`PNP0C02`) whose `_CRS`
//! reserves the ECAM window, so Linux accepts the MCFG window as reserved
//! (`pci_mmcfg_late_init`).

use super::aml::AmlError;
use super::aml::builder::AmlBuilder;
use super::aml::names::{AmlName, NameSeg};
use super::aml::resource::{AddressRange, InterruptFlags, ResourceTemplate, SpaceFlags};
use super::aml::validate::validate_definition_block;
use super::bytes::{head_mut, sub, tail_mut, tri};
use super::manifest::{
    BoardManifest, DeviceEntry, GSI_NONE, IRQ_ACTIVE_LOW, IRQ_LEVEL, device_type,
};
use super::mcfg::{ECAM_BYTES_PER_BUS, EcamWindow, McfgError};
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
    /// The PCIe ECAM entry is `PRESENT` but breaks a rule that MCFG enforces too (PIO window,
    /// reserved `param1` bits, segment, inverted or non-board bus range, length, base, above
    /// 4 GiB); the reason is the MCFG error.
    Ecam(McfgError),
    /// `_S5` was asked for with a `SLP_TYP` above 7 (the field has 3 bits).
    SlpTypOutOfRange(u8),
}

/// Conf1 address/data ports of `PCI0`: `0xCF8..=0xCFF` (MACHINE_CONTRACT.md, S4-30c).
pub const CONF1_PORT_BASE: u16 = 0xCF8;
/// Length of the conf1 port range.
pub const CONF1_PORT_LEN: u8 = 8;

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
/// The fixed one-CPU profile always emits `\_SB.CPU0` (`_HID` `ACPI0007`, integer `_UID` 0),
/// matching the UID 0 Local APIC record in MADT. With a `PRESENT` PCIe ECAM entry the table then
/// has `\_SB.PCI0` (`_CRS`: bus number range and
/// `IO(Decode16, 0xCF8, 0xCF8, 1, 8)`), then `\_SB.ECAM` (`_HID` `PNP0C02`, `_UID` 0, `_CRS` =
/// `Memory32Fixed(ReadWrite, base, 0x10_0000)`), then `COM<n>` if present. Without the entry the
/// table has no PCI0 and no ECAM device.
///
/// A `const fn` (S4-31b0): evaluated at compile time it gives the table as static bytes, called
/// at run time it gives the same bytes.
pub const fn build_dsdt(manifest: &BoardManifest, out: &mut [u8]) -> Result<usize, DsdtError> {
    build_dsdt_inner(manifest, None, out)
}

/// [`build_dsdt`] plus the soft-off object `Name(\_S5, Package() { SLP_TYPa, SLP_TYPb, 0, 0 })`
/// at the root, after the `\_SB` scope (S4-31e). `slp_typ_s5` is the `SLP_TYP` value of S5
/// (`AcpiPmEntry::slp_typ_s5`, `ACPI_SLP_TYP_S5` = 5); it is written for both PM1a and PM1b, and
/// the two reserved elements are zero. The bytes of the table without `_S5` (the golden tables of
/// S4-31b) are produced by [`build_dsdt`] unchanged: `_S5` is a separate build variant, not a
/// change of the old one. `slp_typ_s5` above 7 does not fit the 3-bit `SLP_TYP` field.
pub const fn build_dsdt_with_s5(
    manifest: &BoardManifest,
    slp_typ_s5: u8,
    out: &mut [u8],
) -> Result<usize, DsdtError> {
    if slp_typ_s5 > 7 {
        return Err(DsdtError::SlpTypOutOfRange(slp_typ_s5));
    }
    build_dsdt_inner(manifest, Some(slp_typ_s5), out)
}

const fn build_dsdt_inner(
    manifest: &BoardManifest,
    s5: Option<u8>,
    out: &mut [u8],
) -> Result<usize, DsdtError> {
    if out.len() < SDT_HEADER_LEN {
        return Err(DsdtError::Table(TableError::TooShort));
    }
    let (pci, reservation) = match manifest.device(device_type::PCIE_ECAM) {
        Some(ecam) if ecam.is_present() => {
            let window = tri!(EcamWindow::from_entry(ecam), DsdtError::Ecam);
            (
                Some(tri!(PciRoot::from_window(&window))),
                Some(tri!(EcamReservation::from_window(&window))),
            )
        }
        _ => (None, None),
    };
    let com = match manifest.device(device_type::COM1) {
        Some(entry) if entry.is_present() => Some(tri!(ComPort::from_entry(entry))),
        _ => None,
    };
    let aml_len = {
        let mut builder = AmlBuilder::new(tail_mut(out, SDT_HEADER_LEN));
        let scope = tri!(builder.begin_scope("\\_SB"), DsdtError::Aml);
        tri!(emit_cpu0(&mut builder), DsdtError::Aml);
        // Order is fixed (CPU0, PCI0, ECAM reservation, COM1) so the output is deterministic.
        if let Some(pci) = &pci {
            tri!(pci.emit(&mut builder), DsdtError::Aml);
        }
        if let Some(reservation) = &reservation {
            tri!(reservation.emit(&mut builder), DsdtError::Aml);
        }
        if let Some(com) = &com {
            tri!(com.emit(&mut builder), DsdtError::Aml);
        }
        tri!(builder.end(scope), DsdtError::Aml);
        if let Some(typ) = s5 {
            tri!(emit_s5(&mut builder, typ), DsdtError::Aml);
        }
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

/// The stage-4 profile has one processor: Device(CPU0) with ACPI processor HID and integer UID 0.
const fn emit_cpu0(builder: &mut AmlBuilder<'_>) -> Result<(), AmlError> {
    let device = tri!(builder.begin_device("CPU0"));
    tri!(builder.name_string("_HID", "ACPI0007"));
    tri!(builder.name_integer("_UID", 0));
    builder.end(device)
}

/// `Name(\_S5, Package() { typ, typ, 0, 0 })`. With `typ` = 5 the bytes are
/// `08 5C 5F 53 35 5F 12 08 04 0A 05 0A 05 00 00` (`typ` 0 and 1 would use the one-byte ZeroOp and
/// OneOp, the shorter forms of the AML integer encoder). The package holds `SLP_TYPa`, `SLP_TYPb`
/// and then two reserved values; the text of the ACPI `_Sx` section was NOT read for this slice
/// (the layout is the one Linux/ACPICA read: `acpi_hw_get_sleep_type_data`), the bytes are fixed
/// by hand in `dsdt/tests.rs`.
const fn emit_s5(builder: &mut AmlBuilder<'_>, typ: u8) -> Result<(), AmlError> {
    tri!(builder.begin_name("\\_S5"));
    let package = tri!(builder.begin_package());
    tri!(builder.integer(typ as u64));
    tri!(builder.integer(typ as u64));
    tri!(builder.integer(0));
    tri!(builder.integer(0));
    builder.end_package(package)
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

/// PCIe root bridge `\_SB.PCI0` from the checked ECAM window (FIRMWARE_BOOT.md 6.3, type 7):
/// segment, `start_bus`, `end_bus`.
///
/// The manifest v1.0 has no I/O or MMIO window fields for the root bridge and no interrupt
/// routes, so `_CRS` carries the bus number range and the conf1 ports `0xCF8..=0xCFF` (S4-31b;
/// the ports are board facts of S4-30c, not manifest fields) and `_PRT` is empty. There are no
/// bridge apertures: this is a gap of the contract, recorded in `docs/KNOWN_ISSUES.md`; nothing
/// is invented here.
struct PciRoot {
    segment: u16,
    start_bus: u16,
    crs: ResourceTemplate,
}

impl PciRoot {
    const fn from_window(window: &EcamWindow) -> Result<Self, DsdtError> {
        let segment = window.segment;
        let start_bus = window.start_bus as u16;
        let end_bus = window.end_bus as u16;
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
        tri!(
            template.io(true, CONF1_PORT_BASE, CONF1_PORT_BASE, 1, CONF1_PORT_LEN),
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

/// Motherboard resources `\_SB.ECAM` (S4-31b): `_HID` `PNP0C02`, `_UID` 0 and a `_CRS` with the
/// ECAM window as `Memory32Fixed(ReadWrite, base, length)`. Linux (`pci_mmcfg_late_init` ->
/// `is_acpi_reserved`) accepts the MCFG window as reserved when a `PNP0C02` device covers it.
struct EcamReservation {
    crs: ResourceTemplate,
}

impl EcamReservation {
    /// The window is already checked by MCFG rules, so `base + length <= 4 GiB` and both fit
    /// 32 bits.
    const fn from_window(window: &EcamWindow) -> Result<Self, DsdtError> {
        let length = (window.end_bus as u64 - window.start_bus as u64 + 1) * ECAM_BYTES_PER_BUS;
        // Unreachable after the MCFG checks (end <= 4 GiB); kept as a defence, not a new rule.
        if window.base > u32::MAX as u64 || length > u32::MAX as u64 {
            return Err(DsdtError::Ecam(McfgError::WindowAbove4Gib {
                end: u64::MAX,
            }));
        }
        let mut template = ResourceTemplate::new();
        tri!(
            template.memory32_fixed(true, window.base as u32, length as u32),
            DsdtError::Aml
        );
        let crs = tri!(template.finish(), DsdtError::Aml);
        Ok(EcamReservation { crs })
    }

    const fn emit(&self, b: &mut AmlBuilder<'_>) -> Result<(), AmlError> {
        let device = tri!(b.begin_device("ECAM"));
        tri!(b.name_eisa_id("_HID", "PNP0C02"));
        tri!(b.name_integer("_UID", 0));
        tri!(b.name_resource_template("_CRS", &self.crs));
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
