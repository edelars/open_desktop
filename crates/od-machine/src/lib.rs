//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod bus;
mod identity_memory_adapter;
mod page_walk;
mod physical_memory;

pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError, IoSpace,
    MachineBus, MmioBinding, PortAddress, PortRange, PortRegionBinding,
};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};

pub use page_walk::{
    EffectivePermissions, GuestLinearAddress, InstructionProbeOutcome, InstructionProbeReport,
    Long64ProbeConfig, NonCanonicalAddress, PageFaultProbe, PageFaultReason, PageTableLevel,
    ProbeAccessError, ProbeConfigError, ProbeError, ProbeOutcome, ProbeReport, Translation4K,
    WalkEntry, WalkTrace, probe_instruction_access, probe_long64_4k,
};

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};
