//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod acpi_blob;
mod acpi_pm;
mod apic_pair;
mod bus;
mod diagnostic_vcpu;
mod display_device;
mod event_queue;
mod firmware_board;
mod identity_memory_adapter;
mod io_apic;
mod local_apic;
mod nvme;
mod page_walk;
mod paged_descriptor;
mod paged_exception_delivery;
mod paged_memory_adapter;
mod paged_pio_step;
mod paged_probe;
mod paged_vcpu_session;
mod pci_ecam;
mod physical_memory;
mod pio_step;
mod stage4_alias_probe;
mod stage4_cr3_mixed_probe;
mod stage4_flat_probe;
mod stage4_stack_probe;
mod stage4_store_load_probe;
mod stage4_uart_irq_probe;
#[cfg(feature = "experimental-translation-probe-cache")]
mod translation_probe_cache;
mod uart16550;
mod uart_rx_events;
mod virtual_clock;

pub use acpi_blob::{
    ACPI_BLOB_ALIGN, ACPI_BLOB_DIR_ENTRY_BYTES, ACPI_BLOB_HEADER_BYTES, ACPI_BLOB_MAGIC,
    ACPI_BLOB_MAX_TABLES, ACPI_BLOB_RSDP_SLOT_BYTES, ACPI_BLOB_VERSION, ACPI_MANIFEST_HASH_TAG,
    ACPI_RECLAIM_END, ACPI_REGION_BASE, ACPI_REGION_END, AcpiBlobError, AcpiBlobTable,
    AcpiManifestHash, ValidatedAcpiBlob, acpi_manifest_hash, validate_acpi_blob,
};

pub use acpi_pm::{
    ACPI_PM_BASE_PORT, ACPI_PM_DESCRIPTOR, ACPI_PM_PORT_BYTES, ACPI_PM_TIMER_HZ, ACPI_PM_TMR_BLK,
    ACPI_PM_TMR_LEN, ACPI_PM1_CNT_BM_RLD, ACPI_PM1_CNT_GBL_RLS, ACPI_PM1_CNT_LEN,
    ACPI_PM1_CNT_SCI_EN, ACPI_PM1_CNT_SLP_EN, ACPI_PM1_CNT_SLP_TYP_MASK,
    ACPI_PM1_CNT_SLP_TYP_SHIFT, ACPI_PM1_EVT_LEN, ACPI_PM1A_CNT_BLK, ACPI_PM1A_EVT_BLK,
    ACPI_RESET_REG, ACPI_RESET_REG_WIDTH_BITS, ACPI_RESET_VALUE, ACPI_SCI_GSI, ACPI_SCI_IRQ_FLAGS,
    ACPI_SLP_TYP_S5, ACPI_SMI_CMD, AcpiPm, AcpiPmDescriptor, AcpiPmHandle, MachineStopReason,
};
pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError,
    DisplayBindingError, FramebufferBinding, IoSpace, MachineBus, MmioBinding, PortAddress,
    PortRange, PortRegionBinding, TrackedCodePatchError, TrackedCodeSnapshot,
    TrackedCodeSnapshotError,
};

pub use display_device::{
    CursorReadError, CursorShapeInfo, CursorState, DISPLAY_CURSOR_FORMAT_BGRA8888_PREMUL,
    DISPLAY_CURSOR_IMAGE_WORDS, DISPLAY_CURSOR_MAX_HEIGHT, DISPLAY_CURSOR_MAX_WIDTH,
    DISPLAY_EVENT_CAPACITY, DISPLAY_FORMAT_BGRX8888, DISPLAY_FRAMEBUFFER_BASE,
    DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_MAX_HEIGHT, DISPLAY_MAX_WIDTH, DISPLAY_REGISTERS_BASE,
    DISPLAY_REGISTERS_BYTES, DISPLAY_SECOND_FRAMEBUFFER_BASE, DisplayDevice, DisplayHandle,
    DisplayMode, DisplayReadError, DisplayRejectReason, DisplayRequestError, FrameInfo,
    ModeRequestOutcome, ModeRequestState, SecondScreenState,
};

pub use diagnostic_vcpu::{
    DiagnosticApicQuantumResult, DiagnosticExit, DiagnosticHaltEnterFailure,
    DiagnosticHaltEnterReason, DiagnosticHaltWait, DiagnosticHaltWaitError,
    DiagnosticHaltWakeResult, DiagnosticInterruptResult, DiagnosticIretqResult, DiagnosticJitError,
    DiagnosticJitResult, DiagnosticRunResult, DiagnosticStiApicQuantumResult,
    DiagnosticTerminalExit, DiagnosticVcpuControlError, DiagnosticVcpuSession, DiagnosticVcpuState,
    WorkLimitReason,
};

pub use event_queue::{EventGeneration, EventQueue, EventQueueError, ScheduledEvent};

pub use firmware_board::{
    FIRMWARE_ACPI_FACS_BASE, FIRMWARE_ACPI_FACS_BYTES, FIRMWARE_ACPI_TABLES_BASE,
    FIRMWARE_ACPI_TABLES_BYTES, FIRMWARE_BOOT_PAGE_TABLES_GPA, FIRMWARE_BOOT_STACK_BASE,
    FIRMWARE_BOOT_STACK_TOP, FIRMWARE_COM1_PORT, FIRMWARE_EFER, FIRMWARE_HANDOFF_GPA,
    FIRMWARE_HANDOFF_WINDOW_BYTES, FIRMWARE_IMAGE_BYTES, FIRMWARE_LINK_BASE,
    FIRMWARE_MAX_RAM_BYTES, FIRMWARE_MIN_RAM_BYTES, FIRMWARE_ROM_BASE, FirmwareBoardConfig,
    FirmwareBoardError, FirmwareRunReport, LoadedFirmwareBoard, load_firmware_image,
};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};
pub use io_apic::{IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic, IoApicHandle};
pub use local_apic::{
    InterruptAcknowledge, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle,
    step_apic_base_msr, step_local_apic_cr8,
};

pub use page_walk::{
    AdBit, AdUpdate, EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome,
    InstructionProbeReport, Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe,
    PageFaultReason, PageTableLevel, ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome,
    ProbeReport, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessReport,
    TrackedAccessRequest, Translation4K, WalkEntry, WalkTrace, access_long64_2m_ram,
    access_long64_4k_ram, probe_instruction_access, probe_long64_4k,
};

pub use paged_descriptor::{
    PagedLgdtError, PagedLgdtReport, PagedLidtError, PagedLidtReport, PagedLtrError,
    PagedLtrRejection, PagedLtrReport, PagedSegmentMovError, PagedSegmentMovReport,
    step_paged_lgdt, step_paged_lidt, step_paged_ltr_ax, step_paged_mov_ds, step_paged_mov_es,
    step_paged_mov_fs, step_paged_mov_gs,
};
pub use paged_exception_delivery::{
    PagedCr0WpMovStepReport, PagedExceptionDeliveryError, PagedExceptionDeliveryOutcome,
    PagedExceptionDeliveryReport, PagedExceptionHandlerEntryReport,
    PagedExceptionStackDiscardReport, PagedIretqStepReport, PagedSyscallStepReport,
    PagedUserReturnReport, deliver_and_step_paged_pf_handler, deliver_paged_page_fault,
    run_paged_syscall_roundtrip, step_paged_cpl3_mov, step_paged_cpl3_mov_cr0,
    step_paged_cr0_wp_mov, step_paged_cr0_wp_page_table_repair_mov,
    step_paged_exception_stack_discard, step_paged_iretq, step_paged_iretq_and_user_load,
    step_paged_iretq_to_cpl3, step_paged_syscall_profile,
};
pub use paged_memory_adapter::{
    PagedInvlpgStepError, PagedInvlpgStepReport, PagedMovMemoryScope, PagedMovStepError,
    PagedMovStepReport, step_paged_invlpg, step_paged_mov, step_paged_mov_2m_ram,
    step_paged_mov_dword_read, step_paged_mov_word_read, step_paged_mov_word_read_cross_page_fetch,
    with_paged_mov_memory,
};
pub use paged_pio_step::{PagedPioU8StepReport, step_paged_pio_u8};
pub use paged_probe::{
    PagedProbeExit, PagedProbeIntegrationError, PagedProbeQuantumReport, PagedProbeRejection,
    run_paged_cr0_wp_probe, run_paged_cr3_probe, run_paged_cr3_probe_2m_ram, run_paged_probe,
    run_paged_stack_probe,
};
pub use paged_vcpu_session::{
    PagedProbeProfile, PagedVcpuError, PagedVcpuHostState, PagedVcpuSession,
};
pub use stage4_alias_probe::load_stage4_alias_probe;
pub use stage4_cr3_mixed_probe::{
    LoadedStage4Cr3MixedProbe, STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES,
    STAGE4_CR3_MIXED_PROBE_IMAGE_GPA, STAGE4_CR3_MIXED_PROBE_ROOT_A, STAGE4_CR3_MIXED_PROBE_ROOT_B,
    Stage4Cr3MixedProbeLoadError, load_stage4_cr3_mixed_probe,
};
pub use stage4_flat_probe::{
    LoadedStage4FlatProbe, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_DATA_GVA,
    STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_ENTRY_GVA, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_RAM_BYTES, STAGE4_FLAT_PROBE_STACK_TOP_GVA,
    STAGE4_FLAT_PROBE_UART_BASE, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_flat_probe, load_stage4_uart_echo_probe,
};
pub use stage4_stack_probe::load_stage4_stack_probe;
pub use stage4_store_load_probe::load_stage4_store_load_probe;
pub use stage4_uart_irq_probe::{
    STAGE4_UART_IRQ_PROBE_COM1, STAGE4_UART_IRQ_PROBE_ENTRY, STAGE4_UART_IRQ_PROBE_IMAGE_BYTES,
    STAGE4_UART_IRQ_PROBE_IMAGE_GPA, STAGE4_UART_IRQ_PROBE_STACK_TOP,
    STAGE4_UART_IRQ_PROBE_TRACE_LIMIT, STAGE4_UART_IRQ_PROBE_VECTOR, Stage4UartIrqProbe,
    Stage4UartIrqProbeError, Stage4UartIrqProbeExit, Stage4UartIrqProbeLoadError,
    Stage4UartIrqProbeReport, UartIrqTraceEvent, UartIrqTraceKind, load_stage4_uart_irq_probe,
};

pub use nvme::{
    BackendError, InMemoryBackend, NVME_MMIO_BASE, NVME_MMIO_BYTES, NvmeAttachError, NvmeBackend,
    NvmeConfig, NvmeConfigError, NvmeDevice, NvmeHandle, NvmeServiceReport,
};

pub use pio_step::step_diagnostic_pio_u8;

pub use pci_ecam::{
    EcamAddress, EcamAddressError, PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES, PCI_ECAM_BASE,
    PCI_ECAM_BYTES, PCI_ECAM_DESCRIPTOR, PCI_ECAM_END_BUS, PCI_ECAM_SEGMENT, PCI_ECAM_START_BUS,
    PciEcam, PciEcamDescriptor, PciEcamError, PciFunction, Type0Config, Type0ConfigError,
    decode_ecam_offset,
};

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};

pub use virtual_clock::{VirtualClock, VirtualClockError, VirtualClockHandle};

pub use uart_rx_events::{
    UartRxDispatchExit, UartRxDispatchReport, UartRxEventPump, UartRxPumpError,
};
pub use uart16550::{Uart16550Handle, Uart16550PollingTx};

#[cfg(feature = "experimental-translation-probe-cache")]
pub use translation_probe_cache::{
    MutableTranslationProbeSession, MutableTranslationSessionError,
    TranslationInvalidationConsumption, TranslationProbeCache, TranslationProbeCacheOutcome,
    consume_translation_invalidation,
};
