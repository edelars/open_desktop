//! Firmware board v1: ROM backing, shadow copy, handoff block v1.0 and the
//! direct Long64 entry described by `docs/specs/FIRMWARE_BOOT.md`.
//!
//! The machine knows nothing about UEFI, PE or ACPI here: it validates the
//! flat `ODFWIMG1` header, fills ROM, copies the image to the RAM shadow,
//! builds the temporary 4 KiB identity page tables and the handoff block, and
//! hands over a `LongModeState`. It is not a reset vector and not real mode.

use crate::agent_channel::AgentChannel;
use crate::clock_owner::{ClockOwner, ClockOwnerError, IdleOutcome};
use crate::console_output_context::{
    ConsoleContextTrigger, ConsoleLineTrigger, ConsoleOutputContextSnapshot, MAX_CAPTURE_COUNT,
    UartOutputByteContextSnapshot, UartOutputContextCapture, UartOutputContextDiagnostic,
    capture_frame_chain,
};
use crate::crc32::crc32_ieee;
use crate::decoded_instruction_cache::DecodedInstructionCache;
use crate::execution_observations::{
    LinuxExecutionObservationCollector, LinuxExecutionObservations, RdtscClockReadCompletion,
    RdtscClockReadLedger,
};
use crate::fetch_cache_report::{CallObservation, FetchCacheAccumulator, LinuxFetchCacheReport};
use crate::fetch_cache_sweep::{FetchCacheSweep, FetchSweepReport, SweepConfig};
use crate::fetch_translation_cache::{FetchCacheInvalidation, FetchCacheTag};
use crate::idle_arbiter::{SchedActivity, WakeView};
use crate::irq_source::{BoardIrqSource, IrqBoundaryHook, IrqCounters, NoIrqSource};
use crate::linux_checkpoints::{
    CheckpointRecorder, LinuxCheckpointRecord, LinuxCheckpointTable, LinuxCheckpointTableError,
};
use crate::nic_e1000::irq_source::NicIoapicIrqSource;
use crate::nic_e1000::{
    NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES, NIC_PCI_SLOT, NicConfig, NicDevice, NicHandle,
};
use crate::nvme::irq_source::NvmeIoapicIrqSource;
use crate::nvme::{NVME_PCI_BAR_BASE, NVME_PCI_BAR_BYTES, NVME_PCI_SLOT};
use crate::nvram_device::{
    NVRAM_MMIO_BASE, NVRAM_MMIO_BYTES, NvramConfig, NvramContentsError, NvramDevice, NvramHandle,
};
use crate::page_walk::translate_long64_2m_ram_read_only;
use crate::paged_memory_adapter::deliver_nmi_at_boundary;
use crate::paged_memory_adapter::{
    IrqDeliveryFailure, PagedCpl0SetExit, run_cpl0_set_paged_2m_ram_observed_with_sweep,
};
use crate::paged_probe::{FirmwareMmio, run_paged_firmware_probe, run_paged_probe_2m_ram};
use crate::pit_ioapic_irq_source::PitIoapicIrqSource;
use crate::pit_pic_irq_source::PitPicIrqSource;
use crate::smp_scheduler::{
    BoardSmpHost, Executed, QuantumPolicy, SmpError, SmpScheduler, StopRequest,
};
use crate::stop_diagnostic::{self, VcpuDiagnosticState};
use crate::tpm_crb::{TPM_CRB_BASE, TPM_CRB_LEN, TpmConfig, TpmCrb, TpmCrbHandle, TpmPlatformPort};
use crate::uart_ioapic_irq_source::UartIoapicIrqSource;
use crate::uart_pic_irq_source::UartPicIrqSource;
use crate::vcpu::{
    BOOT_VCPU_INDEX, LINUX_AP_CPUID_PROFILE, MAX_VCPUS, NmiBoundary, StartupApplied, Vcpu,
    VcpuCpuid, vcpu_set,
};
use crate::xhci::irq_source::XhciIoapicIrqSource;
use crate::{
    ACPI_PM_DESCRIPTOR, ACPI_SCI_GSI, ACPI_SCI_IRQ_FLAGS, AbsentPortRegion, AbsentPorts,
    AccessWidth, AcpiPm, AcpiPmHandle, BusAccessError, BusBuildError, CMOS_INDEX_PORT,
    CMOS_PORT_BYTES, DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE,
    DISPLAY_REGISTERS_BYTES, DISPLAY_SECOND_FRAMEBUFFER_BASE, DMA_PAGE_0_PORT,
    DMA_PAGE_0_PORT_BYTES, DisplayDevice, DisplayHandle, DmaPage0, GuestLinearAddress,
    GuestPhysicalAddress, GuestPhysicalRange, IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic,
    LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle, Long64ProbeConfig,
    MachineBus, MachineMemoryMap, MachineStopReason, MemoryAccessError, MemoryRegionKind,
    MemoryRegionSpec, MmioBinding, NVME_MMIO_BASE, NVME_MMIO_BYTES, NvmeConfig, NvmeConfigError,
    NvmeDevice, NvmeHandle, PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES, PCI_ECAM_DESCRIPTOR,
    PIC_ELCR_BASE_PORT, PIC_ELCR_PORT_BYTES, PIC_MASTER_BASE_PORT, PIC_PORT_BYTES,
    PIC_SLAVE_BASE_PORT, PIT_BASE_PORT, PIT_PORT_B, PIT_PORT_B_BYTES, PIT_PORT_BYTES,
    PagedProbeExit, PagedProbeIntegrationError, PagedProbeQuantumReport, PagedProbeRejection,
    PciEcam, Pic8259Handle, Pic8259Pair, Pit8254Devices, Pit8254Handle, PortAddress, PortRange,
    PortRegionBinding, RamLimits, RtcCmos, TrackedAccessError, Uart16550Handle, Uart16550PollingTx,
    UartPortSpec, VirtualClock, VirtualClockError, VirtualClockHandle, run_paged_probe,
    run_paged_probe_with_rdtsc,
};
use crate::{
    DiagnosticReadError, DiagnosticReadFailure, DiagnosticTranslateError, DisplayReadError,
    FrameInfo, HidDevices, XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES, XHCI_PCI_SLOT, XhciConfig,
    XhciDevice, XhciHandle,
};
use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{Cpl0ProfileSet, GuestFpState, RunExit, TranslationContextChange};
use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, InstructionMode, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, IretqUserEntry, LongModeState, Register, StiShadow};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) mod vcpu_quantum;

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
/// `IA32_EFER.NXE` (bit 11).
const EFER_NXE: u64 = 1 << 11;
/// ACPI table area (S4-31c, `acpi_tables`): `[0xE_0000, 0xE_F000)`, an E820 ACPI
/// reclaim range in S4-37; the RSDP will sit at its first byte.
pub const FIRMWARE_ACPI_TABLES_BASE: u64 = 0x000E_0000;
pub const FIRMWARE_ACPI_TABLES_BYTES: u64 = 0xF000;
/// FACS page `[0xE_F000, 0xF_0000)`, an E820 ACPI NVS range in S4-37.
pub const FIRMWARE_ACPI_FACS_BASE: u64 = 0x000E_F000;
pub const FIRMWARE_ACPI_FACS_BYTES: u64 = 0x1000;

pub(crate) const FIRMWARE_CR0: u64 = 0x8001_0033;
pub(crate) const FIRMWARE_CR4: u64 = 0x20;

/// GDTR modeled for the unified loop (S7-31c): base 0, limit 0xFFFF, the value after reset
/// (Intel SDM Vol. 3A, 10.1.1, Table 10-1).
pub(crate) const UNIFIED_ENTRY_GDTR_BASE: u64 = 0;
pub(crate) const UNIFIED_ENTRY_GDTR_LIMIT: u16 = 0xffff;
/// Hidden segment caches modeled for the unified loop: limit 0xFFFF_FFFF, access 0x93 (present,
/// writable data), flags 0xC (D/B and G). These are NOT reset values (a reset gives limit
/// 0xFFFF); they are the flat values of the Linux entry state. In 64-bit mode base and limit of
/// SS/DS/ES/FS/GS are not used, so they affect nothing the guest can observe.
pub(crate) const UNIFIED_ENTRY_SEGMENT_LIMIT: u32 = 0xffff_ffff;
pub(crate) const UNIFIED_ENTRY_SEGMENT_ACCESS: u8 = 0x93;
pub(crate) const UNIFIED_ENTRY_SEGMENT_FLAGS: u8 = 0xc;

/// The descriptor-table and segment-cache state the unified loop (S7-31c) gives a ROM entry
/// state, which leaves all of it unmodeled (`LongModeState::default()`, FIRMWARE_BOOT.md, entry
/// contract). The CPL0 set refuses `LGDT` without a modeled GDTR and loads a selector into
/// SS/DS/ES/FS/GS only over a modeled cache. Each field is set only when it is unmodeled, so a
/// second call changes nothing. The ROM replaces all of it with its own GDT in
/// `od_fw_install_gdt`, its first function.
pub(crate) fn model_unified_entry_descriptors(state: &mut LongModeState) {
    let data = od_isa::DataSegmentDescriptorCache {
        base: 0,
        limit: UNIFIED_ENTRY_SEGMENT_LIMIT,
        access: UNIFIED_ENTRY_SEGMENT_ACCESS,
        flags: UNIFIED_ENTRY_SEGMENT_FLAGS,
    };
    let flat = od_isa::FsDescriptorCache {
        limit: UNIFIED_ENTRY_SEGMENT_LIMIT,
        access: UNIFIED_ENTRY_SEGMENT_ACCESS,
        flags: UNIFIED_ENTRY_SEGMENT_FLAGS,
    };
    state.gdtr.get_or_insert(od_isa::state::Gdtr {
        base: UNIFIED_ENTRY_GDTR_BASE,
        limit: UNIFIED_ENTRY_GDTR_LIMIT,
    });
    if state.ss_cache.is_none() {
        state.ss_cache = Some(data);
        state.ss_selector.get_or_insert(0);
    }
    state.ds_cache.get_or_insert(data);
    state.es_cache.get_or_insert(data);
    state.fs_cache.get_or_insert(flat);
    state.gs_cache.get_or_insert(flat);
}
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
const FV_APP_HANDOFF_HEADER_BYTES: usize = 0xA8;
const ACPI_HANDOFF_HEADER_BYTES: usize = 0xB0;

/// Charge one virtual execution cycle for each retired instruction on this board.
pub const FIRMWARE_EXECUTION_CYCLE_NS: u64 = 1;
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
const REGION_NVRAM: u32 = 5;
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
const DEV_NVRAM: usize = 4;
const DEVICE_CODE_NVRAM: u32 = 5;
const DEV_TPM: usize = 5;
const DEVICE_CODE_TPM: u32 = 6;
/// Device-table index and bus index of the optional NVMe controller (device code 8).
const DEV_NVME_TABLE: usize = 7;
const DEVICE_CODE_NVME: u32 = 8;
/// Device-table index of the PCIe ECAM entry (device code 7, a v1.0 slot).
const DEV_ECAM_TABLE: usize = 6;
const DEVICE_CODE_ECAM: u32 = 7;
/// ACPI fixed hardware: PM block and SCI route (S4-31c, handoff v1.2).
const DEVICE_CODE_ACPI_PM: u32 = 9;
/// RTC/CMOS ports and deterministic UTC base time (handoff v1.4, S7-14).
const DEVICE_CODE_RTC_CMOS: u32 = 10;
const HANDOFF_MINOR_RTC: u16 = 4;
const HANDOFF_MINOR_FV_APP: u16 = 5;
const HANDOFF_MINOR_FV_APP_VARS: u16 = 6;
const HANDOFF_MINOR_NVRAM: u16 = 7;
const HANDOFF_MINOR_FV_APP_RT: u16 = 8;
const HANDOFF_MINOR_FV_APP_ACPI: u16 = 9;
const HANDOFF_MINOR_FV_APP_GOP: u16 = 10;
const HANDOFF_MINOR_FV_APP_BLK: u16 = 11;
const HANDOFF_MINOR_FV_APP_FS: u16 = 12;
const HANDOFF_MINOR_TPM: u16 = 13;
const HANDOFF_MINOR_PROTECTED_NVRAM: u16 = 14;
const HANDOFF_MINOR_SECURE_BOOT: u16 = 15;
const HANDOFF_MINOR_PKCS7: u16 = od_uefi_security::pkcs7::transport::HANDOFF_MINOR;
const MAX_RTC_BASE_UNIX_SECONDS: u64 = 253_402_300_799;
/// Handoff minor version emitted when the NVMe entry is present.
const HANDOFF_MINOR_NVME: u16 = 1;
/// Handoff minor version emitted when the ACPI fixed hardware entry (code 9) is present.
const HANDOFF_MINOR_ACPI: u16 = 2;
const HANDOFF_MINOR_NVME_PCI: u16 = 3;

/// A diagnostic application selected from the built-in firmware volume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FirmwareFvApp {
    Hello = 1,
    Ebs = 2,
    Vars = 3,
    Rt = 4,
    Acpi = 5,
    Gop = 6,
    Blk = 7,
    Fs = 8,
    Tpm = 9,
    /// Counts boots in an NVRAM variable and resets the machine (S7-26); handoff v1.13 like `Tpm`.
    Reboot = 10,
}

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
    /// Optional variable device. Its presence uses handoff v1.7 and slot 5.
    pub nvram: Option<NvramConfig>,
    /// Optional TPM 2.0 CRB. Requires ACPI and handoff v1.13.
    pub tpm: Option<TpmConfig>,
    /// Direct Linux boot only: expose NVMe at 00:04.0 with BAR0/1 and INTx.
    /// Requires `nvme` and `linux_profile` and selects handoff v1.3.
    pub nvme_pci: bool,
    /// USB xHCI controller with a keyboard on port 1 and an absolute pointer on port 2
    /// (S8-42, S8-43; `XHCI_DEVICE.md`): PCI function 00:05.0 with BAR0 `[0xFC00_8000,
    /// 0xFC00_C000)`, INTA on GSI 17. Requires `pci_ecam`. `None` keeps the board, the handoff
    /// block and the ACPI manifest hash byte for byte: the controller has no handoff device
    /// code (firmware finds it by PCI enumeration), only the memory map and its region table
    /// gain the BAR window.
    pub xhci: Option<XhciConfig>,
    /// Network interface (S11-32; `NIC_DEVICE.md`): an 82540EM-compatible NIC at PCI function
    /// 00:06.0 with BAR0 `[0xFC02_0000, 0xFC04_0000)` and INTA on GSI 18, the board form of
    /// `network = nat`. Requires `pci_ecam`. `None` (`network = none`) keeps the board, the
    /// handoff block and the ACPI manifest hash byte for byte: the NIC has no handoff device
    /// code (firmware finds it by PCI enumeration), only the memory map and its region table
    /// gain the BAR window. The host connects the backend with `NicHandle::attach_backend`.
    pub nic: Option<NicConfig>,
    /// PCI configuration device (S4-30c): ECAM window plus conf1 ports 0xCF8..=0xCFF
    /// over bus 0. `false` keeps handoff v1.0 byte for byte (code 7
    /// `PRESENT = 0`); `true` makes code 7 `PRESENT = 1` (a v1.0 slot, so
    /// `version_minor` and the entry count do not change) and adds the MMIO
    /// region of the window. The Linux board may provide an NVMe PCI function.
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
    /// Optional deterministic RTC base time in UTC Unix seconds (S7-14). Supplying it
    /// adds handoff device code 10 and requires an image with `handoff_minor_max >= 4`.
    /// This value is explicit configuration; the machine never reads host wall time.
    pub rtc_base_unix_seconds: Option<u64>,
    /// A present selector requests the handoff v1.5 header extension.
    pub fv_app: Option<FirmwareFvApp>,
    /// Linux profile (S4-37a1), opt-in. Requires `acpi_pm`, `acpi_tables` and
    /// `pci_ecam` (otherwise `Config`). Binds, on the board clock: the PIT
    /// `0x40..=0x43` and port B `0x61`, the 8259 pair `0x20..=0x21` and
    /// `0xA0..=0xA1`, and the absent-device sinks (read `0xFF`, write ignored)
    /// for CMOS `0x70..=0x71`, `0xED` and `0x80`. The handoff and ACPI manifest
    /// are unchanged unless the separate `nvme_pci` option is selected.
    /// Not bound: `0x60/0x64` (a trap), LAPIC shared timer, port tick, IRQ delivery.
    pub linux_profile: bool,
    /// The agent channel (S11-21, ADR-0007): a second UART at COM2 (`0x2F8..0x2FF`, IRQ3, 16450
    /// profile) with bounded host queues ([`crate::AgentChannel`], `AGENT_CHANNEL_DEVICE.md`). `false`
    /// (the default) is the board as it was: nothing is bound at `0x2F8` (a trap, or the absent
    /// UART under the Linux profile, S7-31j). `true` binds the device there instead and takes
    /// `AbsentPortRegion::Uart2` off the Linux profile. It does not change the handoff block or the
    /// ACPI manifest unless the manifest records COM2 (see the spec). The run loops pump the
    /// channel with COM1 and end a quantum on its occupied THR; the IOAPIC board
    /// ([`Self::enable_uart_ioapic_irq4`]) routes its IRQ3 line to INTIN3. Needs no other option.
    pub agent_channel: bool,
    /// Number of vCPUs `N` (S6-11), `1..=4`; `1` is the board as it was. Each has its own Local
    /// APIC with APIC ID equal to its index, and the IOAPIC accepts those IDs as destinations.
    /// Only the boot processor runs; the handoff block, the ACPI blob and CPUID still describe
    /// one processor (their `N` is S6-15/S7-02), so they do not depend on this field.
    pub vcpu_count: usize,
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
            nvram: None,
            tpm: None,
            nvme_pci: false,
            xhci: None,
            nic: None,
            pci_ecam: false,
            acpi_pm: false,
            acpi_tables: false,
            rtc_base_unix_seconds: None,
            fv_app: None,
            linux_profile: false,
            agent_channel: false,
            vcpu_count: 1,
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
        let devices = configured_board_devices(self);
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
    Nvram(NvramContentsError),
    Tpm(od_tpm::StateError),
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
    /// An external interrupt could not be delivered during a Linux run. Keep the original
    /// typed failure's Debug form and the guest execution point for the host evidence record.
    ExternalInterruptDelivery {
        vector: u8,
        failure: String,
        rip: u64,
        cs_selector: Option<u16>,
    },
    /// An application processor in legacy mode (the trampoline between SIPI and its 64-bit jump,
    /// S6-16c) stopped on an instruction or a state the real-mode, protected-mode or long-mode
    /// entry models do not admit. The vCPU keeps its state (the refusal changes nothing).
    ApLegacyRefused {
        cpu: usize,
        refusal: crate::ap_legacy_exec::ApLegacyRefusal,
    },
    /// The long-mode quantum of a vCPU ([`LoadedFirmwareBoard::run_vcpu_set_quantum`], S6-16d)
    /// ended on something other than a budget, a stop request, a `HLT` or a partial `REP`: a
    /// fault, a page fault, an unsupported instruction, a refused configuration. `detail` is the
    /// `Debug` text of the set exit; the vCPU keeps the state the failing call left.
    VcpuQuantum {
        cpu: usize,
        detail: String,
    },
}

impl fmt::Display for FirmwareBoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "firmware board v1 load failed: {self:?}")
    }
}

impl std::error::Error for FirmwareBoardError {}

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
    if !(1..=MAX_VCPUS).contains(&config.vcpu_count) {
        return Err(FirmwareBoardError::Config(
            "vcpu_count must be in 1..=MAX_VCPUS",
        ));
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
    if config.fv_app == Some(FirmwareFvApp::Rt)
        && !(config.acpi_tables && config.rtc_base_unix_seconds.is_some() && config.nvram.is_some())
    {
        return Err(FirmwareBoardError::Config(
            "rt FV application needs ACPI fixed hardware, RTC and NVRAM",
        ));
    }
    if config.fv_app == Some(FirmwareFvApp::Acpi) && !config.acpi_tables {
        return Err(FirmwareBoardError::Config(
            "acpi FV application needs ACPI tables",
        ));
    }
    if config.fv_app == Some(FirmwareFvApp::Gop) && !config.display_enabled {
        return Err(FirmwareBoardError::Config(
            "gop FV application needs the primary display",
        ));
    }
    if matches!(config.fv_app, Some(FirmwareFvApp::Blk | FirmwareFvApp::Fs))
        && (config.nvme.is_none() || config.nvme_pci || config.linux_profile)
    {
        return Err(FirmwareBoardError::Config(
            "storage FV application needs fixed NVMe MMIO",
        ));
    }
    if config.tpm.is_some() && !config.acpi_tables {
        return Err(FirmwareBoardError::Config("TPM CRB needs ACPI tables"));
    }
    if config.fv_app == Some(FirmwareFvApp::Tpm) && config.tpm.is_none() {
        return Err(FirmwareBoardError::Config(
            "tpm FV application needs TPM CRB",
        ));
    }
    if config.fv_app == Some(FirmwareFvApp::Reboot)
        && !(config.tpm.is_some() && config.nvram.is_some())
    {
        return Err(FirmwareBoardError::Config(
            "reboot FV application needs the TPM CRB and NVRAM",
        ));
    }
    if config.linux_profile && !(config.acpi_pm && config.acpi_tables && config.pci_ecam) {
        return Err(FirmwareBoardError::Config(
            "linux_profile needs acpi_pm, acpi_tables and pci_ecam",
        ));
    }
    if config.xhci.is_some() && !config.pci_ecam {
        return Err(FirmwareBoardError::Config("xhci needs pci_ecam"));
    }
    // The Linux-profile loops do not serve the controller yet (the set loop opens no BAR0
    // window, the port reset has no wake deadline, the multi-vCPU path does not service it).
    if config.xhci.is_some() && config.linux_profile {
        return Err(FirmwareBoardError::Config(
            "xhci is served by the firmware profile only (no linux_profile)",
        ));
    }
    if let Some(nic) = &config.nic {
        if !config.pci_ecam {
            return Err(FirmwareBoardError::Config("nic needs pci_ecam"));
        }
        // Like the xHCI function: the Linux-profile loops do not serve it.
        if config.linux_profile {
            return Err(FirmwareBoardError::Config(
                "nic is served by the firmware profile only (no linux_profile)",
            ));
        }
        nic.validate().map_err(|_| {
            FirmwareBoardError::Config("nic mac must be unicast, non-zero and not the gateway")
        })?;
    }
    if config.nvme_pci && !(config.nvme.is_some() && config.linux_profile) {
        return Err(FirmwareBoardError::Config(
            "nvme_pci needs nvme and linux_profile",
        ));
    }
    if config.serial.len() > 64 || config.serial.iter().any(|b| !(0x20..=0x7e).contains(b)) {
        return Err(FirmwareBoardError::Config(
            "serial must be at most 64 ASCII 0x20..0x7E bytes",
        ));
    }
    if config
        .rtc_base_unix_seconds
        .is_some_and(|seconds| seconds > MAX_RTC_BASE_UNIX_SECONDS)
    {
        return Err(FirmwareBoardError::Config(
            "RTC base time must be a UTC Unix timestamp through 9999-12-31",
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

/// Device table of firmware board v1 (FIRMWARE_BOOT.md 6.3). The base NVRAM, TPM CRB,
/// and ECAM slots start absent. ECAM is `PRESENT = 0` unless
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
        DeviceEntry::absent(DEVICE_CODE_NVRAM),
        DeviceEntry::absent(DEVICE_CODE_TPM),
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
    let mut devices = board_devices(
        config.nvme.is_some(),
        config.pci_ecam,
        config.acpi_tables,
        config.display_enabled,
    );
    if let Some(nvram) = &config.nvram {
        devices[DEV_NVRAM] = DeviceEntry {
            flags: DEVICE_PRESENT,
            base: NVRAM_MMIO_BASE,
            length: NVRAM_MMIO_BYTES,
            param0: if nvram.is_protected() { 2 } else { 0 },
            ..DeviceEntry::absent(DEVICE_CODE_NVRAM)
        };
    }
    if config.tpm.is_some() {
        devices[DEV_TPM] = DeviceEntry {
            flags: DEVICE_PRESENT,
            base: TPM_CRB_BASE,
            length: TPM_CRB_LEN,
            ..DeviceEntry::absent(DEVICE_CODE_TPM)
        };
    }
    if let Some(base_unix_seconds) = config.rtc_base_unix_seconds {
        devices.push(DeviceEntry {
            code: DEVICE_CODE_RTC_CMOS,
            flags: DEVICE_PRESENT | DEVICE_PIO,
            base: 0x70,
            length: 2,
            gsi: NO_GSI,
            irq_flags: 0,
            param0: base_unix_seconds,
            param1: 0,
        });
    }
    if let Some(nvme) = devices.get_mut(DEV_NVME_TABLE)
        && config.nvme_pci
        && nvme.code == DEVICE_CODE_NVME
    {
        nvme.base = NVME_PCI_BAR_BASE;
        nvme.length = NVME_PCI_BAR_BYTES;
        nvme.gsi = crate::apic_pair::NVME_PIN as u32;
        nvme.irq_flags = 0b11;
        nvme.param0 = u64::from(NVME_PCI_SLOT);
    }
    devices
}

/// `version_minor` of the handoff block, per FIRMWARE_BOOT.md 6.4.
fn handoff_minor(config: &FirmwareBoardConfig, devices: &[DeviceEntry]) -> u16 {
    if config.nvram.as_ref().is_some_and(NvramConfig::is_protected) {
        HANDOFF_MINOR_PROTECTED_NVRAM
    } else if config.tpm.is_some() {
        HANDOFF_MINOR_TPM
    } else if config.fv_app == Some(FirmwareFvApp::Fs) {
        HANDOFF_MINOR_FV_APP_FS
    } else if config.fv_app == Some(FirmwareFvApp::Blk) {
        HANDOFF_MINOR_FV_APP_BLK
    } else if config.fv_app == Some(FirmwareFvApp::Gop) {
        HANDOFF_MINOR_FV_APP_GOP
    } else if config.fv_app == Some(FirmwareFvApp::Acpi) {
        HANDOFF_MINOR_FV_APP_ACPI
    } else if config.fv_app == Some(FirmwareFvApp::Rt) {
        HANDOFF_MINOR_FV_APP_RT
    } else if config.nvram.is_some() {
        HANDOFF_MINOR_NVRAM
    } else if config.fv_app == Some(FirmwareFvApp::Vars) {
        HANDOFF_MINOR_FV_APP_VARS
    } else if config.fv_app.is_some() {
        HANDOFF_MINOR_FV_APP
    } else if devices.iter().any(|d| d.code == DEVICE_CODE_RTC_CMOS) {
        HANDOFF_MINOR_RTC
    } else if devices
        .iter()
        .any(|d| d.code == DEVICE_CODE_NVME && d.gsi != u32::MAX)
    {
        HANDOFF_MINOR_NVME_PCI
    } else if devices.iter().any(|d| d.code == DEVICE_CODE_ACPI_PM) {
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
    // The xHCI BAR0 has no device-table entry (the firmware enumerates PCI): its window is a
    // memory map region only, and so a region of the handoff table.
    if config.xhci.is_some() {
        regions.push(MemoryRegionSpec::new(
            range(XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES)?,
            MemoryRegionKind::Mmio,
        ));
    }
    // The same for the NIC BAR0.
    if config.nic.is_some() {
        regions.push(MemoryRegionSpec::new(
            range(NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES)?,
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
            MemoryRegionKind::Mmio => {
                let kind = if base == NVRAM_MMIO_BASE {
                    REGION_NVRAM
                } else {
                    REGION_MMIO
                };
                table.push(simple(kind, 0, owner_of(base)));
            }
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
    minor: u16,
) -> Vec<u8> {
    let header_size = if minor >= HANDOFF_MINOR_FV_APP_ACPI {
        ACPI_HANDOFF_HEADER_BYTES
    } else if minor >= HANDOFF_MINOR_FV_APP {
        FV_APP_HANDOFF_HEADER_BYTES
    } else {
        HANDOFF_HEADER_BYTES
    };
    let region_table_offset = header_size;
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
    put16(&mut block, 0x0A, minor);
    put32(&mut block, 0x0C, header_size as u32);
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
    if let Some(app) = config.fv_app {
        put32(&mut block, HANDOFF_HEADER_BYTES, app as u32);
    }
    if minor >= HANDOFF_MINOR_FV_APP_ACPI {
        put32(
            &mut block,
            0xA8,
            (1_000 / FIRMWARE_EXECUTION_CYCLE_NS) as u32,
        );
    }
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

/// The clock owner's error as the board's (S4-23j).
fn owner_error(error: ClockOwnerError) -> FirmwareBoardError {
    match error {
        ClockOwnerError::Clock(error) => FirmwareBoardError::Clock(error),
        ClockOwnerError::Dispatch { .. } => {
            FirmwareBoardError::Config("clock owner: a Local APIC timer dispatch failed")
        }
    }
}

const UNROUTED_LAPIC_WAKE: FirmwareBoardError = FirmwareBoardError::Config(
    "clock owner: a Local APIC vector would wake the CPU, but the loop delivers the board source only",
);

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
    /// `#PF` and `#GP(0)` exceptions delivered to the guest through its IDT in this call; a
    /// delivery is not a retired instruction.
    pub(crate) exceptions_delivered: u64,
    /// Successfully delivered guest `#PF` exceptions in this call.
    pub(crate) page_faults_delivered: u64,
    /// Successfully delivered guest `#GP(0)` exceptions in this call.
    pub(crate) general_protection_faults_delivered: u64,
    /// Present only for a one-shot opt-in execution-observation run.
    pub(crate) execution_observations: Option<LinuxExecutionObservations>,
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

/// Whether the CPL0 set loop calls the set again after a quantum shortened by a deadline or
/// the scheduler: the quantum ran its shortened budget out
/// (`InstructionBudgetExhausted`) after retiring something, the run was not stopped by the guest
/// and the run budget is not spent.
fn resumes_budget_cut(
    exit: &PagedCpl0SetExit,
    shortened: bool,
    stop_requested: bool,
    remaining: u64,
    instructions_executed: u64,
) -> bool {
    shortened
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
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SetExitMappingError {
    Integration(&'static str),
    ExternalInterrupt {
        vector: u8,
        failure: IrqDeliveryFailure,
    },
}

impl SetExitMappingError {
    pub(crate) fn into_board_error(self, state: &LongModeState) -> FirmwareBoardError {
        match self {
            Self::Integration(detail) => FirmwareBoardError::Config(detail),
            Self::ExternalInterrupt { vector, failure } => {
                FirmwareBoardError::ExternalInterruptDelivery {
                    vector,
                    failure: format!("{failure:?}"),
                    rip: state.rip,
                    cs_selector: state.cs_selector,
                }
            }
        }
    }
}

pub(crate) fn set_exit_to_probe_exit(
    exit: PagedCpl0SetExit,
) -> Result<(PagedProbeExit, String), SetExitMappingError> {
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
                return Err(SetExitMappingError::Integration(
                    "the CPL0 set returned an exit the board never asks for",
                ));
            }
            // S6-20: the CPL0 set refuses a non-Running activity at its own entry; the board runs
            // vCPU 0 only, which is Running.
            RunExit::NotRunning => {
                return Err(SetExitMappingError::Integration(
                    "the CPL0 set returned NotRunning, an activity the board never runs",
                ));
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
            return Err(SetExitMappingError::Integration(
                "the CPL0 set returned an exit variant the board does not know",
            ));
        }
        // S4-38L8b: an interrupt of the board source that could not be delivered is an error of
        // the run, never a quiet stop; the vector and the reason are in the `Debug` text of the
        // set exit.
        PagedCpl0SetExit::IrqDeliveryFailed { vector, failure } => {
            return Err(SetExitMappingError::ExternalInterrupt { vector, failure });
        }
    };
    Ok((mapped, detail))
}

/// Where a run stands at a quantum boundary ([`LoadedFirmwareBoard::run_boot_with_quantum_hook`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuantumProgress {
    /// Instructions retired since the run began.
    pub retired: u64,
    /// Quanta completed since the run began.
    pub quanta: u64,
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
    /// Successful guest RDTSC reads in this call; `None` without the Linux-profile source.
    pub rdtsc_reads: Option<u64>,
    /// Successful guest RDTSC reads by CPL ring 0..3; `None` without the Linux-profile source.
    pub rdtsc_reads_by_cpl: Option<[u64; 4]>,
    /// Successful guest reads of PIT port B (`0x61`) during this call; `None` without a PIT.
    pub pit_port_b_reads: Option<u64>,
    /// Successful guest reads of PIT channel 2 (`0x42`) during this call; `None` without a PIT.
    pub pit_counter_port_reads: Option<u64>,
    /// Set when the guest asked the PM block to stop the machine (S5 or reset,
    /// S7-14a); `exit` is then `Stopped`. Always `None` without `acpi_pm`.
    pub stop_reason: Option<MachineStopReason>,
}

/// Who advances the clock of the CPL0-set loop (S4-23j).
#[derive(Debug)]
enum BoardClock {
    /// The board advances its clock itself (`VirtualClockHandle::advance_standalone`).
    Standalone,
    /// The single [`ClockOwner`] holds the clock's advancer for good; every move of time of the
    /// loop goes through it, and a standalone advance fails with `ClockAlreadyOwned`.
    Owned(ClockOwner),
}

#[derive(Default)]
pub(crate) struct Com1Log {
    bytes: Vec<u8>,
    observer: Option<Box<dyn FnMut(u8) + Send>>,
}

impl Com1Log {
    fn push(&mut self, byte: u8) {
        self.bytes.push(byte);
        if let Some(observer) = &mut self.observer {
            observer(byte);
        }
    }
}

/// What a platform reset does to RAM (S7-26, FIRMWARE_BOOT.md section 12.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformResetKind {
    /// Power-on state: every RAM byte is zero again (the reset register of the board performs
    /// this one: it is a hard reset, and so is every `ResetSystem` type of the firmware).
    Cold,
    /// RAM keeps its contents except what the entry contract rewrites: the firmware shadow, the
    /// handoff block and the boot page tables. Requested through the host API only.
    Warm,
}

/// The bytes and the CPU state a platform reset restores besides the ROM, which is its own
/// source of truth (S7-26): the pristine boot page tables, the root the paged interpreter walks
/// from (the firmware's own `MOV CR3` moves `page_table_root` to its tables while it runs) and
/// the direct-entry state.
struct ResetSource {
    tables: Vec<u8>,
    root: u64,
    entry: LongModeState,
}

/// A loaded firmware board: machine, direct-entry CPU state and handoff block.
pub struct LoadedFirmwareBoard {
    bus: MachineBus,
    /// The set of virtual processors (S6-10): exactly one element, index 0; every path takes it.
    vcpus: Vec<Vcpu>,
    /// The state the loader gave the boot processor (the direct entry of the firmware image),
    /// kept for [`Self::restart_vcpus`] (S4-21e). `None` for a Linux-profile board: its caller
    /// sets the entry state itself and the board does not know it.
    entry_state: Option<LongModeState>,
    uart: Uart16550Handle,
    /// The COM2 agent channel when `FirmwareBoardConfig::agent_channel` is set.
    agent_channel: Option<AgentChannel>,
    display: DisplayHandle,
    display_mmio: Option<DisplayDevice>,
    nvme_mmio: Option<NvmeDevice>,
    nvme: Option<NvmeHandle>,
    xhci_mmio: Option<XhciDevice>,
    nic_mmio: Option<NicDevice>,
    ecam_mmio: Option<crate::pci_ecam::SharedPciEcam>,
    xhci: Option<XhciHandle>,
    nic: Option<NicHandle>,
    hid: Option<HidDevices>,
    nvram: Option<NvramHandle>,
    tpm: Option<TpmCrbHandle>,
    acpi_pm: Option<AcpiPmHandle>,
    /// The PIT observer of the Linux profile (S4-37a3); `None` without the flag.
    pit: Option<Pit8254Handle>,
    /// Owner/host handle of the LAPIC (the S4-38 pump becomes its owner); it has
    /// a timer on the board clock only under the Linux profile (S4-37a4).
    lapic: LocalApicHandle,
    /// Shared with the PM timer; this board advances it itself (`Standalone`) unless
    /// [`Self::enable_clock_owner`] hands the advancer to a [`ClockOwner`].
    clock: VirtualClockHandle,
    handoff: Vec<u8>,
    page_table_bytes: u64,
    /// `CR3` the paged interpreter walks from (S4-37L4): the board's page-table
    /// root, `FIRMWARE_BOOT_PAGE_TABLES_GPA` for the firmware boot.
    page_table_root: u64,
    /// What [`Self::platform_reset`] puts back at power-on (S7-26); `None` on a Linux board,
    /// which has no firmware entry and cannot be reset this way.
    reset_source: Option<ResetSource>,
    /// The CONFIG_ADDRESS value last recorded by a LinuxDirect run.
    pci_config_address: u32,
    /// The ordinary CPL0 set changed its address cell since the last transfer to the shared bus.
    pci_config_pending: bool,
    /// COM1 bytes recorded by the latest `run_boot`, kept even when it fails.
    com1_log: Com1Log,
    /// Disabled by default. Diagnostic snapshots are sampled only at completed COM1 lines.
    console_context_parser: Option<ConsoleLineTrigger>,
    console_context_snapshots: Vec<ConsoleOutputContextSnapshot>,
    /// Disabled by default. Captures a bounded window after the first nonce marker.
    uart_output_context: Option<UartOutputContextCapture>,
    /// Pending-tick flag of the guest `RDTSC` (S4-37a6b); `Some` only under
    /// `linux_profile`, where `run` hands the CPU a time source. The source sets
    /// it, the `run_boot` stop predicate reads it and `run_boot` clears it.
    rdtsc_tick: Option<Arc<AtomicBool>>,
    /// Host stop request (S4-41c-1): when set by another thread or a signal handler, the CPL0-set
    /// loop ends the quantum at the next instruction boundary and the next quantum starts with an
    /// empty budget (a budget exit). `None` (default) keeps the loop exactly as it was.
    host_stop: Option<Arc<AtomicBool>>,
    /// The latest run was ended by `host_stop` (cleared at the start of each run).
    host_stop_honoured: bool,
    /// The source of maskable external interrupts (S4-38L8b). [`NoIrqSource`] (never a vector,
    /// never a deadline) until [`Self::enable_pit_pic_irq`] (S4-39g, user-mode run) installs the
    /// PIT -> 8259 source; consulted only by the CPL0-set loop and only when the run asks for the hook (`irq` member).
    irq_source: Box<dyn BoardIrqSource>,
    /// Handle of the 8259 master chip (S4-39g); `Some` under `linux_profile` (the PIC is bound
    /// there).
    pic_master: Option<Pic8259Handle>,
    /// The PIT -> 8259 IR0 source once [`Self::enable_pit_pic_irq`] installed it (S4-39g).
    pit_pic_source: Option<PitPicIrqSource>,
    /// The UART THRE -> 8259 IR4 source once [`Self::enable_uart_irq4`] installed it (S4-39i).
    uart_irq_source: Option<UartPicIrqSource>,
    /// The UART interrupt output -> IOAPIC INTIN4 source (wrapping the PIT -> INTIN2 source) once
    /// [`Self::enable_uart_ioapic_irq4`] installed it; exclusive with `uart_irq_source` (it needs the
    /// IOAPIC PIT source, which excludes the 8259 one).
    uart_ioapic_source: Option<UartIoapicIrqSource>,
    /// The PIT OUT0 -> IOAPIC INTIN2 -> Local APIC source once [`Self::enable_pit_ioapic_irq`]
    /// installed it (S4-38L8d); exclusive with `pit_pic_source`.
    pit_ioapic_source: Option<PitIoapicIrqSource>,
    /// Who moves the board clock inside the CPL0-set loop (S4-23j): the board itself (default,
    /// byte-for-byte the S4-37/38 behaviour) or a [`ClockOwner`] that [`Self::enable_clock_owner`]
    /// installed. Never `Owned` unless that call succeeded.
    clock_driver: BoardClock,
    linux_interleaved_policy: Option<QuantumPolicy>,
    /// The opt-in checkpoint recorder of the CPL0-set loop (S4-38L4i, observation only). `None`
    /// until [`Self::enable_checkpoints`]; then the loop shows it every instruction boundary.
    checkpoints: Option<CheckpointRecorder>,
    /// S4-13b: the opt-in report of the fetch cache and the page walks of the CPL0-set loop
    /// (measurement only). `fetch_cache_report_enabled` is set by
    /// [`Self::enable_fetch_cache_report`]; the loop leaves the report of its last run in
    /// `fetch_cache_report`.
    fetch_cache_report_enabled: bool,
    fetch_cache_report: Option<LinuxFetchCacheReport>,
    /// S4-13c: the shadow caches requested for the next runs (diagnostics only) and the result the
    /// last run left.
    fetch_cache_sweep_configs: Option<Vec<SweepConfig>>,
    /// S4-13d: what one entry of the CPL0-set loop's fetch cache covers; 4 KiB unless asked.
    fetch_cache_tag: FetchCacheTag,
    /// PERF-10: the decoded-instruction cache of the unified and Linux set loops. It lives between
    /// quanta, is bound to this board's memory and is emptied by `platform_reset` and by a snapshot
    /// restore. Diagnostic runs (a sweep, an execution observation, the fetch cache report) do not
    /// use it.
    decoded_instruction_cache: DecodedInstructionCache,
    /// PERF-11 (ADR-0016, prototype): the unified and Linux set loops of this board serve same-page
    /// qword reads through a data translation cache of one quantum. Off unless
    /// [`Self::enable_data_cache_prototype`]; diagnostic runs never use it.
    data_cache_prototype: bool,
    fetch_cache_sweep_report: Option<FetchSweepReport>,
    /// True when the assembly was given a PIT or LAPIC-timer clock other than the board clock
    /// (a test injection of S4-37a1/a4): their deadlines are in another time domain, so the
    /// clock owner is refused.
    foreign_timer_clock: bool,
    /// Test injection (S4-37a2): `run_boot` drops the queued PM timer ticks
    /// instead of applying them. Never set in production code.
    #[cfg(test)]
    suppress_pm_tick: bool,
    /// Test injection (S4-37a6b): `run_boot` drops the pending RDTSC tick.
    #[cfg(test)]
    suppress_rdtsc_tick: bool,
    /// PERF-25: the CPL0-set loop of the Linux runs (`run_linux_boot_cpl0_set*`, the unified loop)
    /// calls the set for several instructions at a time in an ordinary run (on by default; the
    /// loop itself keeps one instruction per call under its diagnostics). Off only in tests that
    /// compare the two.
    instruction_batches: bool,
}

/// The NMI half of a vCPU boundary (see [`LoadedFirmwareBoard::deliver_pending_nmi`]) on what it
/// needs: the vCPU's Local APIC, the vCPU itself, the RAM and the page-table root to use when the
/// vCPU has no CR3 of its own (the board's root, or, for a host thread run of S6-42c, the root the
/// caller named for that vCPU; `None`: there is none, and an NMI that needs one is an error).
/// The caller supplies the EFER used by its instruction walk.
/// A vCPU thread of the parallel mode (S6-34) calls it with the shared reference to the RAM.
pub(crate) fn deliver_pending_nmi_of(
    apic: &LocalApicHandle,
    vcpu: &mut Vcpu,
    memory: &crate::PhysicalMemory,
    page_table_root: Option<u64>,
    efer: u64,
    gate: Option<&dyn crate::bus::SplitLockGate>,
) -> Result<NmiBoundary, FirmwareBoardError> {
    use od_isa::Cpl0ExternalInterruptDelivery as Delivery;
    use od_isa::Cpl0ExternalInterruptRefusal as Refusal;
    if !apic.pending_nmi() {
        return Ok(NmiBoundary::NoPending);
    }
    // Holds first, only for what the architecture itself holds back: NMI blocking, and an
    // activity that takes no NMI (`Shutdown`, `WaitForSipi`; an application processor that
    // waits for SIPI has no paging to deliver through and must not be an error). A state the
    // board does not model (activity `None`) is not a hold but an error, before any change.
    if vcpu.state.nmi_blocked {
        return Ok(NmiBoundary::Held);
    }
    match vcpu.state.activity {
        Some(CpuActivity::Running | CpuActivity::Halted) => {}
        Some(CpuActivity::Shutdown | CpuActivity::WaitForSipi) => {
            return Ok(NmiBoundary::Held);
        }
        None => {
            return Err(FirmwareBoardError::Config(
                "deliver_pending_nmi: vCPU activity not modelled",
            ));
        }
    }
    // A started application processor (or one reset by INIT) holds its state in `legacy`
    // until it enters long mode; the long-mode `state` is not rewritten from it, so delivering
    // through the long-mode IDT would be wrong. NMI delivery in real and protected mode is not
    // modelled (item 6(d) of S6-16: an error).
    if vcpu.legacy.is_some() {
        return Err(FirmwareBoardError::Config(
            "deliver_pending_nmi: NMI in real mode not modelled",
        ));
    }
    match vcpu.state.sti_shadow {
        Some(od_isa::StiShadow::Clear) => {}
        Some(od_isa::StiShadow::Blocking) => return Ok(NmiBoundary::Held),
        None => {
            return Err(FirmwareBoardError::Config(
                "deliver_pending_nmi: STI shadow not modelled",
            ));
        }
    }
    // The walk is built on the vCPU's own CR3 (as the S6-16d quantum does); a vCPU without a
    // modelled CR3 uses the root the caller gave (the board's `page_table_root`, or the host
    // run's root of that vCPU). With neither there is nothing to walk: an explicit refusal, not a
    // read at some address the board happens to report (S6-42c).
    let paging_root = vcpu
        .state
        .cr3
        .or(page_table_root)
        .ok_or(FirmwareBoardError::Config(
            "deliver_pending_nmi: the vCPU has no page-table root (no CR3 and no root named for the NMI)",
        ))?;
    let paging = Long64ProbeConfig::from_cpu_state(&vcpu.state, paging_root, efer)
        .map_err(|_| FirmwareBoardError::Config("deliver_pending_nmi: no paging for the vCPU"))?;
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: if vcpu.state.cs_selector.is_some_and(|cs| cs & 3 == 3) {
            PrivilegeLevel::Ring3
        } else {
            PrivilegeLevel::Ring0
        },
        origin: AccessOrigin::Explicit,
    };
    let failure = |vcpu: &Vcpu, failure: String| FirmwareBoardError::ExternalInterruptDelivery {
        vector: od_isa::NMI_VECTOR,
        failure,
        rip: vcpu.state.rip,
        cs_selector: vcpu.state.cs_selector,
    };
    match deliver_nmi_at_boundary(&mut vcpu.state, memory, &paging, context, gate) {
        Delivery::Delivered { .. } => {
            apic.take_nmi();
            Ok(NmiBoundary::Delivered)
        }
        // The holds above already decided these; kept as holds, never as drops.
        Delivery::Refused(Refusal::NmiBlocked | Refusal::StiShadowActive) => Ok(NmiBoundary::Held),
        other => Err(failure(vcpu, format!("{other:?}"))),
    }
}

/// What the vCPU threads of the parallel mode need of a board (S6-34), split off its fields by
/// [`LoadedFirmwareBoard::thread_parts`]: the bus by shared reference (RAM and the one device lock),
/// every vCPU by exclusive reference (each thread owns one), the Local APICs, the devices the
/// quantum polls, the interrupt source of the board (the boot processor's), the clock owner and the
/// COM1 log.
pub(crate) struct ThreadParts<'a> {
    pub(crate) bus: &'a MachineBus,
    pub(crate) vcpus: &'a mut [Vcpu],
    pub(crate) lapic: LocalApicHandle,
    pub(crate) uart: Uart16550Handle,
    pub(crate) acpi_pm: Option<AcpiPmHandle>,
    pub(crate) pit: Option<Pit8254Handle>,
    pub(crate) pit_ioapic_source: Option<PitIoapicIrqSource>,
    pub(crate) nvme: Option<NvmeHandle>,
    pub(crate) uart_irq_source: Option<&'a UartPicIrqSource>,
    pub(crate) uart_ioapic_source: Option<&'a UartIoapicIrqSource>,
    pub(crate) irq_source: &'a mut dyn BoardIrqSource,
    pub(crate) owner: &'a mut ClockOwner,
    pub(crate) clock: &'a VirtualClockHandle,
    pub(crate) rdtsc_clock: Option<&'a VirtualClockHandle>,
    pub(crate) com1: &'a mut Com1Log,
    /// The board's page-table root (the walk root of a vCPU that has no CR3 of its own).
    pub(crate) page_table_root: u64,
}

impl LoadedFirmwareBoard {
    /// Reset host lock measurements just before a diagnostic run on a quiescent board.
    #[cfg(feature = "lock-wait-metrics")]
    pub(crate) fn reset_lock_wait_metrics(&mut self) {
        self.bus.reset_device_lock_metrics();
        self.lapic.0.reset_metrics();
    }

    /// Read a coherent diagnostic sample after all vCPUs have joined.
    #[cfg(feature = "lock-wait-metrics")]
    pub(crate) fn lock_wait_metrics(&self) -> crate::BoardLockWaitMetrics {
        crate::BoardLockWaitMetrics {
            device_table: self.bus.device_lock_metrics(),
            apic_pair: self.lapic.0.metrics(),
        }
    }

    /// S6-32: the one execution-mode switch of the machine (see [`crate::ExecutionMode`]); it is
    /// stored in the board's physical memory and the interpreter does not know it. The default is
    /// [`crate::ExecutionMode::Interleaved`], which is what every run uses today; the parallel
    /// mode starts vCPU threads only in S6-34, so selecting it now changes the host memory order
    /// of ordinary RAM accesses and the host fence of the guest fences, never the result of a
    /// single-threaded run.
    pub fn set_execution_mode(&mut self, mode: crate::ExecutionMode) {
        self.bus.set_execution_mode(mode);
    }

    /// S6-32: the execution mode of the board.
    pub fn execution_mode(&self) -> crate::ExecutionMode {
        self.bus.execution_mode()
    }

    /// S6-34: split the board for the vCPU threads of the parallel mode. Refused (`Config`) when
    /// the board is not in [`crate::ExecutionMode::Parallel`], has no clock owner, or has one of the
    /// diagnostic hooks that read the COM1 stream inside the pump (`console_context_parser`,
    /// `uart_output_context`: they sample per byte on one thread), or the agent channel (COM2,
    /// which the parallel loops do not pump). Everything but the vCPUs, the
    /// clock owner, the interrupt source and the log is shared.
    pub(crate) fn thread_parts(&mut self) -> Result<ThreadParts<'_>, FirmwareBoardError> {
        if self.bus.execution_mode() != crate::ExecutionMode::Parallel {
            return Err(FirmwareBoardError::Config(
                "thread_parts: the board is not in the parallel execution mode",
            ));
        }
        if self.console_context_parser.is_some() || self.uart_output_context.is_some() {
            return Err(FirmwareBoardError::Config(
                "thread_parts: the console context hooks are single-thread diagnostics",
            ));
        }
        // The vCPU threads of `host_parallel` neither pump the agent channel nor end a quantum on
        // its THR (S11-21): a board with the channel is refused rather than run with a dead COM2.
        if self.agent_channel.is_some() {
            return Err(FirmwareBoardError::Config(
                "thread_parts: the agent channel (COM2) is not pumped by the parallel loops",
            ));
        }
        let owner = match &mut self.clock_driver {
            BoardClock::Owned(owner) => owner,
            BoardClock::Standalone => {
                return Err(FirmwareBoardError::Config(
                    "thread_parts: the board has no clock owner (enable_clock_owner)",
                ));
            }
        };
        Ok(ThreadParts {
            bus: &self.bus,
            vcpus: &mut self.vcpus,
            lapic: self.lapic.clone(),
            uart: self.uart.clone(),
            acpi_pm: self.acpi_pm.clone(),
            pit: self.pit.clone(),
            pit_ioapic_source: self.pit_ioapic_source.clone(),
            nvme: self.nvme.clone(),
            uart_irq_source: self.uart_irq_source.as_ref(),
            uart_ioapic_source: self.uart_ioapic_source.as_ref(),
            irq_source: &mut *self.irq_source,
            owner,
            clock: &self.clock,
            rdtsc_clock: self.rdtsc_tick.as_ref().map(|_| &self.clock),
            com1: &mut self.com1_log,
            page_table_root: self.page_table_root,
        })
    }

    pub fn state(&self) -> &LongModeState {
        &self.vcpus[BOOT_VCPU_INDEX].state
    }

    pub fn state_mut(&mut self) -> &mut LongModeState {
        &mut self.vcpus[BOOT_VCPU_INDEX].state
    }

    /// Registers of vCPU `vcpu` for a stop snapshot (S8-10), `None` when the board has no such
    /// vCPU. Read-only: a copy of what the state holds. The boot processor's CR3 is the root the
    /// board walks from; another vCPU's CR3 is the one its state carries.
    pub fn diagnostic_vcpu_state(&self, vcpu: usize) -> Option<VcpuDiagnosticState> {
        let cpu = self.vcpus.get(vcpu)?;
        let state = &cpu.state;
        let cr3 = state
            .cr3
            .or((vcpu == BOOT_VCPU_INDEX).then_some(self.page_table_root));
        Some(VcpuDiagnosticState {
            vcpu,
            rip: state.rip,
            rflags: state.rflags,
            gpr: state.gpr,
            cr0: state.cr0,
            cr2: cpu.guest_cr2.or(state.cr2),
            cr3,
            cr4: state.cr4,
            efer: cpu.guest_efer,
            cs_selector: state.cs_selector,
            cpl: state.cs_selector.map(|selector| (selector & 3) as u8),
            long_mode_model: cpu.legacy.is_none(),
            activity: state.activity,
        })
    }

    /// Reads `output.len()` bytes of the guest-virtual space of vCPU `vcpu` (S8-10). The 4-level
    /// tables (4 KiB, 2 MiB and 1 GiB pages) are walked from the vCPU's CR3 without setting
    /// accessed or dirty bits, and the bytes come from RAM, ROM or framebuffer only: no device is
    /// touched and no guest byte is written. Permissions are not checked (a diagnostic read).
    /// On failure `read` of the error says how many leading bytes of `output` are valid.
    pub fn diagnostic_read_virtual(
        &self,
        vcpu: usize,
        linear: u64,
        output: &mut [u8],
    ) -> Result<(), DiagnosticReadFailure> {
        let failure = |error| DiagnosticReadFailure {
            read: 0,
            linear,
            error: DiagnosticReadError::Translate(error),
        };
        let state = self
            .diagnostic_vcpu_state(vcpu)
            .ok_or_else(|| failure(DiagnosticTranslateError::NoSuchVcpu))?;
        let cr3 = stop_diagnostic::require_long_mode_paging(
            state.cr0,
            state.cr3,
            state.cr4,
            state.long_mode_model,
        )
        .map_err(failure)?;
        stop_diagnostic::read_virtual(
            |address| {
                stop_diagnostic::translate_long64(
                    |physical| {
                        let mut entry = [0u8; 8];
                        self.bus
                            .read_physical_bytes_readonly(
                                GuestPhysicalAddress::new(physical),
                                &mut entry,
                            )
                            .map(|()| u64::from_le_bytes(entry))
                    },
                    cr3,
                    address,
                )
            },
            |physical, bytes| {
                self.bus
                    .read_physical_bytes_readonly(GuestPhysicalAddress::new(physical), bytes)
            },
            linear,
            output,
        )
    }

    /// Owner/host handle of the board LAPIC (S4-38 pump becomes its owner); it can
    /// write (reset, `set_global_enabled`, `request_fixed_edge`, `acknowledge`,
    /// `dispatch_timer`). `has_timer()` is true only under `linux_profile`.
    pub fn local_apic(&self) -> LocalApicHandle {
        self.lapic.clone()
    }

    /// Number of vCPUs of the board (`FirmwareBoardConfig::vcpu_count`).
    pub fn vcpu_count(&self) -> usize {
        self.vcpus.len()
    }

    /// Handle of the LAPIC of vCPU `index` (its APIC ID), `None` when the board has no such vCPU.
    pub fn local_apic_of(&self, index: usize) -> Option<LocalApicHandle> {
        self.lapic.for_cpu(index)
    }

    /// The state of vCPU `index`, `None` when the board has no such vCPU.
    pub(crate) fn vcpu(&self, index: usize) -> Option<&Vcpu> {
        self.vcpus.get(index)
    }

    /// Mutable state of vCPU `index`, `None` when the board has no such vCPU.
    pub(crate) fn vcpu_mut(&mut self, index: usize) -> Option<&mut Vcpu> {
        self.vcpus.get_mut(index)
    }

    /// The CPUID profile of the legacy models of the board's vCPUs and of the EDX after INIT
    /// (S6-18a), read from `vcpus[0].cpuid.profile`: it is set on every vCPU (`LINUX_AP_CPUID_PROFILE`
    /// on the `linux-direct` board). It is NOT the CPUID of the BSP in long mode: that one is the
    /// `members.cpuid_profile` of `linux_direct.rs`. `DiagnosticV1` until
    /// [`Self::set_cpuid_profile`] changes it.
    pub fn cpuid_profile(&self) -> CpuidProfile {
        self.vcpus[BOOT_VCPU_INDEX].cpuid.profile
    }

    /// Choose the CPUID profile of every vCPU (S6-18a). Each keeps its APIC ID (its index) and
    /// the board's vCPU count. A legacy model that is still in its INIT or power-on state takes
    /// the profile now (its `CPUID` source and EDX); one that has been started keeps what it has,
    /// so the call is made before the guest runs. The Linux boards pick
    /// `OdX8664V1Neutral` (`vcpu::LINUX_AP_CPUID_PROFILE`) for their application processors: the `verify_cpu`
    /// of the kernel's trampoline returns 0 only on it.
    pub fn set_cpuid_profile(&mut self, profile: CpuidProfile) {
        let count = self.vcpus.len();
        for vcpu in &mut self.vcpus {
            let cpuid = VcpuCpuid::new(profile, vcpu.index, count);
            vcpu.set_cpuid(cpuid);
        }
    }

    /// The single clock owner, once [`Self::enable_clock_owner`] installed it. The scheduler of
    /// S6-15 moves time through it; the board does not create a second one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn clock_owner_mut(&mut self) -> Option<&mut ClockOwner> {
        match &mut self.clock_driver {
            BoardClock::Owned(owner) => Some(owner),
            BoardClock::Standalone => None,
        }
    }

    /// The reset of the CPU complex of the board (S6-24; what S6-13 left open): every vCPU goes
    /// back to its power-on state and every Local APIC and the IOAPIC route to theirs.
    ///
    /// - The boot processor gets `bsp_state` and `bsp_guest_efer` (the handoff state the loader
    ///   gave it: the board does not keep it), `Running`, no legacy model, the guest cells at their
    ///   initial values (SMP_CONTRACT.md 3, row "Power-on, index 0").
    /// - Every application processor is `WaitForSipi` with the real-mode reset state, whatever it
    ///   did before (Running, Halted, in long mode, shut down): [`Vcpu::new_ap`]. The CPUID
    ///   configuration of every vCPU is kept.
    /// - Every Local APIC, not only the boot processor's, drops its pending INIT, SIPI, NMI, its
    ///   IRR/ISR/TMR and its armed timer, and the route drops its Remote IRR
    ///   ([`LocalApicHandle::reset_all`]); an earlier INIT that the owner's decision left the
    ///   Remote IRR of cannot veto it.
    ///
    /// Not touched: guest RAM, the devices of the board (UART, PIT, PIC, PM timer, NVMe, display),
    /// the interrupt source and the clock: virtual time does not move. The devices and RAM are
    /// reset by [`Self::platform_reset`] (S7-26), which calls this first. On an error nothing has
    /// changed.
    pub(crate) fn reset_cpu_complex(
        &mut self,
        bsp_state: LongModeState,
        bsp_guest_efer: u64,
    ) -> Result<(), FirmwareBoardError> {
        self.lapic.reset_all().map_err(|_| {
            FirmwareBoardError::Config("reset_cpu_complex: the Local APIC reset failed")
        })?;
        for (index, vcpu) in self.vcpus.iter_mut().enumerate() {
            let cpuid = vcpu.cpuid;
            *vcpu = if index == BOOT_VCPU_INDEX {
                let mut boot = Vcpu::new(BOOT_VCPU_INDEX, bsp_state.clone());
                boot.guest_efer = bsp_guest_efer;
                boot
            } else {
                Vcpu::new_ap(index)
            };
            vcpu.set_cpuid(cpuid);
        }
        Ok(())
    }

    /// Restart the processors after a stop (S4-21e, the processor half of the machine reset of
    /// S7-26): the CPU complex goes back to the entry state the loader gave the boot processor
    /// ([`Self::reset_cpu_complex`]: every vCPU at its power-on state, the Local APICs and the IOAPIC
    /// route without pending events) and the guest stop that ended the run is acknowledged. The
    /// acknowledged stop is returned: `Some(Reset)` after the guest wrote the reset register,
    /// `Some(Shutdown)` after S5, `None` for any other end of a run (a halt, a budget, a host STOP,
    /// a guest shutdown or a host fault: a restart is legal after all of them; the host decides).
    ///
    /// A host STOP flag set with [`Self::set_host_stop`] belongs to the host and is not cleared
    /// (only the CPL0-set Linux runs look at it).
    /// Not touched: guest RAM (the entry state is only usable on RAM the caller has put back, or
    /// on a guest that sets up everything itself), ROM, the devices (including the PM block's
    /// registers), NVRAM, the TPM and virtual time. Their reset, and the RAM policy of cold and
    /// warm reset, belong to [`Self::platform_reset`] (S7-26). Without a recorded entry state (a Linux-profile board) it is a
    /// configuration error. On an error nothing has changed.
    pub fn restart_vcpus(&mut self) -> Result<Option<MachineStopReason>, FirmwareBoardError> {
        let entry = self.entry_state.clone().ok_or(FirmwareBoardError::Config(
            "restart_vcpus: the board has no recorded entry state",
        ))?;
        self.reset_cpu_complex(entry, FIRMWARE_EFER)?;
        Ok(self
            .acpi_pm
            .as_ref()
            .and_then(AcpiPmHandle::take_stop_request))
    }

    /// The platform reset of the firmware board (S7-26): what the reset register of the board
    /// (`MachineStopReason::Reset`) and a host request do, inside the same process and on the
    /// same board value, with no board rebuilt. The caller runs `run_boot` again afterwards.
    ///
    /// - CPU complex: [`Self::reset_cpu_complex`] to the direct-entry state of the loader, and the
    ///   walk root of the paged interpreter back to the boot page tables.
    /// - RAM: `Cold` zeroes every RAM region; `Warm` keeps it. Either way the ROM is read back
    ///   into the firmware shadow, and the handoff block and the pristine boot page tables are
    ///   written again, because the firmware overwrites all three while it runs.
    /// - Devices back to their power-on state: UART (COM1 is configured again as at load),
    ///   display (framebuffer bytes and `generation` are kept, DISPLAY_DEVICE.md 10), NVMe
    ///   (queues and registers; the attached backend and the disk stay), ACPI PM block (registers
    ///   and the latched stop request) and the TPM CRB (`_TPM_Init`, no locality granted).
    ///   The board has no HPET.
    /// - Not reset: the virtual clock, hence the PM timer and the RTC, which read it; the
    ///   committed banks of the NVRAM (only its registers and a write in flight are lost); the
    ///   persistent state of the TPM (its PCRs, sessions and loaded objects are the volatile
    ///   state and are lost). `TPM2_Startup(CLEAR)` is sent by the firmware that boots next,
    ///   as after the first power-on.
    ///
    /// A Linux board (no firmware entry) and a board with the Linux profile (PIT, 8259 pair) are
    /// refused with `Config` before anything changes; so is a ROM that cannot be read back.
    /// There is no other error path: a failure after the CPU complex was reset would leave a
    /// board that must not be run, and none is reachable for the constant layout of the loader.
    pub fn platform_reset(&mut self, kind: PlatformResetKind) -> Result<(), FirmwareBoardError> {
        if self.reset_source.is_none() || self.has_linux_profile() {
            return Err(FirmwareBoardError::Config(
                "platform_reset: only the firmware profile has an entry to reset to",
            ));
        }
        let rom = GuestPhysicalAddress::new(FIRMWARE_ROM_BASE);
        let mut shadow = vec![0u8; FIRMWARE_IMAGE_BYTES];
        self.bus
            .read_flat_bytes(rom, &mut shadow)
            .map_err(FirmwareBoardError::Bus)?;
        let source = self.reset_source.as_ref().expect("checked above");
        let entry = source.entry.clone();
        self.reset_cpu_complex(entry, FIRMWARE_EFER)?;
        self.decoded_instruction_cache.reset();
        if kind == PlatformResetKind::Cold {
            self.clear_ram()?;
        }
        let source = self.reset_source.as_ref().expect("checked above");
        self.page_table_root = source.root;
        let writes = [
            (FIRMWARE_LINK_BASE, &shadow[..]),
            (source.root, &source.tables[..]),
            (FIRMWARE_HANDOFF_GPA, &self.handoff[..]),
        ];
        for (address, bytes) in writes {
            self.bus
                .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
                .map_err(FirmwareBoardError::Bus)?;
        }
        self.uart.reset();
        if let Some(channel) = &self.agent_channel {
            channel.reset();
        }
        configure_com1(&mut self.bus)?;
        self.display.reset();
        if let Some(nvme) = &self.nvme {
            nvme.platform_reset();
        }
        if let Some(xhci) = &self.xhci {
            xhci.platform_reset();
        }
        if let Some(nic) = &self.nic {
            nic.platform_reset();
        }
        if let Some(pm) = &self.acpi_pm {
            pm.platform_reset();
        }
        if let Some(nvram) = &self.nvram {
            nvram.platform_reset();
        }
        if let Some(tpm) = &self.tpm {
            tpm.platform_reset(self.clock.snapshot().now_ns());
        }
        self.pci_config_address = 0;
        self.pci_config_pending = false;
        self.host_stop_honoured = false;
        Ok(())
    }

    /// PERF-10: empties the decoded-instruction cache (a snapshot restore calls this; a platform
    /// reset does it itself).
    pub(crate) fn reset_decoded_instruction_cache(&self) {
        self.decoded_instruction_cache.reset();
    }

    /// PERF-10: the decoded-instruction cache of this board (the tests of the cache read its counters).
    #[cfg(test)]
    pub(crate) fn decoded_instruction_cache(&self) -> &DecodedInstructionCache {
        &self.decoded_instruction_cache
    }

    /// Zero every RAM region of the memory map, 1 MiB at a time, through the committed-write
    /// path (so the code page generations of a decoded or compiled page change).
    fn clear_ram(&mut self) -> Result<(), FirmwareBoardError> {
        const CHUNK: usize = 1 << 20;
        let zeros = vec![0u8; CHUNK];
        let regions: Vec<_> = self
            .bus
            .physical_memory()
            .region_specs()
            .filter(|spec| spec.kind == MemoryRegionKind::Ram)
            .map(|spec| spec.range)
            .collect();
        for region in regions {
            let end = region.end_exclusive();
            let mut at = u128::from(region.start().get());
            while at < end {
                let len = (end - at).min(CHUNK as u128) as usize;
                let address = GuestPhysicalAddress::new(at as u64);
                self.bus
                    .write_flat_bytes(address, &zeros[..len], |_| {})
                    .map_err(FirmwareBoardError::Bus)?;
                at += len as u128;
            }
        }
        Ok(())
    }

    /// The vCPU `index` applies the INIT and SIPI waiting in its own Local APIC (S6-13), on its
    /// own boundary: the LAPIC hands them over and the vCPU changes its state
    /// ([`Vcpu::apply_startup`]). The scheduler of S6-15 calls it on the boundary of every vCPU
    /// before it picks one (`BoardSmpHost::boundary`); the run loop of the board does not. No such vCPU, or a LAPIC that
    /// cannot reset its timer for an INIT, is an error and nothing is applied.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_pending_startup(
        &mut self,
        index: usize,
    ) -> Result<StartupApplied, FirmwareBoardError> {
        const NO_VCPU: FirmwareBoardError =
            FirmwareBoardError::Config("apply_pending_startup: no such vCPU");
        let apic = self.lapic.for_cpu(index).ok_or(NO_VCPU)?;
        let vcpu = self.vcpus.get_mut(index).ok_or(NO_VCPU)?;
        let pending = apic
            .take_startup()
            .map_err(|_| FirmwareBoardError::Config("apply_pending_startup: INIT reset failed"))?;
        Ok(vcpu.apply_startup(pending))
    }

    /// The vCPU `index` delivers the NMI waiting in its own Local APIC through vector 2 on its own
    /// boundary (S6-12b), if it can: nothing waits (`NoPending`); the vCPU holds the NMI back, so
    /// the latch stays set (`Held`, only for architectural holds: NMI blocked until the running
    /// handler executes `IRETQ`, an STI shadow in force (`Some(Blocking)`), or an activity that
    /// takes no NMI: `Shutdown`, `WaitForSipi`); or the frame is on the guest stack and the vCPU
    /// is in the handler (`Delivered`: the latch is taken, `nmi_blocked` is set, a `Halted` vCPU
    /// is `Running`). The scheduler of S6-15 calls it on the boundary of every vCPU, after
    /// [`Self::apply_pending_startup`] and before it picks one (`BoardSmpHost::boundary`); the run
    /// loop of the board does not. A gate that cannot deliver (not present, a failed frame store
    /// or a failed read of the TSS `IST[n]` slot; IST ≠ 0 is supported only for NMI delivery,
    /// S6-12c) and a state the board cannot deliver through are errors, not holds: a state that is not
    /// modelled (activity or STI shadow `None`), a vCPU on a legacy-mode model (`legacy` is
    /// `Some`, the trampoline of S6-16c is still running: NMI in real or protected mode is not
    /// modelled; the error text says "real mode"), paging the walker rejects (the EFER is
    /// the vCPU's own `guest_efer`, the root is the vCPU's own CR3, S6-16f). On an error the vCPU is unchanged and the NMI stays pending,
    /// never silently dropped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn deliver_pending_nmi(
        &mut self,
        index: usize,
    ) -> Result<NmiBoundary, FirmwareBoardError> {
        const NO_VCPU: FirmwareBoardError =
            FirmwareBoardError::Config("deliver_pending_nmi: no such vCPU");
        let apic = self.lapic.for_cpu(index).ok_or(NO_VCPU)?;
        let vcpu = self.vcpus.get_mut(index).ok_or(NO_VCPU)?;
        let efer = vcpu.guest_efer;
        deliver_pending_nmi_of(
            &apic,
            vcpu,
            self.bus.physical_memory(),
            Some(self.page_table_root),
            efer,
            None,
        )
    }

    pub fn bus(&self) -> &MachineBus {
        &self.bus
    }

    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }

    /// Read the complete fixed ACPI address range in address order for diagnostics.
    pub fn read_acpi_reservations(&mut self) -> Result<Vec<u8>, FirmwareBoardError> {
        let mut bytes = vec![0; (FIRMWARE_ACPI_TABLES_BYTES + FIRMWARE_ACPI_FACS_BYTES) as usize];
        self.bus
            .read_flat_bytes(
                GuestPhysicalAddress::new(FIRMWARE_ACPI_TABLES_BASE),
                &mut bytes,
            )
            .map_err(FirmwareBoardError::Bus)?;
        Ok(bytes)
    }

    /// S4-13d diagnostics: SHA-256 over the bytes of every plain-RAM region in address order,
    /// read-only (the end state of a run, to tell two runs of the same guest apart or equal).
    /// It copies all of RAM into one buffer first (a peak of two to three times the RAM size);
    /// a streaming hash is a separate task. A region that cannot be read is an error naming it,
    /// never a hash over the rest.
    pub fn ram_sha256(&mut self) -> Result<[u8; 32], String> {
        let (memory, _) = self.bus.split_for_pio_step();
        let regions: Vec<_> = memory
            .region_specs()
            .filter(|spec| spec.kind == MemoryRegionKind::Ram)
            .collect();
        let mut data = Vec::new();
        for spec in regions {
            let mut bytes = vec![0u8; spec.range.len() as usize];
            memory
                .load_bytes(spec.range.start(), &mut bytes)
                .map_err(|error| {
                    format!(
                        "RAM region at {:#x} (len {:#x}) is unreadable: {error:?}",
                        spec.range.start().get(),
                        spec.range.len()
                    )
                })?;
            data.extend_from_slice(&bytes);
        }
        Ok(crate::acpi_blob::sha256(&data))
    }

    /// COM1 bytes recorded by the latest `run_boot`, including those of a run
    /// that returned an error (so the runner never loses transcript bytes).
    pub fn com1_transcript(&self) -> &[u8] {
        &self.com1_log.bytes
    }

    /// Observe each new COM1 byte after it is appended to the transcript.
    /// Calls follow transcript order, including when workers share COM1. The observer
    /// can run on a vCPU worker under the transcript lock and must not re-enter the
    /// machine or block. Installing another observer replaces the previous one.
    pub fn set_com1_observer(&mut self, observer: impl FnMut(u8) + Send + 'static) {
        self.com1_log.observer = Some(Box::new(observer));
    }
    /// Installs the host stop request (S4-41c-1). Setting the flag (from another thread or an
    /// async-signal-safe handler: one atomic store) ends the current quantum at the next
    /// instruction boundary; the next quantum starts with an empty budget, so the CPL0-set run
    /// ends with the budget exit, and [`Self::take_host_stop_honoured`] says that the stop was
    /// honoured. The quanta are not cut in advance: a run whose flag is never set is identical
    /// to a run without a request. Only `run_linux_boot_cpl0_set*` looks at the request, on one
    /// vCPU (also under the seeded one-vCPU scheduler); the multi-vCPU, threaded and parallel
    /// runners ignore it.
    pub fn set_host_stop(&mut self, flag: Arc<AtomicBool>) {
        self.host_stop = Some(flag);
    }
    /// Whether the latest run was ended by the host stop request; clears the record.
    pub fn take_host_stop_honoured(&mut self) -> bool {
        std::mem::take(&mut self.host_stop_honoured)
    }

    /// Enables bounded diagnostic sampling at selected completed console-output lines.
    /// Sampling reads CPU state and RAM only; the result describes console-output context,
    /// not the unwind state of a previously printed trace.
    pub fn enable_console_output_context_diagnostic(&mut self) {
        self.console_context_parser = Some(ConsoleLineTrigger::default());
        self.console_context_snapshots.clear();
        self.uart_output_context = Some(UartOutputContextCapture::default());
    }

    /// Takes the snapshots produced by the last run, if the diagnostic was enabled.
    pub fn take_console_output_context_diagnostic(&mut self) -> Vec<ConsoleOutputContextSnapshot> {
        std::mem::take(&mut self.console_context_snapshots)
    }

    /// Takes bounded byte-level COM1 provenance, if the diagnostic was enabled.
    pub fn take_uart_output_context_diagnostic(&mut self) -> Option<UartOutputContextDiagnostic> {
        self.uart_output_context
            .take()
            .map(UartOutputContextCapture::finish)
    }

    fn sample_console_output_context(&mut self, byte: u8) {
        if self.console_context_snapshots.len() >= MAX_CAPTURE_COUNT {
            self.console_context_parser = None;
            return;
        }
        let trigger = self
            .console_context_parser
            .as_mut()
            .and_then(|parser| parser.push(byte));
        if let Some(trigger) = trigger {
            self.capture_console_output_context(trigger);
            if self.console_context_snapshots.len() >= MAX_CAPTURE_COUNT {
                self.console_context_parser = None;
            }
        }
    }

    fn sample_uart_output_context(&mut self, byte: u8, offset: u64, retired: u64) {
        let should_capture = self
            .uart_output_context
            .as_mut()
            .is_some_and(|capture| capture.observe_byte(byte, offset));
        if !should_capture {
            return;
        }

        let rip = self.state().rip;
        let rbp = self.state().gpr[Register::Rbp as usize];
        let rsp = self.state().gpr[Register::Rsp as usize];
        let cr3 = self.page_table_root;
        let (frames, ended) = self.capture_console_frame_chain(rip, rbp, cr3);
        let snapshot = UartOutputByteContextSnapshot {
            offset,
            byte,
            retired,
            rip,
            rsp,
            rbp,
            cr3,
            cs_selector: self.state().cs_selector,
            frames,
            ended,
        };
        if let Some(capture) = self.uart_output_context.as_mut() {
            capture.push_snapshot(snapshot);
        }
    }

    fn capture_console_output_context(&mut self, trigger: ConsoleContextTrigger) {
        let rip = self.state().rip;
        let rbp = self.state().gpr[Register::Rbp as usize];
        let cr3 = self.page_table_root;
        let (frames, ended) = self.capture_console_frame_chain(rip, rbp, cr3);
        self.console_context_snapshots
            .push(ConsoleOutputContextSnapshot {
                trigger,
                rip,
                rbp,
                cr3,
                frames,
                ended,
            });
    }

    fn capture_console_frame_chain(
        &mut self,
        rip: u64,
        rbp: u64,
        cr3: u64,
    ) -> (Vec<crate::ConsoleContextFrame>, crate::FrameChainEnd) {
        const CR0_LONG_PAGING: u64 = (1 << 31) | (1 << 16) | (1 << 4) | 1;
        const CR4_PAE: u64 = 1 << 5;
        let paging = Long64ProbeConfig::new(
            self.state().cr0.unwrap_or(CR0_LONG_PAGING),
            cr3,
            self.state().cr4.unwrap_or(CR4_PAE),
            self.efer(),
        );
        let (frames, ended) = match paging {
            Ok(paging) => capture_frame_chain(rbp, |frame_pointer| {
                let mut bytes = [0; 16];
                self.bus
                    .read_virtual_ram_diagnostic(
                        &paging,
                        rip,
                        GuestLinearAddress::new(frame_pointer),
                        AccessContext {
                            mode: InstructionMode::Long64,
                            privilege: PrivilegeLevel::Ring0,
                            origin: AccessOrigin::Explicit,
                        },
                        &mut bytes,
                    )
                    .map(|()| bytes)
                    .map_err(|_| ())
            }),
            Err(_) => (Vec::new(), crate::FrameChainEnd::ReadFailed),
        };
        (frames, ended)
    }

    fn capture_delay_tsc_frame_chain(
        &mut self,
        rip: u64,
        rbp: u64,
        cr3: u64,
    ) -> (Vec<crate::ConsoleContextFrame>, crate::FrameChainEnd) {
        const CR0_LONG_PAGING: u64 = (1 << 31) | (1 << 16) | (1 << 4) | 1;
        const CR4_PAE: u64 = 1 << 5;
        let paging = Long64ProbeConfig::new(
            self.state().cr0.unwrap_or(CR0_LONG_PAGING),
            cr3,
            self.state().cr4.unwrap_or(CR4_PAE),
            self.efer(),
        );
        match paging {
            Ok(paging) => crate::console_output_context::capture_frame_chain_with_limit(
                rbp,
                4,
                |frame_pointer| {
                    let mut bytes = [0; 16];
                    self.bus
                        .read_virtual_ram_diagnostic(
                            &paging,
                            rip,
                            GuestLinearAddress::new(frame_pointer),
                            AccessContext {
                                mode: InstructionMode::Long64,
                                privilege: PrivilegeLevel::Ring0,
                                origin: AccessOrigin::Explicit,
                            },
                            &mut bytes,
                        )
                        .map(|()| bytes)
                        .map_err(|_| ())
                },
            ),
            Err(_) => (Vec::new(), crate::FrameChainEnd::ReadFailed),
        }
    }

    pub fn uart(&self) -> Uart16550Handle {
        self.uart.clone()
    }

    /// The host handle of the COM2 agent channel; `None` unless
    /// `FirmwareBoardConfig::agent_channel` was set. The Linux loop and the firmware loop pump it
    /// with COM1 and end a quantum on its occupied THR (S11-21); `thread_parts` (the parallel
    /// loops) refuses a board that has it.
    pub fn agent_channel(&self) -> Option<AgentChannel> {
        self.agent_channel.clone()
    }

    pub fn display(&self) -> DisplayHandle {
        self.display.clone()
    }

    /// Copies the current frame of the display into `output` (S8-11), read-only: the same
    /// bytes as `self.display().read_frame_into(bus, output)` without needing the bus. Call it
    /// only between quanta (from the hook of [`Self::run_boot_with_quantum_hook`] or after a run).
    pub fn read_display_frame_into(
        &self,
        output: &mut [u8],
    ) -> Result<FrameInfo, DisplayReadError> {
        self.display.read_frame_into(&self.bus, output)
    }

    /// Handle of the NVMe controller; `None` unless `FirmwareBoardConfig::nvme` was set.
    pub fn nvme(&self) -> Option<NvmeHandle> {
        self.nvme.clone()
    }

    /// Handle of the xHCI controller; `None` unless `FirmwareBoardConfig::xhci` was set. The
    /// host queues input with [`XhciHandle::push_input`]; the run loops service the controller.
    pub fn xhci(&self) -> Option<XhciHandle> {
        self.xhci.clone()
    }

    /// Handle of the NIC; `None` unless `FirmwareBoardConfig::nic` was set. The host attaches
    /// the network backend with [`NicHandle::attach_backend`]; the run loops service the NIC.
    pub fn nic(&self) -> Option<NicHandle> {
        self.nic.clone()
    }

    /// Host-side handles of the keyboard and the pointer behind the xHCI ports (LED state,
    /// counters); `None` without `FirmwareBoardConfig::xhci`.
    pub fn hid_devices(&self) -> Option<HidDevices> {
        self.hid.clone()
    }

    /// Both committed NVRAM banks, for the owning host between guest quanta.
    pub fn nvram_snapshot(&self) -> Option<Vec<u8>> {
        self.nvram.as_ref().map(NvramHandle::nvram_snapshot)
    }

    /// Snapshot the TPM's persistent state between guest quanta. Contains
    /// unencrypted secrets and must be protected by the owning host.
    pub fn tpm_state_snapshot(&self) -> Option<Vec<u8>> {
        self.tpm
            .as_ref()
            .map(|handle| handle.persistent_snapshot(self.clock().now_ns()))
    }

    /// Trusted platform locality controls for the board's optional CRB TPM.
    pub fn tpm_platform_port(&self) -> Option<TpmPlatformPort<'_>> {
        self.tpm.as_ref().map(|tpm| tpm.platform_port(&self.clock))
    }

    /// Replaces the board's interrupt source (S4-38L8b). Only tests call it; production code
    /// installs the PIT -> 8259 source through [`Self::enable_pit_pic_irq`] (S4-39g, user-mode
    /// run). The board has [`NoIrqSource`] by default.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_irq_source(&mut self, source: Box<dyn BoardIrqSource>) {
        self.irq_source = source;
    }

    /// A source of the PIT channel 0 line through the IOAPIC INTIN2 and the Local APIC of this
    /// board (S4-38L8c), on the board clock; `None` without the Linux profile (no PIT). It is not
    /// installed: the caller gives it to [`Self::set_irq_source`] (a clone of it stays a handle
    /// for the counters). The `X8664V1Stage4AApic` run installs it through
    /// [`Self::enable_pit_ioapic_irq`] (S4-38L8d); a test may install a source itself.
    ///
    /// The IOAPIC handle is made from the Local APIC handle the way `assemble_board` makes the
    /// IOAPIC device: both views share one pair state.
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
    /// installed without the Linux-profile PIT and PIC, or if the IOAPIC route is active.
    /// Call during board configuration, before executing the guest.
    pub fn enable_pit_pic_irq(&mut self) -> bool {
        if self.pit_pic_source.is_some() {
            return true;
        }
        // S4-38L8d: the 8259 and the IOAPIC sources are exclusive (one board interrupt source).
        if self.pit_ioapic_source.is_some() {
            return false;
        }
        let (Some(pit), Some(pic)) = (self.pit.clone(), self.pic_master.clone()) else {
            return false;
        };
        // S4-39g2: the kernel's clockevent writes channel 0 control word 0x38 (mode 4).
        pit.enable_software_strobe();
        let source = PitPicIrqSource::new(pit, pic, self.clock.clone());
        self.irq_source = Box::new(source.clone());
        self.pit_pic_source = Some(source);
        true
    }

    /// S7-31m: whether [`Self::enable_pit_pic_irq`] would succeed (a pure check): the Linux-profile
    /// PIT and 8259 exist and the IOAPIC source is not installed.
    pub(crate) fn pit_pic_irq_available(&self) -> bool {
        self.pit_pic_source.is_some()
            || (self.pit.is_some() && self.pic_master.is_some() && self.pit_ioapic_source.is_none())
    }

    /// S4-38L8d: whether [`Self::enable_pit_ioapic_irq`] would succeed (a pure check): the
    /// Linux-profile PIT exists and the 8259 source is not installed.
    pub(crate) fn pit_ioapic_irq_available(&self) -> bool {
        self.pit_ioapic_source.is_some() || (self.pit.is_some() && self.pit_pic_source.is_none())
    }

    /// S4-38L8d: install the PIT channel 0 -> IOAPIC INTIN2 -> Local APIC source (ISA IRQ 0 is
    /// overridden to GSI 2 by the MADT) as the board's interrupt source (once; a second call keeps
    /// the installed one). It also arms the PIT software strobe, like the 8259 variant, and opens the
    /// Local APIC and IOAPIC MMIO windows of the set loop (the guest's register accesses). `false`
    /// and nothing installed without the Linux-profile PIT, or when the 8259 source
    /// ([`Self::enable_pit_pic_irq`]) is already installed: the two are exclusive. Called by the
    /// `X8664V1Stage4AApic` run and by explicitly configured host quanta.
    pub fn enable_pit_ioapic_irq(&mut self) -> bool {
        if self.pit_ioapic_source.is_some() {
            return true;
        }
        if self.pit_pic_source.is_some() {
            return false;
        }
        let Some(source) = self.pit_ioapic_irq_source() else {
            return false;
        };
        if let Some(pit) = &self.pit {
            pit.enable_software_strobe();
        }
        self.bus.enable_apic_mmio_windows();
        self.irq_source = Box::new(source.clone());
        self.pit_ioapic_source = Some(source);
        true
    }

    /// The installed PIT -> IOAPIC source (a second handle on it), if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pit_ioapic_source(&self) -> Option<PitIoapicIrqSource> {
        self.pit_ioapic_source.clone()
    }

    /// S4-39i: wrap the installed PIT -> IR0 source with the UART interrupt output -> 8259 master
    /// IR4 (once; a second call keeps the installed one). `false` and nothing changed unless
    /// [`Self::enable_pit_pic_irq`] installed the PIT source first (so the PIC exists). Called by
    /// the user-mode run of the Linux board only; without it the UART line reaches nothing.
    pub(crate) fn enable_uart_irq4(&mut self) -> bool {
        if self.uart_irq_source.is_some() {
            return true;
        }
        let (Some(pit_pic), Some(pic)) = (self.pit_pic_source.clone(), self.pic_master.clone())
        else {
            return false;
        };
        let source = UartPicIrqSource::new(pit_pic, self.uart.clone(), pic);
        self.irq_source = Box::new(source.clone());
        self.uart_irq_source = Some(source);
        true
    }

    /// S4-38 (UART -> INTIN4): wrap the installed PIT -> IOAPIC INTIN2 source with the UART
    /// interrupt output -> IOAPIC INTIN4 (once; a second call keeps the installed one). `false` and
    /// nothing changed unless [`Self::enable_pit_ioapic_irq`] installed the PIT source first (so the
    /// IOAPIC and the Local APIC windows are open). The 8259 variant ([`Self::enable_uart_irq4`])
    /// cannot coexist: it needs the 8259 PIT source, which excludes the IOAPIC one. A PCI NVMe
    /// endpoint and the xHCI function also contribute their INTx lines (GSI 16 and 17) through
    /// the same IOAPIC source.
    pub fn enable_uart_ioapic_irq4(&mut self) -> bool {
        if self.uart_ioapic_source.is_some() {
            return true;
        }
        let Some(pit_ioapic) = self.pit_ioapic_source.clone() else {
            return false;
        };
        let io_apic = IoApic::new(self.lapic.clone()).handle();
        // The agent channel (COM2, IRQ3) is presented to INTIN3 by the same source (S11-21).
        let source = match &self.agent_channel {
            Some(channel) => UartIoapicIrqSource::new_with_com2(
                pit_ioapic,
                self.uart.clone(),
                channel.uart(),
                io_apic.clone(),
            ),
            None => UartIoapicIrqSource::new(pit_ioapic, self.uart.clone(), io_apic.clone()),
        };
        let mut chain: Box<dyn BoardIrqSource> = match &self.nvme {
            Some(nvme) if nvme.has_pci_transport() => Box::new(NvmeIoapicIrqSource::new(
                Box::new(source.clone()),
                nvme.clone(),
                io_apic.clone(),
            )),
            _ => Box::new(source.clone()),
        };
        // The xHCI INTA line (GSI 17) on top, when the board has the controller.
        if let Some(xhci) = &self.xhci {
            chain = Box::new(XhciIoapicIrqSource::new(
                chain,
                xhci.clone(),
                io_apic.clone(),
            ));
        }
        // The NIC INTA line (GSI 18) on top of that.
        if let Some(nic) = &self.nic {
            chain = Box::new(NicIoapicIrqSource::new(chain, nic.clone(), io_apic));
        }
        self.irq_source = chain;
        self.uart_ioapic_source = Some(source);
        true
    }

    /// The installed UART -> INTIN4 source (a second handle on it), if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_ioapic_source(&self) -> Option<UartIoapicIrqSource> {
        self.uart_ioapic_source.clone()
    }

    /// The installed UART -> IR4 source (a second handle on it), if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_irq_source(&self) -> Option<UartPicIrqSource> {
        self.uart_irq_source.clone()
    }

    /// S4-38L4i: install the checkpoint recorder of the CPL0-set loop (replaces a previous one).
    /// It only observes: the run with it retires the same instructions in the same quanta as the
    /// run without it. Off by default.
    /// A table that [`LinuxCheckpointTable::validate`] refuses leaves the board as it was.
    pub(crate) fn enable_checkpoints(
        &mut self,
        table: LinuxCheckpointTable,
    ) -> Result<(), LinuxCheckpointTableError> {
        self.checkpoints = Some(CheckpointRecorder::new(table)?);
        Ok(())
    }

    /// S4-38L4i: the checkpoints recorded so far (in id order) and the recorder removed; `None`
    /// when none was enabled.
    /// S7-31e (diagnostics only): the raw entries of the 4-level walk of `linear` from the board CR3,
    /// read from RAM without touching an accessed or dirty bit. Each item is the level name, the
    /// guest-physical address of the entry and its raw value; the walk stops at the first
    /// non-present entry, at a large page, after the PTE, or when an entry cannot be read from RAM.
    pub(crate) fn raw_page_walk(&mut self, linear: u64) -> Vec<(&'static str, u64, u64)> {
        const LEVELS: [(&str, u32); 4] = [("PML4E", 39), ("PDPTE", 30), ("PDE", 21), ("PTE", 12)];
        let mut table = self.page_table_root & 0x000f_ffff_ffff_f000;
        let mut walk = Vec::new();
        for (index, (name, shift)) in LEVELS.into_iter().enumerate() {
            let address = table + ((linear >> shift) & 0x1ff) * 8;
            let mut raw = [0u8; 8];
            if self
                .bus
                .read_flat_bytes(GuestPhysicalAddress::new(address), &mut raw)
                .is_err()
            {
                break;
            }
            let raw = u64::from_le_bytes(raw);
            walk.push((name, address, raw));
            let large = matches!(index, 1 | 2) && raw & 0x80 != 0;
            if raw & 1 == 0 || large {
                break;
            }
            table = raw & 0x000f_ffff_ffff_f000;
        }
        walk
    }

    /// S7-31e: CR2 and the error code of the first `#PF` delivered to the guest while checkpoints
    /// were enabled (diagnostics only).
    pub(crate) fn first_delivered_page_fault(&self) -> Option<(u64, u32)> {
        self.checkpoints
            .as_ref()
            .and_then(CheckpointRecorder::first_page_fault)
    }

    /// S7-31q: watch the entry of the given functions through the CPL0-set loop (opt-in, observation
    /// only; with no checkpoint table the per-boundary predicates stay off).
    pub(crate) fn enable_call_trace(
        &mut self,
        targets: Vec<crate::linux_checkpoints::CallTraceTarget>,
        system_table: Option<u64>,
    ) {
        match self.checkpoints.as_mut() {
            Some(recorder) => recorder.set_call_trace(targets, system_table),
            None => {
                self.checkpoints = Some(CheckpointRecorder::call_trace_only(targets, system_table))
            }
        }
    }

    /// PERF-25 (tests): one instruction per call of the set in the Linux runs of this board, as
    /// before the batched calls, or (`true`, the default) several.
    #[cfg(test)]
    pub(crate) fn set_instruction_batches(&mut self, on: bool) {
        self.instruction_batches = on;
    }

    /// PERF-20: count the retired operations of the CPL0-set loop (opt-in, observation only).
    pub(crate) fn enable_op_histogram(&mut self) {
        self.checkpoints
            .get_or_insert_with(CheckpointRecorder::histogram_only)
            .enable_op_histogram();
    }

    /// The histogram counted so far and the counting stopped; `None` when none was started.
    pub(crate) fn take_op_histogram(&mut self) -> Option<crate::op_histogram::OpHistogramReport> {
        self.checkpoints
            .as_mut()
            .and_then(CheckpointRecorder::take_op_histogram)
    }

    /// The calls traced so far and the trace removed; `None` when none was started.
    pub(crate) fn take_call_trace(&mut self) -> Option<crate::linux_checkpoints::CallTraceReport> {
        self.checkpoints
            .as_mut()
            .and_then(CheckpointRecorder::take_call_trace)
    }

    pub(crate) fn take_checkpoints(&mut self) -> Option<Vec<LinuxCheckpointRecord>> {
        self.checkpoints.take().map(|recorder| recorder.records())
    }

    /// S4-13b: ask the CPL0-set loop for the fetch cache report of its next runs (page walks by
    /// purpose, the cache counters summed over the calls, the phases around C5). It only
    /// observes. Off by default.
    pub fn enable_fetch_cache_report(&mut self) {
        self.fetch_cache_report_enabled = true;
    }

    /// PERF-11 (ADR-0016, prototype): the set loops of the next runs serve same-page qword reads
    /// through the data translation cache (one quantum, exact `WriteWatch` policy, plain RAM
    /// only). Off by default; a diagnostic run (a sweep, the fetch cache report, an execution
    /// observation) keeps every walk whatever this says.
    pub fn enable_data_cache_prototype(&mut self) {
        self.data_cache_prototype = true;
    }

    /// S4-13b: the report the last run left (also after a failed run, with what was counted up to
    /// the failure) and the request removed; `None` when it was not enabled.
    pub fn take_fetch_cache_report(&mut self) -> Option<LinuxFetchCacheReport> {
        self.fetch_cache_report_enabled = false;
        self.fetch_cache_report.take()
    }

    /// S4-13c: ask the CPL0-set loop for shadow caches (the grid of capacity, replacement, tag
    /// granularity, lifecycle and invalidation) over the events of the fetch cache of its next
    /// runs. They only observe; the run, its report and the real cache's counters are unchanged.
    /// Needs the fetch cache on (without it there are no events); the phases around C5 need the
    /// checkpoint recorder. Off by default.
    pub fn enable_fetch_cache_sweep(&mut self, configs: Vec<SweepConfig>) {
        self.fetch_cache_sweep_configs = Some(configs);
    }

    /// S4-13d: let the fetch cache of the CPL0-set loop's next runs tag a walk that ended at a
    /// 2 MiB leaf (plain RAM) with the whole leaf instead of the 4 KiB page. The answers do not
    /// change (permissions, accessed/dirty bits, faults and flushes are those of the 4 KiB tag);
    /// only the number of page walks does. Off by default (4 KiB tag); needs the fetch cache on.
    pub fn enable_fetch_cache_leaf_2m_tag(&mut self) {
        self.fetch_cache_tag = FetchCacheTag::Leaf2M;
    }

    /// S4-13c: the result of the last run (also after a failed run) and the request removed;
    /// `None` when it was not enabled or the run used no fetch cache.
    pub fn take_fetch_cache_sweep(&mut self) -> Option<FetchSweepReport> {
        self.fetch_cache_sweep_configs = None;
        self.fetch_cache_sweep_report.take()
    }

    /// S4-23j: opt in to the single clock owner for the CPL0-set loop. The owner claims the board
    /// clock's advancer for good and takes the LAPICs of all vCPUs in index order; from then on
    /// every move of time of [`Self::run_linux_boot_cpl0_set`] (retired instructions, the PM, PIT
    /// and RDTSC ticks, the idle jump of a halted CPU) goes through it, the quantum is cut with
    /// [`ClockOwner::step_budget`], and `advance_time_ns` (the standalone advance) fails with
    /// `ClockAlreadyOwned`. A second call keeps the owner and succeeds. Refused (nothing
    /// changed) when the PIT or the LAPIC timer was assembled on a clock other than the board
    /// clock (their deadlines would be in another time domain) or when somebody else already
    /// owns the clock. Without this call the board is exactly as before and never claims the
    /// clock.
    ///
    /// After the call `run_boot`, `run_boot_with` and `run` fail wherever they advance time
    /// (`advance_time_ns` gives `ClockAlreadyOwned`). This is expected: the Linux paths that use
    /// the owner call only `run_linux_boot_cpl0_set`, which goes through the owner.
    ///
    /// Idle: when the wake time of the board source is not after `now` the jump is 0 and the
    /// arbiter is not asked, so a Local APIC vector is not checked (`Woken` refusal) in that
    /// case; the CPU wakes for the board source exactly as in `Standalone`.
    pub(crate) fn enable_clock_owner(&mut self) -> Result<(), FirmwareBoardError> {
        if matches!(self.clock_driver, BoardClock::Owned(_)) {
            return Ok(());
        }
        if self.foreign_timer_clock {
            return Err(FirmwareBoardError::Config(
                "clock owner: a timer runs on a clock other than the board clock",
            ));
        }
        let apics: Option<Vec<LocalApicHandle>> = (0..self.vcpus.len())
            .map(|index| self.lapic.for_cpu(index))
            .collect();
        let apics = apics.ok_or(FirmwareBoardError::Config(
            "clock owner: a vCPU has no Local APIC",
        ))?;
        let owner = ClockOwner::new(&self.clock, apics).map_err(FirmwareBoardError::Clock)?;
        self.clock_driver = BoardClock::Owned(owner);
        Ok(())
    }

    /// Whether the clock owner drives the clock of the CPL0-set loop (S4-23j).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn clock_owner_enabled(&self) -> bool {
        matches!(self.clock_driver, BoardClock::Owned(_))
    }

    /// Use the interleaved scheduler for CPL0 Linux execution on a single-vCPU board.
    /// Zero-budget and non-running entry states use the ordinary CPL0 report path.
    pub fn enable_linux_interleaved(&mut self) -> Result<(), FirmwareBoardError> {
        self.enable_linux_interleaved_with_policy(QuantumPolicy::Fixed(u64::MAX))
    }

    /// Use seeded interleaved quanta and the owned clock for Linux on a single-vCPU board.
    /// Each run starts its scheduler from the given seed.
    pub fn enable_seeded_linux_interleaved(&mut self, seed: u64) -> Result<(), FirmwareBoardError> {
        self.enable_linux_interleaved_with_policy(QuantumPolicy::seeded(seed))
    }

    fn enable_linux_interleaved_with_policy(
        &mut self,
        policy: QuantumPolicy,
    ) -> Result<(), FirmwareBoardError> {
        if self.vcpu_count() != 1 {
            return Err(FirmwareBoardError::Config(
                "Linux interleaved execution requires one vCPU",
            ));
        }
        self.enable_clock_owner()?;
        self.linux_interleaved_policy = Some(policy);
        Ok(())
    }

    pub(crate) fn linux_interleaved(&self) -> bool {
        self.linux_interleaved_policy.is_some()
    }

    /// Move the board clock of the CPL0-set loop by `ns` of device time (the PM, PIT and RDTSC
    /// ticks): through the board itself, or through the owner (which also dispatches the LAPIC
    /// timers) once [`Self::enable_clock_owner`] installed it.
    fn charge_ns(&mut self, ns: u64) -> Result<(), FirmwareBoardError> {
        match &mut self.clock_driver {
            BoardClock::Standalone => self.advance_time_ns(ns),
            BoardClock::Owned(owner) => owner.account_device_ns(ns).map(drop).map_err(owner_error),
        }
    }

    /// The idle skip of a halted CPU (IF = 1) to the wake time `wake` (of the board source, or the
    /// earlier Local APIC timer under the IOAPIC source, `idle_wake_ns`): returns
    /// the nanoseconds skipped. Standalone: the board advances by `wake - now` (0 when the time
    /// has come), exactly as before. Owned (S4-23j): a wake time that is not after `now` skips
    /// nothing; otherwise the arbiter decides through [`ClockOwner::idle_step`] (every vCPU with its own
    /// [`Vcpu::activity`] and IF; S6-15) and the clock jumps to `wake`;
    /// it ends in `NoProgress` with the skip above 0, because the deadline of the board is then
    /// due and the loop consumes it itself. A Local APIC timer due earlier is dispatched on the
    /// way. A Local APIC vector that would wake the CPU is refused (`Config`), except under the
    /// IOAPIC source (S4-38L8i): its `pending_vector` is the highest eligible Local APIC vector,
    /// so the loop delivers a Local APIC timer vector through the IDT. There `idle_to` returns, the
    /// CPU is `Running` again and the next boundary delivers the vector. Under any other source the
    /// loop delivers the board source only, so the guest would pass its `HLT` with no interrupt.
    fn idle_to(&mut self, wake: u64) -> Result<u64, FirmwareBoardError> {
        let delivers_lapic_vectors = self.pit_ioapic_source.is_some();
        let BoardClock::Owned(owner) = &mut self.clock_driver else {
            let idle = wake.saturating_sub(self.clock.snapshot().now_ns());
            self.advance_time_ns(idle)?;
            return Ok(idle);
        };
        let mut idle = 0u64;
        loop {
            if wake <= owner.now_ns() {
                // Nothing to skip: the wake time has come (first iteration: no jump at all).
                return Ok(idle);
            }
            // S6-15: every vCPU shows its own activity ([`Vcpu::activity`]) and IF; before, the
            // boot CPU was assumed halted and every other vCPU assumed waiting for SIPI. The
            // caller halts the boot CPU with IF = 1 before it asks to skip idle time.
            let views = self
                .vcpus
                .iter()
                .enumerate()
                .map(|(index, vcpu)| {
                    Ok(WakeView {
                        activity: SchedActivity::from(vcpu.activity()),
                        if_set: vcpu.interrupts_enabled(),
                        apic: self.lapic.for_cpu(index).ok_or(FirmwareBoardError::Config(
                            "clock owner: a vCPU has no Local APIC",
                        ))?,
                    })
                })
                .collect::<Result<Vec<_>, FirmwareBoardError>>()?;
            let step = owner
                .idle_step(false, &views, &[wake])
                .map_err(owner_error)?;
            idle += step.idle_ns;
            match step.outcome {
                IdleOutcome::NoProgress if step.idle_ns > 0 => return Ok(idle),
                IdleOutcome::Advanced(_) if step.idle_ns > 0 => {}
                IdleOutcome::Woken(_) if delivers_lapic_vectors => return Ok(idle),
                IdleOutcome::Woken(_) => return Err(UNROUTED_LAPIC_WAKE),
                _ => {
                    return Err(FirmwareBoardError::Config(
                        "clock owner: the idle step did not reach the wake time",
                    ));
                }
            }
        }
    }

    /// S4-38L8i: the virtual time at which the Local APIC timer of the boot CPU can wake a halted
    /// CPU with IF = 1 under the APIC profile: now when a Local APIC vector is deliverable now,
    /// else the deadline of the armed timer when its LVT is unmasked and its vector would be
    /// accepted (Local APIC enabled, class above the processor priority), else `None`. Only with
    /// the clock owner (it dispatches the timers; S4-23j) and the IOAPIC source (it delivers the
    /// Local APIC's vectors); otherwise `None`, as before.
    fn lapic_timer_wake_ns(&self) -> Option<u64> {
        if self.pit_ioapic_source.is_none() || !matches!(self.clock_driver, BoardClock::Owned(_)) {
            return None;
        }
        let lapic = self.lapic.for_cpu(BOOT_VCPU_INDEX)?;
        let now = self.clock.snapshot().now_ns();
        if lapic.eligible().is_some() {
            return Some(now);
        }
        let deadline = lapic.next_timer_deadline()?;
        let vector = lapic.timer_vector()?;
        (lapic.would_accept(vector) && deadline > now).then_some(deadline)
    }

    /// The wake time of a halted CPU: the earlier of the board source's and the Local APIC timer's.
    /// The `min` is not observable on its own: `idle_to` hands the time to the arbiter, which jumps
    /// to the earliest of its Local APIC timer deadlines and this time either way; what this
    /// function adds is the wake time when the source has none (`None`).
    fn idle_wake_ns(&self) -> Option<u64> {
        match (self.irq_source.wake_ns(), self.lapic_timer_wake_ns()) {
            (Some(source), Some(timer)) => Some(source.min(timer)),
            (source, timer) => source.or(timer),
        }
    }

    /// The COM1 transmit path between two calls of a set loop (S4-39i): a THR write clears the THRE
    /// condition and the pump below raises it again before the next instruction boundary; the
    /// interrupt sources sample the level on both sides so that the fall and the rise are both seen
    /// (one edge on IR4 per byte). Every byte that completes goes to `com1_log` (and to the opt-in
    /// console contexts; `retired` is the count the UART output context records). Shared by the
    /// Linux loop and the vCPU quantum of S6-16d.
    fn pump_uart_transmit(&mut self, retired: u64) {
        let uart = self.uart.clone();
        if let Some(source) = &self.uart_irq_source {
            source.sample();
        }
        if let Some(source) = &self.uart_ioapic_source {
            source.sample();
        }
        // The agent channel (COM2) moves its bytes between the same two samples, so that the fall
        // and the rise of its line are both seen (S11-21).
        if let Some(channel) = &self.agent_channel {
            channel.pump();
        }
        while uart.transfer_holding_to_shift() {
            if let Some(byte) = uart.complete_transmission() {
                self.com1_log.push(byte);
                if self.console_context_parser.is_some() {
                    self.sample_console_output_context(byte);
                }
                if self.uart_output_context.is_some() {
                    self.sample_uart_output_context(
                        byte,
                        self.com1_log.bytes.len() as u64 - 1,
                        retired,
                    );
                }
            }
        }
        if let Some(source) = &self.uart_irq_source {
            source.sample();
        }
        if let Some(source) = &self.uart_ioapic_source {
            source.sample();
        }
    }

    /// The nearest edge of the board's interrupt source ([`BoardIrqSource::next_deadline_ns`]):
    /// `None` for [`NoIrqSource`]. The deadline the scheduler of S6-15 gives to every quantum
    /// ([`crate::smp_scheduler::SmpHost::board_deadline`], S6-16d).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn irq_deadline_ns(&self) -> Option<u64> {
        self.irq_source.next_deadline_ns()
    }

    /// The time at which the board's interrupt source can next wake a halted boot processor
    /// ([`BoardIrqSource::wake_ns`]): not after the clock when a vector is deliverable now, `None`
    /// when nothing can wake it (the line is masked or in service, no edge). The scheduler of
    /// S6-15 asks it for the boot processor only ([`crate::smp_scheduler::SmpHost::board_wakes`],
    /// S6-21 step 1).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn irq_wake_ns(&self) -> Option<u64> {
        self.irq_source.wake_ns()
    }

    /// An AP receives a board PIT edge only through its own LAPIC.
    pub(crate) fn ap_pit_wake_ns(&self, cpu: usize) -> Option<u64> {
        self.pit_ioapic_source.as_ref()?.wake_ns_for(cpu)
    }

    /// Charge `retired` instructions to the clock of the CPL0-set loop (one ns each, D7).
    fn charge_retired(&mut self, retired: u64) -> Result<(), FirmwareBoardError> {
        match &mut self.clock_driver {
            BoardClock::Standalone => self.advance_time_ns(retired),
            BoardClock::Owned(owner) => owner
                .account_retired(retired)
                .map(drop)
                .map_err(owner_error),
        }
    }

    /// Handle of the 8259 master chip (S4-39g); `None` without the Linux profile.
    pub(crate) fn pic_master(&self) -> Option<Pic8259Handle> {
        self.pic_master.clone()
    }

    /// The installed PIT -> 8259 source (a second handle on it), if any.
    pub(crate) fn pit_pic_source(&self) -> Option<PitPicIrqSource> {
        self.pit_pic_source.clone()
    }

    /// True under `linux_profile`: the RDTSC time source (and with it the tick) exists only
    /// there, see `assemble_board`.
    pub(crate) const fn has_linux_profile(&self) -> bool {
        self.rdtsc_tick.is_some()
    }

    /// True for a board loaded from a firmware image (it has an entry to reset to); false for the
    /// Linux direct board, which has no firmware.
    pub(crate) const fn has_firmware_entry(&self) -> bool {
        self.reset_source.is_some()
    }

    /// S7-31c (ADR-0013 variant (b)): bind the firmware device apertures (NVRAM, display
    /// registers, the firmware NVMe window, TPM CRB) to the machine bus, so that the CPL0 set
    /// loop serves them like the firmware profile loop does. Idempotent for a given board.
    pub(crate) fn serve_firmware_set_windows(&mut self) {
        let mut apertures = Vec::new();
        if self.display_mmio.is_some() {
            apertures.push((
                crate::display_device::DISPLAY_REGISTERS_BASE,
                crate::display_device::DISPLAY_REGISTERS_BYTES,
            ));
        }
        if self.nvram.is_some() {
            apertures.push((
                crate::nvram_device::NVRAM_MMIO_BASE,
                crate::nvram_device::NVRAM_MMIO_BYTES,
            ));
        }
        // A firmware board never has `nvme_pci` (the loader refuses it), so the controller sits
        // at the firmware's own NVMe window; under the Linux profile it is a bus device.
        if self.nvme.is_some() {
            apertures.push((crate::nvme::NVME_MMIO_BASE, crate::nvme::NVME_MMIO_BYTES));
        }
        if self.tpm.is_some() {
            apertures.push((crate::tpm_crb::TPM_CRB_BASE, crate::tpm_crb::TPM_CRB_LEN));
        }
        if self.xhci.is_some() {
            apertures.push((XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES));
        }
        if self.nic.is_some() {
            apertures.push((NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES));
        }
        self.bus.serve_firmware_set_windows(&apertures);
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

    /// The EFER of the translation walkers of the boot processor: `FIRMWARE_EFER` (LME|LMA) with
    /// the NXE bit of the guest EFER (S2-25f). NXE starts set (`FIRMWARE_EFER`) and a guest
    /// `WRMSR IA32_EFER` through the CPL0 set may clear it (or set it again when the CPU profile
    /// declares NX), so every walker this board builds (the quantum runs, the fetch view, the
    /// frame-chain captures, the stop record) takes it from here instead of the constant. LME and
    /// LMA never change in the set; the guest's `SCE` is not part of it.
    pub fn efer(&self) -> u64 {
        (FIRMWARE_EFER & !EFER_NXE) | (self.vcpus[BOOT_VCPU_INDEX].guest_efer & EFER_NXE)
    }

    /// The guest-visible `IA32_EFER`: the walker EFER, plus `SCE` once the guest wrote it
    /// through the CPL0 set (S4-38L7f).
    pub fn guest_efer(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_efer
    }

    /// Stores the guest-visible EFER a CPL0-set run left (S4-38L7f).
    #[cfg(test)]
    pub(crate) fn set_guest_efer(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_efer = value;
    }

    /// The guest PCI CONFIG_ADDRESS register (S4-38L7w); `0` until a guest `OUT` to `0xCF8`.
    pub const fn pci_config_address(&self) -> u32 {
        self.pci_config_address
    }

    /// Stores the CONFIG_ADDRESS a CPL0-set run left (S4-38L7w).
    pub(crate) fn set_pci_config_address(&mut self, value: u32) {
        self.pci_config_pending |= self.pci_config_address != value;
        self.pci_config_address = value;
    }

    pub(crate) fn import_linux_pci_address(&mut self) -> Result<(), FirmwareBoardError> {
        if self.pci_config_pending {
            self.bus
                .write_port_shared(
                    PortAddress::new(crate::pci_ecam::PCI_CONF1_BASE_PORT),
                    AccessWidth::Dword,
                    self.pci_config_address,
                )
                .map_err(FirmwareBoardError::Bus)?;
            self.pci_config_pending = false;
        }
        Ok(())
    }

    /// The guest `IA32_STAR` (S4-38L7ao); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of STAR stays refused.
    pub fn guest_star(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_star
    }

    /// Stores the STAR a CPL0-set run left (S4-38L7ao).
    #[cfg(test)]
    pub(crate) fn set_guest_star(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_star = value;
    }

    /// The guest `IA32_LSTAR` (S4-38L7ap); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of LSTAR stays refused.
    pub fn guest_lstar(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_lstar
    }

    /// Stores the LSTAR a CPL0-set run left (S4-38L7ap).
    #[cfg(test)]
    pub(crate) fn set_guest_lstar(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_lstar = value;
    }

    /// The guest `IA32_CSTAR` (S4-38L7aq); `0` until a guest `WRMSR` of it. Not readable by the
    /// guest: `RDMSR` of CSTAR stays refused.
    pub fn guest_cstar(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_cstar
    }

    /// Stores the CSTAR a CPL0-set run left (S4-38L7aq).
    #[cfg(test)]
    pub(crate) fn set_guest_cstar(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_cstar = value;
    }

    /// The guest `IA32_SYSENTER_CS` (S4-38L7ar); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_CS stays refused.
    pub fn guest_sysenter_cs(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_cs
    }

    /// Stores the SYSENTER_CS a CPL0-set run left (S4-38L7ar).
    #[cfg(test)]
    pub(crate) fn set_guest_sysenter_cs(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_cs = value;
    }

    /// The guest `IA32_SYSENTER_ESP` (S4-38L7as); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_ESP stays refused.
    pub fn guest_sysenter_esp(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_esp
    }

    /// Stores the SYSENTER_ESP a CPL0-set run left (S4-38L7as).
    #[cfg(test)]
    pub(crate) fn set_guest_sysenter_esp(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_esp = value;
    }

    /// The guest `IA32_SYSENTER_EIP` (S4-38L7as); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSENTER_EIP stays refused.
    pub fn guest_sysenter_eip(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_eip
    }

    /// Stores the SYSENTER_EIP a CPL0-set run left (S4-38L7as).
    #[cfg(test)]
    pub(crate) fn set_guest_sysenter_eip(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_sysenter_eip = value;
    }

    /// The guest `IA32_SYSCALL_MASK` (S4-38L7at); `0` until a guest `WRMSR` of it. The full 64-bit
    /// value written. Not readable by the guest: `RDMSR` of SYSCALL_MASK stays refused.
    pub fn guest_syscall_mask(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_syscall_mask
    }

    /// Stores the SYSCALL_MASK a CPL0-set run left (S4-38L7at).
    #[cfg(test)]
    pub(crate) fn set_guest_syscall_mask(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_syscall_mask = value;
    }

    /// The guest `IA32_KERNEL_GS_BASE` (S4-38L7au); `0` until a guest `WRMSR` of it. The full
    /// 64-bit value written. Not readable by the guest: `RDMSR` of KERNEL_GS_BASE stays refused.
    pub fn guest_kernel_gs_base(&self) -> u64 {
        self.vcpus[BOOT_VCPU_INDEX].guest_kernel_gs_base
    }

    /// Stores the KERNEL_GS_BASE a CPL0-set run left (S4-38L7au).
    #[cfg(test)]
    pub(crate) fn set_guest_kernel_gs_base(&mut self, value: u64) {
        self.vcpus[BOOT_VCPU_INDEX].guest_kernel_gs_base = value;
    }

    /// The guest FP state of the CPL0 set's FP member (S4-38L9d); the initial state until a run
    /// with the member changed it.
    pub fn guest_fp(&self) -> &GuestFpState {
        &self.vcpus[BOOT_VCPU_INDEX].guest_fp
    }

    /// Stores the FP state a CPL0-set run left (S4-38L9d).
    #[cfg(test)]
    pub(crate) fn set_guest_fp(&mut self, value: GuestFpState) {
        self.vcpus[BOOT_VCPU_INDEX].guest_fp = value;
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

    /// The virtual clock as of now (a snapshot). Time moves only when somebody advances it: in
    /// `Standalone` through [`Self::advance_time_ns`], once [`Self::enable_clock_owner`] took
    /// the clock through the [`ClockOwner`] (`charge_ns`, `charge_retired`, `idle_to`).
    pub fn clock(&self) -> VirtualClock {
        self.clock.snapshot()
    }

    pub(crate) fn clock_handle(&self) -> &VirtualClockHandle {
        &self.clock
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
        self.run_quantum(
            false,
            false,
            instruction_budget,
            stop,
            committed_write,
            |_, _| {},
        )
    }

    /// [`Self::run`] with the 4 KiB walker (`large_ram = false`) or with the 2 MiB walk of the
    /// Linux direct boot (`large_ram = true`, S4-38L1: `run_paged_probe_2m_ram`). The CPU
    /// profile, the board CR3 and the RDTSC source are the same in both.
    fn run_quantum(
        &mut self,
        large_ram: bool,
        firmware_boot_profile: bool,
        instruction_budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
        observe: impl FnMut(u64, u64),
    ) -> PagedProbeQuantumReport {
        // S2-25f: the walker EFER is the guest's current NXE, not the constant.
        let efer = self.efer();
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
                &mut self.vcpus[BOOT_VCPU_INDEX].state,
                &mut self.bus,
                self.page_table_root,
                efer,
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
        if firmware_boot_profile {
            let clock = self.clock();
            let vcpu = &mut self.vcpus[BOOT_VCPU_INDEX];
            let report = run_paged_firmware_probe(
                &mut vcpu.state,
                &mut vcpu.guest_fp,
                &mut self.bus,
                FirmwareMmio {
                    nvram: self.nvram.as_ref(),
                    display: self.display_mmio.as_ref(),
                    nvme: self.nvme_mmio.as_ref(),
                    tpm: self.tpm.as_ref(),
                    xhci: self.xhci_mmio.as_ref(),
                    nic: self.nic_mmio.as_ref(),
                    ecam: self.ecam_mmio.as_ref(),
                },
                self.page_table_root,
                efer,
                context,
                instruction_budget,
                INTEGER_FLAGS,
                stop,
                committed_write,
                Some(clock),
                observe,
            );
            if let Some(cr3) = vcpu.state.cr3 {
                self.page_table_root = cr3;
            }
            return report;
        }
        if let Some(tick) = self.rdtsc_tick.clone() {
            let clock = self.clock.clone();
            let mut source = move || {
                tick.store(true, Ordering::Relaxed);
                clock.snapshot().tsc()
            };
            return run_paged_probe_with_rdtsc(
                &mut self.vcpus[BOOT_VCPU_INDEX].state,
                &mut self.bus,
                self.page_table_root,
                efer,
                context,
                instruction_budget,
                INTEGER_FLAGS,
                stop,
                committed_write,
                &mut source,
            );
        }
        run_paged_probe(
            &mut self.vcpus[BOOT_VCPU_INDEX].state,
            &mut self.bus,
            self.page_table_root,
            efer,
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
        let efer = self.efer();
        self.fetch_view_2m_with_privilege(rip, len, PrivilegeLevel::Ring0, efer)
    }

    pub(crate) fn fetch_view_2m_with_privilege(
        &mut self,
        rip: u64,
        len: usize,
        privilege: PrivilegeLevel,
        efer: u64,
    ) -> (Option<u64>, Vec<u8>, Option<String>) {
        self.fetch_vcpu_view_2m(BOOT_VCPU_INDEX, rip, len, privilege, efer)
    }

    pub(crate) fn fetch_vcpu_view_2m(
        &mut self,
        cpu: usize,
        rip: u64,
        len: usize,
        privilege: PrivilegeLevel,
        efer: u64,
    ) -> (Option<u64>, Vec<u8>, Option<String>) {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        };
        let state = &self.vcpus[cpu].state;
        let cr3 = if cpu == BOOT_VCPU_INDEX {
            self.page_table_root
        } else if let Some(cr3) = state.cr3 {
            cr3
        } else {
            return (None, Vec::new(), Some(format!("vCPU {cpu} has no CR3")));
        };
        let paging = match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
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
            match memory.load_u8(physical) {
                Ok(byte) => bytes.push(byte),
                Err(error) => return (first, bytes, Some(format!("{error:?}"))),
            }
        }
        (first, bytes, None)
    }

    pub(crate) fn sync_linux_boot_views(&mut self) -> Result<(), FirmwareBoardError> {
        let cr3 = self.vcpus[BOOT_VCPU_INDEX]
            .state
            .cr3
            .ok_or(FirmwareBoardError::Config(
                "LinuxDirect: the BSP has no CR3",
            ))?;
        self.page_table_root = cr3;
        let cf8 = self
            .bus
            .read_port_shared(
                PortAddress::new(crate::pci_ecam::PCI_CONF1_BASE_PORT),
                AccessWidth::Dword,
            )
            .map_err(FirmwareBoardError::Bus)?;
        self.pci_config_address = cf8;
        Ok(())
    }

    pub(crate) fn pit_read_counts(&self) -> Option<[u64; 2]> {
        self.pit
            .as_ref()
            .map(|pit| [pit.port_b_reads(), pit.counter_port_reads()])
    }

    /// PIT channel 0 as the guest left it (S7-31m); `None` without the Linux profile.
    pub(crate) fn pit_channel0_snapshot(&self) -> Option<crate::PitChannelSnapshot> {
        self.pit.as_ref().map(|pit| pit.snapshot().channel0)
    }

    pub(crate) fn irq0_edges(&self) -> u64 {
        self.irq_source.irq0_edges()
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
        self.run_boot_with(false, instruction_budget, |_, _, _| {}, |_, _| {})
            .map_err(|failure| failure.error)
    }

    /// [`Self::run_boot`] that also calls `between_quanta` at every quantum boundary at which
    /// the run continues (S8-11), after the quantum's COM1 bytes and its virtual-time advance are
    /// applied. The hook gets a shared reference: it can read the COM1 transcript, the clock, the
    /// state and a display frame ([`Self::read_display_frame_into`]). It gets no mutable reference;
    /// the callers in this tree call only reading methods (`diagnostic_*`, `read_*`), but the type
    /// does not forbid `&self` methods that change display state (for example through
    /// `display()`), so "read only" is a convention of the hook, not a guarantee of the type.
    /// Quanta are not cut: a run whose hook does nothing is identical to [`Self::run_boot`], so
    /// the hook sees only the boundaries the guest's own I/O and the budget create. The final
    /// boundary (the run ends) is not reported: the caller reads the machine after the run.
    pub fn run_boot_with_quantum_hook(
        &mut self,
        instruction_budget: u64,
        between_quanta: impl FnMut(&Self, QuantumProgress),
    ) -> Result<FirmwareRunReport, FirmwareBoardError> {
        self.run_boot_with(false, instruction_budget, |_, _, _| {}, between_quanta)
            .map_err(|failure| failure.error)
    }

    /// Run the firmware interpreter and observe each instruction before fetch.
    /// The observer receives RIP, the total retired instructions (including
    /// this quantum), and the completed quanta since this call began.
    /// Observing an instruction does not imply that it will retire.
    pub fn run_boot_observed(
        &mut self,
        instruction_budget: u64,
        observe: impl FnMut(u64, u64, u64),
    ) -> Result<FirmwareRunReport, FirmwareBoardError> {
        if self.has_linux_profile() {
            return Err(FirmwareBoardError::Config(
                "firmware boot observer requires the firmware profile",
            ));
        }
        self.run_boot_with(false, instruction_budget, observe, |_, _| {})
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
        self.run_boot_with(true, instruction_budget, |_, _, _| {}, |_, _| {})
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
    /// for it, and the counters of the report are filled. The board has [`NoIrqSource`] until
    /// [`Self::enable_pit_pic_irq`] (user-mode run, S4-39g) or a test installs another source; with
    /// [`NoIrqSource`] the hook changes no result. Under the PIT -> 8259 source a halted CPU is
    /// woken (see the idle wake in the loop).
    pub(crate) fn run_linux_boot_cpl0_set(
        &mut self,
        instruction_budget: u64,
        set: &Cpl0ProfileSet<'_>,
        cpuid_boundary: bool,
        fetch_cache: bool,
        irq_hook: bool,
    ) -> Result<LinuxSetRunReport, LinuxRunFailure> {
        self.run_linux_boot_cpl0_set_with_observations(
            instruction_budget,
            set,
            cpuid_boundary,
            fetch_cache,
            irq_hook,
            false,
        )
    }

    pub(crate) fn run_linux_boot_cpl0_set_with_observations(
        &mut self,
        instruction_budget: u64,
        set: &Cpl0ProfileSet<'_>,
        cpuid_boundary: bool,
        fetch_cache: bool,
        irq_hook: bool,
        capture_execution_observations: bool,
    ) -> Result<LinuxSetRunReport, LinuxRunFailure> {
        self.run_linux_boot_cpl0_set_with_post_iret_index(
            instruction_budget,
            set,
            cpuid_boundary,
            fetch_cache,
            irq_hook,
            capture_execution_observations,
            None,
            None,
            1,
            None,
            None,
            None,
            false,
            false,
            false,
            false,
        )
    }

    pub(crate) fn run_linux_boot_cpl0_set_with_post_iret_index(
        &mut self,
        instruction_budget: u64,
        set: &Cpl0ProfileSet<'_>,
        cpuid_boundary: bool,
        fetch_cache: bool,
        irq_hook: bool,
        capture_execution_observations: bool,
        selected_post_iret_ordinal: Option<u64>,
        selected_rdtsc_sample_rip: Option<u64>,
        selected_rdtsc_sample_stride: u64,
        selected_rdtsc_entry_sample_rip: Option<u64>,
        selected_rdtsc_sample_window: Option<crate::RdtscSampleWindow>,
        selected_rdtsc_prefix_sentinel_rip: Option<u64>,
        selected_instruction_path_trace: bool,
        selected_elf_stack_copy: bool,
        selected_delay_tsc_call_summary: bool,
        schedule_standalone: bool,
    ) -> Result<LinuxSetRunReport, LinuxRunFailure> {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let cpuid = self.vcpus[BOOT_VCPU_INDEX].cpuid;
        let set = set.with_cpuid_topology(cpuid.apic_id, cpuid.cpu_count);
        let mut remaining = instruction_budget;
        let mut retired = 0u64;
        let mut quanta = 0u64;
        let mut changes: Vec<TranslationContextChange> = Vec::new();
        let mut exceptions_delivered = 0u64;
        let mut page_faults_delivered = 0u64;
        let mut general_protection_faults_delivered = 0u64;
        let mut execution_observations = capture_execution_observations.then(|| {
            LinuxExecutionObservationCollector::with_post_iret_ordinal(selected_post_iret_ordinal)
                .with_elf_stack_copy(selected_elf_stack_copy)
                .with_rdtsc_sample_rip(selected_rdtsc_sample_rip)
                .with_rdtsc_sample_stride(selected_rdtsc_sample_stride)
                .with_rdtsc_entry_sample_rip(selected_rdtsc_entry_sample_rip)
                .with_rdtsc_sample_window(selected_rdtsc_sample_window)
                .with_rdtsc_prefix_sentinel(selected_rdtsc_prefix_sentinel_rip)
                .with_delay_tsc_call_summary(selected_delay_tsc_call_summary)
                .with_instruction_path_trace(selected_instruction_path_trace)
        });
        let mut cpl3_retired = 0u64;
        let mut syscall_journal = crate::linux_direct::SyscallJournal::default();
        self.host_stop_honoured = false;
        self.com1_log.bytes.clear();
        self.console_context_snapshots.clear();
        if self.console_context_parser.is_some() {
            self.console_context_parser = Some(ConsoleLineTrigger::default());
        }
        if self.uart_output_context.is_some() {
            self.uart_output_context = Some(UartOutputContextCapture::default());
        }
        let nvme = self.nvme.clone();
        let xhci = self.xhci.clone();
        let nic = self.nic.clone();
        let acpi_pm = self.acpi_pm.clone();
        let pit = self.pit.clone();
        let rdtsc_tick = self.rdtsc_tick.clone();
        let pit_port_b_reads_before = pit.as_ref().map(Pit8254Handle::port_b_reads);
        let pit_counter_port_reads_before = pit.as_ref().map(Pit8254Handle::counter_port_reads);
        let mut rdtsc_reads = 0u64;
        let mut rdtsc_reads_by_cpl = [0u64; 4];
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
        if let Some(xhci) = &xhci {
            xhci.service(&mut self.bus, |_| {});
        }
        if let Some(nic) = &nic {
            nic.service(&mut self.bus, |_| {});
        }
        self.vcpus[BOOT_VCPU_INDEX].state.cr3 = Some(self.page_table_root);
        let mut irq_counters = IrqCounters::default();
        let mut clock_origin = None;
        let mut pm_timer_ns_charged = 0u64;
        let mut pit_ns_charged = 0u64;
        let mut rdtsc_ns_charged = 0u64;
        let mut idle_ns_advanced = 0u64;
        let delivered_irq_mirror = Cell::new(0u64);
        let mut fetch_report = self
            .fetch_cache_report_enabled
            .then(|| FetchCacheAccumulator::new(fetch_cache, self.checkpoints.is_some()));
        self.fetch_cache_report = None;
        self.fetch_cache_sweep_report = None;
        let fetch_sweep = self
            .fetch_cache_sweep_configs
            .as_deref()
            .filter(|_| fetch_cache)
            .map(|configs| std::rc::Rc::new(FetchCacheSweep::new(configs)));
        // The next call resumes a partial REP: its first boundary is inside the REP.
        let mut partial_rep = false;
        // S2-25 NXE slice: the walker's EFER is the board's `efer()` (the NXE bit of the guest EFER;
        // a guest `WRMSR IA32_EFER` that cleared NXE in an earlier run keeps it clear); a retired
        // NXE write of a quantum reaches the next one through `report.efer`.
        let mut walker_efer = self.efer();
        // S4-38L5a/c: the set models CR2 for `MOV RAX, CR2`, and a delivered `#PF` writes it
        // (S4-38L5c). Like CR3 (kept in `page_table_root` through `report.cr3` and fed back on
        // entry), the CR2 the guest left is kept in `guest_cr2` and fed back on entry,
        // so a handler that reads CR2 in a later call gets the address of its fault. CR2 is
        // `0` (the architectural reset value) until a `#PF` was delivered or the guest wrote
        // it; the CPU state itself carries no CR2 outside this call (the prior value is
        // restored on exit).
        let prior_cr2 = self.vcpus[BOOT_VCPU_INDEX].state.cr2;
        self.vcpus[BOOT_VCPU_INDEX].state.cr2 = Some(
            self.vcpus[BOOT_VCPU_INDEX]
                .guest_cr2
                .or(prior_cr2)
                .unwrap_or(0),
        );
        let mut previous_halt: Option<(u64, bool)> = None;
        let mut run_quantum = |board: &mut Self,
                               scheduler_budget: Option<u64>,
                               scheduler_stop: &mut dyn FnMut() -> bool,
                               executed: &mut Executed,
                               can_resume_halt: &mut bool| {
            if let Some((halted_after_ns, nmi_was_blocked)) = previous_halt.take() {
                let now = board.clock().now_ns();
                let woke_for_nmi = !nmi_was_blocked && board.state().nmi_blocked;
                if !schedule_standalone
                    && !woke_for_nmi
                    && board.pit_ioapic_source.is_none()
                    && board.local_apic().eligible().is_some()
                    && board.irq_wake_ns().is_none_or(|wake| wake > now)
                {
                    board.vcpus[BOOT_VCPU_INDEX].state.activity = Some(CpuActivity::Halted);
                    return ControlFlow::Break(Err(LinuxRunFailure {
                        error: UNROUTED_LAPIC_WAKE,
                        retired,
                    }));
                }
                if !woke_for_nmi {
                    let idle = now.saturating_sub(halted_after_ns);
                    irq_counters.halt_wakes += 1;
                    irq_counters.halt_idle_ns += idle;
                    idle_ns_advanced = idle_ns_advanced.saturating_add(idle);
                    remaining -= 1;
                }
                partial_rep = false;
            }
            // S4-41c-1: a host stop request (it also ended the previous quantum through the stop
            // predicate) empties the remaining budget at this quantum boundary, so the quantum
            // below executes nothing and the run ends with the ordinary budget exit (state as at
            // this boundary). Once honoured it stays honoured.
            if board.host_stop_honoured
                || board
                    .host_stop
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                board.host_stop_honoured = true;
                remaining = 0;
            }
            let uart = board.uart.clone();
            let agent = board.agent_channel.clone();
            let clock_read = Cell::new(None);
            let quantum_clock_ledger = execution_observations.as_ref().and_then(|observations| {
                observations.build_rdtsc_clock_trace_if_active(|| {
                    let (initial_now_ns, tsc_hz) = *clock_origin.get_or_insert_with(|| {
                        let snapshot = board.clock.snapshot();
                        (snapshot.now_ns(), snapshot.tsc_hz())
                    });
                    delivered_irq_mirror.set(irq_counters.irqs_delivered);
                    RdtscClockReadLedger {
                        initial_now_ns,
                        now_ns: 0,
                        tsc: 0,
                        tsc_hz,
                        retired_instructions_charged: retired,
                        pm_timer_ns_charged,
                        pit_ns_charged,
                        rdtsc_ns_charged,
                        idle_ns_advanced,
                        quantum_ordinal: quanta.saturating_add(1),
                        irqs_delivered: delivered_irq_mirror.get(),
                    }
                })
            });
            let capture_clock_read = quantum_clock_ledger.is_some();
            let clock_read_ref = &clock_read;
            let delivered_irq_mirror_ref = &delivered_irq_mirror;
            let mut source = rdtsc_tick.clone().map(|tick| {
                let clock = board.clock.clone();
                move || {
                    tick.store(true, Ordering::Relaxed);
                    if let Some(quantum_clock_ledger) = quantum_clock_ledger {
                        let snapshot = clock.snapshot();
                        let tsc = snapshot.tsc();
                        clock_read_ref.set(Some(RdtscClockReadLedger {
                            now_ns: snapshot.now_ns(),
                            tsc,
                            irqs_delivered: delivered_irq_mirror_ref.get(),
                            ..quantum_clock_ledger
                        }));
                        tsc
                    } else {
                        clock.snapshot().tsc()
                    }
                }
            });
            // S4-41c-1: the host stop request ends the quantum at the next instruction boundary
            // (one relaxed load, only when a request is installed); the quantum is not cut in
            // advance, so the guest-visible time and the quantum layout of a run whose request is
            // never set are those of a run without a request.
            let host_stop = board.host_stop.clone();
            // PERF-04: the bus flags below (the THR of the two UARTs, an NVMe doorbell, the PM
            // stop and timer tick, the PIT port tick) are raised only while the guest's own
            // instruction reaches the device through the set-step dispatch, or between quanta by
            // the host (the quantum's first call sees those). Their mutexes are taken at the first
            // call of a quantum and whenever the count of device accesses has changed since the
            // last look. The atomic conditions and the flags a host thread can raise during a
            // quantum (the xHCI input queue) are read at every call, as before. See
            // CPU_CORE_PERF_REPORT.md, PERF-04, for the table of who raises each flag.
            let device_accesses = board.bus.device_access_counter();
            let bus_flags_looked_at = Cell::new(false);
            let device_accesses_seen = Cell::new(0u64);
            let stop = || {
                if (scheduler_budget.is_some() && scheduler_stop())
                    // A guest write to an xHCI register (a doorbell) or a host input event ends
                    // the quantum (one atomic load).
                    || xhci.as_ref().is_some_and(XhciHandle::wakeup_pending)
                    // A guest write to a NIC register (the tail pointers) ends it as well.
                    || nic.as_ref().is_some_and(NicHandle::wakeup_pending)
                    || rdtsc_tick
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Relaxed))
                    || host_stop
                        .as_deref()
                        .is_some_and(|flag| flag.load(Ordering::Relaxed))
                {
                    return true;
                }
                let accesses = device_accesses.load(Ordering::Relaxed);
                if bus_flags_looked_at.replace(true) && accesses == device_accesses_seen.get() {
                    return false;
                }
                device_accesses_seen.set(accesses);
                uart.holding_occupied()
                    || agent.as_ref().is_some_and(AgentChannel::holding_occupied)
                    || nvme.as_ref().is_some_and(NvmeHandle::doorbell_pending)
                    || acpi_pm
                        .as_ref()
                        .is_some_and(|pm| pm.stop_request().is_some())
                    || acpi_pm
                        .as_ref()
                        .is_some_and(AcpiPmHandle::timer_tick_pending)
                    || pit.as_ref().is_some_and(Pit8254Handle::port_tick_pending)
            };
            // S4-38L8b: the quantum ends at the next edge of the source; the clock moves only
            // after the quantum (one instruction is one ns), so the limit is the time left.
            let mut quantum_budget = remaining;
            if let BoardClock::Owned(owner) = &board.clock_driver {
                // S4-23j: the owner cuts the quantum at the earliest of its LAPIC timers and the
                // source's deadline (the source is asked only under the hook, once per quantum,
                // as before, also with nothing left). An empty budget is never handed to the
                // owner (it would give one) and stays empty.
                let deadline = if irq_hook {
                    board.irq_source.next_deadline_ns()
                } else {
                    None
                };
                if remaining > 0 {
                    quantum_budget = owner.step_budget(remaining, deadline);
                    debug_assert!(quantum_budget >= 1 && quantum_budget <= remaining);
                }
            } else if irq_hook && let Some(deadline) = board.irq_source.next_deadline_ns() {
                let limit = deadline
                    .saturating_sub(board.clock.snapshot().now_ns())
                    .max(1);
                if limit < remaining {
                    quantum_budget = limit;
                }
            }
            let deadline_budget = quantum_budget;
            if let Some(budget) = scheduler_budget {
                quantum_budget = quantum_budget.min(budget);
            }
            // A seeded turn can end earlier than a pending device deadline.
            if irq_hook && deadline_budget < remaining && quantum_budget == deadline_budget {
                irq_counters.quantum_cut_by_deadline += 1;
            }
            let shortened = quantum_budget < remaining;
            // S4-38L4i: the recorder (opt-in, observation only) learns where the quantum starts.
            if let Some(observations) = execution_observations.as_mut() {
                observations.begin_instruction_path_quantum(quanta.saturating_add(1), retired);
            }
            if board.checkpoints.is_some() {
                let ns_at_start = board.clock.snapshot().now_ns();
                if let Some(recorder) = board.checkpoints.as_mut() {
                    recorder.begin_call(retired, quanta + 1, ns_at_start);
                }
            }
            // PERF-10: an ordinary run only; a diagnostic (a sweep, the fetch cache report, an
            // execution observation) keeps the byte-per-fetch decoding.
            let decoded_cache = (fetch_cache
                && fetch_sweep.is_none()
                && !board.fetch_cache_report_enabled
                && execution_observations.is_none())
            .then_some(&board.decoded_instruction_cache);
            // PERF-11 (ADR-0016, prototype): the data translation cache, only when asked for and
            // under the conditions of the decoded-instruction cache (an ordinary run).
            let data_cache = board.data_cache_prototype && decoded_cache.is_some();
            let report = run_cpl0_set_paged_2m_ram_observed_with_sweep(
                &mut board.vcpus[BOOT_VCPU_INDEX].state,
                &mut board.bus,
                board.page_table_root,
                walker_efer,
                context,
                quantum_budget,
                INTEGER_FLAGS,
                &set,
                source
                    .as_mut()
                    .map(|source| source as &mut dyn FnMut() -> u64),
                stop,
                |_| {},
                cpuid_boundary,
                fetch_cache.then_some(FetchCacheInvalidation::WriteWatch),
                irq_hook.then(|| IrqBoundaryHook {
                    source: &mut *board.irq_source,
                    counters: &mut irq_counters,
                    delivered_mirror: capture_clock_read.then_some(&delivered_irq_mirror),
                    skip_first_boundary: partial_rep,
                    // PERF-05: the count of the guest device accesses of this bus.
                    device_accesses: Some(&*device_accesses),
                    polled: None,
                }),
                board.checkpoints.as_mut(),
                execution_observations.as_mut(),
                fetch_sweep.as_ref(),
                board.fetch_cache_tag,
                decoded_cache,
                data_cache,
                // PERF-25: an ordinary run calls the set for several instructions at a time (the
                // loop keeps one per call wherever it has a diagnostic to show every boundary;
                // the fetch cache report and the sweep keep one per call here too).
                board.instruction_batches
                    && !board.fetch_cache_report_enabled
                    && fetch_sweep.is_none(),
            );
            quanta += 1;
            partial_rep = matches!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)
            );
            // The walker of the next quantum starts from the CR3 the guest left.
            board.page_table_root = report.cr3;
            walker_efer = report.efer;
            changes.extend(report.translation_context_changes.iter().copied());
            exceptions_delivered += report.delivered_exceptions;
            page_faults_delivered += report.delivered_page_faults;
            general_protection_faults_delivered += report.delivered_general_protection_faults;
            cpl3_retired += report.cpl3_retired;
            for event in &report.syscall_events {
                syscall_journal.record(*event);
            }
            if let Some(nvme) = &nvme {
                nvme.service(&mut board.bus, |_| {});
            }
            if let Some(xhci) = &xhci {
                xhci.service(&mut board.bus, |_| {});
            }
            if let Some(nic) = &nic {
                nic.service(&mut board.bus, |_| {});
            }
            retired += report.instructions_executed;
            remaining = remaining.saturating_sub(report.instructions_executed);
            if let Some(accumulator) = fetch_report.as_mut() {
                accumulator.add_call(&CallObservation {
                    retired: report.instructions_executed,
                    walks: report.walk_counts,
                    cache: report.fetch_cache_stats,
                    c5_at_start: report.c5_at_call_start,
                    c5_split: report.c5_split,
                });
            }
            board.pump_uart_transmit(retired);
            let failure = |error| LinuxRunFailure { error, retired };
            executed.retired = report.instructions_executed;
            executed.partial_rep = partial_rep;
            if scheduler_budget.is_none()
                && let Err(error) = board.charge_retired(report.instructions_executed)
            {
                return ControlFlow::Break(Err(failure(error)));
            }
            let mut standalone_time = if schedule_standalone && scheduler_budget.is_some() {
                let Some(now) = board.clock().now_ns().checked_add(executed.retired) else {
                    // The scheduler will refuse the retirement charge. Leave device ticks queued.
                    return ControlFlow::Break(Err(failure(FirmwareBoardError::Clock(
                        VirtualClockError::TimeOverflow,
                    ))));
                };
                Some(now)
            } else {
                None
            };
            let mut charge_device = |board: &mut Self, ns| {
                if scheduler_budget.is_some() {
                    if let Some(time) = &mut standalone_time {
                        // Preserve the direct loop's prefix if this tick cannot advance the clock.
                        *time = time
                            .checked_add(ns)
                            .ok_or(FirmwareBoardError::Clock(VirtualClockError::TimeOverflow))?;
                    }
                    executed.device_ns = executed
                        .device_ns
                        .checked_add(ns)
                        .ok_or(FirmwareBoardError::Clock(VirtualClockError::TimeOverflow))?;
                    Ok(())
                } else {
                    board.charge_ns(ns)
                }
            };
            if let Some(pm) = &acpi_pm {
                let tick_ns = pm.take_timer_tick_ns();
                #[cfg(test)]
                let tick_ns = if board.suppress_pm_tick { 0 } else { tick_ns };
                if let Err(error) = charge_device(board, tick_ns) {
                    return ControlFlow::Break(Err(failure(error)));
                }
                if capture_clock_read {
                    pm_timer_ns_charged = pm_timer_ns_charged.saturating_add(tick_ns);
                }
            }
            if let Some(pit) = &pit {
                let tick_ns = pit.take_port_tick_ns();
                if let Err(error) = charge_device(board, tick_ns) {
                    return ControlFlow::Break(Err(failure(error)));
                }
                if capture_clock_read {
                    pit_ns_charged = pit_ns_charged.saturating_add(tick_ns);
                }
            }
            if let Some(flag) = &rdtsc_tick
                && flag.swap(false, Ordering::Relaxed)
            {
                rdtsc_reads = rdtsc_reads.saturating_add(1);
                let cpl = board
                    .state()
                    .cs_selector
                    .map_or(0, |selector| (selector & 3) as usize);
                rdtsc_reads_by_cpl[cpl] = rdtsc_reads_by_cpl[cpl].saturating_add(1);
                if cpl == 0
                    && let Some(observations) = execution_observations.as_mut()
                {
                    let (frame_links, frame_chain_end) = if observations
                        .wants_delay_tsc_call_summary()
                        && report.last_instruction_rip == Some(crate::LINUX_DELAY_TSC_ENTRY_RIP)
                    {
                        board.capture_delay_tsc_frame_chain(
                            report.last_instruction_rip.unwrap_or_default(),
                            board.state().gpr[Register::Rbp as usize],
                            board.page_table_root,
                        )
                    } else {
                        (Vec::new(), crate::FrameChainEnd::NullFramePointer)
                    };
                    observations.record_delay_tsc_entry(
                        report.last_instruction_rip,
                        board.state(),
                        frame_links,
                        frame_chain_end,
                    );
                    observations.record_cpl0_rdtsc(report.last_instruction_rip);
                    observations.record_rdtsc_site_sample(
                        report.last_instruction_rip,
                        board.state(),
                        clock_read.get().filter(|_| {
                            report.last_instruction_rip == selected_rdtsc_entry_sample_rip
                        }),
                    );
                    let prefix_ordinal = observations.record_rdtsc_prefix(
                        report.last_instruction_rip,
                        board.state(),
                        clock_read.get(),
                        page_faults_delivered,
                        general_protection_faults_delivered,
                    );
                    observations.complete_rdtsc_clock_trace(
                        report.last_instruction_rip,
                        RdtscClockReadCompletion {
                            quantum_instructions_executed: report.instructions_executed,
                            retired_instructions_through_rdtsc: retired,
                        },
                    );
                    observations.complete_rdtsc_prefix(
                        prefix_ordinal,
                        RdtscClockReadCompletion {
                            quantum_instructions_executed: report.instructions_executed,
                            retired_instructions_through_rdtsc: retired,
                        },
                    );
                }
                let tick_ns = RDTSC_TICK_NS;
                #[cfg(test)]
                let tick_ns = if board.suppress_rdtsc_tick {
                    0
                } else {
                    tick_ns
                };
                if let Err(error) = charge_device(board, tick_ns) {
                    return ControlFlow::Break(Err(failure(error)));
                }
                if execution_observations
                    .as_ref()
                    .is_some_and(LinuxExecutionObservationCollector::wants_any_rdtsc_clock_trace)
                {
                    rdtsc_ns_charged = rdtsc_ns_charged.saturating_add(tick_ns);
                }
            }
            if let Some(observations) = execution_observations.as_mut() {
                observations
                    .complete_instruction_path_quantum(quanta, report.instructions_executed);
            }
            let stop_reason = acpi_pm.as_ref().and_then(AcpiPmHandle::stop_request);
            // S4-39g: a halted CPU (`HLT`) with IF = 1 and a source that can wake it: the clock
            // skips to the wake time (the board has nothing else to run), the CPU is `Running`
            // again, and the next boundary delivers the vector. The `HLT` itself retired (its
            // instruction and 1 ns are counted above). IF = 0, a source without a wake time
            // (`None`: the line is masked or in service, no edge) or a PM stop request leave the
            // exit as it was: the run ends as `halted`. Each wake costs one unit of the budget,
            // so a source that wakes the CPU without ever giving a vector cannot loop forever.
            let halted_with_budget = stop_reason.is_none()
                && remaining > 0
                && matches!(report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
            let wake_allowed = irq_hook
                && halted_with_budget
                && board.vcpus[BOOT_VCPU_INDEX].state.rflags & (1 << 9) != 0;
            if scheduler_budget.is_some()
                && halted_with_budget
                && ((wake_allowed
                    && (board.pit_ioapic_source.is_some() || board.irq_wake_ns().is_some()))
                    || (!schedule_standalone
                        && !board.state().nmi_blocked
                        && board.local_apic().pending_nmi()))
            {
                *can_resume_halt = true;
                previous_halt = Some((
                    board
                        .clock()
                        .now_ns()
                        .saturating_add(executed.retired)
                        .saturating_add(executed.device_ns),
                    board.state().nmi_blocked,
                ));
            }
            if scheduler_budget.is_none()
                && wake_allowed
                && let Some(wake) = board.idle_wake_ns()
            {
                let idle = match board.idle_to(wake) {
                    Ok(idle) => idle,
                    Err(error) => return ControlFlow::Break(Err(failure(error))),
                };
                irq_counters.halt_wakes += 1;
                irq_counters.halt_idle_ns += idle;
                if execution_observations
                    .as_ref()
                    .is_some_and(LinuxExecutionObservationCollector::wants_any_rdtsc_clock_trace)
                {
                    idle_ns_advanced = idle_ns_advanced.saturating_add(idle);
                }
                board.vcpus[BOOT_VCPU_INDEX].state.activity = Some(CpuActivity::Running);
                remaining -= 1;
                partial_rep = false;
                return ControlFlow::Continue(());
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
            ) || resumes_budget_cut(
                &report.exit,
                shortened,
                stop_reason.is_some(),
                remaining,
                report.instructions_executed,
            ) {
                return ControlFlow::Continue(());
            }
            let (exit, set_exit_detail) = match set_exit_to_probe_exit(report.exit) {
                Ok(exit) => exit,
                Err(error) => {
                    return ControlFlow::Break(Err(failure(error.into_board_error(board.state()))));
                }
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
            ControlFlow::Break(
                Ok(FirmwareRunReport {
                    exit,
                    page_fault: report.page_fault,
                    memory_error: report.memory_error,
                    retired,
                    quanta,
                    com1: board.com1_log.bytes.clone(),
                    virtual_ns: board.clock().now_ns(),
                    tsc_hz: board.clock().tsc_hz(),
                    rdtsc_reads: rdtsc_tick.as_ref().map(|_| rdtsc_reads),
                    rdtsc_reads_by_cpl: rdtsc_tick.as_ref().map(|_| rdtsc_reads_by_cpl),
                    pit_port_b_reads: pit_port_b_reads_before
                        .zip(pit.as_ref().map(Pit8254Handle::port_b_reads))
                        .map(|(before, after)| after.saturating_sub(before)),
                    pit_counter_port_reads: pit_counter_port_reads_before
                        .zip(pit.as_ref().map(Pit8254Handle::counter_port_reads))
                        .map(|(before, after)| after.saturating_sub(before)),
                    stop_reason,
                })
                .map(|run| (run, set_exit_detail, cpuid_divergence_leaf, user_entry)),
            )
        };
        // Without an explicit policy, a one-vCPU run is cut by its budget and device deadlines.
        let policy = self
            .linux_interleaved_policy
            .clone()
            .or(schedule_standalone.then_some(QuantumPolicy::Fixed(u64::MAX)));
        let result = if let Some(policy) = policy
            && instruction_budget != 0
            && self.vcpus[BOOT_VCPU_INDEX].activity() == CpuActivity::Running
        {
            let stop_request = StopRequest::new();
            let mut scheduler = SmpScheduler::new(1, policy, stop_request.clone())
                .expect("one vCPU and a valid Linux quantum policy");
            let mut outcome = None;
            let run = |board: &mut Self, _, budget, stop: &mut dyn FnMut() -> bool| {
                let mut executed = Executed::default();
                let mut can_resume_halt = false;
                match run_quantum(
                    board,
                    Some(budget),
                    stop,
                    &mut executed,
                    &mut can_resume_halt,
                ) {
                    ControlFlow::Continue(()) => outcome = None,
                    ControlFlow::Break(result) => {
                        outcome = Some(result);
                        if !can_resume_halt {
                            stop_request.request();
                        }
                    }
                }
                Ok(executed)
            };
            let host = if schedule_standalone {
                BoardSmpHost::for_standalone_linux(self, run)
            } else {
                BoardSmpHost::new(self, run)
            };
            let schedule = match host {
                Ok(mut host) => scheduler.run(&mut host, instruction_budget),
                Err(error) => Err(SmpError::Host(error)),
            };
            let failure = |error| LinuxRunFailure { error, retired };
            match schedule {
                Ok(_) => outcome.unwrap_or_else(|| {
                    Err(failure(FirmwareBoardError::Config(
                        "Linux scheduler ended without a quantum result",
                    )))
                }),
                Err(SmpError::Host(error)) => Err(failure(error)),
                Err(SmpError::Clock(error)) => Err(failure(owner_error(error))),
                Err(_) => Err(failure(FirmwareBoardError::Config(
                    "Linux scheduler refused a quantum",
                ))),
            }
            .map(|(mut run, detail, leaf, entry)| {
                run.virtual_ns = self.clock().now_ns();
                (run, detail, leaf, entry)
            })
        } else {
            loop {
                if let ControlFlow::Break(result) = run_quantum(
                    self,
                    None,
                    &mut || false,
                    &mut Executed::default(),
                    &mut false,
                ) {
                    break result;
                }
            }
        };
        if let Some(observations) = execution_observations.as_mut() {
            observations.finish_elf_copy_run(
                self.vcpus[BOOT_VCPU_INDEX].state.cr3.unwrap_or(0),
                page_faults_delivered,
                exceptions_delivered,
                changes.len() as u64,
                rdtsc_reads_by_cpl[0],
                retired,
            );
        }
        // The CPU state carries no CR3 outside this call, whatever way the loop ended.
        self.vcpus[BOOT_VCPU_INDEX].state.cr3 = None;
        // CR2 moves to the vCPU's `guest_cr2` cell (see the entry comment); the CPU state gets
        // back what it had.
        self.vcpus[BOOT_VCPU_INDEX].guest_cr2 = self.vcpus[BOOT_VCPU_INDEX].state.cr2;
        self.vcpus[BOOT_VCPU_INDEX].state.cr2 = prior_cr2;
        if let Some(observations) = execution_observations.as_mut() {
            observations.finish_instruction_path_trace();
        }
        let leaf_2m_tag = fetch_cache && self.fetch_cache_tag == FetchCacheTag::Leaf2M;
        self.fetch_cache_report = fetch_report.map(|accumulator| {
            let mut report = accumulator.finish();
            report.leaf_2m_tag = leaf_2m_tag;
            report
        });
        self.fetch_cache_sweep_report = fetch_sweep.map(|sweep| sweep.report());
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
            page_faults_delivered,
            general_protection_faults_delivered,
            execution_observations: execution_observations
                .map(LinuxExecutionObservationCollector::finish),
            irq: irq_counters,
        })
    }

    fn run_boot_with(
        &mut self,
        large_ram: bool,
        instruction_budget: u64,
        mut observe: impl FnMut(u64, u64, u64),
        mut between_quanta: impl FnMut(&Self, QuantumProgress),
    ) -> Result<FirmwareRunReport, LinuxRunFailure> {
        let firmware_boot_profile = !large_ram && !self.has_linux_profile();
        let mut remaining = instruction_budget;
        let mut retired = 0u64;
        let mut quanta = 0u64;
        self.com1_log.bytes.clear();
        let nvme = self.nvme.clone();
        let xhci = self.xhci.clone();
        let nic = self.nic.clone();
        let acpi_pm = self.acpi_pm.clone();
        let pit = self.pit.clone();
        let rdtsc_tick = self.rdtsc_tick.clone();
        let pit_port_b_reads_before = pit.as_ref().map(Pit8254Handle::port_b_reads);
        let pit_counter_port_reads_before = pit.as_ref().map(Pit8254Handle::counter_port_reads);
        let mut rdtsc_reads = 0u64;
        let mut rdtsc_reads_by_cpl = [0u64; 4];
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
        if let Some(xhci) = &xhci {
            xhci.service(&mut self.bus, |_| {});
        }
        if let Some(nic) = &nic {
            nic.service(&mut self.bus, |_| {});
        }
        loop {
            let uart = self.uart.clone();
            let agent = self.agent_channel.clone();
            let report = self.run_quantum(
                large_ram,
                firmware_boot_profile,
                remaining,
                || {
                    uart.holding_occupied()
                        || agent.as_ref().is_some_and(AgentChannel::holding_occupied)
                        || nvme.as_ref().is_some_and(NvmeHandle::doorbell_pending)
                        // A guest write to an xHCI register (a doorbell) ends the quantum.
                        || xhci.as_ref().is_some_and(XhciHandle::wakeup_pending)
                        // A guest write to a NIC register (the tail pointers) ends it as well.
                        || nic.as_ref().is_some_and(NicHandle::wakeup_pending)
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
                |rip, retired_in_quantum| observe(rip, retired + retired_in_quantum, quanta),
            );
            quanta += 1;
            // Doorbell writes only record queue pointers: execute the submitted
            // commands and post completions between quanta (NVME_DEVICE.md).
            if let Some(nvme) = &nvme {
                nvme.service(&mut self.bus, |_| {});
            }
            if let Some(xhci) = &xhci {
                xhci.service(&mut self.bus, |_| {});
            }
            if let Some(nic) = &nic {
                nic.service(&mut self.bus, |_| {});
            }
            retired += report.instructions_executed;
            remaining = remaining.saturating_sub(report.instructions_executed);
            if let Some(channel) = &self.agent_channel {
                channel.pump();
            }
            while uart.transfer_holding_to_shift() {
                if let Some(byte) = uart.complete_transmission() {
                    self.com1_log.push(byte);
                }
            }
            self.advance_time_ns(report.instructions_executed * FIRMWARE_EXECUTION_CYCLE_NS)
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
                rdtsc_reads = rdtsc_reads.saturating_add(1);
                let cpl = self
                    .state()
                    .cs_selector
                    .map_or(0, |selector| (selector & 3) as usize);
                rdtsc_reads_by_cpl[cpl] = rdtsc_reads_by_cpl[cpl].saturating_add(1);
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
                PagedProbeExit::Stopped if report.instructions_executed > 0 => {
                    between_quanta(self, QuantumProgress { retired, quanta });
                    continue;
                }
                other => other,
            };
            return Ok(FirmwareRunReport {
                exit,
                page_fault: report.page_fault,
                memory_error: report.memory_error,
                retired,
                quanta,
                com1: self.com1_log.bytes.clone(),
                virtual_ns: self.clock().now_ns(),
                tsc_hz: self.clock().tsc_hz(),
                rdtsc_reads: rdtsc_tick.as_ref().map(|_| rdtsc_reads),
                rdtsc_reads_by_cpl: rdtsc_tick.as_ref().map(|_| rdtsc_reads_by_cpl),
                pit_port_b_reads: pit_port_b_reads_before
                    .zip(pit.as_ref().map(Pit8254Handle::port_b_reads))
                    .map(|(before, after)| after.saturating_sub(before)),
                pit_counter_port_reads: pit_counter_port_reads_before
                    .zip(pit.as_ref().map(Pit8254Handle::counter_port_reads))
                    .map(|(before, after)| after.saturating_sub(before)),
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
    agent_channel: Option<AgentChannel>,
    display: DisplayHandle,
    display_mmio: Option<DisplayDevice>,
    nvme_mmio: Option<NvmeDevice>,
    nvme: Option<NvmeHandle>,
    xhci_mmio: Option<XhciDevice>,
    nic_mmio: Option<NicDevice>,
    ecam_mmio: Option<crate::pci_ecam::SharedPciEcam>,
    xhci: Option<XhciHandle>,
    nic: Option<NicHandle>,
    hid: Option<HidDevices>,
    nvram: Option<NvramHandle>,
    tpm: Option<TpmCrbHandle>,
    acpi_pm: Option<AcpiPmHandle>,
    pit: Option<Pit8254Handle>,
    pic_master: Option<Pic8259Handle>,
    lapic: LocalApicHandle,
    clock: VirtualClockHandle,
    /// See `LoadedFirmwareBoard::foreign_timer_clock`.
    foreign_timer_clock: bool,
    /// Handoff block bytes, to be written at [`FIRMWARE_HANDOFF_GPA`].
    handoff: Vec<u8>,
    /// Temporary identity tables, to be written at `page_table_root`.
    tables: Vec<u8>,
    page_table_bytes: u64,
    /// GPA of the PML4 and the `CR3` the paged interpreter uses.
    page_table_root: u64,
    rdtsc_tick: Option<Arc<AtomicBool>>,
}

struct FirmwareLayout {
    devices: Vec<DeviceEntry>,
    map: MachineMemoryMap,
    table_pages: u64,
    minor: u16,
    handoff: Vec<u8>,
    nvram: Option<NvramDevice>,
}

impl FirmwareLayout {
    fn for_config(
        config: &FirmwareBoardConfig,
        secure_boot_minor: u16,
    ) -> Result<Self, FirmwareBoardError> {
        validate_config(config)?;
        let devices = configured_board_devices(config);
        let map = board_map(config, &devices)?;
        let table_pages = boot_page_table_count(&map);
        let page_table_bytes = table_pages * PAGE;
        let low_end = config.ram_bytes.min(FIRMWARE_LOW_RAM_LIMIT);
        debug_assert_eq!(FIRMWARE_BOOT_PAGE_TABLES_GPA, SHADOW_END);
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
        let nvram = match config.nvram.as_ref() {
            None => None,
            Some(NvramConfig::Empty) => Some(NvramDevice::new()),
            Some(NvramConfig::Snapshot(bytes)) => {
                Some(NvramDevice::for_firmware_boot(bytes).map_err(FirmwareBoardError::Nvram)?)
            }
        };
        let minor = if nvram
            .as_ref()
            .is_some_and(|device| device.has_protected_store() && !device.secure_boot_disabled())
        {
            secure_boot_minor
        } else {
            handoff_minor(config, &devices)
        };
        let handoff = build_handoff(config, &devices, &regions, minor);
        Ok(Self {
            devices,
            map,
            table_pages,
            minor,
            handoff,
            nvram,
        })
    }
}

/// Serialize the board's handoff without a ROM. Use the baseline Secure Boot version
/// until a specific ROM advertises the optional PKCS#7 transport.
pub fn firmware_handoff_block(config: &FirmwareBoardConfig) -> Result<Vec<u8>, FirmwareBoardError> {
    Ok(FirmwareLayout::for_config(config, HANDOFF_MINOR_SECURE_BOOT)?.handoff)
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
    let layout = FirmwareLayout::for_config(config, HANDOFF_MINOR_SECURE_BOOT)?;
    if layout.minor == HANDOFF_MINOR_SECURE_BOOT {
        return Err(FirmwareBoardError::Nvram(
            NvramContentsError::ProtectionRequiresFirmware,
        ));
    }
    assemble_board_with_layout(config, pci, pit_clock, apic_timer_clock, layout)
}

fn assemble_board_with_layout(
    config: &FirmwareBoardConfig,
    mut pci: PciEcam,
    pit_clock: Option<VirtualClockHandle>,
    apic_timer_clock: Option<VirtualClockHandle>,
    layout: FirmwareLayout,
) -> Result<AssembledBoard, FirmwareBoardError> {
    let FirmwareLayout {
        devices,
        map,
        table_pages,
        handoff,
        nvram,
        ..
    } = layout;
    let foreign_timer_clock = pit_clock.is_some() || apic_timer_clock.is_some();
    let page_table_bytes = table_pages * PAGE;
    let page_table_root = FIRMWARE_BOOT_PAGE_TABLES_GPA;
    let tables = build_boot_tables(&map, page_table_root, table_pages)?;

    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    // One clock for the whole board: the PM timer, the PIT and (Linux profile) the
    // LAPIC timer observe it; the board advances it itself unless a `ClockOwner` takes it over
    // (`enable_clock_owner`, S4-23j), in which case the owner is the only advancer.
    let clock = VirtualClockHandle::new(config.tsc_hz).map_err(FirmwareBoardError::Clock)?;
    // Under `linux_profile` the LAPIC has a timer on the board clock (S4-37a4). The
    // timer only reads the clock; delivery of its IRQ is S4-38.
    let lapic = if config.linux_profile {
        LocalApic::with_shared_timers(
            apic_timer_clock.unwrap_or_else(|| clock.clone()),
            FIRMWARE_APIC_TIMER_HZ,
            config.vcpu_count,
        )
        // `validate_config` has accepted `vcpu_count`, so only the frequency can be refused.
        .map_err(|_| FirmwareBoardError::Config("apic timer needs a nonzero frequency"))?
    } else {
        LocalApic::with_cpus(config.vcpu_count).expect("validate_config checked vcpu_count")
    };
    let lapic_handle = lapic.handle();
    let ioapic = IoApic::new(lapic.handle());
    let (display_device, display) = if config.second_screen {
        DisplayDevice::new_two_screens_with_handle()
    } else {
        DisplayDevice::new_with_handle()
    };
    let display_mmio = config.display_enabled.then(|| display_device.clone());
    // Bus order: COM1, LAPIC, IOAPIC, optional display, then NVMe,
    // the optional PCI configuration device and the optional ACPI PM block, then the optional
    // COM2 agent channel (S11-21), the RTC (or the absent CMOS) and the 0x80..; the
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
    let nvram = if let Some(device) = nvram {
        let handle = device.handle();
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_NVRAM].base),
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        Some(handle)
    } else {
        None
    };
    let tpm = if let Some(initial) = &config.tpm {
        let device = TpmCrb::new(&initial.entropy, initial.snapshot.as_deref(), clock.clone())
            .map_err(FirmwareBoardError::Tpm)?;
        let handle = device.handle();
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_TPM].base),
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        Some(handle)
    } else {
        None
    };
    // The device exists on the bus only when the manifest entry is PRESENT.
    let mut nvme = None;
    let mut nvme_mmio = None;
    if let Some(nvme_config) = &config.nvme
        && devices[DEV_NVME_TABLE].present()
    {
        let (device, handle) = if config.nvme_pci {
            let (device, handle, function) =
                NvmeDevice::new_pci(nvme_config.clone(), lapic_handle.clone())
                    .map_err(FirmwareBoardError::Nvme)?;
            pci.add_function(0, NVME_PCI_SLOT, 0, Box::new(function))
                .map_err(|_| FirmwareBoardError::Config("NVMe PCI slot is occupied"))?;
            (device, handle)
        } else {
            let (device, handle) = NvmeDevice::new_with_handle(nvme_config.clone())
                .map_err(FirmwareBoardError::Nvme)?;
            if !config.linux_profile {
                nvme_mmio = Some(device.mmio_endpoint());
            }
            (device, handle)
        };
        let device_index = bus_devices.len();
        bus_devices.push(Box::new(device));
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(devices[DEV_NVME_TABLE].base),
            device_index,
        });
        nvme = Some(handle);
    }
    // The xHCI function (S8-42): PCI 00:05.0, BAR0 on the bus, the keyboard and the pointer
    // behind its ports; it reads the board clock for the port reset and MFINDEX.
    let mut xhci = None;
    let mut xhci_mmio = None;
    let mut hid = None;
    if let Some(xhci_config) = config.xhci {
        let (device, handle, function) = XhciDevice::new_pci(xhci_config, clock.clone());
        pci.add_function(0, XHCI_PCI_SLOT, 0, Box::new(function))
            .map_err(|_| FirmwareBoardError::Config("xHCI PCI slot is occupied"))?;
        hid = Some(handle.attach_hid_devices());
        xhci_mmio = Some(device.mmio_endpoint());
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(XHCI_PCI_BAR_BASE),
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        xhci = Some(handle);
    }
    // The NIC function (S11-32): PCI 00:06.0 with BAR0 on the bus; the backend is attached by the
    // host afterwards.
    let mut nic = None;
    let mut nic_mmio = None;
    if let Some(nic_config) = config.nic {
        let (device, handle, function) = NicDevice::new_pci(nic_config).map_err(|_| {
            FirmwareBoardError::Config("nic mac must be unicast, non-zero and not the gateway")
        })?;
        pci.add_function(0, NIC_PCI_SLOT, 0, Box::new(function))
            .map_err(|_| FirmwareBoardError::Config("NIC PCI slot is occupied"))?;
        nic_mmio = Some(device.mmio_endpoint());
        mmio.push(MmioBinding {
            region_start: GuestPhysicalAddress::new(NIC_PCI_BAR_BASE),
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        nic = Some(handle);
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
    // The agent channel: COM2 with the host queues (S11-21). Opt-in; without it `0x2F8` is unbound.
    let mut agent_channel = None;
    if config.agent_channel {
        let (device, channel) = AgentChannel::new();
        ports.push(PortRegionBinding {
            range: PortRange::new(
                PortAddress::new(UartPortSpec::COM2.base_port),
                UartPortSpec::PORT_BYTES,
            )
            .map_err(FirmwareBoardError::BusBuild)?,
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(device));
        agent_channel = Some(channel);
        // The COM2 line is connected to the IOAPIC input INTIN3; without the option INTIN3 stays
        // an input without a line, as it always was.
        IoApic::new(lapic_handle.clone())
            .handle()
            .connect_com2_line();
    }
    // A configured RTC is part of the firmware board as well as the Linux
    // profile. Without an explicit UTC base, Linux-profile boards retain the
    // S4-29 absent-port behavior; the ordinary firmware board leaves these
    // ports unmapped. Never invent a host date or epoch.
    if let Some(base_unix_seconds) = config.rtc_base_unix_seconds {
        let rtc = RtcCmos::new(clock.clone(), base_unix_seconds)
            .map_err(|_| FirmwareBoardError::Config("RTC base time is out of range"))?;
        ports.push(PortRegionBinding {
            range: PortRange::new(PortAddress::new(CMOS_INDEX_PORT), CMOS_PORT_BYTES)
                .map_err(FirmwareBoardError::BusBuild)?,
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(rtc));
    } else if config.linux_profile {
        ports.push(PortRegionBinding {
            range: PortRange::new(PortAddress::new(CMOS_INDEX_PORT), CMOS_PORT_BYTES)
                .map_err(FirmwareBoardError::BusBuild)?,
            device_index: bus_devices.len(),
        });
        bus_devices.push(Box::new(AbsentPorts::new(AbsentPortRegion::Cmos)));
    }
    // One PCI configuration device, two bindings (ECAM MMIO + conf1 PIO) with the
    // same device index, so both front ends share one function table.
    let mut ecam_mmio = None;
    if devices[DEV_ECAM_TABLE].present() {
        let device_index = bus_devices.len();
        let shared = crate::pci_ecam::SharedPciEcam::new(pci);
        bus_devices.push(Box::new(shared.clone()));
        ecam_mmio = Some(shared);
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
    // The PM timer reads the board's clock; the board advances it unless a `ClockOwner` took it.
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
        // COM3 and COM4 (and COM2 unless the agent channel owns it) are not declared by the board:
        // absent UARTs read 0xFF (S7-31j).
        for region in [
            AbsentPortRegion::IoDelay,
            AbsentPortRegion::Post,
            AbsentPortRegion::Uart2,
            AbsentPortRegion::Uart3,
            AbsentPortRegion::Uart4,
        ]
        .into_iter()
        .filter(|region| !(config.agent_channel && *region == AbsentPortRegion::Uart2))
        {
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
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(config.ram_bytes),
        bus_devices,
        mmio,
        ports,
    )
    .map_err(FirmwareBoardError::BusBuild)?;
    if config.nvme_pci {
        bus.enable_nvme_pci_mmio_window();
    }
    Ok(AssembledBoard {
        bus,
        uart,
        agent_channel,
        display,
        display_mmio,
        nvme_mmio,
        nvme,
        xhci_mmio,
        nic_mmio,
        ecam_mmio,
        xhci,
        nic,
        hid,
        nvram,
        tpm,
        acpi_pm,
        pit: pit_handle,
        pic_master: pic_master_handle,
        lapic: lapic_handle,
        clock,
        foreign_timer_clock,
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
            agent_channel,
            display,
            display_mmio,
            nvme_mmio,
            nvme,
            xhci_mmio,
            nic_mmio,
            ecam_mmio,
            xhci,
            nic,
            hid,
            nvram,
            tpm,
            acpi_pm,
            pit,
            pic_master,
            lapic,
            clock,
            foreign_timer_clock,
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
        let reset_source = ResetSource {
            tables,
            root: page_table_root,
            entry: state.clone(),
        };
        Ok(LoadedFirmwareBoard {
            bus,
            entry_state: Some(state.clone()),
            vcpus: vcpu_set(state, lapic.cpu_count()),
            uart,
            agent_channel,
            display,
            display_mmio,
            nvme_mmio,
            nvme,
            xhci_mmio,
            nic_mmio,
            ecam_mmio,
            xhci,
            nic,
            hid,
            nvram,
            tpm,
            acpi_pm,
            pit,
            lapic,
            clock,
            handoff,
            page_table_bytes,
            page_table_root,
            reset_source: Some(reset_source),
            pci_config_address: 0,
            pci_config_pending: false,
            com1_log: Com1Log::default(),
            console_context_parser: None,
            console_context_snapshots: Vec::new(),
            uart_output_context: None,
            rdtsc_tick,
            host_stop: None,
            host_stop_honoured: false,
            irq_source: Box::new(NoIrqSource),
            pic_master,
            pit_pic_source: None,
            uart_irq_source: None,
            uart_ioapic_source: None,
            pit_ioapic_source: None,
            clock_driver: BoardClock::Standalone,
            linux_interleaved_policy: None,
            checkpoints: None,
            fetch_cache_report_enabled: false,
            fetch_cache_report: None,
            fetch_cache_sweep_configs: None,
            fetch_cache_tag: FetchCacheTag::Page4K,
            decoded_instruction_cache: DecodedInstructionCache::default(),
            data_cache_prototype: false,
            fetch_cache_sweep_report: None,
            foreign_timer_clock,
            #[cfg(test)]
            suppress_pm_tick: false,
            #[cfg(test)]
            suppress_rdtsc_tick: false,
            instruction_batches: true,
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
            agent_channel,
            display,
            nvme,
            xhci,
            nic,
            hid,
            nvram,
            tpm,
            acpi_pm,
            pit,
            pic_master,
            lapic,
            clock,
            foreign_timer_clock,
            rdtsc_tick,
            ..
        } = self;
        configure_com1(&mut bus)?;
        let mut board = LoadedFirmwareBoard {
            bus,
            entry_state: None,
            vcpus: vcpu_set(LongModeState::default(), lapic.cpu_count()),
            uart,
            agent_channel,
            display,
            display_mmio: None,
            nvme_mmio: None,
            nvme,
            xhci_mmio: None,
            nic_mmio: None,
            ecam_mmio: None,
            xhci,
            nic,
            hid,
            nvram,
            tpm,
            acpi_pm,
            pit,
            lapic,
            clock,
            handoff: Vec::new(),
            page_table_bytes,
            page_table_root,
            reset_source: None,
            pci_config_address: 0,
            pci_config_pending: false,
            com1_log: Com1Log::default(),
            console_context_parser: None,
            console_context_snapshots: Vec::new(),
            uart_output_context: None,
            rdtsc_tick,
            host_stop: None,
            host_stop_honoured: false,
            irq_source: Box::new(NoIrqSource),
            pic_master,
            pit_pic_source: None,
            uart_irq_source: None,
            uart_ioapic_source: None,
            pit_ioapic_source: None,
            clock_driver: BoardClock::Standalone,
            linux_interleaved_policy: None,
            checkpoints: None,
            fetch_cache_report_enabled: false,
            fetch_cache_report: None,
            fetch_cache_sweep_configs: None,
            fetch_cache_tag: FetchCacheTag::Page4K,
            decoded_instruction_cache: DecodedInstructionCache::default(),
            data_cache_prototype: false,
            fetch_cache_sweep_report: None,
            foreign_timer_clock,
            #[cfg(test)]
            suppress_pm_tick: false,
            #[cfg(test)]
            suppress_rdtsc_tick: false,
            instruction_batches: true,
        };
        // S6-18a: the application processors of a Linux board read the CPUID of the target
        // profile in their legacy models (the kernel's `verify_cpu` needs it); one vCPU has none.
        board.set_cpuid_profile(LINUX_AP_CPUID_PROFILE);
        Ok(board)
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
    let entry_offset = validate_image(image, handoff_minor(config, &devices))?;
    if config.nvme_pci {
        return Err(FirmwareBoardError::Config(
            "nvme_pci needs Linux direct boot",
        ));
    }
    let advertised = le_u16(image, 0x2A)?;
    let secure_boot_minor = if advertised >= HANDOFF_MINOR_PKCS7 {
        HANDOFF_MINOR_PKCS7
    } else {
        HANDOFF_MINOR_SECURE_BOOT
    };
    let layout = FirmwareLayout::for_config(config, secure_boot_minor)?;
    if layout.minor >= HANDOFF_MINOR_SECURE_BOOT && advertised < layout.minor {
        return Err(FirmwareBoardError::Nvram(
            NvramContentsError::ProtectionRequiresFirmware,
        ));
    }
    assemble_board_with_layout(config, pci, pit_clock, apic_timer_clock, layout)?
        .load_firmware(image, entry_offset)
}

#[cfg(test)]
mod secure_boot_pcr7_tests;

#[cfg(test)]
mod tcg_log_replay_tests;

#[cfg(test)]
mod failure_smoke_tests;

#[cfg(test)]
mod tests {
    mod nic_board;
    mod xhci_board;
    use super::*;
    use crate::{
        ACPI_PM_TMR_BLK, ACPI_PM1_CNT_SCI_EN, ACPI_PM1_CNT_SLP_EN, ACPI_PM1A_CNT_BLK,
        ACPI_PM1A_EVT_BLK, ACPI_RESET_REG, ACPI_RESET_VALUE, ACPI_SLP_TYP_S5, DeviceError,
        PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES, Type0Config,
    };
    use od_isa::memory::ArchitecturalFault;

    #[test]
    #[ignore = "requires a locally built firmware ROM and TPM absence probe ESP"]
    fn firmware_boot_continues_after_tpm_startup_failure() {
        let rom =
            std::fs::read(std::env::var_os("OD_TPM_FAILURE_ROM").expect("OD_TPM_FAILURE_ROM"))
                .unwrap();
        let esp =
            std::fs::read(std::env::var_os("OD_TPM_FAILURE_ESP").expect("OD_TPM_FAILURE_ESP"))
                .unwrap();
        let setup = NvramDevice::with_secure_boot_keys(crate::SecureBootKeys::SetupMode)
            .unwrap()
            .nvram_snapshot();
        let config = FirmwareBoardConfig {
            acpi_pm: true,
            acpi_tables: true,
            nvme: Some(NvmeConfig::default()),
            nvram: Some(NvramConfig::Snapshot(setup)),
            tpm: Some(TpmConfig {
                entropy: vec![0x55; 48],
                snapshot: None,
            }),
            ..FirmwareBoardConfig::default()
        };
        let mut board = load_firmware_image(&rom, &config).unwrap();
        board.tpm.as_ref().unwrap().fail_startup();
        board
            .nvme()
            .unwrap()
            .attach_backend(Box::new(crate::InMemoryBackend::from_bytes(esp).unwrap()))
            .unwrap();
        let report = board.run_boot(20_000_000).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        let serial = String::from_utf8_lossy(&report.com1);
        assert_eq!(
            serial
                .matches("OD-FW TPM unavailable: TPM2_Startup returned TPM_RC_FAILURE\r\n")
                .count(),
            1,
            "{serial}"
        );
        assert!(
            serial.ends_with(concat!(
                "OD-UEFI TPM unavailable: no TCG2 protocol or final events\r\n",
                "OD-FW no bootable option\r\n",
                "OD-FW STOP code=0x21 detail=0x0000000000000001\r\n",
            )),
            "{serial}"
        );
        let acpi = board.read_acpi_reservations().unwrap();
        assert!(acpi.starts_with(b"RSD PTR "));
        assert!(acpi.windows(4).any(|bytes| bytes == b"FACP"));
        for hidden in [b"TPM2".as_slice(), b"TPM0", b"MSFT0101"] {
            assert!(!acpi.windows(hidden.len()).any(|bytes| bytes == hidden));
        }
        let crb = |offset| GuestPhysicalAddress::new(TPM_CRB_BASE + offset);
        let bus = board.bus_mut();
        assert_eq!(bus.read_physical(crb(0x0c), AccessWidth::Dword).unwrap(), 0);
        bus.write_physical(crb(0x08), AccessWidth::Dword, 1, |_| {})
            .unwrap();
        assert_eq!(bus.read_physical(crb(0x44), AccessWidth::Dword).unwrap(), 2);
        bus.write_physical(crb(0x08), AccessWidth::Dword, 2, |_| {})
            .unwrap();
        println!(
            "TPM absent after {} instructions, {} quanta",
            report.retired, report.quanta
        );
    }

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

    /// S6-32: one execution-mode switch, on the board; the board starts interleaved and the
    /// switch reaches the memory the vCPU adapters share.
    #[test]
    fn the_board_has_one_execution_mode_switch_and_starts_interleaved() {
        let mut board = load(&hlt_image());
        assert_eq!(board.execution_mode(), crate::ExecutionMode::Interleaved);
        assert_eq!(
            board.bus.physical_memory().execution_mode(),
            crate::ExecutionMode::Interleaved
        );
        board.set_execution_mode(crate::ExecutionMode::Parallel);
        assert_eq!(board.execution_mode(), crate::ExecutionMode::Parallel);
        assert_eq!(
            board.bus.physical_memory().execution_mode(),
            crate::ExecutionMode::Parallel
        );
        board.set_execution_mode(crate::ExecutionMode::Interleaved);
        assert_eq!(board.execution_mode(), crate::ExecutionMode::Interleaved);
    }

    #[test]
    fn board_has_exactly_one_vcpu_with_index_zero_and_state_is_read_through_it() {
        let mut board = load(&hlt_image());
        assert_eq!(board.vcpus.len(), 1);
        assert_eq!(board.vcpus[0].index, 0);
        assert_eq!(board.vcpus[0].guest_efer, FIRMWARE_EFER);
        assert_eq!(board.guest_efer(), FIRMWARE_EFER);
        assert_eq!(board.vcpus[0].guest_cr2, None);
        assert_eq!(board.vcpus[0].guest_fp, GuestFpState::initial());
        // The public accessors read and write element 0.
        assert!(std::ptr::eq(board.state(), &board.vcpus[0].state));
        board.set_guest_lstar(0x1234);
        assert_eq!(board.vcpus[0].guest_lstar, 0x1234);
        board.state_mut().rip = 0x55;
        assert_eq!(board.vcpus[0].state.rip, 0x55);
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
    fn boot_observer_counts_before_fetch_across_quantum_boundaries() {
        let image = out_byte_image(FIRMWARE_COM1_PORT, b'X');
        let mut board = load(&image);
        let entry = board.state().rip;
        let mut samples = Vec::new();
        let observed = board
            .run_boot_observed(4, |rip, retired, quanta| {
                samples.push((rip, retired, quanta))
            })
            .unwrap();
        assert_eq!(
            samples,
            [
                (entry, 0, 0),
                (entry + 2, 1, 0),
                (entry + 6, 2, 0),
                (entry + 7, 3, 1),
            ]
        );
        assert_eq!(observed.com1, b"X");
        assert_eq!(observed, load(&image).run_boot(4).unwrap());

        let mut board = load(&image);
        assert_eq!(
            board
                .run_boot_observed(0, |_, _, _| panic!("zero budget"))
                .unwrap()
                .retired,
            0
        );
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
    fn run_boot_selects_the_narrow_handoff_cpu_profile() {
        // MOV ECX,4; CALL rel32 to HLT, skipping the return-site NOP.
        let image = test_image(&[0xb9, 4, 0, 0, 0, 0xe8, 1, 0, 0, 0, 0x90, 0xf4], &[]);
        let mut board = load(&image);

        let diagnostic = board.run(1, || false, |_| {});
        assert_eq!(
            diagnostic.exit,
            PagedProbeExit::Unsupported { opcode: 0xb9 }
        );
        assert_eq!(diagnostic.instructions_executed, 0);

        let report = board.run_boot(3).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 3);
        assert_eq!(board.state().gpr[Register::Rcx as usize], 4);
        assert_eq!(board.state().rip, FIRMWARE_LINK_BASE + 0x4c);
        assert_eq!(
            board.state().gpr[Register::Rsp as usize],
            FIRMWARE_BOOT_STACK_TOP - 8
        );
    }

    fn firmware_cr3_test_image(root: u64, next: u8) -> Vec<u8> {
        let mut code = [0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0x0f, 0x22, 0xd8, next];
        code[2..10].copy_from_slice(&root.to_le_bytes());
        test_image(&code, &[])
    }

    #[test]
    fn firmware_mov_cr3_changes_the_next_fetch_in_whole_and_split_quanta() {
        const ROOT: u64 = 0x15_0000;
        const CODE_PAGE: u64 = ROOT + 0x4000;
        let image = firmware_cr3_test_image(ROOT, 0xcc);
        for split in [false, true] {
            let mut board = load(&image);
            for (address, value) in [
                (ROOT, (ROOT + 0x1000) | 3),
                (ROOT + 0x1000, (ROOT + 0x2000) | 3),
                (ROOT + 0x2000 + 8, (ROOT + 0x3000) | 3),
                (ROOT + 0x3000, CODE_PAGE | 3),
            ] {
                board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(address),
                        crate::AccessWidth::Qword,
                        value,
                        |_| {},
                    )
                    .unwrap();
            }
            board
                .bus_mut()
                .write_physical(
                    GuestPhysicalAddress::new(CODE_PAGE + 0x4d),
                    crate::AccessWidth::Byte,
                    0xf4,
                    |_| {},
                )
                .unwrap();

            if split {
                let first = board.run_boot(2).unwrap();
                assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
                assert_eq!(first.retired, 2);
                assert_eq!(board.state().rip, FIRMWARE_LINK_BASE + 0x4d);
                assert_eq!(board.cr3(), ROOT);
                assert_eq!(board.state().cr3, Some(ROOT));
            }
            let halted = board.run_boot(if split { 1 } else { 3 }).unwrap();
            assert_eq!(halted.exit, PagedProbeExit::Halted);
            assert_eq!(halted.retired, if split { 1 } else { 3 });
            assert_eq!(board.state().rip, FIRMWARE_LINK_BASE + 0x4e);
            assert_eq!(board.cr3(), ROOT);
            assert_eq!(board.state().cr3, Some(ROOT));
        }
    }

    #[test]
    fn firmware_mov_cr3_commits_only_a_valid_write_and_keeps_it_on_a_later_fault() {
        let mut board = load(&firmware_cr3_test_image(1 << 36, 0xf4));
        let boot_root = board.cr3();
        let invalid = board.run_boot(3).unwrap();
        assert_eq!(invalid.retired, 1);
        assert_eq!(board.state().rip, FIRMWARE_LINK_BASE + 0x4a);
        assert_eq!(
            invalid.exit,
            PagedProbeExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(board.state().cr3, None);
        assert_eq!(board.cr3(), boot_root);

        let mut board = load(&firmware_cr3_test_image(boot_root, 0xf4));
        assert_eq!(board.run_boot(3).unwrap().exit, PagedProbeExit::Halted);
        assert_eq!(board.state().cr3, Some(boot_root));

        const EMPTY_ROOT: u64 = 0x16_0000;
        let mut board = load(&firmware_cr3_test_image(EMPTY_ROOT, 0xf4));
        let faulted = board.run_boot(3).unwrap();
        assert_eq!(faulted.retired, 2);
        assert_eq!(board.state().rip, FIRMWARE_LINK_BASE + 0x4d);
        let expected = ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(FIRMWARE_LINK_BASE + 0x4d),
        };
        assert_eq!(faulted.exit, PagedProbeExit::Fault(expected.clone()));
        let page_fault = faulted.page_fault.expect("tracked fetch fault");
        assert_eq!(page_fault.fault(), &expected);
        assert_eq!(page_fault.access(), od_isa::AccessKind::Fetch);
        assert_eq!(board.state().cr3, Some(EMPTY_ROOT));
        assert_eq!(board.cr3(), EMPTY_ROOT);
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
    fn linux_nvme_msix_masks_completions_and_routes_the_io_vector_to_the_ap() {
        const ASQ: u64 = 0x10_0000;
        const ACQ: u64 = 0x11_0000;
        const IO_SQ: u64 = 0x12_0000;
        const IO_CQ: u64 = 0x13_0000;
        const MSE: u64 = 1 << 1;
        const BME: u64 = 1 << 2;
        const ADMIN_VECTOR: u8 = 0x59;
        const IO_VECTOR: u8 = 0x5a;
        let config = FirmwareBoardConfig {
            linux_profile: true,
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            nvme: Some(NvmeConfig::default()),
            nvme_pci: true,
            vcpu_count: 2,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None).unwrap();
        let bsp = board.lapic.clone();
        let ap = bsp.for_cpu(1).unwrap();
        let nvme = board.nvme.as_ref().unwrap().clone();
        let bus = &mut board.bus;
        let cfg = |reg| {
            GuestPhysicalAddress::new(
                PCI_ECAM_DESCRIPTOR.base + (u64::from(NVME_PCI_SLOT) << 15) + reg,
            )
        };
        let bar = |off| GuestPhysicalAddress::new(NVME_PCI_BAR_BASE + off);
        let read = |bus: &mut MachineBus, addr, width| bus.read_physical(addr, width).unwrap();
        let write = |bus: &mut MachineBus, addr, width, value| {
            bus.write_physical(addr, width, value, |_| {}).unwrap();
        };
        let dword = AccessWidth::Dword;
        let qword = AccessWidth::Qword;
        assert_ne!(read(bus, cfg(4), dword) & (1 << 20), 0);
        assert_eq!(read(bus, cfg(0x34), AccessWidth::Byte), 0x50);
        assert_eq!(read(bus, cfg(0x50), dword), 0x0004_0011);
        assert_eq!(read(bus, cfg(0x54), dword), 0x2000);
        assert_eq!(read(bus, cfg(0x58), dword), 0x3000);
        write(bus, cfg(0x54), dword, 0x1000);
        assert_eq!(read(bus, cfg(0x54), dword), 0x2000);

        write(bus, cfg(4), dword, MSE | BME);
        write(bus, cfg(0x53), AccessWidth::Byte, 0xc0);
        assert_eq!(read(bus, cfg(0x52), AccessWidth::Word), 0xc004);
        for (offset, destination, vector) in [(0x2000, 0, ADMIN_VECTOR), (0x2010, 1, IO_VECTOR)] {
            assert_eq!(read(bus, bar(offset + 12), dword), 1);
            write(bus, bar(offset), qword, 0xfee0_0000 | (destination << 12));
            write(bus, bar(offset + 8), dword, u64::from(vector));
        }
        write(bus, bar(0x200c), dword, 0xffff_fffe);
        assert_eq!(read(bus, bar(0x200c), dword), 0);
        write(bus, bar(0x24), dword, 0x000f_000f);
        write(bus, bar(0x28), qword, ASQ);
        write(bus, bar(0x30), qword, ACQ);
        write(bus, bar(0x14), dword, 1 | (6 << 16) | (4 << 20));
        let put_command =
            |bus: &mut MachineBus, address, opcode, prp1: u64, cdw10: u32, cdw11: u32| {
                let mut command = [0; 64];
                command[0] = opcode;
                command[24..32].copy_from_slice(&prp1.to_le_bytes());
                command[40..44].copy_from_slice(&cdw10.to_le_bytes());
                command[44..48].copy_from_slice(&cdw11.to_le_bytes());
                bus.write_flat_bytes(GuestPhysicalAddress::new(address), &command, |_| {})
                    .unwrap();
            };
        put_command(bus, ASQ, 0x05, IO_CQ, 1 | (3 << 16), 3 | (1 << 16));
        put_command(bus, ASQ + 64, 0x01, IO_SQ, 1 | (3 << 16), 1 | (1 << 16));
        put_command(
            bus,
            ASQ + 128,
            0x05,
            IO_CQ + 4096,
            2 | (3 << 16),
            3 | (5 << 16),
        );
        put_command(bus, ASQ + 192, 0x09, 0, 9, (1 << 16) | 1);
        put_command(bus, ASQ + 256, 0x0a, 0, 9, 1);
        put_command(bus, ASQ + 320, 0x09, 0, 9, (1 << 16) | 4);
        write(bus, bar(0x1000), dword, 6);
        assert_eq!(nvme.service(bus, |_| {}).completions, 6);
        // Failed commands report More because they generate Error Information entries.
        let status = GuestPhysicalAddress::new(ACQ + 2 * 16 + 14);
        assert_eq!(read(bus, status, AccessWidth::Word) >> 1, 0x6108);
        let features = GuestPhysicalAddress::new(ACQ + 4 * 16);
        assert_eq!(read(bus, features, dword), 0x0001_0001);
        let status = GuestPhysicalAddress::new(ACQ + 5 * 16 + 14);
        assert_eq!(read(bus, status, AccessWidth::Word) >> 1, 0x6002);
        assert_eq!(read(bus, cfg(4), dword) & (1 << 19), 0, "INTx suppressed");
        assert_eq!(
            read(bus, bar(0x3000), qword),
            1,
            "function mask latches admin events"
        );
        assert!(!bsp.vector_state(ADMIN_VECTOR).0);

        write(bus, cfg(4), dword, MSE);
        write(bus, cfg(0x52), AccessWidth::Word, 0x8000);
        assert_eq!(
            read(bus, bar(0x3000), qword),
            1,
            "BME clear blocks MSI-X delivery"
        );
        write(bus, cfg(4), dword, MSE | BME);
        assert_eq!(read(bus, bar(0x3000), qword), 0);
        assert!(bsp.vector_state(ADMIN_VECTOR).0);
        assert!(!ap.vector_state(ADMIN_VECTOR).0);

        put_command(bus, IO_SQ, 0xff, 0, 0, 0);
        write(bus, bar(0x1008), dword, 1);
        assert_eq!(nvme.service(bus, |_| {}).completions, 1);
        assert_eq!(
            read(bus, bar(0x3000), qword),
            2,
            "vector mask latches I/O events"
        );
        assert!(!ap.vector_state(IO_VECTOR).0);
        write(bus, bar(0x100c), dword, 1);
        write(bus, bar(0x3000), qword, 0);
        assert_eq!(
            read(bus, bar(0x3000), qword),
            2,
            "read-only PBA is independent of CQ head"
        );
        ap.set_global_enabled(false).unwrap();
        write(bus, bar(0x201c), dword, 0);
        assert_eq!(
            read(bus, bar(0x3000), qword),
            2,
            "a refused LAPIC edge remains pending"
        );
        ap.set_global_enabled(true).unwrap();
        assert!(!nvme.sample_pci_interrupts());
        assert_eq!(read(bus, bar(0x3000), qword), 0);
        assert!(ap.vector_state(IO_VECTOR).0);
        assert!(!bsp.vector_state(IO_VECTOR).0);

        write(bus, bar(0x201c), dword, 1);
        write(bus, bar(0x2018), dword, u64::from(IO_VECTOR + 1));
        write(bus, bar(0x201c), dword, 0);
        put_command(bus, IO_SQ + 64, 0xff, 0, 0, 0);
        write(bus, bar(0x1008), dword, 2);
        assert_eq!(nvme.service(bus, |_| {}).completions, 1);
        assert!(
            ap.vector_state(IO_VECTOR + 1).0,
            "unmasked CQE routes without BSP polling"
        );
        assert_eq!(read(bus, bar(0x3000), qword), 0);
        write(bus, bar(0x100c), dword, 2);

        write(bus, cfg(0x53), AccessWidth::Byte, 0);
        assert_ne!(
            read(bus, cfg(4), dword) & (1 << 19),
            0,
            "unacknowledged admin CQ resumes INTx"
        );
        write(bus, bar(0x1004), dword, 6);
        assert_eq!(read(bus, cfg(4), dword) & (1 << 19), 0);
    }

    #[test]
    fn linux_nvme_pci_bar_decode_bus_master_and_intx_follow_guest_register_writes() {
        use crate::bus::SetStepMmio;
        use crate::{AccessWidth, GuestPhysicalAddress};
        const ASQ: u64 = 0x10_0000;
        const ACQ: u64 = 0x11_0000;
        const MSE: u64 = 1 << 1;
        const BME: u64 = 1 << 2;
        const PCI_INTERRUPT_STATUS: u64 = 1 << 19;
        const VECTOR: u8 = 0x51;
        let local = |off| GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + off);
        let io = |off| GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + off);
        let config = FirmwareBoardConfig {
            linux_profile: true,
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            nvme: Some(NvmeConfig::default()),
            nvme_pci: true,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None).unwrap();
        assert_eq!(le_u16(board.handoff_block(), 0x0a).unwrap(), 3);
        // Generated by the independent firmware ACPI builder for this board.
        assert_eq!(
            hex(&config.acpi_manifest_hash().unwrap()),
            "94ddada367e48ff19c2b79879797829214b533fa97e4ba20a8128c222e6c615a",
        );
        let apic = board.lapic.clone();
        let bus = &mut board.bus;
        let cfg = |reg| {
            GuestPhysicalAddress::new(
                PCI_ECAM_DESCRIPTOR.base + (u64::from(NVME_PCI_SLOT) << 15) + reg,
            )
        };
        let bar = |off| GuestPhysicalAddress::new(NVME_PCI_BAR_BASE + off);
        let read =
            |bus: &mut MachineBus, addr| bus.read_physical(addr, AccessWidth::Dword).unwrap();
        let write = |bus: &mut MachineBus, addr, value| {
            bus.write_physical(addr, AccessWidth::Dword, value, |_| {})
                .unwrap();
        };
        assert_eq!(read(bus, cfg(8)) >> 8, 0x01_08_02);
        assert_eq!(read(bus, cfg(0x3c)) & 0xff00, 0x100);
        assert_eq!(read(bus, cfg(0x10)), NVME_PCI_BAR_BASE | 4);
        assert_eq!(read(bus, bar(0)), u64::from(u32::MAX));
        write(bus, bar(0x14), 1);

        // Probe both halves of the BAR and restore it as firmware would.
        write(bus, cfg(0x10), u64::from(u32::MAX));
        write(bus, cfg(0x14), u64::from(u32::MAX));
        assert_eq!(read(bus, cfg(0x10)), 0xffff_c004);
        assert_eq!(read(bus, cfg(0x14)), u64::from(u32::MAX));
        write(bus, cfg(4), MSE);
        assert_eq!(read(bus, bar(0)), u64::from(u32::MAX));
        write(bus, cfg(0x10), NVME_PCI_BAR_BASE);

        // ECAM and conf1 program the same address, including the high half.
        let conf1_bar_high = (1 << 31) | (u32::from(NVME_PCI_SLOT) << 11) | 0x14;
        bus.write_port(
            PortAddress::new(PCI_CONF1_BASE_PORT),
            AccessWidth::Dword,
            conf1_bar_high,
        )
        .unwrap();
        bus.write_port(
            PortAddress::new(PCI_CONF1_BASE_PORT + 4),
            AccessWidth::Dword,
            0,
        )
        .unwrap();
        assert_eq!(read(bus, bar(0)) & 0xffff, 1023);
        assert_eq!(
            read(bus, bar(0x14)),
            0,
            "the disabled CC write was discarded"
        );
        let (_, set_step_io) = bus.split_for_set_step();
        assert!(
            set_step_io
                .windows()
                .contains(&(NVME_PCI_BAR_BASE, NVME_PCI_BAR_BYTES))
        );

        write(bus, bar(0x24), 0x000f_000f);
        bus.write_physical(bar(0x28), AccessWidth::Qword, ASQ, |_| {})
            .unwrap();
        bus.write_physical(bar(0x30), AccessWidth::Qword, ACQ, |_| {})
            .unwrap();
        write(bus, bar(0x14), 1 | (6 << 16) | (4 << 20));
        let mut get_features = [0; 64];
        get_features[0] = 0x0a; // Get Features.
        get_features[40] = 0x07; // Number of Queues.
        bus.write_flat_bytes(GuestPhysicalAddress::new(ASQ), &get_features, |_| {})
            .unwrap();
        write(bus, bar(0x1000), 1);
        let nvme = board.nvme.as_ref().unwrap();

        assert!(!nvme.doorbell_pending());
        assert_eq!(
            nvme.service(bus, |_| {}),
            crate::NvmeServiceReport::default()
        );
        assert_eq!(read(bus, cfg(4)) & PCI_INTERRUPT_STATUS, 0);

        let source = NvmeIoapicIrqSource::new(
            Box::new(NoIrqSource),
            nvme.clone(),
            IoApic::new(apic.clone()).handle(),
        );
        write(bus, local(0xf0), 0x1ff);
        write(bus, io(0), (0x10 + 2 * crate::apic_pair::NVME_PIN) as u64);
        write(
            bus,
            io(0x10),
            u64::from(crate::apic_pair::SCI_LEVEL_LOW) | u64::from(VECTOR),
        );

        write(
            bus,
            cfg(4),
            MSE | BME | u64::from(crate::pci_ecam::COMMAND_INTX_DISABLE),
        );
        assert!(
            nvme.doorbell_pending(),
            "disabled DMA must retain queued work"
        );
        assert_eq!(nvme.service(bus, |_| {}).completions, 1);
        assert_eq!(
            read(bus, cfg(4)) & PCI_INTERRUPT_STATUS,
            PCI_INTERRUPT_STATUS
        );
        source.pending_vector();
        assert_eq!(apic.pending(), None, "PCI Command disables the wire");

        write(bus, bar(0x0c), 1); // INTMS masks vector 0.
        assert_eq!(read(bus, cfg(4)) & PCI_INTERRUPT_STATUS, 0);
        write(bus, cfg(4), MSE | BME);
        source.pending_vector();
        assert_eq!(apic.pending(), None, "INTMS masks the interrupt");

        write(bus, bar(0x10), 1); // INTMC unmasks it while a CQE is still pending.
        assert_eq!(
            read(bus, cfg(4)) & PCI_INTERRUPT_STATUS,
            PCI_INTERRUPT_STATUS
        );
        source.pending_vector();
        assert_eq!(apic.acknowledge(), Some(VECTOR));
        assert_ne!(
            read(bus, io(0x10)) & u64::from(crate::apic_pair::REMOTE_IRR),
            0
        );
        write(bus, local(0xb0), 0); // EOI with CQE still pending.
        assert_eq!(
            apic.acknowledge(),
            Some(VECTOR),
            "level re-delivered while asserted"
        );

        write(bus, bar(0x1004), 1); // CQ head acknowledges the completion.
        assert_eq!(read(bus, cfg(4)) & PCI_INTERRUPT_STATUS, 0);
        source.pending_vector();
        write(bus, local(0xb0), 0);
        assert_eq!(
            read(bus, io(0x10)) & u64::from(crate::apic_pair::REMOTE_IRR),
            0
        );
        assert_eq!(apic.pending(), None);

        // Identify reports the same vendor IDs as Function 0's Type-0 header.
        const IDENTIFY_DATA: u64 = 0x12_0000;
        let mut identify = [0u8; 64];
        identify[0] = 0x06; // Identify.
        identify[24..32].copy_from_slice(&IDENTIFY_DATA.to_le_bytes());
        identify[40] = 0x01; // Identify Controller.
        bus.write_flat_bytes(GuestPhysicalAddress::new(ASQ + 64), &identify, |_| {})
            .unwrap();
        write(bus, bar(0x1000), 2);
        assert_eq!(nvme.service(bus, |_| {}).completions, 1);
        assert_eq!(read(bus, GuestPhysicalAddress::new(ACQ + 16 + 12)) >> 17, 0);
        let vendor = read(bus, cfg(0x00)) & 0xFFFF;
        let subsystem_vendor = read(bus, cfg(0x2c)) & 0xFFFF;
        assert_ne!(vendor, 0);
        assert_eq!(
            read(bus, GuestPhysicalAddress::new(IDENTIFY_DATA)),
            vendor | (subsystem_vendor << 16)
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

    // ---- S4-21e: restart of the processors after a stop ----

    fn two_vcpu_pm_config() -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            vcpu_count: 2,
            ..pm_config()
        }
    }

    #[test]
    fn restart_after_a_guest_reset_returns_the_processors_to_the_entry_state() {
        let image = out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE);
        let mut board = load_firmware_image(&image, &two_vcpu_pm_config()).unwrap();
        let entry = board.state().clone();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
        assert_ne!(*board.state(), entry, "the run moved the boot processor");
        // Events the old run left for the processors: a fixed interrupt and an NMI in the
        // boot processor's Local APIC, an INIT for the application processor.
        board.local_apic().request_fixed_edge(0x41).unwrap();
        let ap_apic = board.local_apic_of(1).unwrap();
        ap_apic.write_icr(1, 0, 4 << 8).unwrap();
        assert!(board.local_apic().pending_nmi());
        board
            .local_apic()
            .write_icr(0, 1, (5 << 8) | (3 << 14))
            .unwrap();
        let ram = board.ram_sha256().unwrap();
        let clock = board.clock();
        assert_eq!(
            board.acpi_pm().unwrap().stop_request(),
            Some(MachineStopReason::Reset)
        );

        assert_eq!(
            board.restart_vcpus(),
            Ok(Some(MachineStopReason::Reset)),
            "the acknowledged stop is the reset"
        );

        assert_eq!(*board.state(), entry);
        let boot = board.vcpu(0).unwrap();
        assert_eq!(boot.guest_efer, FIRMWARE_EFER);
        assert_eq!(boot.guest_cr2, None);
        assert!(boot.legacy.is_none());
        let ap = board.vcpu(1).unwrap();
        assert_eq!(ap.activity(), CpuActivity::WaitForSipi);
        assert!(ap.real_mode().is_some());
        // No old pending event survives, for any vCPU, and the stop is acknowledged.
        for index in 0..2 {
            let apic = board.local_apic_of(index).unwrap();
            assert_eq!(apic.pending(), None, "vCPU {index}");
            assert!(!apic.pending_nmi(), "vCPU {index}");
            assert_eq!(apic.take_startup().unwrap(), Default::default());
        }
        assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
        // Neither RAM nor virtual time moved.
        assert_eq!(board.ram_sha256().unwrap(), ram);
        assert_eq!(board.clock(), clock);

        // The restarted processors run the entry code again on the RAM the guest left.
        let again = board.run_boot(100).unwrap();
        assert_eq!(again.exit, PagedProbeExit::Stopped);
        assert_eq!(again.stop_reason, Some(MachineStopReason::Reset));
        assert_eq!(again.retired, 3);
    }

    #[test]
    fn restart_acknowledges_a_shutdown_once_and_any_other_end_of_a_run_returns_none() {
        let slp = ((u16::from(ACPI_SLP_TYP_S5) << 10) | ACPI_PM1_CNT_SLP_EN) >> 8;
        let image = out_byte_image(ACPI_PM1A_CNT_BLK + 1, slp as u8);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        assert_eq!(
            board.run_boot(100).unwrap().stop_reason,
            Some(MachineStopReason::Shutdown)
        );
        assert_eq!(board.restart_vcpus(), Ok(Some(MachineStopReason::Shutdown)));
        // The stop was taken: a second restart has nothing to acknowledge.
        assert_eq!(board.restart_vcpus(), Ok(None));

        // A halted guest (no stop request) is restartable too and runs again from the entry.
        let mut halted = load_firmware_image(&hlt_image(), &pm_config()).unwrap();
        let entry = halted.state().clone();
        let report = halted.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(halted.state().activity, Some(CpuActivity::Halted));
        assert_eq!(halted.restart_vcpus(), Ok(None));
        assert_eq!(*halted.state(), entry);
        assert_eq!(halted.state().activity, Some(CpuActivity::Running));
        assert_eq!(halted.run_boot(100).unwrap().exit, PagedProbeExit::Halted);

        // Without the PM block there is nothing to acknowledge either.
        let mut plain = load(&hlt_image());
        plain.run_boot(100).unwrap();
        assert_eq!(plain.restart_vcpus(), Ok(None));
    }

    #[test]
    fn restart_leaves_the_host_stop_flag_to_the_host() {
        let image = out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        board.set_host_stop(Arc::clone(&flag));
        board.run_boot(100).unwrap();
        flag.store(true, Ordering::Relaxed);
        board.restart_vcpus().unwrap();
        assert!(
            flag.load(Ordering::Relaxed),
            "the host's STOP request stays"
        );
    }

    #[test]
    fn a_stop_on_the_last_budgeted_instruction_is_not_a_budget_exhaustion() {
        let image = out_byte_image(ACPI_RESET_REG, ACPI_RESET_VALUE);
        let mut board = load_firmware_image(&image, &pm_config()).unwrap();
        let report = board.run_boot(3).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
    }

    /// A board that ran `MOV AL,imm8; MOV DX,imm16; OUT DX,AL; HLT` on the reset register to its
    /// reset stop.
    fn reset_stopped_board(config: FirmwareBoardConfig) -> LoadedFirmwareBoard {
        let [lo, hi] = ACPI_RESET_REG.to_le_bytes();
        let code = [0xb0, ACPI_RESET_VALUE, 0x66, 0xba, lo, hi, 0xee, 0xf4];
        // `handoff_minor_max` 13: the image takes the TPM handoff of the device test.
        let image = test_image(&code, &[(0x2A, &13u16.to_le_bytes())]);
        let mut board = load_firmware_image(&image, &config).unwrap();
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
        board
    }

    fn ram_at(board: &mut LoadedFirmwareBoard, address: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(address), &mut bytes)
            .unwrap();
        bytes
    }

    fn put_ram(board: &mut LoadedFirmwareBoard, address: u64, bytes: &[u8]) {
        board
            .bus_mut()
            .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
            .unwrap();
    }

    fn mmio32(board: &mut LoadedFirmwareBoard, address: u64) -> u64 {
        board
            .bus_mut()
            .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Dword)
            .unwrap()
    }

    fn put_mmio32(board: &mut LoadedFirmwareBoard, address: u64, value: u64) {
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Dword,
                value,
                |_| {},
            )
            .unwrap();
    }

    #[test]
    fn platform_reset_runs_the_same_firmware_again_on_the_same_board() {
        let mut board = reset_stopped_board(pm_config());
        for cycle in 1..=3u64 {
            board.platform_reset(PlatformResetKind::Cold).unwrap();
            assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
            let report = board.run_boot(100).unwrap();
            assert_eq!(report.exit, PagedProbeExit::Stopped);
            assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
            assert_eq!(report.retired, 3, "the same instructions every cycle");
            // The virtual clock is not reset: three nanoseconds per run, cumulative.
            assert_eq!(report.virtual_ns, 3 * (cycle + 1));
        }
    }

    #[test]
    fn platform_reset_empties_the_decoded_instruction_cache() {
        for kind in [PlatformResetKind::Cold, PlatformResetKind::Warm] {
            let mut board = reset_stopped_board(pm_config());
            let cache = board.decoded_instruction_cache();
            let (rip, ring0, probe, _) =
                crate::decoded_instruction_cache::test_support::insert_probe(cache);
            assert!(cache.entries() > 0);
            board.platform_reset(kind).unwrap();
            let cache = board.decoded_instruction_cache();
            assert_eq!(cache.entries(), 0, "{kind:?}: the table is empty");
            assert_eq!(
                cache.lookup(rip, ring0, probe),
                None,
                "{kind:?}: probe gone"
            );
        }
    }

    #[test]
    fn cold_reset_zeroes_ram_warm_reset_keeps_it_and_both_restore_the_entry_contract() {
        for kind in [PlatformResetKind::Cold, PlatformResetKind::Warm] {
            let mut board = reset_stopped_board(pm_config());
            // Markers in low RAM and far above the tables, garbage over the shadow, the handoff
            // block and the boot page tables, the way a running firmware leaves them.
            let markers = [0x2_0000u64, 0x0100_0000];
            for at in markers {
                put_ram(&mut board, at, &[0xA5; 64]);
            }
            let root = board.page_table_root;
            for at in [
                FIRMWARE_LINK_BASE + 0x100,
                FIRMWARE_HANDOFF_GPA + 8,
                root + 8,
            ] {
                put_ram(&mut board, at, &[0xCC; 8]);
            }
            // The firmware's `MOV CR3` moved the walk root to its own tables.
            board.page_table_root = 0x0015_0000;
            board.vcpus[0].state.cr3 = Some(0x0015_0000);
            board.vcpus[0].state.rip = 0x1234;
            board.vcpus[0].state.gpr[Register::Rsp as usize] = 0x5678;

            board.platform_reset(kind).unwrap();

            let expected = if kind == PlatformResetKind::Cold {
                0
            } else {
                0xA5
            };
            for at in markers {
                assert_eq!(ram_at(&mut board, at, 64), vec![expected; 64], "{kind:?}");
            }
            let mut rom = vec![0; FIRMWARE_IMAGE_BYTES];
            board
                .bus_mut()
                .read_flat_bytes(GuestPhysicalAddress::new(FIRMWARE_ROM_BASE), &mut rom)
                .unwrap();
            assert_eq!(ram_at(&mut board, FIRMWARE_LINK_BASE, rom.len()), rom);
            let handoff = board.handoff_block().to_vec();
            assert_eq!(
                ram_at(&mut board, FIRMWARE_HANDOFF_GPA, handoff.len()),
                handoff
            );
            let tables = board.reset_source.as_ref().unwrap().tables.clone();
            assert_eq!(ram_at(&mut board, root, tables.len()), tables);
            assert_eq!(
                board.page_table_root, root,
                "{kind:?}: the walk root is the boot one"
            );
            let state = board.state();
            assert_eq!(state.rip, FIRMWARE_LINK_BASE + 0x40);
            assert_eq!(state.gpr[Register::Rsp as usize], FIRMWARE_BOOT_STACK_TOP);
            assert_eq!(state.gpr[Register::Rdi as usize], FIRMWARE_HANDOFF_GPA);
            // No CR3 of the vCPU's own: the interpreter walks from the boot tables again (the
            // `#PF` of the first Warm run came from a stale root).
            assert_eq!(state.cr3, None, "{kind:?}");
            assert_eq!(board.guest_efer(), FIRMWARE_EFER);
        }
    }

    #[test]
    fn platform_reset_takes_the_devices_to_power_on_and_keeps_the_clock() {
        let config = FirmwareBoardConfig {
            acpi_pm: true,
            acpi_tables: true,
            nvram: Some(NvramConfig::Empty),
            nvme: Some(NvmeConfig::default()),
            tpm: Some(TpmConfig {
                entropy: vec![0x55; 48],
                snapshot: None,
            }),
            ..reference_config()
        };
        let mut board = reset_stopped_board(config);
        // PM block: an enabled event.
        let pm_en = ACPI_PM1A_EVT_BLK + 2;
        board
            .bus_mut()
            .write_port(io_port(pm_en), AccessWidth::Word, 1)
            .unwrap();
        // COM1: a byte in the holding register.
        board
            .bus_mut()
            .write_port(io_port(FIRMWARE_COM1_PORT), AccessWidth::Byte, 0x41)
            .unwrap();
        assert!(board.uart.holding_occupied());
        // NVMe: the admin queue size the driver programmed.
        put_mmio32(&mut board, NVME_MMIO_BASE + 0x24, 0x001f_001f);
        assert_eq!(mmio32(&mut board, NVME_MMIO_BASE + 0x24), 0x001f_001f);
        // NVRAM: a transfer address, and a committed bank that must survive.
        let nvram =
            crate::nvram_device::NVRAM_MMIO_BASE + crate::nvram_device::NVRAM_STORE_BYTES as u64;
        put_mmio32(&mut board, nvram, 0x1234);
        let committed = board.nvram_snapshot().unwrap();
        // TPM CRB: a granted locality.
        let crb_loc_ctrl = TPM_CRB_BASE + 0x08;
        let crb_loc_sts = TPM_CRB_BASE + 0x0c;
        put_mmio32(&mut board, crb_loc_ctrl, 1);
        assert_eq!(mmio32(&mut board, crb_loc_sts), 1);
        let before = board.clock().now_ns();
        assert!(before > 0);

        board.platform_reset(PlatformResetKind::Cold).unwrap();

        assert_eq!(board.acpi_pm().unwrap().stop_request(), None);
        let pm_en_value = board
            .bus_mut()
            .read_port(io_port(pm_en), AccessWidth::Word)
            .unwrap();
        assert_eq!(pm_en_value, 0);
        assert!(!board.uart.holding_occupied());
        assert_eq!(mmio32(&mut board, NVME_MMIO_BASE + 0x24), 0);
        assert_eq!(mmio32(&mut board, nvram), 0);
        assert_eq!(board.nvram_snapshot().unwrap(), committed);
        assert_eq!(mmio32(&mut board, crb_loc_sts), 0);
        assert_eq!(board.clock().now_ns(), before, "time does not move");
        // COM1 is configured again as at load, and the firmware runs to its reset again.
        let report = board.run_boot(100).unwrap();
        assert_eq!(report.stop_reason, Some(MachineStopReason::Reset));
    }

    #[test]
    fn platform_reset_refuses_a_linux_board_and_changes_nothing() {
        let mut board = linux_board();
        let rip = board.state().rip;
        assert!(matches!(
            board.platform_reset(PlatformResetKind::Cold),
            Err(FirmwareBoardError::Config(_))
        ));
        assert_eq!(board.state().rip, rip);
    }

    #[test]
    fn reboot_app_needs_tpm_and_nvram() {
        let config = FirmwareBoardConfig {
            fv_app: Some(FirmwareFvApp::Reboot),
            acpi_pm: true,
            acpi_tables: true,
            ..reference_config()
        };
        assert!(matches!(
            validate_config(&config),
            Err(FirmwareBoardError::Config(_))
        ));
        let full = FirmwareBoardConfig {
            nvram: Some(NvramConfig::Empty),
            tpm: Some(TpmConfig {
                entropy: vec![0; 48],
                snapshot: None,
            }),
            ..config
        };
        assert!(validate_config(&full).is_ok());
        let block = firmware_handoff_block(&full).unwrap();
        assert_eq!(u16::from_le_bytes([block[0x0a], block[0x0b]]), 13);
        assert_eq!(
            u32::from_le_bytes(block[0xa0..0xa4].try_into().unwrap()),
            10
        );
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

    /// MOV AL,'X'; MOV DX,0x3F8; OUT DX,AL; MOV AL,'Y'; OUT DX,AL; MOV AL,'Z'; OUT DX,AL; HLT.
    fn com1_xyz_image() -> Vec<u8> {
        test_image(
            &[
                0xb0, b'X', 0x66, 0xba, 0xf8, 0x03, 0xee, 0xb0, b'Y', 0xee, 0xb0, b'Z', 0xee, 0xf4,
            ],
            &[],
        )
    }

    #[test]
    fn quantum_hook_sees_each_boundary_and_leaves_the_run_unchanged() {
        let plain = load(&com1_xyz_image()).run_boot(1000).unwrap();
        assert_eq!(plain.com1, b"XYZ");
        assert!(plain.quanta > 1, "a COM1 write ends the quantum");

        let mut board = load(&com1_xyz_image());
        let mut seen = Vec::new();
        let hooked = board
            .run_boot_with_quantum_hook(1000, |board, progress| {
                // The transcript and the clock of the boundary are readable from the hook.
                seen.push((
                    progress,
                    board.com1_transcript().len(),
                    board.clock().now_ns(),
                ));
            })
            .unwrap();
        assert_eq!(hooked, plain, "the hook must not change the run");
        // Every quantum but the last (which ends the run) is reported, in order.
        assert_eq!(seen.len() as u64, plain.quanta - 1);
        assert!(seen.windows(2).all(|pair| {
            pair[0].0.quanta + 1 == pair[1].0.quanta && pair[0].0.retired < pair[1].0.retired
        }));
        assert!(seen.iter().all(|(progress, _, ns)| *ns == progress.retired));
        assert!(
            seen.last().unwrap().1 >= 2,
            "earlier bytes were already in the transcript"
        );
    }

    #[test]
    fn diagnostic_reads_follow_the_boot_page_tables_and_change_nothing() {
        let mut board = load(&hlt_image());
        let before = board.state().clone();
        let view = board.diagnostic_vcpu_state(BOOT_VCPU_INDEX).unwrap();
        assert_eq!(view.rip, before.rip);
        assert_eq!(view.cr3, Some(FIRMWARE_BOOT_PAGE_TABLES_GPA));
        assert_eq!(view.cr0, Some(FIRMWARE_CR0));
        assert_eq!(view.cr4, Some(FIRMWARE_CR4));
        assert_eq!(view.efer, FIRMWARE_EFER);
        assert!(view.long_mode_model);
        assert!(board.diagnostic_vcpu_state(board.vcpu_count()).is_none());

        // The entry code is readable through the identity-mapped boot tables.
        let mut code = [0u8; 4];
        board
            .diagnostic_read_virtual(BOOT_VCPU_INDEX, view.rip, &mut code)
            .unwrap();
        assert_eq!(code, [0xf4, 0, 0, 0]);

        // An address no boot table maps: the failure says why and how much was read.
        let unmapped = 0x0000_7fff_0000_0000;
        let failure = board
            .diagnostic_read_virtual(BOOT_VCPU_INDEX, unmapped, &mut code)
            .unwrap_err();
        assert_eq!(failure.read, 0);
        assert!(matches!(
            failure.error,
            DiagnosticReadError::Translate(DiagnosticTranslateError::NotPresent { .. })
        ));
        // MMIO is not read: the LAPIC window translates (identity) but has no readable backing.
        let failure = board
            .diagnostic_read_virtual(BOOT_VCPU_INDEX, LOCAL_APIC_MMIO_BASE, &mut code)
            .unwrap_err();
        assert!(matches!(
            failure.error,
            DiagnosticReadError::Physical { .. }
        ));
        let failure = board
            .diagnostic_read_virtual(board.vcpu_count(), view.rip, &mut code)
            .unwrap_err();
        assert!(matches!(
            failure.error,
            DiagnosticReadError::Translate(DiagnosticTranslateError::NoSuchVcpu)
        ));
        assert_eq!(
            board.state(),
            &before,
            "diagnostics do not change the CPU state"
        );
        assert_eq!(board.run_boot(10).unwrap().exit, PagedProbeExit::Halted);
    }

    #[test]
    fn firmware_rdtsc_counts_retirement_inside_and_across_quanta() {
        // RDTSC; MOV R8,RAX; STD; RDTSC; HLT.
        let image = test_image(&[0x0f, 0x31, 0x49, 0x89, 0xc0, 0xfd, 0x0f, 0x31, 0xf4], &[]);
        let config = FirmwareBoardConfig {
            tsc_hz: 625_000_000,
            ..reference_config()
        };
        for budget in [1, 16] {
            let mut board = load_firmware_image(&image, &config).unwrap();
            board.advance_time_ns(1).unwrap();
            let mut retired = 0;
            let mut quanta = 0;
            loop {
                let result = board.run_boot(budget).unwrap();
                retired += result.retired;
                quanta += result.quanta;
                if result.exit == PagedProbeExit::Halted {
                    break;
                }
                assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
                assert!(retired < 5);
            }
            assert_eq!(retired, 5);
            assert_eq!(quanta, if budget == 1 { 5 } else { 1 });
            // Absolute time advances from 1 ns to 4 ns before the second
            // sample. Independently rounding 3 ns forward would lose a tick.
            assert_eq!(board.state().gpr[Register::R8 as usize], 0);
            assert_eq!(board.state().gpr[Register::Rax as usize], 2);
            assert_eq!(board.state().gpr[Register::Rdx as usize], 0);
            assert_eq!(board.clock().now_ns(), 6);
            assert_eq!(board.clock().tsc(), 3);
        }
    }

    #[test]
    fn firmware_rdtsc_projection_overflow_preserves_the_faulting_instruction() {
        // STD; CLD; RDTSC; HLT. The sample would overflow after two retirees.
        let image = test_image(&[0xfd, 0xfc, 0x0f, 0x31, 0xf4], &[]);
        for (hz, now_ns, expected_error) in [
            (1, u64::MAX - 1, VirtualClockError::TimeOverflow),
            (
                2_000_000_000,
                u64::MAX / 2 - 1,
                VirtualClockError::TscOverflow,
            ),
        ] {
            let config = FirmwareBoardConfig {
                tsc_hz: hz,
                ..reference_config()
            };
            let mut board = load_firmware_image(&image, &config).unwrap();
            board.advance_time_ns(now_ns).unwrap();
            let clock = board.clock();
            let before = board.state().clone();
            assert!(matches!(
                board.run_boot(4),
                Err(FirmwareBoardError::Clock(error)) if error == expected_error
            ));
            assert_eq!(board.state().rip, before.rip + 2);
            assert_eq!(board.state().gpr, before.gpr);
            assert_eq!(board.state().rflags, before.rflags);
            assert_eq!(board.clock(), clock);
        }
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

    /// An image that declares support for handoff minor 4.
    fn minor4_image(code: &[u8]) -> Vec<u8> {
        test_image(code, &[(0x2A, &HANDOFF_MINOR_RTC.to_le_bytes())])
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
    fn fv_app_extends_only_selected_boots_and_requires_minor_five_support() {
        let current_image = test_image(&[0xf4], &[(0x2A, &5u16.to_le_bytes())]);
        let legacy = load_firmware_image(&current_image, &reference_config()).unwrap();
        assert_eq!(hex(legacy.handoff_block()), REFERENCE_HANDOFF_HEX);

        for app in [FirmwareFvApp::Hello, FirmwareFvApp::Ebs] {
            let config = FirmwareBoardConfig {
                fv_app: Some(app),
                ..reference_config()
            };
            assert!(matches!(
                load_firmware_image(&minor4_image(&[0xf4]), &config),
                Err(FirmwareBoardError::ImageHeader {
                    field: "handoff_minor_max",
                    value: 4
                })
            ));
            let board = load_firmware_image(&current_image, &config).unwrap();
            let bytes = board.handoff_block();
            assert_eq!(le_u16(bytes, 0x0A).unwrap(), 5);
            assert_eq!(le_u32(bytes, 0x0C).unwrap(), 0xA8);
            assert_eq!(le_u32(bytes, 0x30).unwrap(), 0xA8);
            assert_eq!(le_u32(bytes, 0xA0).unwrap(), app as u32);
            assert_eq!(le_u32(bytes, 0xA4).unwrap(), 0);
            assert!(crc_is_valid(bytes));
        }
    }

    #[test]
    fn acpi_selector_seals_the_same_handoff_for_dump_and_boot() {
        let legacy = test_image(&[0xf4], &[(0x2A, &8u16.to_le_bytes())]);
        let image = test_image(&[0xf4], &[(0x2A, &9u16.to_le_bytes())]);
        let config = FirmwareBoardConfig {
            fv_app: Some(FirmwareFvApp::Acpi),
            acpi_pm: true,
            acpi_tables: true,
            rtc_base_unix_seconds: Some(1_704_067_200),
            pci_ecam: true,
            tsc_hz: 700_000_000,
            ..reference_config()
        };
        assert!(matches!(
            load_firmware_image(&legacy, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 8
            })
        ));
        let board = load_firmware_image(&image, &config).unwrap();
        let block = firmware_handoff_block(&config).unwrap();
        assert_eq!(block, board.handoff_block());
        assert!(crc_is_valid(&block));
        assert_eq!(le_u16(&block, 0x0A).unwrap(), 9);
        assert_eq!(le_u32(&block, 0x0C).unwrap(), 0xB0);
        assert_eq!(le_u32(&block, 0xA0).unwrap(), 5);
        assert_eq!(le_u32(&block, 0xA8).unwrap(), 1000);
        assert_eq!(le_u64(&block, 0x20).unwrap(), 700_000_000);
        assert_eq!(le_u32(&block, 0xAC).unwrap(), 0);
        assert_eq!(le_u32(device_entry(&block, 4), 4).unwrap(), 0);
    }

    #[test]
    fn measured_firmware_can_read_rom_data_without_enabling_rom_fetch() {
        // MOV RAX,[RBX]; HLT. The image header occupies ROM at the same base.
        let image = test_image(&[0x48, 0x8b, 0x03, 0xf4], &[(0x2a, &13u16.to_le_bytes())]);
        let mut without_tpm = load_firmware_image(&image, &reference_config()).unwrap();
        without_tpm.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_ROM_BASE;
        let report = without_tpm.run_boot(2).unwrap();
        assert_eq!(report.retired, 0);
        assert!(matches!(
            report.memory_error,
            Some(crate::TrackedAccessError::OperandPhysical {
                source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Rom),
                ..
            })
        ));

        let config = FirmwareBoardConfig {
            tpm: Some(TpmConfig {
                entropy: vec![0xf2; 48],
                snapshot: None,
            }),
            acpi_pm: true,
            acpi_tables: true,
            ..reference_config()
        };
        let mut with_tpm = load_firmware_image(&image, &config).unwrap();
        with_tpm.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_ROM_BASE;
        let report = with_tpm.run_boot(3).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(
            with_tpm.state().gpr[Register::Rax as usize],
            u64::from_le_bytes(*b"ODFWIMG1")
        );

        let mut fetch = load_firmware_image(&image, &config).unwrap();
        fetch.state_mut().rip = FIRMWARE_ROM_BASE + 0x40;
        let report = fetch.run_boot(1).unwrap();
        assert_eq!(report.retired, 0);
        assert!(matches!(
            report.exit,
            PagedProbeExit::Fault(ArchitecturalFault { vector: 14, .. })
        ));

        let mut crossing = load_firmware_image(&image, &config).unwrap();
        crossing.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_ROM_BASE + 0xffc;
        let report = crossing.run_boot(1).unwrap();
        assert_eq!(report.retired, 0);
        assert!(report.memory_error.is_some());

        let store_image = test_image(&[0x48, 0x89, 0x03, 0xf4], &[(0x2a, &13u16.to_le_bytes())]);
        let mut store = load_firmware_image(&store_image, &config).unwrap();
        store.state_mut().gpr[Register::Rbx as usize] = FIRMWARE_ROM_BASE;
        assert_eq!(store.run_boot(1).unwrap().retired, 0);
        assert_eq!(
            store
                .bus_mut()
                .read_physical(
                    GuestPhysicalAddress::new(FIRMWARE_ROM_BASE),
                    AccessWidth::Qword
                )
                .unwrap(),
            u64::from_le_bytes(*b"ODFWIMG1")
        );
    }

    #[test]
    fn tpm_is_opt_in_with_an_exact_owned_handoff_region_and_snapshot_restore() {
        let image = test_image(&[0xf4], &[(0x2A, &13u16.to_le_bytes())]);
        let old = test_image(&[0xf4], &[(0x2A, &12u16.to_le_bytes())]);
        let tpm = TpmConfig {
            entropy: vec![0x63; 48],
            snapshot: None,
        };
        let config = FirmwareBoardConfig {
            fv_app: Some(FirmwareFvApp::Tpm),
            tpm: Some(tpm),
            acpi_pm: true,
            acpi_tables: true,
            ..reference_config()
        };
        let default = load_firmware_image(&image, &reference_config()).unwrap();
        assert!(default.tpm_state_snapshot().is_none());
        assert!(matches!(
            load_firmware_image(&old, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 12
            })
        ));
        let mut board = load_firmware_image(&image, &config).unwrap();
        let block = board.handoff_block();
        assert_eq!(le_u16(block, 0x0a).unwrap(), 13);
        assert_eq!(le_u32(block, 0xa0).unwrap(), 9);
        let at = le_u32(block, 0x3c).unwrap() as usize + DEV_TPM * DEVICE_ENTRY_BYTES;
        assert_eq!(le_u32(block, at).unwrap(), DEVICE_CODE_TPM);
        assert_eq!(le_u32(block, at + 4).unwrap(), DEVICE_PRESENT);
        assert_eq!(le_u64(block, at + 8).unwrap(), TPM_CRB_BASE);
        assert_eq!(le_u64(block, at + 16).unwrap(), TPM_CRB_LEN);
        let start = le_u32(block, 0x30).unwrap() as usize;
        let count = le_u32(block, 0x34).unwrap() as usize;
        assert!(
            block[start..start + count * REGION_ENTRY_BYTES]
                .chunks_exact(REGION_ENTRY_BYTES)
                .any(|entry| region_tuple(entry)
                    == (REGION_MMIO, TPM_CRB_BASE, TPM_CRB_LEN, DEV_TPM as u32))
        );
        let reg = GuestPhysicalAddress::new(TPM_CRB_BASE + 8);
        let status = GuestPhysicalAddress::new(TPM_CRB_BASE + 12);
        board
            .bus_mut()
            .write_physical(reg, AccessWidth::Dword, 1, |_| {})
            .unwrap();
        assert_eq!(
            board
                .bus_mut()
                .read_physical(status, AccessWidth::Dword)
                .unwrap(),
            1
        );
        let snapshot = board.tpm_state_snapshot().unwrap();
        let restored = FirmwareBoardConfig {
            tpm: Some(TpmConfig {
                entropy: vec![0x64; 48],
                snapshot: Some(snapshot),
            }),
            ..config
        };
        let mut reopened = load_firmware_image(&image, &restored).unwrap();
        assert_eq!(
            reopened
                .bus_mut()
                .read_physical(status, AccessWidth::Dword)
                .unwrap(),
            0
        );
        let reopened_snapshot = zeroize::Zeroizing::new(reopened.tpm_state_snapshot().unwrap());
        assert!(
            Some(reopened_snapshot.as_slice())
                == restored.tpm.as_ref().unwrap().snapshot.as_deref()
        );
        let mut broken = restored.clone();
        broken.tpm.as_mut().unwrap().snapshot.as_mut().unwrap()[0] ^= 1;
        assert!(matches!(
            load_firmware_image(&image, &broken),
            Err(FirmwareBoardError::Tpm(_))
        ));
        // A snapshot of a newer build (ADR-0009, decision 6) is refused, not read as empty state.
        let mut newer = restored.clone();
        let newer_version = (od_tpm::SNAPSHOT_VERSION + 1).to_be_bytes();
        newer.tpm.as_mut().unwrap().snapshot.as_mut().unwrap()[8..10]
            .copy_from_slice(&newer_version);
        assert!(matches!(
            load_firmware_image(&image, &newer),
            Err(FirmwareBoardError::Tpm(od_tpm::StateError::UnsupportedVersion(v)))
                if v == od_tpm::SNAPSHOT_VERSION + 1
        ));
    }

    #[test]
    fn rt_selector_requires_v18_and_all_runtime_devices() {
        let legacy = test_image(&[0xf4], &[(0x2A, &7u16.to_le_bytes())]);
        let image = test_image(&[0xf4], &[(0x2A, &8u16.to_le_bytes())]);
        let config = FirmwareBoardConfig {
            fv_app: Some(FirmwareFvApp::Rt),
            acpi_pm: true,
            acpi_tables: true,
            rtc_base_unix_seconds: Some(1_704_067_200),
            nvram: Some(NvramConfig::Empty),
            ..reference_config()
        };
        assert!(matches!(
            load_firmware_image(&legacy, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 7,
            })
        ));
        for missing in [
            FirmwareBoardConfig {
                acpi_tables: false,
                ..config.clone()
            },
            FirmwareBoardConfig {
                rtc_base_unix_seconds: None,
                ..config.clone()
            },
            FirmwareBoardConfig {
                nvram: None,
                ..config.clone()
            },
        ] {
            assert!(matches!(
                load_firmware_image(&image, &missing),
                Err(FirmwareBoardError::Config(
                    "rt FV application needs ACPI fixed hardware, RTC and NVRAM"
                ))
            ));
        }
        let board = load_firmware_image(&image, &config).unwrap();
        let bytes = board.handoff_block();
        assert_eq!(le_u16(bytes, 0x0A).unwrap(), 8);
        assert_eq!(le_u32(bytes, 0x0C).unwrap(), 0xA8);
        assert_eq!(le_u32(bytes, 0xA0).unwrap(), 4);
        assert!(crc_is_valid(bytes));
    }

    #[test]
    fn vars_selector_requires_v16_and_encodes_selector_three() {
        let legacy_image = test_image(&[0xf4], &[(0x2A, &5u16.to_le_bytes())]);
        let image = test_image(&[0xf4], &[(0x2A, &6u16.to_le_bytes())]);
        let config = FirmwareBoardConfig {
            fv_app: Some(FirmwareFvApp::Vars),
            ..reference_config()
        };
        assert!(matches!(
            load_firmware_image(&legacy_image, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 5
            })
        ));
        let board = load_firmware_image(&image, &config).unwrap();
        let bytes = board.handoff_block();
        assert_eq!(le_u16(bytes, 0x0A).unwrap(), 6);
        assert_eq!(le_u32(bytes, 0x0C).unwrap(), 0xA8);
        assert_eq!(le_u32(bytes, 0xA0).unwrap(), 3);
        assert!(crc_is_valid(bytes));
    }

    #[test]
    fn firmware_without_protected_nvram_support_rejects_a_protected_snapshot() {
        use od_uefi_security::protected::{Flags, HEADER_BYTES, Image};

        let mut store = [0; HEADER_BYTES];
        Image::write_header(
            &mut store,
            Flags {
                secure_boot_disabled: false,
                vendor_keys_modified: false,
            },
        )
        .unwrap();
        let snapshot = NvramDevice::with_protected_store(&store)
            .unwrap()
            .nvram_snapshot();
        let config = FirmwareBoardConfig {
            nvram: Some(NvramConfig::Snapshot(snapshot)),
            ..reference_config()
        };
        let legacy = test_image(&[0xf4], &[(0x2A, &13u16.to_le_bytes())]);
        assert!(matches!(
            load_firmware_image(&legacy, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 13
            })
        ));
        let firmware = test_image(
            &[0xf4],
            &[(0x2A, &HANDOFF_MINOR_PROTECTED_NVRAM.to_le_bytes())],
        );
        assert!(matches!(
            load_firmware_image(&firmware, &config),
            Err(FirmwareBoardError::Nvram(
                NvramContentsError::ProtectionRequiresFirmware
            ))
        ));
        let enforcing_rom =
            test_image(&[0xf4], &[(0x2A, &HANDOFF_MINOR_SECURE_BOOT.to_le_bytes())]);
        let board = load_firmware_image(&enforcing_rom, &config).unwrap();
        assert_eq!(
            le_u16(board.handoff_block(), 0x0A).unwrap(),
            HANDOFF_MINOR_SECURE_BOOT
        );
        assert_eq!(
            firmware_handoff_block(&config).unwrap(),
            board.handoff_block()
        );
        let pkcs7_rom = test_image(&[0xf4], &[(0x2A, &HANDOFF_MINOR_PKCS7.to_le_bytes())]);
        let board = load_firmware_image(&pkcs7_rom, &config).unwrap();
        assert_eq!(
            le_u16(board.handoff_block(), 0x0A).unwrap(),
            HANDOFF_MINOR_PKCS7
        );
        assert!(matches!(
            assemble_board(&config, PciEcam::new(), None, None),
            Err(FirmwareBoardError::Nvram(
                NvramContentsError::ProtectionRequiresFirmware
            ))
        ));

        Image::write_header(
            &mut store,
            Flags {
                secure_boot_disabled: true,
                vendor_keys_modified: false,
            },
        )
        .unwrap();
        let snapshot = NvramDevice::with_protected_store(&store)
            .unwrap()
            .nvram_snapshot();
        let debug = FirmwareBoardConfig {
            nvram: Some(NvramConfig::Snapshot(snapshot)),
            ..config
        };
        let board = load_firmware_image(&firmware, &debug).unwrap();
        let block = board.handoff_block();
        assert_eq!(le_u16(block, 0x0A).unwrap(), HANDOFF_MINOR_PROTECTED_NVRAM);
        let device = le_u32(block, 0x3C).unwrap() as usize + DEV_NVRAM * DEVICE_ENTRY_BYTES;
        assert_eq!(le_u64(block, device + 0x20).unwrap(), 2);
        assert_eq!(le_u64(block, device + 0x28).unwrap(), 0);
    }

    #[test]
    fn nvram_is_opt_in_v17_owned_mmio_and_roundtrips_an_independent_board() {
        let config = FirmwareBoardConfig {
            nvram: Some(NvramConfig::Empty),
            ..reference_config()
        };
        let old = test_image(&[0xf4], &[(0x2A, &6u16.to_le_bytes())]);
        assert!(matches!(
            load_firmware_image(&old, &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 6
            })
        ));
        assert!(
            load_firmware_image(&old, &reference_config())
                .unwrap()
                .nvram_snapshot()
                .is_none()
        );
        let image = test_image(&[0xf4], &[(0x2A, &7u16.to_le_bytes())]);
        let mut board = load_firmware_image(&image, &config).unwrap();
        let block = board.handoff_block();
        assert_eq!(le_u16(block, 0x0A).unwrap(), 7);
        assert_eq!(le_u32(block, 0x0C).unwrap(), 0xA8);
        assert_eq!(le_u32(block, 0xA0).unwrap(), 0);
        assert_eq!(le_u32(block, 0x40).unwrap(), 7);
        let device = le_u32(block, 0x3C).unwrap() as usize + DEV_NVRAM * DEVICE_ENTRY_BYTES;
        assert_eq!(le_u32(block, device).unwrap(), DEVICE_CODE_NVRAM);
        assert_eq!(le_u32(block, device + 4).unwrap(), DEVICE_PRESENT);
        assert_eq!(le_u64(block, device + 8).unwrap(), NVRAM_MMIO_BASE);
        let start = le_u32(block, 0x30).unwrap() as usize;
        let count = le_u32(block, 0x34).unwrap() as usize;
        let table = &block[start..start + count * REGION_ENTRY_BYTES];
        assert!(
            table
                .chunks_exact(REGION_ENTRY_BYTES)
                .any(|region| region_tuple(region)
                    == (
                        REGION_NVRAM,
                        NVRAM_MMIO_BASE,
                        NVRAM_MMIO_BYTES,
                        DEV_NVRAM as u32
                    ))
        );
        assert!(crc_is_valid(block));

        let register = NVRAM_MMIO_BASE + crate::NVRAM_STORE_BYTES as u64;
        let bus = board.bus_mut();
        for (address, width, value) in [
            (register + 4, AccessWidth::Dword, 4),
            (register + 8, AccessWidth::Dword, 2),
            (NVRAM_MMIO_BASE, AccessWidth::Dword, 0x7856_3412),
            (register + 8, AccessWidth::Dword, 3),
        ] {
            bus.write_physical(GuestPhysicalAddress::new(address), width, value, |_| {})
                .unwrap();
        }
        let bytes = board.nvram_snapshot().unwrap();
        let restored = FirmwareBoardConfig {
            nvram: Some(NvramConfig::Snapshot(bytes.clone())),
            ..config
        };
        let mut board = load_firmware_image(&image, &restored).unwrap();
        assert_eq!(board.nvram_snapshot().unwrap(), bytes);
        assert_eq!(
            board
                .bus_mut()
                .read_physical(GuestPhysicalAddress::new(register + 16), AccessWidth::Dword)
                .unwrap(),
            4
        );
        assert_eq!(
            board
                .bus_mut()
                .read_physical(GuestPhysicalAddress::new(register + 12), AccessWidth::Dword)
                .unwrap(),
            0
        );
    }

    #[test]
    fn rtc_base_time_adds_v14_device_entry_without_changing_older_handoffs() {
        let old = load(&hlt_image());
        assert_eq!(le_u16(old.handoff_block(), 0x0A).unwrap(), 0);
        assert_eq!(hex(old.handoff_block()), REFERENCE_HANDOFF_HEX);

        let config = FirmwareBoardConfig {
            rtc_base_unix_seconds: Some(1_700_000_000),
            ..reference_config()
        };
        let board = load_firmware_image(&minor4_image(&[0xf4]), &config).unwrap();
        let bytes = board.handoff_block();
        assert_eq!(le_u16(bytes, 0x0A).unwrap(), HANDOFF_MINOR_RTC);
        assert_eq!(le_u32(bytes, 0x40).unwrap(), 8);
        let rtc = le_u32(bytes, 0x3C).unwrap() as usize + 7 * DEVICE_ENTRY_BYTES;
        assert_eq!(le_u32(bytes, rtc).unwrap(), DEVICE_CODE_RTC_CMOS);
        assert_eq!(le_u32(bytes, rtc + 4).unwrap(), DEVICE_PRESENT | DEVICE_PIO);
        assert_eq!(le_u64(bytes, rtc + 8).unwrap(), 0x70);
        assert_eq!(le_u64(bytes, rtc + 0x10).unwrap(), 2);
        assert_eq!(le_u32(bytes, rtc + 0x18).unwrap(), NO_GSI);
        assert_eq!(le_u32(bytes, rtc + 0x1C).unwrap(), 0);
        assert_eq!(le_u64(bytes, rtc + 0x20).unwrap(), 1_700_000_000);
        assert_eq!(le_u64(bytes, rtc + 0x28).unwrap(), 0);
        assert!(crc_is_valid(bytes));

        assert!(matches!(
            load_firmware_image(&minor2_image(&[0xf4]), &config),
            Err(FirmwareBoardError::ImageHeader {
                field: "handoff_minor_max",
                value: 2
            })
        ));
        let invalid = FirmwareBoardConfig {
            rtc_base_unix_seconds: Some(MAX_RTC_BASE_UNIX_SECONDS + 1),
            ..reference_config()
        };
        assert_eq!(
            load_firmware_image(&minor4_image(&[0xf4]), &invalid).err(),
            Some(FirmwareBoardError::Config(
                "RTC base time must be a UTC Unix timestamp through 9999-12-31"
            ))
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

    #[test]
    fn unified_entry_descriptors_have_the_documented_values_and_a_second_call_changes_nothing() {
        let mut state = LongModeState::default();
        model_unified_entry_descriptors(&mut state);
        // GDTR: the value after reset (SDM Vol. 3A, Table 10-1).
        assert_eq!(
            state.gdtr,
            Some(od_isa::state::Gdtr {
                base: 0,
                limit: 0xffff
            })
        );
        // Flat caches: not reset values (reset gives limit 0xFFFF), the Linux entry values.
        let data = od_isa::DataSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0x93,
            flags: 0xc,
        };
        assert_eq!(state.ss_cache, Some(data));
        assert_eq!(state.ds_cache, Some(data));
        assert_eq!(state.es_cache, Some(data));
        assert_eq!(state.ss_selector, Some(0));
        let flat = od_isa::FsDescriptorCache {
            limit: 0xffff_ffff,
            access: 0x93,
            flags: 0xc,
        };
        assert_eq!((state.fs_cache, state.gs_cache), (Some(flat), Some(flat)));
        // Nothing else of the entry state changes.
        let mut expected = LongModeState::default();
        (expected.gdtr, expected.ss_cache, expected.ss_selector) =
            (state.gdtr, state.ss_cache, state.ss_selector);
        (expected.ds_cache, expected.es_cache) = (state.ds_cache, state.es_cache);
        (expected.fs_cache, expected.gs_cache) = (state.fs_cache, state.gs_cache);
        assert_eq!(state, expected);
        let once = state.clone();
        model_unified_entry_descriptors(&mut state);
        assert_eq!(state, once);
        // A modeled value is never overwritten.
        let mut modeled = LongModeState::default();
        modeled.gdtr = Some(od_isa::state::Gdtr {
            base: 0x1000,
            limit: 0x17,
        });
        modeled.ss_selector = Some(0x10);
        model_unified_entry_descriptors(&mut modeled);
        assert_eq!(modeled.gdtr.map(|gdtr| gdtr.base), Some(0x1000));
        assert_eq!(modeled.ss_selector, Some(0x10));
    }

    #[test]
    fn a_marker_only_observes_the_unified_run() {
        // 40 NOPs, then HLT at IF = 0 (S7-31m: the set has the halt member; nothing can wake it).
        let mut code = vec![0x90u8; 40];
        code.push(0xf4);
        let image = minor2_image(&code);
        let mut plain = load_firmware_image(&image, &linux_config()).unwrap();
        let mut marked = load_firmware_image(&image, &linux_config()).unwrap();
        let entry = plain.state().rip;
        let without = crate::run_firmware_unified(&mut plain, 1_000).unwrap();
        let with =
            crate::run_firmware_unified_marked(&mut marked, 1_000, Some(entry + 10)).unwrap();
        // The marker is the boundary before the 11th instruction.
        assert_eq!(without.marker, None);
        assert_eq!(with.marker_rip, Some(entry + 10));
        assert_eq!(with.marker.as_ref().map(|marker| marker.retired), Some(10));
        // Retired, quanta, time, exit and the final state are those of the run without it.
        assert_eq!(with.run, without.run);
        assert_eq!(with.set_exit_detail, without.set_exit_detail);
        // The HLT retires, then the CPU stays halted: IF = 0 (SDM: only NMI, SMI or reset wake it).
        assert_eq!(with.run.retired, 41);
        assert_eq!(with.run.exit, PagedProbeExit::Halted);
        assert_eq!(marked.state(), plain.state());
        // S7-31m: the interrupt state of the stop is reported and observes only. The CPU is
        // halted with IF clear (the reason is recorded), the PIC is as reset (all masked, nothing
        // requested), channel 0 is unprogrammed, the PIT -> 8259 source is installed but has
        // nothing to deliver, and the CPUID profile declares no APIC.
        assert_eq!(with.interrupts, without.interrupts);
        let interrupts = &with.interrupts;
        assert!(!interrupts.interrupt_flag);
        assert_eq!(interrupts.activity.as_deref(), Some("Halted"));
        assert_eq!(interrupts.halt_stop, Some("if_clear"));
        assert_eq!(interrupts.source, "pit_pic");
        assert_eq!(
            (interrupts.pic_imr, interrupts.pic_irr, interrupts.pic_isr),
            (Some(0xff), Some(0), Some(0))
        );
        assert_eq!(interrupts.pit_channel0_programmed, Some(false));
        assert!(!interrupts.apic_declared);
        assert_eq!(
            (
                interrupts.irqs_delivered,
                interrupts.irq0_edges,
                interrupts.halt_wakes
            ),
            (0, 0, 0)
        );
        // The final RIP is reported with its raw paging-structure entries (S7-31e): the PML4E
        // first, present, at the board CR3.
        assert_eq!(with.rip_walk[0].level, "PML4E");
        assert_eq!(with.rip_walk[0].entry_gpa & !0xfff, marked.cr3());
        assert_eq!(with.rip_walk[0].raw & 1, 1);
        // A marker the run never reaches is reported as not reached.
        let mut unreached = load_firmware_image(&image, &linux_config()).unwrap();
        let never =
            crate::run_firmware_unified_marked(&mut unreached, 1_000, Some(0x1234)).unwrap();
        assert_eq!(never.marker, None);
        assert_eq!(never.run, without.run);
    }

    /// S7-31q: the call trace names a function, the GUID and the UCS-2 name its arguments point to
    /// (RDX and RCX, as the Microsoft x64 ABI passes the 2nd and 1st argument), and only observes.
    #[test]
    fn a_call_trace_records_the_call_with_its_guid_and_name_and_changes_nothing() {
        const FUNCTION: usize = 0x40;
        const GUID: usize = 0x80;
        const NAME: usize = 0xA0;
        let mut code = vec![0x90u8; 0xC0];
        let guid: [u8; 16] = [
            0x91, 0x6e, 0x57, 0x09, 0x3f, 0x6d, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69,
            0x72, 0x3b,
        ];
        code[GUID..GUID + 16].copy_from_slice(&guid);
        code[NAME..NAME + 6].copy_from_slice(&[b'A', 0, b'b', 0, 0, 0]);
        code[FUNCTION] = 0xC3; // ret
        let image = minor2_image(&code);
        let entry = load_firmware_image(&image, &linux_config())
            .unwrap()
            .state()
            .rip;
        // mov edx, GUID; mov ecx, NAME; call FUNCTION; hlt
        let mut program = vec![0xBA];
        program.extend_from_slice(&((entry as u32) + GUID as u32).to_le_bytes());
        program.push(0xB9);
        program.extend_from_slice(&((entry as u32) + NAME as u32).to_le_bytes());
        program.push(0xE8);
        program.extend_from_slice(&((FUNCTION as i32) - 15).to_le_bytes());
        program.push(0xF4);
        let mut image = image;
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let mut traced = load_firmware_image(&image, &linux_config()).unwrap();
        let mut plain = load_firmware_image(&image, &linux_config()).unwrap();
        let target = crate::CallTraceTarget {
            name: "svc".to_owned(),
            physical: entry + FUNCTION as u64,
            guid_arg: Some(1),
            name_arg: Some(0),
            rt_slot: None,
        };
        let with = crate::run_firmware_unified_traced(&mut traced, 1_000, None, vec![target], None)
            .unwrap();
        let without = crate::run_firmware_unified(&mut plain, 1_000).unwrap();
        let trace = with.call_trace.expect("a trace was asked for");
        assert_eq!(trace.total, 1);
        assert_eq!(trace.entries.len(), 1);
        let call = &trace.entries[0];
        assert_eq!((call.name.as_str(), call.cpl), ("svc", 0));
        assert_eq!(
            call.caller,
            Some(entry + 15),
            "the return address: the instruction after the call"
        );
        assert_eq!(call.guid, Some(guid));
        assert_eq!(call.variable_name.as_deref(), Some("Ab"));
        assert_eq!(
            call.retired, 3,
            "mov, mov, call retired before the first instruction of svc"
        );
        assert!(without.call_trace.is_none());
        // Observation only: retired, quanta, exit and the state are those of the run without it.
        assert_eq!(with.run, without.run);
        assert_eq!(traced.state(), plain.state());
    }

    /// PERF-20: the histogram alone leaves the call trace off (`call_trace` stays `None`, as in a
    /// run without it), counts every retired instruction and changes nothing else.
    #[test]
    fn an_op_histogram_without_a_call_trace_leaves_the_call_trace_none() {
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        image[0x40..0x43].copy_from_slice(&[0x90, 0x50, 0xF4]); // nop; push rax; hlt
        seal(&mut image);
        let mut counted = load_firmware_image(&image, &linux_config()).unwrap();
        let mut plain = load_firmware_image(&image, &linux_config()).unwrap();
        let with =
            crate::run_firmware_unified_profiled(&mut counted, 1_000, None, Vec::new(), None, true)
                .unwrap();
        let without = crate::run_firmware_unified(&mut plain, 1_000).unwrap();
        assert!(with.call_trace.is_none());
        assert!(without.call_trace.is_none());
        let histogram = with.op_histogram.expect("a histogram was asked for");
        assert!(without.op_histogram.is_none());
        assert_eq!(histogram.total, with.run.retired);
        assert_eq!(with.run, without.run);
        assert_eq!(counted.state(), plain.state());
    }

    /// PERF-22: the fast-path check in the unified loop (ADR-0017, section 5) changes nothing a run
    /// does (retired, quanta, exit, state) and finds no divergence with the fast path of the
    /// tree (it takes PUSH, POP and CALL and refuses the NOP) nor with the general path as its own fast path (every
    /// instruction compared, the memory calls of the board adapter recorded and replayed).
    #[test]
    fn a_fast_path_check_leaves_the_unified_run_unchanged_and_finds_no_divergence() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // nop; push rax; pop rcx; call +0; pop rdx; hlt
        image[0x40..0x4a].copy_from_slice(&[0x90, 0x50, 0x59, 0xE8, 0, 0, 0, 0, 0x5A, 0xF4]);
        seal(&mut image);
        let mut plain = load_firmware_image(&image, &linux_config()).unwrap();
        let without = crate::run_firmware_unified(&mut plain, 1_000).unwrap();
        assert!(without.fast_path_check.is_none());
        for mode in [FastPathMode::Tree, FastPathMode::GeneralPath] {
            fast_path_check::set_mode(mode);
            let mut checked = load_firmware_image(&image, &linux_config()).unwrap();
            let with = crate::run_firmware_unified_checked(
                &mut checked,
                1_000,
                None,
                Vec::new(),
                None,
                false,
                true,
            )
            .unwrap();
            fast_path_check::set_mode(FastPathMode::Tree);
            let report = with
                .fast_path_check
                .clone()
                .expect("the check was asked for");
            assert_eq!(report.divergence, None, "{mode:?}");
            assert_eq!(report.counters.divergences, 0, "{mode:?}");
            assert_eq!(with.run, without.run, "{mode:?}");
            assert_eq!(checked.state(), plain.state(), "{mode:?}");
            let steps = report.counters.compared + report.counters.refused_by_admission;
            assert!(steps >= with.run.retired, "{mode:?} {report:?}");
            match mode {
                // PERF-25: PUSH, POP, CALL and POP are group G5 of the tree; the NOP is refused.
                FastPathMode::Tree => assert_eq!(report.counters.compared, 4, "{report:?}"),
                _ => assert_eq!(report.counters.refused_by_admission, 0),
            }
        }
    }

    /// PERF-23: the fast path of the tree takes the register ALU forms (group G2) in the unified
    /// run, the check finds no divergence and the run is the run without the check.
    #[test]
    fn the_tree_fast_path_takes_register_alu_in_the_unified_run_and_agrees() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // add rax, rcx; xor edx, edx; shl rax, 3; cmp rax, rdx; nop; hlt
        let program = [
            0x48, 0x01, 0xC8, 0x31, 0xD2, 0x48, 0xC1, 0xE0, 0x03, 0x48, 0x39, 0xD0, 0x90, 0xF4,
        ];
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let mut plain = load_firmware_image(&image, &linux_config()).unwrap();
        let without = crate::run_firmware_unified(&mut plain, 1_000).unwrap();
        fast_path_check::set_mode(FastPathMode::Tree);
        let mut checked = load_firmware_image(&image, &linux_config()).unwrap();
        let with = crate::run_firmware_unified_checked(
            &mut checked,
            1_000,
            None,
            Vec::new(),
            None,
            false,
            true,
        )
        .unwrap();
        let report = with
            .fast_path_check
            .clone()
            .expect("the check was asked for");
        assert_eq!(report.divergence, None);
        assert_eq!(report.counters.divergences, 0);
        assert_eq!(with.run, without.run);
        assert_eq!(checked.state(), plain.state());
        // The four ALU instructions are taken; the NOP and HLT are refused or end before decode.
        assert!(report.counters.compared >= 4, "{report:?}");
    }

    /// PERF-23 (ADR-0017, section 3): the unified run with the fast path of the step retires the
    /// same instructions and ends in the same state, with the same COM1 transcript, as the run
    /// without it; the fast path really takes the register and immediate ALU forms (debug builds
    /// count them); with the check on as well, the check wins (the general path runs for real).
    #[test]
    fn the_unified_run_with_the_fast_path_equals_the_run_without_it() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // mov edx, 0x3F8; mov eax, 0x40; add eax, 0x0B; out dx, al ('K'); xor eax, eax;
        // or eax, 0x4F; sub eax, 1; out dx, al ('N'); mov eax, 0x25; shl eax, 1; out dx, al ('J'); hlt
        let program = [
            0xBA, 0xF8, 0x03, 0x00, 0x00, 0xB8, 0x40, 0x00, 0x00, 0x00, 0x83, 0xC0, 0x0B, 0xEE,
            0x31, 0xC0, 0x83, 0xC8, 0x4F, 0x83, 0xE8, 0x01, 0xEE, 0xB8, 0x25, 0x00, 0x00, 0x00,
            0xD1, 0xE0, 0xEE, 0xF4,
        ];
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let run = |fast_path: bool, verify: bool| {
            let mut board = load_firmware_image(&image, &linux_config()).unwrap();
            fast_path_check::set_mode(FastPathMode::Tree);
            fast_path_check::reset_fast_steps_taken();
            let run = crate::run_firmware_unified_with_fast_path(
                &mut board,
                1_000,
                None,
                Vec::new(),
                None,
                false,
                verify,
                fast_path,
            )
            .unwrap();
            (run, board, fast_path_check::fast_steps_taken())
        };
        let (without, plain, taken_without) = run(false, false);
        assert_eq!(taken_without, 0);
        assert_eq!(plain.com1_transcript(), b"KNJ");
        assert_eq!(without.run.retired, 12);
        let (with, fast, taken_with) = run(true, false);
        assert_eq!(with.run, without.run);
        assert_eq!(with.run.retired, 12);
        assert_eq!(fast.state(), plain.state());
        assert_eq!(fast.com1_transcript(), plain.com1_transcript());
        assert!(with.fast_path_check.is_none());
        if od_isa::fast_path_check::FAST_STEPS_COUNTED {
            // add, xor, or, sub, shl (G2) and, since PERF-24, the three register MOVs (G1).
            assert_eq!(taken_with, 8);
        }
        // The check wins over the switch: nothing is taken by the step, the five ALU forms and the
        // three MOVs are compared by the check, and the run is the same.
        let (checked, board, taken_checked) = run(true, true);
        let report = checked
            .fast_path_check
            .clone()
            .expect("the check was asked for");
        assert_eq!(report.divergence, None);
        assert_eq!(report.counters.compared, 8, "{report:?}");
        assert_eq!(taken_checked, 0);
        assert_eq!(checked.run, without.run);
        assert_eq!(board.state(), plain.state());
        assert_eq!(board.com1_transcript(), plain.com1_transcript());
    }

    /// PERF-24b, group G4: the moves with memory (stack-relative loads and stores of the board's
    /// paged memory, MOV m, imm, MOVZX, MOVSXD, LEA) in the unified run: with the fast path the run
    /// and the state are those of the run without it and every move is taken (debug builds count
    /// them); under the check every move is compared with exactly the seven data calls of the
    /// general path through the board adapter, and nothing diverges.
    #[test]
    fn the_unified_run_takes_the_g4_moves_and_the_check_agrees() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // mov [rsp-8], rax; mov rcx, [rsp-8]; mov byte [rsp-0x10], 0x5a;
        // movzx edx, byte [rsp-0x10]; movsxd rdi, dword [rsp-8]; lea rsi, [rsp-0x20];
        // mov dword [rsi], 0x12345678; mov eax, [rsi]; hlt
        let program = [
            0x48, 0x89, 0x44, 0x24, 0xF8, 0x48, 0x8B, 0x4C, 0x24, 0xF8, 0xC6, 0x44, 0x24, 0xF0,
            0x5A, 0x0F, 0xB6, 0x54, 0x24, 0xF0, 0x48, 0x63, 0x7C, 0x24, 0xF8, 0x48, 0x8D, 0x74,
            0x24, 0xE0, 0xC7, 0x06, 0x78, 0x56, 0x34, 0x12, 0x8B, 0x06, 0xF4,
        ];
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let run = |fast_path: bool, verify: bool| {
            let mut board = load_firmware_image(&image, &linux_config()).unwrap();
            fast_path_check::set_mode(FastPathMode::Tree);
            fast_path_check::reset_fast_steps_taken();
            let run = crate::run_firmware_unified_with_fast_path(
                &mut board,
                1_000,
                None,
                Vec::new(),
                None,
                false,
                verify,
                fast_path,
            )
            .unwrap();
            (run, board, fast_path_check::fast_steps_taken())
        };
        let (without, plain, _) = run(false, false);
        assert_eq!(without.run.retired, 9);
        assert_eq!(plain.state().gpr[2], 0x5a);
        assert_eq!(plain.state().gpr[0], 0x1234_5678);
        let (with, fast, taken_with) = run(true, false);
        assert_eq!(with.run, without.run);
        assert_eq!(fast.state(), plain.state());
        if od_isa::fast_path_check::FAST_STEPS_COUNTED {
            assert_eq!(taken_with, 8);
        }
        let (checked, board, _) = run(false, true);
        let report = checked
            .fast_path_check
            .clone()
            .expect("the check was asked for");
        assert_eq!(report.divergence, None);
        assert_eq!(report.counters.compared, 8, "{report:?}");
        assert_eq!(report.counters.memory_calls_compared, 7, "{report:?}");
        assert_eq!(checked.run, without.run);
        assert_eq!(board.state(), plain.state());
    }

    /// PERF-11 (ADR-0016, prototype): with the data translation cache the unified run, the state and
    /// COM1 are those of the run without it, with and without the fast path; the cache really serves
    /// the stack qword reads (debug builds count the hits) and is not used unless asked for.
    #[test]
    fn the_unified_run_with_the_data_cache_prototype_equals_the_run_without_it() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // mov [rsp-8], rax; mov rcx, [rsp-8]; mov rdx, [rsp-8]; mov rbx, [rsp-8]; push rcx;
        // pop rsi; mov rdi, [rsp-8]; hlt
        let program = [
            0x48, 0x89, 0x44, 0x24, 0xF8, 0x48, 0x8B, 0x4C, 0x24, 0xF8, 0x48, 0x8B, 0x54, 0x24,
            0xF8, 0x48, 0x8B, 0x5C, 0x24, 0xF8, 0x51, 0x5E, 0x48, 0x8B, 0x7C, 0x24, 0xF8, 0xF4,
        ];
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let run = |fast_path: bool, data_cache: bool| {
            let mut board = load_firmware_image(&image, &linux_config()).unwrap();
            if data_cache {
                board.enable_data_cache_prototype();
            }
            fast_path_check::set_mode(FastPathMode::Tree);
            crate::reset_data_cache_counters();
            let run = crate::run_firmware_unified_with_fast_path(
                &mut board,
                1_000,
                None,
                Vec::new(),
                None,
                false,
                false,
                fast_path,
            )
            .unwrap();
            (run, board, crate::data_cache_counters())
        };
        let (without, plain, counters) = run(false, false);
        assert_eq!(counters, crate::DataCacheCounters::default());
        assert_eq!(without.run.retired, 8);
        for fast_path in [false, true] {
            let (with, cached, counters) = run(fast_path, true);
            assert_eq!(with.run, without.run, "fast path {fast_path}");
            assert_eq!(cached.state(), plain.state(), "fast path {fast_path}");
            assert_eq!(cached.com1_transcript(), plain.com1_transcript());
            if crate::DATA_CACHE_COUNTED {
                // The first read walks; the other stack reads of the same page hit.
                assert!(counters.inserts >= 1, "{counters:?}");
                assert!(counters.hits >= 3, "{counters:?}");
            }
        }
    }

    /// PERF-11 (ADR-0016, prototype): the invalidation wiring of the loop. The program reads a
    /// qword of the page X (a miss, then a hit), writes a new PTE of X through a mapping of its own
    /// page table, reads X again (the cache must have dropped the entry: the new frame), switches CR3
    /// to a second root whose PT maps X elsewhere and reads X again. With the cache the run, the
    /// state, COM1 and the RAM are those of the run without it, and the cache served the repeated
    /// reads (debug builds count the hits).
    #[test]
    fn the_data_cache_prototype_sees_a_guest_pte_store_and_a_cr3_switch_in_the_unified_run() {
        const ROOT_A: u64 = 0x90_0000;
        const ROOT_B: u64 = 0x91_0000;
        const X: u64 = 0x40_0000;
        const VIEW: u64 = 0x40_1000;
        let frames = [0x98_0000u64, 0x98_1000, 0x98_2000];
        let values = [
            0x1111_2222_3333_4444u64,
            0x5555_6666_7777_8888,
            0x9999_aaaa_bbbb_cccc,
        ];
        let mut code = Vec::new();
        let read_x = |code: &mut Vec<u8>, rex: u8, modrm: u8| {
            code.extend_from_slice(&[rex, 0x8B, modrm, 0x25]);
            code.extend_from_slice(&(X as u32).to_le_bytes());
        };
        read_x(&mut code, 0x48, 0x04); // mov rax, [X]
        read_x(&mut code, 0x4C, 0x04); // mov r8, [X]
        // mov rcx, frames[1] | 3; mov [VIEW], rcx (PTE[0] of X through PT[1])
        code.extend_from_slice(&[0x48, 0xC7, 0xC1]);
        code.extend_from_slice(&((frames[1] | 3) as u32).to_le_bytes());
        code.extend_from_slice(&[0x48, 0x89, 0x0C, 0x25]);
        code.extend_from_slice(&(VIEW as u32).to_le_bytes());
        read_x(&mut code, 0x4C, 0x0C); // mov r9, [X]
        read_x(&mut code, 0x4C, 0x14); // mov r10, [X]
        // mov rdx, ROOT_B; mov cr3, rdx
        code.extend_from_slice(&[0x48, 0xC7, 0xC2]);
        code.extend_from_slice(&(ROOT_B as u32).to_le_bytes());
        code.extend_from_slice(&[0x0F, 0x22, 0xDA]);
        read_x(&mut code, 0x4C, 0x1C); // mov r11, [X]
        read_x(&mut code, 0x4C, 0x24); // mov r12, [X]
        code.push(0xF4);
        let board = |data_cache: bool| {
            let mut board = load_firmware_image(&minor2_image(&code), &linux_config()).unwrap();
            let mut put = |address: u64, value: u64| {
                board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(address),
                        AccessWidth::Qword,
                        value,
                        |_| {},
                    )
                    .unwrap();
            };
            // Two roots: 2 MiB identity leaves for the first 4 MiB (code, stack, IDT, GDT) and a
            // 4 KiB PT for X whose [0] maps the data frame and [1] the PT itself.
            for (root, frame) in [(ROOT_A, frames[0]), (ROOT_B, frames[2])] {
                put(root, (root + 0x1000) | 3);
                put(root + 0x1000, (root + 0x2000) | 3);
                put(root + 0x2000, 0x83);
                put(root + 0x2008, 0x20_0000 | 0x83);
                put(root + 0x2010, (root + 0x3000) | 3);
                put(root + 0x3000, frame | 3);
                // A and D already set: the walk through [1] for the store to VIEW changes nothing,
                // so the only change of the PT is the guest's store to [0].
                put(root + 0x3008, (root + 0x3000) | 0x63);
            }
            for (frame, value) in frames.iter().zip(values) {
                put(*frame, value);
            }
            board.page_table_root = ROOT_A;
            if data_cache {
                board.enable_data_cache_prototype();
            }
            board
        };
        let run = |data_cache: bool| {
            let mut board = board(data_cache);
            crate::reset_data_cache_counters();
            let run = crate::run_firmware_unified(&mut board, 1_000).unwrap();
            (run, board, crate::data_cache_counters())
        };
        let (without, plain, counters) = run(false);
        assert_eq!(counters, crate::DataCacheCounters::default());
        assert_eq!(without.run.exit, PagedProbeExit::Halted, "{without:?}");
        assert_eq!(without.run.retired, 11, "{without:?}");
        use od_isa::Register::{R8, R9, R10, R11, R12, Rax};
        let gpr = |board: &LoadedFirmwareBoard, register: od_isa::Register| {
            board.state().gpr[register as usize]
        };
        assert_eq!(
            [Rax, R8, R9, R10, R11, R12].map(|register| gpr(&plain, register)),
            [
                values[0], values[0], values[1], values[1], values[2], values[2]
            ]
        );
        let (with, cached, counters) = run(true);
        assert_eq!(format!("{with:?}"), format!("{without:?}"));
        assert_eq!(cached.state(), plain.state());
        assert_eq!(cached.com1_transcript(), plain.com1_transcript());
        assert_eq!(perf25_ram(&cached), perf25_ram(&plain));
        if crate::DATA_CACHE_COUNTED {
            // The second read of each pair hits; the PTE store and MOV CR3 drop the entry.
            assert_eq!(counters.hits, 3, "{counters:?}");
            assert_eq!(counters.inserts, 3, "{counters:?}");
            assert_eq!(counters.context_flushes, 1, "MOV CR3: {counters:?}");
        }
    }

    /// PERF-25, group G5: ALU with memory on the stack, PUSH/POP, CALL and RET in the unified run
    /// over the board's paged memory: with the fast path the run and the state are those of the run
    /// without it and every G5 form is taken (debug builds count them); under the check every one is
    /// compared with exactly the nine data calls of the general path through the board adapter, and
    /// nothing diverges.
    #[test]
    fn the_unified_run_takes_the_g5_forms_and_the_check_agrees() {
        use od_isa::fast_path_check::{self, FastPathMode};
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        // mov eax, 0x11; push rax; add qword [rsp], 5; cmp qword [rsp], 0x16; pop rcx; call +1;
        // hlt (the return lands here); add rax, [rsp]; sub rax, [rsp]; ret
        let program = [
            0xB8, 0x11, 0x00, 0x00, 0x00, 0x50, 0x48, 0x83, 0x04, 0x24, 0x05, 0x48, 0x83, 0x3C,
            0x24, 0x16, 0x59, 0xE8, 0x01, 0x00, 0x00, 0x00, 0xF4, 0x48, 0x03, 0x04, 0x24, 0x48,
            0x2B, 0x04, 0x24, 0xC3,
        ];
        image[0x40..0x40 + program.len()].copy_from_slice(&program);
        seal(&mut image);
        let run = |fast_path: bool, verify: bool| {
            let mut board = load_firmware_image(&image, &linux_config()).unwrap();
            fast_path_check::set_mode(FastPathMode::Tree);
            fast_path_check::reset_fast_steps_taken();
            let run = crate::run_firmware_unified_with_fast_path(
                &mut board,
                1_000,
                None,
                Vec::new(),
                None,
                false,
                verify,
                fast_path,
            )
            .unwrap();
            (run, board, fast_path_check::fast_steps_taken())
        };
        let (without, plain, _) = run(false, false);
        assert_eq!(without.run.retired, 10);
        assert_eq!(plain.state().gpr[1], 0x16);
        assert_eq!(plain.state().gpr[0], 0x11);
        let (with, fast, taken_with) = run(true, false);
        assert_eq!(with.run, without.run);
        assert_eq!(fast.state(), plain.state());
        if od_isa::fast_path_check::FAST_STEPS_COUNTED {
            // Everything but the HLT (the MOV is G1).
            assert_eq!(taken_with, 9);
        }
        let (checked, board, _) = run(false, true);
        let report = checked
            .fast_path_check
            .clone()
            .expect("the check was asked for");
        assert_eq!(report.divergence, None);
        assert_eq!(report.counters.compared, 9, "{report:?}");
        assert_eq!(report.counters.memory_calls_compared, 9, "{report:?}");
        assert_eq!(checked.run, without.run);
        assert_eq!(board.state(), plain.state());
    }

    /// PERF-22: a wrong draft handler (a NOP that moves RIP one byte too far) is found by the
    /// check in the unified loop at the first instruction it takes, and the divergence ends the run
    /// and is reported with the RIP and the bytes of the instruction.
    #[test]
    fn a_fast_path_check_stops_the_unified_run_at_a_wrong_draft_handler() {
        use od_isa::execute::StepOutcome;
        use od_isa::fast_path_check::{
            self, CheckMemory, DraftFastStep, FastPathMode, FastStepResult,
        };
        use od_isa::long_decode::{Instruction, Operation};
        fn wrong_nop(
            state: &mut od_isa::LongModeState,
            _: &mut CheckMemory<'_>,
            _: od_isa::AccessContext,
            _: &mut u64,
            instruction: &Instruction,
            _: bool,
        ) -> FastStepResult {
            matches!(instruction.operation, Operation::Nop).then(|| {
                state.rip = state.rip.wrapping_add(u64::from(instruction.length) + 1);
                Ok(StepOutcome::Advanced)
            })
        }
        let mut image = minor2_image(&vec![0x90u8; 0x80]);
        image[0x40..0x43].copy_from_slice(&[0x90, 0x50, 0xF4]); // nop; push rax; hlt
        seal(&mut image);
        let entry = load_firmware_image(&image, &linux_config())
            .unwrap()
            .state()
            .rip;
        let mut board = load_firmware_image(&image, &linux_config()).unwrap();
        fast_path_check::set_mode(FastPathMode::Custom(wrong_nop as DraftFastStep));
        let run = crate::run_firmware_unified_checked(
            &mut board,
            1_000,
            None,
            Vec::new(),
            None,
            false,
            true,
        )
        .unwrap();
        fast_path_check::set_mode(FastPathMode::Tree);
        let report = run.fast_path_check.expect("the check was asked for");
        let divergence = report.divergence.expect("the wrong NOP is found");
        assert_eq!(
            (divergence.rip, divergence.bytes.as_slice()),
            (entry, &[0x90][..])
        );
        assert!(
            divergence
                .differences
                .iter()
                .any(|line| line.starts_with("state.rip:")),
            "{divergence}"
        );
        assert_eq!(report.counters.divergences, 1);
        // The run stopped at the divergent instruction: the push after it never retired.
        assert!(run.run.retired <= 1, "{:?}", run.run);
    }

    /// S7-31q: a runtime services table slot is followed once a traced call refreshed it, a NULL
    /// GUID pointer is no GUID, and the function behind the slot is traced under the slot's name.
    #[test]
    fn a_call_trace_follows_a_runtime_services_slot_and_skips_a_null_guid() {
        let (service, dynamic, system_table, runtime_table) =
            (0x40usize, 0x60usize, 0x100usize, 0x140usize);
        let mut code = vec![0x90u8; 0x200];
        code[service] = 0xC3;
        code[dynamic] = 0xC3;
        code[0x1C0..0x1D0].copy_from_slice(&[7; 16]);
        code[0x1E0..0x1E6].copy_from_slice(&[b'A', 0, b'b', 0, 0, 0]);
        let image = minor2_image(&code);
        let entry = load_firmware_image(&image, &linux_config())
            .unwrap()
            .state()
            .rip;
        let put = |code: &mut Vec<u8>, at: usize, value: u64| {
            code[at..at + 8].copy_from_slice(&value.to_le_bytes())
        };
        put(&mut code, system_table + 0x58, entry + runtime_table as u64);
        put(&mut code, runtime_table + 72, entry + dynamic as u64);
        // mov edx, GUID; mov ecx, NAME; call service; call dynamic; xor edx, edx; call dynamic; hlt
        let mut program = vec![0xBA];
        program.extend_from_slice(&((entry as u32) + 0x1C0).to_le_bytes());
        program.push(0xB9);
        program.extend_from_slice(&((entry as u32) + 0x1E0).to_le_bytes());
        for (target, next) in [(service, 15usize), (dynamic, 20)] {
            program.push(0xE8);
            program.extend_from_slice(&((target as i32) - next as i32).to_le_bytes());
        }
        program.extend_from_slice(&[0x31, 0xD2, 0xE8]);
        program.extend_from_slice(&((dynamic as i32) - 27).to_le_bytes());
        program.push(0xF4);
        code[..program.len()].copy_from_slice(&program);
        let mut image = minor2_image(&code);
        seal(&mut image);
        let mut board = load_firmware_image(&image, &linux_config()).unwrap();
        let targets = vec![
            crate::CallTraceTarget {
                name: "service".into(),
                physical: entry + service as u64,
                guid_arg: None,
                name_arg: None,
                rt_slot: None,
            },
            crate::CallTraceTarget {
                name: "GetVariable".into(),
                physical: 0,
                guid_arg: Some(1),
                name_arg: Some(0),
                rt_slot: Some(72),
            },
        ];
        let run = crate::run_firmware_unified_traced(
            &mut board,
            1_000,
            None,
            targets,
            Some(entry + system_table as u64),
        )
        .unwrap();
        let trace = run.call_trace.unwrap();
        let calls: Vec<_> = trace
            .entries
            .iter()
            .map(|call| {
                (
                    call.name.as_str(),
                    call.guid.is_some(),
                    call.variable_name.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            calls,
            [
                ("service", false, None),
                ("GetVariable", true, Some("Ab")),
                ("GetVariable", false, Some("Ab"))
            ],
            "{trace:?}"
        );
        assert_eq!(trace.entries[1].guid, Some([7; 16]));
        assert_eq!(trace.total, 3);
    }

    /// S7-31m (ISSUE-640): the kernel's idle loop in miniature. The code programs the 8259
    /// (vector base 0x30, IR0 unmasked) and PIT channel 0 (mode 2, reload 1193), then runs
    /// `sti; hlt`. The unified loop wakes the halted CPU at the timer edge and delivers vector
    /// 0x30 through the guest IDT; the handler's own `hlt` runs at IF = 0 (an interrupt gate
    /// clears IF), so the run ends halted with the reason `if_clear`.
    #[test]
    fn unified_hlt_at_if_set_is_woken_by_the_pit_and_hlt_at_if_clear_ends_the_run() {
        const IDT: u64 = 0x2000;
        const HANDLER_OFFSET: usize = 0x80;
        // `mov al, imm8; out imm8, al` pairs: ICW1..ICW4 and OCW1 of the master 8259, then the
        // PIT control word (channel 0, LSB+MSB, mode 2) and the reload 0x04A9 (1193).
        let mut code = Vec::new();
        for (port, value) in [
            (0x20u8, 0x11u8),
            (0x21, 0x30),
            (0x21, 0x04),
            (0x21, 0x01),
            (0x21, 0xFE),
            (0x43, 0x34),
            (0x40, 0xA9),
            (0x40, 0x04),
        ] {
            code.extend_from_slice(&[0xB0, value, 0xE6, port]);
        }
        code.extend_from_slice(&[0xFB, 0xF4]); // sti; hlt
        let wake_rip_offset = code.len();
        code.resize(HANDLER_OFFSET, 0x90);
        // Handler: EOI to the master (`mov al, 0x20; out 0x20, al`), so IR0 can fire again, then
        // `hlt` at IF = 0: the source has the next edge, yet an interrupt gate cleared IF.
        code.extend_from_slice(&[0xB0, 0x20, 0xE6, 0x20, 0xF4]);
        let mut board = load_firmware_image(&minor2_image(&code), &linux_config()).unwrap();
        let entry = board.state().rip;
        let handler = entry + HANDLER_OFFSET as u64;
        let mut write = |address: u64, bytes: &[u8]| {
            for (index, byte) in bytes.iter().enumerate() {
                board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(address + index as u64),
                        AccessWidth::Byte,
                        u64::from(*byte),
                        |_| {},
                    )
                    .unwrap();
            }
        };
        // GDT entry 1: a 64-bit ring-0 code segment; IDT gate 0x30: an interrupt gate to it.
        write(8, &[0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0]);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&8u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
        gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
        write(IDT + 0x30 * 16, &gate);
        let state = board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: IDT,
            limit: 0xFFF,
        });
        // The ROM loads CS from its own GDT; this entry state models the same segment.
        state.cs_selector = Some(8);
        state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 0xA,
        });
        let run = crate::run_firmware_unified(&mut board, 100_000).unwrap();
        let interrupts = &run.interrupts;
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        assert_eq!(
            board.state().rip,
            handler + 5,
            "stopped after the handler's hlt"
        );
        assert!(wake_rip_offset < HANDLER_OFFSET);
        assert_eq!(interrupts.irqs_delivered, 1);
        assert_eq!(interrupts.halt_wakes, 1);
        assert!(interrupts.irq0_edges >= 1);
        assert!(!interrupts.interrupt_flag, "the interrupt gate cleared IF");
        assert_eq!(interrupts.halt_stop, Some("if_clear"));
        assert_eq!(interrupts.source, "pit_pic");
        assert_eq!(interrupts.pit_channel0_programmed, Some(true));
        // The handler's EOI cleared the in-service bit; the timer has a next edge (a wake is
        // available) and the run still ends: IF = 0 gives no delivery and no second wake.
        assert_eq!(interrupts.pic_isr, Some(0));
        assert!(board.irq_wake_ns().is_some());
        assert_eq!(interrupts.pic_icw2, Some(0x30));
    }

    /// PERF-05: the guest unmasks IR0 by a write to port 0x21 while the edge sits in the IRR; the
    /// vector is delivered at the very boundary after the `out`, not later: the instruction after
    /// it (`mov ebx, 0x1234`) has not run when the handler starts. The saved answer of the source
    /// (nothing pending while IR0 was masked) must not survive the write to the 8259.
    #[test]
    fn unmasking_ir0_by_a_port_write_delivers_at_the_boundary_after_the_out() {
        const IDT: u64 = 0x2000;
        const HANDLER_OFFSET: usize = 0x80;
        let mut code = Vec::new();
        // The master 8259 with every line masked (OCW1 0xFF), then PIT channel 0 in mode 2 with a
        // reload of 2 (an edge every ~1.7 us of virtual time).
        for (port, value) in [
            (0x20u8, 0x11u8),
            (0x21, 0x30),
            (0x21, 0x04),
            (0x21, 0x01),
            (0x21, 0xFF),
            (0x43, 0x34),
            (0x40, 0x02),
            (0x40, 0x00),
        ] {
            code.extend_from_slice(&[0xB0, value, 0xE6, port]);
        }
        code.push(0xFB); // sti
        // A delay of ~6000 instructions: `mov ecx, 3000; dec ecx; jnz -4`. Edges latch in the IRR.
        code.extend_from_slice(&[0xB9, 0xB8, 0x0B, 0x00, 0x00, 0xFF, 0xC9, 0x75, 0xFC]);
        code.extend_from_slice(&[0xB0, 0xFE, 0xE6, 0x21]); // unmask IR0
        code.extend_from_slice(&[0xBB, 0x34, 0x12, 0x00, 0x00]); // mov ebx, 0x1234
        code.push(0xF4); // hlt
        assert!(code.len() < HANDLER_OFFSET);
        code.resize(HANDLER_OFFSET, 0x90);
        code.extend_from_slice(&[0xB0, 0x20, 0xE6, 0x20, 0xF4]); // EOI; hlt at IF = 0
        let mut board = load_firmware_image(&minor2_image(&code), &linux_config()).unwrap();
        let entry = board.state().rip;
        let handler = entry + HANDLER_OFFSET as u64;
        let mut write = |address: u64, bytes: &[u8]| {
            for (index, byte) in bytes.iter().enumerate() {
                board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(address + index as u64),
                        AccessWidth::Byte,
                        u64::from(*byte),
                        |_| {},
                    )
                    .unwrap();
            }
        };
        write(8, &[0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0]);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&8u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
        gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
        write(IDT + 0x30 * 16, &gate);
        let state = board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: IDT,
            limit: 0xFFF,
        });
        state.cs_selector = Some(8);
        state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 0xA,
        });
        let run = crate::run_firmware_unified(&mut board, 100_000).unwrap();
        let interrupts = &run.interrupts;
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        assert_eq!(interrupts.irqs_delivered, 1, "{run:?}");
        assert_eq!(interrupts.halt_stop, Some("if_clear"));
        assert_eq!(
            board.state().rip,
            handler + 5,
            "stopped after the handler's hlt"
        );
        assert_eq!(
            board.state().gpr[od_isa::Register::Rbx as usize],
            0,
            "the delivery came before the instruction after the out"
        );
        assert_eq!(
            board.state().gpr[od_isa::Register::Rcx as usize],
            0,
            "the delay ran out"
        );
        assert!(interrupts.irq0_edges >= 1);
    }

    // ---- PERF-25: batched calls of the set in the unified loop ----

    /// A unified-run board for `code` with the GDT entry 1 (64-bit ring-0 code), the IDT at 0x2000
    /// with an interrupt gate for vector 0x30 to `code[handler_offset..]`, and the entry state
    /// on that segment (as the PIT -> 8259 tests above build it).
    fn perf25_irq_board(code: &[u8], handler_offset: usize) -> LoadedFirmwareBoard {
        perf25_board(code, &[(0x30, handler_offset)])
    }

    /// `perf25_irq_board` with an interrupt gate for every `(vector, offset)` of `gates` (to
    /// `code[offset..]`), and GDT entries 2 (ring-0 data, 0x10), 4 (ring-3 data, 0x23) and 5
    /// (64-bit ring-3 code, 0x2B) next to entry 1.
    fn perf25_board(code: &[u8], gates: &[(u8, usize)]) -> LoadedFirmwareBoard {
        const IDT: u64 = 0x2000;
        let mut board = load_firmware_image(&minor2_image(code), &linux_config()).unwrap();
        let entry = board.state().rip;
        let mut write = |address: u64, bytes: &[u8]| {
            for (index, byte) in bytes.iter().enumerate() {
                board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(address + index as u64),
                        AccessWidth::Byte,
                        u64::from(*byte),
                        |_| {},
                    )
                    .unwrap();
            }
        };
        write(8, &[0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0]);
        write(0x10, &[0xFF, 0xFF, 0, 0, 0, 0x93, 0xCF, 0]);
        write(0x20, &[0xFF, 0xFF, 0, 0, 0, 0xF3, 0xCF, 0]);
        write(0x28, &[0xFF, 0xFF, 0, 0, 0, 0xFB, 0xAF, 0]);
        for &(vector, offset) in gates {
            let handler = entry + offset as u64;
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&8u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
            gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
            write(IDT + u64::from(vector) * 16, &gate);
        }
        let state = board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: IDT,
            limit: 0xFFF,
        });
        state.cs_selector = Some(8);
        state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 0xA,
        });
        board
    }

    /// The 8259 programmed (vector base 0x30, IMR `imr`) and PIT channel 0 in mode 2 with
    /// `reload`, as `mov al, imm8; out imm8, al` pairs.
    fn perf25_pic_pit_code(imr: u8, reload: u16) -> Vec<u8> {
        let mut code = Vec::new();
        for (port, value) in [
            (0x20u8, 0x11u8),
            (0x21, 0x30),
            (0x21, 0x04),
            (0x21, 0x01),
            (0x21, imr),
            (0x43, 0x34),
            (0x40, reload as u8),
            (0x40, (reload >> 8) as u8),
        ] {
            code.extend_from_slice(&[0xB0, value, 0xE6, port]);
        }
        code
    }

    /// The unified run of `board()` with and without batched calls: the whole report, the state and
    /// the RAM below 16 MiB (the stack, the boot page tables) must be equal; the calls of the
    /// batched run are returned.
    fn perf25_compare(
        board: impl Fn() -> LoadedFirmwareBoard,
        budget: u64,
    ) -> (
        crate::FirmwareUnifiedRun,
        Vec<crate::paged_memory_adapter::batch_trace::Call>,
    ) {
        let (mut runs, calls) = perf25_compare_runs(board, &[budget]);
        (runs.pop().expect("one run"), calls)
    }

    /// The RAM pages below 16 MiB (`None` for a page that is not RAM).
    fn perf25_ram(board: &LoadedFirmwareBoard) -> Vec<Option<Vec<u8>>> {
        (0..0x100_0000u64)
            .step_by(0x1000)
            .map(|address| {
                let mut page = vec![0u8; 0x1000];
                board
                    .bus()
                    .physical_memory()
                    .load_bytes(GuestPhysicalAddress::new(address), &mut page)
                    .ok()
                    .map(|()| page)
            })
            .collect()
    }

    /// `perf25_compare` for consecutive unified runs of the same board with `budgets`: every
    /// report, and after the last run the state and the RAM, must be equal.
    fn perf25_compare_runs(
        board: impl Fn() -> LoadedFirmwareBoard,
        budgets: &[u64],
    ) -> (
        Vec<crate::FirmwareUnifiedRun>,
        Vec<crate::paged_memory_adapter::batch_trace::Call>,
    ) {
        let ram = perf25_ram;
        let mut single = board();
        single.set_instruction_batches(false);
        crate::paged_memory_adapter::batch_trace::start();
        let one: Vec<_> = budgets
            .iter()
            .map(|&budget| crate::run_firmware_unified(&mut single, budget).unwrap())
            .collect();
        let single_calls = crate::paged_memory_adapter::batch_trace::take();
        assert!(single_calls.iter().all(|call| call.budget == 1));
        let mut batched = board();
        crate::paged_memory_adapter::batch_trace::start();
        let many: Vec<_> = budgets
            .iter()
            .map(|&budget| crate::run_firmware_unified(&mut batched, budget).unwrap())
            .collect();
        let calls = crate::paged_memory_adapter::batch_trace::take();
        assert_eq!(format!("{many:?}"), format!("{one:?}"));
        assert_eq!(batched.state(), single.state());
        assert_eq!(ram(&batched), ram(&single));
        // The batched run retired the same instructions in fewer calls.
        assert_eq!(
            calls.iter().map(|call| call.retired).sum::<u64>(),
            single_calls.iter().map(|call| call.retired).sum::<u64>()
        );
        assert!(calls.len() < single_calls.len(), "{calls:?}");
        (many, calls)
    }

    #[test]
    fn batched_calls_end_the_unified_run_like_one_instruction_calls_and_after_each_out() {
        // Straight code, then the PIT/8259 setup (eight OUTs), STI, a delay loop and HLT at IF = 1
        // (woken by the timer; the handler's own HLT at IF = 0 ends the run).
        const HANDLER_OFFSET: usize = 0xC0;
        let mut code = vec![0x90; 20];
        let outs_start = code.len();
        code.extend_from_slice(&perf25_pic_pit_code(0xFE, 0x04A9));
        let sti_offset = code.len();
        code.push(0xFB);
        // mov ecx, 3000; dec ecx; jnz -4
        code.extend_from_slice(&[0xB9, 0xB8, 0x0B, 0x00, 0x00, 0xFF, 0xC9, 0x75, 0xFC]);
        code.push(0xF4);
        code.resize(HANDLER_OFFSET, 0x90);
        code.extend_from_slice(&[0xB0, 0x20, 0xE6, 0x20, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_irq_board(&code, HANDLER_OFFSET), 100_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted);
        assert_eq!(run.interrupts.irqs_delivered, 1);
        let entry = perf25_irq_board(&code, HANDLER_OFFSET).state().rip;
        // The first call runs the NOPs and the first `mov al`, and ends right after the first
        // OUT (a device access); every OUT of the setup ends its call.
        assert_eq!(
            calls[0],
            crate::paged_memory_adapter::batch_trace::Call {
                budget: calls[0].budget,
                retired: 22,
                rip: entry + outs_start as u64 + 4,
            }
        );
        assert!(calls[0].budget > 22);
        for pair in 1..8 {
            assert_eq!(calls[pair].retired, 2, "{calls:?}");
            assert_eq!(
                calls[pair].rip,
                entry + (outs_start + 4 * (pair + 1)) as u64,
                "{calls:?}"
            );
        }
        // STI changes IF and opens the shadow: its call ends right after it; the next instruction
        // closes the shadow and ends its call too.
        assert_eq!(
            (calls[8].retired, calls[8].rip),
            (1, entry + sti_offset as u64 + 1)
        );
        assert_eq!(
            (calls[9].retired, calls[9].rip),
            (1, entry + sti_offset as u64 + 6)
        );
        // The delay loop runs in a few calls (the quantum ends at the timer deadline).
        assert!(calls[10].retired > 1_000, "{calls:?}");
    }

    #[test]
    fn a_vector_unmasked_by_an_out_is_delivered_at_the_boundary_after_it_in_a_batched_run() {
        // PERF-05's case: the edges latch while IR0 is masked; the OUT that unmasks it ends the
        // batched call, and the vector goes in before `mov ebx, 0x1234`.
        const HANDLER_OFFSET: usize = 0x80;
        let mut code = perf25_pic_pit_code(0xFF, 2);
        code.push(0xFB);
        code.extend_from_slice(&[0xB9, 0xB8, 0x0B, 0x00, 0x00, 0xFF, 0xC9, 0x75, 0xFC]);
        let unmask = code.len();
        code.extend_from_slice(&[0xB0, 0xFE, 0xE6, 0x21]);
        code.extend_from_slice(&[0xBB, 0x34, 0x12, 0x00, 0x00, 0xF4]);
        code.resize(HANDLER_OFFSET, 0x90);
        code.extend_from_slice(&[0xB0, 0x20, 0xE6, 0x20, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_irq_board(&code, HANDLER_OFFSET), 100_000);
        assert_eq!(run.interrupts.irqs_delivered, 1);
        let entry = perf25_irq_board(&code, HANDLER_OFFSET).state().rip;
        let after_unmask = entry + unmask as u64 + 4;
        let index = calls
            .iter()
            .position(|call| call.rip == after_unmask)
            .expect("a call ends right after the unmasking OUT");
        // The next call starts in the handler: the delivery came at that boundary.
        let handler = entry + HANDLER_OFFSET as u64;
        assert_eq!(calls[index + 1].rip, handler + 4, "{calls:?}");
        assert_eq!(calls[index + 1].retired, 2);
    }

    #[test]
    fn refusals_of_a_pending_vector_are_counted_at_every_boundary_inside_a_batched_call() {
        // IR0 unmasked and a fast timer, but IF = 0 over a delay loop: the vector is pending at every
        // boundary, refused (masked) each time, inside the batched calls too; then STI: refused once
        // in the shadow, delivered at the boundary after it.
        const HANDLER_OFFSET: usize = 0x80;
        let mut code = vec![0xFA]; // cli
        code.extend_from_slice(&perf25_pic_pit_code(0xFE, 2));
        code.extend_from_slice(&[0xB9, 0xB8, 0x0B, 0x00, 0x00, 0xFF, 0xC9, 0x75, 0xFC]);
        code.extend_from_slice(&[0xFB, 0x90, 0xF4]); // sti; nop; hlt
        code.resize(HANDLER_OFFSET, 0x90);
        code.extend_from_slice(&[0xB0, 0x20, 0xE6, 0x20, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_irq_board(&code, HANDLER_OFFSET), 100_000);
        let interrupts = &run.interrupts;
        assert_eq!(interrupts.irqs_delivered, 1, "{run:?}");
        assert!(interrupts.irq_refused_masked > 1_000, "{interrupts:?}");
        assert_eq!(interrupts.irq_refused_shadow, 1, "{interrupts:?}");
        // The masked stretch ran in calls of many instructions.
        assert!(calls.iter().any(|call| call.retired > 1_000), "{calls:?}");
    }

    #[test]
    fn every_rdtsc_ends_its_batched_call_and_quantum_like_one_instruction_calls() {
        // NOPs between the reads: the RDTSC tick ends the quantum right after each RDTSC.
        let mut code = Vec::new();
        for _ in 0..4 {
            code.extend_from_slice(&[0x90, 0x90, 0x90, 0x0F, 0x31]);
        }
        code.push(0xF4);
        let (run, calls) = perf25_compare(
            || load_firmware_image(&minor2_image(&code), &linux_config()).unwrap(),
            1_000,
        );
        assert_eq!(run.run.exit, PagedProbeExit::Halted);
        let entry = load_firmware_image(&minor2_image(&code), &linux_config())
            .unwrap()
            .state()
            .rip;
        let ends: Vec<_> = calls
            .iter()
            .map(|call| (call.retired, call.rip - entry))
            .collect();
        assert_eq!(ends, [(4, 5), (4, 10), (4, 15), (4, 20), (1, 21)]);
    }

    /// The entry RIP of every `perf25_board` (the image layout does not depend on the code).
    fn perf25_entry() -> u64 {
        perf25_board(&[0xF4], &[]).state().rip
    }

    /// `code` with `HLT` at `handler` (an interrupt gate leaves IF = 0: the run ends there).
    fn perf25_with_handler(mut code: Vec<u8>, handler: usize) -> Vec<u8> {
        assert!(code.len() <= handler);
        code.resize(handler, 0x90);
        code.push(0xF4);
        code
    }

    #[test]
    fn a_general_protection_fault_inside_a_batched_call_counts_the_instructions_before_it() {
        // Five NOPs and `mov ecx, 0x1234`, then `RDMSR` of an MSR the set does not model: `#GP(0)`
        // through the gate of vector 13 to a HLT. The batched call retires the six instructions
        // and ends with the fault; they count as in six one-instruction calls.
        const HANDLER: usize = 0x40;
        let mut code = vec![0x90; 5];
        code.extend_from_slice(&[0xB9, 0x34, 0x12, 0x00, 0x00]);
        let rdmsr = code.len();
        code.extend_from_slice(&[0x0F, 0x32, 0xF4]);
        let code = perf25_with_handler(code, HANDLER);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[(13, HANDLER)]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        assert_eq!(
            (run.exceptions_delivered, run.page_faults_delivered),
            (1, 0)
        );
        assert_eq!(
            run.run.retired, 7,
            "six before the fault and the handler's HLT"
        );
        let entry = perf25_entry();
        assert_eq!((calls[0].retired, calls[0].rip), (6, entry + rdmsr as u64));
        assert!(calls[0].budget > 6, "{calls:?}");
        assert_eq!(calls[1].rip, entry + HANDLER as u64 + 1, "{calls:?}");
    }

    #[test]
    fn a_page_fault_inside_a_batched_call_counts_the_instructions_before_it() {
        // Five NOPs and `mov rax, 0x4000_0000_0000`, then a load from that unmapped address: `#PF`
        // through the gate of vector 14 to a HLT.
        const HANDLER: usize = 0x40;
        let mut code = vec![0x90; 5];
        code.extend_from_slice(&[0x48, 0xB8, 0, 0, 0, 0, 0, 0x40, 0, 0]);
        let load = code.len();
        code.extend_from_slice(&[0x48, 0x8B, 0x18, 0xF4]);
        let code = perf25_with_handler(code, HANDLER);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[(14, HANDLER)]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        assert_eq!(
            (run.exceptions_delivered, run.page_faults_delivered),
            (1, 1)
        );
        assert_eq!(
            run.run.retired, 7,
            "six before the fault and the handler's HLT"
        );
        let entry = perf25_entry();
        assert_eq!((calls[0].retired, calls[0].rip), (6, entry + load as u64));
        assert!(calls[0].budget > 6, "{calls:?}");
        assert_eq!(calls[1].rip, entry + HANDLER as u64 + 1, "{calls:?}");
    }

    #[test]
    fn an_efer_write_ends_its_batched_call_like_one_instruction_calls() {
        // `WRMSR IA32_EFER` with NXE cleared: a translation context change ends the call.
        let mut code = vec![0x90; 3];
        code.extend_from_slice(&[0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32]);
        code.extend_from_slice(&[0x25, 0xFF, 0xF7, 0xFF, 0xFF, 0x0F, 0x30]);
        let after = code.len();
        code.extend_from_slice(&[0x90, 0x90, 0x90, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        let entry = perf25_entry();
        assert!(
            calls.iter().any(|call| call.rip == entry + after as u64),
            "{calls:?}"
        );
    }

    #[test]
    fn popf_changing_ac_if_and_tf_ends_its_batched_call_like_one_instruction_calls() {
        // `pushfq; pop rax; <op> eax, imm32; push rax; popfq`: AC set, IF set, both cleared, TF set.
        let mut code = vec![0x90; 2];
        let mut ends = Vec::new();
        for op in [
            [0x0D, 0x00, 0x00, 0x04, 0x00],
            [0x0D, 0x00, 0x02, 0x00, 0x00],
            [0x25, 0xFF, 0xFD, 0xFB, 0xFF],
            [0x0D, 0x00, 0x01, 0x00, 0x00],
        ] {
            code.extend_from_slice(&[0x9C, 0x58]);
            code.extend_from_slice(&op);
            code.extend_from_slice(&[0x50, 0x9D]);
            ends.push(code.len());
            code.extend_from_slice(&[0x90, 0x90]);
        }
        code.push(0xF4);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
        let entry = perf25_entry();
        for end in &ends[..3] {
            assert!(
                calls.iter().any(|call| call.rip == entry + *end as u64),
                "{end:#x}: {calls:?} {run:?}"
            );
        }
        // TF: the set does not model the trap, so the POPFQ that sets it is refused and the run
        // stops with RIP on it (the call before it ends there).
        assert_eq!(
            run.run.exit,
            PagedProbeExit::Unsupported { opcode: 0x9d },
            "{run:?}"
        );
        assert!(
            calls
                .iter()
                .any(|call| call.rip == entry + ends[3] as u64 - 1),
            "{calls:?}"
        );
    }

    #[test]
    fn mov_ss_ends_its_batched_call_and_the_shadowed_instruction_runs_alone() {
        let mut code = vec![0x90; 3];
        code.extend_from_slice(&[0xB8, 0x10, 0x00, 0x00, 0x00, 0x8E, 0xD0]);
        let after = code.len();
        code.extend_from_slice(&[0x90, 0x90, 0x90, 0x90, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        let entry = perf25_entry();
        let index = calls
            .iter()
            .position(|call| call.rip == entry + after as u64)
            .expect("a call ends right after MOV SS");
        assert_eq!(
            (calls[index + 1].retired, calls[index + 1].rip),
            (1, entry + after as u64 + 1),
            "{calls:?}"
        );
    }

    #[test]
    fn mov_cr4_ends_its_batched_call_and_an_unsupported_mov_cr0_ends_it_before_itself() {
        // Two NOPs, the read of the register and the change of one bit (CR4.PGE set, CR0.WP
        // cleared), then the write. This board's set writes CR4 (the call ends right after it) and
        // has no MOV to CR0: that write stops the run as `Unsupported` (no change), after the
        // batched call retired the four instructions before it.
        for (read, op, write, cr0) in [
            (
                [0x0F, 0x20, 0xE0],
                [0x0D, 0x80, 0x00, 0x00, 0x00],
                [0x0F, 0x22, 0xE0],
                false,
            ),
            (
                [0x0F, 0x20, 0xC0],
                [0x25, 0xFF, 0xFF, 0xFE, 0xFF],
                [0x0F, 0x22, 0xC0],
                true,
            ),
        ] {
            let mut code = vec![0x90; 2];
            code.extend_from_slice(&read);
            code.extend_from_slice(&op);
            let at = code.len();
            code.extend_from_slice(&write);
            code.extend_from_slice(&[0x90, 0x90, 0xF4]);
            let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
            let entry = perf25_entry();
            if cr0 {
                assert_eq!(
                    run.run.exit,
                    PagedProbeExit::Unsupported { opcode: 0x22 },
                    "{run:?}"
                );
                assert_eq!(run.run.retired, 4, "{run:?}");
                assert_eq!((calls[0].retired, calls[0].rip), (4, entry + at as u64));
            } else {
                assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
                assert_eq!((calls[0].retired, calls[0].rip), (5, entry + at as u64 + 3));
            }
        }
    }

    #[test]
    fn an_mmio_write_ends_its_batched_call_like_one_instruction_calls() {
        // `mov rbx, 0xFC00_0040; mov eax, 0x10; mov [rbx], eax`: a store to the WIDTH register of
        // the display (a set window of the unified run).
        let mut code = vec![0x90; 2];
        code.extend_from_slice(&[0x48, 0xBB, 0x40, 0x00, 0x00, 0xFC, 0, 0, 0, 0]);
        code.extend_from_slice(&[0xB8, 0x10, 0x00, 0x00, 0x00, 0x89, 0x03]);
        let after = code.len();
        code.extend_from_slice(&[0x90, 0x90, 0x90, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        let entry = perf25_entry();
        assert!(
            calls.iter().any(|call| call.rip == entry + after as u64),
            "{calls:?}"
        );
    }

    #[test]
    fn a_partial_rep_of_a_batched_call_resumes_like_one_instruction_calls() {
        // `mov ecx, 2500; mov edi, 0x3000; mov al, 0xAB; rep stosb`: the REP stops after each
        // chunk of iterations (a partial REP, RIP on the REP) and resumes in the next call; then the
        // same in two runs whose first budget ends right after the REP started.
        let mut code = vec![0x90; 3];
        code.extend_from_slice(&[0xB9, 0xC4, 0x09, 0, 0, 0xBF, 0x00, 0x30, 0, 0, 0xB0, 0xAB]);
        let rep = code.len();
        code.extend_from_slice(&[0xF3, 0xAA, 0x90, 0x90, 0x90, 0xF4]);
        let board = || perf25_board(&code, &[]);
        let (run, calls) = perf25_compare(board, 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        let at_rep = perf25_entry() + rep as u64;
        assert!(
            calls.iter().filter(|call| call.rip == at_rep).count() >= 2,
            "{calls:?}"
        );
        let (runs, _) = perf25_compare_runs(board, &[7, 1_000]);
        assert_eq!(runs[1].run.exit, PagedProbeExit::Halted, "{runs:?}");
        let mut filled = vec![0u8; 2501];
        let mut batched_again = board();
        crate::run_firmware_unified(&mut batched_again, 1_000).unwrap();
        batched_again
            .bus()
            .physical_memory()
            .load_bytes(GuestPhysicalAddress::new(0x3000), &mut filled)
            .unwrap();
        assert!(filled[..2500].iter().all(|byte| *byte == 0xAB));
        assert_eq!(filled[2500], 0);
    }

    #[test]
    fn rf_from_iretq_runs_the_next_instruction_alone_like_one_instruction_calls() {
        // An `IRETQ` frame (SS 0x10, the entry RSP, RFLAGS with RF, CS 8, `target`): the
        // instruction at `target` starts with RF set and runs in a call of its own.
        let mut code = vec![
            0x48, 0x89, 0xE3, 0x6A, 0x10, 0x53, 0x68, 0x02, 0x00, 0x01, 0x00,
        ];
        code.extend_from_slice(&[0x6A, 0x08]);
        let lea = code.len();
        code.extend_from_slice(&[0x48, 0x8D, 0x05, 0, 0, 0, 0, 0x50, 0x48, 0xCF]);
        let target = code.len();
        let rel = (target - (lea + 7)) as u32;
        code[lea + 3..lea + 7].copy_from_slice(&rel.to_le_bytes());
        code.extend_from_slice(&[0x90, 0x90, 0x90, 0x90, 0xF4]);
        let (run, calls) = perf25_compare(|| perf25_board(&code, &[]), 1_000);
        assert_eq!(run.run.exit, PagedProbeExit::Halted, "{run:?}");
        let entry = perf25_entry();
        assert!(
            calls.contains(&crate::paged_memory_adapter::batch_trace::Call {
                budget: 1,
                retired: 1,
                rip: entry + target as u64 + 1,
            }),
            "{calls:?}"
        );
        assert_eq!(run.final_state.rflags & (1 << 16), 0, "{run:?}");
    }

    #[test]
    fn a_host_stop_inside_a_batched_call_leaves_the_state_of_as_many_one_instruction_calls() {
        // `inc rax; jmp back` at IF = 0; a host thread raises the stop while the run goes on. The
        // batched run stops at some boundary; one-instruction calls for that many instructions
        // must leave the same state and RAM.
        let code = [0x48, 0xFF, 0xC0, 0xEB, 0xFB];
        const BUDGET: u64 = 1 << 40;
        let mut batched = perf25_board(&code, &[]);
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        batched.set_host_stop(std::sync::Arc::clone(&flag));
        let setter = {
            let flag = std::sync::Arc::clone(&flag);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                flag.store(true, std::sync::atomic::Ordering::Release);
            })
        };
        crate::paged_memory_adapter::batch_trace::start();
        let stopped = crate::run_firmware_unified(&mut batched, BUDGET).unwrap();
        let calls = crate::paged_memory_adapter::batch_trace::take();
        setter.join().unwrap();
        assert!(batched.take_host_stop_honoured());
        let retired = stopped.run.retired;
        assert!(retired > 0 && retired < BUDGET, "{stopped:?}");
        assert!(calls.iter().any(|call| call.retired > 1), "{calls:?}");
        let mut single = perf25_board(&code, &[]);
        single.set_instruction_batches(false);
        let reference = crate::run_firmware_unified(&mut single, retired).unwrap();
        assert_eq!(reference.run.retired, retired);
        assert_eq!(batched.state(), single.state());
        assert_eq!(perf25_ram(&batched), perf25_ram(&single));
    }

    #[test]
    fn run_firmware_unified_refuses_boards_it_cannot_run_and_leaves_them_untouched() {
        const NEEDS_LINUX_PROFILE: &str =
            "run_firmware_unified needs a firmware board with the Linux profile";
        // A firmware board without the Linux profile.
        let mut firmware =
            load_firmware_image(&minor2_image(&[0xf4]), &tables_config_with_ecam()).unwrap();
        // A Linux board (Linux profile, no firmware entry).
        let mut linux = assemble_board(&linux_config(), PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(0x1000, 0x1000)
            .unwrap();
        // A firmware board with the Linux profile and two vCPUs.
        let mut two_cpus = load_firmware_image(
            &minor2_image(&[0xf4]),
            &FirmwareBoardConfig {
                vcpu_count: 2,
                ..linux_config()
            },
        )
        .unwrap();
        // A firmware board with the Linux profile whose timer source is the IOAPIC one: the 8259
        // source cannot be installed (S7-31m).
        let mut ioapic = load_firmware_image(&minor2_image(&[0xf4]), &linux_config()).unwrap();
        assert!(ioapic.enable_pit_ioapic_irq());
        for (board, expected) in [
            (&mut firmware, NEEDS_LINUX_PROFILE),
            (&mut linux, NEEDS_LINUX_PROFILE),
            (&mut two_cpus, "run_firmware_unified runs one vCPU"),
            (
                &mut ioapic,
                "run_firmware_unified needs the Linux-profile PIT and 8259",
            ),
        ] {
            let before = board.state().clone();
            for _ in 0..2 {
                assert_eq!(
                    crate::run_firmware_unified(board, 10).unwrap_err(),
                    FirmwareBoardError::Config(expected)
                );
                // A refusal models nothing and runs nothing.
                assert_eq!(board.state(), &before);
            }
        }
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

    /// S7-31j: the three legacy UART positions the board does not declare read `0xFF` and ignore
    /// writes under the Linux profile (the kernel's 8250 driver probes all four); COM1 is the
    /// declared UART and answers as before; undeclared ports outside these stay traps. This is
    /// the board without the agent channel (the default, S11-21); with it COM2 is a real UART, see
    /// `linux_profile_with_the_agent_channel_binds_com2_and_keeps_com3_and_com4_absent`.
    #[test]
    fn linux_profile_absent_uarts_read_ff_ignore_writes_and_leave_com1_alone() {
        let mut board = linux_board();
        let com1_lsr = inb(&mut board, 0x3fd).expect("COM1 line status");
        assert_ne!(
            com1_lsr, 0xff,
            "COM1 is a declared UART, not an absent port"
        );
        for base in [0x2f8u16, 0x3e8, 0x2e8] {
            for offset in 0..8u16 {
                let port = base + offset;
                assert_eq!(inb(&mut board, port), Ok(0xff), "read {port:#x}");
                board
                    .bus_mut()
                    .write_port(io_port(port), AccessWidth::Byte, 0x5a)
                    .unwrap_or_else(|error| panic!("write {port:#x}: {error:?}"));
                assert_eq!(
                    inb(&mut board, port),
                    Ok(0xff),
                    "read after write {port:#x}"
                );
            }
        }
        // Not byte-wide: refused (the width policy of the absent-port devices), not 0xFFFF.
        assert!(
            board
                .bus_mut()
                .read_port(io_port(0x2f9), AccessWidth::Word)
                .is_err()
        );
        // The ports next to the absent UARTs stay unbound traps (the policy of this board).
        for port in [0x2f7u16, 0x300, 0x3e7, 0x3f0, 0x2e7] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
        }
        // COM1 unchanged by the new bindings.
        assert_eq!(inb(&mut board, 0x3fd), Ok(com1_lsr));
    }

    /// S7-31k: without the Linux profile the absent UARTs are not bound: COM2..COM4 stay traps
    /// (without the agent channel; with it COM2 is bound, see
    /// `the_agent_channel_exchanges_bytes_through_the_board_bus_without_the_linux_profile`).
    #[test]
    fn absent_uarts_are_not_bound_without_the_linux_profile() {
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        for port in [0x2f8u16, 0x2f9, 0x3e8, 0x2e8] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
        }
    }

    fn out_ok(board: &mut LoadedFirmwareBoard, port: u16, value: u8) {
        board
            .bus_mut()
            .write_port(io_port(port), AccessWidth::Byte, u32::from(value))
            .unwrap_or_else(|error| panic!("write {port:#x}: {error:?}"));
    }

    /// S11-21 (S7-31j with COM2): with the agent channel the Linux profile binds a real UART at
    /// `0x2F8..0x2FF` instead of the absent one, COM3 and COM4 stay absent (read `0xFF`, writes
    /// ignored), COM1 is not touched and the neighbours stay traps.
    #[test]
    fn linux_profile_with_the_agent_channel_binds_com2_and_keeps_com3_and_com4_absent() {
        let config = FirmwareBoardConfig {
            agent_channel: true,
            ..linux_config()
        };
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
        let com1_lsr = inb(&mut board, 0x3fd).expect("COM1 line status");
        // COM2 is a declared UART: THR and transmitter empty, not the absent `0xFF`.
        assert_eq!(inb(&mut board, 0x2fd), Ok(0x60));
        out_ok(&mut board, 0x2ff, 0x5a);
        assert_eq!(inb(&mut board, 0x2ff), Ok(0x5a), "scratch register of COM2");
        assert_eq!(
            inb(&mut board, 0x3ff),
            Ok(0),
            "COM1 scratch is a different register"
        );
        for base in [0x3e8u16, 0x2e8] {
            for offset in 0..8u16 {
                let port = base + offset;
                assert_eq!(inb(&mut board, port), Ok(0xff), "read {port:#x}");
                out_ok(&mut board, port, 0x5a);
                assert_eq!(
                    inb(&mut board, port),
                    Ok(0xff),
                    "read after write {port:#x}"
                );
            }
        }
        // Width policy of the UART model: byte accesses only.
        assert!(
            board
                .bus_mut()
                .read_port(io_port(0x2f9), AccessWidth::Word)
                .is_err()
        );
        for port in [0x2f7u16, 0x300, 0x3e7, 0x3f0, 0x2e7] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
        }
        assert_eq!(inb(&mut board, 0x3fd), Ok(com1_lsr), "COM1 unchanged");
    }

    /// S11-21: the channel is opt-in. By default COM2 is the absent UART under the Linux profile
    /// and unbound without it, and the board has no channel handle.
    #[test]
    fn the_agent_channel_is_off_by_default_and_com2_stays_absent_or_a_trap() {
        assert!(!FirmwareBoardConfig::default().agent_channel);
        let mut linux = linux_board();
        assert!(linux.agent_channel().is_none());
        assert_eq!(inb(&mut linux, 0x2fd), Ok(0xff));
        let mut plain = load_firmware_image(&minor2_image(&[0xf4]), &tables_config()).unwrap();
        assert!(plain.agent_channel().is_none());
        assert_eq!(
            inb(&mut plain, 0x2fd),
            Err(BusAccessError::UnmappedPort(io_port(0x2fd)))
        );
    }

    /// S11-21 (S7-31k with COM2): the channel does not need the Linux profile; without it COM3
    /// and COM4 stay traps, and COM2 exchanges bytes with the host through the board's bus.
    #[test]
    fn the_agent_channel_exchanges_bytes_through_the_board_bus_without_the_linux_profile() {
        let config = FirmwareBoardConfig {
            agent_channel: true,
            ..tables_config()
        };
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
        for port in [0x3e8u16, 0x2e8] {
            assert_eq!(
                inb(&mut board, port),
                Err(BusAccessError::UnmappedPort(io_port(port))),
                "read {port:#x}"
            );
        }
        let channel = board.agent_channel().expect("the channel handle");
        // Host to guest: the byte shows in LSR.DR and RBR of COM2, not of COM1.
        assert_eq!(channel.write_to_guest(b"Z"), 1);
        assert_eq!(inb(&mut board, 0x2fd), Ok(0x61));
        assert_eq!(inb(&mut board, 0x3fd).map(|lsr| lsr & 1), Ok(0));
        assert_eq!(inb(&mut board, 0x2f8), Ok(u32::from(b'Z')));
        // Guest to host: configure 8N1 with a divisor as a driver does, write THR, pump.
        for (port, value) in [(0x2fbu16, 0x83u8), (0x2f8, 1), (0x2f9, 0), (0x2fb, 3)] {
            out_ok(&mut board, port, value);
        }
        out_ok(&mut board, 0x2f8, b'Q');
        assert!(channel.holding_occupied());
        let mut buf = [0u8; 4];
        assert_eq!(channel.read_from_guest(&mut buf), 1);
        assert_eq!(buf[0], b'Q');
        // COM1's transcript saw nothing.
        assert!(board.uart().received_byte().is_none());
    }

    /// S11-21: the board's loop cycle (`pump_uart_transmit`) pumps the agent channel and samples
    /// the COM2 line into INTIN3, with COM1 untouched. A byte the host queues reaches the guest's
    /// COM2 registers and raises the line; the guest's THR write reaches the host queue.
    #[test]
    fn the_board_cycle_pumps_the_agent_channel_and_routes_its_irq3_to_the_ioapic() {
        let config = FirmwareBoardConfig {
            agent_channel: true,
            ..linux_config()
        };
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
        assert!(board.enable_pit_ioapic_irq());
        assert!(board.enable_uart_ioapic_irq4());
        let channel = board.agent_channel().expect("the channel handle");
        let source = board.uart_ioapic_source().expect("the UART source");
        let dword = |board: &mut LoadedFirmwareBoard, address: u64, value: u64| {
            board
                .bus_mut()
                .write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .unwrap();
        };
        // The guest enables its Local APIC and routes INTIN3 (IOREGSEL 0x16) to vector 0x63 as an
        // edge, then enables the receive interrupt of COM2 (IER bit 0, MCR OUT2).
        dword(&mut board, crate::LOCAL_APIC_MMIO_BASE + 0xf0, 0x1ff);
        dword(&mut board, crate::IO_APIC_MMIO_BASE, 0x16);
        dword(&mut board, crate::IO_APIC_MMIO_BASE + 0x10, 0x63);
        for (port, value) in [(0x2fbu16, 0x03u8), (0x2fc, 0x08), (0x2f9, 0x01)] {
            out_ok(&mut board, port, value);
        }
        assert_eq!(board.lapic.pending(), None);
        // Host to guest: the byte is in COM2's RBR; the loop cycle presents the line to INTIN3.
        assert_eq!(channel.write_to_guest(b"R"), 1);
        assert_eq!(board.lapic.pending(), None);
        board.pump_uart_transmit(0);
        assert_eq!(source.com2_edges(), 1);
        assert_eq!(source.uart_edges(), 0);
        assert_eq!(board.lapic.acknowledge(), Some(0x63));
        assert_eq!(inb(&mut board, 0x2f8), Ok(u32::from(b'R')));
        // Guest to host: a THR write is moved to the host queue by the same cycle, without any
        // host call that would pump by itself.
        for (port, value) in [(0x2fbu16, 0x83u8), (0x2f8, 1), (0x2f9, 0), (0x2fb, 3)] {
            out_ok(&mut board, port, value);
        }
        out_ok(&mut board, 0x2f8, b'T');
        assert!(channel.holding_occupied());
        assert_eq!(channel.stats().tx_bytes_accepted, 0);
        board.pump_uart_transmit(0);
        assert!(!channel.holding_occupied());
        assert_eq!(channel.stats().tx_bytes_accepted, 1);
        // COM1 saw nothing of this.
        assert!(board.com1_log.bytes.is_empty());
    }

    /// S11-21: the parallel loops do not pump the agent channel, so `thread_parts` refuses a board
    /// that has it instead of running with a dead COM2.
    #[test]
    fn thread_parts_refuses_a_board_with_the_agent_channel() {
        let mut board = load_firmware_image(
            &minor2_image(&[0xf4]),
            &FirmwareBoardConfig {
                agent_channel: true,
                ..linux_config()
            },
        )
        .unwrap();
        board.set_execution_mode(crate::ExecutionMode::Parallel);
        assert_eq!(
            board.thread_parts().err(),
            Some(FirmwareBoardError::Config(
                "thread_parts: the agent channel (COM2) is not pumped by the parallel loops"
            ))
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
        assert_eq!(report.rdtsc_reads, Some(0));
        assert_eq!(report.pit_port_b_reads, Some(0));
        assert_eq!(report.pit_counter_port_reads, Some(3));
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
        assert_eq!(report.rdtsc_reads, Some(0));
        assert_eq!(report.pit_port_b_reads, Some(2));
        assert_eq!(report.pit_counter_port_reads, Some(3));
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
        assert_eq!(report.rdtsc_reads, Some(100));
        assert_eq!(report.rdtsc_reads_by_cpl, Some([100, 0, 0, 0]));
        assert_eq!(report.pit_port_b_reads, Some(0));
        assert_eq!(report.pit_counter_port_reads, Some(0));
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
    fn rdtsc_without_the_linux_profile_counts_time_only_in_the_firmware_boot_run() {
        let mut board = load_firmware_image(&rdtsc_image(2), &tables_config()).unwrap();
        assert!(board.rdtsc_tick.is_none());
        assert!(!board.has_linux_profile());
        let raw = board.run(100, || false, |_| {});
        assert_eq!(raw.exit, PagedProbeExit::Unsupported { opcode: 0x31 });
        assert_eq!(raw.instructions_executed, 0);
        assert_eq!(board.clock().now_ns(), 0);

        let report = board.run_boot(100).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 3, "RDTSC, RDTSC, HLT");
        assert_eq!(report.virtual_ns, 3, "one ns per retired instruction");
        assert_eq!(edx_eax(&board), 1, "second RDTSC after one retirement");
        assert_eq!(report.rdtsc_reads, None);
        assert_eq!(report.rdtsc_reads_by_cpl, None);
        assert_eq!(report.pit_port_b_reads, None);
        assert_eq!(report.pit_counter_port_reads, None);
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
    fn opt_in_refuses_foreign_timer_clock() {
        // S4-23j: a PIT or LAPIC timer on a clock of its own would have deadlines in another time
        // domain than the owner's quantum cut; the owner is refused and the board stays as it was.
        for (pit_clock, apic_clock) in [
            (Some(VirtualClockHandle::new(1_000_000_000).unwrap()), None),
            (None, Some(VirtualClockHandle::new(1_000_000_000).unwrap())),
        ] {
            let mut board = load_firmware_image_inner(
                &minor2_image(&[0xf4]),
                &linux_config(),
                PciEcam::new(),
                pit_clock,
                apic_clock,
            )
            .unwrap();
            assert!(matches!(
                board.enable_clock_owner(),
                Err(FirmwareBoardError::Config(text)) if text.contains("other than the board clock")
            ));
            assert!(!board.clock_owner_enabled());
            // The clock was not claimed: the standalone advance still works.
            board.advance_time_ns(7).unwrap();
            assert_eq!(board.clock().now_ns(), 7);
        }
    }

    #[test]
    fn opt_in_takes_the_clock_once_and_refuses_when_somebody_owns_it() {
        let mut board = linux_board();
        board.enable_clock_owner().unwrap();
        assert!(board.clock_owner_enabled());
        // A second call keeps the owner; the clock is claimed and only the owner moves it.
        board.enable_clock_owner().unwrap();
        assert_eq!(
            board.advance_time_ns(1),
            Err(FirmwareBoardError::Clock(
                VirtualClockError::ClockAlreadyOwned
            ))
        );
        // A clock somebody else already claimed is refused, nothing changes.
        let mut other = linux_board();
        let _foreign_owner = other.clock.claim_advancer().unwrap();
        assert_eq!(
            other.enable_clock_owner(),
            Err(FirmwareBoardError::Clock(
                VirtualClockError::ClockAlreadyOwned
            ))
        );
        assert!(!other.clock_owner_enabled());
    }

    #[test]
    fn idle_to_shows_every_vcpu_with_its_own_activity() {
        // S6-15: it used to show every application processor as waiting for SIPI.
        let config = FirmwareBoardConfig {
            vcpu_count: 2,
            ..linux_config()
        };
        let mut board = load_firmware_image(&minor2_image(&[0xf4]), &config).unwrap();
        board.enable_clock_owner().unwrap();
        // The boot CPU halted with IF = 1, the application processor waits for SIPI: the clock
        // skips to the wake time.
        board.vcpus[BOOT_VCPU_INDEX].state.activity = Some(CpuActivity::Halted);
        board.vcpus[BOOT_VCPU_INDEX].state.rflags |= 1 << 9;
        assert_eq!(board.idle_to(1000), Ok(1000));
        assert_eq!(board.clock().now_ns(), 1000);
        // An application processor that runs is not idle time: nothing may be skipped.
        board.vcpus[1].state.activity = Some(CpuActivity::Running);
        assert_eq!(
            board.idle_to(2000),
            Err(FirmwareBoardError::Config(
                "clock owner: the idle step did not reach the wake time"
            ))
        );
        assert_eq!(board.clock().now_ns(), 1000);
        // The same processor halted with IF = 0 and no timer does not hold the skip back.
        board.vcpus[1].state.activity = Some(CpuActivity::Halted);
        assert_eq!(board.idle_to(2000), Ok(1000));
        assert_eq!(board.clock().now_ns(), 2000);
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
    fn firmware_nvme_doorbells_complete_admin_and_read_dma_before_the_next_load() {
        const ASQ: u64 = 0x0700_0000;
        const IDENTIFY: u64 = 0x0700_2000;
        const IO_SQ: u64 = 0x0700_3000;
        const IO_CQ: u64 = 0x0700_4000;
        const DATA: u64 = 0x0700_5000;
        let code = [
            0xb8, 0, 0x10, 0, 0xfc, // mov eax,0xfc001000 (NVMe MMIO)
            0xb9, 3, 0, 3, 0, // mov ecx,0x00030003
            0x89, 0x48, 0x24, // mov [rax+AQA],ecx
            0xb9, 0, 0, 0, 7, // mov ecx,ASQ
            0x89, 0x48, 0x28, // mov [rax+ASQ.low],ecx
            0xb9, 0, 0x10, 0, 7, // mov ecx,ACQ
            0x89, 0x48, 0x30, // mov [rax+ACQ.low],ecx
            0xb9, 1, 0, 0x46, 0, // mov ecx,0x00460001
            0x89, 0x48, 0x14, // mov [rax+CC],ecx
            0x8b, 0x50, 0x1c, // mov edx,[rax+CSTS]
            0xb9, 3, 0, 0, 0, // mov ecx,3
            0x89, 0x88, 0, 0x10, 0, 0, // mov [rax+admin SQ tail],ecx
            0xbe, 0, 0x10, 0, 7, // mov esi,ACQ
            0x8b, 0x7e, 0x2c, // mov edi,[rsi+2*16+12] (last admin CQE)
            0xb9, 1, 0, 0, 0, // mov ecx,1
            0x89, 0x88, 8, 0x10, 0, 0, // mov [rax+I/O SQ tail],ecx
            0xbe, 0, 0x40, 0, 7, // mov esi,IO_CQ
            0x8b, 0x4e, 0x0c, // mov ecx,[rsi+12] (first I/O CQE)
            0xf4,
        ];
        let config = FirmwareBoardConfig {
            nvme: Some(NvmeConfig::default()),
            ..FirmwareBoardConfig::default()
        };
        let image = test_image(&code, &[(0x2a, &1u16.to_le_bytes())]);
        let mut board = load_firmware_image(&image, &config).unwrap();
        let sector: Vec<u8> = (0..512).map(|index| (index % 251) as u8).collect();
        board
            .nvme()
            .unwrap()
            .attach_backend(Box::new(
                crate::nvme::InMemoryBackend::from_bytes(sector.clone()).unwrap(),
            ))
            .unwrap();
        for (at, opcode, cid, nsid, prp, cdw10, cdw11) in [
            (ASQ, 0x06, 0x10u16, 0u32, IDENTIFY, 1u32, 0u32),
            (ASQ + 64, 0x05, 0x20, 0, IO_CQ, 0x0003_0001, 1),
            (ASQ + 128, 0x01, 0x30, 0, IO_SQ, 0x0003_0001, 0x0001_0001),
            (IO_SQ, 0x02, 0x40, 1, DATA, 0, 0),
        ] {
            let mut command = [0; 64];
            command[0] = opcode;
            command[2..4].copy_from_slice(&cid.to_le_bytes());
            command[4..8].copy_from_slice(&nsid.to_le_bytes());
            command[24..32].copy_from_slice(&prp.to_le_bytes());
            command[40..44].copy_from_slice(&cdw10.to_le_bytes());
            command[44..48].copy_from_slice(&cdw11.to_le_bytes());
            board
                .bus_mut()
                .write_flat_bytes(GuestPhysicalAddress::new(at), &command, |_| {})
                .unwrap();
        }
        let report = board.run_boot(32).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 19);
        assert_eq!(report.quanta, 3, "both doorbells end the current quantum");
        let regs = &board.state().gpr;
        assert_eq!(regs[Register::Rdx as usize], 1);
        assert_eq!(regs[Register::Rdi as usize], 0x0001_0030);
        assert_eq!(regs[Register::Rcx as usize], 0x0001_0040);
        let mut identify = [0; 4096];
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(IDENTIFY), &mut identify)
            .unwrap();
        assert_eq!(identify[77], 8, "Identify Controller MDTS");
        let mut data = [0; 512];
        board
            .bus_mut()
            .read_flat_bytes(GuestPhysicalAddress::new(DATA), &mut data)
            .unwrap();
        assert_eq!(data.as_slice(), sector);
    }

    #[test]
    fn firmware_display_mmio_commits_the_bound_device_and_publishes_a_frame() {
        let code = [
            0xb8, 0, 0, 0, 0xfc, // mov eax,0xfc000000
            0x8b, 0x10, // mov edx,[rax]
            0xb9, 0x80, 0x02, 0, 0, // mov ecx,640
            0x89, 0x48, 0x40, // mov [rax+0x40],ecx
            0xb9, 0xe0, 0x01, 0, 0, // mov ecx,480
            0x89, 0x48, 0x44, // mov [rax+0x44],ecx
            0xb9, 1, 0, 0, 0, // mov ecx,1
            0x89, 0x48, 0x48, // mov [rax+0x48],ecx
            0x89, 0x48, 0x4c, // mov [rax+0x4c],ecx
            0x89, 0x88, 0x80, 0, 0, 0,    // mov [rax+0x80],ecx
            0xf4, // hlt
        ];
        let mut board = load(&test_image(&code, &[]));
        let report = board.run_boot(32).unwrap();
        assert_eq!(report.exit, PagedProbeExit::Halted);
        assert_eq!(report.retired, 11);
        assert_eq!(board.state().gpr[Register::Rdx as usize], 0x4246_444f);
        let display = board.display();
        let mode = display.current_mode();
        assert_eq!((mode.width, mode.height, mode.stride), (640, 480, 2560));
        assert_eq!(display.frame_generation(), 1);
        assert!(display.has_current_frame());
    }

    #[test]
    fn firmware_display_mmio_rejects_a_wide_register_write_before_retirement() {
        let code = [
            0xb8, 0, 0, 0, 0xfc, // mov eax,0xfc000000
            0xb9, 1, 0, 0, 0, // mov ecx,1
            0x48, 0x89, 0x88, 0x80, 0, 0, 0,    // mov [rax+0x80],rcx
            0xf4, // hlt
        ];
        let mut board = load(&test_image(&code, &[]));
        let rip = board.state().rip;
        let report = board.run_boot(32).unwrap();
        assert!(matches!(report.exit, PagedProbeExit::Backend(_)));
        assert_eq!(report.retired, 2);
        assert_eq!(board.state().rip, rip + 10);
        assert_eq!(board.display().frame_generation(), 0);
        assert!(format!("{:?}", report.memory_error).contains("UnsupportedWidth"));
    }

    #[test]
    fn configured_rtc_answers_through_board_ports_and_follows_virtual_clock() {
        let config = FirmwareBoardConfig {
            rtc_base_unix_seconds: Some(1_704_067_200), // 2024-01-01 00:00:00 UTC
            ..reference_config()
        };
        let mut board = load_firmware_image(&minor4_image(&[0xf4]), &config).unwrap();
        let read_cmos = |index: u8, board: &mut LoadedFirmwareBoard| {
            board
                .bus_mut()
                .write_port(
                    PortAddress::new(CMOS_INDEX_PORT),
                    AccessWidth::Byte,
                    u32::from(index),
                )
                .unwrap();
            board
                .bus_mut()
                .read_port(PortAddress::new(CMOS_INDEX_PORT + 1), AccessWidth::Byte)
                .unwrap() as u8
        };
        assert_eq!(read_cmos(0x08, &mut board), 0x01);
        assert_eq!(read_cmos(0x09, &mut board), 0x24);
        board.clock.advance_standalone(61_000_000_000).unwrap();
        assert_eq!(read_cmos(0x02, &mut board), 0x01);
        assert_eq!(read_cmos(0x00, &mut board), 0x01);
    }

    #[test]
    fn rtc_reads_cross_uip_and_second_boundaries_in_one_firmware_quantum() {
        let config = FirmwareBoardConfig {
            rtc_base_unix_seconds: Some(1_704_067_200),
            ..reference_config()
        };
        let code = [
            0x66, 0xba, 0x71, 0,    // mov dx,0x71
            0xec, // in al,dx
            0x89, 0xc3, // mov ebx,eax
            0xec, // in al,dx
            0xf4, // hlt
        ];
        let image = minor4_image(&code);
        for (index, boundary, before, after) in
            [(0x0a, 999_756_000, 0x26, 0xa6), (0, 1_000_000_000, 0, 1)]
        {
            let start = boundary - 2;
            let load = || {
                let mut board = load_firmware_image(&image, &config).unwrap();
                board.advance_time_ns(start).unwrap();
                board
                    .bus_mut()
                    .write_port(PortAddress::new(CMOS_INDEX_PORT), AccessWidth::Byte, index)
                    .unwrap();
                board
            };
            let mut whole = load();
            let report = whole.run_boot(5).unwrap();
            assert_eq!(report.exit, PagedProbeExit::Halted);
            assert_eq!((report.retired, report.quanta), (5, 1));
            assert_eq!(whole.state().gpr[Register::Rbx as usize] as u8, before);
            assert_eq!(whole.state().gpr[Register::Rax as usize] as u8, after);
            assert_eq!(report.virtual_ns, start + report.retired);

            let mut split = load();
            let mut retired = 0;
            for _ in 0..5 {
                let quantum = split.run_boot(1).unwrap();
                retired += quantum.retired;
                assert_eq!(
                    quantum.exit,
                    if retired == 5 {
                        PagedProbeExit::Halted
                    } else {
                        PagedProbeExit::BudgetExhausted
                    },
                );
            }
            assert_eq!(retired, report.retired);
            assert_eq!(split.state(), whole.state());
            assert_eq!(split.clock(), whole.clock());
        }
    }

    #[test]
    fn rtc_timestamp_overflow_does_not_retire_the_port_access() {
        let config = FirmwareBoardConfig {
            rtc_base_unix_seconds: Some(1_704_067_200),
            ..reference_config()
        };
        for opcode in [0xec, 0xee] {
            let code = [0x66, 0xba, 0x71, 0, 0xb0, 0x82, opcode, 0xf4];
            let mut board = load_firmware_image(&minor4_image(&code), &config).unwrap();
            let entry = board.state().rip;
            board.advance_time_ns(u64::MAX).unwrap();
            board
                .bus_mut()
                .write_port(PortAddress::new(CMOS_INDEX_PORT), AccessWidth::Byte, 0x0b)
                .unwrap();
            let report = board.run_quantum(false, true, 8, || false, |_| {}, |_, _| {});
            assert!(matches!(
                report.exit,
                PagedProbeExit::PortIo(BusAccessError::Device {
                    source: crate::DeviceError::ClockOverflow,
                    ..
                })
            ));
            assert_eq!(report.instructions_executed, 2);
            assert_eq!(board.state().rip, entry + 6);
            assert_eq!(board.state().gpr[Register::Rax as usize] as u8, 0x82);
            assert_eq!(board.clock().now_ns(), u64::MAX);
            assert_eq!(
                board
                    .bus_mut()
                    .read_port(PortAddress::new(crate::CMOS_DATA_PORT), AccessWidth::Byte),
                Ok(2)
            );
        }
    }

    #[test]
    fn board_without_rtc_configuration_keeps_cmos_absent() {
        let mut board = linux_board();
        assert_eq!(
            board
                .bus_mut()
                .read_port(PortAddress::new(CMOS_INDEX_PORT), AccessWidth::Byte),
            Ok(u32::from(crate::ABSENT_PORT_READ_VALUE))
        );
        assert_eq!(
            board
                .bus_mut()
                .read_port(PortAddress::new(crate::CMOS_DATA_PORT), AccessWidth::Byte),
            Ok(u32::from(crate::ABSENT_PORT_READ_VALUE))
        );
    }

    #[test]
    fn irq_delivery_failure_is_an_error_of_the_run_and_a_deadline_cut_resumes() {
        use crate::paged_memory_adapter::IrqDeliveryFailure;
        use od_isa::Cpl0ExternalInterruptRefusal;
        let mapping_error = set_exit_to_probe_exit(PagedCpl0SetExit::IrqDeliveryFailed {
            vector: 0x30,
            failure: IrqDeliveryFailure::Refused(Cpl0ExternalInterruptRefusal::GateNotPresent),
        })
        .expect_err("a refused external IRQ remains a run error");
        assert_eq!(
            mapping_error,
            SetExitMappingError::ExternalInterrupt {
                vector: 0x30,
                failure: IrqDeliveryFailure::Refused(Cpl0ExternalInterruptRefusal::GateNotPresent,),
            }
        );
        let state = LongModeState {
            rip: 0xffff_ffff_8159_1b14,
            cs_selector: Some(0x33),
            ..LongModeState::default()
        };
        assert_eq!(
            mapping_error.into_board_error(&state),
            FirmwareBoardError::ExternalInterruptDelivery {
                vector: 0x30,
                failure: "Refused(GateNotPresent)".to_string(),
                rip: 0xffff_ffff_8159_1b14,
                cs_selector: Some(0x33),
            }
        );
        let budget = PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted);
        assert!(resumes_budget_cut(&budget, true, false, 5, 3));
        assert!(!resumes_budget_cut(&budget, false, false, 5, 3), "no cut");
        assert!(!resumes_budget_cut(&budget, true, true, 5, 3), "guest stop");
        assert!(!resumes_budget_cut(&budget, true, false, 0, 3), "no budget");
        assert!(
            !resumes_budget_cut(&budget, true, false, 5, 0),
            "no progress"
        );
        let tick = PagedCpl0SetExit::Run(RunExit::StopRequested);
        assert!(!resumes_budget_cut(&tick, true, false, 5, 3));
    }

    #[test]
    fn set_exit_without_a_probe_counterpart_is_an_integration_error_of_the_board() {
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::UnknownSetExit),
            Err(SetExitMappingError::Integration(
                "the CPL0 set returned an exit variant the board does not know",
            ))
        );
        // The loop passes no stop address.
        assert_eq!(
            set_exit_to_probe_exit(PagedCpl0SetExit::Run(RunExit::StopAddress)),
            Err(SetExitMappingError::Integration(
                "the CPL0 set returned an exit the board never asks for",
            ))
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

    // ---- S6-16a: the set loop of a vCPU reaches its own Local APIC ----

    /// PERF-04: the stop predicate of the CPL0-set loop looks at the bus flags (UART THR, NVMe
    /// doorbell, PM stop and tick, PIT port tick) only at the first call of a quantum and after
    /// the guest reached a device.
    mod stop_gate {
        use super::*;
        use od_isa::execute::{ControlRegisterPolicy, MovCr3Long64V1};
        use od_isa::{CliProfile, HaltProfile};

        const CLD: u8 = 0xFC;
        /// `MOV EDX, imm32`.
        fn mov_edx(value: u32) -> Vec<u8> {
            let mut code = vec![0xBA];
            code.extend_from_slice(&value.to_le_bytes());
            code
        }

        /// A Linux-profile board that runs `code` at the entry of the CPL0 set loop.
        fn board_running(code: &[u8]) -> LoadedFirmwareBoard {
            let mut board = load_firmware_image(&minor2_image(code), &linux_config()).unwrap();
            let mut state = board.state().clone();
            state.cr3 = Some(board.cr3());
            state.cs_selector = Some(0x08);
            state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9B,
                flags: 2,
            });
            state.ss_selector = Some(0x10);
            state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            });
            state.rflags = 2;
            state.activity = Some(CpuActivity::Running);
            *board.state_mut() = state;
            board
        }

        fn run(board: &mut LoadedFirmwareBoard, budget: u64) -> LinuxSetRunReport {
            let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
            let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                .with_cli(&cli)
                .with_cpu_owned_cr3(MovCr3Long64V1)
                .with_pio_u8()
                .with_halt(&halt);
            board
                .run_linux_boot_cpl0_set(budget, &set, false, false, false)
                .unwrap()
        }

        #[test]
        fn a_guest_write_to_the_uart_thr_ends_the_quantum_at_the_same_boundary() {
            // MOV EDX,0x3F8; MOV AL,'A'; OUT DX,AL; MOV AL,'B'; OUT DX,AL; six CLD.
            let mut code = mov_edx(0x3F8);
            code.extend([0xB0, b'A', 0xEE, 0xB0, b'B', 0xEE]);
            code.extend([CLD; 6]);
            let mut board = board_running(&code);
            let report = run(&mut board, 11);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.run.retired, 11);
            // Each OUT ends its quantum (3 and 2 instructions); the third quantum has the rest.
            assert_eq!(report.run.quanta, 3, "{report:?}");
            assert_eq!(report.run.com1, b"AB");
        }

        #[test]
        fn a_flag_raised_before_the_quantum_stops_it_at_the_first_boundary() {
            let mut board = board_running(&[CLD; 4]);
            board
                .bus_mut()
                .write_port(PortAddress::new(0x3F8), AccessWidth::Byte, u32::from(b'Z'))
                .unwrap();
            assert!(board.uart.holding_occupied());
            let report = run(&mut board, 4);
            // No device access happens in the run: only the first call of the first quantum can
            // see the flag. It stops the quantum before the first instruction.
            assert_eq!(report.run.exit, PagedProbeExit::Stopped, "{report:?}");
            assert_eq!(report.set_exit_detail, "Run(StopRequested)");
            assert_eq!(
                (report.run.retired, report.run.quanta),
                (0, 1),
                "{report:?}"
            );
            assert_eq!(report.run.com1, b"Z");
        }

        #[test]
        fn a_device_access_without_a_raised_flag_does_not_end_the_quantum() {
            // MOV EDX,0x3FD (the UART line status); three IN AL,DX; two CLD.
            let mut code = mov_edx(0x3FD);
            code.extend([0xEC; 3]);
            code.extend([CLD; 2]);
            let mut board = board_running(&code);
            let before = board.bus().device_access_counter().load(Ordering::Relaxed);
            let report = run(&mut board, 6);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.run.retired, 6);
            assert_eq!(report.run.quanta, 1, "{report:?}");
            assert!(report.run.com1.is_empty());
            let after = board.bus().device_access_counter().load(Ordering::Relaxed);
            assert_eq!(after - before, 3, "each IN reaches the device once");
        }
    }

    mod vcpu_mmio {
        use super::*;
        use crate::lapic_irq_source::LapicIrqSource;
        use crate::paged_memory_adapter::{
            PagedCpl0SetReport, run_cpl0_set_paged_2m_ram_with_cache,
        };
        use od_isa::execute::{ControlRegisterPolicy, MovCr3Long64V1};
        use od_isa::{CliProfile, HaltProfile, StiProfile};

        const IDT_AT: u64 = 0x0030_1000;
        const GDT_AT: u64 = 0x0030_2000;
        const HANDLER_AT: u64 = 0x0030_0000;
        const VECTOR: u8 = 0x30;
        /// SDM, ICR: Self shorthand (bits 19:18 = 01), Level = assert (bit 14), edge trigger,
        /// Fixed delivery mode, vector 0x30.
        const SELF_IPI: u32 = 0x0004_4000 | VECTOR as u32;

        fn mov_ebx_lapic_base() -> Vec<u8> {
            let mut code = vec![0xBB];
            code.extend_from_slice(&(LOCAL_APIC_MMIO_BASE as u32).to_le_bytes());
            code
        }

        fn mov_eax(value: u32) -> Vec<u8> {
            let mut code = vec![0xB8];
            code.extend_from_slice(&value.to_le_bytes());
            code
        }

        /// `MOV [RBX+offset], EAX` (`89 83 disp32`).
        fn store_eax(offset: u32) -> Vec<u8> {
            let mut code = vec![0x89, 0x83];
            code.extend_from_slice(&offset.to_le_bytes());
            code
        }

        /// The guest of 6 instructions: `MOV EBX, LAPIC base; MOV ECX, [RBX+0x20]` (APIC ID);
        /// `MOV EAX, 0x1FF; MOV [RBX+0xF0], EAX` (SVR: software enable);
        /// `MOV EAX, SELF_IPI; MOV [RBX+0x300], EAX` (ICR_LOW).
        fn guest() -> Vec<u8> {
            let mut code = mov_ebx_lapic_base();
            code.extend([0x8B, 0x4B, 0x20]);
            code.extend(mov_eax(0x1FF));
            code.extend(store_eax(0xF0));
            code.extend(mov_eax(SELF_IPI));
            code.extend(store_eax(0x300));
            code
        }

        /// A two-vCPU Linux-profile board whose set loop serves the Local APIC window, running
        /// `code` at the entry, with the handler (`MOV EAX, 0; MOV [RBX+0xB0], EAX; IRETQ`) and the
        /// IDT gate of `VECTOR` in guest RAM. Returns the board and a state of IF = 1 that can run
        /// on any vCPU (a copy of the boot state).
        fn board_and_state(code: &[u8]) -> (LoadedFirmwareBoard, LongModeState) {
            let config = FirmwareBoardConfig {
                vcpu_count: 2,
                ..linux_config()
            };
            let mut board = load_firmware_image(&minor2_image(code), &config).unwrap();
            board.bus_mut().enable_apic_mmio_windows();
            let mut handler = mov_eax(0);
            handler.extend(store_eax(0xB0));
            handler.extend([0x48, 0xCF]); // IRETQ
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
            gate[5] = 0x8E; // present, DPL 0, 64-bit interrupt gate
            gate[6..8].copy_from_slice(&((HANDLER_AT >> 16) as u16).to_le_bytes());
            let bus = board.bus_mut();
            bus.patch_physical_ram(GuestPhysicalAddress::new(HANDLER_AT), &handler)
                .unwrap();
            bus.patch_physical_ram(
                GuestPhysicalAddress::new(IDT_AT + u64::from(VECTOR) * 16),
                &gate,
            )
            .unwrap();
            // GDT: null, 64-bit ring-0 code (0x08), ring-0 data (0x10).
            let mut gdt = [0u8; 24];
            gdt[8..16].copy_from_slice(&0x00AF_9B00_0000_FFFFu64.to_le_bytes());
            gdt[16..24].copy_from_slice(&0x00CF_9300_0000_FFFFu64.to_le_bytes());
            board
                .bus_mut()
                .patch_physical_ram(GuestPhysicalAddress::new(GDT_AT), &gdt)
                .unwrap();
            let mut state = board.state().clone();
            state.cr3 = Some(board.cr3());
            state.cs_selector = Some(0x08);
            state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9B,
                flags: 2,
            });
            state.ss_selector = Some(0x10);
            state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            });
            state.gdtr = Some(od_isa::state::Gdtr {
                base: GDT_AT,
                limit: 0x17,
            });
            state.idtr = Some(od_isa::state::Idtr {
                base: IDT_AT,
                limit: 0xFFF,
            });
            state.rflags = 2 | (1 << 9);
            state.sti_shadow = Some(StiShadow::Clear);
            state.activity = Some(CpuActivity::Running);
            (board, state)
        }

        /// One call of the set loop of vCPU `vcpu`, with `source` as its interrupt source.
        fn run(
            board: &mut LoadedFirmwareBoard,
            vcpu: usize,
            state: &mut LongModeState,
            budget: u64,
            source: &mut dyn BoardIrqSource,
            counters: &mut IrqCounters,
        ) -> PagedCpl0SetReport {
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                .with_cli(&cli)
                .with_cpu_owned_cr3(MovCr3Long64V1)
                .with_pio_u8()
                .with_halt(&halt)
                .with_sti(&sti);
            let cr3 = board.cr3();
            // PERF-05: the hook carries the device-access count of the bus, as the board loop's.
            let device_accesses = board.bus().device_access_counter();
            run_cpl0_set_paged_2m_ram_with_cache(
                state,
                board.bus_mut(),
                vcpu,
                cr3,
                FIRMWARE_EFER,
                AccessContext {
                    mode: InstructionMode::Long64,
                    privilege: PrivilegeLevel::Ring0,
                    origin: AccessOrigin::Explicit,
                },
                budget,
                INTEGER_FLAGS,
                &set,
                None,
                || false,
                |_| {},
                false,
                None,
                Some(IrqBoundaryHook {
                    source,
                    counters,
                    delivered_mirror: None,
                    skip_first_boundary: false,
                    device_accesses: Some(&*device_accesses),
                    polled: None,
                }),
                None,
                None,
                None,
            )
        }

        fn irr_isr(board: &LoadedFirmwareBoard, vcpu: usize) -> (bool, bool) {
            board.local_apic_of(vcpu).unwrap().vector_state(VECTOR)
        }

        #[test]
        fn a_set_loop_of_vcpu_1_reads_its_own_apic_id_and_its_self_ipi_lands_in_its_own_irr() {
            let (mut board, mut state) = board_and_state(&guest());
            let (mut source, mut counters) = (NoIrqSource, IrqCounters::default());
            // 6 instructions retire; the pending vector (IRR of vCPU 1) is not offered to this
            // call's source, which is the empty one.
            let report = run(&mut board, 1, &mut state, 6, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 6, "{report:?}");
            assert_eq!(report.memory_error, None, "{report:?}");
            // The APIC ID register reads the index of the vCPU in bits 31:24 (SDM).
            assert_eq!(state.gpr[Register::Rcx as usize], 1 << 24);
            // The self-IPI is in the IRR of vCPU 1 and of no other LAPIC.
            assert_eq!(irr_isr(&board, 1), (true, false));
            assert_eq!(irr_isr(&board, 0), (false, false));
            // The SVR write enabled vCPU 1's LAPIC only.
            assert_eq!(
                board.local_apic_of(1).unwrap().spurious_vector(),
                Some(0xFF)
            );
            assert_eq!(
                board.local_apic_of(0).unwrap().eligible(),
                None,
                "vCPU 0's LAPIC is untouched"
            );
        }

        #[test]
        fn the_same_guest_on_vcpu_0_is_the_old_path() {
            let (mut board, mut state) = board_and_state(&guest());
            let (mut source, mut counters) = (NoIrqSource, IrqCounters::default());
            let report = run(&mut board, 0, &mut state, 6, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 6, "{report:?}");
            assert_eq!(state.gpr[Register::Rcx as usize], 0);
            assert_eq!(irr_isr(&board, 0), (true, false));
            assert_eq!(irr_isr(&board, 1), (false, false));
        }

        #[test]
        fn lapic_irq_source_of_vcpu_1_delivers_through_the_idt_and_the_eoi_clears_isr_of_vcpu_1() {
            let (mut board, mut state) = board_and_state(&guest());
            let lapic1 = board.local_apic_of(1).unwrap();
            let mut source = LapicIrqSource::new(lapic1);
            let mut counters = IrqCounters::default();
            let rsp = state.gpr[Register::Rsp as usize];
            // 6 guest instructions, the delivery at the boundary after the ICR write (retires
            // nothing), then the handler's first instruction.
            let report = run(&mut board, 1, &mut state, 7, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 7, "{report:?}");
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(
                state.rip,
                HANDLER_AT + 5,
                "inside the handler of the IDT gate"
            );
            assert_eq!(state.rflags & (1 << 9), 0, "an interrupt gate clears IF");
            assert_eq!(state.gpr[Register::Rsp as usize], (rsp & !15) - 40);
            // Acknowledged in vCPU 1's LAPIC (IRR -> ISR) and in no other.
            assert_eq!(irr_isr(&board, 1), (false, true));
            assert_eq!(irr_isr(&board, 0), (false, false));
            // The handler's EOI (written as vCPU 1) clears the ISR of vCPU 1; the IRETQ returns.
            let report = run(&mut board, 1, &mut state, 2, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 2, "{report:?}");
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(irr_isr(&board, 1), (false, false));
            assert_eq!(irr_isr(&board, 0), (false, false));
            assert_eq!(state.rflags & (1 << 9), 1 << 9, "IRETQ restores IF");
        }

        #[test]
        fn an_eoi_of_vcpu_1_does_not_touch_the_isr_of_vcpu_0() {
            // The same vector is in service on both processors; vCPU 1's handler writes EOI.
            let (mut board, mut state) = board_and_state(&guest());
            let lapic0 = board.local_apic_of(0).unwrap();
            let lapic1 = board.local_apic_of(1).unwrap();
            let (mut source0, mut source1) = (
                LapicIrqSource::new(lapic0.clone()),
                LapicIrqSource::new(lapic1),
            );
            let mut counters = IrqCounters::default();
            // vCPU 0 runs the guest to the delivery of its own self-IPI: ISR of vCPU 0 holds it.
            let mut state0 = state.clone();
            run(&mut board, 0, &mut state0, 7, &mut source0, &mut counters);
            assert_eq!(irr_isr(&board, 0), (false, true));
            // vCPU 1 does the same and then runs its handler to the EOI and the IRETQ.
            run(&mut board, 1, &mut state, 7, &mut source1, &mut counters);
            assert_eq!(irr_isr(&board, 1), (false, true));
            run(&mut board, 1, &mut state, 2, &mut source1, &mut counters);
            assert_eq!(irr_isr(&board, 1), (false, false));
            assert_eq!(
                irr_isr(&board, 0),
                (false, true),
                "vCPU 0 is still in service"
            );
        }

        /// PERF-05: a source that counts its questions; the vector it answers is fixed.
        struct CountingSource {
            polls: std::sync::Arc<std::sync::atomic::AtomicU64>,
            stable: bool,
            vector: Option<u8>,
        }

        impl CountingSource {
            fn new(
                stable: bool,
                vector: Option<u8>,
            ) -> (Self, std::sync::Arc<std::sync::atomic::AtomicU64>) {
                let polls = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                (
                    Self {
                        polls: polls.clone(),
                        stable,
                        vector,
                    },
                    polls,
                )
            }
        }

        impl BoardIrqSource for CountingSource {
            fn pending_vector(&self) -> Option<u8> {
                self.polls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.vector
            }

            fn acknowledge(&mut self, _vector: u8) {}

            fn next_deadline_ns(&self) -> Option<u64> {
                None
            }

            fn stable_between_device_accesses(&self) -> bool {
                self.stable
            }
        }

        /// PERF-05: a source that does not claim stability is asked at every boundary, also when
        /// no device is reached (the hook has the count of device accesses).
        #[test]
        fn a_source_that_is_not_stable_is_asked_at_every_boundary() {
            let (mut board, mut state) = board_and_state(&[0x90; 8]);
            let (mut source, polls) = CountingSource::new(false, None);
            let mut counters = IrqCounters::default();
            let report = run(&mut board, 0, &mut state, 8, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 8, "{report:?}");
            assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 8);
        }

        /// PERF-05: a stable source is asked at the first boundary of the call and again only after a
        /// guest access reached a device (here a port read: `in al, 0x21`, then 3 `nop`).
        #[test]
        fn a_stable_source_is_asked_again_only_after_a_device_access() {
            let (mut board, mut state) = board_and_state(&[0x90; 8]);
            let (mut source, polls) = CountingSource::new(true, None);
            let mut counters = IrqCounters::default();
            let report = run(&mut board, 0, &mut state, 8, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 8, "{report:?}");
            assert_eq!(
                polls.load(std::sync::atomic::Ordering::Relaxed),
                1,
                "no device access: one question for the 8 boundaries"
            );

            let (mut board, mut state) = board_and_state(&[0xE4, 0x21, 0x90, 0x90, 0x90]);
            let before = board
                .bus()
                .device_access_counter()
                .load(std::sync::atomic::Ordering::Relaxed);
            let (mut source, polls) = CountingSource::new(true, None);
            let report = run(&mut board, 0, &mut state, 4, &mut source, &mut counters);
            assert_eq!(report.instructions_executed, 4, "{report:?}");
            let after = board
                .bus()
                .device_access_counter()
                .load(std::sync::atomic::Ordering::Relaxed);
            assert!(after > before, "the port read reached a device");
            // Boundary 1 asks; the boundary after the port read asks again; the last two reuse.
            assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 2);
        }

        /// PERF-05: the refusal counters count at every boundary on the saved answer.
        #[test]
        fn the_refusal_counters_count_every_boundary_on_a_saved_answer() {
            for stable in [false, true] {
                let (mut board, mut state) = board_and_state(&[0x90; 8]);
                state.rflags = 2; // IF = 0
                let (mut source, polls) = CountingSource::new(stable, Some(VECTOR));
                let mut counters = IrqCounters::default();
                let report = run(&mut board, 0, &mut state, 5, &mut source, &mut counters);
                assert_eq!(report.instructions_executed, 5, "{report:?}");
                assert_eq!(counters.irq_refused_masked, 5, "stable = {stable}");
                assert_eq!(counters.irq_refused_shadow, 0);
                assert_eq!(counters.irqs_delivered, 0);
                assert_eq!(
                    polls.load(std::sync::atomic::Ordering::Relaxed),
                    if stable { 1 } else { 5 },
                    "stable = {stable}"
                );
            }
        }
    }
}
