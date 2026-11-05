//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod bus;
mod diagnostic_vcpu;
mod identity_memory_adapter;
mod page_walk;
mod physical_memory;
#[cfg(feature = "experimental-translation-probe-cache")]
mod translation_probe_cache;
mod virtual_clock;

pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError, IoSpace,
    MachineBus, MmioBinding, PortAddress, PortRange, PortRegionBinding,
};

pub use diagnostic_vcpu::{
    DiagnosticExit, DiagnosticRunResult, DiagnosticTerminalExit, DiagnosticVcpuControlError,
    DiagnosticVcpuSession, DiagnosticVcpuState, WorkLimitReason,
};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};

pub use page_walk::{
    AdBit, AdUpdate, EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome,
    InstructionProbeReport, Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe,
    PageFaultReason, PageTableLevel, ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome,
    ProbeReport, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessReport,
    TrackedAccessRequest, Translation4K, WalkEntry, WalkTrace, access_long64_4k_ram,
    probe_instruction_access, probe_long64_4k,
};

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};

pub use virtual_clock::{VirtualClock, VirtualClockError};

#[cfg(feature = "experimental-translation-probe-cache")]
pub use translation_probe_cache::{TranslationProbeCache, TranslationProbeCacheOutcome};
