//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod absent_ports;
mod acpi_blob;
mod acpi_pm;
mod ap_legacy_exec;
mod apic_pair;
mod bus;
mod clock_owner;
mod console_output_context;
mod crc32;
mod diagnostic_vcpu;
mod display_device;
mod dma_page;
mod event_queue;
mod execution_observations;
mod fetch_cache_report;
mod fetch_cache_sweep;
mod fetch_translation_cache;
mod firmware_board;
#[cfg(test)]
mod host_order_litmus;
mod host_parallel;
mod identity_memory_adapter;
mod idle_arbiter;
#[cfg(test)]
mod interleaved_harness;
mod interrupt_pump;
mod io_apic;
mod irq_source;
mod lapic_irq_source;
mod linux_boot;
mod linux_boot_params;
mod linux_checkpoints;
mod linux_direct;
mod linux_load;
mod linux_placement;
mod litmus;
mod litmus_interleaved_hooked;
mod litmus_interleaving;
mod litmus_parallel;
mod local_apic;
mod lock_wait_metrics;
mod nvme;
mod nvram_device;
mod pae_fetch_view;
mod pae_operand_access;
mod page_walk;
#[cfg(test)]
mod page_walk_2m_matrix_tests;
mod paged_apic_entry;
mod paged_descriptor;
mod paged_exception_delivery;
mod paged_memory_adapter;
mod paged_pio_step;
mod paged_probe;
mod paged_pump_backend;
mod paged_vcpu_session;
mod parallel_run;
#[cfg(test)]
mod parallel_snapshot_tests;
mod pci_ecam;
mod physical_memory;
mod pic_8259;
mod pio_step;
mod pit_8254;
mod pit_ioapic_irq_source;
mod pit_pic_irq_source;
mod ram_storage;
mod rtc_cmos;
mod smp_scheduler;
mod stage4_alias_probe;
mod stage4_cpl3_syscall_probe;
mod stage4_cr3_mixed_probe;
mod stage4_cross_page_probe;
mod stage4_flat_probe;
mod stage4_huge_pages_probe;
mod stage4_interrupt_probe_common;
mod stage4_interrupt_pump_paged_probe;
mod stage4_interrupt_pump_probe;
mod stage4_interrupt_queue_probe;
mod stage4_interrupt_timer_probe;
mod stage4_paged_irq_probe;
mod stage4_paged_sti_hlt_probe;
mod stage4_pf_repair_probe;
mod stage4_pit_calibrate_probe;
mod stage4_pit_irq0_probe;
mod stage4_stack_probe;
mod stage4_store_load_probe;
mod stage4_uart_irq_probe;
mod stage6_smp_probe;
mod stop_diagnostic;
mod tpm_crb;
#[cfg(feature = "experimental-translation-probe-cache")]
mod translation_probe_cache;
mod uart16550;
mod uart_ioapic_irq_source;
mod uart_pic_irq_source;
mod uart_rx_events;
mod vcpu;
mod virtual_clock;
mod vm_snapshot;

pub use acpi_blob::{
    ACPI_BLOB_ALIGN, ACPI_BLOB_DIR_ENTRY_BYTES, ACPI_BLOB_HEADER_BYTES, ACPI_BLOB_MAGIC,
    ACPI_BLOB_MAX_TABLES, ACPI_BLOB_RSDP_SLOT_BYTES, ACPI_BLOB_VERSION, ACPI_MANIFEST_HASH_TAG,
    ACPI_RECLAIM_END, ACPI_REGION_BASE, ACPI_REGION_END, AcpiBlobError, AcpiBlobTable,
    AcpiManifestHash, ValidatedAcpiBlob, acpi_manifest_hash, validate_acpi_blob,
    validate_acpi_blob_for_vcpus,
};

pub use absent_ports::{
    ABSENT_PORT_READ_VALUE, AbsentPortRegion, AbsentPorts, CMOS_DATA_PORT, CMOS_INDEX_PORT,
    CMOS_PORT_BYTES, IO_DELAY_PORT, IO_DELAY_PORT_BYTES, POST_PORT, POST_PORT_BYTES,
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
    TrackedCodeSnapshotError, TrackedCrossPageCodeSnapshot,
};
pub use nvram_device::{
    NVRAM_MMIO_BASE, NVRAM_MMIO_BYTES, NVRAM_SNAPSHOT_BYTES, NVRAM_STORE_BYTES, NvramConfig,
    NvramContentsError, NvramDevice, SecureBootKeys,
};
pub use rtc_cmos::{
    CMOS_RTC_NOT_PRESENT_BIT, RTC_BASE_UNIX_SECONDS_MAX, RTC_CENTURY_REGISTER, RtcCmos,
    RtcCmosError,
};
pub use stop_diagnostic::{
    DiagnosticReadError, DiagnosticReadFailure, DiagnosticTranslateError, DiagnosticTranslation,
    VcpuDiagnosticState,
};
pub use tpm_crb::{TPM_CRB_BASE, TPM_CRB_LEN, TpmConfig, TpmPlatformPort};

pub use console_output_context::{
    ConsoleContextFrame, ConsoleContextTrigger, ConsoleOutputContextSnapshot, FrameChainEnd,
    LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY, MAX_UART_PROVENANCE_BYTES, UartOutputByteContextSnapshot,
    UartOutputContextDiagnostic, UartOutputContextEnd,
};
pub use dma_page::{DMA_PAGE_0_PORT, DMA_PAGE_0_PORT_BYTES, DmaPage0};
pub use pic_8259::{
    ISA_IRQ_VECTOR_BASE, PIC_ELCR_BASE_PORT, PIC_ELCR_MASTER_MASK, PIC_ELCR_PORT_BYTES,
    PIC_ELCR_SLAVE_MASK, PIC_ICW1_ACCEPTED, PIC_ICW4_ACCEPTED, PIC_ICW4_MASTER_AEOI,
    PIC_MASTER_BASE_PORT, PIC_MASTER_ICW3, PIC_PORT_BYTES, PIC_SLAVE_BASE_PORT, PIC_SLAVE_ICW3,
    Pic8259, Pic8259Elcr, Pic8259ElcrHandle, Pic8259Handle, Pic8259InitPhase, Pic8259Pair,
    Pic8259Role, Pic8259Snapshot, isa_irq_vector,
};

pub use pit_8254::{
    PIT_BASE_PORT, PIT_INPUT_HZ, PIT_PORT_B, PIT_PORT_B_BYTES, PIT_PORT_B_READ_TICK_NS,
    PIT_PORT_BYTES, PIT_REFRESH_TOGGLE_TICKS, Pit8254, Pit8254Devices, Pit8254Handle, PitAccess,
    PitChannel, PitChannelSnapshot, PitLatch, PitMode, PitNextReload, PitPortB, PitRun,
    PitSnapshot, pit_ticks,
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

pub use interrupt_pump::{
    BackendEnv, BackendFlow, BackendStep, FlatBackend, INTERRUPT_PUMP_QUEUE_CAPACITY,
    INTERRUPT_PUMP_TRACE_LIMIT, InterruptPump, PumpBackend, PumpEntrySource, PumpExit, PumpHost,
    PumpHostEvent, PumpHostStep, PumpInjection, PumpQueueError, PumpRejection, PumpResult,
    PumpTraceEvent, PumpTraceKind, PumpTraceMode, PumpTraceModeError,
};
pub use paged_pump_backend::PagedBackend;
pub use stage4_interrupt_pump_paged_probe::{
    STAGE4_INTERRUPT_PUMP_PAGED_HANDLER_RIP, STAGE4_INTERRUPT_PUMP_PAGED_HLT_RIPS,
    STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES, STAGE4_INTERRUPT_PUMP_PAGED_MAIN_CODE,
    STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA, STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA,
    STAGE4_INTERRUPT_PUMP_PAGED_STI_RIP, STAGE4_INTERRUPT_PUMP_PAGED_TSC_HZ,
    STAGE4_INTERRUPT_PUMP_PAGED_VECTOR, Stage4InterruptPumpPagedStart,
    enable_stage4_interrupt_pump_paged_apic, expected_stage4_interrupt_pump_paged_image,
    load_stage4_interrupt_pump_paged_probe,
};

pub use fetch_cache_report::{
    FETCH_CACHE_PHASE_RULE_C5, FETCH_CACHE_PHASE_RULE_NONE, FetchCacheStats, LinuxFetchCachePhase,
    LinuxFetchCacheReport, LinuxWalkCounts,
};
pub use fetch_cache_sweep::{
    FetchSweepConfigReport, FetchSweepReport, SweepConfig, SweepEventCounts, SweepInvalidation,
    SweepLifecycle, SweepReplacement, SweepTag,
};
pub use linux_boot::{LinuxBzImageHeader, LinuxHeaderError, parse_linux_bzimage_header};
pub use linux_checkpoints::{
    LINUX_CHECKPOINT_COUNT, LinuxCheckpointRecord, LinuxCheckpointTable, LinuxCheckpointTableError,
    LinuxCheckpointTarget,
};
pub use linux_direct::{
    LINUX_STOP_RIP_BYTES, LinuxCpl0SetMembers, LinuxDirectBoot, LinuxDirectE820,
    LinuxDirectEntryState, LinuxDirectError, LinuxDirectInput, LinuxDirectZone,
    LinuxInterleavedRunReport, LinuxInterleavedVcpu, LinuxParallelRunReport, LinuxStopInputs,
    LinuxStopRecord, LinuxThreadedOptions, LinuxThreadedRunReport, LinuxVcpuRunMode,
    LinuxVcpuRunState, SYSCALL_JOURNAL_CAPACITY, SyscallJournal, SyscallJournalEntry,
    load_linux_direct_board, load_linux_direct_board_with_nvme,
    load_linux_direct_board_with_nvme_and_vcpus, load_linux_direct_board_with_vcpus,
    run_linux_direct_board, run_linux_direct_board_set, run_linux_direct_board_set_observed,
    run_linux_direct_board_set_user_mode, run_linux_direct_board_set_user_mode_interleaved,
    run_linux_direct_board_set_user_mode_parallel, run_linux_direct_board_set_user_mode_threaded,
    run_linux_direct_board_set_user_mode_with_delay_tsc_summary,
    run_linux_direct_board_set_user_mode_with_elf_stack_copy,
    run_linux_direct_board_set_user_mode_with_execution_observations,
    run_linux_direct_board_set_user_mode_with_execution_observations_options,
    run_linux_direct_board_set_user_mode_with_execution_observations_sampling_options,
    run_linux_direct_board_set_user_mode_with_post_iret_index,
};

pub use event_queue::{EventGeneration, EventQueue, EventQueueError, ScheduledEvent};

pub use ap_legacy_exec::ApLegacyRefusal;
pub use execution_observations::{
    DelayTscCallSummary, DelayTscCallSummaryCollection, DelayTscPollSequenceStatus,
    DeliveredPageFaultObservation, ElfStackAlignmentCapture, ElfStackAlignmentPoint,
    ElfStackCopyBoundaryMarker, ElfStackCopyCounterSnapshot, ElfStackCopyCr3Event,
    ElfStackCopyCr3LedgerStatus, ElfStackCopyInvocation, ElfStackCopyInvocationStatus,
    ElfStackCopyPageFaultEvent, ElfStackCopyPreCopyCr3Event, ElfStackCopyPreCopyCr3Ledger,
    ElfStackCopyProbe, ElfStackCopyRdtscSiteCount, ElfStackCopySnapshot, ElfStackCopyStatus,
    ElfStackCopyTargetCr3Capture, ElfStackCopyTargetCr3ChainStatus, ElfStackCopyTargetCr3Frame,
    FaultingPageFaultContext, FaultingPageWalkContext, InstructionPathControlFlow,
    InstructionPathRegisterValue, InstructionPathTrace, InstructionPathTraceHeader,
    InstructionPathTraceRecord, InstructionPathTraceStatus, LINUX_DELAY_TSC_CALL_CAPACITY,
    LINUX_DELAY_TSC_ENTRY_RIP, LINUX_DELAY_TSC_POLL_RIP, LINUX_ELF_STACK_ALIGN_AFTER_RIP,
    LINUX_ELF_STACK_ALIGN_BEFORE_RIP, LINUX_ELF_STACK_COPY_BOUNDARIES,
    LINUX_ELF_STACK_COPY_CR3_EVENT_CAPACITY, LINUX_ELF_STACK_COPY_PF_EVENT_CAPACITY,
    LINUX_ELF_STACK_COPY_PRE_COPY_CR3_EVENT_CAPACITY, LINUX_ELF_STACK_COPY_PROBE_CAPACITY,
    LINUX_ELF_STACK_COPY_PROFILE, LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY,
    LINUX_ELF_STACK_COPY_RDTSC_SITE_CAPACITY, LINUX_ELF_STACK_COPY_RETIRED_CAPACITY,
    LINUX_ELF_STACK_COPY_RIP, LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_ORDINAL,
    LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP, LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY,
    LINUX_INSTRUCTION_PATH_TRACE_CAPACITY, LINUX_INSTRUCTION_PATH_TRACE_END_HIT,
    LINUX_INSTRUCTION_PATH_TRACE_QUANTUM, LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
    LINUX_INSTRUCTION_PATH_TRACE_START_HITS, LINUX_PAGE_FAULT_EVENT_CAPACITY,
    LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY, LINUX_RDTSC_CLOCK_TRACE_CAPACITY,
    LINUX_RDTSC_PREFIX_CAPACITY, LINUX_RDTSC_PREFIX_SENTINEL_RIP, LINUX_RDTSC_SAMPLE_CAPACITY,
    LINUX_RDTSC_SITE_CAPACITY, LinuxExecutionObservations, PostIretPageWalkSnapshot,
    RdtscClockTraceSample, RdtscEntrySample, RdtscPrefixSample, RdtscPrefixStatus,
    RdtscSampleWindow, RdtscSiteCount, RdtscSiteSample,
};
pub use execution_observations::{
    ElfMmuGatherCapture, ElfMmuGatherClosure, ElfMmuGatherContext, ElfMmuGatherInvocation,
    ElfMmuGatherPageWalk, ElfMmuGatherPhase, ElfMmuGatherRaw, ElfMmuGatherSetupData,
    ElfMmuGatherUnpaired, LINUX_ELF_MMU_GATHER_SETUP_CAPACITY,
    LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP, LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP,
    LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP, LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP,
};
pub use firmware_board::{
    FIRMWARE_ACPI_FACS_BASE, FIRMWARE_ACPI_FACS_BYTES, FIRMWARE_ACPI_TABLES_BASE,
    FIRMWARE_ACPI_TABLES_BYTES, FIRMWARE_BOOT_PAGE_TABLES_GPA, FIRMWARE_BOOT_STACK_BASE,
    FIRMWARE_BOOT_STACK_TOP, FIRMWARE_COM1_PORT, FIRMWARE_EFER, FIRMWARE_EXECUTION_CYCLE_NS,
    FIRMWARE_HANDOFF_GPA, FIRMWARE_HANDOFF_WINDOW_BYTES, FIRMWARE_IMAGE_BYTES, FIRMWARE_LINK_BASE,
    FIRMWARE_MAX_RAM_BYTES, FIRMWARE_MIN_RAM_BYTES, FIRMWARE_ROM_BASE, FirmwareBoardConfig,
    FirmwareBoardError, FirmwareFvApp, FirmwareRunReport, LoadedFirmwareBoard, QuantumProgress,
    firmware_handoff_block, load_firmware_image,
};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};
pub use io_apic::{IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic, IoApicHandle};
pub use local_apic::{
    InterruptAcknowledge, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle,
    step_apic_base_msr, step_local_apic_cr8,
};

pub use page_walk::pae::{
    PaeAccessRequest, PaeConfigError, PaeOperandOperation, PaePageSize, PaeProbeConfig,
    PaeTranslation, access_pae_ram, load_pae_pdptes, translate_pae_ram,
    translate_pae_ram_read_only, translate_pae_ram_read_only_with_trace,
};
pub use page_walk::{
    AdBit, AdUpdate, EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome,
    InstructionProbeReport, Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe,
    PageFaultReason, PageTableLevel, ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome,
    ProbeReport, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessReport,
    TrackedAccessRequest, Translation4K, WalkEntry, WalkTrace, access_long64_2m_ram,
    access_long64_4k_ram, probe_instruction_access, probe_long64_4k,
    validate_long64_read_only_walk_observation,
};

pub use host_parallel::{
    AccountedHostQuantumFn, HostBoardIrqSource, HostCpl0Step, HostDeviceServices, HostIrqBoundary,
    HostParallelConfig, HostParallelControl, HostParallelFinish, HostParallelReport,
    HostQuantumCall, HostQuantumExit, HostQuantumFailure, HostQuantumFn, HostQuantumResult,
    HostSetStepReport, HostSplitGate, HostStopError, HostStopProof, HostThreadExitFn,
    host_deliver_pending_irq, host_install_vcpu, host_parallel_board,
    host_parallel_board_with_nvme, run_accounted_host_threads, run_host_set_step, run_host_threads,
    run_host_threads_with_nmi_paging,
};
#[cfg(feature = "lock-wait-metrics")]
pub use lock_wait_metrics::{BoardLockWaitMetrics, MutexAcquisitionMetrics};
pub use paged_apic_entry::{PagedApicEntryReport, PagedApicEntryResult, poll_paged_apic_entry};
pub use paged_descriptor::{
    PagedLgdtError, PagedLgdtReport, PagedLidtError, PagedLidtReport, PagedLtrError,
    PagedLtrRejection, PagedLtrReport, PagedSegmentMovError, PagedSegmentMovReport,
    step_paged_lgdt, step_paged_lidt, step_paged_ltr_ax, step_paged_mov_ds, step_paged_mov_es,
    step_paged_mov_fs, step_paged_mov_gs,
};
pub use paged_exception_delivery::{
    PagedCr0WpMovStepReport, PagedExceptionDeliveryError, PagedExceptionDeliveryOutcome,
    PagedExceptionDeliveryReport, PagedExceptionHandlerEntryReport,
    PagedExceptionStackDiscardReport, PagedExternalInterruptDeliveryReport, PagedIretqStepReport,
    PagedSyscallStepReport, PagedUserReturnReport, deliver_and_step_paged_pf_handler,
    deliver_paged_external_interrupt, deliver_paged_page_fault, run_paged_syscall_roundtrip,
    step_paged_cpl3_mov, step_paged_cpl3_mov_cr0, step_paged_cr0_wp_mov,
    step_paged_cr0_wp_page_table_repair_mov, step_paged_exception_stack_discard, step_paged_iretq,
    step_paged_iretq_and_user_load, step_paged_iretq_to_cpl3, step_paged_syscall_profile,
};
pub use paged_memory_adapter::{
    PagedAtomicOperand, PagedCpl0ExceptionFailure, PagedDwordStoreStepReport, PagedInvlpgStepError,
    PagedInvlpgStepReport, PagedMovCr0StepReport, PagedMovCr3StepError, PagedMovCr3StepReport,
    PagedMovMemoryAdapter, PagedMovMemoryScope, PagedMovStepError, PagedMovStepReport,
    PagedMovStoreScope, PagedStateOpStepReport, PagedStoreRefusal, deliver_host_native_fault,
    locate_paged_atomic_operand, step_paged_hlt, step_paged_invlpg, step_paged_mov,
    step_paged_mov_2m_ram, step_paged_mov_cr0_wp, step_paged_mov_cr3_wp, step_paged_mov_dword_read,
    step_paged_mov_dword_store, step_paged_mov_word_read,
    step_paged_mov_word_read_cross_page_fetch, step_paged_sti, with_paged_cpl0_set_memory,
    with_paged_mov_dword_memory, with_paged_mov_memory, with_paged_mov_store_memory,
};
pub use paged_pio_step::{PagedPioU8StepReport, step_paged_pio_u8};
pub use paged_probe::{
    PagedProbeExit, PagedProbeIntegrationError, PagedProbeQuantumReport, PagedProbeRejection,
    run_paged_cr0_wp_probe, run_paged_cr3_probe, run_paged_cr3_probe_2m_ram, run_paged_probe,
    run_paged_probe_shared, run_paged_probe_with_rdtsc, run_paged_stack_probe,
};
pub use paged_vcpu_session::{
    PagedProbeProfile, PagedVcpuError, PagedVcpuHostState, PagedVcpuSession,
};
pub use parallel_run::HostNmiPaging;
pub use stage4_alias_probe::load_stage4_alias_probe;
pub use stage4_cpl3_syscall_probe::{
    LoadedStage4Cpl3SyscallProbe, STAGE4_CPL3_SYSCALL_DATA_GPA, STAGE4_CPL3_SYSCALL_FINAL_RIP,
    STAGE4_CPL3_SYSCALL_INITIAL_OFFSET, STAGE4_CPL3_SYSCALL_INITIAL_RAX,
    STAGE4_CPL3_SYSCALL_INITIAL_RSP, STAGE4_CPL3_SYSCALL_INSTRUCTIONS,
    STAGE4_CPL3_SYSCALL_KERNEL_CODE_GVA, STAGE4_CPL3_SYSCALL_KERNEL_DATA_GVA,
    STAGE4_CPL3_SYSCALL_STORE_OFFSET, STAGE4_CPL3_SYSCALL_USER_CODE_GVA,
    STAGE4_CPL3_SYSCALL_USER_DATA_GVA, Stage4Cpl3SyscallEvent, Stage4Cpl3SyscallInjection,
    Stage4Cpl3SyscallReport, Stage4Cpl3SyscallStop, load_stage4_cpl3_syscall_probe,
};
pub use stage4_cr3_mixed_probe::{
    LoadedStage4Cr3MixedProbe, STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES,
    STAGE4_CR3_MIXED_PROBE_IMAGE_GPA, STAGE4_CR3_MIXED_PROBE_ROOT_A, STAGE4_CR3_MIXED_PROBE_ROOT_B,
    Stage4Cr3MixedProbeLoadError, load_stage4_cr3_mixed_probe,
};
pub use stage4_cross_page_probe::{
    LoadedStage4CrossPageProbe, STAGE4_CROSS_PAGE_CODE0_GPA, STAGE4_CROSS_PAGE_CODE0_GVA,
    STAGE4_CROSS_PAGE_CODE1_GPA, STAGE4_CROSS_PAGE_CODE1_GVA, STAGE4_CROSS_PAGE_CONTROL_GVA,
    STAGE4_CROSS_PAGE_DATA0_GPA, STAGE4_CROSS_PAGE_DATA0_GVA, STAGE4_CROSS_PAGE_DATA1_GPA,
    STAGE4_CROSS_PAGE_DATA1_GVA, STAGE4_CROSS_PAGE_ENTRY_GVA, STAGE4_CROSS_PAGE_FINAL_RIP,
    STAGE4_CROSS_PAGE_FIRST_OFFSET, STAGE4_CROSS_PAGE_IMAGE_BYTES, STAGE4_CROSS_PAGE_INITIAL_RAX,
    STAGE4_CROSS_PAGE_INSTRUCTIONS, STAGE4_CROSS_PAGE_OPERAND_GVA, STAGE4_CROSS_PAGE_PTE_GPAS,
    STAGE4_CROSS_PAGE_PTES, STAGE4_CROSS_PAGE_SECOND_OFFSET, STAGE4_CROSS_PAGE_STEP_RIPS,
    STAGE4_CROSS_PAGE_STEPS, Stage4CrossPageEvent, Stage4CrossPageFault, Stage4CrossPageInjection,
    Stage4CrossPageOperand, Stage4CrossPageReport, Stage4CrossPageStep, Stage4CrossPageStop,
    load_stage4_cross_page_probe,
};
pub use stage4_flat_probe::{
    LoadedStage4FlatProbe, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_DATA_GVA,
    STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_ENTRY_GVA, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_RAM_BYTES, STAGE4_FLAT_PROBE_STACK_TOP_GVA,
    STAGE4_FLAT_PROBE_UART_BASE, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_flat_probe, load_stage4_uart_echo_probe,
};
pub use stage4_huge_pages_probe::{
    LoadedStage4HugePagesProbe, STAGE4_HUGE_PAGES_DECOY, STAGE4_HUGE_PAGES_END_GPA,
    STAGE4_HUGE_PAGES_END_GVA, STAGE4_HUGE_PAGES_END_OFFSET, STAGE4_HUGE_PAGES_ENTRIES,
    STAGE4_HUGE_PAGES_ENTRY_GPAS, STAGE4_HUGE_PAGES_ENTRY_GVA, STAGE4_HUGE_PAGES_ENTRY_NAMES,
    STAGE4_HUGE_PAGES_FINAL_RIP, STAGE4_HUGE_PAGES_FRAME_GPA, STAGE4_HUGE_PAGES_GIB_GVA,
    STAGE4_HUGE_PAGES_GIB_STEPS, STAGE4_HUGE_PAGES_IMAGE_BYTES, STAGE4_HUGE_PAGES_INITIAL_RAX,
    STAGE4_HUGE_PAGES_INSTRUCTIONS, STAGE4_HUGE_PAGES_MIDDLE_GPA, STAGE4_HUGE_PAGES_MIDDLE_GVA,
    STAGE4_HUGE_PAGES_MIDDLE_OFFSET, STAGE4_HUGE_PAGES_PAST_PAGE_GVA, STAGE4_HUGE_PAGES_RAM_BYTES,
    STAGE4_HUGE_PAGES_SENTINEL, STAGE4_HUGE_PAGES_SENTINEL_GPA, STAGE4_HUGE_PAGES_START_GPA,
    STAGE4_HUGE_PAGES_START_GVA, STAGE4_HUGE_PAGES_START_OFFSET, STAGE4_HUGE_PAGES_STEP_RIPS,
    STAGE4_HUGE_PAGES_STEPS, STAGE4_HUGE_PAGES_STORE_GPA, STAGE4_HUGE_PAGES_STORE_GVA,
    STAGE4_HUGE_PAGES_STORE_PREFILL, STAGE4_HUGE_PAGES_STORE_VALUE, Stage4HugePagesCase,
    Stage4HugePagesEvent, Stage4HugePagesFault, Stage4HugePagesInjection,
    Stage4HugePagesInjectionMismatch, Stage4HugePagesReport, Stage4HugePagesStep,
    Stage4HugePagesStop, load_stage4_huge_pages_probe,
};
pub use stage4_interrupt_pump_probe::{
    STAGE4_INTERRUPT_PUMP_PROBE_ENTRY, STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES,
    STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_GPA, STAGE4_INTERRUPT_PUMP_PROBE_RAM_BYTES,
    STAGE4_INTERRUPT_PUMP_PROBE_STACK_TOP, STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A,
    STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B, Stage4InterruptPumpProbeLoadError,
    load_stage4_interrupt_pump_probe,
};
pub use stage4_interrupt_queue_probe::{
    STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA, STAGE4_INTERRUPT_QUEUE_PROBE_ENTRY,
    STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA,
    STAGE4_INTERRUPT_QUEUE_PROBE_RAM_BYTES, STAGE4_INTERRUPT_QUEUE_PROBE_STACK_TOP,
    STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ, STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR,
    Stage4InterruptQueueProbeLoadError, load_stage4_interrupt_queue_probe,
};
pub use stage4_interrupt_timer_probe::{
    STAGE4_INTERRUPT_TIMER_PROBE_COUNTER_GPA, STAGE4_INTERRUPT_TIMER_PROBE_ENTRY,
    STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_GPA,
    STAGE4_INTERRUPT_TIMER_PROBE_RAM_BYTES, STAGE4_INTERRUPT_TIMER_PROBE_STACK_TOP,
    STAGE4_INTERRUPT_TIMER_PROBE_TIMER_HZ, STAGE4_INTERRUPT_TIMER_PROBE_TSC_HZ,
    STAGE4_INTERRUPT_TIMER_PROBE_VECTOR, Stage4InterruptTimerProbeLoadError,
    load_stage4_interrupt_timer_probe,
};
pub use stage4_paged_irq_probe::{
    LoadedStage4PagedIrqProbe, STAGE4_PAGED_IRQ_EOI_GVA, STAGE4_PAGED_IRQ_FINAL_RIP,
    STAGE4_PAGED_IRQ_GDT_GVA, STAGE4_PAGED_IRQ_HANDLER_GVA, STAGE4_PAGED_IRQ_IDT_GVA,
    STAGE4_PAGED_IRQ_IMAGE_BYTES, STAGE4_PAGED_IRQ_INITIAL_RSP, STAGE4_PAGED_IRQ_INSTRUCTIONS,
    STAGE4_PAGED_IRQ_LAPIC_GVA, STAGE4_PAGED_IRQ_MAIN_GVA, STAGE4_PAGED_IRQ_PTE_GDT,
    STAGE4_PAGED_IRQ_PTE_HANDLER, STAGE4_PAGED_IRQ_PTE_IDT, STAGE4_PAGED_IRQ_PTE_LAPIC,
    STAGE4_PAGED_IRQ_PTE_MAIN, STAGE4_PAGED_IRQ_PTE_STACK, STAGE4_PAGED_IRQ_RETURN_RIP,
    STAGE4_PAGED_IRQ_STACK_GVA, STAGE4_PAGED_IRQ_STEPS, STAGE4_PAGED_IRQ_VECTOR,
    Stage4PagedIrqEvent, Stage4PagedIrqFault, Stage4PagedIrqInjection, Stage4PagedIrqPtes,
    Stage4PagedIrqReport, Stage4PagedIrqStep, Stage4PagedIrqStop, load_stage4_paged_irq_probe,
};
pub use stage4_paged_sti_hlt_probe::{
    LoadedStage4PagedStiHltProbe, STAGE4_PAGED_STI_HLT_BOUNDARY_INSTRUCTIONS,
    STAGE4_PAGED_STI_HLT_BOUNDARY_STEPS, STAGE4_PAGED_STI_HLT_FIRST_HLT_RIP,
    STAGE4_PAGED_STI_HLT_IMAGE_BYTES, STAGE4_PAGED_STI_HLT_MAIN_CODE,
    STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP, STAGE4_PAGED_STI_HLT_WAKE_FINAL_RIP,
    STAGE4_PAGED_STI_HLT_WAKE_INSTRUCTIONS, STAGE4_PAGED_STI_HLT_WAKE_STEPS,
    Stage4PagedStiHltEvent, Stage4PagedStiHltInjection, Stage4PagedStiHltReport,
    Stage4PagedStiHltScenario, Stage4PagedStiHltStep, Stage4PagedStiHltStop,
    expected_stage4_paged_sti_hlt_image, load_stage4_paged_sti_hlt_probe,
};
pub use stage4_pf_repair_probe::{
    LoadedStage4PfRepairProbe, STAGE4_PF_REPAIR_CANARY_OFFSET, STAGE4_PF_REPAIR_DATA_GPA,
    STAGE4_PF_REPAIR_DATA_GVA, STAGE4_PF_REPAIR_FINAL_RIP, STAGE4_PF_REPAIR_HANDLER_GVA,
    STAGE4_PF_REPAIR_IMAGE_BYTES, STAGE4_PF_REPAIR_INITIAL_OFFSET, STAGE4_PF_REPAIR_INITIAL_RSP,
    STAGE4_PF_REPAIR_INSTRUCTIONS, STAGE4_PF_REPAIR_MAIN_GVA, STAGE4_PF_REPAIR_PTE_ALIAS_GVA,
    STAGE4_PF_REPAIR_PTE_GPA, STAGE4_PF_REPAIR_PTE_RO, STAGE4_PF_REPAIR_PTE_RO_ACCESSED,
    STAGE4_PF_REPAIR_PTE_RW, STAGE4_PF_REPAIR_PTE_RW_DIRTY, STAGE4_PF_REPAIR_STEPS,
    STAGE4_PF_REPAIR_STORE_OFFSET, Stage4PfRepairEvent, Stage4PfRepairFault,
    Stage4PfRepairInjection, Stage4PfRepairReport, Stage4PfRepairStep, Stage4PfRepairStop,
    load_stage4_pf_repair_probe,
};
pub use stage4_pit_calibrate_probe::{
    STAGE4_PIT_CALIBRATE_FAILED_FREQUENCY, STAGE4_PIT_CALIBRATE_PROBE_COM1,
    STAGE4_PIT_CALIBRATE_PROBE_ENTRY, STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES,
    STAGE4_PIT_CALIBRATE_PROBE_IMAGE_GPA, STAGE4_PIT_CALIBRATE_PROBE_RAM_BYTES,
    STAGE4_PIT_CALIBRATE_PROBE_STACK_TOP, STAGE4_PIT_CALIBRATE_PROBE_TSC_HZ,
    STAGE4_PIT_CALIBRATE_RECORD_BYTES, Stage4PitCalibrateExit, Stage4PitCalibrateInjection,
    Stage4PitCalibrateProbe, Stage4PitCalibrateProbeLoadError, Stage4PitCalibrateRecord,
    Stage4PitCalibrateReport, load_stage4_pit_calibrate_probe,
};
pub use stage4_pit_irq0_probe::{
    PitIrq0Stats, STAGE4_PIT_IRQ0_PROBE_COUNTER_GPA, STAGE4_PIT_IRQ0_PROBE_ENTRY,
    STAGE4_PIT_IRQ0_PROBE_EOI_GPA, STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES,
    STAGE4_PIT_IRQ0_PROBE_IMAGE_GPA, STAGE4_PIT_IRQ0_PROBE_RAM_BYTES,
    STAGE4_PIT_IRQ0_PROBE_STACK_TOP, STAGE4_PIT_IRQ0_PROBE_TSC_HZ,
    STAGE4_PIT_IRQ0_PROBE_UNMASK_GPA, STAGE4_PIT_IRQ0_PROBE_VECTOR, Stage4PitIrq0Injection,
    Stage4PitIrq0Probe, Stage4PitIrq0ProbeLoadError, load_stage4_pit_irq0_probe,
    next_out0_rising_tick, pit_tick_deadline_ns,
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

pub use litmus::{
    LITMUS_EXHAUSTIVE_THRESHOLD, LITMUS_MAX_SEEDS, LitmusCase, LitmusError, LitmusInstruction,
    LitmusOutcome, LitmusPolicy, LitmusReport, LitmusUnsupported, LitmusVerdict, LitmusWitness,
    litmus_seed_schedule, run_litmus,
};
/// S6-36: this build has the host memory order weakened (`weak-host-ordering`); never in a product
/// build. Evidence files record it.
pub const BUILD_WEAK_HOST_ORDERING: bool = cfg!(feature = "weak-host-ordering");
/// S6-36: this build has `LOCK` non-atomic (`nonatomic-lock-test`); never in a product build.
pub const BUILD_NONATOMIC_LOCK_TEST: bool = cfg!(feature = "nonatomic-lock-test");
pub use litmus_interleaved_hooked::{
    LitmusInterleavedOutcome, LitmusInterleavedReport, LitmusInterleavedVerdict,
    run_litmus_interleaved_hooked,
};
pub use litmus_parallel::{
    LITMUS_PARALLEL_MAX_VCPUS, LitmusParallelOutcome, LitmusParallelReport, LitmusParallelVerdict,
    LitmusStep, LitmusStepHook, LitmusStepOutcome, run_litmus_parallel, run_litmus_parallel_hooked,
    run_litmus_parallel_ram_probe,
};

pub use stage6_smp_probe::{
    STAGE6_SMP_PROBE_AP_RX_GPA, STAGE6_SMP_PROBE_BSP_RX_GPA, STAGE6_SMP_PROBE_COUNT_GPA,
    STAGE6_SMP_PROBE_FLAG_GPA, STAGE6_SMP_PROBE_IDT_GPA, STAGE6_SMP_PROBE_IDT_LIMIT,
    STAGE6_SMP_PROBE_IDTR_BSP_GPA, STAGE6_SMP_PROBE_IMAGE_BYTES, STAGE6_SMP_PROBE_IMAGE_GPA,
    STAGE6_SMP_PROBE_INFO_BYTES, STAGE6_SMP_PROBE_INFO_GPA, STAGE6_SMP_PROBE_INFO_MAGIC,
    STAGE6_SMP_PROBE_INFO_VERSION, STAGE6_SMP_PROBE_LOCK_GPA, STAGE6_SMP_PROBE_MARKER_AP_GPA,
    STAGE6_SMP_PROBE_MARKER_BSP_GPA, STAGE6_SMP_PROBE_MARKER_BYTES, STAGE6_SMP_PROBE_MARKER_MAGIC,
    STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM, STAGE6_SMP_PROBE_NMI_GPA, STAGE6_SMP_PROBE_NONCE_MAX,
    STAGE6_SMP_PROBE_PML4_GPA, STAGE6_SMP_PROBE_SD_ACK_GPA, STAGE6_SMP_PROBE_SD_POST_GPA,
    STAGE6_SMP_PROBE_SD_PTE_GPA, STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA,
    STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA, STAGE6_SMP_PROBE_SD_X_LINEAR, STAGE6_SMP_PROBE_SIPI_VECTOR,
    STAGE6_SMP_PROBE_TRAMPOLINE_GPA, STAGE6_SMP_PROBE_VECTOR_AP, STAGE6_SMP_PROBE_VECTOR_BSP,
    STAGE6_SMP_PROBE_VECTOR_SHOOTDOWN, Stage6SmpProbe, Stage6SmpProbeCpu, Stage6SmpProbeFinish,
    Stage6SmpProbeLoadError, Stage6SmpProbeParallelCpu, Stage6SmpProbeParallelFinish,
    Stage6SmpProbeParallelReport, Stage6SmpProbeParallelStop, Stage6SmpProbeQuantum,
    Stage6SmpProbeReport, Stage6SmpProbeRunError, Stage6SmpProbeSession, Stage6SmpProbeSnapshot,
    Stage6SmpProbeStopError, load_stage6_smp_probe,
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
    AtomicOperandWindow, ExecutionMode, GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap,
    MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError,
    RamLimits,
};

pub use virtual_clock::{VirtualClock, VirtualClockError, VirtualClockHandle};

pub use uart_rx_events::{
    UartRxDispatchExit, UartRxDispatchReport, UartRxEventPump, UartRxPumpError,
};
pub use uart16550::{
    UART_RX_ERROR_BREAK, UART_RX_ERROR_FRAMING, UART_RX_ERROR_PARITY, Uart16550Handle,
    Uart16550PollingTx,
};

#[cfg(feature = "experimental-translation-probe-cache")]
pub use translation_probe_cache::{
    MutableTranslationProbeSession, MutableTranslationSessionError,
    TranslationInvalidationConsumption, TranslationProbeCache, TranslationProbeCacheOutcome,
    consume_translation_invalidation,
};
