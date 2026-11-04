//! Guest physical memory, MMU, devices, virtual CPUs, and virtual time.

mod bus;
mod identity_memory_adapter;
mod physical_memory;

pub use bus::{
    AccessWidth, BusAccessError, BusBuildError, BusDevice, DeviceAccess, DeviceError, IoSpace,
    MachineBus, MmioBinding, PortAddress, PortRange, PortRegionBinding,
};

pub use identity_memory_adapter::{DiagnosticFlatAccess, IdentityMemoryAdapter};

pub use physical_memory::{
    GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, PhysicalWidthError, RamLimits,
};
