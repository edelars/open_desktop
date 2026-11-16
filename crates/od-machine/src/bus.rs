use crate::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory,
    ProbeReport, RamAccessOperation, RamLimits, TrackedAccessError, TrackedAccessOutcome,
    TrackedAccessReport, TrackedAccessRequest,
};
use od_isa::{
    PortIo,
    memory::{AccessContext, AccessKind, AccessOrigin},
};
use std::cell::RefCell;
use std::fmt;

// Must match the bounded 36-bit physical-address format accepted by
// page_walk.rs. Higher PTE bits can contain software metadata or NX.
const TRACKED_CODE_FRAME_MASK: u64 = ((1u64 << 36) - 1) & !0xfff;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PortAddress(u16);

impl PortAddress {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortRange {
    start: PortAddress,
    len: u32,
}

impl PortRange {
    pub fn new(start: PortAddress, len: u32) -> Result<Self, BusBuildError> {
        let end = u64::from(start.0) + u64::from(len);
        if len == 0 || end > 65_536 {
            return Err(BusBuildError::InvalidPortRange);
        }
        Ok(Self { start, len })
    }
    pub const fn start(self) -> PortAddress {
        self.start
    }
    pub const fn len(self) -> u32 {
        self.len
    }
    fn end(self) -> u32 {
        u32::from(self.start.0) + self.len
    }
    fn contains(self, start: PortAddress, len: u32) -> bool {
        let end = u32::from(start.0) + len;
        u32::from(start.0) >= u32::from(self.start.0) && end <= self.end()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessWidth {
    Byte,
    Word,
    Dword,
    Qword,
}

impl AccessWidth {
    pub const fn bytes(self) -> u8 {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Dword => 4,
            Self::Qword => 8,
        }
    }
    const fn mask(self) -> u64 {
        match self {
            Self::Byte => 0xff,
            Self::Word => 0xffff,
            Self::Dword => 0xffff_ffff,
            Self::Qword => u64::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoSpace {
    Mmio,
    Pio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceAccess {
    pub space: IoSpace,
    pub offset: u64,
    pub width: AccessWidth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceError {
    UnknownOffset,
    UnsupportedWidth,
    UnsupportedSpace,
    UnsupportedConfiguration,
    Busy,
    Misaligned,
    ReadOnly,
    WriteOnly,
    CrossesRegister,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device access error: {self:?}")
    }
}
impl std::error::Error for DeviceError {}

pub trait BusDevice {
    /// An error must be returned before any guest-visible state or external
    /// effect is committed. Implementations validate the complete operation
    /// before applying its synchronous effect.
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError>;
    /// An error must be returned before any guest-visible state or external
    /// effect is committed. Implementations validate the complete operation
    /// before applying its synchronous effect.
    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError>;

    /// The access of vCPU `cpu`. Only a device with one register window per processor (the
    /// Local APIC, S6-11) tells the processors apart; every other device serves all of them with
    /// `read`.
    fn read_from_cpu(&mut self, _cpu: usize, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.read(access)
    }
    /// The access of vCPU `cpu`; see [`BusDevice::read_from_cpu`].
    fn write_from_cpu(
        &mut self,
        _cpu: usize,
        access: DeviceAccess,
        value: u64,
    ) -> Result<(), DeviceError> {
        self.write(access, value)
    }

    /// A display device reports the framebuffer window its registers
    /// advertise. `MachineBus::new` checks it against the memory map; every
    /// other device keeps the default `None`.
    fn framebuffer_binding(&self) -> Option<FramebufferBinding> {
        None
    }

    /// Framebuffer window of the second screen (DISPLAY_DEVICE.md 16.3). Only a
    /// display device in the two-screen configuration reports one.
    fn secondary_framebuffer_binding(&self) -> Option<FramebufferBinding> {
        None
    }
}

/// Framebuffer GPA window advertised by a display device (`FB_BASE`/`FB_SIZE`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferBinding {
    pub base: GuestPhysicalAddress,
    pub size: u64,
}

/// Length of the register window an MMIO-bound display device requires.
pub(crate) const DISPLAY_REGISTER_WINDOW_BYTES: u64 = 0x1000;

/// Why a display device and the `Framebuffer` region of the map disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBindingError {
    /// The map has a `Framebuffer` region but no device advertises one.
    MissingDevice,
    /// A `Framebuffer` region of the map is not advertised by the device
    /// (its base and length match no `FB_BASE`/`FB_SIZE` of any screen).
    UnboundFramebufferRegion {
        region_start: GuestPhysicalAddress,
        region_len: u64,
    },
    /// A display device exists but the map has no `Framebuffer` region.
    MissingFramebufferRegion,
    /// Version 1 has exactly one screen.
    MultipleDevices,
    /// The region start or length differs from the device's `FB_BASE`/`FB_SIZE`.
    FramebufferMismatch {
        region_start: GuestPhysicalAddress,
        region_len: u64,
        device_base: GuestPhysicalAddress,
        device_size: u64,
    },
    /// The display device has no MMIO binding.
    RegisterWindowUnbound,
    /// The display device is bound to more than one MMIO region.
    MultipleRegisterWindows,
    /// The MMIO region bound to the display device is not 0x1000 bytes long.
    RegisterWindowLength { actual: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioBinding {
    pub region_start: GuestPhysicalAddress,
    pub device_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortRegionBinding {
    pub range: PortRange,
    pub device_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BusBuildError {
    InvalidPortRange,
    OverlappingPortRanges(PortRange, PortRange),
    InvalidDeviceIndex(usize),
    InvalidMmioBinding(GuestPhysicalAddress),
    MissingMmioBinding(GuestPhysicalAddress),
    DuplicateMmioBinding(GuestPhysicalAddress),
    Display(DisplayBindingError),
    Memory(MemoryAccessError),
}

impl fmt::Display for BusBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "machine bus build error: {self:?}")
    }
}
impl std::error::Error for BusBuildError {}

#[derive(Debug, Eq, PartialEq)]
pub enum BusAccessError {
    UnmappedPhysical(GuestPhysicalAddress),
    UnmappedPort(PortAddress),
    AddressOverflow,
    CrossesRegion,
    UnsupportedWidth,
    Memory(MemoryAccessError),
    Device {
        device_index: usize,
        source: DeviceError,
    },
}

impl fmt::Display for BusAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "machine bus access error: {self:?}")
    }
}
impl std::error::Error for BusAccessError {}

pub struct MachineBus {
    memory: PhysicalMemory,
    devices: Vec<Box<dyn BusDevice>>,
    mmio: Vec<MmioBinding>,
    /// Byte length of the region of each `mmio` binding (same order), for the set-step MMIO
    /// dispatch, which must not look at `memory` while the page walk holds it (S4-38L9s).
    mmio_lens: Vec<u64>,
    /// S4-38L8d: the set loop also serves the Local APIC and IOAPIC windows. `false` unless the
    /// board turned the APIC profile on (`enable_apic_mmio_windows`), so every other run keeps the ECAM
    /// window as the only MMIO of the set loop.
    apic_mmio_windows: bool,
    ports: Vec<PortRegionBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedCodeSnapshot {
    pub linear_base: u64,
    pub physical_base: GuestPhysicalAddress,
    pub bytes: Vec<u8>,
    pub backing_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedCodeSnapshotError {
    InvalidLength,
    InvalidAccessContext,
    Access(TrackedAccessError),
    NotRam(GuestPhysicalAddress),
    MissingTranslation,
    Physical(MemoryAccessError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedCodePatchError {
    InvalidAccessContext,
    Access(TrackedAccessError),
    PhysicalPageMismatch {
        expected: GuestPhysicalAddress,
        actual: GuestPhysicalAddress,
    },
}

impl MachineBus {
    /// Diagnostic-only read of one guest virtual range. The page walk uses a no-op A/D writer,
    /// and the final physical access is admitted only when its entire range is backed by RAM.
    pub(crate) fn read_virtual_ram_diagnostic(
        &mut self,
        paging: &Long64ProbeConfig,
        rip: u64,
        linear: GuestLinearAddress,
        context: AccessContext,
        output: &mut [u8],
    ) -> Result<(), String> {
        let mut offset = 0usize;
        while offset < output.len() {
            let linear_address = linear
                .get()
                .checked_add(u64::try_from(offset).map_err(|_| "virtual offset overflow")?)
                .ok_or("virtual address overflow")?;
            let bytes_to_page_end = 4096 - (linear_address as usize & 0xfff);
            let fragment_len = bytes_to_page_end.min(output.len() - offset);
            let physical = crate::page_walk::translate_long64_2m_ram_read_only(
                &mut self.memory,
                paging,
                rip,
                GuestLinearAddress::new(linear_address),
                context,
                AccessKind::Read,
            )
            .map_err(|error| format!("read-only translation failed: {error:?}"))?;
            let end = u128::from(physical.get()) + fragment_len as u128;
            let in_ram = self.memory.region_specs().any(|region| {
                region.kind == MemoryRegionKind::Ram
                    && physical.get() >= region.range.start().get()
                    && end <= region.range.end_exclusive()
            });
            if !in_ram {
                return Err("translated range is not wholly backed by RAM".into());
            }
            self.memory
                .read_bytes(physical, &mut output[offset..offset + fragment_len])
                .map_err(|error| format!("RAM read failed: {error:?}"))?;
            offset += fragment_len;
        }
        Ok(())
    }

    pub fn code_page_generation(
        &self,
        address: GuestPhysicalAddress,
    ) -> Result<u64, MemoryAccessError> {
        self.memory.code_page_generation(address)
    }

    /// Capture one executable byte range contained in a single guest 4 KiB
    /// page. The first byte is translated as an instruction fetch; the page
    /// walker enforces execute permissions and commits its A-bit effects.
    pub fn tracked_code_snapshot(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_start: u64,
        length: usize,
        context: AccessContext,
    ) -> Result<TrackedCodeSnapshot, TrackedCodeSnapshotError> {
        self.tracked_code_snapshot_with_callback(paging, linear_start, length, context, |_| {})
    }

    /// Capture one executable byte range and report each committed page-walk
    /// write in the order it occurs.
    pub fn tracked_code_snapshot_with_callback(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_start: u64,
        length: usize,
        context: AccessContext,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<TrackedCodeSnapshot, TrackedCodeSnapshotError> {
        if context.origin != AccessOrigin::Explicit {
            return Err(TrackedCodeSnapshotError::InvalidAccessContext);
        }
        let offset = (linear_start & 0xfff) as usize;
        if length == 0 || length > 0x1000 - offset {
            return Err(TrackedCodeSnapshotError::InvalidLength);
        }
        let report = self.access_tracked_ram(
            paging,
            TrackedAccessRequest {
                instruction_rip: linear_start,
                address: crate::GuestLinearAddress::new(linear_start),
                context,
                operation: RamAccessOperation::Fetch,
            },
            committed_write,
        );
        let physical = match report.outcome {
            Ok(TrackedAccessOutcome::Fetched(_)) => report
                .trace
                .entries()
                .iter()
                .flatten()
                .last()
                .map(|entry| entry.entry_value & TRACKED_CODE_FRAME_MASK)
                .ok_or(TrackedCodeSnapshotError::MissingTranslation)?,
            Ok(_) => return Err(TrackedCodeSnapshotError::MissingTranslation),
            Err(error) => return Err(TrackedCodeSnapshotError::Access(error)),
        };
        let physical_base = GuestPhysicalAddress::new(physical);
        let region = self
            .memory
            .locate_region_kind(physical_base, 0x1000)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        if region != MemoryRegionKind::Ram {
            return Err(TrackedCodeSnapshotError::NotRam(physical_base));
        }
        let byte_address = GuestPhysicalAddress::new(physical + offset as u64);
        let mut bytes = vec![0; length];
        self.memory
            .read_bytes(byte_address, &mut bytes)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        Ok(TrackedCodeSnapshot {
            linear_base: linear_start,
            physical_base,
            bytes,
            backing_generation: self
                .memory
                .code_page_generation(physical_base)
                .map_err(TrackedCodeSnapshotError::Physical)?,
        })
    }

    /// Store one loader patch byte through a writable guest virtual alias of
    /// the captured physical page. The read preflight verifies the alias
    /// before the tracked write is allowed to commit. Either walk may commit
    /// Accessed effects and the write walk may commit Dirty effects before a
    /// later failure.
    pub fn patch_tracked_code_byte(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_address: u64,
        expected_physical_page: GuestPhysicalAddress,
        value: u8,
        context: AccessContext,
    ) -> Result<GuestPhysicalAddress, TrackedCodePatchError> {
        self.patch_tracked_code_byte_with_callback(
            paging,
            linear_address,
            expected_physical_page,
            value,
            context,
            |_| {},
        )
    }

    /// Store one code byte and report committed A/D and operand writes in
    /// operation order. Failed operations report only writes that committed
    /// before the failure.
    pub fn patch_tracked_code_byte_with_callback(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_address: u64,
        expected_physical_page: GuestPhysicalAddress,
        value: u8,
        context: AccessContext,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<GuestPhysicalAddress, TrackedCodePatchError> {
        if context.origin != AccessOrigin::Explicit {
            return Err(TrackedCodePatchError::InvalidAccessContext);
        }
        let request = |operation| TrackedAccessRequest {
            instruction_rip: linear_address,
            address: crate::GuestLinearAddress::new(linear_address),
            context,
            operation,
        };
        let preflight = self.access_tracked_ram(
            paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            &mut committed_write,
        );
        if let Err(error) = preflight.outcome {
            return Err(TrackedCodePatchError::Access(error));
        }
        let actual = preflight
            .trace
            .entries()
            .iter()
            .flatten()
            .last()
            .map(|entry| {
                GuestPhysicalAddress::new(
                    (entry.entry_value & TRACKED_CODE_FRAME_MASK) | (linear_address & 0xfff),
                )
            })
            .ok_or_else(|| {
                TrackedCodePatchError::Access(TrackedAccessError::DeviceUnavailable {
                    address: expected_physical_page,
                })
            })?;
        if actual.get() & TRACKED_CODE_FRAME_MASK != expected_physical_page.get() {
            return Err(TrackedCodePatchError::PhysicalPageMismatch {
                expected: expected_physical_page,
                actual,
            });
        }
        let write = self.access_tracked_ram(
            paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: u64::from(value),
            }),
            &mut committed_write,
        );
        match write.outcome {
            Ok(TrackedAccessOutcome::Written) => Ok(actual),
            Ok(_) => Err(TrackedCodePatchError::Access(
                TrackedAccessError::DeviceUnavailable { address: actual },
            )),
            Err(error) => Err(TrackedCodePatchError::Access(error)),
        }
    }

    /// Loader write used by the inactive-code session. PhysicalMemory
    /// validates the whole access and advances its conservative code version
    /// only after the RAM bytes commit.
    pub fn patch_physical_ram(
        &mut self,
        address: GuestPhysicalAddress,
        bytes: &[u8],
    ) -> Result<(), MemoryAccessError> {
        self.patch_physical_ram_with_callback(address, bytes, |_| {})
    }

    /// Write physical RAM and report the exact range after the bytes commit.
    /// Validation and generation advancement remain owned by PhysicalMemory.
    pub fn patch_physical_ram_with_callback(
        &mut self,
        address: GuestPhysicalAddress,
        bytes: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.memory.write_bytes(address, bytes, committed_write)
    }
}

/// Disjoint PIO view used while the CPU fetch adapter borrows physical RAM.
pub(crate) struct MachineBusPortIo<'a> {
    devices: &'a mut [Box<dyn BusDevice>],
    ports: &'a [PortRegionBinding],
}

/// The device and port halves of one set-loop instruction (S4-38L9s). The CPU core takes the
/// memory (which routes MMIO data accesses) and the port view in the same call, and both reach the
/// same device table (the PCI device is bound to ECAM MMIO and to the conf1 ports), so the table
/// sits in a `RefCell`. One instruction makes its accesses one after another, so the cell is never
/// borrowed twice; if it ever is, that is a typed `Busy` error and not a panic.
pub(crate) struct MachineBusSetStepIo<'a> {
    devices: RefCell<&'a mut [Box<dyn BusDevice>]>,
    ports: &'a [PortRegionBinding],
    mmio: &'a [MmioBinding],
    mmio_lens: &'a [u64],
    windows: &'static [crate::page_walk::MmioWindow],
}

/// The port view of [`MachineBusSetStepIo`].
pub(crate) struct SetStepPortIo<'a, 'b>(&'b MachineBusSetStepIo<'a>);

/// MMIO dispatch of one tracked data access, for the page walker of the set loop.
pub(crate) trait SetStepMmio {
    /// Every physical range this dispatch serves: the ECAM window, and with the APIC profile
    /// (S4-38L8d) the Local APIC and IOAPIC windows. Every other MMIO operand of the set loop
    /// stays the typed error it was before S4-38L9s.
    fn windows(&self) -> &[crate::page_walk::MmioWindow];

    fn dispatch(
        &self,
        address: GuestPhysicalAddress,
        width: u8,
        operation: RamAccessOperation,
    ) -> Result<u64, crate::page_walk::MmioDispatchError>;
}

impl<'a> MachineBusSetStepIo<'a> {
    pub(crate) fn port_io(&self) -> SetStepPortIo<'a, '_> {
        SetStepPortIo(self)
    }
}

/// The ECAM window: the PCI configuration device is the one MMIO device the set loop's own profile
/// needs (S4-38L9s). Without the APIC profile it is the only MMIO the set loop serves; the other
/// MMIO (Local APIC, IOAPIC, display, NVMe) is not admitted.
const ECAM_WINDOW: crate::page_walk::MmioWindow = (
    crate::PCI_ECAM_DESCRIPTOR.base,
    crate::PCI_ECAM_DESCRIPTOR.size,
);

const SET_STEP_WINDOWS: [crate::page_walk::MmioWindow; 1] = [ECAM_WINDOW];
/// S4-38L8d: with the APIC profile the set loop also reaches the Local APIC and the IOAPIC (one 4 KiB
/// window each, MADT addresses), and still nothing else.
const SET_STEP_APIC_WINDOWS: [crate::page_walk::MmioWindow; 3] = [
    ECAM_WINDOW,
    (crate::LOCAL_APIC_MMIO_BASE, crate::LOCAL_APIC_MMIO_SIZE),
    (crate::IO_APIC_MMIO_BASE, crate::IO_APIC_MMIO_SIZE),
];

impl SetStepMmio for MachineBusSetStepIo<'_> {
    fn windows(&self) -> &[crate::page_walk::MmioWindow] {
        self.windows
    }

    fn dispatch(
        &self,
        address: GuestPhysicalAddress,
        width: u8,
        operation: RamAccessOperation,
    ) -> Result<u64, crate::page_walk::MmioDispatchError> {
        use crate::page_walk::MmioDispatchError::Backend;
        let width = match width {
            8 => AccessWidth::Qword,
            4 => AccessWidth::Dword,
            2 => AccessWidth::Word,
            1 => AccessWidth::Byte,
            _ => return Err(Backend(format!("unsupported MMIO width {width}"))),
        };
        let len = u64::from(width.bytes());
        let served = self.windows.iter().any(|&(base, size)| {
            address.get() >= base
                && u128::from(address.get()) + u128::from(len)
                    <= u128::from(base) + u128::from(size)
        });
        if !served {
            return Err(Backend(format!(
                "MMIO address {address:?} is outside the windows the set loop serves"
            )));
        }
        let (binding, _) = self
            .mmio
            .iter()
            .zip(self.mmio_lens)
            .find(|(binding, region_len)| {
                binding.region_start <= address
                    && u128::from(address.get()) + u128::from(len)
                        <= u128::from(binding.region_start.get()) + u128::from(**region_len)
            })
            .ok_or_else(|| {
                Backend("MMIO range is not wholly inside one device region".to_owned())
            })?;
        let access = DeviceAccess {
            space: IoSpace::Mmio,
            offset: address.get() - binding.region_start.get(),
            width,
        };
        let mut devices = self
            .devices
            .try_borrow_mut()
            .map_err(|_| Backend("the device table is already borrowed".to_owned()))?;
        let device = &mut devices[binding.device_index];
        match operation {
            RamAccessOperation::Read { .. } => device
                .read(access)
                .map(|value| value & width.mask())
                .map_err(|error| {
                    Backend(format!(
                        "MMIO device {} read failed: {error:?}",
                        binding.device_index
                    ))
                }),
            RamAccessOperation::Write { value, .. } => device
                .write(access, value & width.mask())
                .map(|()| 0)
                .map_err(|error| {
                    Backend(format!(
                        "MMIO device {} write failed: {error:?}",
                        binding.device_index
                    ))
                }),
            RamAccessOperation::Fetch => Err(Backend(
                "instruction fetch from MMIO is unsupported".to_owned(),
            )),
            RamAccessOperation::ReadPseudoDescriptor => {
                Err(Backend("pseudo-descriptor reads are RAM-only".to_owned()))
            }
        }
    }
}

impl<'a, 'b> SetStepPortIo<'a, 'b> {
    fn devices(
        &self,
    ) -> Result<std::cell::RefMut<'b, &'a mut [Box<dyn BusDevice>]>, BusAccessError> {
        self.0
            .devices
            .try_borrow_mut()
            .map_err(|_| BusAccessError::Device {
                device_index: usize::MAX,
                source: DeviceError::Busy,
            })
    }
}

impl PortIo for SetStepPortIo<'_, '_> {
    type Error = BusAccessError;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        let mut devices = self.devices()?;
        read_port_from(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
        )
        .map(|value| value as u8)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        let mut devices = self.devices()?;
        write_port_to(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
            u32::from(value),
        )
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        let mut devices = self.devices()?;
        read_port_from(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Word,
        )
        .map(|value| value as u16)
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        let mut devices = self.devices()?;
        write_port_to(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Word,
            u32::from(value),
        )
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        let mut devices = self.devices()?;
        read_port_from(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
        )
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        let mut devices = self.devices()?;
        write_port_to(
            &mut devices,
            self.0.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
            value,
        )
    }
}

impl MachineBus {
    /// Memory, ports and MMIO of one set-loop instruction (S4-38L9s); the cell inside the
    /// returned value lets the port view and the MMIO dispatch share the device table.
    pub(crate) fn split_for_set_step(&mut self) -> (&mut PhysicalMemory, MachineBusSetStepIo<'_>) {
        let Self {
            memory,
            devices,
            mmio,
            mmio_lens,
            apic_mmio_windows,
            ports,
        } = self;
        (
            memory,
            MachineBusSetStepIo {
                devices: RefCell::new(devices.as_mut_slice()),
                ports,
                mmio,
                mmio_lens,
                windows: if *apic_mmio_windows {
                    &SET_STEP_APIC_WINDOWS
                } else {
                    &SET_STEP_WINDOWS
                },
            },
        )
    }

    /// S4-38L8d: let the set loop reach the Local APIC and IOAPIC windows (off by default). Only the
    /// APIC profile of the Linux board turns it on.
    pub(crate) fn enable_apic_mmio_windows(&mut self) {
        self.apic_mmio_windows = true;
    }

    pub(crate) fn split_for_pio_step(&mut self) -> (&mut PhysicalMemory, MachineBusPortIo<'_>) {
        let Self {
            memory,
            devices,
            ports,
            ..
        } = self;
        (memory, MachineBusPortIo { devices, ports })
    }

    /// A tracked access by vCPU `vcpu`: an MMIO operand reaches the device as that vCPU's access.
    pub(crate) fn access_tracked(
        &mut self,
        vcpu: usize,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        let Self {
            memory,
            devices,
            mmio,
            ..
        } = self;
        let regions = memory.region_specs().collect::<Vec<_>>();
        crate::page_walk::access_long64_4k_with_mmio(
            memory,
            paging,
            request,
            committed_write,
            |address, width, operation| {
                let width = match width {
                    8 => AccessWidth::Qword,
                    4 => AccessWidth::Dword,
                    2 => AccessWidth::Word,
                    1 => AccessWidth::Byte,
                    _ => {
                        return Err(crate::page_walk::MmioDispatchError::Backend(format!(
                            "unsupported MMIO width {width}"
                        )));
                    }
                };
                let len = u64::from(width.bytes());
                let region = regions
                    .iter()
                    .find(|region| {
                        region.kind == MemoryRegionKind::Mmio
                            && region.range.start() <= address
                            && u128::from(address.get()) + u128::from(len)
                                <= region.range.end_exclusive()
                    })
                    .ok_or_else(|| {
                        crate::page_walk::MmioDispatchError::Backend(
                            "MMIO range is not wholly inside one device region".to_owned(),
                        )
                    })?;
                let binding = mmio
                    .iter()
                    .find(|binding| binding.region_start == region.range.start())
                    .ok_or_else(|| {
                        crate::page_walk::MmioDispatchError::Backend(format!(
                            "unmapped MMIO address {address:?}"
                        ))
                    })?;
                let access = DeviceAccess {
                    space: IoSpace::Mmio,
                    offset: address.get() - region.range.start().get(),
                    width,
                };
                match operation {
                    RamAccessOperation::Read { .. } => devices[binding.device_index]
                        .read_from_cpu(vcpu, access)
                        .map(|value| value & width.mask())
                        .map_err(|error| {
                            crate::page_walk::MmioDispatchError::Backend(format!(
                                "MMIO device {} read failed: {error:?}",
                                binding.device_index
                            ))
                        }),
                    RamAccessOperation::Write { value, .. } => devices[binding.device_index]
                        .write_from_cpu(vcpu, access, value & width.mask())
                        .map(|()| 0)
                        .map_err(|error| {
                            crate::page_walk::MmioDispatchError::Backend(format!(
                                "MMIO device {} write failed: {error:?}",
                                binding.device_index
                            ))
                        }),
                    RamAccessOperation::Fetch => Err(crate::page_walk::MmioDispatchError::Backend(
                        "instruction fetch from MMIO is unsupported".to_owned(),
                    )),
                    RamAccessOperation::ReadPseudoDescriptor => {
                        Err(crate::page_walk::MmioDispatchError::Backend(
                            "pseudo-descriptor reads are RAM-only".to_owned(),
                        ))
                    }
                }
            },
        )
    }

    pub(crate) fn access_tracked_ram(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        crate::page_walk::access_long64_4k_ram(&mut self.memory, paging, request, committed_write)
    }

    pub(crate) fn access_tracked_2m_ram(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        crate::page_walk::access_long64_2m_ram(&mut self.memory, paging, request, committed_write)
    }

    pub(crate) fn probe_tracked_ram_access(
        &self,
        paging: &Long64ProbeConfig,
        address: GuestLinearAddress,
        access: AccessKind,
        context: AccessContext,
    ) -> ProbeReport {
        crate::page_walk::probe_long64_4k(&self.memory, paging, address, access, context)
    }

    pub(crate) fn tracked_physical_range_is_ram(&self, range: GuestPhysicalRange) -> bool {
        self.memory
            .locate_region_kind(range.start(), range.len())
            .is_ok_and(|kind| kind == MemoryRegionKind::Ram)
    }

    pub(crate) fn read_tracked_bounded_ram_read(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, crate::TrackedAccessError> {
        crate::page_walk::read_long64_4k_bounded_ram_read(
            &mut self.memory,
            paging,
            request,
            committed_write,
        )
    }

    pub(crate) fn read_tracked_bounded_ram_read_with_fault_trace(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
        fault_trace: &mut Option<crate::WalkTrace>,
    ) -> Result<u64, crate::TrackedAccessError> {
        crate::page_walk::read_long64_4k_bounded_ram_read_with_fault_trace(
            &mut self.memory,
            paging,
            request,
            committed_write,
            fault_trace,
        )
    }

    pub(crate) fn memory_kind(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<MemoryRegionKind, BusAccessError> {
        self.memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)
    }

    /// Cross-checks the display device's advertised `FB_BASE`/`FB_SIZE` and
    /// register window against the map (see `DISPLAY_DEVICE.md`, 7.3).
    fn check_display_binding(
        map: &MachineMemoryMap,
        devices: &[Box<dyn BusDevice>],
        mmio: &[MmioBinding],
    ) -> Result<(), BusBuildError> {
        let mut displays = devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| device.framebuffer_binding().map(|b| (index, b)));
        let display = displays.next();
        if displays.next().is_some() {
            return Err(BusBuildError::Display(DisplayBindingError::MultipleDevices));
        }
        let secondary =
            display.and_then(|(index, _)| devices[index].secondary_framebuffer_binding());
        let regions: Vec<&MemoryRegionSpec> = map
            .regions()
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Framebuffer)
            .collect();
        let (device_index, primary) = match (display, regions.is_empty()) {
            (None, true) => return Ok(()),
            (None, false) => {
                return Err(BusBuildError::Display(DisplayBindingError::MissingDevice));
            }
            (Some(_), true) => {
                return Err(BusBuildError::Display(
                    DisplayBindingError::MissingFramebufferRegion,
                ));
            }
            (Some((index, binding)), false) => (index, binding),
        };
        // Every advertised window must match exactly one region of the map, and
        // every region must be advertised (DISPLAY_DEVICE.md 16.3).
        let advertised: Vec<FramebufferBinding> =
            std::iter::once(primary).chain(secondary).collect();
        if advertised.len() != regions.len() {
            let unmatched = regions
                .iter()
                .find(|region| {
                    !advertised
                        .iter()
                        .any(|b| region.range.start() == b.base && region.range.len() == b.size)
                })
                .copied();
            if let Some(region) = unmatched {
                return Err(BusBuildError::Display(
                    DisplayBindingError::UnboundFramebufferRegion {
                        region_start: region.range.start(),
                        region_len: region.range.len(),
                    },
                ));
            }
        }
        for binding in &advertised {
            let region = regions.iter().find(|region| {
                region.range.start() == binding.base && region.range.len() == binding.size
            });
            if region.is_none() {
                let nearest = regions[0];
                return Err(BusBuildError::Display(
                    DisplayBindingError::FramebufferMismatch {
                        region_start: nearest.range.start(),
                        region_len: nearest.range.len(),
                        device_base: binding.base,
                        device_size: binding.size,
                    },
                ));
            }
        }
        let mut windows = mmio
            .iter()
            .filter(|binding| binding.device_index == device_index);
        let window = windows.next().ok_or(BusBuildError::Display(
            DisplayBindingError::RegisterWindowUnbound,
        ))?;
        if windows.next().is_some() {
            return Err(BusBuildError::Display(
                DisplayBindingError::MultipleRegisterWindows,
            ));
        }
        // Every MMIO binding was already validated to name an `Mmio` region.
        let window_len = map
            .regions()
            .iter()
            .find(|region| region.range.start() == window.region_start)
            .map(|region| region.range.len())
            .unwrap_or(0);
        if window_len != DISPLAY_REGISTER_WINDOW_BYTES {
            return Err(BusBuildError::Display(
                DisplayBindingError::RegisterWindowLength { actual: window_len },
            ));
        }
        Ok(())
    }

    /// Copies frame bytes out of the `Framebuffer` region at `base`. Used only
    /// by the display handle between guest quanta.
    pub(crate) fn read_framebuffer_bytes(
        &self,
        base: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        self.memory.read_framebuffer_bytes(base, output)
    }

    pub fn new(
        map: MachineMemoryMap,
        ram_limits: RamLimits,
        devices: Vec<Box<dyn BusDevice>>,
        mut mmio: Vec<MmioBinding>,
        mut ports: Vec<PortRegionBinding>,
    ) -> Result<Self, BusBuildError> {
        for binding in &mmio {
            if binding.device_index >= devices.len() {
                return Err(BusBuildError::InvalidDeviceIndex(binding.device_index));
            }
        }
        for binding in &ports {
            if binding.device_index >= devices.len() {
                return Err(BusBuildError::InvalidDeviceIndex(binding.device_index));
            }
        }
        mmio.sort_by_key(|binding| binding.region_start);
        for pair in mmio.windows(2) {
            if pair[0].region_start == pair[1].region_start {
                return Err(BusBuildError::DuplicateMmioBinding(pair[0].region_start));
            }
        }
        for binding in &mmio {
            if !map.regions().iter().any(|region| {
                region.kind == MemoryRegionKind::Mmio
                    && region.range.start() == binding.region_start
            }) {
                return Err(BusBuildError::InvalidMmioBinding(binding.region_start));
            }
        }
        for region in map
            .regions()
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Mmio)
        {
            if !mmio
                .iter()
                .any(|binding| binding.region_start == region.range.start())
            {
                return Err(BusBuildError::MissingMmioBinding(region.range.start()));
            }
        }
        Self::check_display_binding(&map, &devices, &mmio)?;
        ports.sort_by_key(|binding| binding.range.start);
        for pair in ports.windows(2) {
            if pair[0].range.end() > u32::from(pair[1].range.start.0) {
                return Err(BusBuildError::OverlappingPortRanges(
                    pair[0].range,
                    pair[1].range,
                ));
            }
        }
        let mmio_lens = mmio
            .iter()
            .map(|binding| {
                map.regions()
                    .iter()
                    .find(|region| {
                        region.kind == MemoryRegionKind::Mmio
                            && region.range.start() == binding.region_start
                    })
                    .map_or(0, |region| region.range.len())
            })
            .collect();
        let memory = PhysicalMemory::new(&map, ram_limits).map_err(BusBuildError::Memory)?;
        Ok(Self {
            memory,
            devices,
            mmio,
            mmio_lens,
            apic_mmio_windows: false,
            ports,
        })
    }

    pub fn read_physical(
        &mut self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
    ) -> Result<u64, BusAccessError> {
        let len = u64::from(width.bytes());
        match self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?
        {
            MemoryRegionKind::Ram => {
                read_ram(&self.memory, address, width).map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Mmio => {
                let region = self
                    .memory
                    .region_specs()
                    .find(|region| {
                        region.kind == MemoryRegionKind::Mmio
                            && region.range.start() <= address
                            && u128::from(address.get()) + u128::from(len)
                                <= region.range.end_exclusive()
                    })
                    .ok_or(BusAccessError::CrossesRegion)?;
                let device_index = self
                    .mmio
                    .iter()
                    .find(|binding| binding.region_start == region.range.start())
                    .ok_or(BusAccessError::UnmappedPhysical(address))?
                    .device_index;
                let access = DeviceAccess {
                    space: IoSpace::Mmio,
                    offset: address.get() - region.range.start().get(),
                    width,
                };
                self.devices[device_index]
                    .read(access)
                    .map(|v| v & width.mask())
                    .map_err(|source| BusAccessError::Device {
                        device_index,
                        source,
                    })
            }
            // Pixel memory is read as data like RAM; ROM bytes are read-only data.
            MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom => {
                read_ram(&self.memory, address, width).map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Reserved => {
                Err(BusAccessError::Memory(MemoryAccessError::UnreadableRegion(
                    self.memory.locate_region_kind(address, len).unwrap(),
                )))
            }
        }
    }

    pub fn write_physical(
        &mut self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), BusAccessError> {
        let len = u64::from(width.bytes());
        match self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?
        {
            MemoryRegionKind::Ram => {
                write_ram(&mut self.memory, address, width, value, committed_write)
                    .map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Mmio => {
                let region = self
                    .memory
                    .region_specs()
                    .find(|region| {
                        region.kind == MemoryRegionKind::Mmio
                            && region.range.start() <= address
                            && u128::from(address.get()) + u128::from(len)
                                <= region.range.end_exclusive()
                    })
                    .ok_or(BusAccessError::CrossesRegion)?;
                let device_index = self
                    .mmio
                    .iter()
                    .find(|binding| binding.region_start == region.range.start())
                    .ok_or(BusAccessError::UnmappedPhysical(address))?
                    .device_index;
                let access = DeviceAccess {
                    space: IoSpace::Mmio,
                    offset: address.get() - region.range.start().get(),
                    width,
                };
                self.devices[device_index]
                    .write(access, value & width.mask())
                    .map_err(|source| BusAccessError::Device {
                        device_index,
                        source,
                    })
            }
            // Pixel writes never reach the committed-write callback: nothing
            // depends on framebuffer bytes (no fetch, no page tables).
            MemoryRegionKind::Framebuffer => {
                write_ram(&mut self.memory, address, width, value, |_| {})
                    .map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Rom | MemoryRegionKind::Reserved => {
                Err(BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(
                    self.memory.locate_region_kind(address, len).unwrap(),
                )))
            }
        }
    }

    pub fn read_port(
        &mut self,
        port: PortAddress,
        width: AccessWidth,
    ) -> Result<u32, BusAccessError> {
        read_port_from(&mut self.devices, &self.ports, port, width)
    }

    pub fn write_port(
        &mut self,
        port: PortAddress,
        width: AccessWidth,
        value: u32,
    ) -> Result<(), BusAccessError> {
        write_port_to(&mut self.devices, &self.ports, port, width, value)
    }

    pub(crate) fn read_flat_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), BusAccessError> {
        let len = u64::try_from(output.len()).map_err(|_| BusAccessError::AddressOverflow)?;
        let kind = self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?;
        match kind {
            MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom => self
                .memory
                .read_bytes(address, output)
                .map_err(BusAccessError::Memory),
            MemoryRegionKind::Mmio => {
                let width = access_width(output.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let value = self.read_physical(address, width)?;
                output.copy_from_slice(&value.to_le_bytes()[..output.len()]);
                Ok(())
            }
            kind @ MemoryRegionKind::Reserved => Err(BusAccessError::Memory(
                MemoryAccessError::UnreadableRegion(kind),
            )),
        }
    }

    /// Machine-side one-time fill of the ROM region (firmware image at power
    /// on). Guest-visible writes keep returning `ReadOnlyRegion(Rom)`.
    pub(crate) fn load_rom_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
    ) -> Result<(), BusAccessError> {
        self.memory
            .load_rom_bytes(address, input)
            .map_err(BusAccessError::Memory)
    }

    pub(crate) fn write_flat_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), BusAccessError> {
        let len = u64::try_from(input.len()).map_err(|_| BusAccessError::AddressOverflow)?;
        let kind = self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?;
        match kind {
            MemoryRegionKind::Ram => self
                .memory
                .write_bytes(address, input, committed_write)
                .map_err(BusAccessError::Memory),
            // `PhysicalMemory::write_bytes` skips the callback for Framebuffer.
            MemoryRegionKind::Framebuffer => self
                .memory
                .write_bytes(address, input, committed_write)
                .map_err(BusAccessError::Memory),
            MemoryRegionKind::Mmio => {
                let width = access_width(input.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let mut bytes = [0; 8];
                bytes[..input.len()].copy_from_slice(input);
                self.write_physical(address, width, u64::from_le_bytes(bytes), committed_write)
            }
            kind @ (MemoryRegionKind::Rom | MemoryRegionKind::Reserved) => Err(
                BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(kind)),
            ),
        }
    }
}

impl PortIo for MachineBusPortIo<'_> {
    type Error = BusAccessError;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        read_port_from(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
        )
        .map(|value| value as u8)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        write_port_to(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
            u32::from(value),
        )
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        read_port_from(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Word,
        )
        .map(|value| value as u16)
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        write_port_to(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Word,
            u32::from(value),
        )
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        read_port_from(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
        )
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        write_port_to(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
            value,
        )
    }
}

fn read_port_from(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
) -> Result<u32, BusAccessError> {
    if width == AccessWidth::Qword {
        return Err(BusAccessError::UnsupportedWidth);
    }
    let len = u32::from(width.bytes());
    let binding = ports
        .iter()
        .find(|binding| binding.range.contains(port, len))
        .ok_or_else(|| {
            if ports.iter().any(|b| {
                u32::from(port.0) >= u32::from(b.range.start.0) && u32::from(port.0) < b.range.end()
            }) {
                BusAccessError::CrossesRegion
            } else {
                BusAccessError::UnmappedPort(port)
            }
        })?;
    let access = DeviceAccess {
        space: IoSpace::Pio,
        offset: u64::from(port.0 - binding.range.start.0),
        width,
    };
    devices[binding.device_index]
        .read(access)
        .map(|v| (v & width.mask()) as u32)
        .map_err(|source| BusAccessError::Device {
            device_index: binding.device_index,
            source,
        })
}

fn write_port_to(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
    value: u32,
) -> Result<(), BusAccessError> {
    if width == AccessWidth::Qword {
        return Err(BusAccessError::UnsupportedWidth);
    }
    let len = u32::from(width.bytes());
    let binding = ports
        .iter()
        .find(|binding| binding.range.contains(port, len))
        .ok_or_else(|| {
            if ports.iter().any(|b| {
                u32::from(port.0) >= u32::from(b.range.start.0) && u32::from(port.0) < b.range.end()
            }) {
                BusAccessError::CrossesRegion
            } else {
                BusAccessError::UnmappedPort(port)
            }
        })?;
    let access = DeviceAccess {
        space: IoSpace::Pio,
        offset: u64::from(port.0 - binding.range.start.0),
        width,
    };
    devices[binding.device_index]
        .write(access, u64::from(value) & width.mask())
        .map_err(|source| BusAccessError::Device {
            device_index: binding.device_index,
            source,
        })
}

fn access_width(len: usize) -> Option<AccessWidth> {
    match len {
        1 => Some(AccessWidth::Byte),
        2 => Some(AccessWidth::Word),
        4 => Some(AccessWidth::Dword),
        8 => Some(AccessWidth::Qword),
        _ => None,
    }
}

fn map_memory_error(error: MemoryAccessError) -> BusAccessError {
    match error {
        MemoryAccessError::Unmapped(address) => BusAccessError::UnmappedPhysical(address),
        MemoryAccessError::AccessCrossesRegion(_) => BusAccessError::CrossesRegion,
        MemoryAccessError::AddressOverflow => BusAccessError::AddressOverflow,
        error => BusAccessError::Memory(error),
    }
}

fn read_ram(
    memory: &PhysicalMemory,
    address: GuestPhysicalAddress,
    width: AccessWidth,
) -> Result<u64, MemoryAccessError> {
    Ok(match width {
        AccessWidth::Byte => u64::from(memory.read_u8(address)?),
        AccessWidth::Word => u64::from(memory.read_u16(address)?),
        AccessWidth::Dword => u64::from(memory.read_u32(address)?),
        AccessWidth::Qword => memory.read_u64(address)?,
    })
}
fn write_ram(
    memory: &mut PhysicalMemory,
    address: GuestPhysicalAddress,
    width: AccessWidth,
    value: u64,
    callback: impl FnMut(GuestPhysicalRange),
) -> Result<(), MemoryAccessError> {
    match width {
        AccessWidth::Byte => memory.write_u8(address, value as u8, callback),
        AccessWidth::Word => memory.write_u16(address, value as u16, callback),
        AccessWidth::Dword => memory.write_u32(address, value as u32, callback),
        AccessWidth::Qword => memory.write_u64(address, value, callback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct DeviceState {
        calls: usize,
        value: u64,
    }

    struct CountingDevice(Arc<Mutex<DeviceState>>);

    impl BusDevice for CountingDevice {
        fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 16 {
                return Err(DeviceError::UnknownOffset);
            }
            if access.offset % u64::from(access.width.bytes()) != 0 {
                return Err(DeviceError::Misaligned);
            }
            Ok(state.value)
        }

        fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 16 {
                return Err(DeviceError::UnknownOffset);
            }
            if access.offset % u64::from(access.width.bytes()) != 0 {
                return Err(DeviceError::Misaligned);
            }
            if access.offset == 4 {
                return Err(DeviceError::ReadOnly);
            }
            state.value = value;
            Ok(())
        }
    }

    fn range(start: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
    }

    fn setup(with_adjacent_ram: bool) -> (MachineBus, Arc<Mutex<DeviceState>>) {
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let mut regions = vec![
            crate::MemoryRegionSpec::new(range(0x1000, 0x100), MemoryRegionKind::Ram),
            crate::MemoryRegionSpec::new(range(0x2000, 0x100), MemoryRegionKind::Mmio),
        ];
        if with_adjacent_ram {
            regions.push(crate::MemoryRegionSpec::new(
                range(0x2100, 0x10),
                MemoryRegionKind::Ram,
            ));
        }
        let map = MachineMemoryMap::new(36, regions).unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(0x200),
            vec![Box::new(CountingDevice(Arc::clone(&state)))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(0x2000),
                device_index: 0,
            }],
            vec![PortRegionBinding {
                range: PortRange::new(PortAddress::new(0x20), 16).unwrap(),
                device_index: 0,
            }],
        )
        .unwrap();
        (bus, state)
    }

    fn diagnostic_page_bus(map_second_page: bool) -> (MachineBus, Long64ProbeConfig) {
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(
                range(0, 0x10_000),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(0x10_000), vec![], vec![], vec![]).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4020, 0x7007),
            (0x4028, if map_second_page { 0x9007 } else { 0 }),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7ff8), &[0x11; 8])
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x8000), &[0x22; 8])
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x9000), &[0x33; 8])
            .unwrap();
        let paging = Long64ProbeConfig::new(
            0x8001_0033,
            0x1000,
            1 << 5,
            (1 << 8) | (1 << 10) | (1 << 11),
        )
        .unwrap();
        (bus, paging)
    }

    #[test]
    fn diagnostic_ram_read_translates_each_virtual_page_fragment() {
        let (mut bus, paging) = diagnostic_page_bus(true);
        let pte_before = bus
            .read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
            .unwrap();
        let next_pte_before = bus
            .read_physical(GuestPhysicalAddress::new(0x4028), AccessWidth::Qword)
            .unwrap();
        let mut output = [0; 16];
        bus.read_virtual_ram_diagnostic(
            &paging,
            0x1234,
            GuestLinearAddress::new(0x4ff8),
            AccessContext::default(),
            &mut output,
        )
        .unwrap();
        assert_eq!(
            output,
            [0x11; 8].into_iter().chain([0x33; 8]).collect::<Vec<_>>()[..]
        );
        assert_ne!(&output[8..], &[0x22; 8]);
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
                .unwrap(),
            pte_before
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x4028), AccessWidth::Qword)
                .unwrap(),
            next_pte_before
        );
    }

    #[test]
    fn diagnostic_ram_read_refuses_unmapped_second_virtual_page() {
        let (mut bus, paging) = diagnostic_page_bus(false);
        let mut output = [0xaa; 16];
        let error = bus
            .read_virtual_ram_diagnostic(
                &paging,
                0x1234,
                GuestLinearAddress::new(0x4ff8),
                AccessContext::default(),
                &mut output,
            )
            .unwrap_err();
        assert!(error.contains("read-only translation failed"));
        assert_eq!(&output[8..], &[0xaa; 8]);
    }

    #[test]
    fn tracked_code_snapshot_reads_one_ram_page_through_two_aliases() {
        let ram = range(0, 0x9000);
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus = MachineBus::new(map, RamLimits::new(0x9000), vec![], vec![], vec![]).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4020, 0x7007),
            (0x4028, 0x7007),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        // Bits 52–62 are accepted software metadata; they must not leak into
        // the translated physical frame extracted from the leaf PTE.
        for address in [0x4020, 0x4028] {
            let pte = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                pte | (1 << 52) | (1 << 58),
                |_| {},
            )
            .unwrap();
        }
        let code = [0x48, 0xb8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7000), &code)
            .unwrap();
        let paging = Long64ProbeConfig::new(
            0x8001_0033,
            0x1000,
            1 << 5,
            (1 << 8) | (1 << 10) | (1 << 11),
        )
        .unwrap();

        let first = bus
            .tracked_code_snapshot(&paging, 0x4000, code.len(), AccessContext::default())
            .unwrap();
        let second = bus
            .tracked_code_snapshot(&paging, 0x5000, code.len(), AccessContext::default())
            .unwrap();
        assert_eq!(first.physical_base, GuestPhysicalAddress::new(0x7000));
        assert_eq!(second.physical_base, first.physical_base);
        assert_eq!(first.bytes, code);
        assert_eq!(second.bytes, code);
        assert_eq!(first.backing_generation, second.backing_generation);
        assert_eq!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xaa,
                AccessContext::default(),
            )
            .unwrap(),
            GuestPhysicalAddress::new(0x7002)
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x7002), AccessWidth::Byte)
                .unwrap(),
            0xaa
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x1000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x2000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x3000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );

        let pte = bus
            .read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
            .unwrap();
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            pte | (1 << 63),
            |_| {},
        )
        .unwrap();
        assert!(matches!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), AccessContext::default()),
            Err(TrackedCodeSnapshotError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            (pte | (1 << 5)) & !(1 << 63),
            |_| {},
        )
        .unwrap();

        // The loader API models an explicit fetch/write at the supplied CPL.
        // Descriptor-table origin has architecturally privileged page checks,
        // so it is rejected instead of being accepted as a CPL3 code access.
        let supervisor_only = pte & !(1 << 2);
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            supervisor_only | (1 << 5),
            |_| {},
        )
        .unwrap();
        let cpl3 = AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring3,
            ..AccessContext::default()
        };
        assert!(matches!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), cpl3),
            Err(TrackedCodeSnapshotError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        assert!(matches!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xbb,
                cpl3,
            ),
            Err(TrackedCodePatchError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        let descriptor_origin = AccessContext {
            origin: od_isa::memory::AccessOrigin::DescriptorTable,
            ..cpl3
        };
        assert_eq!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), descriptor_origin),
            Err(TrackedCodeSnapshotError::InvalidAccessContext)
        );
        assert_eq!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xbb,
                descriptor_origin,
            ),
            Err(TrackedCodePatchError::InvalidAccessContext)
        );
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            pte | (1 << 5),
            |_| {},
        )
        .unwrap();

        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7002), &[0xaa])
            .unwrap();
        let patched = bus
            .tracked_code_snapshot(&paging, 0x5000, code.len(), AccessContext::default())
            .unwrap();
        assert_eq!(patched.bytes[2], 0xaa);
        assert!(patched.backing_generation > first.backing_generation);

        // Reset walker A/D bits and require the callback API to report the
        // read preflight's A updates, then the write walk's D update and the
        // committed operand byte, in architectural operation order.
        for address in [0x1000, 0x2000, 0x3000, 0x4020] {
            let entry = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                entry & !((1 << 5) | (1 << 6)),
                |_| {},
            )
            .unwrap();
        }
        let mut committed = Vec::new();
        bus.patch_tracked_code_byte_with_callback(
            &paging,
            0x4002,
            GuestPhysicalAddress::new(0x7000),
            0xbb,
            AccessContext::default(),
            |range| committed.push(range),
        )
        .unwrap();
        assert_eq!(
            committed,
            [0x1000, 0x2000, 0x3000, 0x4020, 0x4020, 0x7002].map(|address| {
                GuestPhysicalRange::new(
                    GuestPhysicalAddress::new(address),
                    if address == 0x7002 { 1 } else { 8 },
                )
                .unwrap()
            })
        );
    }

    #[test]
    fn tracked_code_callbacks_preserve_commits_before_snapshot_and_patch_errors() {
        let make_bus = || {
            let ram = range(0, 0x9000);
            let map = MachineMemoryMap::new(
                36,
                vec![crate::MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)],
            )
            .unwrap();
            let mut bus =
                MachineBus::new(map, RamLimits::new(0x9000), vec![], vec![], vec![]).unwrap();
            for (address, value) in [
                (0x7000, 0x2007), // PML4[0] resides in the code backing page.
                (0x7400, 0x2007), // PML4[128] is used by the failing fetch.
                (0x2000, 0x3007),
                (0x3010, 0x4007),
                (0x4008, 0x7007),
            ] {
                bus.write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Qword,
                    value,
                    |_| {},
                )
                .unwrap();
            }
            let paging = Long64ProbeConfig::new(
                0x8001_0033,
                0x7000,
                1 << 5,
                (1 << 8) | (1 << 10) | (1 << 11),
            )
            .unwrap();
            (bus, paging)
        };
        let ring3 = AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring3,
            ..AccessContext::default()
        };

        // The fetch sets PML4 and PDPT Accessed bits, then faults on the
        // absent PD entry. Only those two committed writes are reported.
        let (mut snapshot_bus, paging) = make_bus();
        let fault_linear = 0x4000_0000_0000;
        let mut snapshot_writes = Vec::new();
        let snapshot_error = snapshot_bus
            .tracked_code_snapshot_with_callback(&paging, fault_linear, 1, ring3, |range| {
                snapshot_writes.push(range)
            })
            .unwrap_err();
        assert!(matches!(
            snapshot_error,
            TrackedCodeSnapshotError::Access(TrackedAccessError::PageFault { ref record, .. })
                if record.cr2_value() == fault_linear
                    && record.fault().error_code == Some(0x14)
        ));
        assert_eq!(
            snapshot_writes,
            [0x7400, 0x2000].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );

        // The patch read preflight commits A updates through the root, PDPT,
        // PDE and read-only PTE, then the write walk faults with #PF(P=1,W=1,U=1).
        let (mut patch_bus, paging) = make_bus();
        patch_bus
            .write_physical(
                GuestPhysicalAddress::new(0x4008),
                AccessWidth::Qword,
                0x7005,
                |_| {},
            )
            .unwrap();
        let mut patch_writes = Vec::new();
        let patch_error = patch_bus
            .patch_tracked_code_byte_with_callback(
                &paging,
                0x0040_1202,
                GuestPhysicalAddress::new(0x7000),
                0xaa,
                ring3,
                |range| patch_writes.push(range),
            )
            .unwrap_err();
        assert!(matches!(
            patch_error,
            TrackedCodePatchError::Access(TrackedAccessError::PageFault { ref record, .. })
                if record.cr2_value() == 0x0040_1202
                    && record.fault().error_code == Some(0x7)
        ));
        assert_eq!(
            patch_writes,
            [0x7000, 0x2000, 0x3010, 0x4008].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
        assert_eq!(
            patch_bus
                .read_physical(GuestPhysicalAddress::new(0x7000), AccessWidth::Qword)
                .unwrap(),
            0x2027
        );

        // A mismatched expected frame fails after the read preflight. Its A
        // updates remain observable and no write-walk/operand callback occurs.
        let (mut mismatch_bus, paging) = make_bus();
        let mut mismatch_writes = Vec::new();
        let mismatch = mismatch_bus
            .patch_tracked_code_byte_with_callback(
                &paging,
                0x0040_1202,
                GuestPhysicalAddress::new(0x8000),
                0xaa,
                ring3,
                |range| mismatch_writes.push(range),
            )
            .unwrap_err();
        assert_eq!(
            mismatch,
            TrackedCodePatchError::PhysicalPageMismatch {
                expected: GuestPhysicalAddress::new(0x8000),
                actual: GuestPhysicalAddress::new(0x7202),
            }
        );
        assert_eq!(
            mismatch_writes,
            [0x7000, 0x2000, 0x3010, 0x4008].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
    }

    #[test]
    fn dispatches_all_supported_widths_and_rejects_pio_qword() {
        let (mut bus, state) = setup(false);
        let mut commits = 0;
        for width in [
            AccessWidth::Byte,
            AccessWidth::Word,
            AccessWidth::Dword,
            AccessWidth::Qword,
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(0x2000),
                width,
                0x8877_6655_4433_2211,
                |_| commits += 1,
            )
            .unwrap();
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(0x2000), width)
                    .unwrap(),
                0x8877_6655_4433_2211 & width.mask()
            );
        }
        for width in [AccessWidth::Byte, AccessWidth::Word, AccessWidth::Dword] {
            bus.write_port(PortAddress::new(0x20), width, 0x8877_6655)
                .unwrap();
            assert_eq!(
                bus.read_port(PortAddress::new(0x20), width).unwrap(),
                (0x8877_6655u64 & width.mask()) as u32
            );
        }
        let before = state.lock().unwrap().calls;
        assert_eq!(
            bus.read_port(PortAddress::new(0x20), AccessWidth::Qword),
            Err(BusAccessError::UnsupportedWidth)
        );
        assert_eq!(
            bus.write_port(PortAddress::new(0x20), AccessWidth::Qword, 1),
            Err(BusAccessError::UnsupportedWidth)
        );
        assert_eq!(state.lock().unwrap().calls, before);
        assert_eq!(commits, 0);
    }

    #[test]
    fn device_errors_are_distinct_and_dispatch_once() {
        let (mut bus, state) = setup(false);
        assert!(matches!(
            bus.read_physical(GuestPhysicalAddress::new(0x2010), AccessWidth::Byte),
            Err(BusAccessError::Device {
                source: DeviceError::UnknownOffset,
                ..
            })
        ));
        assert!(matches!(
            bus.read_physical(GuestPhysicalAddress::new(0x2001), AccessWidth::Word),
            Err(BusAccessError::Device {
                source: DeviceError::Misaligned,
                ..
            })
        ));
        assert!(matches!(
            bus.write_physical(
                GuestPhysicalAddress::new(0x2004),
                AccessWidth::Byte,
                1,
                |_| {}
            ),
            Err(BusAccessError::Device {
                source: DeviceError::ReadOnly,
                ..
            })
        ));
        assert_eq!(state.lock().unwrap().calls, 3);
        assert_eq!(state.lock().unwrap().value, 0);
    }

    #[test]
    fn pre_dispatch_rejections_do_not_call_device_or_change_ram() {
        let (mut bus, state) = setup(true);
        let mut commits = 0;
        bus.write_physical(
            GuestPhysicalAddress::new(0x1000),
            AccessWidth::Dword,
            0x1234_5678,
            |_| commits += 1,
        )
        .unwrap();
        assert_eq!(commits, 1);
        let mut rejected_commits = 0;
        assert_eq!(
            bus.write_physical(
                GuestPhysicalAddress::new(0x20fe),
                AccessWidth::Dword,
                7,
                |_| rejected_commits += 1
            ),
            Err(BusAccessError::CrossesRegion)
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x1000), AccessWidth::Dword)
                .unwrap(),
            0x1234_5678
        );
        assert_eq!(rejected_commits, 0);
        assert_eq!(state.lock().unwrap().calls, 0);
    }

    #[test]
    fn validates_port_overlap_and_mmio_bindings() {
        let overlap = PortRange::new(PortAddress::new(0x20), 4).unwrap();
        let overlap2 = PortRange::new(PortAddress::new(0x23), 2).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(
                range(0x2000, 16),
                MemoryRegionKind::Mmio,
            )],
        )
        .unwrap();
        let device: Vec<Box<dyn BusDevice>> = vec![Box::new(CountingDevice(Arc::new(Mutex::new(
            DeviceState::default(),
        ))))];
        assert!(matches!(
            MachineBus::new(
                map.clone(),
                RamLimits::new(0),
                vec![Box::new(CountingDevice(Arc::new(Mutex::new(
                    DeviceState::default()
                ))))],
                vec![],
                vec![]
            ),
            Err(BusBuildError::MissingMmioBinding(_))
        ));
        assert!(matches!(
            MachineBus::new(
                map.clone(),
                RamLimits::new(0),
                device,
                vec![MmioBinding {
                    region_start: GuestPhysicalAddress::new(0x2000),
                    device_index: 0
                }],
                vec![
                    PortRegionBinding {
                        range: overlap,
                        device_index: 0
                    },
                    PortRegionBinding {
                        range: overlap2,
                        device_index: 0
                    }
                ]
            ),
            Err(BusBuildError::OverlappingPortRanges(_, _))
        ));
        assert_eq!(
            PortRange::new(PortAddress::new(u16::MAX), 2),
            Err(BusBuildError::InvalidPortRange)
        );
        assert_eq!(
            PortRange::new(PortAddress::new(0), u32::MAX),
            Err(BusBuildError::InvalidPortRange)
        );
        assert_eq!(
            PortRange::new(PortAddress::new(1), u32::MAX),
            Err(BusBuildError::InvalidPortRange)
        );
        let final_port = PortRange::new(PortAddress::new(u16::MAX), 1).unwrap();
        assert_eq!(final_port.end(), 65_536);
    }
}
