//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod bus;
mod diagnostic_vcpu;
mod event_queue;
mod identity_memory_adapter;
mod page_walk;
mod paged_memory_adapter;
mod paged_pio_step;
mod paged_probe;
mod physical_memory;
mod pio_step;
#[cfg(feature = "experimental-translation-probe-cache")]
mod translation_probe_cache;
mod uart16550;
mod virtual_clock;

pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError, IoSpace,
    MachineBus, MmioBinding, PortAddress, PortRange, PortRegionBinding,
};

pub use diagnostic_vcpu::{
    DiagnosticExit, DiagnosticRunResult, DiagnosticTerminalExit, DiagnosticVcpuControlError,
    DiagnosticVcpuSession, DiagnosticVcpuState, WorkLimitReason,
};

pub use event_queue::{EventQueue, EventQueueError, ScheduledEvent};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};

pub use page_walk::{
    AdBit, AdUpdate, EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome,
    InstructionProbeReport, Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe,
    PageFaultReason, PageTableLevel, ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome,
    ProbeReport, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessReport,
    TrackedAccessRequest, Translation4K, WalkEntry, WalkTrace, access_long64_4k_ram,
    probe_instruction_access, probe_long64_4k,
};

pub use paged_memory_adapter::{PagedMovStepError, PagedMovStepReport, step_paged_mov};
pub use paged_pio_step::{PagedPioU8StepReport, step_paged_pio_u8};
pub use paged_probe::{
    PagedProbeExit, PagedProbeQuantumReport, PagedProbeRejection, run_paged_probe,
};

pub use pio_step::step_diagnostic_pio_u8;

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};

pub use virtual_clock::{VirtualClock, VirtualClockError};

pub use uart16550::{Uart16550Handle, Uart16550PollingTx};

#[cfg(feature = "experimental-translation-probe-cache")]
pub use translation_probe_cache::{TranslationProbeCache, TranslationProbeCacheOutcome};
