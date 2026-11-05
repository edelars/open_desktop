use crate::{
    GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig, MachineMemoryMap,
    MemoryAccessError, MemoryRegionKind, PhysicalMemory, RamAccessOperation, RamLimits,
    TrackedAccessReport, TrackedAccessRequest,
};
use od_isa::PortIo;
use std::fmt;

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
    ports: Vec<PortRegionBinding>,
}

/// Disjoint PIO view used while the CPU fetch adapter borrows physical RAM.
pub(crate) struct MachineBusPortIo<'a> {
    devices: &'a mut [Box<dyn BusDevice>],
    ports: &'a [PortRegionBinding],
}

impl MachineBus {
    pub(crate) fn split_for_pio_step(&mut self) -> (&mut PhysicalMemory, MachineBusPortIo<'_>) {
        let Self {
            memory,
            devices,
            ports,
            ..
        } = self;
        (memory, MachineBusPortIo { devices, ports })
    }

    pub(crate) fn access_tracked(
        &mut self,
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
                        .read(access)
                        .map(|value| value & width.mask())
                        .map_err(|error| {
                            crate::page_walk::MmioDispatchError::Backend(format!(
                                "MMIO device {} read failed: {error:?}",
                                binding.device_index
                            ))
                        }),
                    RamAccessOperation::Write { value, .. } => devices[binding.device_index]
                        .write(access, value & width.mask())
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
                }
            },
        )
    }

    pub(crate) fn read_tracked_cross_page_ram(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, crate::TrackedAccessError> {
        crate::page_walk::read_long64_4k_cross_page_ram(
            &mut self.memory,
            paging,
            request,
            committed_write,
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
        ports.sort_by_key(|binding| binding.range.start);
        for pair in ports.windows(2) {
            if pair[0].range.end() > u32::from(pair[1].range.start.0) {
                return Err(BusBuildError::OverlappingPortRanges(
                    pair[0].range,
                    pair[1].range,
                ));
            }
        }
        let memory = PhysicalMemory::new(&map, ram_limits).map_err(BusBuildError::Memory)?;
        Ok(Self {
            memory,
            devices,
            mmio,
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
            _ => Err(BusAccessError::Memory(MemoryAccessError::UnreadableRegion(
                self.memory.locate_region_kind(address, len).unwrap(),
            ))),
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
            MemoryRegionKind::Ram => self
                .memory
                .read_bytes(address, output)
                .map_err(BusAccessError::Memory),
            MemoryRegionKind::Mmio => {
                let width = access_width(output.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let value = self.read_physical(address, width)?;
                output.copy_from_slice(&value.to_le_bytes()[..output.len()]);
                Ok(())
            }
            kind => Err(BusAccessError::Memory(MemoryAccessError::UnreadableRegion(
                kind,
            ))),
        }
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
            MemoryRegionKind::Mmio => {
                let width = access_width(input.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let mut bytes = [0; 8];
                bytes[..input.len()].copy_from_slice(input);
                self.write_physical(address, width, u64::from_le_bytes(bytes), committed_write)
            }
            kind => Err(BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(
                kind,
            ))),
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
