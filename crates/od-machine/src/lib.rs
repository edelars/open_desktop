//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod apic_pair;
mod bus;
mod diagnostic_vcpu;
mod event_queue;
mod identity_memory_adapter;
mod io_apic;
mod local_apic;
mod page_walk;
mod paged_descriptor;
mod paged_exception_delivery;
mod paged_memory_adapter;
mod paged_pio_step;
mod paged_probe;
mod paged_vcpu_session;
mod physical_memory;
mod pio_step;
mod stage4_alias_probe;
mod stage4_flat_probe;
mod stage4_stack_probe;
mod stage4_store_load_probe;
#[cfg(feature = "experimental-translation-probe-cache")]
mod translation_probe_cache;
mod uart16550;
mod uart_rx_events;
mod virtual_clock;

pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError, IoSpace,
    MachineBus, MmioBinding, PortAddress, PortRange, PortRegionBinding, TrackedCodePatchError,
    TrackedCodeSnapshot, TrackedCodeSnapshotError,
};

pub use diagnostic_vcpu::{
    DiagnosticExit, DiagnosticInterruptResult, DiagnosticIretqResult, DiagnosticRunResult,
    DiagnosticTerminalExit, DiagnosticVcpuControlError, DiagnosticVcpuSession, DiagnosticVcpuState,
    WorkLimitReason,
};

pub use event_queue::{EventGeneration, EventQueue, EventQueueError, ScheduledEvent};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};
pub use io_apic::{IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic, IoApicHandle};
pub use local_apic::{
    LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle, step_apic_base_msr,
    step_local_apic_cr8,
};

pub use page_walk::{
    AdBit, AdUpdate, EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome,
    InstructionProbeReport, Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe,
    PageFaultReason, PageTableLevel, ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome,
    ProbeReport, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessReport,
    TrackedAccessRequest, Translation4K, WalkEntry, WalkTrace, access_long64_4k_ram,
    probe_instruction_access, probe_long64_4k,
};

pub use paged_descriptor::{
    PagedLgdtError, PagedLgdtReport, PagedLidtError, PagedLidtReport, step_paged_lgdt,
    step_paged_lidt,
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
    PagedMovStepReport, step_paged_invlpg, step_paged_mov, step_paged_mov_dword_read,
    step_paged_mov_word_read, step_paged_mov_word_read_cross_page_fetch, with_paged_mov_memory,
};
pub use paged_pio_step::{PagedPioU8StepReport, step_paged_pio_u8};
pub use paged_probe::{
    PagedProbeExit, PagedProbeIntegrationError, PagedProbeQuantumReport, PagedProbeRejection,
    run_paged_cr0_wp_probe, run_paged_cr3_probe, run_paged_probe, run_paged_stack_probe,
};
pub use paged_vcpu_session::{
    PagedProbeProfile, PagedVcpuError, PagedVcpuHostState, PagedVcpuSession,
};
pub use stage4_alias_probe::load_stage4_alias_probe;
pub use stage4_flat_probe::{
    LoadedStage4FlatProbe, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_DATA_GVA,
    STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_ENTRY_GVA, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_RAM_BYTES, STAGE4_FLAT_PROBE_STACK_TOP_GVA,
    STAGE4_FLAT_PROBE_UART_BASE, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_flat_probe, load_stage4_uart_echo_probe,
};
pub use stage4_stack_probe::load_stage4_stack_probe;
pub use stage4_store_load_probe::load_stage4_store_load_probe;

pub use pio_step::step_diagnostic_pio_u8;

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};

pub use virtual_clock::{VirtualClock, VirtualClockError};

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
