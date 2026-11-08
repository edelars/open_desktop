//! Firmware board v1: ROM backing, shadow copy, handoff block v1.0 and the
//! direct Long64 entry described by `docs/specs/FIRMWARE_BOOT.md`.
//!
//! The machine knows nothing about UEFI, PE or ACPI here: it validates the
//! flat `ODFWIMG1` header, fills ROM, copies the image to the RAM shadow,
//! builds the temporary 4 KiB identity page tables and the handoff block, and
//! hands over a `LongModeState`. It is not a reset vector and not real mode.

use crate::{
    AccessWidth, BusAccessError, BusBuildError, DISPLAY_FRAMEBUFFER_BASE,
    DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES,
    DISPLAY_SECOND_FRAMEBUFFER_BASE, DisplayDevice, DisplayHandle, GuestPhysicalAddress,
    GuestPhysicalRange, IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic, LOCAL_APIC_MMIO_BASE,
    LOCAL_APIC_MMIO_SIZE, LocalApic, MachineBus, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, MmioBinding, NVME_MMIO_BASE, NVME_MMIO_BYTES, NvmeConfig,
    NvmeConfigError, NvmeDevice, NvmeHandle, PagedProbeExit, PagedProbeQuantumReport, PortAddress,
    PortRange, PortRegionBinding, RamLimits, TrackedAccessError, Uart16550Handle,
    Uart16550PollingTx, VirtualClock, VirtualClockError, run_paged_probe,
};
use od_isa::memory::{
    AccessContext, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

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

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const LOW_RAM_LIMIT: u64 = 0xF400_0000;
const HIGH_RAM_BASE: u64 = 0x1_0000_0000;
const PAGE: u64 = 0x1000;
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

const DEVICE_PRESENT: u32 = 1;
const DEVICE_PIO: u32 = 2;
const DEV_COM1: usize = 0;
const DEV_LAPIC: usize = 1;
const DEV_IOAPIC: usize = 2;
const DEV_DISPLAY: usize = 3;
/// Device-table index and bus index of the optional NVMe controller (device code 8).
const DEV_NVME_TABLE: usize = 7;
const DEV_NVME_BUS: usize = 4;
const DEVICE_CODE_NVME: u32 = 8;
/// Handoff minor version emitted when the NVMe entry is present.
const HANDOFF_MINOR_NVME: u16 = 1;

/// Host-supplied board configuration. Defaults follow FIRMWARE_BOOT.md 6.1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareBoardConfig {
    /// Total RAM `R`: a multiple of 2 MiB in `[128 MiB, 0xF_F400_0000]`.
    pub ram_bytes: u64,
    /// Second display screen (DISPLAY_DEVICE.md 16.3).
    pub second_screen: bool,
    pub tsc_hz: u64,
    /// SMBIOS Type 1 UUID bytes, copied verbatim; all zero means "absent".
    pub system_uuid: [u8; 16],
    /// ASCII `0x20..=0x7E`, at most 64 bytes; empty means no serial number.
    pub serial: Vec<u8>,
    /// NVMe controller (S7-16). `None` keeps handoff v1.0 byte for byte; `Some`
    /// adds device code 8 and raises `version_minor` to 1 (FIRMWARE_BOOT.md 6.3).
    pub nvme: Option<NvmeConfig>,
}

impl Default for FirmwareBoardConfig {
    fn default() -> Self {
        Self {
            ram_bytes: FIRMWARE_MIN_RAM_BYTES,
            second_screen: false,
            tsc_hz: 1_000_000_000,
            system_uuid: [0; 16],
            serial: Vec::new(),
            nvme: None,
        }
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

/// Device table of firmware board v1 (FIRMWARE_BOOT.md 6.3). NVRAM, TPM CRB
/// and ECAM are `PRESENT = 0` until S7-10, S7-22 and S4-30 add them. With `nvme`
/// the table gets the eighth entry, device code 8 (FIRMWARE_BOOT.md 6.3).
fn board_devices(nvme: bool) -> Vec<DeviceEntry> {
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
        mmio(4, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
        DeviceEntry::absent(5),
        DeviceEntry::absent(6),
        DeviceEntry::absent(7),
    ];
    debug_assert_eq!(devices.len(), BASE_DEVICE_COUNT);
    if nvme {
        // Polling only: no IRQ route in slice A (`gsi = 0xFFFF_FFFF`).
        devices.push(mmio(DEVICE_CODE_NVME, NVME_MMIO_BASE, NVME_MMIO_BYTES));
    }
    devices
}

fn range(start: u64, len: u64) -> Result<GuestPhysicalRange, FirmwareBoardError> {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len)
        .map_err(FirmwareBoardError::Memory)
}

fn board_map(
    config: &FirmwareBoardConfig,
    devices: &[DeviceEntry],
) -> Result<MachineMemoryMap, FirmwareBoardError> {
    let low_end = config.ram_bytes.min(LOW_RAM_LIMIT);
    let high = config.ram_bytes - low_end;
    let mut regions = vec![MemoryRegionSpec::new(
        range(0, low_end)?,
        MemoryRegionKind::Ram,
    )];
    if high > 0 {
        regions.push(MemoryRegionSpec::new(
            range(HIGH_RAM_BASE, high)?,
            MemoryRegionKind::Ram,
        ));
    }
    regions.push(MemoryRegionSpec::new(
        range(FIRMWARE_ROM_BASE, FIRMWARE_IMAGE_BYTES as u64)?,
        MemoryRegionKind::Rom,
    ));
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
    expected_pages: u64,
) -> Result<Vec<u8>, FirmwareBoardError> {
    let mut entries: Vec<u64> = vec![0; 512];
    let mut pdpt: BTreeMap<u64, usize> = BTreeMap::new();
    let mut pd: BTreeMap<u64, usize> = BTreeMap::new();
    let mut pt: BTreeMap<u64, usize> = BTreeMap::new();
    let table_address = |page: usize| FIRMWARE_BOOT_PAGE_TABLES_GPA + page as u64 * PAGE;
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
                let pieces = [
                    (0, FIRMWARE_HANDOFF_GPA, REGION_RAM),
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
                ];
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
    put16(
        &mut block,
        0x0A,
        if devices.len() > BASE_DEVICE_COUNT {
            HANDOFF_MINOR_NVME
        } else {
            0
        },
    );
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
        put32(&mut block, at, device.code);
        put32(&mut block, at + 0x04, device.flags);
        put64(&mut block, at + 0x08, device.base);
        put64(&mut block, at + 0x10, device.length);
        put32(&mut block, at + 0x18, device.gsi);
        put32(&mut block, at + 0x1C, device.irq_flags);
        put64(&mut block, at + 0x20, device.param0);
        put64(&mut block, at + 0x28, device.param1);
    }
    let crc = crc32_ieee(&[&block]);
    put32(&mut block, 0x14, crc);
    block
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
}

/// A loaded firmware board: machine, direct-entry CPU state and handoff block.
pub struct LoadedFirmwareBoard {
    bus: MachineBus,
    state: LongModeState,
    uart: Uart16550Handle,
    display: DisplayHandle,
    nvme: Option<NvmeHandle>,
    clock: VirtualClock,
    handoff: Vec<u8>,
    page_table_bytes: u64,
    /// COM1 bytes recorded by the latest `run_boot`, kept even when it fails.
    com1_log: Vec<u8>,
}

impl LoadedFirmwareBoard {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut LongModeState {
        &mut self.state
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

    pub const fn cr3(&self) -> u64 {
        FIRMWARE_BOOT_PAGE_TABLES_GPA
    }

    pub const fn efer(&self) -> u64 {
        FIRMWARE_EFER
    }

    /// The exact bytes written to RAM at [`FIRMWARE_HANDOFF_GPA`].
    pub fn handoff_block(&self) -> &[u8] {
        &self.handoff
    }

    /// Length of the `BOOT_PAGE_TABLES` region (`4096 * N`).
    pub const fn boot_page_table_bytes(&self) -> u64 {
        self.page_table_bytes
    }

    pub fn clock(&self) -> &VirtualClock {
        &self.clock
    }

    /// Explicit virtual-time advance; time never moves on its own
    /// (MACHINE_CONTRACT.md, caller-driven virtual clock).
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), FirmwareBoardError> {
        self.clock
            .advance_time_ns(delta_ns)
            .map_err(FirmwareBoardError::Clock)
    }

    /// One quantum of the existing bounded paged interpreter path.
    pub fn run(
        &mut self,
        instruction_budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> PagedProbeQuantumReport {
        run_paged_probe(
            &mut self.state,
            &mut self.bus,
            FIRMWARE_BOOT_PAGE_TABLES_GPA,
            FIRMWARE_EFER,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
            instruction_budget,
            INTEGER_FLAGS,
            stop,
            committed_write,
        )
    }

    /// Run quanta until the guest halts, faults or the instruction budget is
    /// spent. Each quantum ends on a COM1 THR write (FIRMWARE_BOOT.md 11); the
    /// byte is moved through the UART and recorded, then virtual time advances
    /// by `retired * 1 ns` before the next quantum (FIRMWARE_BOOT.md 8).
    pub fn run_boot(
        &mut self,
        instruction_budget: u64,
    ) -> Result<FirmwareRunReport, FirmwareBoardError> {
        let mut remaining = instruction_budget;
        let mut retired = 0u64;
        let mut quanta = 0u64;
        self.com1_log.clear();
        let nvme = self.nvme.clone();
        // Doorbells the host rang before the run are executed before the guest starts.
        if let Some(nvme) = &nvme {
            nvme.service(&mut self.bus, |_| {});
        }
        loop {
            let uart = self.uart.clone();
            let report = self.run(
                remaining,
                || {
                    uart.holding_occupied()
                        || nvme.as_ref().is_some_and(NvmeHandle::doorbell_pending)
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
            self.advance_time_ns(report.instructions_executed)?;
            let exit = match report.exit {
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
                virtual_ns: self.clock.now_ns(),
                tsc_hz: self.clock.tsc_hz(),
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
    let required_minor = if config.nvme.is_some() {
        HANDOFF_MINOR_NVME
    } else {
        0
    };
    let entry_offset = validate_image(image, required_minor)?;
    validate_config(config)?;

    let devices = board_devices(config.nvme.is_some());
    let map = board_map(config, &devices)?;
    let table_pages = boot_page_table_count(&map);
    let page_table_bytes = table_pages * PAGE;
    let low_end = config.ram_bytes.min(LOW_RAM_LIMIT);
    let tables_end = FIRMWARE_BOOT_PAGE_TABLES_GPA + page_table_bytes;
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
    let tables = build_boot_tables(&map, table_pages)?;

    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let lapic = LocalApic::new();
    let ioapic = IoApic::new(lapic.handle());
    let (display_device, display) = if config.second_screen {
        DisplayDevice::new_two_screens_with_handle()
    } else {
        DisplayDevice::new_with_handle()
    };
    // Bus order: COM1, LAPIC, IOAPIC, display, then the optional NVMe controller.
    let mut bus_devices: Vec<Box<dyn crate::BusDevice>> = vec![
        Box::new(uart_device),
        Box::new(lapic),
        Box::new(ioapic),
        Box::new(display_device),
    ];
    let mut mmio: Vec<MmioBinding> = [DEV_LAPIC, DEV_IOAPIC, DEV_DISPLAY]
        .iter()
        .map(|&index| MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[index].base),
            device_index: index,
        })
        .collect();
    // The device exists on the bus only when the manifest entry is PRESENT.
    let mut nvme = None;
    if let Some(nvme_config) = &config.nvme
        && devices[DEV_NVME_TABLE].present()
    {
        let (device, handle) =
            NvmeDevice::new_with_handle(nvme_config.clone()).map_err(FirmwareBoardError::Nvme)?;
        bus_devices.push(Box::new(device));
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_NVME_TABLE].base),
            device_index: DEV_NVME_BUS,
        });
        nvme = Some(handle);
    }
    let com1 = PortRange::new(
        PortAddress::new(devices[DEV_COM1].base as u16),
        devices[DEV_COM1].length as u32,
    )
    .map_err(|_| FirmwareBoardError::BusBuild(BusBuildError::InvalidPortRange))?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(config.ram_bytes),
        bus_devices,
        mmio,
        vec![PortRegionBinding {
            range: com1,
            device_index: DEV_COM1,
        }],
    )
    .map_err(FirmwareBoardError::BusBuild)?;

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
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(FIRMWARE_BOOT_PAGE_TABLES_GPA),
        &tables,
        |_| {},
    )
    .map_err(FirmwareBoardError::Bus)?;
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(FIRMWARE_HANDOFF_GPA),
        &handoff,
        |_| {},
    )
    .map_err(FirmwareBoardError::Bus)?;
    // The polling UART rejects THR writes until 8N1 and a divisor are set; the
    // machine presents it configured (as the flat probe does). The firmware's
    // own initialisation (FIRMWARE_BOOT.md 11) writes the same values.
    for (offset, value) in [(3, 0x83), (0, 0x01), (1, 0x00), (3, 0x03)] {
        bus.write_port(
            PortAddress::new(FIRMWARE_COM1_PORT + offset),
            AccessWidth::Byte,
            value,
        )
        .map_err(FirmwareBoardError::Bus)?;
    }

    let mut state = LongModeState {
        cr3: None,
        rip: FIRMWARE_LINK_BASE + entry_offset,
        rflags: 2,
        cr0: Some(CR0),
        cr4: Some(CR4),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = FIRMWARE_BOOT_STACK_TOP;
    state.gpr[Register::Rdi as usize] = FIRMWARE_HANDOFF_GPA;
    let clock = VirtualClock::new(config.tsc_hz).map_err(FirmwareBoardError::Clock)?;
    Ok(LoadedFirmwareBoard {
        bus,
        state,
        uart,
        display,
        nvme,
        clock,
        handoff,
        page_table_bytes,
        com1_log: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let devices = board_devices(false);
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
            let low_end = ram_bytes.min(LOW_RAM_LIMIT);
            let mut specs = vec![MemoryRegionSpec::new(
                range(0, low_end).unwrap(),
                MemoryRegionKind::Ram,
            )];
            if ram_bytes > low_end {
                specs.push(MemoryRegionSpec::new(
                    range(HIGH_RAM_BASE, ram_bytes - low_end).unwrap(),
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
}
