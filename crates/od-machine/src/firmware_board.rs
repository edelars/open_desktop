//! Firmware board v1: ROM backing, shadow copy, handoff block v1.0 and the
//! direct Long64 entry described by `docs/specs/FIRMWARE_BOOT.md`.
//!
//! The machine knows nothing about UEFI, PE or ACPI here: it validates the
//! flat `ODFWIMG1` header, fills ROM, copies the image to the RAM shadow,
//! builds the temporary 4 KiB identity page tables and the handoff block, and
//! hands over a `LongModeState`. It is not a reset vector and not real mode.

use crate::irq_source::{BoardIrqSource, IrqBoundaryHook, IrqCounters, NoIrqSource};
use crate::page_walk::translate_long64_2m_ram_read_only;
use crate::paged_memory_adapter::{
    IrqDeliveryFailure, PagedCpl0SetExit, run_cpl0_set_paged_2m_ram_with_cpuid_boundary,
};
use crate::paged_probe::run_paged_probe_2m_ram;
use crate::pit_ioapic_irq_source::PitIoapicIrqSource;
use crate::pit_pic_irq_source::PitPicIrqSource;
use crate::{
    ACPI_PM_DESCRIPTOR, ACPI_SCI_GSI, ACPI_SCI_IRQ_FLAGS, AbsentPortRegion, AbsentPorts,
    AccessWidth, AcpiPm, AcpiPmHandle, BusAccessError, BusBuildError, DISPLAY_FRAMEBUFFER_BASE,
    DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES,
    DISPLAY_SECOND_FRAMEBUFFER_BASE, DMA_PAGE_0_PORT, DMA_PAGE_0_PORT_BYTES, DisplayDevice,
    DisplayHandle, DmaPage0, GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange,
    IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE,
    LocalApic, LocalApicHandle, Long64ProbeConfig, MachineBus, MachineMemoryMap, MachineStopReason,
    MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, MmioBinding, NVME_MMIO_BASE,
    NVME_MMIO_BYTES, NvmeConfig, NvmeConfigError, NvmeDevice, NvmeHandle, PCI_CONF1_BASE_PORT,
    PCI_CONF1_PORT_BYTES, PCI_ECAM_DESCRIPTOR, PIC_ELCR_BASE_PORT, PIC_ELCR_PORT_BYTES,
    PIC_MASTER_BASE_PORT, PIC_PORT_BYTES, PIC_SLAVE_BASE_PORT, PIT_BASE_PORT, PIT_PORT_B,
    PIT_PORT_B_BYTES, PIT_PORT_BYTES, PagedProbeExit, PagedProbeIntegrationError,
    PagedProbeQuantumReport, PagedProbeRejection, PciEcam, Pic8259Handle, Pic8259Pair,
    Pit8254Devices, Pit8254Handle, PortAddress, PortRange, PortRegionBinding, RamLimits,
    TrackedAccessError, Uart16550Handle, Uart16550PollingTx, VirtualClock, VirtualClockError,
    VirtualClockHandle, run_paged_probe, run_paged_probe_with_rdtsc,
};
use od_isa::execute::{Cpl0ProfileSet, GuestFpState, RunExit, TranslationContextChange};
use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, IretqUserEntry, LongModeState, Register, StiShadow};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Image size, fixed by the contract (FIRMWARE_BOOT.md 3.2).
pub const FIRMWARE_IMAGE_BYTES: usize = 0x0040_0000;
/// ROM window `[0xFFC0_0000, 2^32)` (FIRMWARE_BOOT.md section 2).
pub const FIRMWARE_ROM_BASE: u64 = 0xFFC0_0000;
/// Link base of the image and start of the RAM shadow.
pub const FIRMWARE_LINK_BASE: u64 = 0x0020_0000;
/// Handoff block GPA; also the value of `RDI` at entry.
pub const FIRMWARE_HANDOFF_GPA: u64 = 0x0010_0000;
pub const FIRMWARE_HANDOFF_WINDOW_BYTES: u64 = 0x1_0000;
pub const FIRMWARE_BOOT_STACK_BASE: u64 = 0x0011_0000;
/// Initial `RSP`: the top of the boot stack.
pub const FIRMWARE_BOOT_STACK_TOP: u64 = 0x0015_0000;
/// `CR3` at entry: first page of the temporary page tables.
pub const FIRMWARE_BOOT_PAGE_TABLES_GPA: u64 = 0x0060_0000;
pub const FIRMWARE_MIN_RAM_BYTES: u64 = 128 << 20;
pub const FIRMWARE_MAX_RAM_BYTES: u64 = 0xF_F400_0000;
pub const FIRMWARE_COM1_PORT: u16 = 0x03f8;
pub const FIRMWARE_EFER: u64 = 0x0d00;
/// ACPI table area (S4-31c, `acpi_tables`): `[0xE_0000, 0xE_F000)`, an E820 ACPI
/// reclaim range in S4-37; the RSDP will sit at its first byte.
pub const FIRMWARE_ACPI_TABLES_BASE: u64 = 0x000E_0000;
pub const FIRMWARE_ACPI_TABLES_BYTES: u64 = 0xF000;
/// FACS page `[0xE_F000, 0xF_0000)`, an E820 ACPI NVS range in S4-37.
pub const FIRMWARE_ACPI_FACS_BASE: u64 = 0x000E_F000;
pub const FIRMWARE_ACPI_FACS_BYTES: u64 = 0x1000;

pub(crate) const FIRMWARE_CR0: u64 = 0x8001_0033;
pub(crate) const FIRMWARE_CR4: u64 = 0x20;
pub(crate) const FIRMWARE_LOW_RAM_LIMIT: u64 = 0xF400_0000;
pub(crate) const FIRMWARE_HIGH_RAM_BASE: u64 = 0x1_0000_0000;
const PAGE: u64 = 0x1000;
/// Count frequency of the LAPIC timer under `linux_profile` (S4-37a4): 100 MHz,
/// before the divide configuration. A board choice (as in S4-27i2), not declared
/// in the handoff.
const FIRMWARE_APIC_TIMER_HZ: u64 = 100_000_000;
/// Virtual time one guest `RDTSC` costs on the board under `linux_profile`
/// (S4-37a6b), like a port read of the PM timer or the PIT.
const RDTSC_TICK_NS: u64 = 1_000;
const NX: u64 = 1 << 63;
const IMAGE_HEADER_BYTES: usize = 0x40;
const IMAGE_MAGIC: &[u8; 8] = b"ODFWIMG1";
const IMAGE_CRC_OFFSET: usize = 0x30;
const SHADOW_END: u64 = FIRMWARE_LINK_BASE + FIRMWARE_IMAGE_BYTES as u64;

// Handoff block v1.0 layout (FIRMWARE_BOOT.md section 6).
const HANDOFF_MAGIC: &[u8; 8] = b"ODHANDOF";
const HANDOFF_HEADER_BYTES: usize = 0xA0;
const REGION_ENTRY_BYTES: usize = 32;
const DEVICE_ENTRY_BYTES: usize = 48;
/// Devices of handoff v1.0; the optional NVMe entry of v1.1 follows them.
const BASE_DEVICE_COUNT: usize = 7;
const MAX_REGIONS: usize = 128;
const BOARD_PROFILE: u32 = 1;
const NO_OWNER: u32 = 0xFFFF_FFFF;
const NO_GSI: u32 = 0xFFFF_FFFF;

const REGION_RAM: u32 = 1;
const REGION_RESERVED: u32 = 2;
const REGION_MMIO: u32 = 3;
const REGION_ROM: u32 = 4;
const REGION_FRAMEBUFFER: u32 = 6;
const REGION_HANDOFF: u32 = 7;
const REGION_BOOT_STACK: u32 = 8;
const REGION_FIRMWARE_SHADOW: u32 = 9;
const REGION_BOOT_PAGE_TABLES: u32 = 10;
/// Sub-ranges of low RAM written only with `acpi_tables` (handoff v1.2).
const REGION_ACPI_TABLES: u32 = 11;
const REGION_ACPI_NVS: u32 = 12;

const DEVICE_PRESENT: u32 = 1;
const DEVICE_PIO: u32 = 2;
const DEV_COM1: usize = 0;
const DEV_LAPIC: usize = 1;
const DEV_IOAPIC: usize = 2;
const DEV_DISPLAY: usize = 3;
/// Device-table index and bus index of the optional NVMe controller (device code 8).
const DEV_NVME_TABLE: usize = 7;
const DEVICE_CODE_NVME: u32 = 8;
/// Device-table index of the PCIe ECAM entry (device code 7, a v1.0 slot).
const DEV_ECAM_TABLE: usize = 6;
const DEVICE_CODE_ECAM: u32 = 7;
/// ACPI fixed hardware: PM block and SCI route (S4-31c, handoff v1.2).
const DEVICE_CODE_ACPI_PM: u32 = 9;
/// Handoff minor version emitted when the NVMe entry is present.
const HANDOFF_MINOR_NVME: u16 = 1;
/// Handoff minor version emitted when the ACPI fixed hardware entry (code 9) is present.
const HANDOFF_MINOR_ACPI: u16 = 2;

/// Host-supplied board configuration. Defaults follow FIRMWARE_BOOT.md 6.1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareBoardConfig {
    /// Total RAM `R`: a multiple of 2 MiB in `[128 MiB, 0xF_F400_0000]`.
    pub ram_bytes: u64,
    /// Second display screen (DISPLAY_DEVICE.md 16.3).
    pub second_screen: bool,
    /// Primary display device. Defaults to true for the existing firmware profile;
    /// the S5 NVMe stand profile disables it.
    pub display_enabled: bool,
    pub tsc_hz: u64,
    /// SMBIOS Type 1 UUID bytes, copied verbatim; all zero means "absent".
    pub system_uuid: [u8; 16],
    /// ASCII `0x20..=0x7E`, at most 64 bytes; empty means no serial number.
    pub serial: Vec<u8>,
    /// NVMe controller (S7-16). `None` keeps handoff v1.0 byte for byte; `Some`
    /// adds device code 8 and raises `version_minor` to 1 (FIRMWARE_BOOT.md 6.3).
    pub nvme: Option<NvmeConfig>,
    /// PCI configuration device (S4-30c): ECAM window plus conf1 ports 0xCF8..=0xCFF
    /// over an empty bus 0. `false` keeps handoff v1.0 byte for byte (code 7
    /// `PRESENT = 0`); `true` makes code 7 `PRESENT = 1` (a v1.0 slot, so
    /// `version_minor` and the entry count do not change) and adds the MMIO
    /// region of the window. The board declares no PCI function.
    pub pci_ecam: bool,
    /// ACPI fixed hardware (S7-14a): PM1a event/control, PM timer and reset
    /// register on ports `0x0600..0x0610`. The handoff block does not describe it
    /// (`false` and `true` give the same bytes); `ACPI_PM_DESCRIPTOR` is the
    /// source of the FADT. With the flag `run_boot` stops on an S5 or reset write.
    pub acpi_pm: bool,
    /// ACPI discovery tables manifest (S4-31c). `false` keeps handoff v1.0 (or
    /// v1.1 with NVMe) byte for byte. `true` requires `acpi_pm` and adds device
    /// code 9 (the PM block and the SCI route: GSI 9, level, active-low), splits
    /// low RAM in the region table into RAM, `ACPI_TABLES`
    /// `[0xE_0000, 0xE_F000)`, `ACPI_NVS` `[0xE_F000, 0xF_0000)` and RAM again,
    /// and raises `version_minor` to 2. The memory map is unchanged (the range
    /// is part of the low RAM region); no table bytes are written yet (S4-31d..g).
    pub acpi_tables: bool,
    /// Linux profile (S4-37a1), opt-in. Requires `acpi_pm`, `acpi_tables` and
    /// `pci_ecam` (otherwise `Config`). Binds, on the board clock: the PIT
    /// `0x40..=0x43` and port B `0x61`, the 8259 pair `0x20..=0x21` and
    /// `0xA0..=0xA1`, and the absent-device sinks (read `0xFF`, write ignored)
    /// for CMOS `0x70..=0x71`, `0xED` and `0x80`. The handoff block and the ACPI
    /// manifest hash do not depend on it (`false` and `true` give the same bytes).
    /// Not bound: `0x60/0x64` (a trap), LAPIC shared timer, port tick, IRQ delivery.
    pub linux_profile: bool,
}

impl Default for FirmwareBoardConfig {
    fn default() -> Self {
        Self {
            ram_bytes: FIRMWARE_MIN_RAM_BYTES,
            second_screen: false,
            display_enabled: true,
            tsc_hz: 1_000_000_000,
            system_uuid: [0; 16],
            serial: Vec::new(),
            nvme: None,
            pci_ecam: false,
            acpi_pm: false,
            acpi_tables: false,
            linux_profile: false,
        }
    }
}

impl FirmwareBoardConfig {
    /// Host-side S5 NVMe stand: 128 MiB RAM, COM1, LAPIC/IOAPIC, PM timer block,
    /// and the polling NVMe controller, without PCI, display, vTPM, or network.
    pub fn s5_nvme_stand() -> Self {
        Self {
            display_enabled: false,
            nvme: Some(NvmeConfig::default()),
            acpi_pm: true,
            ..Self::default()
        }
    }

    /// Manifest hash of the ACPI table blob for this board (S4-31d): SHA-256 over the resolved
    /// device table (the entries the handoff block would carry), see
    /// [`crate::acpi_manifest_hash`]. A blob built for another configuration carries another
    /// hash and is refused by [`crate::validate_acpi_blob`]; changing the configuration
    /// therefore requires rebuilding the blob.
    pub fn acpi_manifest_hash(&self) -> Result<crate::AcpiManifestHash, FirmwareBoardError> {
        validate_config(self)?;
        let devices = board_devices(
            self.nvme.is_some(),
            self.pci_ecam,
            self.acpi_tables,
            self.display_enabled,
        );
        let encoded: Vec<[u8; DEVICE_ENTRY_BYTES]> =
            devices.iter().copied().map(DeviceEntry::to_bytes).collect();
        crate::acpi_manifest_hash(&encoded).map_err(|_| {
            FirmwareBoardError::Config("the device table has an invalid or repeated type code")
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum FirmwareBoardError {
    ImageSize {
        expected: usize,
        actual: usize,
    },
    /// A header field differs from the v1 contract (the host never starts the guest).
    ImageHeader {
        field: &'static str,
        value: u64,
    },
    /// A header read ran past the end of the buffer (malformed input, not a panic).
    ImageTruncated {
        offset: usize,
        len: usize,
    },
    ImageCrc {
        stored: u32,
        computed: u32,
    },
    Config(&'static str),
    Nvme(NvmeConfigError),
    BootTablesDoNotFit {
        required_end: u64,
        ram_end: u64,
    },
    TooManyRegions(usize),
    PageTableCountMismatch {
        expected: u64,
        built: u64,
    },
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Bus(BusAccessError),
    Clock(VirtualClockError),
}

impl fmt::Display for FirmwareBoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "firmware board v1 load failed: {self:?}")
    }
}

impl std::error::Error for FirmwareBoardError {}

/// CRC-32 IEEE 802.3, reflected (FIRMWARE_BOOT.md 6.6).
pub(crate) fn crc32_ieee(parts: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 != 0 {
                    (value >> 1) ^ 0xEDB8_8320
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &byte in *part {
            crc = TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
        }
    }
    !crc
}

/// `N` bytes at `offset`, or `ImageTruncated` when the buffer is too short
/// (no panic on a malformed buffer).
fn le_bytes<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], FirmwareBoardError> {
    let truncated = || FirmwareBoardError::ImageTruncated {
        offset,
        len: bytes.len(),
    };
    let end = offset.checked_add(N).ok_or_else(truncated)?;
    bytes
        .get(offset..end)
        .and_then(|slice| <[u8; N]>::try_from(slice).ok())
        .ok_or_else(truncated)
}
fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, FirmwareBoardError> {
    le_bytes(bytes, offset).map(u16::from_le_bytes)
}
fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, FirmwareBoardError> {
    le_bytes(bytes, offset).map(u32::from_le_bytes)
}
fn le_u64(bytes: &[u8], offset: usize) -> Result<u64, FirmwareBoardError> {
    le_bytes(bytes, offset).map(u64::from_le_bytes)
}

/// Validate the `ODFWIMG1` header (FIRMWARE_BOOT.md 3.2) and return `entry_offset`.
fn validate_image(image: &[u8], required_minor: u16) -> Result<u64, FirmwareBoardError> {
    if image.len() != FIRMWARE_IMAGE_BYTES {
        return Err(FirmwareBoardError::ImageSize {
            expected: FIRMWARE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let bad = |field, value| Err(FirmwareBoardError::ImageHeader { field, value });
    if &image[..8] != IMAGE_MAGIC {
        return bad("magic", le_u64(image, 0)?);
    }
    if le_u32(image, 0x08)? != 1 {
        return bad("format_version", u64::from(le_u32(image, 0x08)?));
    }
    if le_u32(image, 0x0C)? as usize != IMAGE_HEADER_BYTES {
        return bad("header_size", u64::from(le_u32(image, 0x0C)?));
    }
    if le_u64(image, 0x10)? != FIRMWARE_IMAGE_BYTES as u64 {
        return bad("image_size", le_u64(image, 0x10)?);
    }
    if le_u64(image, 0x18)? != FIRMWARE_LINK_BASE {
        return bad("link_base", le_u64(image, 0x18)?);
    }
    let entry_offset = le_u64(image, 0x20)?;
    if !(IMAGE_HEADER_BYTES as u64..FIRMWARE_IMAGE_BYTES as u64).contains(&entry_offset) {
        return bad("entry_offset", entry_offset);
    }
    if le_u16(image, 0x28)? != 1 {
        return bad("handoff_major", u64::from(le_u16(image, 0x28)?));
    }
    // The machine emits handoff v1.0 unless the NVMe entry is configured (v1.1),
    // so the image must understand that minor; for 0 every u16 value passes.
    if le_u16(image, 0x2A)? < required_minor {
        return bad("handoff_minor_max", u64::from(le_u16(image, 0x2A)?));
    }
    if le_u32(image, 0x2C)? != 0 {
        return bad("flags", u64::from(le_u32(image, 0x2C)?));
    }
    if image[0x34..IMAGE_HEADER_BYTES]
        .iter()
        .any(|&byte| byte != 0)
    {
        return bad("reserved", 0);
    }
    let stored = le_u32(image, IMAGE_CRC_OFFSET)?;
    let computed = crc32_ieee(&[
        &image[..IMAGE_CRC_OFFSET],
        &[0; 4],
        &image[IMAGE_CRC_OFFSET + 4..],
    ]);
    if stored != computed {
        return Err(FirmwareBoardError::ImageCrc { stored, computed });
    }
    Ok(entry_offset)
}

fn validate_config(config: &FirmwareBoardConfig) -> Result<(), FirmwareBoardError> {
    if config.ram_bytes % (2 << 20) != 0
        || !(FIRMWARE_MIN_RAM_BYTES..=FIRMWARE_MAX_RAM_BYTES).contains(&config.ram_bytes)
    {
        return Err(FirmwareBoardError::Config(
            "ram_bytes must be a multiple of 2 MiB in [128 MiB, 0xF_F400_0000]",
        ));
    }
    if config.tsc_hz == 0 {
        return Err(FirmwareBoardError::Config("tsc_hz must not be zero"));
    }
    if config.second_screen && !config.display_enabled {
        return Err(FirmwareBoardError::Config(
            "second_screen requires display_enabled",
        ));
    }
    if config.acpi_tables && !config.acpi_pm {
        return Err(FirmwareBoardError::Config(
            "acpi_tables describes the PM block and needs acpi_pm",
        ));
    }
    if config.linux_profile && !(config.acpi_pm && config.acpi_tables && config.pci_ecam) {
        return Err(FirmwareBoardError::Config(
            "linux_profile needs acpi_pm, acpi_tables and pci_ecam",
        ));
    }
    if config.serial.len() > 64 || config.serial.iter().any(|b| !(0x20..=0x7e).contains(b)) {
        return Err(FirmwareBoardError::Config(
            "serial must be at most 64 ASCII 0x20..0x7E bytes",
        ));
    }
    Ok(())
}

/// One entry of the device table. The same list builds the bus bindings, the
/// MMIO windows of the memory map and the handoff block (S7-03, item 2).
#[derive(Clone, Copy, Debug)]
struct DeviceEntry {
    code: u32,
    flags: u32,
    base: u64,
    length: u64,
    gsi: u32,
    irq_flags: u32,
    param0: u64,
    param1: u64,
}

impl DeviceEntry {
    const fn present(&self) -> bool {
        self.flags & DEVICE_PRESENT != 0
    }
    const fn is_mmio(&self) -> bool {
        self.present() && self.flags & DEVICE_PIO == 0
    }
    /// The 48-byte handoff encoding of the entry (FIRMWARE_BOOT.md 6.3).
    fn to_bytes(self) -> [u8; DEVICE_ENTRY_BYTES] {
        let mut raw = [0u8; DEVICE_ENTRY_BYTES];
        raw[0x00..0x04].copy_from_slice(&self.code.to_le_bytes());
        raw[0x04..0x08].copy_from_slice(&self.flags.to_le_bytes());
        raw[0x08..0x10].copy_from_slice(&self.base.to_le_bytes());
        raw[0x10..0x18].copy_from_slice(&self.length.to_le_bytes());
        raw[0x18..0x1C].copy_from_slice(&self.gsi.to_le_bytes());
        raw[0x1C..0x20].copy_from_slice(&self.irq_flags.to_le_bytes());
        raw[0x20..0x28].copy_from_slice(&self.param0.to_le_bytes());
        raw[0x28..0x30].copy_from_slice(&self.param1.to_le_bytes());
        raw
    }
    const fn absent(code: u32) -> Self {
        Self {
            code,
            flags: 0,
            base: 0,
            length: 0,
            gsi: NO_GSI,
            irq_flags: 0,
            param0: 0,
            param1: 0,
        }
    }
}

/// Device table of firmware board v1 (FIRMWARE_BOOT.md 6.3). NVRAM and TPM CRB
/// are `PRESENT = 0` until S7-10 and S7-22 add them; ECAM is `PRESENT = 0` unless
/// `pci_ecam` is set (S4-30c). With `nvme` the table gets the eighth entry,
/// device code 8 (FIRMWARE_BOOT.md 6.3); with `acpi_tables` the last entry is code 9.
fn board_devices(nvme: bool, pci_ecam: bool, acpi_tables: bool, display: bool) -> Vec<DeviceEntry> {
    let mmio = |code, base, length| DeviceEntry {
        code,
        flags: DEVICE_PRESENT,
        base,
        length,
        gsi: NO_GSI,
        irq_flags: 0,
        param0: 0,
        param1: 0,
    };
    let mut devices = vec![
        DeviceEntry {
            code: 1,
            flags: DEVICE_PRESENT | DEVICE_PIO,
            base: u64::from(FIRMWARE_COM1_PORT),
            length: 8,
            gsi: 4,
            irq_flags: 0,
            param0: 1,
            param1: 0,
        },
        mmio(2, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE),
        mmio(3, IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE),
        if display {
            mmio(4, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES)
        } else {
            DeviceEntry::absent(4)
        },
        DeviceEntry::absent(5),
        DeviceEntry::absent(6),
        DeviceEntry::absent(DEVICE_CODE_ECAM),
    ];
    debug_assert_eq!(devices.len(), BASE_DEVICE_COUNT);
    if pci_ecam {
        let ecam = PCI_ECAM_DESCRIPTOR;
        devices[DEV_ECAM_TABLE] = DeviceEntry {
            param0: u64::from(ecam.segment),
            param1: ecam.handoff_param1(),
            ..mmio(DEVICE_CODE_ECAM, ecam.base, ecam.size)
        };
    }
    if nvme {
        // Polling only: no IRQ route in slice A (`gsi = 0xFFFF_FFFF`).
        devices.push(mmio(DEVICE_CODE_NVME, NVME_MMIO_BASE, NVME_MMIO_BYTES));
    }
    if acpi_tables {
        // PIO block with the SCI route; the entry follows NVMe when both exist.
        let pm = ACPI_PM_DESCRIPTOR;
        devices.push(DeviceEntry {
            code: DEVICE_CODE_ACPI_PM,
            flags: DEVICE_PRESENT | DEVICE_PIO,
            base: u64::from(pm.base_port),
            length: u64::from(pm.port_bytes),
            gsi: ACPI_SCI_GSI,
            irq_flags: ACPI_SCI_IRQ_FLAGS,
            param0: pm.handoff_param0(),
            param1: pm.handoff_param1(),
        });
    }
    devices
}

fn configured_board_devices(config: &FirmwareBoardConfig) -> Vec<DeviceEntry> {
    board_devices(
        config.nvme.is_some(),
        config.pci_ecam,
        config.acpi_tables,
        config.display_enabled,
    )
}

/// `version_minor` of the handoff block: 2 with the ACPI entry (code 9), else 1
/// with the NVMe entry (code 8), else 0 (FIRMWARE_BOOT.md 6.4).
fn handoff_minor(devices: &[DeviceEntry]) -> u16 {
    if devices.iter().any(|d| d.code == DEVICE_CODE_ACPI_PM) {
        HANDOFF_MINOR_ACPI
    } else if devices.iter().any(|d| d.code == DEVICE_CODE_NVME) {
        HANDOFF_MINOR_NVME
    } else {
        0
    }
}

fn range(start: u64, len: u64) -> Result<GuestPhysicalRange, FirmwareBoardError> {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len)
        .map_err(FirmwareBoardError::Memory)
}

fn board_map(
    config: &FirmwareBoardConfig,
    devices: &[DeviceEntry],
) -> Result<MachineMemoryMap, FirmwareBoardError> {
    let low_end = config.ram_bytes.min(FIRMWARE_LOW_RAM_LIMIT);
    let high = config.ram_bytes - low_end;
    let mut regions = vec![MemoryRegionSpec::new(
        range(0, low_end)?,
        MemoryRegionKind::Ram,
    )];
    if high > 0 {
        regions.push(MemoryRegionSpec::new(
            range(FIRMWARE_HIGH_RAM_BASE, high)?,
            MemoryRegionKind::Ram,
        ));
    }
    regions.push(MemoryRegionSpec::new(
        range(FIRMWARE_ROM_BASE, FIRMWARE_IMAGE_BYTES as u64)?,
        MemoryRegionKind::Rom,
    ));
    if config.display_enabled {
        regions.push(MemoryRegionSpec::new(
            range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES)?,
            MemoryRegionKind::Framebuffer,
        ));
        if config.second_screen {
            regions.push(MemoryRegionSpec::new(
                range(DISPLAY_SECOND_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES)?,
                MemoryRegionKind::Framebuffer,
            ));
        }
    }
    for device in devices.iter().filter(|device| device.is_mmio()) {
        regions.push(MemoryRegionSpec::new(
            range(device.base, device.length)?,
            MemoryRegionKind::Mmio,
        ));
    }
    MachineMemoryMap::new(36, regions).map_err(FirmwareBoardError::Memory)
}

fn is_mapped(kind: MemoryRegionKind) -> bool {
    kind != MemoryRegionKind::Reserved
}

/// `N` of FIRMWARE_BOOT.md section 4: distinct 2 MiB blocks (PT pages), 1 GiB
/// blocks (PD), 512 GiB blocks (PDPT) touched by mapped regions, plus the PML4.
fn boot_page_table_count(map: &MachineMemoryMap) -> u64 {
    let (mut pt, mut pd, mut pdpt) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for region in map.regions().iter().filter(|region| is_mapped(region.kind)) {
        let first = region.range.start().get();
        let last = first + region.range.len() - 1;
        pt.extend(first >> 21..=last >> 21);
        pd.extend(first >> 30..=last >> 30);
        pdpt.extend(first >> 39..=last >> 39);
    }
    (pt.len() + pd.len() + pdpt.len()) as u64 + 1
}

/// Temporary identity tables, 4 KiB leaves only (FIRMWARE_BOOT.md section 4).
/// Pages are allocated in order of first use over the address-sorted regions;
/// the PML4 is page 0.
fn build_boot_tables(
    map: &MachineMemoryMap,
    root: u64,
    expected_pages: u64,
) -> Result<Vec<u8>, FirmwareBoardError> {
    let mut entries: Vec<u64> = vec![0; 512];
    let mut pdpt: BTreeMap<u64, usize> = BTreeMap::new();
    let mut pd: BTreeMap<u64, usize> = BTreeMap::new();
    let mut pt: BTreeMap<u64, usize> = BTreeMap::new();
    let table_address = |page: usize| root + page as u64 * PAGE;
    fn allocate(entries: &mut Vec<u64>) -> usize {
        entries.resize(entries.len() + 512, 0);
        entries.len() / 512 - 1
    }
    for region in map.regions().iter().filter(|region| is_mapped(region.kind)) {
        let leaf_flags = match region.kind {
            MemoryRegionKind::Ram => 0x3,
            MemoryRegionKind::Rom => 0x1 | NX,
            _ => 0x3 | NX,
        };
        let start = region.range.start().get();
        let end = start + region.range.len();
        for address in (start..end).step_by(PAGE as usize) {
            let pml4_index = ((address >> 39) & 511) as usize;
            let pdpt_index = ((address >> 30) & 511) as usize;
            let pd_index = ((address >> 21) & 511) as usize;
            let pt_index = ((address >> 12) & 511) as usize;
            let pdpt_page = *pdpt.entry(address >> 39).or_insert_with(|| {
                let page = allocate(&mut entries);
                entries[pml4_index] = table_address(page) | 0x3;
                page
            });
            let pd_page = *pd.entry(address >> 30).or_insert_with(|| {
                let page = allocate(&mut entries);
                entries[pdpt_page * 512 + pdpt_index] = table_address(page) | 0x3;
                page
            });
            let pt_page = *pt.entry(address >> 21).or_insert_with(|| {
                let page = allocate(&mut entries);
                entries[pd_page * 512 + pd_index] = table_address(page) | 0x3;
                page
            });
            entries[pt_page * 512 + pt_index] = address | leaf_flags;
        }
    }
    let built = (entries.len() / 512) as u64;
    if built != expected_pages {
        return Err(FirmwareBoardError::PageTableCountMismatch {
            expected: expected_pages,
            built,
        });
    }
    Ok(entries
        .iter()
        .flat_map(|entry| entry.to_le_bytes())
        .collect())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HandoffRegion {
    kind: u32,
    attrs: u32,
    base: u64,
    length: u64,
    owner: u32,
}

/// Handoff region table: every region of the map, with the lowest RAM region
/// split by the boot reservations of FIRMWARE_BOOT.md section 2.
/// The boot page tables region is fixed at `FIRMWARE_BOOT_PAGE_TABLES_GPA`, which must equal
/// `SHADOW_END` (checked in `assemble_board`).
fn handoff_regions(
    map: &MachineMemoryMap,
    devices: &[DeviceEntry],
    page_table_bytes: u64,
) -> Vec<HandoffRegion> {
    let owner_of = |base: u64| {
        devices
            .iter()
            .position(|device| device.is_mmio() && device.base == base)
            .map_or(NO_OWNER, |index| index as u32)
    };
    let mut table = Vec::new();
    for region in map.regions() {
        let base = region.range.start().get();
        let length = region.range.len();
        let simple = |kind, attrs, owner| HandoffRegion {
            kind,
            attrs,
            base,
            length,
            owner,
        };
        match region.kind {
            MemoryRegionKind::Ram if base == 0 => {
                let tables_end = FIRMWARE_BOOT_PAGE_TABLES_GPA + page_table_bytes;
                // Code 9 present: the ACPI table area and the FACS page are cut
                // out of low RAM below the handoff block (S4-31c).
                let acpi = devices.iter().any(|d| d.code == DEVICE_CODE_ACPI_PM);
                let facs_end = FIRMWARE_ACPI_FACS_BASE + FIRMWARE_ACPI_FACS_BYTES;
                let mut pieces = Vec::new();
                if acpi {
                    pieces.extend([
                        (0, FIRMWARE_ACPI_TABLES_BASE, REGION_RAM),
                        (
                            FIRMWARE_ACPI_TABLES_BASE,
                            FIRMWARE_ACPI_FACS_BASE,
                            REGION_ACPI_TABLES,
                        ),
                        (FIRMWARE_ACPI_FACS_BASE, facs_end, REGION_ACPI_NVS),
                        (facs_end, FIRMWARE_HANDOFF_GPA, REGION_RAM),
                    ]);
                } else {
                    pieces.push((0, FIRMWARE_HANDOFF_GPA, REGION_RAM));
                }
                pieces.extend([
                    (
                        FIRMWARE_HANDOFF_GPA,
                        FIRMWARE_BOOT_STACK_BASE,
                        REGION_HANDOFF,
                    ),
                    (
                        FIRMWARE_BOOT_STACK_BASE,
                        FIRMWARE_BOOT_STACK_TOP,
                        REGION_BOOT_STACK,
                    ),
                    (FIRMWARE_BOOT_STACK_TOP, FIRMWARE_LINK_BASE, REGION_RAM),
                    (FIRMWARE_LINK_BASE, SHADOW_END, REGION_FIRMWARE_SHADOW),
                    (
                        FIRMWARE_BOOT_PAGE_TABLES_GPA,
                        tables_end,
                        REGION_BOOT_PAGE_TABLES,
                    ),
                    (tables_end, length, REGION_RAM),
                ]);
                for (start, end, kind) in pieces {
                    if end > start {
                        table.push(HandoffRegion {
                            kind,
                            attrs: 0,
                            base: start,
                            length: end - start,
                            owner: NO_OWNER,
                        });
                    }
                }
            }
            MemoryRegionKind::Ram => table.push(simple(REGION_RAM, 0, NO_OWNER)),
            MemoryRegionKind::Reserved => table.push(simple(REGION_RESERVED, 0, NO_OWNER)),
            MemoryRegionKind::Mmio => table.push(simple(REGION_MMIO, 0, owner_of(base))),
            MemoryRegionKind::Rom => table.push(simple(REGION_ROM, 0, NO_OWNER)),
            MemoryRegionKind::Framebuffer => {
                let screen = u32::from(base == DISPLAY_SECOND_FRAMEBUFFER_BASE);
                table.push(simple(REGION_FRAMEBUFFER, screen, DEV_DISPLAY as u32));
            }
        }
    }
    table
}

fn build_handoff(
    config: &FirmwareBoardConfig,
    devices: &[DeviceEntry],
    regions: &[HandoffRegion],
) -> Vec<u8> {
    let region_table_offset = HANDOFF_HEADER_BYTES;
    let device_table_offset = region_table_offset + REGION_ENTRY_BYTES * regions.len();
    let total_size = device_table_offset + DEVICE_ENTRY_BYTES * devices.len();
    let mut block = vec![0u8; total_size];
    let put16 = |block: &mut [u8], offset: usize, value: u16| {
        block[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    };
    let put32 = |block: &mut [u8], offset: usize, value: u32| {
        block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    let put64 = |block: &mut [u8], offset: usize, value: u64| {
        block[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    };
    block[..8].copy_from_slice(HANDOFF_MAGIC);
    put16(&mut block, 0x08, 1);
    put16(&mut block, 0x0A, handoff_minor(devices));
    put32(&mut block, 0x0C, HANDOFF_HEADER_BYTES as u32);
    put32(&mut block, 0x10, total_size as u32);
    // 0x14 crc32 is filled last.
    put32(&mut block, 0x1C, 1);
    put64(&mut block, 0x20, config.tsc_hz);
    put32(&mut block, 0x28, BOARD_PROFILE);
    put32(&mut block, 0x30, region_table_offset as u32);
    put32(&mut block, 0x34, regions.len() as u32);
    put32(&mut block, 0x38, REGION_ENTRY_BYTES as u32);
    put32(&mut block, 0x3C, device_table_offset as u32);
    put32(&mut block, 0x40, devices.len() as u32);
    put32(&mut block, 0x44, DEVICE_ENTRY_BYTES as u32);
    block[0x48..0x58].copy_from_slice(&config.system_uuid);
    put16(&mut block, 0x58, config.serial.len() as u16);
    block[0x60..0x60 + config.serial.len()].copy_from_slice(&config.serial);
    for (index, region) in regions.iter().enumerate() {
        let at = region_table_offset + index * REGION_ENTRY_BYTES;
        put32(&mut block, at, region.kind);
        put32(&mut block, at + 0x04, region.attrs);
        put64(&mut block, at + 0x08, region.base);
        put64(&mut block, at + 0x10, region.length);
        put32(&mut block, at + 0x18, region.owner);
    }
    for (index, device) in devices.iter().enumerate() {
        let at = device_table_offset + index * DEVICE_ENTRY_BYTES;
        // The same encoder the manifest hash uses (`to_bytes`), so the two cannot drift apart.
        block[at..at + DEVICE_ENTRY_BYTES].copy_from_slice(&device.to_bytes());
    }
    let crc = crc32_ieee(&[&block]);
    put32(&mut block, 0x14, crc);
    block
}

/// A failed run of the board loop (S4-38L1): the error and the instructions retired before it.
#[derive(Debug)]
pub(crate) struct LinuxRunFailure {
    pub(crate) error: FirmwareBoardError,
    /// Instructions of the quanta that had executed when the error happened (including the
    /// quantum whose clock advance failed).
    pub(crate) retired: u64,
}

/// Result of [`LoadedFirmwareBoard::run_linux_boot_cpl0_set`] (S4-38L3).
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct LinuxSetRunReport {
    /// The usual run report; `exit` is the set's exit in `PagedProbeExit` terms.
    pub(crate) run: FirmwareRunReport,
    /// Every retired `MOV CR3`, in retirement order; a `MOV CR3` that ended the run in an
    /// integration error is the last entry (see `PagedCpl0SetReport::translation_context_changes`).
    pub(crate) translation_context_changes: Vec<TranslationContextChange>,
    /// The `Debug` text of the set's own final exit (`PagedCpl0SetExit`), before it was
    /// expressed as a `PagedProbeExit`.
    pub(crate) set_exit_detail: String,
    /// The leaf (EAX) of the `CPUID` the run stopped before (S4-38L4, `cpuid_divergence`): the
    /// instruction did not execute. `None` for every other exit.
    pub(crate) cpuid_divergence_leaf: Option<u32>,
    /// The validated frame of the `IRETQ` the run stopped at (S4-38L9I, `user_entry`): a return to
    /// CPL3 the set does not execute. The instruction did not retire: RIP is on it. `None` for
    /// every other exit.
    pub(crate) user_entry: Option<IretqUserEntry>,
    /// Instructions retired through the CPL3 entry in this call (S4-39c, `with_user_mode`); part
    /// of `run.retired`. `0` for a set without the member.
    pub(crate) cpl3_retired: u64,
    /// The system calls of this call (S4-39f, `with_user_mode`): `SYSCALL` entries and `SYSRETQ`
    /// returns, as a ring of the last entries. Empty for a set without the member.
    pub(crate) syscall_journal: crate::linux_direct::SyscallJournal,
    /// `#PF` exceptions delivered to the guest through its IDT in this call (S4-38L5c); a
    /// delivery is not a retired instruction.
    pub(crate) exceptions_delivered: u64,
    /// What the interrupt hook counted in this call (S4-38L8b); all zero without a source or
    /// without the hook.
    pub(crate) irq: IrqCounters,
}

/// Whether the CPL0 set loop calls the set again after a quantum that ended on `exit` (S4-38L4b).
///
/// Both resumable exits need the run to be neither stopped by the guest (`stop_requested`) nor
/// out of budget (`remaining == 0`):
/// - a `StopRequested` of a tick flag, once the quantum retired something (the flag is consumed
///   between quanta, so a quantum that retired nothing would only repeat itself);
/// - the partial REP exit `RepIterationBudgetExhausted`: the iteration limit of one REP step is
///   a quantum boundary like a tick. RIP is still on the REP instruction and RCX/RSI/RDI carry
///   the iterations done, so the next call continues the same instruction. It retires nothing
///   and costs no budget (budget unit: the instruction; the REP is counted once, when it
///   completes), and it cannot repeat forever: every call completes at least one iteration,
///   which lowers the count.
fn resumes_set_exit(
    exit: &PagedCpl0SetExit,
    stop_requested: bool,
    remaining: u64,
    instructions_executed: u64,
) -> bool {
    !stop_requested
        && remaining > 0
        && match exit {
            PagedCpl0SetExit::Run(RunExit::StopRequested) => instructions_executed > 0,
            PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted) => true,
            _ => false,
        }
}

/// Whether the CPL0 set loop calls the set again after a quantum the board cut to the deadline
/// of the interrupt source (S4-38L8b): the quantum ran its shortened budget out
/// (`InstructionBudgetExhausted`) after retiring something, the run was not stopped by the guest
/// and the run budget is not spent.
fn resumes_deadline_cut(
    exit: &PagedCpl0SetExit,
    cut_by_deadline: bool,
    stop_requested: bool,
    remaining: u64,
    instructions_executed: u64,
) -> bool {
    cut_by_deadline
        && !stop_requested
        && remaining > 0
        && instructions_executed > 0
        && matches!(
            exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted)
        )
}

/// Expresses the final exit of the CPL0 set loop as a `PagedProbeExit`, together with its own
/// `Debug` text. `StopAddress` has no counterpart: the loop passes no stop address, so it is an
/// integration error of the board, reported as such instead of being folded into another exit.
///
/// `RepIterationBudgetExhausted` (S4-38L4b) is the set's partial REP exit. The loop resumes it
/// ([`resumes_set_exit`]); it reaches this function only when the run ends on it anyway (a stop
/// the guest asked for, or no budget left) and is then expressed like the quantum stop
/// (`Stopped`), which the loop turns into `BudgetExhausted` once the budget is spent.
fn set_exit_to_probe_exit(
    exit: PagedCpl0SetExit,
) -> Result<(PagedProbeExit, String), &'static str> {
    let detail = format!("{exit:?}");
    let mapped = match exit {
        PagedCpl0SetExit::Run(run) => match run {
            RunExit::Halted => PagedProbeExit::Halted,
            RunExit::InstructionBudgetExhausted => PagedProbeExit::BudgetExhausted,
            RunExit::StopRequested | RunExit::RepIterationBudgetExhausted => {
                PagedProbeExit::Stopped
            }
            RunExit::Fault(fault) => PagedProbeExit::Fault(fault),
            RunExit::Unsupported { opcode } => PagedProbeExit::Unsupported { opcode },
            RunExit::MemoryError(error) => PagedProbeExit::Backend(error),
            RunExit::StopAddress => {
                return Err("the CPL0 set returned an exit the board never asks for");
            }
        },
        PagedCpl0SetExit::Rejected(error) => {
            PagedProbeExit::Rejected(PagedProbeRejection::Cpl0Set(error))
        }
        PagedCpl0SetExit::Configuration(error) => {
            PagedProbeExit::Rejected(PagedProbeRejection::Configuration(error))
        }
        PagedCpl0SetExit::Cr3EffectMismatch {
            active_cr3,
            effect_old_cr3,
            cpu_cr3,
            effect_new_cr3,
        } => PagedProbeExit::IntegrationError(PagedProbeIntegrationError::Cr3EffectMismatch {
            active_cr3,
            effect_old_cr3,
            cpu_cr3,
            effect_new_cr3,
        }),
        PagedCpl0SetExit::Cr3SnapshotRejected { effect, error } => {
            PagedProbeExit::IntegrationError(PagedProbeIntegrationError::Cr3SnapshotRejected {
                effect,
                error,
            })
        }
        PagedCpl0SetExit::UnexpectedContextChange { effect } => {
            PagedProbeExit::IntegrationError(PagedProbeIntegrationError::UnexpectedContextChange {
                effect,
            })
        }
        PagedCpl0SetExit::PortIo(error) => PagedProbeExit::PortIo(error),
        // S4-38L5c: the guest `#PF` stays the stop, as it was before delivery existed; the
        // reason it was not delivered is in the `Debug` text of the set exit (`detail`).
        PagedCpl0SetExit::ExceptionDeliveryFailed { fault, .. } => PagedProbeExit::Fault(fault),
        // A stop of the board, not an exit of the CPU: `Stopped`, told apart by the report's
        // `cpuid_divergence_leaf` (the exit enum gets no new variant).
        PagedCpl0SetExit::CpuidDivergence { .. } => PagedProbeExit::Stopped,
        // S4-38L9I: likewise a stop of the board (the set never runs CPL3 code), told apart by the
        // report's `user_entry` frame.
        PagedCpl0SetExit::UserEntry(_) => PagedProbeExit::Stopped,
        PagedCpl0SetExit::UnknownSetExit => {
            return Err("the CPL0 set returned an exit variant the board does not know");
        }
        // S4-38L8b: an interrupt of the board source that could not be delivered is an error of
        // the run, never a quiet stop; the vector and the reason are in the `Debug` text of the
        // set exit.
        PagedCpl0SetExit::IrqDeliveryFailed { failure, .. } => {
            return Err(match failure {
                IrqDeliveryFailure::Refused(_) => {
                    "an external interrupt of the board source was refused by the CPU delivery"
                }
                IrqDeliveryFailure::Aborted { .. } => {
                    "an external interrupt of the board source aborted on a memory access"
                }
            });
        }
    };
    Ok((mapped, detail))
}

/// Result of [`LoadedFirmwareBoard::run_boot`].
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareRunReport {
    pub exit: PagedProbeExit,
    pub page_fault: Option<PageFaultRecord>,
    pub memory_error: Option<TrackedAccessError>,
    pub retired: u64,
    pub quanta: u64,
    /// Bytes the guest transmitted through COM1, in order.
    pub com1: Vec<u8>,
    pub virtual_ns: u64,
    pub tsc_hz: u64,
    /// Set when the guest asked the PM block to stop the machine (S5 or reset,
    /// S7-14a); `exit` is then `Stopped`. Always `None` without `acpi_pm`.
    pub stop_reason: Option<MachineStopReason>,
}

/// A loaded firmware board: machine, direct-entry CPU state and handoff block.
pub struct LoadedFirmwareBoard {
    bus: MachineBus,
    state: LongModeState,
    uart: Uart16550Handle,
    display: DisplayHandle,
    nvme: Option<NvmeHandle>,
    acpi_pm: Option<AcpiPmHandle>,
    /// The PIT observer of the Linux profile (S4-37a3); `None` without the flag.
    pit: Option<Pit8254Handle>,
    /// Owner/host handle of the LAPIC (the S4-38 pump becomes its owner); it has
    /// a timer on the board clock only under the Linux profile (S4-37a4).
    lapic: LocalApicHandle,
    /// Shared with the PM timer; this board is the only advancer.
    clock: VirtualClockHandle,
    handoff: Vec<u8>,
    page_table_bytes: u64,
    /// `CR3` the paged interpreter walks from (S4-37L4): the board's page-table
    /// root, `FIRMWARE_BOOT_PAGE_TABLES_GPA` for the firmware boot.
    page_table_root: u64,
    /// The whole CR2 the guest left at the end of the latest `run_linux_boot_cpl0_set`
    /// (S4-38L5c): the faulting address of the last delivered `#PF`, or any other value the CR2
    /// took (a guest write of CR2, should the set ever model it). Fed back as CR2 on entry the way
    /// `page_table_root` is for CR3. `None` until the first CPL0-set run.
    guest_cr2: Option<u64>,
    /// The guest-visible `IA32_EFER` (S4-38L7f): `FIRMWARE_EFER` until a guest `WRMSR` of EFER
    /// changed `SCE` (the only bit the set lets change). The walkers keep using the constant
    /// [`FIRMWARE_EFER`] (LME/LMA/NXE never change), so [`Self::efer`] stays the walker's EFER.
    guest_efer: u64,
    /// The guest PCI CONFIG_ADDRESS register (port `0xCF8`, S4-38L7w), kept between CPL0-set runs.
    /// There is no device behind CONFIG_DATA; the value is only stored and read back.
    pci_config_address: u32,
    /// The guest `IA32_STAR` (S4-38L7ao): `0` until a guest `WRMSR` of it through the CPL0 set.
    /// Only stored; nothing consumes it (`SYSCALL`/`SYSRET` are not modelled).
    guest_star: u64,
    /// The guest `IA32_LSTAR` (S4-38L7ap): `0` until a guest `WRMSR` of it through the CPL0 set.
    /// Only stored; nothing consumes it (`SYSCALL`/`SYSRET` are not modelled).
    guest_lstar: u64,
    /// The guest `IA32_CSTAR` (S4-38L7aq): `0` until a guest `WRMSR` of it through the CPL0 set.
    /// Only stored; nothing consumes it (`SYSCALL`/`SYSRET` are not modelled).
    guest_cstar: u64,
    /// The guest `IA32_SYSENTER_CS` (S4-38L7ar): `0` until a guest `WRMSR` of it through the CPL0 set.
    /// The full 64-bit value written (the real register holds a 16-bit selector; not modelled as
    /// such). Only stored; nothing consumes it (`SYSENTER`/`SYSEXIT` are not modelled).
    guest_sysenter_cs: u64,
    /// The guest `IA32_SYSENTER_ESP` (S4-38L7as): `0` until a guest `WRMSR` of it through the CPL0
    /// set. The full 64-bit value written. Only stored; nothing consumes it.
    guest_sysenter_esp: u64,
    /// The guest `IA32_SYSENTER_EIP` (S4-38L7as): `0` until a guest `WRMSR` of it through the CPL0
    /// set. The full 64-bit value written. Only stored; nothing consumes it.
    guest_sysenter_eip: u64,
    /// The guest `IA32_SYSCALL_MASK` (S4-38L7at): `0` until a guest `WRMSR` of it through the CPL0
    /// set. The full 64-bit value written (the flag-clearing behaviour of the real FMASK is not
    /// modelled). Only stored; nothing consumes it.
    guest_syscall_mask: u64,
    /// The guest `IA32_KERNEL_GS_BASE` (S4-38L7au): `0` until a guest `WRMSR` of it through the
    /// CPL0 set. The full 64-bit value written (no canonical check; `SWAPGS` is not modelled).
    /// Only stored; nothing consumes it.
    guest_kernel_gs_base: u64,
    /// The guest FP state of the CPL0 set's FP member (S4-38L9d): x87 environment and registers,
    /// MXCSR and XMM. `GuestFpState::initial()` (FCW `0x037F`, MXCSR `0x1F80`, everything else
    /// zero) until a run with the member changes it; carried between runs like the MSR cells.
    guest_fp: GuestFpState,
    /// COM1 bytes recorded by the latest `run_boot`, kept even when it fails.
    com1_log: Vec<u8>,
    /// Pending-tick flag of the guest `RDTSC` (S4-37a6b); `Some` only under
    /// `linux_profile`, where `run` hands the CPU a time source. The source sets
    /// it, the `run_boot` stop predicate reads it and `run_boot` clears it.
    rdtsc_tick: Option<Arc<AtomicBool>>,
    /// The source of maskable external interrupts (S4-38L8b). [`NoIrqSource`] (never a vector,
    /// never a deadline) until a later slice gives the board a controller; consulted only by the
    /// CPL0-set loop and only when the run asks for the hook (`irq` member).
    irq_source: Box<dyn BoardIrqSource>,
    /// Handle of the 8259 master chip (S4-39g); `Some` under `linux_profile` (the PIC is bound
    /// there).
    pic_master: Option<Pic8259Handle>,
    /// The PIT -> 8259 IR0 source once [`Self::enable_pit_pic_irq`] installed it (S4-39g).
    pit_pic_source: Option<PitPicIrqSource>,
    /// Test injection (S4-37a2): `run_boot` drops the queued PM timer ticks
    /// instead of applying them. Never set in production code.
    #[cfg(test)]
    suppress_pm_tick: bool,
    /// Test injection (S4-37a6b): `run_boot` drops the pending RDTSC tick.
    #[cfg(test)]
    suppress_rdtsc_tick: bool,
}

impl LoadedFirmwareBoard {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut LongModeState {
        &mut self.state
    }

    /// Owner/host handle of the board LAPIC (S4-38 pump becomes its owner); it can
    /// write (reset, `set_global_enabled`, `request_fixed_edge`, `acknowledge`,
    /// `dispatch_timer`). `has_timer()` is true only under `linux_profile`.
    pub fn local_apic(&self) -> LocalApicHandle {
        self.lapic.clone()
    }

    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }

    /// COM1 bytes recorded by the latest `run_boot`, including those of a run
    /// that returned an error (so the runner never loses transcript bytes).
    pub fn com1_transcript(&self) -> &[u8] {
        &self.com1_log
    }

    pub fn uart(&self) -> Uart16550Handle {
        self.uart.clone()
    }

    pub fn display(&self) -> DisplayHandle {
        self.display.clone()
    }

    /// Handle of the NVMe controller; `None` unless `FirmwareBoardConfig::nvme` was set.
    pub fn nvme(&self) -> Option<NvmeHandle> {
        self.nvme.clone()
    }

    /// Replaces the board's interrupt source (S4-38L8b). Nothing in production code calls it
    /// yet: the board has [`NoIrqSource`] by default and the runner has no flag for a source
    /// (S4-38L8c).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_irq_source(&mut self, source: Box<dyn BoardIrqSource>) {
        self.irq_source = source;
    }

    /// A source of the PIT channel 0 line through the IOAPIC INTIN2 and the Local APIC of this
    /// board (S4-38L8c), on the board clock; `None` without the Linux profile (no PIT). It is not
    /// installed: the caller gives it to [`Self::set_irq_source`] (a clone of it stays a handle
    /// for the counters). The board does this for no run today: the Linux board has no APIC in
    /// CPUID, and a synthetic guest or a test installs the source itself.
    ///
    /// The IOAPIC handle is made from the Local APIC handle the way `assemble_board` makes the
    /// IOAPIC device: both views share one pair state.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pit_ioapic_irq_source(&self) -> Option<PitIoapicIrqSource> {
        let pit = self.pit.clone()?;
        let io_apic = IoApic::new(self.lapic.clone()).handle();
        Some(PitIoapicIrqSource::new(
            pit,
            io_apic,
            self.lapic.clone(),
            self.clock.clone(),
        ))
    }

    /// S4-39g: install the PIT channel 0 -> 8259 master IR0 source as the board's interrupt
    /// source (once; a second call keeps the installed one and its state). `false` and nothing
    /// installed without the Linux-profile PIT and PIC. Called by the user-mode run of the
    /// Linux board only; the default board keeps [`NoIrqSource`].
    pub(crate) fn enable_pit_pic_irq(&mut self) -> bool {
        if self.pit_pic_source.is_some() {
            return true;
        }
        let (Some(pit), Some(pic)) = (self.pit.clone(), self.pic_master.clone()) else {
            return false;
        };
        let source = PitPicIrqSource::new(pit, pic, self.clock.clone());
        self.irq_source = Box::new(source.clone());
        self.pit_pic_source = Some(source);
        true
    }

    /// Handle of the 8259 master chip (S4-39g); `None` without the Linux profile.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pic_master(&self) -> Option<Pic8259Handle> {
        self.pic_master.clone()
    }

    /// The installed PIT -> 8259 source (a second handle on it), if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pit_pic_source(&self) -> Option<PitPicIrqSource> {
        self.pit_pic_source.clone()
    }

    /// True under `linux_profile`: the RDTSC time source (and with it the tick) exists only
    /// there, see `assemble_board`.
    pub(crate) const fn has_linux_profile(&self) -> bool {
        self.rdtsc_tick.is_some()
    }

    /// The board's `CR3`: the firmware boot tables, or for a Linux board the root of the Linux
    /// identity tables (`page_table_root` passed to `AssembledBoard::into_linux_board`). After a
    /// run through the CPL0 set (`run_linux_boot_cpl0_set`) it is the CR3 of the last `MOV CR3`
    /// the guest retired, also when the run ended in an integration error of that very
    /// instruction (`Cr3EffectMismatch`, `Cr3SnapshotRejected`): the instruction retired, only
    /// the walker was not rebuilt for it.
    pub const fn cr3(&self) -> u64 {
        self.page_table_root
    }

    /// The EFER of the translation walkers (LME|LMA|NXE); the guest's `SCE` is not part of it.
    pub const fn efer(&self) -> u64 {
        FIRMWARE_EFER
    }

    /// The guest-visible `IA32_EFER`: the walker EFER, plus `SCE` once the guest wrote it
    /// through the CPL0 set (S4-38L7f).
    pub const fn guest_efer(&self) -> u64 {
        self.guest_efer
    }

    /// Stores the guest-visible EFER a CPL0-set run left (S4-38L7f).
    pub(crate) fn set_guest_efer(&mut self, value: u64) {
        self.guest_efer = value;
    }

    /// The guest PCI CONFIG_ADDRESS register (S4-38L7w); `0` until a guest `OUT` to `0xCF8`.
    pub const fn pci_config_address(&self) -> u32 {
        self.pci_config_address
    }

    /// Stores the CONFIG_ADDRESS a CPL0-set run left (S4-38L7w).
    pub(crate) fn set_pci_config_address(&mut self, value: u32) {
        self.pci_config_address = value;
    }

    /// The guest `IA32_STAR` (S4-38L7ao); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of STAR stays refused.
    pub const fn guest_star(&self) -> u64 {
        self.guest_star
    }

    /// Stores the STAR a CPL0-set run left (S4-38L7ao).
    pub(crate) fn set_guest_star(&mut self, value: u64) {
        self.guest_star = value;
    }

    /// The guest `IA32_LSTAR` (S4-38L7ap); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of LSTAR stays refused.
    pub const fn guest_lstar(&self) -> u64 {
        self.guest_lstar
    }

    /// Stores the LSTAR a CPL0-set run left (S4-38L7ap).
    pub(crate) fn set_guest_lstar(&mut self, value: u64) {
        self.guest_lstar = value;
    }

    /// The guest `IA32_CSTAR` (S4-38L7aq); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of CSTAR stays refused.
    pub const fn guest_cstar(&self) -> u64 {
        self.guest_cstar
    }

    /// Stores the CSTAR a CPL0-set run left (S4-38L7aq).
    pub(crate) fn set_guest_cstar(&mut self, value: u64) {
        self.guest_cstar = value;
    }

    /// The guest `IA32_SYSENTER_CS` (S4-38L7ar); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_CS stays refused.
    pub const fn guest_sysenter_cs(&self) -> u64 {
        self.guest_sysenter_cs
    }

    /// Stores the SYSENTER_CS a CPL0-set run left (S4-38L7ar).
    pub(crate) fn set_guest_sysenter_cs(&mut self, value: u64) {
        self.guest_sysenter_cs = value;
    }

    /// The guest `IA32_SYSENTER_ESP` (S4-38L7as); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_ESP stays refused.
    pub const fn guest_sysenter_esp(&self) -> u64 {
        self.guest_sysenter_esp
    }

    /// Stores the SYSENTER_ESP a CPL0-set run left (S4-38L7as).
    pub(crate) fn set_guest_sysenter_esp(&mut self, value: u64) {
        self.guest_sysenter_esp = value;
    }

    /// The guest `IA32_SYSENTER_EIP` (S4-38L7as); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_EIP stays refused.
    pub const fn guest_sysenter_eip(&self) -> u64 {
        self.guest_sysenter_eip
    }

    /// Stores the SYSENTER_EIP a CPL0-set run left (S4-38L7as).
    pub(crate) fn set_guest_sysenter_eip(&mut self, value: u64) {
        self.guest_sysenter_eip = value;
    }

    /// The guest `IA32_SYSCALL_MASK` (S4-38L7at); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSCALL_MASK stays refused.
    pub const fn guest_syscall_mask(&self) -> u64 {
        self.guest_syscall_mask
    }

    /// Stores the SYSCALL_MASK a CPL0-set run left (S4-38L7at).
    pub(crate) fn set_guest_syscall_mask(&mut self, value: u64) {
        self.guest_syscall_mask = value;
    }

    /// The guest `IA32_KERNEL_GS_BASE` (S4-38L7au); `0` until a guest `WRMSR` of it. The full
    /// 64-bit value written. Not readable by the guest: `RDMSR` of KERNEL_GS_BASE stays refused.
    pub const fn guest_kernel_gs_base(&self) -> u64 {
        self.guest_kernel_gs_base
    }

    /// Stores the KERNEL_GS_BASE a CPL0-set run left (S4-38L7au).
    pub(crate) fn set_guest_kernel_gs_base(&mut self, value: u64) {
        self.guest_kernel_gs_base = value;
    }

    /// The guest FP state of the CPL0 set's FP member (S4-38L9d); the initial state until a run
    /// with the member changed it.
    pub const fn guest_fp(&self) -> &GuestFpState {
        &self.guest_fp
    }

    /// Stores the FP state a CPL0-set run left (S4-38L9d).
    pub(crate) fn set_guest_fp(&mut self, value: GuestFpState) {
        self.guest_fp = value;
    }

    /// The exact bytes written to RAM at [`FIRMWARE_HANDOFF_GPA`]. Empty for a Linux board
    /// (`AssembledBoard::into_linux_board` builds no handoff block).
    pub fn handoff_block(&self) -> &[u8] {
        &self.handoff
    }

    /// Length of the `BOOT_PAGE_TABLES` region (`4096 * N`). For a Linux board it is the size of
    /// the Linux identity tables written by the loader (`3..=6` pages), not the firmware's.
    pub const fn boot_page_table_bytes(&self) -> u64 {
        self.page_table_bytes
    }

    /// Handle of the ACPI PM block; `None` unless `FirmwareBoardConfig::acpi_pm` was set.
    pub fn acpi_pm(&self) -> Option<AcpiPmHandle> {
        self.acpi_pm.clone()
    }

    /// The virtual clock as of now (a snapshot; time moves only through
    /// [`Self::advance_time_ns`]).
    pub fn clock(&self) -> VirtualClock {
        self.clock.snapshot()
    }

    /// Explicit virtual-time advance; time never moves on its own
    /// (MACHINE_CONTRACT.md, caller-driven virtual clock).
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), FirmwareBoardError> {
        self.clock
            .advance_standalone(delta_ns)
            .map_err(FirmwareBoardError::Clock)
    }

    /// One quantum of the existing bounded paged interpreter path. Like the PM
    /// timer and PIT port ticks, the RDTSC tick is not applied here: only
    /// `run_boot` advances the clock between quanta.
    ///
    /// Under `linux_profile` a bare `RDTSC` is executed (S4-37a6b): it returns
    /// the board `clock.tsc()` as of the quantum boundary (the clock does not move
    /// inside a quantum, a known deviation) and queues a 1 us tick. Without the
    /// profile `RDTSC` stays `Unsupported { opcode: 0x31 }`.
    pub fn run(
        &mut self,
        instruction_budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> PagedProbeQuantumReport {
        self.run_quantum(false, instruction_budget, stop, committed_write)
    }

    /// [`Self::run`] with the 4 KiB walker (`large_ram = false`) or with the 2 MiB walk of the
    /// Linux direct boot (`large_ram = true`, S4-38L1: `run_paged_probe_2m_ram`). The CPU
    /// profile, the board CR3 and the RDTSC source are the same in both.
    fn run_quantum(
        &mut self,
        large_ram: bool,
        instruction_budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> PagedProbeQuantumReport {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        if large_ram {
            let mut source = self.rdtsc_tick.clone().map(|tick| {
                let clock = self.clock.clone();
                move || {
                    tick.store(true, Ordering::Relaxed);
                    clock.snapshot().tsc()
                }
            });
            return run_paged_probe_2m_ram(
                &mut self.state,
                &mut self.bus,
                self.page_table_root,
                FIRMWARE_EFER,
                context,
                instruction_budget,
                INTEGER_FLAGS,
                stop,
                committed_write,
                source
                    .as_mut()
                    .map(|source| source as &mut dyn FnMut() -> u64),
            );
        }
        if let Some(tick) = self.rdtsc_tick.clone() {
            let clock = self.clock.clone();
            let mut source = move || {
                tick.store(true, Ordering::Relaxed);
                clock.snapshot().tsc()
            };
            return run_paged_probe_with_rdtsc(
                &mut self.state,
                &mut self.bus,
                self.page_table_root,
                FIRMWARE_EFER,
                context,
                instruction_budget,
                INTEGER_FLAGS,
                stop,
                committed_write,
                &mut source,
            );
        }
        run_paged_probe(
            &mut self.state,
            &mut self.bus,
            self.page_table_root,
            FIRMWARE_EFER,
            context,
            instruction_budget,
            INTEGER_FLAGS,
            stop,
            committed_write,
        )
    }

    /// Up to `len` bytes at `rip` as an instruction fetch would see them (S4-38L1): every byte is
    /// translated through the 2 MiB walk with the board CR3 (no accessed/dirty bit is written) and
    /// read from RAM. Returns the GPA of `rip` (when it translates), the bytes read (they stop at
    /// the first byte that does not translate or is not RAM) and the text of that failure.
    pub(crate) fn fetch_view_2m(
        &mut self,
        rip: u64,
        len: usize,
    ) -> (Option<u64>, Vec<u8>, Option<String>) {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let paging = match Long64ProbeConfig::from_cpu_state(
            &self.state,
            self.page_table_root,
            FIRMWARE_EFER,
        ) {
            Ok(paging) => paging,
            Err(error) => return (None, Vec::new(), Some(format!("{error:?}"))),
        };
        let (memory, _) = self.bus.split_for_pio_step();
        let mut first = None;
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len as u64 {
            let linear = rip.wrapping_add(offset);
            let physical = match translate_long64_2m_ram_read_only(
                memory,
                &paging,
                rip,
                GuestLinearAddress::new(linear),
                context,
                AccessKind::Fetch,
            ) {
                Ok(physical) => physical,
                Err(error) => return (first, bytes, Some(format!("{error:?}"))),
            };
            if offset == 0 {
                first = Some(physical.get());
            }
            match memory.read_u8(physical) {
                Ok(byte) => bytes.push(byte),
                Err(error) => return (first, bytes, Some(format!("{error:?}"))),
            }
        }
        (first, bytes, None)
    }

    /// Run quanta until the guest halts, faults or the instruction budget is
    /// spent. Each quantum ends on a COM1 THR write (FIRMWARE_BOOT.md 11); the
    /// byte is moved through the UART and recorded, then virtual time advances
    /// by `retired * 1 ns` before the next quantum (FIRMWARE_BOOT.md 8).
    ///
    /// PM timer port tick (S4-37a2, ISSUE-067): the queue of the PM device is
    /// cleared at the start (host reads through `bus_mut` never move the clock).
    /// A guest read of the PM timer ends the quantum after its instruction, and
    /// the queued 1 us is applied to the clock before the next instruction.
    /// The same holds for the PIT port tick of the Linux profile (S4-37a3):
    /// guest reads of port B `0x61` and of the channel 2 counter `0x42` queue
    /// 1 us each; the queue is cleared at the start and applied after each
    /// quantum together with the PM timer ticks.
    /// The same holds for a guest `RDTSC` under the Linux profile (S4-37a6b): it
    /// ends the quantum, the flag is cleared at the start, and 1 us is applied
    /// after the PIT tick (order: retired ns, PM, PIT, RDTSC).
    pub fn run_boot(
        &mut self,
        instruction_budget: u64,
    ) -> Result<FirmwareRunReport, FirmwareBoardError> {
        self.run_boot_with(false, instruction_budget)
            .map_err(|failure| failure.error)
    }

    /// [`Self::run_boot`] through the 2 MiB walk of the Linux direct boot (S4-38L1). The ticks
    /// (retired ns, PM timer, PIT, RDTSC) are applied exactly as in `run_boot`.
    ///
    /// Unlike `run_boot`, a failure also carries the instructions retired before it (the clock
    /// can fail after a quantum has executed), so the caller does not report a false zero.
    pub(crate) fn run_linux_boot(
        &mut self,
        instruction_budget: u64,
    ) -> Result<FirmwareRunReport, LinuxRunFailure> {
        self.run_boot_with(true, instruction_budget)
    }

    /// The Linux direct boot through the CPL0 profile set instead of the base probe profile
    /// (S4-38L3). Same quantum structure and ticks as [`Self::run_linux_boot`] (retired ns, PM
    /// timer, PIT, RDTSC in that order; a guest port B/PIT/PM-timer read or RDTSC ends the
    /// quantum); the quantum is [`run_cpl0_set_paged_2m_ram`], which rebuilds the 2 MiB walker
    /// on every `MOV CR3` the set retires. A `MOV CR3` is therefore a continuation, not a stop.
    ///
    /// The board CR3 is given to the CPU state for the run (the set must therefore be built
    /// `with_cpu_owned_cr3`, otherwise it refuses the state: `Cr3NotAdmitted`). Afterwards the
    /// CPU state carries no CR3 again and [`Self::cr3`] is the CR3 of the last retired `MOV CR3`
    /// (also on an integration-error exit of that instruction), so the board can be run again
    /// through either path.
    ///
    /// COM1 (`OUT DX, AL`, `with_pio_u8`) goes through the machine bus like in `run_boot`; the
    /// PIT and PM timer port reads of the Linux profile are tick sources only through it.
    ///
    /// A partial REP exit of the set (`RepIterationBudgetExhausted`, S4-38L4b) is a quantum
    /// boundary like a tick: the loop calls the set again on the same RIP while the budget lasts
    /// and no stop was asked for. The budget unit is the instruction: the REP retires, and costs
    /// one unit and one ns, when it completes.
    ///
    /// With `cpuid_boundary` the run stops before a `CPUID` whose EAX is 1 or 0x8000_0001
    /// (S4-38L4, `cpuid_divergence`): the instruction does not execute and the report carries
    /// its leaf. Nothing is answered in its place.
    ///
    /// With `fetch_cache` (S4-13) each call of the set loop keeps an opt-in translation cache for
    /// the instruction fetch (see `fetch_translation_cache`); results are identical to the run
    /// without it, only the number of page walks differs.
    ///
    /// With `irq_hook` (S4-38L8b, the `irq` member) the board source is consulted on every
    /// instruction boundary (`deliver_pending_irq` of the loop), a quantum is never longer than
    /// the instructions left until the source's `next_deadline_ns` (at least one; each cut is
    /// counted and resumed), a guest write to the PIT ports ends the quantum when the source asks
    /// for it, and the counters of the report are filled. The board has [`NoIrqSource`] unless a
    /// test installs another, so the hook changes no result today.
    pub(crate) fn run_linux_boot_cpl0_set(
        &mut self,
        instruction_budget: u64,
        set: &Cpl0ProfileSet<'_>,
        cpuid_boundary: bool,
        fetch_cache: bool,
        irq_hook: bool,
    ) -> Result<LinuxSetRunReport, LinuxRunFailure> {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let mut remaining = instruction_budget;
        let mut retired = 0u64;
        let mut quanta = 0u64;
        let mut changes: Vec<TranslationContextChange> = Vec::new();
        let mut exceptions_delivered = 0u64;
        let mut cpl3_retired = 0u64;
        let mut syscall_journal = crate::linux_direct::SyscallJournal::default();
        self.com1_log.clear();
        let nvme = self.nvme.clone();
        let acpi_pm = self.acpi_pm.clone();
        let pit = self.pit.clone();
        let rdtsc_tick = self.rdtsc_tick.clone();
        if let Some(pm) = &acpi_pm {
            pm.take_timer_tick_ns();
        }
        if let Some(pit) = &pit {
            pit.take_port_tick_ns();
        }
        if let Some(flag) = &rdtsc_tick {
            flag.store(false, Ordering::Relaxed);
        }
        if let Some(nvme) = &nvme {
            nvme.service(&mut self.bus, |_| {});
        }
        self.state.cr3 = Some(self.page_table_root);
        let mut irq_counters = IrqCounters::default();
        // The next call resumes a partial REP: its first boundary is inside the REP.
        let mut partial_rep = false;
        // S4-38L5a/c: the set models CR2 for `MOV RAX, CR2`, and a delivered `#PF` writes it
        // (S4-38L5c). Like CR3 (kept in `page_table_root` through `report.cr3` and fed back on
        // entry), the CR2 the guest left is kept in `guest_cr2` and fed back on entry,
        // so a handler that reads CR2 in a later call gets the address of its fault. CR2 is
        // `0` (the architectural reset value) until a `#PF` was delivered or the guest wrote
        // it; the CPU state itself carries no CR2 outside this call (the prior value is
        // restored on exit).
        let prior_cr2 = self.state.cr2;
        self.state.cr2 = Some(self.guest_cr2.or(prior_cr2).unwrap_or(0));
        let result = loop {
            let uart = self.uart.clone();
            let mut source = rdtsc_tick.clone().map(|tick| {
                let clock = self.clock.clone();
                move || {
                    tick.store(true, Ordering::Relaxed);
                    clock.snapshot().tsc()
                }
            });
            let stop = || {
                uart.holding_occupied()
                    || nvme.as_ref().is_some_and(NvmeHandle::doorbell_pending)
                    || acpi_pm
                        .as_ref()
                        .is_some_and(|pm| pm.stop_request().is_some())
                    || acpi_pm
                        .as_ref()
                        .is_some_and(AcpiPmHandle::timer_tick_pending)
                    || pit.as_ref().is_some_and(Pit8254Handle::port_tick_pending)
                    || rdtsc_tick
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Relaxed))
            };
            // S4-38L8b: the quantum ends at the next edge of the source; the clock moves only
            // after the quantum (one instruction is one ns), so the limit is the time left.
            let mut quantum_budget = remaining;
            let mut cut_by_deadline = false;
            if irq_hook && let Some(deadline) = self.irq_source.next_deadline_ns() {
                let limit = deadline
                    .saturating_sub(self.clock.snapshot().now_ns())
                    .max(1);
                if limit < remaining {
                    quantum_budget = limit;
                    cut_by_deadline = true;
                    irq_counters.quantum_cut_by_deadline += 1;
                }
            }
            let report = run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
                &mut self.state,
                &mut self.bus,
                self.page_table_root,
                FIRMWARE_EFER,
                context,
                quantum_budget,
                INTEGER_FLAGS,
                set,
                source
                    .as_mut()
                    .map(|source| source as &mut dyn FnMut() -> u64),
                stop,
                |_| {},
                cpuid_boundary,
                fetch_cache,
                irq_hook.then(|| IrqBoundaryHook {
                    source: &mut *self.irq_source,
                    counters: &mut irq_counters,
                    skip_first_boundary: partial_rep,
                }),
            );
            quanta += 1;
            partial_rep = matches!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)
            );
            // The walker of the next quantum starts from the CR3 the guest left.
            self.page_table_root = report.cr3;
            changes.extend(report.translation_context_changes.iter().copied());
            exceptions_delivered += report.delivered_exceptions;
            cpl3_retired += report.cpl3_retired;
            for event in &report.syscall_events {
                syscall_journal.record(*event);
            }
            if let Some(nvme) = &nvme {
                nvme.service(&mut self.bus, |_| {});
            }
            retired += report.instructions_executed;
            remaining = remaining.saturating_sub(report.instructions_executed);
            while uart.transfer_holding_to_shift() {
                if let Some(byte) = uart.complete_transmission() {
                    self.com1_log.push(byte);
                }
            }
            let failure = |error| LinuxRunFailure { error, retired };
            if let Err(error) = self.advance_time_ns(report.instructions_executed) {
                break Err(failure(error));
            }
            if let Some(pm) = &acpi_pm {
                let tick_ns = pm.take_timer_tick_ns();
                #[cfg(test)]
                let tick_ns = if self.suppress_pm_tick { 0 } else { tick_ns };
                if let Err(error) = self.advance_time_ns(tick_ns) {
                    break Err(failure(error));
                }
            }
            if let Some(pit) = &pit
                && let Err(error) = self.advance_time_ns(pit.take_port_tick_ns())
            {
                break Err(failure(error));
            }
            if let Some(flag) = &rdtsc_tick
                && flag.swap(false, Ordering::Relaxed)
            {
                let tick_ns = RDTSC_TICK_NS;
                #[cfg(test)]
                let tick_ns = if self.suppress_rdtsc_tick { 0 } else { tick_ns };
                if let Err(error) = self.advance_time_ns(tick_ns) {
                    break Err(failure(error));
                }
            }
            let stop_reason = acpi_pm.as_ref().and_then(AcpiPmHandle::stop_request);
            // S4-39g: a halted CPU (`HLT`) with IF = 1 and a source that can wake it: the clock
            // skips to the wake time (the board has nothing else to run), the CPU is `Running`
            // again, and the next boundary delivers the vector. The `HLT` itself retired (its
            // instruction and 1 ns are counted above). IF = 0, a source without a wake time
            // (`None`: the line is masked or in service, no edge) or a PM stop request leave the
            // exit as it was: the run ends as `halted`. Each wake costs one unit of the budget,
            // so a source that wakes the CPU without ever giving a vector cannot loop forever.
            if irq_hook
                && stop_reason.is_none()
                && remaining > 0
                && matches!(report.exit, PagedCpl0SetExit::Run(RunExit::Halted))
                && self.state.rflags & (1 << 9) != 0
                && let Some(wake) = self.irq_source.wake_ns()
            {
                let idle = wake.saturating_sub(self.clock.snapshot().now_ns());
                if let Err(error) = self.advance_time_ns(idle) {
                    break Err(failure(error));
                }
                irq_counters.halt_wakes += 1;
                irq_counters.halt_idle_ns += idle;
                self.state.activity = Some(CpuActivity::Running);
                remaining -= 1;
                partial_rep = false;
                continue;
            }
            let cpuid_divergence_leaf = match report.exit {
                PagedCpl0SetExit::CpuidDivergence { leaf } => Some(leaf),
                _ => None,
            };
            let user_entry = match report.exit {
                PagedCpl0SetExit::UserEntry(entry) => Some(entry),
                _ => None,
            };
            // A quantum stopped by a tick flag (no PM stop request) with budget left resumes,
            // and so does a partial REP exit (S4-38L4b); decided before the exit is converted,
            // so the `Debug` text is built only for the exit that ends the run. The CPUID
            // boundary is neither, so it never resumes.
            if resumes_set_exit(
                &report.exit,
                stop_reason.is_some(),
                remaining,
                report.instructions_executed,
            ) || resumes_deadline_cut(
                &report.exit,
                cut_by_deadline,
                stop_reason.is_some(),
                remaining,
                report.instructions_executed,
            ) {
                continue;
            }
            let (exit, set_exit_detail) = match set_exit_to_probe_exit(report.exit) {
                Ok(exit) => exit,
                Err(detail) => break Err(failure(FirmwareBoardError::Config(detail))),
            };
            let exit = match exit {
                PagedProbeExit::Stopped
                    if stop_reason.is_none()
                        && remaining == 0
                        && cpuid_divergence_leaf.is_none()
                        && user_entry.is_none() =>
                {
                    PagedProbeExit::BudgetExhausted
                }
                other => other,
            };
            break Ok(FirmwareRunReport {
                exit,
                page_fault: report.page_fault,
                memory_error: report.memory_error,
                retired,
                quanta,
                com1: self.com1_log.clone(),
                virtual_ns: self.clock().now_ns(),
                tsc_hz: self.clock().tsc_hz(),
                stop_reason,
            })
            .map(|run| (run, set_exit_detail, cpuid_divergence_leaf, user_entry));
        };
        // The CPU state carries no CR3 outside this call, whatever way the loop ended.
        self.state.cr3 = None;
        // CR2 moves to the board field (see the entry comment); the CPU state gets back what
        // it had.
        self.guest_cr2 = self.state.cr2;
        self.state.cr2 = prior_cr2;
        let (run, set_exit_detail, cpuid_divergence_leaf, user_entry) = result?;
        irq_counters.irq0_edges = self.irq_source.irq0_edges();
        Ok(LinuxSetRunReport {
            run,
            translation_context_changes: changes,
            set_exit_detail,
            cpuid_divergence_leaf,
            user_entry,
            cpl3_retired,
            syscall_journal,
            exceptions_delivered,
            irq: irq_counters,
        })
    }

    fn run_boot_with(
        &mut self,
        large_ram: bool,
        instruction_budget: u64,
    ) -> Result<FirmwareRunReport, LinuxRunFailure> {
        let mut remaining = instruction_budget;
        let mut retired = 0u64;
        let mut quanta = 0u64;
        self.com1_log.clear();
        let nvme = self.nvme.clone();
        let acpi_pm = self.acpi_pm.clone();
        let pit = self.pit.clone();
        let rdtsc_tick = self.rdtsc_tick.clone();
        // Ticks queued before this run came from the host, not from the guest.
        if let Some(pm) = &acpi_pm {
            pm.take_timer_tick_ns();
        }
        if let Some(pit) = &pit {
            pit.take_port_tick_ns();
        }
        if let Some(flag) = &rdtsc_tick {
            flag.store(false, Ordering::Relaxed);
        }
        // Doorbells the host rang before the run are executed before the guest starts.
        if let Some(nvme) = &nvme {
            nvme.service(&mut self.bus, |_| {});
        }
        loop {
            let uart = self.uart.clone();
            let report = self.run_quantum(
                large_ram,
                remaining,
                || {
                    uart.holding_occupied()
                        || nvme.as_ref().is_some_and(NvmeHandle::doorbell_pending)
                        || acpi_pm
                            .as_ref()
                            .is_some_and(|pm| pm.stop_request().is_some())
                        // A guest PM timer read ends the quantum so that its tick
                        // reaches the clock before the next instruction.
                        || acpi_pm.as_ref().is_some_and(AcpiPmHandle::timer_tick_pending)
                        // The same for a guest read of port B or of the PIT counter.
                        || pit.as_ref().is_some_and(Pit8254Handle::port_tick_pending)
                        // A guest RDTSC (Linux profile) ends the quantum as well.
                        || rdtsc_tick
                            .as_ref()
                            .is_some_and(|flag| flag.load(Ordering::Relaxed))
                },
                |_| {},
            );
            quanta += 1;
            // Doorbell writes only record queue pointers: execute the submitted
            // commands and post completions between quanta (NVME_DEVICE.md).
            if let Some(nvme) = &nvme {
                nvme.service(&mut self.bus, |_| {});
            }
            retired += report.instructions_executed;
            remaining = remaining.saturating_sub(report.instructions_executed);
            while uart.transfer_holding_to_shift() {
                if let Some(byte) = uart.complete_transmission() {
                    self.com1_log.push(byte);
                }
            }
            self.advance_time_ns(report.instructions_executed)
                .map_err(|error| LinuxRunFailure { error, retired })?;
            if let Some(pm) = &acpi_pm {
                let tick_ns = pm.take_timer_tick_ns();
                #[cfg(test)]
                let tick_ns = if self.suppress_pm_tick { 0 } else { tick_ns };
                self.advance_time_ns(tick_ns)
                    .map_err(|error| LinuxRunFailure { error, retired })?;
            }
            if let Some(pit) = &pit {
                self.advance_time_ns(pit.take_port_tick_ns())
                    .map_err(|error| LinuxRunFailure { error, retired })?;
            }
            if let Some(flag) = &rdtsc_tick
                && flag.swap(false, Ordering::Relaxed)
            {
                let tick_ns = RDTSC_TICK_NS;
                #[cfg(test)]
                let tick_ns = if self.suppress_rdtsc_tick { 0 } else { tick_ns };
                self.advance_time_ns(tick_ns)
                    .map_err(|error| LinuxRunFailure { error, retired })?;
            }
            let stop_reason = acpi_pm.as_ref().and_then(AcpiPmHandle::stop_request);
            let exit = match report.exit {
                // A requested machine stop is final: no further quantum, and it is
                // not reported as an exhausted budget.
                PagedProbeExit::Stopped if stop_reason.is_some() => PagedProbeExit::Stopped,
                PagedProbeExit::Stopped if remaining == 0 => PagedProbeExit::BudgetExhausted,
                PagedProbeExit::Stopped if report.instructions_executed > 0 => continue,
                other => other,
            };
            return Ok(FirmwareRunReport {
                exit,
                page_fault: report.page_fault,
                memory_error: report.memory_error,
                retired,
                quanta,
                com1: self.com1_log.clone(),
                virtual_ns: self.clock().now_ns(),
                tsc_hz: self.clock().tsc_hz(),
                stop_reason,
            });
        }
    }
}

/// Validate `image`, fill ROM, copy it to the RAM shadow, build the temporary
/// page tables and the handoff block, and return the entry state.
pub fn load_firmware_image(
    image: &[u8],
    config: &FirmwareBoardConfig,
) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
    // The board declares no PCI function: bus 0 stays empty.
    load_firmware_image_with_pci(image, config, PciEcam::new())
}

/// Shared loader. `pci` is registered only when `config.pci_ecam` is set; the
/// public entry point passes an empty one, tests pass a diagnostic endpoint.
fn load_firmware_image_with_pci(
    image: &[u8],
    config: &FirmwareBoardConfig,
    pci: PciEcam,
) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
    load_firmware_image_inner(image, config, pci, None, None)
}

/// A board that is assembled but not loaded (S4-37L4): the memory map, bus and
/// devices, the handoff block and the boot page tables exist as values, but no
/// byte has been written to guest memory and no ROM is filled. The firmware loader
/// ([`AssembledBoard::load_firmware`]) and the Linux direct boot reuse it.
pub(crate) struct AssembledBoard {
    bus: MachineBus,
    uart: Uart16550Handle,
    display: DisplayHandle,
    nvme: Option<NvmeHandle>,
    acpi_pm: Option<AcpiPmHandle>,
    pit: Option<Pit8254Handle>,
    pic_master: Option<Pic8259Handle>,
    lapic: LocalApicHandle,
    clock: VirtualClockHandle,
    /// Handoff block bytes, to be written at [`FIRMWARE_HANDOFF_GPA`].
    handoff: Vec<u8>,
    /// Temporary identity tables, to be written at `page_table_root`.
    tables: Vec<u8>,
    page_table_bytes: u64,
    /// GPA of the PML4 and the `CR3` the paged interpreter uses.
    page_table_root: u64,
    rdtsc_tick: Option<Arc<AtomicBool>>,
}

/// Assemble the board without touching guest memory (S4-37L4): validate the
/// configuration, build the memory map, handoff block, boot tables, devices and
/// bus.
///
/// `pit_clock` replaces the clock of the Linux-profile PIT. Production code always
/// passes `None` (the PIT then reads the board clock); only a test injects another
/// handle, to prove that the binding really follows the board clock (S4-37a1).
/// `apic_timer_clock` does the same for the Linux-profile LAPIC timer (S4-37a4).
pub(crate) fn assemble_board(
    config: &FirmwareBoardConfig,
    pci: PciEcam,
    pit_clock: Option<VirtualClockHandle>,
    apic_timer_clock: Option<VirtualClockHandle>,
) -> Result<AssembledBoard, FirmwareBoardError> {
    let devices = configured_board_devices(config);
    validate_config(config)?;

    let map = board_map(config, &devices)?;
    let table_pages = boot_page_table_count(&map);
    let page_table_bytes = table_pages * PAGE;
    let low_end = config.ram_bytes.min(FIRMWARE_LOW_RAM_LIMIT);
    let page_table_root = FIRMWARE_BOOT_PAGE_TABLES_GPA;
    // `handoff_regions` still derives the table region from the constant and assumes the
    // tables start exactly at SHADOW_END. Moving the root requires passing it through there.
    // A Linux board is assembled through this same path, so the handoff block is built too: the
    // Linux loader compares its E820 map against it (S4-37L6) before `into_linux_board` drops
    // the block and replaces the page table root with its own.
    debug_assert_eq!(page_table_root, FIRMWARE_BOOT_PAGE_TABLES_GPA);
    debug_assert_eq!(page_table_root, SHADOW_END);
    let tables_end = page_table_root + page_table_bytes;
    if tables_end > low_end {
        return Err(FirmwareBoardError::BootTablesDoNotFit {
            required_end: tables_end,
            ram_end: low_end,
        });
    }
    let regions = handoff_regions(&map, &devices, page_table_bytes);
    if regions.len() > MAX_REGIONS {
        return Err(FirmwareBoardError::TooManyRegions(regions.len()));
    }
    let handoff = build_handoff(config, &devices, &regions);
    let tables = build_boot_tables(&map, page_table_root, table_pages)?;

    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    // One clock for the whole board: the PM timer, the PIT and (Linux profile) the
    // LAPIC timer observe it; the board stays its only advancer.
    let clock = VirtualClockHandle::new(config.tsc_hz).map_err(FirmwareBoardError::Clock)?;
    // Under `linux_profile` the LAPIC has a timer on the board clock (S4-37a4). The
    // timer only reads the clock; delivery of its IRQ is S4-38.
    let lapic = if config.linux_profile {
        LocalApic::with_shared_timer(
            apic_timer_clock.unwrap_or_else(|| clock.clone()),
            FIRMWARE_APIC_TIMER_HZ,
        )
        .map_err(|_| FirmwareBoardError::Config("apic timer needs a nonzero frequency"))?
    } else {
        LocalApic::new()
    };
    let lapic_handle = lapic.handle();
    let ioapic = IoApic::new(lapic.handle());
    let (display_device, display) = if config.second_screen {
        DisplayDevice::new_two_screens_with_handle()
    } else {
        DisplayDevice::new_with_handle()
    };
    // Bus order: COM1, LAPIC, IOAPIC, optional display, then NVMe,
    // the optional PCI configuration device and the optional ACPI PM block; the
    // `linux_profile` devices (PIT, port B, 8259 pair, CMOS, 0xED, 0x80) are appended
    // at the very end of the list.
    let mut bus_devices: Vec<Box<dyn crate::BusDevice>> =
        vec![Box::new(uart_device), Box::new(lapic), Box::new(ioapic)];
    if config.display_enabled {
        bus_devices.push(Box::new(display_device));
    }
    let mut mmio: Vec<MmioBinding> = [DEV_LAPIC, DEV_IOAPIC]
        .iter()
        .map(|&index| MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[index].base),
            device_index: index,
        })
        .collect();
    if config.display_enabled {
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_DISPLAY].base),
            device_index: DEV_DISPLAY,
        });
    }
    // The device exists on the bus only when the manifest entry is PRESENT.
    let mut nvme = None;
    if let Some(nvme_config) = &config.nvme
        && devices[DEV_NVME_TABLE].present()
    {
        let (device, handle) =
            NvmeDevice::new_with_handle(nvme_config.clone()).map_err(FirmwareBoardError::Nvme)?;
        let device_index = bus_devices.len();
        bus_devices.push(Box::new(device));
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_NVME_TABLE].base),
            device_index,
        });
        nvme = Some(handle);
    }
    let com1 = PortRange::new(
        PortAddress::new(devices[DEV_COM1].base as u16),
        devices[DEV_COM1].length as u32,
    )
    .map_err(|_| FirmwareBoardError::BusBuild(BusBuildError::InvalidPortRange))?;
    let mut ports = vec![PortRegionBinding {
        range: com1,
        device_index: DEV_COM1,
    }];
    // One PCI configuration device, two bindings (ECAM MMIO + conf1 PIO) with the
    // same device index, so both front ends share one function table.
    if devices[DEV_ECAM_TABLE].present() {
        let device_index = bus_devices.len();
        bus_devices.push(Box::new(pci));
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_ECAM_TABLE].base),
            device_index,
        });
        ports.push(PortRegionBinding {
            range: PortRange::new(PortAddress::new(PCI_CONF1_BASE_PORT), PCI_CONF1_PORT_BYTES)
                .map_err(FirmwareBoardError::BusBuild)?,
            device_index,
        });
    }
    // The PM timer reads the board's clock; the board stays its only advancer.
    let mut acpi_pm = None;
    let mut pit_handle = None;
    let mut pic_master_handle = None;
    if config.acpi_pm {
        let (device, handle) = AcpiPm::new_with_handle(clock.clone());
        ports.push(PortRegionBinding {
            range: PortRange::new(
                PortAddress::new(ACPI_PM_DESCRIPTOR.base_port),
                ACPI_PM_DESCRIPTOR.port_bytes,
            )
            .map_err(FirmwareBoardError::BusBuild)?,
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        acpi_pm = Some(handle);
    }
    if config.linux_profile {
        let mut bind = |base: u16, bytes: u32, device: Box<dyn crate::BusDevice>| {
            ports.push(PortRegionBinding {
                range: PortRange::new(PortAddress::new(base), bytes)
                    .map_err(FirmwareBoardError::BusBuild)?,
                device_index: bus_devices.len(),
            });
            bus_devices.push(device);
            Ok::<(), FirmwareBoardError>(())
        };
        // The PIT and port B read the same clock as the PM timer.
        // Counter reads (0x42) tick like port B reads (S4-37a3).
        let pit = Pit8254Devices::new_with_counter_tick(pit_clock.unwrap_or_else(|| clock.clone()));
        pit_handle = Some(pit.handle.clone());
        bind(PIT_BASE_PORT, PIT_PORT_BYTES, Box::new(pit.pit))?;
        bind(PIT_PORT_B, PIT_PORT_B_BYTES, Box::new(pit.port_b))?;
        let pair = Pic8259Pair::new();
        pic_master_handle = Some(pair.master_handle.clone());
        bind(PIC_MASTER_BASE_PORT, PIC_PORT_BYTES, Box::new(pair.master))?;
        bind(PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES, Box::new(pair.slave))?;
        bind(PIC_ELCR_BASE_PORT, PIC_ELCR_PORT_BYTES, Box::new(pair.elcr))?;
        for region in [
            AbsentPortRegion::Cmos,
            AbsentPortRegion::IoDelay,
            AbsentPortRegion::Post,
        ] {
            bind(
                region.base_port(),
                region.port_bytes(),
                Box::new(AbsentPorts::new(region)),
            )?;
        }
        // The DMA page register the kernel probes in `i8237A_init_ops` (S4-38L9w).
        bind(
            DMA_PAGE_0_PORT,
            DMA_PAGE_0_PORT_BYTES,
            Box::new(DmaPage0::new()),
        )?;
    }
    let bus = MachineBus::new(
        map,
        RamLimits::new(config.ram_bytes),
        bus_devices,
        mmio,
        ports,
    )
    .map_err(FirmwareBoardError::BusBuild)?;
    Ok(AssembledBoard {
        bus,
        uart,
        display,
        nvme,
        acpi_pm,
        pit: pit_handle,
        pic_master: pic_master_handle,
        lapic: lapic_handle,
        clock,
        handoff,
        tables,
        page_table_bytes,
        page_table_root,
        rdtsc_tick: config
            .linux_profile
            .then(|| Arc::new(AtomicBool::new(false))),
    })
}

/// The polling UART rejects THR writes until 8N1 and a divisor are set; the
/// machine presents it configured (as the flat probe does). The firmware's
/// own initialisation (FIRMWARE_BOOT.md 11) writes the same values.
fn configure_com1(bus: &mut MachineBus) -> Result<(), FirmwareBoardError> {
    for (offset, value) in [(3, 0x83), (0, 0x01), (1, 0x00), (3, 0x03)] {
        bus.write_port(
            PortAddress::new(FIRMWARE_COM1_PORT + offset),
            AccessWidth::Byte,
            value,
        )
        .map_err(FirmwareBoardError::Bus)?;
    }
    Ok(())
}

impl AssembledBoard {
    /// Fill ROM, copy the image to the RAM shadow, write the boot page tables and
    /// the handoff block, configure COM1 and build the direct Long64 entry state.
    fn load_firmware(
        self,
        image: &[u8],
        entry_offset: u64,
    ) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
        let AssembledBoard {
            mut bus,
            uart,
            display,
            nvme,
            acpi_pm,
            pit,
            pic_master,
            lapic,
            clock,
            handoff,
            tables,
            page_table_bytes,
            page_table_root,
            rdtsc_tick,
        } = self;

        // ROM is the source of truth; the shadow is a copy read back from ROM.
        let rom = GuestPhysicalAddress::new(FIRMWARE_ROM_BASE);
        bus.load_rom_bytes(rom, image)
            .map_err(FirmwareBoardError::Bus)?;
        let mut shadow = vec![0u8; FIRMWARE_IMAGE_BYTES];
        bus.read_flat_bytes(rom, &mut shadow)
            .map_err(FirmwareBoardError::Bus)?;
        bus.write_flat_bytes(
            GuestPhysicalAddress::new(FIRMWARE_LINK_BASE),
            &shadow,
            |_| {},
        )
        .map_err(FirmwareBoardError::Bus)?;
        bus.write_flat_bytes(GuestPhysicalAddress::new(page_table_root), &tables, |_| {})
            .map_err(FirmwareBoardError::Bus)?;
        bus.write_flat_bytes(
            GuestPhysicalAddress::new(FIRMWARE_HANDOFF_GPA),
            &handoff,
            |_| {},
        )
        .map_err(FirmwareBoardError::Bus)?;
        configure_com1(&mut bus)?;

        let mut state = LongModeState {
            cr3: None,
            rip: FIRMWARE_LINK_BASE + entry_offset,
            rflags: 2,
            cr0: Some(FIRMWARE_CR0),
            cr4: Some(FIRMWARE_CR4),
            sti_shadow: Some(StiShadow::Clear),
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        };
        state.gpr[Register::Rsp as usize] = FIRMWARE_BOOT_STACK_TOP;
        state.gpr[Register::Rdi as usize] = FIRMWARE_HANDOFF_GPA;
        Ok(LoadedFirmwareBoard {
            bus,
            state,
            uart,
            display,
            nvme,
            acpi_pm,
            pit,
            lapic,
            clock,
            handoff,
            page_table_bytes,
            page_table_root,
            guest_cr2: None,
            guest_efer: FIRMWARE_EFER,
            pci_config_address: 0,
            guest_star: 0,
            guest_lstar: 0,
            guest_cstar: 0,
            guest_sysenter_cs: 0,
            guest_sysenter_esp: 0,
            guest_sysenter_eip: 0,
            guest_syscall_mask: 0,
            guest_kernel_gs_base: 0,
            guest_fp: GuestFpState::initial(),
            com1_log: Vec::new(),
            rdtsc_tick,
            irq_source: Box::new(NoIrqSource),
            pic_master,
            pit_pic_source: None,
            #[cfg(test)]
            suppress_pm_tick: false,
            #[cfg(test)]
            suppress_rdtsc_tick: false,
        })
    }

    /// The handoff block of the assembled board (not written anywhere under the Linux profile);
    /// the Linux loader compares its region table with the E820 map (S4-37L6).
    pub(crate) fn handoff_block(&self) -> &[u8] {
        &self.handoff
    }

    /// Turn the assembled board into a loaded board for the Linux direct boot (S4-37L5)
    /// without writing a byte of guest RAM or ROM: no firmware shadow, no firmware tables, no
    /// handoff block (`handoff_block` is empty, `boot_page_table_bytes` is the Linux table
    /// size). COM1 is configured as for the firmware boot. `page_table_root` / `page_table_bytes` describe the tables the Linux loader writes
    /// itself, and the CPU state starts as `LongModeState::default()`; the caller writes
    /// memory through `bus_mut` and sets the entry state through `state_mut`.
    pub(crate) fn into_linux_board(
        self,
        page_table_root: u64,
        page_table_bytes: u64,
    ) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
        let AssembledBoard {
            mut bus,
            uart,
            display,
            nvme,
            acpi_pm,
            pit,
            pic_master,
            lapic,
            clock,
            rdtsc_tick,
            ..
        } = self;
        configure_com1(&mut bus)?;
        Ok(LoadedFirmwareBoard {
            bus,
            state: LongModeState::default(),
            uart,
            display,
            nvme,
            acpi_pm,
            pit,
            lapic,
            clock,
            handoff: Vec::new(),
            page_table_bytes,
            page_table_root,
            guest_cr2: None,
            guest_efer: FIRMWARE_EFER,
            pci_config_address: 0,
            guest_star: 0,
            guest_lstar: 0,
            guest_cstar: 0,
            guest_sysenter_cs: 0,
            guest_sysenter_esp: 0,
            guest_sysenter_eip: 0,
            guest_syscall_mask: 0,
            guest_kernel_gs_base: 0,
            guest_fp: GuestFpState::initial(),
            com1_log: Vec::new(),
            rdtsc_tick,
            irq_source: Box::new(NoIrqSource),
            pic_master,
            pit_pic_source: None,
            #[cfg(test)]
            suppress_pm_tick: false,
            #[cfg(test)]
            suppress_rdtsc_tick: false,
        })
    }
}

/// Validate `image` and the configuration, assemble the board and load the image
/// into it. The image is checked first, so its errors keep precedence over
/// configuration errors. See [`assemble_board`] for the injected clocks.
fn load_firmware_image_inner(
    image: &[u8],
    config: &FirmwareBoardConfig,
    pci: PciEcam,
    pit_clock: Option<VirtualClockHandle>,
    apic_timer_clock: Option<VirtualClockHandle>,
) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
    let devices = configured_board_devices(config);
    let entry_offset = validate_image(image, handoff_minor(&devices))?;
    assemble_board(config, pci, pit_clock, apic_timer_clock)?.load_firmware(image, entry_offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ACPI_PM_TMR_BLK, ACPI_PM1_CNT_SCI_EN, ACPI_PM1_CNT_SLP_EN, ACPI_PM1A_CNT_BLK,
        ACPI_PM1A_EVT_BLK, ACPI_RESET_REG, ACPI_RESET_VALUE, ACPI_SLP_TYP_S5, DeviceError,
        PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES, Type0Config,
    };
    use od_isa::memory::ArchitecturalFault;

    // Independent reference for `reference_config()`, generated by a separate
    // script straight from the tables of FIRMWARE_BOOT.md sections 2, 4 and 6
    // (12 regions, `N = 91`, total_size 0x370).
    const REFERENCE_HANDOFF_HEX: &str = concat!(
        "4f4448414e444f4601000000a000000070030000544ffedf000000000100000000ca9a3b000000000100000000000000",
        "a00000000c00000020000000200200000700000030000000101112131415161718191a1b1c1d1e1f0700000000000000",
        "4f442d303030310000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "00000000000000000000000000000000010000000000000000000000000000000000100000000000ffffffff00000000",
        "070000000000000000001000000000000000010000000000ffffffff0000000008000000000000000000110000000000",
        "0000040000000000ffffffff000000000100000000000000000015000000000000000b0000000000ffffffff00000000",
        "090000000000000000002000000000000000400000000000ffffffff000000000a000000000000000000600000000000",
        "00b0050000000000ffffffff00000000010000000000000000b065000000000000509a0700000000ffffffff00000000",
        "0600000000000000000000f800000000000040020000000003000000000000000300000000000000000000fc00000000",
        "0010000000000000030000000000000003000000000000000000c0fe0000000000100000000000000200000000000000",
        "03000000000000000000e0fe000000000010000000000000010000000000000004000000000000000000c0ff00000000",
        "0000400000000000ffffffff000000000100000003000000f80300000000000008000000000000000400000000000000",
        "0100000000000000000000000000000002000000010000000000e0fe000000000010000000000000ffffffff00000000",
        "0000000000000000000000000000000003000000010000000000c0fe000000000010000000000000ffffffff00000000",
        "000000000000000000000000000000000400000001000000000000fc000000000010000000000000ffffffff00000000",
        "00000000000000000000000000000000050000000000000000000000000000000000000000000000ffffffff00000000",
        "00000000000000000000000000000000060000000000000000000000000000000000000000000000ffffffff00000000",
        "00000000000000000000000000000000070000000000000000000000000000000000000000000000ffffffff00000000",
        "00000000000000000000000000000000"
    );

    fn reference_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            system_uuid: std::array::from_fn(|index| 0x10 + index as u8),
            serial: b"OD-0001".to_vec(),
            ..FirmwareBoardConfig::default()
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// A valid `ODFWIMG1` image whose code starts at `entry_offset = 0x40`.
    fn test_image(code: &[u8], patches: &[(usize, &[u8])]) -> Vec<u8> {
        let mut image = vec![0u8; FIRMWARE_IMAGE_BYTES];
        image[..8].copy_from_slice(IMAGE_MAGIC);
        image[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
        image[0x0C..0x10].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x10..0x18].copy_from_slice(&(FIRMWARE_IMAGE_BYTES as u64).to_le_bytes());
        image[0x18..0x20].copy_from_slice(&FIRMWARE_LINK_BASE.to_le_bytes());
        image[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        image[0x28..0x2A].copy_from_slice(&1u16.to_le_bytes());
        image[0x40..0x40 + code.len()].copy_from_slice(code);
        for (offset, bytes) in patches {
            image[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        seal(&mut image);
        image
    }

    fn seal(image: &mut [u8]) {
        image[IMAGE_CRC_OFFSET..IMAGE_CRC_OFFSET + 4].copy_from_slice(&[0; 4]);
        let crc = crc32_ieee(&[image]);
        image[IMAGE_CRC_OFFSET..IMAGE_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    }

    fn hlt_image() -> Vec<u8> {
        test_image(&[0xf4], &[])
    }

    fn load(image: &[u8]) -> LoadedFirmwareBoard {
        load_firmware_image(image, &reference_config()).unwrap()
    }

    #[test]
    fn crc32_matches_the_contract_check_value() {
        assert_eq!(crc32_ieee(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32_ieee(&[b"1234", b"56789"]), 0xCBF4_3926);
    }

    #[test]
    fn handoff_block_is_byte_identical_to_the_reference() {
        let mut board = load(&hlt_image());
        assert_eq!(hex(board.handoff_block()), REFERENCE_HANDOFF_HEX);
        assert_eq!(board.boot_page_table_bytes(), 91 * 4096);
        // The guest sees exactly these bytes at GPA 0x0010_0000.
        let mut ram = vec![0u8; board.handoff_block().len()];
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_HANDOFF_GPA), &mut ram)
            .unwrap();
        assert_eq!(ram, board.handoff_block());
    }

    #[test]
    fn entry_state_follows_the_contract() {
        let board = load(&hlt_image());
        let state = board.state();
        assert_eq!(state.rip, 0x0020_0040);
        assert_eq!(state.cr0, Some(0x8001_0033));
        assert_eq!(state.cr4, Some(0x20));
        assert_eq!(state.cr3, None);
        assert_eq!(board.cr3(), 0x0060_0000);
        assert_eq!(board.efer(), 0xd00);
        assert_eq!(state.rflags, 2);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x0015_0000);
        assert_eq!(state.gpr[Register::Rdi as usize], 0x0010_0000);
        for (index, value) in state.gpr.iter().enumerate() {
            if index != Register::Rsp as usize && index != Register::Rdi as usize {
                assert_eq!(*value, 0, "gpr {index}");
            }
        }
        assert_eq!(state.activity, Some(CpuActivity::Running));
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    }

    #[test]
    fn map_regions_are_disjoint_and_the_handoff_table_covers_exactly_the_map() {
        for (ram_bytes, second_screen) in [
            (FIRMWARE_MIN_RAM_BYTES, false),
            (FIRMWARE_MIN_RAM_BYTES, true),
            (4 << 30, true),
            (FIRMWARE_MAX_RAM_BYTES, true),
        ] {
            let config = FirmwareBoardConfig {
                ram_bytes,
                second_screen,
                ..FirmwareBoardConfig::default()
            };
            let devices = board_devices(false, false, false, true);
            // `MachineMemoryMap::new` itself rejects overlaps and width overflow.
            let map = board_map(&config, &devices).unwrap();
            let table_bytes = boot_page_table_count(&map) * PAGE;
            let table = handoff_regions(&map, &devices, table_bytes);
            assert!(table.len() <= MAX_REGIONS);
            for pair in table.windows(2) {
                assert!(pair[0].base + pair[0].length <= pair[1].base, "{pair:?}");
            }
            // Merge adjacent handoff entries per map region and compare.
            for region in map.regions() {
                let start = region.range.start().get();
                let end = start + region.range.len();
                let covered: u64 = table
                    .iter()
                    .filter(|entry| entry.base >= start && entry.base + entry.length <= end)
                    .map(|entry| entry.length)
                    .sum();
                assert_eq!(covered, end - start, "region {start:#x}");
            }
            let total: u64 = table.iter().map(|entry| entry.length).sum();
            assert_eq!(
                total,
                map.regions().iter().map(|r| r.range.len()).sum::<u64>()
            );
            let low_ram_end = map
                .regions()
                .iter()
                .filter(|r| r.kind == MemoryRegionKind::Ram && r.range.start().get() == 0)
                .map(|r| r.range.len())
                .next()
                .unwrap();
            assert!(low_ram_end <= 0xF400_0000);
            assert!(
                FIRMWARE_BOOT_PAGE_TABLES_GPA + table_bytes <= low_ram_end,
                "boot tables fit in low RAM"
            );
            // Windows of FIRMWARE_BOOT.md section 15 never intersect.
            for entry in &table {
                if entry.kind == REGION_ROM {
                    assert_eq!(entry.base + entry.length, 1 << 32);
                }
                if entry.kind == REGION_RAM && entry.base < (1 << 32) {
                    assert!(entry.base + entry.length <= 0xF400_0000);
                }
            }
        }
    }

    #[test]
    fn page_table_count_matches_the_documented_values() {
        // FIRMWARE_BOOT.md section 4: both screens, NVRAM and TPM CRB present.
        let count = |ram_bytes: u64| {
            let low_end = ram_bytes.min(FIRMWARE_LOW_RAM_LIMIT);
            let mut specs = vec![MemoryRegionSpec::new(
                range(0, low_end).unwrap(),
                MemoryRegionKind::Ram,
            )];
            if ram_bytes > low_end {
                specs.push(MemoryRegionSpec::new(
                    range(FIRMWARE_HIGH_RAM_BASE, ram_bytes - low_end).unwrap(),
                    MemoryRegionKind::Ram,
                ));
            }
            for (base, len, kind) in [
                (0xF400_0000, 0x0240_0000, MemoryRegionKind::Framebuffer),
                (0xF800_0000, 0x0240_0000, MemoryRegionKind::Framebuffer),
                (0xFC00_0000, 0x1000, MemoryRegionKind::Mmio),
                (0xFEC0_0000, 0x1000, MemoryRegionKind::Mmio),
                (0xFED4_0000, 0x5000, MemoryRegionKind::Mmio),
                (0xFEE0_0000, 0x1000, MemoryRegionKind::Mmio),
                (0xFF00_0000, 0x4_1000, MemoryRegionKind::Mmio),
                (0xFFC0_0000, 0x40_0000, MemoryRegionKind::Rom),
            ] {
                specs.push(MemoryRegionSpec::new(range(base, len).unwrap(), kind));
            }
            boot_page_table_count(&MachineMemoryMap::new(36, specs).unwrap())
        };
        assert_eq!(count(128 << 20), 110);
        assert_eq!(count(4 << 30), 2097);
        assert_eq!(count(FIRMWARE_MAX_RAM_BYTES), 32780);
    }

    #[test]
    fn boot_tables_are_4k_identity_with_the_documented_leaf_flags() {
        let mut board = load(&hlt_image());
        let mut read = |address: u64| {
            board
                .bus_mut()
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        };
        // Walk: only present, non-PS entries; return the leaf (None if not present).
        let mut walk = |address: u64| {
            let mut table = FIRMWARE_BOOT_PAGE_TABLES_GPA;
            for shift in [39u32, 30, 21, 12] {
                let entry = read(table + ((address >> shift) & 511) * 8);
                if entry & 1 == 0 {
                    return None;
                }
                if shift == 12 {
                    return Some(entry);
                }
                assert_eq!(entry & !0x000f_ffff_ffff_f000, 0x3, "intermediate P|RW");
                table = entry & 0x000f_ffff_ffff_f000;
            }
            unreachable!()
        };
        assert_eq!(walk(0x0020_0000), Some(0x0020_0000 | 0x3));
        assert_eq!(walk(0x0010_0000), Some(0x0010_0000 | 0x3));
        assert_eq!(walk(0x0800_0000 - 0x1000), Some(0x0800_0000 - 0x1000 + 0x3));
        assert_eq!(walk(0x0800_0000), None, "hole above RAM");
        assert_eq!(walk(0xF800_0000), Some(0xF800_0000 | 0x3 | NX));
        assert_eq!(walk(0xFC00_0000), Some(0xFC00_0000 | 0x3 | NX));
        assert_eq!(walk(0xFEC0_0000), Some(0xFEC0_0000 | 0x3 | NX));
        assert_eq!(walk(0xFEE0_0000), Some(0xFEE0_0000 | 0x3 | NX));
        assert_eq!(walk(0xFFC0_0000), Some(0xFFC0_0000 | 0x1 | NX));
        assert_eq!(walk(0xFFFF_F000), Some(0xFFFF_F000 | 0x1 | NX));
        assert_eq!(
            walk(0xFF00_0000),
            None,
            "NVRAM window is absent until S7-10"
        );
        assert_eq!(
            walk(0xF400_0000),
            None,
            "second screen absent in this config"
        );
        assert_eq!(walk(0xFED4_0000), None, "TPM CRB absent until S7-22");
    }

    #[test]
    fn rom_holds_the_image_and_the_shadow_is_a_copy() {
        let image = test_image(&[0xf4], &[(0x1234, &[0xde, 0xad, 0xbe, 0xef])]);
        let mut board = load(&image);
        let bus = board.bus_mut();
        let mut rom = vec![0u8; FIRMWARE_IMAGE_BYTES];
        bus.read_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_ROM_BASE), &mut rom)
            .unwrap();
        assert!(rom == image, "ROM bytes equal the image");
        let mut shadow = vec![0u8; FIRMWARE_IMAGE_BYTES];
        bus.read_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_LINK_BASE), &mut shadow)
            .unwrap();
        assert!(shadow == image, "shadow equals ROM");
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(FIRMWARE_ROM_BASE + 0x1234),
                AccessWidth::Dword
            )
            .unwrap(),
            0xefbe_adde
        );
        // Guest writes to ROM are a backend error, never silently ignored.
        for width in [AccessWidth::Byte, AccessWidth::Qword] {
            assert!(matches!(
                bus.write_physical(
                    GuestPhysicalAddress::new(FIRMWARE_ROM_BASE + 0x1234),
                    width,
                    0,
                    |_| {}
                ),
                Err(BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(
                    MemoryRegionKind::Rom
                )))
            ));
        }
        assert!(matches!(
            bus.write_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_ROM_BASE), &[0], |_| {}),
            Err(BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(
                MemoryRegionKind::Rom
            )))
        ));
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(FIRMWARE_ROM_BASE + 0x1234),
                AccessWidth::Dword
            )
            .unwrap(),
            0xefbe_adde
        );
    }

    #[test]
    fn short_buffers_return_an_error_instead_of_panicking() {
        assert_eq!(le_u16(&[1, 2], 0), Ok(0x0201));
        assert_eq!(le_u32(&[1, 2, 3, 4], 0), Ok(0x0403_0201));
        assert_eq!(
            le_u32(&[1, 2, 3], 0),
            Err(FirmwareBoardError::ImageTruncated { offset: 0, len: 3 })
        );
        assert_eq!(
            le_u64(&[0; 8], 1),
            Err(FirmwareBoardError::ImageTruncated { offset: 1, len: 8 })
        );
        assert_eq!(
            le_u16(&[0; 4], usize::MAX),
            Err(FirmwareBoardError::ImageTruncated {
                offset: usize::MAX,
                len: 4
            })
        );
    }

    #[test]
    fn invalid_images_and_configs_are_rejected_before_any_guest_runs() {
        let config = FirmwareBoardConfig::default();
        let expect = |image: &[u8], error: FirmwareBoardError| {
            assert_eq!(load_firmware_image(image, &config).err(), Some(error));
        };
        expect(
            &vec![0; FIRMWARE_IMAGE_BYTES - 1],
            FirmwareBoardError::ImageSize {
                expected: FIRMWARE_IMAGE_BYTES,
                actual: FIRMWARE_IMAGE_BYTES - 1,
            },
        );
        expect(
            &vec![0; FIRMWARE_IMAGE_BYTES + 1],
            FirmwareBoardError::ImageSize {
                expected: FIRMWARE_IMAGE_BYTES,
                actual: FIRMWARE_IMAGE_BYTES + 1,
            },
        );
        let field = |image: Vec<u8>, name: &'static str| {
            let mut image = image;
            seal(&mut image);
            match load_firmware_image(&image, &config).err() {
                Some(FirmwareBoardError::ImageHeader { field, .. }) => assert_eq!(field, name),
                other => panic!("{name}: {other:?}"),
            }
        };
        let mutate = |offset: usize, bytes: &[u8]| {
            let mut image = hlt_image();
            image[offset..offset + bytes.len()].copy_from_slice(bytes);
            image
        };
        field(mutate(0, b"ODFWIMG2"), "magic");
        field(mutate(0x08, &2u32.to_le_bytes()), "format_version");
        field(mutate(0x0C, &0x80u32.to_le_bytes()), "header_size");
        field(mutate(0x10, &0x1000u64.to_le_bytes()), "image_size");
        field(mutate(0x18, &0x0030_0000u64.to_le_bytes()), "link_base");
        field(mutate(0x20, &0x3fu64.to_le_bytes()), "entry_offset");
        field(
            mutate(0x20, &(FIRMWARE_IMAGE_BYTES as u64).to_le_bytes()),
            "entry_offset",
        );
        field(mutate(0x28, &2u16.to_le_bytes()), "handoff_major");
        field(mutate(0x2C, &1u32.to_le_bytes()), "flags");
        field(mutate(0x3F, &[1]), "reserved");
        // CRC mismatch: corrupt a payload byte without resealing.
        let mut corrupt = hlt_image();
        corrupt[0x1000] ^= 1;
        assert!(matches!(
            load_firmware_image(&corrupt, &config).err(),
            Some(FirmwareBoardError::ImageCrc { .. })
        ));
        let bad_config = |config: FirmwareBoardConfig| {
            assert!(matches!(
                load_firmware_image(&hlt_image(), &config).err(),
                Some(FirmwareBoardError::Config(_))
            ));
        };
        bad_config(FirmwareBoardConfig {
            ram_bytes: 127 << 20,
            ..FirmwareBoardConfig::default()
        });
        bad_config(FirmwareBoardConfig {
            ram_bytes: (128 << 20) + 0x1000,
            ..FirmwareBoardConfig::default()
        });
        bad_config(FirmwareBoardConfig {
            ram_bytes: FIRMWARE_MAX_RAM_BYTES + (2 << 20),
            ..FirmwareBoardConfig::default()
        });
        bad_config(FirmwareBoardConfig {
            tsc_hz: 0,
            ..FirmwareBoardConfig::default()
        });
        bad_config(FirmwareBoardConfig {
            serial: vec![b'x'; 65],
            ..FirmwareBoardConfig::default()
        });
        bad_config(FirmwareBoardConfig {
            serial: vec![0x7f],
            ..FirmwareBoardConfig::default()
        });
    }

    #[test]
    fn instruction_fetch_from_rom_is_a_page_fault_by_nx() {
        let mut board = load(&hlt_image());
        board.state_mut().rip = FIRMWARE_ROM_BASE + 0x40;
        let report = board.run(1, || false, |_| {});
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(
            report.exit,
            PagedProbeExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0x11),
                linear_address: Some(FIRMWARE_ROM_BASE + 0x40),
            })
        );
        assert!(report.memory_error.is_none(), "not a backend error");
        assert!(report.page_fault.is_some());
    }

    #[test]
    fn shadow_image_runs_and_com1_bytes_reach_the_transcript_across_quanta() {
        // MOV RAX,[RBX]; OUT DX,AL x3; HLT. The registers are test-seeded
        // because the entry contract zeroes every GPR except RSP and RDI.
        let image = test_image(
            &[0x48, 0x8b, 0x03, 0xee, 0xee, 0xee, 0xf4],
            &[(0x100, b"X\0\0\0\0\0\0\0")],
        );
        let mut board = load(&image);
        board.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_LINK_BASE + 0x100;
        board.state_mut().gpr[Register::Rdx as usize] = u64::from(FIRMWARE_COM1_PORT);
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 5);
        assert_eq!(report.quanta, 4, "one quantum per THR write plus the HLT");
        assert_eq!(report.com1, b"XXX");
        assert_eq!(report.virtual_ns, 5, "retired * 1 ns");
        assert_eq!(report.tsc_hz, 1_000_000_000);
        assert_eq!(board.clock().now_ns(), 5);
    }

    #[test]
    fn budget_exhaustion_is_reported_and_time_still_advances() {
        // Endless `OUT DX,AL` loop is not expressible; use a two-OUT image and a budget of 2.
        let image = test_image(
            &[0x48, 0x8b, 0x03, 0xee, 0xee, 0xf4],
            &[(0x100, b"Y\0\0\0\0\0\0\0")],
        );
        let mut board = load(&image);
        board.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_LINK_BASE + 0x100;
        board.state_mut().gpr[Register::Rdx as usize] = u64::from(FIRMWARE_COM1_PORT);
        let report = board.run_boot(2).unwrap();
        assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(report.retired, 2);
        assert_eq!(report.com1, b"Y");
        assert_eq!(report.virtual_ns, 2);
    }

    /// S7-03: the base paged probe admits `MOV r8,imm8` (`B0+r ib`) and
    /// `66 MOV r16,imm16` (`66 B8+r iw`), so the smoke image can set AL and DX
    /// itself from the all-zero GPR entry state and print `X` on COM1.
    #[test]
    fn com1_bytes_survive_a_failed_run() {
        // OUT DX,AL; HLT: the byte is recorded, then the clock overflows.
        let mut board = load(&test_image(&[0xee, 0xf4], &[]));
        board.state_mut().gpr[Register::Rax as usize] = u64::from(b'Z');
        board.state_mut().gpr[Register::Rdx as usize] = u64::from(FIRMWARE_COM1_PORT);
        board.advance_time_ns(u64::MAX).unwrap();
        assert!(matches!(
            board.run_boot(100),
            Err(FirmwareBoardError::Clock(_))
        ));
        assert_eq!(board.com1_transcript(), b"Z");
    }

    #[test]
    fn smoke_code_writes_x_to_com1_with_zero_gprs_at_entry() {
        let image = test_image(&[0xb0, b'X', 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4], &[]);
        let mut board = load(&image);
        for (index, value) in board.state().gpr.iter().enumerate() {
            if index != Register::Rsp as usize && index != Register::Rdi as usize {
                assert_eq!(*value, 0, "gpr {index} must be zero at entry");
            }
        }
        let report = board.run_boot(10).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 4);
        assert_eq!(report.com1, b"X");
        assert_eq!(report.virtual_ns, 4);
    }

    #[test]
    fn second_screen_adds_a_framebuffer_region_and_still_builds_the_bus() {
        let config = FirmwareBoardConfig {
            second_screen: true,
            ..reference_config()
        };
        let board = load_firmware_image(&hlt_image(), &config).unwrap();
        let block = board.handoff_block();
        assert_eq!(
            le_u32(block, 0x34).unwrap(),
            13,
            "13 regions with two screens"
        );
        assert_eq!(le_u32(block, 0x3C).unwrap(), 0xA0 + 32 * 13);
        let region = |index: usize| &block[0xA0 + 32 * index..0xA0 + 32 * (index + 1)];
        let framebuffers: Vec<(u64, u32)> = (0..13)
            .filter(|&i| le_u32(region(i), 0).unwrap() == REGION_FRAMEBUFFER)
            .map(|i| (le_u64(region(i), 8).unwrap(), le_u32(region(i), 4).unwrap()))
            .collect();
        assert_eq!(
            framebuffers,
            vec![(0xF400_0000, 1), (0xF800_0000, 0)],
            "attrs carry the screen number"
        );
    }

    fn nvme_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            nvme: Some(NvmeConfig::default()),
            ..reference_config()
        }
    }

    /// An image that declares support for handoff minor 1.
    fn nvme_image(code: &[u8]) -> Vec<u8> {
        test_image(code, &[(0x2A, &1u16.to_le_bytes())])
    }

    #[test]
    fn s5_nvme_stand_omits_display_and_keeps_fixed_nvme_slot_and_window() {
        let config = FirmwareBoardConfig::s5_nvme_stand();
        assert_eq!(config.ram_bytes, 128 << 20);
        assert!(config.nvme.is_some());
        assert!(config.acpi_pm);
        assert!(!config.pci_ecam && !config.acpi_tables && !config.linux_profile);
        assert!(!config.display_enabled && !config.second_screen);

        let devices = configured_board_devices(&config);
        let map = board_map(&config, &devices).unwrap();
        let window = map
            .regions()
            .iter()
            .find(|region| region.range.start().get() == NVME_MMIO_BASE)
            .expect("stand maps the fixed NVMe window");
        assert_eq!(window.range.len(), NVME_MMIO_BYTES);
        for region in map.regions() {
            if region.range.start().get() == NVME_MMIO_BASE {
                continue;
            }
            let start = region.range.start().get();
            let end = start + region.range.len();
            assert!(
                end <= NVME_MMIO_BASE || start >= NVME_MMIO_BASE + NVME_MMIO_BYTES,
                "NVMe window overlaps {region:?}"
            );
        }
        for display_range in [
            (DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
            (DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
            (DISPLAY_SECOND_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
        ] {
            assert!(!map.regions().iter().any(|region| {
                region.range.start().get() == display_range.0
                    && region.range.len() == display_range.1
            }));
        }
        let nvme_start = NVME_MMIO_BASE;
        let nvme_end = NVME_MMIO_BASE + NVME_MMIO_BYTES;
        for (base, length) in [
            (u64::from(FIRMWARE_COM1_PORT), 8),
            (
                u64::from(PCI_CONF1_BASE_PORT),
                u64::from(PCI_CONF1_PORT_BYTES),
            ),
            (
                u64::from(ACPI_PM_DESCRIPTOR.base_port),
                u64::from(ACPI_PM_DESCRIPTOR.port_bytes),
            ),
        ] {
            assert!(nvme_end <= base || base + length <= nvme_start);
        }

        let board = load_firmware_image(&nvme_image(&[0xf4]), &config).unwrap();
        let handoff = board.handoff_block();
        assert_eq!(le_u32(handoff, 0x40).unwrap(), 8);
        assert_eq!(le_u32(device_entry(handoff, DEV_DISPLAY), 0).unwrap(), 4);
        assert_eq!(le_u32(device_entry(handoff, DEV_DISPLAY), 4).unwrap(), 0);
        assert_eq!(le_u64(device_entry(handoff, DEV_DISPLAY), 8).unwrap(), 0);
        assert_eq!(le_u32(device_entry(handoff, DEV_NVME_TABLE), 0).unwrap(), 8);
        assert_eq!(
            le_u64(device_entry(handoff, DEV_NVME_TABLE), 8).unwrap(),
            NVME_MMIO_BASE
        );
        assert!(!board.nvme().unwrap().is_ready());
    }

    #[test]
    fn s5_stand_without_nvme_keeps_seven_base_entries_and_default_display_is_compatible() {
        let image = nvme_image(&[0xf4]);
        let mut no_nvme = FirmwareBoardConfig::s5_nvme_stand();
        no_nvme.nvme = None;
        let board = load_firmware_image(&image, &no_nvme).unwrap();
        assert_eq!(le_u32(board.handoff_block(), 0x40).unwrap(), 7);
        assert!(board.nvme().is_none());
        assert_eq!(
            le_u32(device_entry(board.handoff_block(), DEV_DISPLAY), 4).unwrap(),
            0
        );

        let default_board = load(&hlt_image());
        assert!(FirmwareBoardConfig::default().display_enabled);
        assert_eq!(
            le_u32(device_entry(default_board.handoff_block(), DEV_DISPLAY), 4).unwrap(),
            DEVICE_PRESENT
        );
    }

    #[test]
    fn nvme_entry_extends_the_handoff_to_v1_1_and_binds_the_window() {
        let mut board = load_firmware_image(&nvme_image(&[0xf4]), &nvme_config()).unwrap();
        let block = board.handoff_block().to_vec();
        assert_eq!(le_u16(&block, 0x0A).unwrap(), 1, "version_minor");
        assert_eq!(le_u32(&block, 0x34).unwrap(), 13, "one more MMIO region");
        assert_eq!(le_u32(&block, 0x40).unwrap(), 8, "device_count");
        let device_table = le_u32(&block, 0x3C).unwrap() as usize;
        assert_eq!(device_table, 0xA0 + 32 * 13);
        assert_eq!(
            le_u32(&block, 0x10).unwrap() as usize,
            device_table + 48 * 8
        );
        assert_eq!(block.len(), device_table + 48 * 8);
        let entry = &block[device_table + 7 * 48..device_table + 8 * 48];
        assert_eq!(le_u32(entry, 0x00).unwrap(), 8, "device code");
        assert_eq!(le_u32(entry, 0x04).unwrap(), 1, "PRESENT, MMIO");
        assert_eq!(le_u64(entry, 0x08).unwrap(), 0xFC00_1000);
        assert_eq!(le_u64(entry, 0x10).unwrap(), 0x2000);
        assert_eq!(le_u32(entry, 0x18).unwrap(), 0xFFFF_FFFF, "no IRQ route");
        assert_eq!(le_u32(entry, 0x1C).unwrap(), 0);
        // The window is an MMIO region owned by the new entry.
        let regions: Vec<&[u8]> = (0..13)
            .map(|i| &block[0xA0 + 32 * i..0xA0 + 32 * (i + 1)])
            .collect();
        let window = regions
            .iter()
            .find(|region| le_u64(region, 8).unwrap() == NVME_MMIO_BASE)
            .expect("NVMe window region");
        assert_eq!(le_u32(window, 0).unwrap(), REGION_MMIO);
        assert_eq!(le_u64(window, 0x10).unwrap(), NVME_MMIO_BYTES);
        assert_eq!(le_u32(window, 0x18).unwrap(), 7, "owner_device");
        // The block is self-consistent: CRC over the whole block with the field zeroed.
        let mut zeroed = block.clone();
        zeroed[0x14..0x18].copy_from_slice(&[0; 4]);
        assert_eq!(le_u32(&block, 0x14).unwrap(), crc32_ieee(&[&zeroed]));
        // The device is on the bus and reads CAP.MQES.
        assert!(board.nvme().is_some());
        let cap = board
            .bus_mut()
            .read_physical(
                GuestPhysicalAddress::new(NVME_MMIO_BASE),
                AccessWidth::Qword,
            )
            .unwrap();
        assert_eq!(cap & 0xFFFF, 1023);
    }

    #[test]
    fn without_the_nvme_entry_the_window_is_not_mapped() {
        let mut board = load(&hlt_image());
        assert!(board.nvme().is_none());
        assert!(
            board
                .bus_mut()
                .read_physical(
                    GuestPhysicalAddress::new(NVME_MMIO_BASE),
                    AccessWidth::Dword
                )
                .is_err()
        );
    }

    #[test]
    fn nvme_needs_an_image_that_understands_handoff_minor_1_and_a_valid_config() {
        assert_eq!(
            load_firmware_image(&hlt_image(), &nvme_config()).err(),
            Some(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 0
            })
        );
        let config = FirmwareBoardConfig {
            nvme: Some(NvmeConfig {
                namespace_sectors: 0,
                ..NvmeConfig::default()
            }),
            ..reference_config()
        };
        assert_eq!(
            load_firmware_image(&nvme_image(&[0xf4]), &config).err(),
            Some(FirmwareBoardError::Nvme(NvmeConfigError::EmptyNamespace))
        );
    }

    // ---- S4-30c: PCI configuration device on the board (opt-in) ----

    fn ecam_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            pci_ecam: true,
            ..reference_config()
        }
    }

    fn ecam_at(offset: u64) -> GuestPhysicalAddress {
        GuestPhysicalAddress::new(PCI_ECAM_DESCRIPTOR.base + offset)
    }

    fn ecam_off(device: u64, function: u64, register: u64) -> u64 {
        (device << 15) | (function << 12) | register
    }

    fn device_entry(block: &[u8], index: usize) -> &[u8] {
        let table = le_u32(block, 0x3C).unwrap() as usize;
        &block[table + index * 48..table + (index + 1) * 48]
    }

    fn regions_of(block: &[u8]) -> Vec<&[u8]> {
        let count = le_u32(block, 0x34).unwrap() as usize;
        (0..count)
            .map(|i| &block[0xA0 + 32 * i..0xA0 + 32 * (i + 1)])
            .collect()
    }

    /// Diagnostic-only endpoint at 00:03.0 (not declared by the board).
    fn pci_with_diagnostic_endpoint() -> PciEcam {
        let mut pci = PciEcam::new();
        pci.add_function(
            0,
            3,
            0,
            Box::new(Type0Config::new(0xD1A6, 1, 1, 0xFF_0000, Some(4096)).unwrap()),
        )
        .unwrap();
        pci
    }

    #[test]
    fn pci_ecam_is_off_by_default_and_the_default_handoff_is_unchanged() {
        assert!(!FirmwareBoardConfig::default().pci_ecam);
        let mut board = load(&hlt_image());
        // Byte for byte the reference of the previous slices (golden test above).
        assert_eq!(hex(board.handoff_block()), REFERENCE_HANDOFF_HEX);
        let absent = device_entry(board.handoff_block(), DEV_ECAM_TABLE).to_vec();
        assert_eq!(le_u32(&absent, 0).unwrap(), 7);
        assert_eq!(le_u32(&absent, 4).unwrap(), 0, "PRESENT = 0");
        assert!(absent[8..0x18].iter().all(|&b| b == 0));
        assert_eq!(le_u32(&absent, 0x18).unwrap(), NO_GSI);
        // No window and no conf1 ports without the flag.
        assert!(
            board
                .bus_mut()
                .read_physical(ecam_at(0), AccessWidth::Dword)
                .is_err()
        );
        assert!(
            board
                .bus_mut()
                .read_port(PortAddress::new(PCI_CONF1_BASE_PORT), AccessWidth::Dword)
                .is_err()
        );
    }

    #[test]
    fn pci_ecam_flag_makes_code_7_present_inside_handoff_v1_0() {
        let off = load(&hlt_image());
        let on = load_firmware_image(&hlt_image(), &ecam_config()).unwrap();
        let (a, b) = (off.handoff_block(), on.handoff_block());
        // Still v1.0: the code-7 slot exists since the first minor.
        assert_eq!(le_u16(b, 0x0A).unwrap(), 0, "version_minor");
        assert_eq!(le_u32(b, 0x40).unwrap(), 7, "device_count");
        assert_eq!(le_u32(b, 0x34).unwrap(), le_u32(a, 0x34).unwrap() + 1);
        assert_eq!(le_u32(b, 0x10).unwrap() as usize, b.len());
        assert_eq!(b.len(), a.len() + 32);
        // Entry 7: PRESENT, MMIO, base/length of the descriptor, segment 0, buses 0..=0.
        let entry = device_entry(b, DEV_ECAM_TABLE);
        let mut expected = [0u8; 48];
        expected[0..4].copy_from_slice(&7u32.to_le_bytes());
        expected[4..8].copy_from_slice(&1u32.to_le_bytes());
        expected[8..16].copy_from_slice(&0xFC10_0000u64.to_le_bytes());
        expected[16..24].copy_from_slice(&0x10_0000u64.to_le_bytes());
        expected[24..28].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(hex(entry), hex(&expected), "param0 = 0, param1 = 0");
        // The other six entries are unchanged.
        for index in (0..7).filter(|&i| i != DEV_ECAM_TABLE) {
            assert_eq!(device_entry(a, index), device_entry(b, index), "{index}");
        }
        // One extra MMIO region owned by the entry (index 6); the rest is identical.
        let (ra, rb) = (regions_of(a), regions_of(b));
        let added: Vec<_> = rb.iter().filter(|r| !ra.contains(r)).collect();
        assert_eq!(added.len(), 1);
        let region = added[0];
        assert_eq!(le_u32(region, 0).unwrap(), REGION_MMIO);
        assert_eq!(le_u64(region, 8).unwrap(), 0xFC10_0000);
        assert_eq!(le_u64(region, 0x10).unwrap(), 0x10_0000);
        assert_eq!(le_u32(region, 0x18).unwrap(), DEV_ECAM_TABLE as u32);
        assert!(ra.iter().all(|r| rb.contains(r)));
        // The same 2 MiB page-table block as the display registers: N is unchanged.
        assert_eq!(on.boot_page_table_bytes(), off.boot_page_table_bytes());
        let mut zeroed = b.to_vec();
        zeroed[0x14..0x18].copy_from_slice(&[0; 4]);
        assert_eq!(le_u32(b, 0x14).unwrap(), crc32_ieee(&[&zeroed]));
        // An image that only knows minor 0 is enough: no minor bump.
        assert!(load_firmware_image(&hlt_image(), &ecam_config()).is_ok());
    }

    #[test]
    fn pci_ecam_with_nvme_keeps_both_entries_and_distinct_bus_indices() {
        let config = FirmwareBoardConfig {
            pci_ecam: true,
            ..nvme_config()
        };
        let mut board = load_firmware_image(&nvme_image(&[0xf4]), &config).unwrap();
        let block = board.handoff_block().to_vec();
        assert_eq!(le_u16(&block, 0x0A).unwrap(), 1);
        assert_eq!(le_u32(&block, 0x40).unwrap(), 8);
        assert_eq!(le_u32(device_entry(&block, 6), 4).unwrap(), 1);
        assert_eq!(le_u64(device_entry(&block, 7), 8).unwrap(), NVME_MMIO_BASE);
        let cap = board
            .bus_mut()
            .read_physical(
                GuestPhysicalAddress::new(NVME_MMIO_BASE),
                AccessWidth::Qword,
            )
            .unwrap();
        assert_eq!(cap & 0xFFFF, 1023, "NVMe still answers");
        assert_eq!(
            board
                .bus_mut()
                .read_physical(ecam_at(0), AccessWidth::Dword),
            Ok(0xFFFF_FFFF)
        );
    }

    #[test]
    fn ecam_window_is_non_ram_mmio_and_disjoint_from_every_other_region() {
        let config = FirmwareBoardConfig {
            second_screen: true,
            nvme: Some(NvmeConfig::default()),
            ram_bytes: FIRMWARE_MAX_RAM_BYTES,
            pci_ecam: true,
            ..FirmwareBoardConfig::default()
        };
        let devices = board_devices(true, true, false, true);
        let map = board_map(&config, &devices).unwrap();
        let ecam = PCI_ECAM_DESCRIPTOR;
        let (start, end) = (ecam.base, ecam.base + ecam.size);
        assert!(end <= 1 << 32, "below 4 GiB");
        assert!(
            end <= 1u64 << map.physical_address_width(),
            "inside physical36"
        );
        let mut found = 0;
        for region in map.regions() {
            let r_start = region.range.start().get();
            let r_end = r_start + region.range.len();
            if r_start == start {
                found += 1;
                assert_eq!(region.kind, MemoryRegionKind::Mmio);
                assert_eq!(r_end, end);
            } else {
                assert!(r_end <= start || r_start >= end, "{region:?} overlaps ECAM");
            }
        }
        assert_eq!(found, 1);
        // Explicit neighbours named by the contract.
        for (base, len) in [
            (DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
            (DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
            (DISPLAY_SECOND_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
            (NVME_MMIO_BASE, NVME_MMIO_BYTES),
            (IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE),
            (LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE),
            (FIRMWARE_ROM_BASE, FIRMWARE_IMAGE_BYTES as u64),
        ] {
            assert!(base + len <= start || base >= end, "{base:#x}");
        }
        assert!(
            map.regions()
                .iter()
                .filter(|r| r.kind == MemoryRegionKind::Ram)
                .all(|r| r.range.start().get() + r.range.len() <= start
                    || r.range.start().get() >= end)
        );
    }

    #[test]
    fn patch_physical_ram_on_the_ecam_window_is_rejected() {
        let mut board = load_firmware_image(&hlt_image(), &ecam_config()).unwrap();
        let before = board
            .bus_mut()
            .read_physical(ecam_at(0), AccessWidth::Dword)
            .unwrap();
        for offset in [0, 0x1000, 0xF_FFFC] {
            assert!(
                matches!(
                    board
                        .bus_mut()
                        .patch_physical_ram(ecam_at(offset), &[0, 0, 0, 0]),
                    Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Mmio))
                ),
                "{offset:#x}"
            );
        }
        assert_eq!(
            board
                .bus_mut()
                .read_physical(ecam_at(0), AccessWidth::Dword)
                .unwrap(),
            before
        );
    }

    #[test]
    fn empty_bus_0_reads_all_ones_for_all_32_devices_through_the_board_bus() {
        let mut board = load_firmware_image(&hlt_image(), &ecam_config()).unwrap();
        for device in 0..32u64 {
            for function in [0u64, 1, 7] {
                let at = ecam_at(ecam_off(device, function, 0));
                for (width, ones) in [
                    (AccessWidth::Dword, 0xFFFF_FFFFu64),
                    (AccessWidth::Word, 0xFFFF),
                    (AccessWidth::Byte, 0xFF),
                ] {
                    assert_eq!(
                        board.bus_mut().read_physical(at, width),
                        Ok(ones),
                        "{device}:{function}"
                    );
                }
            }
            // Writes to an absent function are accepted and dropped.
            let at = ecam_at(ecam_off(device, 0, 4));
            board
                .bus_mut()
                .write_physical(at, AccessWidth::Word, 7, |_| {})
                .unwrap();
            assert_eq!(
                board.bus_mut().read_physical(at, AccessWidth::Word),
                Ok(0xFFFF)
            );
        }
        // The last byte of the window is device 31 function 7 register 0xFFF.
        assert_eq!(
            board
                .bus_mut()
                .read_physical(ecam_at(0xF_FFFF), AccessWidth::Byte),
            Ok(0xFF)
        );
        // An address past the last byte of the 1 MiB window is not routed to the device.
        assert!(
            board
                .bus_mut()
                .read_physical(ecam_at(0x10_0000), AccessWidth::Byte)
                .is_err()
        );
    }

    fn inl(bus: &mut MachineBus, port: u16) -> u32 {
        bus.read_port(PortAddress::new(port), AccessWidth::Dword)
            .unwrap()
    }

    fn outl(bus: &mut MachineBus, port: u16, value: u32) {
        bus.write_port(PortAddress::new(port), AccessWidth::Dword, value)
            .unwrap();
    }

    fn conf1(device: u32, register: u32) -> u32 {
        0x8000_0000 | (device << 11) | (register & 0xFC)
    }

    #[test]
    fn linux_type1_and_type2_probes_on_the_board_instance() {
        let mut board = load_firmware_image(&hlt_image(), &ecam_config()).unwrap();
        let bus = board.bus_mut();
        let outb = |bus: &mut MachineBus, port: u16, value: u32| {
            bus.write_port(PortAddress::new(port), AccessWidth::Byte, value)
                .unwrap();
        };
        // pci_check_type1: outb 0xCFB, save latch, write 0x80000000, read back, restore.
        outb(bus, 0xCFB, 1);
        let saved = inl(bus, 0xCF8);
        assert_eq!(saved, 0);
        outl(bus, 0xCF8, 0x8000_0000);
        assert_eq!(inl(bus, 0xCF8), 0x8000_0000, "latch reads back");
        // The latch readback above is all this test proves for type1. Sanity
        // inputs, 00:00.0 class and vendor words, are all ones: with no function
        // of class 0x0600/0x0300 and no DMI year >= 2001 pci_sanity_check
        // (direct.c:204-216) rejects conf1 and Linux moves on to type2, which
        // also fails below (MACHINE_CONTRACT.md, S4-30c). That is expected here.
        outl(bus, 0xCF8, conf1(0, 0x0A));
        assert_eq!(
            bus.read_port(PortAddress::new(0xCFC + 2), AccessWidth::Word),
            Ok(0xFFFF)
        );
        outl(bus, 0xCF8, conf1(0, 0));
        assert_eq!(
            bus.read_port(PortAddress::new(0xCFC), AccessWidth::Word),
            Ok(0xFFFF)
        );
        outl(bus, 0xCF8, saved);
        assert_eq!(inl(bus, 0xCF8), 0);
        // pci_check_type2: three outb(0), then both bytes must read 0.
        outb(bus, 0xCFB, 0);
        outb(bus, 0xCF8, 0);
        outb(bus, 0xCFA, 0);
        let gate = bus.read_port(PortAddress::new(0xCF8), AccessWidth::Byte) == Ok(0)
            && bus.read_port(PortAddress::new(0xCFA), AccessWidth::Byte) == Ok(0);
        assert!(!gate, "type2 is rejected");
        // 0xCF9 stays an explicit device error (reset is S7-14a), never a silent success.
        assert!(
            bus.write_port(PortAddress::new(0xCF9), AccessWidth::Byte, 0x0E)
                .is_err()
        );
    }

    #[test]
    fn conf1_write_is_visible_through_ecam_on_the_diagnostic_endpoint() {
        let mut board = load_firmware_image_with_pci(
            &hlt_image(),
            &ecam_config(),
            pci_with_diagnostic_endpoint(),
        )
        .unwrap();
        let bus = board.bus_mut();
        // The function exists in both views; neighbours stay absent.
        assert_eq!(
            bus.read_physical(ecam_at(ecam_off(3, 0, 0)), AccessWidth::Dword),
            Ok(0x0001_D1A6)
        );
        outl(bus, 0xCF8, conf1(3, 0));
        assert_eq!(inl(bus, 0xCFC), 0x0001_D1A6);
        for device in [0u64, 2, 4, 31] {
            assert_eq!(
                bus.read_physical(ecam_at(ecam_off(device, 0, 0)), AccessWidth::Dword),
                Ok(0xFFFF_FFFF)
            );
        }
        // Command and BAR0 through conf1, seen through ECAM.
        outl(bus, 0xCF8, conf1(3, 0x04));
        bus.write_port(PortAddress::new(0xCFC), AccessWidth::Word, 0xFFFF)
            .unwrap();
        assert_eq!(
            bus.read_physical(ecam_at(ecam_off(3, 0, 4)), AccessWidth::Word),
            Ok(7)
        );
        outl(bus, 0xCF8, conf1(3, 0x10));
        outl(bus, 0xCFC, 0xFFFF_FFFF);
        assert_eq!(
            bus.read_physical(ecam_at(ecam_off(3, 0, 0x10)), AccessWidth::Dword),
            Ok(0xFFFF_F000)
        );
        // And back: ECAM write, conf1 read.
        bus.write_physical(
            ecam_at(ecam_off(3, 0, 0x10)),
            AccessWidth::Dword,
            0x1234_5678,
            |_| {},
        )
        .unwrap();
        assert_eq!(inl(bus, 0xCFC), 0x1234_5000);
    }

    #[test]
    fn without_the_flag_a_supplied_pci_device_is_not_registered() {
        let mut board = load_firmware_image_with_pci(
            &hlt_image(),
            &reference_config(),
            pci_with_diagnostic_endpoint(),
        )
        .unwrap();
        assert_eq!(hex(board.handoff_block()), REFERENCE_HANDOFF_HEX);
        assert!(
            board
                .bus_mut()
                .read_physical(ecam_at(ecam_off(3, 0, 0)), AccessWidth::Dword)
                .is_err()
        );
    }

    #[test]
    fn run_boot_executes_commands_submitted_before_the_guest_starts() {
        const ASQ: u64 = 0x0100_0000;
        const ACQ: u64 = 0x0100_1000;
        const DATA: u64 = 0x0100_2000;
        let mut board = load_firmware_image(&nvme_image(&[0xf4]), &nvme_config()).unwrap();
        let mmio = |offset: u64| GuestPhysicalAddress::new(NVME_MMIO_BASE + offset);
        let bus = board.bus_mut();
        let mut program = |offset: u64, width: AccessWidth, value: u64| {
            bus.write_physical(mmio(offset), width, value, |_| {})
                .unwrap();
        };
        program(0x24, AccessWidth::Dword, 0x000F_000F); // AQA: 16 entries each
        program(0x28, AccessWidth::Qword, ASQ);
        program(0x30, AccessWidth::Qword, ACQ);
        program(0x14, AccessWidth::Dword, 0x0046_0001); // CC: EN, IOSQES 6, IOCQES 4
        // Identify Controller, cid 0x42, then the admin SQ tail doorbell.
        let mut sqe = [0u8; 64];
        sqe[0..4].copy_from_slice(&(0x06u32 | (0x42 << 16)).to_le_bytes());
        sqe[24..32].copy_from_slice(&DATA.to_le_bytes());
        sqe[40..44].copy_from_slice(&1u32.to_le_bytes());
        bus.write_flat_bytes(GuestPhysicalAddress::new(ASQ), &sqe, |_| {})
            .unwrap();
        bus.write_physical(mmio(0x1000), AccessWidth::Dword, 1, |_| {})
            .unwrap();
        let nvme = board.nvme().unwrap();
        assert!(nvme.doorbell_pending());
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 1);
        assert!(!nvme.doorbell_pending());
        let mut cqe = [0u8; 16];
        let bus = board.bus_mut();
        bus.read_flat_bytes(GuestPhysicalAddress::new(ACQ), &mut cqe)
            .unwrap();
        let dw3 = u32::from_le_bytes([cqe[12], cqe[13], cqe[14], cqe[15]]);
        assert_eq!(dw3 & 0xFFFF, 0x42, "CID");
        assert_eq!((dw3 >> 16) & 1, 1, "phase tag");
        assert_eq!(dw3 >> 17, 0, "success");
        let mut serial = [0u8; 12];
        bus.read_flat_bytes(GuestPhysicalAddress::new(DATA + 4), &mut serial)
            .unwrap();
        assert_eq!(&serial, b"OD-NVME-0001");
    }

    #[test]
    fn doorbell_pending_stops_quantum_then_host_services_before_the_next_quantum() {
        const ASQ: u64 = 0x0100_0000;
        const ACQ: u64 = 0x0100_1000;
        const DATA: u64 = 0x0100_2000;
        let mut board = load_firmware_image(&nvme_image(&[0xF4]), &nvme_config()).unwrap();
        let nvme = board.nvme().unwrap();
        let mmio = |offset: u64| GuestPhysicalAddress::new(NVME_MMIO_BASE + offset);
        {
            let bus = board.bus_mut();
            for (offset, width, value) in [
                (0x24, AccessWidth::Dword, 0x000F_000F),
                (0x28, AccessWidth::Qword, ASQ),
                (0x30, AccessWidth::Qword, ACQ),
                (0x14, AccessWidth::Dword, 0x0046_0001),
            ] {
                bus.write_physical(mmio(offset), width, value, |_| {})
                    .unwrap();
            }
            let mut sqe = [0u8; 64];
            sqe[0..4].copy_from_slice(&(0x06u32 | (0x42 << 16)).to_le_bytes());
            sqe[24..32].copy_from_slice(&DATA.to_le_bytes());
            sqe[40..44].copy_from_slice(&1u32.to_le_bytes());
            bus.write_flat_bytes(GuestPhysicalAddress::new(ASQ), &sqe, |_| {})
                .unwrap();
            bus.write_physical(mmio(0x1000), AccessWidth::Dword, 1, |_| {})
                .unwrap();
        }
        assert!(nvme.doorbell_pending(), "32-bit MMIO doorbell sets pending");

        let mut stop_checks = 0;
        let first = board.run(
            100,
            || {
                stop_checks += 1;
                nvme.doorbell_pending()
            },
            |_| {},
        );
        assert_eq!(first.exit, PagedProbeExit::Stopped);
        assert_eq!(stop_checks, 1, "the stop was requested by doorbell_pending");
        assert_eq!(
            first.instructions_executed, 0,
            "pending doorbell stops the quantum"
        );
        assert!(nvme.doorbell_pending());
        assert_eq!(
            board
                .bus_mut()
                .read_physical(GuestPhysicalAddress::new(ACQ + 12), AccessWidth::Dword)
                .unwrap(),
            0,
            "CQE must not exist before host service"
        );

        let serviced = nvme.service(board.bus_mut(), |_| {});
        assert_eq!((serviced.commands, serviced.completions), (1, 1));
        assert!(!nvme.doorbell_pending());
        let cqe = board
            .bus_mut()
            .read_physical(GuestPhysicalAddress::new(ACQ + 12), AccessWidth::Dword)
            .unwrap() as u32;
        assert_eq!(cqe & 0xFFFF, 0x42, "CID");
        assert_eq!((cqe >> 16) & 1, 1, "phase tag");
        assert_eq!(cqe >> 17, 0, "success");

        let second = board.run(100, || nvme.doorbell_pending(), |_| {});
        assert_eq!(second.exit, PagedProbeExit::Halted);
        assert_eq!(second.instructions_executed, 1);
    }

    // ---- S7-14a: ACPI fixed hardware on the board (opt-in) ----

    fn pm_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            acpi_pm: true,
            ..reference_config()
        }
    }

    fn io_port(port: u16) -> PortAddress {
        PortAddress::new(port)
    }

    /// `MOV AL,imm8; MOV DX,imm16; OUT DX,AL; HLT`.
    fn out_byte_image(port: u16, value: u8) -> Vec<u8> {
        let [lo, hi] = port.to_le_bytes();
        test_image(&[0xb0, value, 0x66, 0xba, lo, hi, 0xee, 0xf4], &[])
    }

    #[test]
    fn acpi_pm_is_off_by_default_and_the_handoff_never_changes() {
        assert!(!FirmwareBoardConfig::default().acpi_pm);
        let mut off = load(&hlt_image());
        assert_eq!(hex(off.handoff_block()), REFERENCE_HANDOFF_HEX);
        let on = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        assert_eq!(
            on.handoff_block(),
            off.handoff_block(),
            "the PM block is not in the handoff"
        );
        assert!(off.acpi_pm().is_none() && on.acpi_pm().is_some());
        for port in [
            ACPI_PM1A_EVT_BLK,
            ACPI_PM1A_CNT_BLK,
            ACPI_PM_TMR_BLK,
            ACPI_RESET_REG,
        ] {
            assert!(
                matches!(
                    off.bus_mut().read_port(io_port(port), AccessWidth::Byte),
                    Err(BusAccessError::UnmappedPort(_))
                ),
                "{port:#x}"
            );
        }
    }

    #[test]
    fn acpi_pm_registers_answer_through_the_board_bus() {
        let mut board = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        let bus = board.bus_mut();
        assert_eq!(
            bus.read_port(io_port(ACPI_PM1A_CNT_BLK), AccessWidth::Word),
            Ok(u32::from(ACPI_PM1_CNT_SCI_EN)),
            "SCI_EN is set from reset"
        );
        assert_eq!(
            bus.read_port(io_port(ACPI_PM1A_EVT_BLK), AccessWidth::Word),
            Ok(0)
        );
        board.advance_time_ns(1_000_000_000).unwrap();
        assert_eq!(
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword),
            Ok(3_579_545),
            "the PM timer follows the board clock"
        );
        assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
    }

    #[test]
    fn s5_nvme_stand_exposes_pm_tmr_on_board_clock_without_linux_profile() {
        let config = FirmwareBoardConfig::s5_nvme_stand();
        assert!(!config.linux_profile);
        let mut board = load_firmware_image(&nvme_image(&[0xf4]), &config).unwrap();
        assert!(board.nvme().is_some());
        assert!(board.acpi_pm().is_some());

        board.advance_time_ns(1_000_000_000).unwrap();
        assert_eq!(
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword),
            Ok(3_579_545),
            "S5 PM_TMR follows the board clock"
        );
        assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
    }

    #[test]
    fn acpi_pm_ports_avoid_com1_conf1_and_0xcf9_on_the_fully_populated_board() {
        let d = ACPI_PM_DESCRIPTOR;
        let pm = d.base_port..d.base_port + d.port_bytes as u16;
        let com1 = FIRMWARE_COM1_PORT..FIRMWARE_COM1_PORT + 8;
        let conf1 = PCI_CONF1_BASE_PORT..PCI_CONF1_BASE_PORT + PCI_CONF1_PORT_BYTES as u16;
        for port in pm.clone() {
            assert!(!com1.contains(&port) && !conf1.contains(&port), "{port:#x}");
        }
        assert!(!pm.contains(&0xCF9));
        // The bus refuses overlapping port regions, so a successful build with
        // every optional device proves the map; behaviour is checked below.
        let config = FirmwareBoardConfig {
            second_screen: true,
            nvme: Some(NvmeConfig::default()),
            pci_ecam: true,
            acpi_pm: true,
            ..reference_config()
        };
        let mut board = load_firmware_image(&nvme_image(&[0xf4]), &config).unwrap();
        let bus = board.bus_mut();
        // COM1 line status (THR empty + transmitter empty) still answers.
        assert_eq!(
            bus.read_port(io_port(FIRMWARE_COM1_PORT + 5), AccessWidth::Byte)
                .map(|v| v & 0x60),
            Ok(0x60)
        );
        // conf1 still reaches the PCI device, and 0xCF9 is still UnknownOffset.
        assert_eq!(
            bus.read_port(io_port(PCI_CONF1_BASE_PORT), AccessWidth::Dword),
            Ok(0)
        );
        assert!(matches!(
            bus.read_port(io_port(0xCF9), AccessWidth::Byte),
            Err(BusAccessError::Device {
                source: DeviceError::UnknownOffset,
                ..
            })
        ));
        assert!(matches!(
            bus.write_port(io_port(0xCF9), AccessWidth::Byte, 0x06),
            Err(BusAccessError::Device {
                source: DeviceError::UnknownOffset,
                ..
            })
        ));
        // Writing the PCI reset-like value to 0xCF9 does not stop the machine.
        assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
        // The PM block is on its own device, not on the PCI one.
        assert_eq!(
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM1A_CNT_BLK), AccessWidth::Word),
            Ok(1)
        );
    }

    #[test]
    fn acpi_pm_does_not_alias_mmio_windows_of_the_board() {
        // Ports and MMIO are separate spaces: the PM block adds no MMIO region,
        // so the memory map and the handoff regions are identical to the default.
        let devices = board_devices(true, true, false, true);
        let plain = FirmwareBoardConfig {
            ram_bytes: FIRMWARE_MAX_RAM_BYTES,
            nvme: Some(NvmeConfig::default()),
            pci_ecam: true,
            ..FirmwareBoardConfig::default()
        };
        let with_pm = FirmwareBoardConfig {
            acpi_pm: true,
            ..plain.clone()
        };
        assert_eq!(
            board_map(&plain, &devices).unwrap().regions(),
            board_map(&with_pm, &devices).unwrap().regions()
        );
        // No MMIO device sits at a numerically equal address either.
        for entry in devices.iter().filter(|d| d.is_mmio()) {
            assert!(entry.base > u64::from(u16::MAX));
        }
    }

    #[test]
    fn guest_s5_write_stops_the_run_with_shutdown() {
        // OUT to the high byte of PM1_CNT: SLP_TYP = 5 (bits 10..12) and SLP_EN (bit 13).
        let slp = ((u16::from(ACPI_SLP_TYP_S5) << 10) | ACPI_PM1_CNT_SLP_EN) >> 8;
        let image = out_byte_image(ACPI_PM1A_CNT_BLK + 1, slp as u8);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(report.stop_reason, Some(MachineStopReason::Shutdown));
        assert_eq!(report.retired, 3, "MOV, MOV, OUT; the HLT never runs");
        assert_eq!(report.virtual_ns, 3);
        // The stop is latched: running again executes nothing.
        let again = board.run_boot(100).unwrap();
        assert_eq!(again.exit, PagedProbeExit::Stopped);
        assert_eq!(again.stop_reason, Some(MachineStopReason::Shutdown));
        assert_eq!(again.retired, 0);
    }

    #[test]
    fn guest_reset_write_stops_the_run_with_reset() {
        let image = out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
        assert_eq!(report.retired, 3);
        assert_ne!(Some(MachineStopReason::Shutdown), report.stop_reason);
    }

    #[test]
    fn a_stop_on_the_last_budgeted_instruction_is_not_a_budget_exhaustion() {
        let image = out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let report = board.run_boot(3).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
    }

    #[test]
    fn other_reset_values_and_a_board_without_the_flag_do_not_stop() {
        let mut board =
            load_firmware_image(&out_byte_image(ACPI_RESET_REG, 0x05), &pm_config()).unwrap();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.stop_reason, None);
        // Without the flag the port does not exist: a bus error, not a stop.
        let mut off = load(&out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE));
        let report = off.run_boot(100).unwrap();
        assert!(
            matches!(report.exit, PagedProbeExit::PortIo(_)),
            "{:?}",
            report.exit
        );
        assert_eq!(report.stop_reason, None);
    }

    #[test]
    fn the_pm_timer_follows_the_board_clock() {
        let mut board = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        board.advance_time_ns(2_000_000_000).unwrap();
        let first = board
            .bus_mut()
            .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword)
            .unwrap();
        board.advance_time_ns(1_000_000).unwrap();
        let second = board
            .bus_mut()
            .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword)
            .unwrap();
        assert_eq!(first, 7_159_090);
        assert_eq!(second - first, 3_579);
        assert_eq!(board.clock().now_ns(), 2_001_000_000);
    }

    #[test]
    fn run_boot_advances_the_clock_by_retired_instructions_and_pm_tmr_sees_it() {
        let mut board = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        // One tick of the PM timer (3.579545 MHz) completes at exactly 1 s.
        board.advance_time_ns(999_999_999).unwrap();
        let tmr = |board: &mut LoadedFirmwareBoard| {
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword)
                .unwrap()
        };
        assert_eq!(tmr(&mut board), 3_579_544);
        let report = board.run_boot(10).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 1, "HLT");
        assert_eq!(report.virtual_ns, 1_000_000_000);
        assert_eq!(board.clock().now_ns(), 1_000_000_000);
        assert_eq!(
            tmr(&mut board),
            3_579_545,
            "the retired HLT moved the timer"
        );
    }

    // ---- S4-37a2: PM timer port tick (ISSUE-067) ----

    /// `MOV DX,0x609; IN AL,DX x reads; HLT`. The base paged probe has no compare
    /// and no jump, so a wait on the PM timer is a straight run of reads; the last
    /// one leaves bits 8..16 of the timer in AL. Only the byte form of IN is decoded.
    fn pm_tmr_reads_image(reads: usize) -> Vec<u8> {
        let [lo, hi] = (ACPI_PM_TMR_BLK + 1).to_le_bytes();
        let mut code = vec![0x66, 0xba, lo, hi];
        code.extend(std::iter::repeat_n(0xec, reads));
        code.push(0xf4);
        test_image(&code, &[])
    }

    #[test]
    fn a_guest_sees_the_pm_timer_advance_because_each_read_ticks_the_clock() {
        // Bits 8..16 become non-zero after 256 timer ticks (~71.5 us): 72 reads of
        // 1 us each are enough, 100 reads leave a safe margin.
        let mut board = load_firmware_image(&pm_tmr_reads_image(100), &pm_config()).unwrap();
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(board.acpi_pm().unwrap().timer_reads(), 100);
        assert_eq!(report.virtual_ns, report.retired + 100 * 1_000);
        assert_eq!(board.clock().now_ns(), report.virtual_ns);
        assert_eq!(board.state().gpr[0] & 0xff, 1, "AL: the timer moved");
    }

    #[test]
    fn without_the_pm_tick_the_same_guest_sees_a_frozen_timer() {
        let mut board = load_firmware_image(&pm_tmr_reads_image(100), &pm_config()).unwrap();
        board.suppress_pm_tick = true;
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(board.acpi_pm().unwrap().timer_reads(), 100);
        assert_eq!(
            report.virtual_ns, report.retired,
            "only instructions moved time"
        );
        assert_eq!(board.state().gpr[0] & 0xff, 0, "AL: the timer stood still");
    }

    #[test]
    fn n_guest_reads_of_pm_tmr_move_the_clock_by_n_microseconds() {
        // MOV DX,0x608; then three IN AL,DX; HLT.
        let [lo, hi] = ACPI_PM_TMR_BLK.to_le_bytes();
        let image = test_image(&[0x66, 0xba, lo, hi, 0xec, 0xec, 0xec, 0xf4], &[]);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 5);
        assert_eq!(board.acpi_pm().unwrap().timer_reads(), 3);
        assert_eq!(report.virtual_ns, 5 + 3 * 1_000);
    }

    #[test]
    fn host_reads_through_bus_mut_do_not_move_the_clock_or_leak_into_a_run() {
        let mut board = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        for _ in 0..2 {
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword)
                .unwrap();
        }
        assert_eq!(board.clock().now_ns(), 0, "host reads never move the clock");
        // The queued host ticks are dropped when the run starts: only the HLT counts.
        let report = board.run_boot(10).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.virtual_ns, 1);
        assert_eq!(board.clock().now_ns(), 1);
    }

    // ---- S4-31c: ACPI tables manifest (opt-in `acpi_tables`, handoff v1.2) ----

    fn tables_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            acpi_pm: true,
            acpi_tables: true,
            ..reference_config()
        }
    }

    /// An image that declares support for handoff minor 2.
    fn minor2_image(code: &[u8]) -> Vec<u8> {
        test_image(code, &[(0x2A, &2u16.to_le_bytes())])
    }

    fn crc_is_valid(block: &[u8]) -> bool {
        let mut zeroed = block.to_vec();
        zeroed[0x14..0x18].copy_from_slice(&[0; 4]);
        le_u32(block, 0x14).unwrap() == crc32_ieee(&[&zeroed])
    }

    fn region_tuple(region: &[u8]) -> (u32, u64, u64, u32) {
        (
            le_u32(region, 0).unwrap(),
            le_u64(region, 8).unwrap(),
            le_u64(region, 0x10).unwrap(),
            le_u32(region, 0x18).unwrap(),
        )
    }

    #[test]
    fn acpi_tables_is_off_by_default_and_every_default_handoff_is_unchanged() {
        assert!(!FirmwareBoardConfig::default().acpi_tables);
        // The golden reference (not edited by S4-31c): default, and the PM block alone.
        assert_eq!(
            hex(load(&hlt_image()).handoff_block()),
            REFERENCE_HANDOFF_HEX
        );
        let pm = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        assert_eq!(hex(pm.handoff_block()), REFERENCE_HANDOFF_HEX);
        // The two older flags keep their own shapes without the new one.
        let both = FirmwareBoardConfig {
            pci_ecam: true,
            acpi_pm: true,
            ..reference_config()
        };
        let both = load_firmware_image(&hlt_image(), &both).unwrap();
        assert_eq!(le_u16(both.handoff_block(), 0x0A).unwrap(), 0);
        assert_eq!(le_u32(both.handoff_block(), 0x40).unwrap(), 7);
        let nvme = load_firmware_image(&nvme_image(&[0xf4]), &nvme_config()).unwrap();
        assert_eq!(le_u16(nvme.handoff_block(), 0x0A).unwrap(), 1);
        assert_eq!(le_u32(nvme.handoff_block(), 0x40).unwrap(), 8);
    }

    #[test]
    fn acpi_tables_needs_the_pm_block_and_an_image_for_handoff_minor_2() {
        let without_pm = FirmwareBoardConfig {
            acpi_tables: true,
            ..reference_config()
        };
        assert_eq!(
            load_firmware_image(&minor2_image(&[0xf4]), &without_pm).err(),
            Some(FirmwareBoardError::Config(
                "acpi_tables describes the PM block and needs acpi_pm"
            ))
        );
        for max in [0u16, 1] {
            let image = test_image(&[0xf4], &[(0x2A, &max.to_le_bytes())]);
            assert_eq!(
                load_firmware_image(&image, &tables_config()).err(),
                Some(FirmwareBoardError::ImageHeader {
                    field: "handoff_minor_max",
                    value: u64::from(max)
                })
            );
        }
        assert!(load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).is_ok());
    }

    #[test]
    fn acpi_tables_adds_code_9_and_the_ram_split_to_a_v1_2_handoff() {
        let off = load(&hlt_image());
        let on = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        let (a, b) = (off.handoff_block(), on.handoff_block());
        assert_eq!(le_u16(b, 0x08).unwrap(), 1, "version_major");
        assert_eq!(le_u16(b, 0x0A).unwrap(), 2, "version_minor");
        assert_eq!(le_u32(b, 0x0C).unwrap(), 0xA0, "header_size");
        // 12 regions + 3 (one RAM region becomes four), 7 devices + 1.
        assert_eq!(le_u32(a, 0x34).unwrap(), 12);
        assert_eq!(le_u32(b, 0x34).unwrap(), 15);
        assert_eq!(le_u32(b, 0x40).unwrap(), 8);
        assert_eq!(le_u32(b, 0x3C).unwrap(), 0xA0 + 32 * 15);
        let total = 0xA0 + 32 * 15 + 48 * 8;
        assert_eq!(le_u32(b, 0x10).unwrap() as usize, total);
        assert_eq!(b.len(), total);
        assert_eq!(total % 8, 0);
        assert!(crc_is_valid(b));
        // Fields outside the counts, size, minor and CRC (tsc, uuid, serial) are the same.
        assert_eq!(a[0x18..0x34], b[0x18..0x34]);
        assert_eq!(a[0x48..0xA0], b[0x48..0xA0]);
        // Entry 7 is code 9, written out by hand in little-endian order.
        let expected: [u8; 48] = [
            0x09, 0x00, 0x00, 0x00, // type
            0x03, 0x00, 0x00, 0x00, // flags: PRESENT | PIO
            0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // base 0x0600
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // length 0x10
            0x09, 0x00, 0x00, 0x00, // gsi 9
            0x03, 0x00, 0x00, 0x00, // level, active-low
            0x00, 0x06, 0x04, 0x06, 0x08, 0x06, 0x0C,
            0x06, // param0: 0x600, 0x604, 0x608, 0x60C
            0x06, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, // param1: RESET_VALUE 6, SLP_TYP 5
        ];
        assert_eq!(device_entry(b, 7), expected);
        // The seven v1.0 entries are unchanged.
        for index in 0..7 {
            assert_eq!(device_entry(a, index), device_entry(b, index), "{index}");
        }
        // Fields come from the typed descriptor and the SCI decision.
        let d = ACPI_PM_DESCRIPTOR;
        let entry = device_entry(b, 7);
        assert_eq!(le_u64(entry, 0x08).unwrap(), u64::from(d.base_port));
        assert_eq!(le_u64(entry, 0x10).unwrap(), u64::from(d.port_bytes));
        assert_eq!(le_u32(entry, 0x18).unwrap(), 9, "SCI GSI");
        assert_eq!(le_u32(entry, 0x1C).unwrap(), 0b11, "level and active-low");
        assert_eq!(le_u64(entry, 0x20).unwrap(), d.handoff_param0());
        assert_eq!(le_u64(entry, 0x28).unwrap(), d.handoff_param1());
        assert_eq!(
            le_u64(entry, 0x20).unwrap() & 0xFFFF,
            u64::from(ACPI_PM1A_EVT_BLK)
        );
        assert_eq!(
            (le_u64(entry, 0x20).unwrap() >> 16) & 0xFFFF,
            u64::from(ACPI_PM1A_CNT_BLK)
        );
        assert_eq!(
            (le_u64(entry, 0x20).unwrap() >> 32) & 0xFFFF,
            u64::from(ACPI_PM_TMR_BLK)
        );
        assert_eq!(
            (le_u64(entry, 0x20).unwrap() >> 48) & 0xFFFF,
            u64::from(ACPI_RESET_REG)
        );
        assert_eq!(
            le_u64(entry, 0x28).unwrap(),
            u64::from(ACPI_RESET_VALUE) | u64::from(ACPI_SLP_TYP_S5) << 8
        );
    }

    #[test]
    fn acpi_tables_split_low_ram_into_reclaim_nvs_and_zero_ram() {
        let off = load(&hlt_image());
        let on = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        let before: Vec<_> = regions_of(off.handoff_block())
            .into_iter()
            .map(region_tuple)
            .collect();
        let after: Vec<_> = regions_of(on.handoff_block())
            .into_iter()
            .map(region_tuple)
            .collect();
        // Hand-written new region entries (first one, bytes in little-endian order).
        assert_eq!(
            regions_of(on.handoff_block())[1],
            [
                0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // type 11, attrs 0
                0x00, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x00, 0x00, // base 0xE_0000
                0x00, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // length 0xF000
                0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, // no owner
            ]
        );
        // The region table equals the old one with the first RAM piece cut in four.
        let mut expected = vec![
            (REGION_RAM, 0, 0xE_0000, NO_OWNER),
            (REGION_ACPI_TABLES, 0xE_0000, 0xF000, NO_OWNER),
            (REGION_ACPI_NVS, 0xE_F000, 0x1000, NO_OWNER),
            (REGION_RAM, 0xF_0000, 0x1_0000, NO_OWNER),
        ];
        expected.extend_from_slice(&before[1..]);
        assert_eq!(before[0], (REGION_RAM, 0, FIRMWARE_HANDOFF_GPA, NO_OWNER));
        assert_eq!(after, expected);
        // Sorted, disjoint, same total as before; no RAM piece covers the new ranges.
        for pair in after.windows(2) {
            assert!(pair[0].1 + pair[0].2 <= pair[1].1, "{pair:?}");
        }
        let sum = |table: &[(u32, u64, u64, u32)]| table.iter().map(|r| r.2).sum::<u64>();
        assert_eq!(sum(&before), sum(&after));
        let (tables_end, facs_end) = (
            FIRMWARE_ACPI_TABLES_BASE + FIRMWARE_ACPI_TABLES_BYTES,
            FIRMWARE_ACPI_FACS_BASE + FIRMWARE_ACPI_FACS_BYTES,
        );
        assert_eq!(tables_end, FIRMWARE_ACPI_FACS_BASE);
        assert_eq!(facs_end, 0xF_0000);
        assert!(facs_end <= FIRMWARE_HANDOFF_GPA, "below the handoff block");
        for r in after.iter().filter(|r| r.0 == REGION_RAM) {
            assert!(
                r.1 + r.2 <= FIRMWARE_ACPI_TABLES_BASE || r.1 >= facs_end,
                "{r:?}"
            );
        }
    }

    #[test]
    fn acpi_tables_leave_the_memory_map_and_page_tables_unchanged_and_ram_zero() {
        for ram_bytes in [FIRMWARE_MIN_RAM_BYTES, FIRMWARE_MAX_RAM_BYTES] {
            let base = FirmwareBoardConfig {
                ram_bytes,
                second_screen: true,
                nvme: Some(NvmeConfig::default()),
                pci_ecam: true,
                acpi_pm: true,
                ..FirmwareBoardConfig::default()
            };
            let with = FirmwareBoardConfig {
                acpi_tables: true,
                ..base.clone()
            };
            let map_without = board_map(&base, &board_devices(true, true, false, true)).unwrap();
            let map_with = board_map(&with, &board_devices(true, true, true, true)).unwrap();
            let shape = |map: &MachineMemoryMap| {
                map.regions()
                    .iter()
                    .map(|r| (r.range.start().get(), r.range.len(), r.kind))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                shape(&map_without),
                shape(&map_with),
                "the area is part of low RAM"
            );
            assert_eq!(
                boot_page_table_count(&map_without),
                boot_page_table_count(&map_with)
            );
            // The area is inside the single low RAM region and disjoint from
            // every other region of the real map (ROM, ECAM, NVMe, IOAPIC, LAPIC,
            // display registers and both framebuffers, high RAM).
            let (start, end) = (
                FIRMWARE_ACPI_TABLES_BASE,
                FIRMWARE_ACPI_FACS_BASE + FIRMWARE_ACPI_FACS_BYTES,
            );
            let mut inside = 0;
            for region in map_with.regions() {
                let r_start = region.range.start().get();
                let r_end = r_start + region.range.len();
                if r_start <= start && end <= r_end {
                    inside += 1;
                    assert_eq!(region.kind, MemoryRegionKind::Ram);
                    assert_eq!(r_start, 0);
                } else {
                    assert!(r_end <= start || r_start >= end, "{region:?}");
                }
            }
            assert_eq!(inside, 1);
            let ecam = PCI_ECAM_DESCRIPTOR;
            for (base, len) in [
                (ecam.base, ecam.size),
                (FIRMWARE_ROM_BASE, FIRMWARE_IMAGE_BYTES as u64),
                (DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
                (DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                (DISPLAY_SECOND_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                (NVME_MMIO_BASE, NVME_MMIO_BYTES),
                (IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE),
                (LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE),
            ] {
                assert!(base >= end || base + len <= start, "{base:#x}");
            }
        }
        // The PM block is port I/O (a separate space) and the area is zero RAM
        // that the guest reads back as zero.
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        let mut area = vec![0xAAu8; 0x10_0000 - 0xE_0000];
        board
            .bus_mut()
            .read_flat_bytes(
                GuestPhysicalAddress::new(FIRMWARE_ACPI_TABLES_BASE),
                &mut area,
            )
            .unwrap();
        assert!(area.iter().all(|&byte| byte == 0));
        assert!(device_entry(board.handoff_block(), 7)[4] & 2 != 0, "PIO");
    }

    #[test]
    fn acpi_tables_with_every_other_flag_keep_indices_and_owners() {
        let config = FirmwareBoardConfig {
            second_screen: true,
            nvme: Some(NvmeConfig::default()),
            pci_ecam: true,
            ..tables_config()
        };
        let image = minor2_image(&[0xf4]);
        let mut board = load_firmware_image(&image, &config).unwrap();
        let block = board.handoff_block().to_vec();
        assert_eq!(le_u16(&block, 0x0A).unwrap(), 2);
        // 13 (two screens) + 1 (ECAM) + 1 (NVMe) + 3 (RAM split) regions, 9 devices.
        assert_eq!(le_u32(&block, 0x34).unwrap(), 18);
        assert_eq!(le_u32(&block, 0x40).unwrap(), 9);
        assert_eq!(block.len(), 0xA0 + 32 * 18 + 48 * 9);
        assert!(crc_is_valid(&block));
        let code = |index: usize| le_u32(device_entry(&block, index), 0).unwrap();
        assert_eq!((code(6), code(7), code(8)), (7, 8, 9), "ECAM, NVMe, ACPI");
        assert_eq!(
            le_u32(device_entry(&block, 6), 4).unwrap(),
            1,
            "ECAM PRESENT"
        );
        let owner_of = |base: u64| {
            regions_of(&block)
                .into_iter()
                .map(region_tuple)
                .find(|r| r.1 == base)
                .map(|r| r.3)
                .unwrap()
        };
        assert_eq!(owner_of(PCI_ECAM_DESCRIPTOR.base), 6);
        assert_eq!(owner_of(NVME_MMIO_BASE), 7);
        assert_eq!(owner_of(DISPLAY_FRAMEBUFFER_BASE), DEV_DISPLAY as u32);
        assert_eq!(
            owner_of(DISPLAY_SECOND_FRAMEBUFFER_BASE),
            DEV_DISPLAY as u32
        );
        assert_eq!(owner_of(FIRMWARE_ACPI_TABLES_BASE), NO_OWNER);
        // Without NVMe the ACPI entry takes index 7.
        let no_nvme = load_firmware_image(
            &image,
            &FirmwareBoardConfig {
                nvme: None,
                ..config
            },
        )
        .unwrap();
        assert_eq!(
            le_u32(device_entry(no_nvme.handoff_block(), 7), 0).unwrap(),
            9
        );
        assert_eq!(le_u32(no_nvme.handoff_block(), 0x40).unwrap(), 8);
        // The devices behind the flags still work: PM block, ECAM window, NVMe.
        let pm = io_port(ACPI_PM1A_CNT_BLK);
        assert_eq!(
            board.bus_mut().read_port(pm, AccessWidth::Word),
            Ok(u32::from(ACPI_PM1_CNT_SCI_EN))
        );
        assert!(board.acpi_pm().is_some() && board.nvme().is_some());
        assert_eq!(
            board
                .bus_mut()
                .read_physical(ecam_at(0), AccessWidth::Dword),
            Ok(0xFFFF_FFFF)
        );
    }

    // ---- S4-37a1: Linux profile (opt-in `linux_profile`) ----

    fn linux_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            pci_ecam: true,
            linux_profile: true,
            ..tables_config()
        }
    }

    fn linux_board() -> LoadedFirmwareBoard {
        load_firmware_image(&minor2_image(&[0xf4]), &linux_config()).unwrap()
    }

    fn inb(board: &mut LoadedFirmwareBoard, port: u16) -> Result<u32, BusAccessError> {
        board.bus_mut().read_port(io_port(port), AccessWidth::Byte)
    }

    fn outb(board: &mut LoadedFirmwareBoard, port: u16, value: u32) -> Result<(), BusAccessError> {
        board
            .bus_mut()
            .write_port(io_port(port), AccessWidth::Byte, value)
    }

    /// Channel 0 in mode 0, LSB then MSB, reload 10000; returns the counter read
    /// as two bytes.
    fn pit_ch0_program_and_read(board: &mut LoadedFirmwareBoard) -> u32 {
        outb(board, 0x43, 0x30).unwrap();
        outb(board, 0x40, 10_000 & 0xFF).unwrap();
        outb(board, 0x40, 10_000 >> 8).unwrap();
        pit_ch0_read(board)
    }

    fn pit_ch0_read(board: &mut LoadedFirmwareBoard) -> u32 {
        let lo = inb(board, 0x40).unwrap();
        let hi = inb(board, 0x40).unwrap();
        (hi << 8) | lo
    }

    /// Ports the profile binds (or, without the flag, leaves as a trap).
    const LINUX_PROFILE_PORTS: [u16; 18] = [
        0x40, 0x41, 0x42, 0x43, 0x61, 0x20, 0x21, 0xA0, 0xA1, 0x70, 0x71, 0xED, 0x80, 0x60, 0x64,
        0x4D0, 0x4D1, 0x87,
    ];

    #[test]
    fn linux_profile_is_off_by_default_and_leaves_its_ports_unmapped() {
        assert!(!FirmwareBoardConfig::default().linux_profile);
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        for port in LINUX_PROFILE_PORTS {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
            assert_eq!(
                outb(&mut board, port, 0),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "write {port:#x}"
            );
        }
        // With the profile the PIC ELCR ports (S4-38L7bs) are mapped and read 0.
        let mut linux = linux_board();
        for port in [0x4D0u16, 0x4D1] {
            assert_eq!(inb(&mut linux, port), Ok(0), "linux read {port:#x}");
        }
        // The default board is unchanged byte for byte.
        let mut default = load(&hlt_image());
        assert_eq!(hex(default.handoff_block()), REFERENCE_HANDOFF_HEX);
        assert_eq!(
            inb(&mut default, 0x80),
            Err(BusAccessError::UnmappedPort(io_port(0x80)))
        );
    }

    #[test]
    fn linux_profile_does_not_change_the_handoff_or_the_manifest_hash() {
        let off_config = tables_config_with_ecam();
        let on_config = linux_config();
        let image = minor2_image(&[0xf4]);
        let off = load_firmware_image(&image, &off_config).unwrap();
        let on = load_firmware_image(&image, &on_config).unwrap();
        assert_eq!(on.handoff_block(), off.handoff_block());
        assert_eq!(
            on_config.acpi_manifest_hash().unwrap(),
            off_config.acpi_manifest_hash().unwrap()
        );
        assert_eq!(on.boot_page_table_bytes(), off.boot_page_table_bytes());
    }

    fn tables_config_with_ecam() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            pci_ecam: true,
            ..tables_config()
        }
    }

    #[test]
    fn linux_profile_needs_acpi_pm_acpi_tables_and_pci_ecam() {
        let wanted =
            || FirmwareBoardError::Config("linux_profile needs acpi_pm, acpi_tables and pci_ecam");
        let image = minor2_image(&[0xf4]);
        for (acpi_pm, acpi_tables, pci_ecam) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
            (true, false, true),
            (false, false, true),
        ] {
            let config = FirmwareBoardConfig {
                acpi_pm,
                acpi_tables,
                pci_ecam,
                linux_profile: true,
                ..reference_config()
            };
            assert_eq!(
                load_firmware_image(&image, &config).err(),
                Some(wanted()),
                "pm={acpi_pm} tables={acpi_tables} ecam={pci_ecam}"
            );
            assert_eq!(config.acpi_manifest_hash().err(), Some(wanted()));
        }
        // `acpi_tables` without `acpi_pm` keeps its own, earlier error.
        let tables_no_pm = FirmwareBoardConfig {
            acpi_tables: true,
            pci_ecam: true,
            linux_profile: true,
            ..reference_config()
        };
        assert!(matches!(
            load_firmware_image(&image, &tables_no_pm),
            Err(FirmwareBoardError::Config(_))
        ));
        assert!(load_firmware_image(&image, &linux_config()).is_ok());
    }

    #[test]
    fn linux_profile_pit_counts_with_the_board_clock() {
        let mut board = linux_board();
        let first = pit_ch0_program_and_read(&mut board);
        assert!(first <= 10_000 && first > 9_000, "{first}");
        board.advance_time_ns(1_000_000).unwrap(); // 1193 PIT ticks
        let second = pit_ch0_read(&mut board);
        assert_eq!(first - second, 1_193);
        // Port B answers as well (refresh bit toggles, no error).
        assert!(inb(&mut board, 0x61).is_ok());
    }

    #[test]
    fn linux_profile_pit_on_a_foreign_clock_stands_still() {
        // Injection (test only): the PIT reads a clock the board never advances.
        let foreign = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut board = load_firmware_image_inner(
            &minor2_image(&[0xf4]),
            &linux_config(),
            PciEcam::new(),
            Some(foreign),
            None,
        )
        .unwrap();
        let first = pit_ch0_program_and_read(&mut board);
        board.advance_time_ns(1_000_000).unwrap();
        assert_eq!(pit_ch0_read(&mut board), first, "the counter must not move");
        // The same sequence on the board clock does move (previous test), so a
        // regression of the binding to a private clock is caught.
    }

    #[test]
    fn linux_profile_8259_pair_accepts_the_linux_check_timer_init() {
        use crate::Pic8259Role;
        let mut board = linux_board();
        // Master: ICW1, ICW2, ICW3, ICW4 = 0x03 (AEOI).
        outb(&mut board, 0x20, 0x11).unwrap();
        outb(&mut board, 0x21, u32::from(Pic8259Role::Master.icw2())).unwrap();
        outb(&mut board, 0x21, u32::from(Pic8259Role::Master.icw3())).unwrap();
        outb(&mut board, 0x21, 0x03).unwrap();
        // Slave: ICW4 = 0x01.
        outb(&mut board, 0xA0, 0x11).unwrap();
        outb(&mut board, 0xA1, u32::from(Pic8259Role::Slave.icw2())).unwrap();
        outb(&mut board, 0xA1, u32::from(Pic8259Role::Slave.icw3())).unwrap();
        outb(&mut board, 0xA1, 0x01).unwrap();
        // OCW1: the mask register reads back from both chips.
        outb(&mut board, 0x21, 0xFB).unwrap();
        outb(&mut board, 0xA1, 0xFF).unwrap();
        assert_eq!(inb(&mut board, 0x21), Ok(0xFB));
        assert_eq!(inb(&mut board, 0xA1), Ok(0xFF));
    }

    #[test]
    fn linux_profile_absent_ports_read_0xff_and_ignore_writes() {
        let mut board = linux_board();
        for port in [0x70u16, 0x71, 0xED, 0x80] {
            assert_eq!(inb(&mut board, port), Ok(0xFF), "read {port:#x}");
            for value in [0u32, 0x55, 0xFF] {
                outb(&mut board, port, value).unwrap();
                assert_eq!(
                    inb(&mut board, port),
                    Ok(0xFF),
                    "{port:#x} after {value:#x}"
                );
            }
        }
    }

    /// S4-38L9w: `i8237A_init_ops` reads `DMA_PAGE_0` (`0x87`) once; the
    /// register is a stored byte (0 from reset) and the other page registers and
    /// the DMA controller ports stay traps.
    #[test]
    fn linux_profile_dma_page_0_stores_a_byte_and_its_neighbours_stay_traps() {
        let mut board = linux_board();
        assert_eq!(inb(&mut board, 0x87), Ok(0));
        for value in [0x12u32, 0xFF, 0x00] {
            outb(&mut board, 0x87, value).unwrap();
            assert_eq!(inb(&mut board, 0x87), Ok(value), "after {value:#x}");
        }
        outb(&mut board, 0x87, 0x5A).unwrap();
        // The POST port still reads 0xFF, the other page registers and the
        // controllers are unbound, and none of them saw the write.
        assert_eq!(inb(&mut board, 0x80), Ok(0xFF));
        for port in [
            0x81u16, 0x82, 0x83, 0x86, 0x88, 0x89, 0x8A, 0x8B, 0x8F, 0x00, 0x0F, 0xC0, 0xDF,
        ] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
            assert_eq!(
                outb(&mut board, port, 0),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "write {port:#x}"
            );
        }
        assert_eq!(inb(&mut board, 0x87), Ok(0x5A));
        // Only byte accesses reach the register.
        assert!(
            board
                .bus_mut()
                .read_port(io_port(0x87), AccessWidth::Word)
                .is_err()
        );
        // Without the profile the port is a trap (covered by the off-by-default
        // test through LINUX_PROFILE_PORTS).
    }

    #[test]
    fn linux_profile_keeps_the_8042_and_neighbours_as_traps() {
        let mut board = linux_board();
        for port in [0x60u16, 0x64, 0x72, 0x81, 0xEC, 0xEE, 0x44, 0x62] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
        }
        // The older devices of the board still answer.
        assert_eq!(
            board
                .bus_mut()
                .read_port(io_port(ACPI_PM1A_CNT_BLK), AccessWidth::Word),
            Ok(u32::from(ACPI_PM1_CNT_SCI_EN))
        );
    }

    // ---- S4-37a3: PIT port tick on 0x42 and 0x61 in run_boot (ISSUE-067) ----

    /// `MOV DX,port; IN AL,DX x count` for each step, then `HLT`. The paged probe
    /// has no compare and no jump (FIRMWARE_BOOT.md 7), so a wait on the PIT is a
    /// straight run of byte reads; a counter wait loop cannot run on the board.
    fn pit_reads_image(steps: &[(u16, usize)]) -> Vec<u8> {
        let mut code = Vec::new();
        for &(port, count) in steps {
            let [lo, hi] = port.to_le_bytes();
            code.extend([0x66, 0xba, lo, hi]);
            code.extend(std::iter::repeat_n(0xec, count));
        }
        code.push(0xf4);
        minor2_image(&code)
    }

    /// Host programming of channel 2: gate high, MSB-only mode 0, reload `0x1000`.
    /// Host writes never tick.
    fn program_channel_2(board: &mut LoadedFirmwareBoard) {
        outb(board, 0x61, 0x01).unwrap();
        outb(board, 0x43, 0xA0).unwrap();
        outb(board, 0x42, 0x10).unwrap();
    }

    #[test]
    fn n_guest_reads_of_pit_counter_0x42_move_the_clock_by_n_microseconds() {
        let mut board =
            load_firmware_image(&pit_reads_image(&[(0x42, 3)]), &linux_config()).unwrap();
        program_channel_2(&mut board);
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 5, "MOV, 3 x IN, HLT");
        assert_eq!(board.pit.as_ref().unwrap().counter_port_reads(), 3);
        assert_eq!(board.pit.as_ref().unwrap().port_b_reads(), 0);
        assert_eq!(report.virtual_ns, 5 + 3 * 1_000);
        assert_eq!(board.clock().now_ns(), report.virtual_ns);
    }

    #[test]
    fn guest_reads_of_0x61_and_0x42_both_tick_the_clock_in_run_boot() {
        let mut board =
            load_firmware_image(&pit_reads_image(&[(0x61, 2), (0x42, 3)]), &linux_config())
                .unwrap();
        program_channel_2(&mut board);
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 8, "2 x MOV, 5 x IN, HLT");
        assert_eq!(board.pit.as_ref().unwrap().port_b_reads(), 2);
        assert_eq!(board.pit.as_ref().unwrap().counter_port_reads(), 3);
        assert_eq!(report.virtual_ns, 8 + 5 * 1_000);
    }

    #[test]
    fn a_guest_sees_the_pit_counter_move_because_each_0x42_read_ticks_the_clock() {
        // Reload 0x1000, MSB only. 300 reads: the last one is sampled at
        // 300 ns + 299 us = 299 300 ns = 357 PIT ticks, count 4096 - 357 = 0x0E9B.
        let image = pit_reads_image(&[(0x42, 300)]);
        let mut board = load_firmware_image(&image, &linux_config()).unwrap();
        program_channel_2(&mut board);
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.virtual_ns, report.retired + 300 * 1_000);
        assert_eq!(board.state().gpr[0] & 0xff, 0x0E, "AL: the counter moved");
    }

    #[test]
    fn without_the_0x42_tick_the_same_guest_sees_a_frozen_counter() {
        // Injection (test only): the tick on 0x42 is off, the one on 0x61 stays.
        let image = pit_reads_image(&[(0x42, 300)]);
        let mut board = load_firmware_image(&image, &linux_config()).unwrap();
        board.pit.as_ref().unwrap().set_counter_port_tick(false);
        program_channel_2(&mut board);
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(board.pit.as_ref().unwrap().counter_port_reads(), 0);
        assert_eq!(
            report.virtual_ns, report.retired,
            "only instructions moved time"
        );
        assert_eq!(
            board.state().gpr[0] & 0xff,
            0x10,
            "AL: the counter stood still"
        );
        // The tick on 0x61 is independent of the injection.
        let mut board =
            load_firmware_image(&pit_reads_image(&[(0x61, 4)]), &linux_config()).unwrap();
        board.pit.as_ref().unwrap().set_counter_port_tick(false);
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.virtual_ns, report.retired + 4 * 1_000);
    }

    #[test]
    fn host_pit_reads_through_bus_mut_do_not_move_the_clock_or_leak_into_a_run() {
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &linux_config()).unwrap();
        program_channel_2(&mut board);
        for _ in 0..2 {
            inb(&mut board, 0x42).unwrap();
            inb(&mut board, 0x61).unwrap();
        }
        assert_eq!(board.clock().now_ns(), 0, "host reads never move the clock");
        // The queued host ticks are dropped when the run starts: only the HLT counts.
        let report = board.run_boot(10).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.virtual_ns, 1);
        assert_eq!(board.clock().now_ns(), 1);
    }

    #[test]
    fn the_pit_ioapic_irq_source_exists_only_with_the_linux_profile_pit() {
        // S4-38L8c: no PIT, no source; the board's own source stays the empty one.
        assert!(load(&hlt_image()).pit_ioapic_irq_source().is_none());
        assert!(linux_board().pit_ioapic_irq_source().is_some());
    }

    #[test]
    fn pit_ports_stay_unmapped_without_the_profile_and_a_guest_0x42_read_is_a_bus_error() {
        let image = pit_reads_image(&[(0x42, 1)]);
        let mut board = load_firmware_image(&image, &tables_config()).unwrap();
        assert!(board.pit.is_none());
        let report = board.run_boot(100).unwrap();
        assert!(
            matches!(report.exit, PagedProbeExit::PortIo(_)),
            "{:?}",
            report.exit
        );
        assert_eq!(
            report.virtual_ns, report.retired,
            "no tick without the profile"
        );
    }

    // ---- S4-37a6b: RDTSC as a time port on the board (Linux profile) ----

    /// `n` bare `RDTSC` (`0F 31`), then `HLT`. The base probe has no compare and
    /// no jump, so a `udelay`-style wait is a straight run of reads.
    fn rdtsc_image(n: usize) -> Vec<u8> {
        let mut code: Vec<u8> = std::iter::repeat_n([0x0f, 0x31], n).flatten().collect();
        code.push(0xf4);
        minor2_image(&code)
    }

    fn edx_eax(board: &LoadedFirmwareBoard) -> u64 {
        let gpr = &board.state().gpr;
        ((gpr[Register::Rdx as usize] & 0xffff_ffff) << 32) | (gpr[0] & 0xffff_ffff)
    }

    #[test]
    fn rdtsc_returns_floor_now_times_tsc_hz_and_each_rdtsc_ticks_one_microsecond() {
        // IN AL,imm8 (E4) is not decoded: port 0x42 is read through DX.
        // RDTSC; MOV DX,0x42; IN AL,DX; RDTSC; HLT. The huge tsc_hz makes EDX
        // non-zero; 2003 * hz mod 1e9 = 900_001_981 (fraction 0.9), so floor and
        // round differ.
        let hz = 10_000_000_000_449_327u64;
        let config = FirmwareBoardConfig {
            tsc_hz: hz,
            ..linux_config()
        };
        let image = minor2_image(&[0x0f, 0x31, 0x66, 0xba, 0x42, 0x00, 0xec, 0x0f, 0x31, 0xf4]);
        let mut board = load_firmware_image(&image, &config).unwrap();
        program_channel_2(&mut board);
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 5, "RDTSC, MOV, IN, RDTSC, HLT");
        // Before the second RDTSC: 1 ns (RDTSC) + 1 us (its tick) + 2 ns (MOV, IN)
        // + 1 us (PIT tick on 0x42).
        let at_second_rdtsc = 1 + 1_000 + 2 + 1_000;
        assert_eq!(at_second_rdtsc, 2_003);
        // floor(2003 * 10_000_000_000_449_327 / 1e9) = 20_030_000_000 (round: ...001).
        let expected = 20_030_000_000u64;
        assert!(expected > u64::from(u32::MAX), "EDX is exercised");
        assert_eq!(edx_eax(&board), expected);
        // Then: the second RDTSC (1 ns + 1 us tick) and HLT (1 ns).
        assert_eq!(report.virtual_ns, at_second_rdtsc + 1 + 1_000 + 1);
        assert_eq!(report.virtual_ns, 5 + 2 * 1_000 + 1_000);
        assert_eq!(board.clock().now_ns(), report.virtual_ns);
    }

    #[test]
    fn a_hundred_rdtsc_take_about_a_hundred_microseconds_of_virtual_time() {
        let mut board = load_firmware_image(&rdtsc_image(100), &linux_config()).unwrap();
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 101);
        assert_eq!(report.virtual_ns, 100 * 1_000 + 101);
        // The 100th RDTSC saw 99 earlier (1 ns + 1 us) steps; tsc_hz is 1 GHz.
        assert_eq!(edx_eax(&board), 99 * 1_001);
    }

    #[test]
    fn without_the_rdtsc_tick_the_same_guest_sees_time_only_from_instructions() {
        let mut board = load_firmware_image(&rdtsc_image(100), &linux_config()).unwrap();
        board.suppress_rdtsc_tick = true;
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.virtual_ns, 101, "100 us less than with the tick");
        assert_eq!(edx_eax(&board), 99);
    }

    #[test]
    fn rdtsc_without_the_linux_profile_stays_unsupported_without_a_tick() {
        let mut board = load_firmware_image(&rdtsc_image(2), &tables_config()).unwrap();
        assert!(board.rdtsc_tick.is_none());
        assert!(!board.has_linux_profile());
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
        assert_eq!(report.retired, 0, "Unsupported on the first instruction");
        assert_eq!(report.virtual_ns, 0, "no tick without the profile");
    }

    #[test]
    fn a_single_run_quantum_does_not_apply_the_rdtsc_tick() {
        let mut board = load_firmware_image(&rdtsc_image(2), &linux_config()).unwrap();
        let report = board.run(10, || false, |_| {});
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.instructions_executed, 3);
        assert_eq!(board.clock().now_ns(), 0, "only run_boot moves the clock");
        // Both RDTSC read the clock as of the quantum boundary.
        assert_eq!(edx_eax(&board), 0);
    }

    // ---- S4-37a4: LAPIC shared timer on the board clock (Linux profile) ----

    const TMICT: u64 = 0x380;
    const TMCCT: u64 = 0x390;
    const TM_LVT: u64 = 0x320;
    const TM_DIVIDE: u64 = 0x3e0;
    /// Count of the LAPIC timer in the tests: 1 000 000, divided by one.
    const TM_INITIAL: u64 = 1_000_000;

    fn apic_read(board: &mut LoadedFirmwareBoard, offset: u64) -> Result<u64, BusAccessError> {
        board.bus_mut().read_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
        )
    }

    fn apic_write(
        board: &mut LoadedFirmwareBoard,
        offset: u64,
        value: u64,
    ) -> Result<(), BusAccessError> {
        board.bus_mut().write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
            value,
            |_| {},
        )
    }

    /// Divide by one, then arm the timer; the LVT stays masked (reset value).
    fn arm_apic_timer(board: &mut LoadedFirmwareBoard) {
        apic_write(board, TM_DIVIDE, 0xb).unwrap();
        apic_write(board, TMICT, TM_INITIAL).unwrap();
    }

    fn pm_tmr_host_read(board: &mut LoadedFirmwareBoard) -> u32 {
        board
            .bus_mut()
            .read_port(io_port(ACPI_PM_TMR_BLK), AccessWidth::Dword)
            .unwrap()
    }

    #[test]
    fn linux_profile_lapic_has_a_timer_that_counts_with_the_board_clock() {
        let mut board = linux_board();
        assert!(board.local_apic().has_timer());
        assert_eq!(apic_read(&mut board, TMCCT), Ok(0), "not armed yet");
        arm_apic_timer(&mut board);
        assert_eq!(apic_read(&mut board, TMICT), Ok(TM_INITIAL));
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL));
        // 100 MHz: 10 ns per count.
        board.advance_time_ns(100_000).unwrap();
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL - 10_000));
        board.advance_time_ns(900_000).unwrap();
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL - 100_000));
        // The device never moves the clock itself.
        assert_eq!(board.clock().now_ns(), 1_000_000);
        for _ in 0..3 {
            apic_read(&mut board, TMCCT).unwrap();
        }
        assert_eq!(board.clock().now_ns(), 1_000_000);
    }

    #[test]
    fn linux_profile_tmcct_pm_tmr_and_pit_move_by_the_same_board_clock() {
        let mut board = linux_board();
        arm_apic_timer(&mut board);
        let pit0 = pit_ch0_program_and_read(&mut board);
        let (tm0, pm0) = (
            apic_read(&mut board, TMCCT).unwrap(),
            pm_tmr_host_read(&mut board),
        );
        assert_eq!((tm0, pm0), (TM_INITIAL, 0));
        let mut previous = (pit0, tm0, pm0);
        // Three steps, the reads interleaved; each step is read in a different order.
        for (step, delta_ns) in [1_000_000u64, 2_000_000, 500_000].into_iter().enumerate() {
            board.advance_time_ns(delta_ns).unwrap();
            let (pit, tm, pm) = match step {
                0 => {
                    let tm = apic_read(&mut board, TMCCT).unwrap();
                    let pm = pm_tmr_host_read(&mut board);
                    (pit_ch0_read(&mut board), tm, pm)
                }
                1 => {
                    let pm = pm_tmr_host_read(&mut board);
                    let pit = pit_ch0_read(&mut board);
                    (pit, apic_read(&mut board, TMCCT).unwrap(), pm)
                }
                _ => {
                    let pit = pit_ch0_read(&mut board);
                    let tm = apic_read(&mut board, TMCCT).unwrap();
                    (pit, tm, pm_tmr_host_read(&mut board))
                }
            };
            let total = board.clock().now_ns();
            // Absolute values from the one clock: 100 MHz, 3.579545 MHz, 1.193182 MHz.
            assert_eq!(u64::from(tm0) - u64::from(tm), total / 10, "TMCCT {step}");
            assert_eq!(
                u64::from(pm),
                total * 3_579_545 / 1_000_000_000,
                "PM_TMR {step}"
            );
            assert_eq!(
                u64::from(pit0 - pit),
                total * 1_193_182 / 1_000_000_000,
                "PIT {step}"
            );
            // The Deltas of one step agree with the step length.
            assert_eq!(u64::from(previous.1 - tm), delta_ns / 10);
            assert!(u64::from(pm - previous.2).abs_diff(delta_ns * 3_579_545 / 1_000_000_000) <= 1);
            assert!(
                u64::from(previous.0 - pit).abs_diff(delta_ns * 1_193_182 / 1_000_000_000) <= 1
            );
            previous = (pit, tm, pm);
        }
    }

    #[test]
    fn linux_profile_tmcct_runs_with_a_masked_lvt_and_a_masked_expiry_raises_no_irq() {
        let mut board = linux_board();
        // The LVT Timer comes up masked (reset value) and stays so.
        assert_eq!(apic_read(&mut board, TM_LVT), Ok(0x1_0000));
        apic_write(&mut board, TM_LVT, 0x1_0040).unwrap();
        arm_apic_timer(&mut board);
        assert_eq!(
            board.local_apic().next_timer_deadline(),
            None,
            "a masked LVT has no interrupt deadline"
        );
        board.advance_time_ns(500_000).unwrap();
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL - 50_000));
        // Past the deadline (10 ms) the count is 0; nothing dispatched it yet.
        board.advance_time_ns(20_000_000).unwrap();
        assert_eq!(apic_read(&mut board, TMCCT), Ok(0));
        // Until the owner dispatches (S4-38) a due timer rejects register writes.
        for offset in [TM_LVT, TMICT, TM_DIVIDE] {
            assert!(
                matches!(
                    apic_write(&mut board, offset, 0xb),
                    Err(BusAccessError::Device {
                        source: DeviceError::Busy,
                        ..
                    })
                ),
                "write {offset:#x}"
            );
        }
        // The owner (S4-38) will dispatch; a masked expiry only consumes the deadline.
        assert_eq!(board.local_apic().dispatch_timer(), Ok(true));
        for bank in 0..8 {
            assert_eq!(
                apic_read(&mut board, 0x200 + 0x10 * bank),
                Ok(0),
                "IRR {bank}"
            );
        }
        assert_eq!(apic_read(&mut board, TMCCT), Ok(0));
        assert_eq!(board.local_apic().dispatch_timer(), Ok(false));
    }

    #[test]
    fn without_the_linux_profile_the_lapic_has_no_timer_as_before() {
        for config in [pm_config(), tables_config(), tables_config_with_ecam()] {
            let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
            assert!(!board.local_apic().has_timer());
            for offset in [TM_LVT, TMICT, TMCCT, TM_DIVIDE] {
                assert!(
                    matches!(
                        apic_read(&mut board, offset),
                        Err(BusAccessError::Device {
                            device_index: DEV_LAPIC,
                            source: DeviceError::UnknownOffset
                        })
                    ),
                    "read {offset:#x}"
                );
            }
            assert!(matches!(
                apic_write(&mut board, TMICT, 1),
                Err(BusAccessError::Device {
                    source: DeviceError::UnknownOffset,
                    ..
                })
            ));
            // The rest of the LAPIC answers as before.
            assert_eq!(apic_read(&mut board, 0x30), Ok(0x0003_0014));
            assert_eq!(apic_read(&mut board, 0xf0), Ok(0xff));
        }
    }

    #[test]
    fn linux_profile_lapic_timer_on_a_foreign_clock_stands_still() {
        // Injection (test only): the timer observes a clock the board never advances.
        let foreign = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut board = load_firmware_image_inner(
            &minor2_image(&[0xf4]),
            &linux_config(),
            PciEcam::new(),
            None,
            Some(foreign),
        )
        .unwrap();
        assert!(board.local_apic().has_timer());
        arm_apic_timer(&mut board);
        board.advance_time_ns(1_000_000).unwrap();
        assert_eq!(
            apic_read(&mut board, TMCCT),
            Ok(TM_INITIAL),
            "the counter must not move"
        );
        // The same sequence on the board clock does move (first test).
    }

    /// `MOV DX,0x608` + `IN AL,DX` x `pm`; `MOV DX,0x61` + `IN` x `b`; `MOV DX,0x42`
    /// + `IN` x `c`; `HLT`. The paged probe has no compare and no jump, and its
    /// only memory read is the qword `MOV RAX,[RBX]`, which the dword-only LAPIC
    /// registers reject; the guest therefore only ticks the clock and the host reads
    /// TMCCT around the run.
    fn tick_guest_image(pm: usize, b: usize, c: usize) -> Vec<u8> {
        let mut code = Vec::new();
        for (port, count) in [(ACPI_PM_TMR_BLK, pm), (0x61, b), (0x42, c)] {
            let [lo, hi] = port.to_le_bytes();
            code.extend([0x66, 0xba, lo, hi]);
            code.extend(std::iter::repeat_n(0xec, count));
        }
        code.push(0xf4);
        minor2_image(&code)
    }

    #[test]
    fn run_boot_port_ticks_of_pm_tmr_and_the_pit_reach_tmcct() {
        let mut board =
            load_firmware_image(&tick_guest_image(40, 20, 20), &linux_config()).unwrap();
        arm_apic_timer(&mut board);
        program_channel_2(&mut board);
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL));
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        // 3 x MOV DX, 80 x IN, HLT.
        assert_eq!(report.retired, 3 + 80 + 1);
        assert_eq!(report.virtual_ns, report.retired + 80 * 1_000);
        // 80 us of ticks plus the retired instructions, 100 MHz.
        assert_eq!(
            apic_read(&mut board, TMCCT),
            Ok(TM_INITIAL - report.virtual_ns / 10)
        );
        assert!(report.virtual_ns / 10 >= 8_000);
        // The timer did not move the clock itself.
        assert_eq!(board.clock().now_ns(), report.virtual_ns);
        // The same elapsed time shows on the PM timer.
        assert_eq!(
            u64::from(pm_tmr_host_read(&mut board)),
            report.virtual_ns * 3_579_545 / 1_000_000_000
        );
    }

    #[test]
    fn run_boot_leaves_tmcct_frozen_when_the_timer_is_on_a_foreign_clock() {
        let foreign = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut board = load_firmware_image_inner(
            &tick_guest_image(40, 20, 20),
            &linux_config(),
            PciEcam::new(),
            None,
            Some(foreign),
        )
        .unwrap();
        arm_apic_timer(&mut board);
        program_channel_2(&mut board);
        let report = board.run_boot(1_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert!(report.virtual_ns > 80_000, "the board clock moved");
        assert_eq!(apic_read(&mut board, TMCCT), Ok(TM_INITIAL));
    }

    #[test]
    fn a_guest_cannot_read_lapic_mmio_through_the_paged_probe_yet() {
        // Observation: the memory path of the paged probe is RAM only (and only
        // the qword MOV RAX,[RBX]); a guest read of TMCCT stops at the MMIO
        // region before any device is touched. Guest MMIO is S4-38 / a6 work.
        let image = minor2_image(&[0x48, 0x8b, 0x03, 0xf4]);
        let mut board = load_firmware_image(&image, &linux_config()).unwrap();
        arm_apic_timer(&mut board);
        board.state_mut().gpr[Register::Rbx as usize] = LOCAL_APIC_MMIO_BASE + TMCCT;
        let report = board.run_boot(10).unwrap();
        assert!(
            matches!(
                report.memory_error,
                Some(TrackedAccessError::OperandPhysical {
                    source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Mmio),
                    ..
                })
            ),
            "{:?}",
            report.memory_error
        );
        assert_eq!(
            apic_read(&mut board, TMCCT),
            Ok(TM_INITIAL - report.virtual_ns / 10)
        );
    }

    // ---- S4-37L4: board refactor golden (values taken before the refactor) ----

    /// Every optional device and the Linux profile at once, on 512 MiB.
    fn golden_full_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            ram_bytes: 512 << 20,
            second_screen: true,
            nvme: Some(NvmeConfig::default()),
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            linux_profile: true,
            ..reference_config()
        }
    }

    const GOLDEN_FULL_HANDOFF_HEX: &str = concat!(
        "4f4448414e444f4601000200a000000090040000ff227302000000000100000000ca9a3b000000000100000000000000",
        "a00000001200000020000000e00200000900000030000000101112131415161718191a1b1c1d1e1f0700000000000000",
        "4f442d303030310000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "000000000000000000000000000000000100000000000000000000000000000000000e0000000000ffffffff00000000",
        "0b0000000000000000000e000000000000f0000000000000ffffffff000000000c0000000000000000f00e0000000000",
        "0010000000000000ffffffff00000000010000000000000000000f00000000000000010000000000ffffffff00000000",
        "070000000000000000001000000000000000010000000000ffffffff0000000008000000000000000000110000000000",
        "0000040000000000ffffffff000000000100000000000000000015000000000000000b0000000000ffffffff00000000",
        "090000000000000000002000000000000000400000000000ffffffff000000000a000000000000000000600000000000",
        "00d0120000000000ffffffff00000000010000000000000000d072000000000000308d1f00000000ffffffff00000000",
        "0600000001000000000000f400000000000040020000000003000000000000000600000000000000000000f800000000",
        "000040020000000003000000000000000300000000000000000000fc0000000000100000000000000300000000000000",
        "0300000000000000001000fc00000000002000000000000007000000000000000300000000000000000010fc00000000",
        "0000100000000000060000000000000003000000000000000000c0fe0000000000100000000000000200000000000000",
        "03000000000000000000e0fe000000000010000000000000010000000000000004000000000000000000c0ff00000000",
        "0000400000000000ffffffff000000000100000003000000f80300000000000008000000000000000400000000000000",
        "0100000000000000000000000000000002000000010000000000e0fe000000000010000000000000ffffffff00000000",
        "0000000000000000000000000000000003000000010000000000c0fe000000000010000000000000ffffffff00000000",
        "000000000000000000000000000000000400000001000000000000fc000000000010000000000000ffffffff00000000",
        "00000000000000000000000000000000050000000000000000000000000000000000000000000000ffffffff00000000",
        "00000000000000000000000000000000060000000000000000000000000000000000000000000000ffffffff00000000",
        "000000000000000000000000000000000700000001000000000010fc000000000000100000000000ffffffff00000000",
        "000000000000000000000000000000000800000001000000001000fc000000000020000000000000ffffffff00000000",
        "000000000000000000000000000000000900000003000000000600000000000010000000000000000900000003000000",
        "0006040608060c060605000000000000"
    );

    #[test]
    fn board_refactor_golden_handoff_manifest_hash_tables_and_entry_state() {
        let config = golden_full_config();
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
        assert_eq!(hex(board.handoff_block()), GOLDEN_FULL_HANDOFF_HEX);
        assert_eq!(
            hex(&config.acpi_manifest_hash().unwrap()),
            "e0a4281a967d6e6fe2ed939210ec74a66ee718baa63aa4d761e434d34945e9ee"
        );
        assert_eq!(
            hex(&tables_config_with_ecam().acpi_manifest_hash().unwrap()),
            "2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2"
        );
        assert_eq!(board.boot_page_table_bytes(), 0x12d000);
        assert_eq!(board.cr3(), 0x0060_0000);
        let mut tables = vec![0u8; board.boot_page_table_bytes() as usize];
        let cr3 = board.cr3();
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(cr3), &mut tables)
            .unwrap();
        assert_eq!(crc32_ieee(&[&tables]), 0x7d9e_71ea);
        let mut shadow = vec![0u8; FIRMWARE_IMAGE_BYTES];
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_LINK_BASE), &mut shadow)
            .unwrap();
        assert_eq!(crc32_ieee(&[&shadow]), 0x161a_397f);
        let state = board.state();
        assert_eq!(state.rip, 0x0020_0040);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x0015_0000);
        assert_eq!(state.gpr[Register::Rdi as usize], 0x0010_0000);
        assert_eq!(state.cr3, None);
        assert_eq!(state.cr0, Some(0x8001_0033));
        assert_eq!(state.cr4, Some(0x20));
    }

    // ---- S4-38L3: the set's final exit expressed as a probe exit ----

    #[test]
    fn cpuid_divergence_is_a_stop_of_the_board_told_apart_by_its_leaf() {
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::CpuidDivergence { leaf: 1 }),
            Ok((
                PagedProbeExit::Stopped,
                "CpuidDivergence { leaf: 1 }".to_string()
            ))
        );
    }

    #[test]
    fn user_entry_is_a_stop_of_the_board_with_its_frame_in_the_detail_not_an_error() {
        let entry = IretqUserEntry {
            rip: 0x0000_7fff_0000_1000,
            cs: 0x33,
            rflags: 0x246,
            rsp: 0x0000_7fff_ffff_f000,
            ss: 0x2b,
        };
        let exit = PagedCpl0SetExit::UserEntry(entry);
        assert_eq!(
            set_exit_to_probe_exit(exit),
            Ok((PagedProbeExit::Stopped, format!("UserEntry({entry:?})")))
        );
        // The stop never resumes, whatever budget is left.
        assert!(!resumes_set_exit(
            &PagedCpl0SetExit::UserEntry(entry),
            false,
            5,
            1
        ));
    }

    #[test]
    fn irq_delivery_failure_is_an_error_of_the_run_and_a_deadline_cut_resumes() {
        use crate::paged_memory_adapter::IrqDeliveryFailure;
        use od_isa::Cpl0ExternalInterruptRefusal;
        assert!(
            set_exit_to_probe_exit(PagedCpl0SetExit::IrqDeliveryFailed {
                vector: 0x30,
                failure: IrqDeliveryFailure::Refused(Cpl0ExternalInterruptRefusal::GateNotPresent),
            })
            .is_err()
        );
        let budget = PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted);
        assert!(resumes_deadline_cut(&budget, true, false, 5, 3));
        assert!(!resumes_deadline_cut(&budget, false, false, 5, 3), "no cut");
        assert!(
            !resumes_deadline_cut(&budget, true, true, 5, 3),
            "guest stop"
        );
        assert!(
            !resumes_deadline_cut(&budget, true, false, 0, 3),
            "no budget"
        );
        assert!(
            !resumes_deadline_cut(&budget, true, false, 5, 0),
            "no progress"
        );
        let tick = PagedCpl0SetExit::Run(RunExit::StopRequested);
        assert!(!resumes_deadline_cut(&tick, true, false, 5, 3));
    }

    #[test]
    fn set_exit_without_a_probe_counterpart_is_an_integration_error_of_the_board() {
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::UnknownSetExit),
            Err("the CPL0 set returned an exit variant the board does not know")
        );
        // The loop passes no stop address.
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::Run(RunExit::StopAddress)),
            Err("the CPL0 set returned an exit the board never asks for")
        );
    }

    #[test]
    fn partial_rep_exit_is_a_stop_of_the_board_not_an_integration_error() {
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)),
            Ok((
                PagedProbeExit::Stopped,
                "Run(RepIterationBudgetExhausted)".to_string()
            ))
        );
    }

    #[test]
    fn resume_decision_for_the_partial_rep_and_the_tick_stop() {
        let partial = PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted);
        let tick = PagedCpl0SetExit::Run(RunExit::StopRequested);
        // A partial REP resumes with budget left, also when nothing retired in the quantum.
        assert!(resumes_set_exit(&partial, false, 5, 0));
        assert!(resumes_set_exit(&partial, false, 5, 3));
        // Not when the guest asked for the stop, and not when the budget is spent.
        assert!(!resumes_set_exit(&partial, true, 5, 3));
        assert!(!resumes_set_exit(&partial, false, 0, 3));
        // The tick stop keeps its rule: it needs a retired instruction.
        assert!(resumes_set_exit(&tick, false, 5, 1));
        assert!(!resumes_set_exit(&tick, false, 5, 0));
        assert!(!resumes_set_exit(&tick, true, 5, 1));
        // Other exits never resume.
        for exit in [
            PagedCpl0SetExit::Run(RunExit::Halted),
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            PagedCpl0SetExit::CpuidDivergence { leaf: 1 },
        ] {
            assert!(!resumes_set_exit(&exit, false, 5, 1));
        }
    }

    #[test]
    fn set_exit_integration_errors_keep_their_fields_and_the_debug_text() {
        let effect = TranslationContextChange::Cr3Write {
            old_cr3: 0x1000,
            new_cr3: 0x2000,
        };
        let exit = PagedCpl0SetExit::Cr3EffectMismatch {
            active_cr3: 0x1000,
            effect_old_cr3: 0x1000,
            cpu_cr3: Some(0x3000),
            effect_new_cr3: 0x2000,
        };
        assert_eq!(
            set_exit_to_probe_exit(exit),
            Ok((
                PagedProbeExit::IntegrationError(PagedProbeIntegrationError::Cr3EffectMismatch {
                    active_cr3: 0x1000,
                    effect_old_cr3: 0x1000,
                    cpu_cr3: Some(0x3000),
                    effect_new_cr3: 0x2000,
                }),
                "Cr3EffectMismatch { active_cr3: 4096, effect_old_cr3: 4096, cpu_cr3: Some(12288), \
                 effect_new_cr3: 8192 }"
                    .to_string()
            ))
        );
        let error = crate::ProbeConfigError::InvalidCr3 {
            value: 1 << 36,
            offending_bits: 1 << 36,
        };
        let (mapped, detail) = set_exit_to_probe_exit(PagedCpl0SetExit::Cr3SnapshotRejected {
            effect,
            error: error.clone(),
        })
        .expect("mapped");
        assert_eq!(
            mapped,
            PagedProbeExit::IntegrationError(PagedProbeIntegrationError::Cr3SnapshotRejected {
                effect,
                error
            })
        );
        assert_eq!(
            detail,
            "Cr3SnapshotRejected { effect: Cr3Write { old_cr3: 4096, new_cr3: 8192 }, error: \
             InvalidCr3 { value: 68719476736, offending_bits: 68719476736 } }"
        );
        let effect = TranslationContextChange::Cr0Write {
            old_cr0: 1,
            new_cr0: 2,
        };
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::UnexpectedContextChange { effect }),
            Ok((
                PagedProbeExit::IntegrationError(
                    PagedProbeIntegrationError::UnexpectedContextChange { effect }
                ),
                "UnexpectedContextChange { effect: Cr0Write { old_cr0: 1, new_cr0: 2 } }"
                    .to_string()
            ))
        );
    }
}
