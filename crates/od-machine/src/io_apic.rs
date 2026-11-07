use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, LocalApicHandle};
use std::sync::{Arc, Mutex};

pub const IO_APIC_MMIO_BASE: u64 = 0xfec0_0000;
pub const IO_APIC_MMIO_SIZE: u64 = 0x1000;
const MASK: u32 = 1 << 16;
const READ_ONLY_STATUS: u32 = (1 << 12) | (1 << 14);

/// A single opt-in INTIN4 fixed, physical, active-high edge route to APIC ID 0.
/// Register definitions follow Intel 82093AA, 290566-001, sections 3.1–3.2.4.
pub struct IoApic(Arc<Mutex<State>>);
#[derive(Clone)]
pub struct IoApicHandle(Arc<Mutex<State>>);

struct State {
    selector: u8,
    low: u32,
    high_line: bool,
    destination: LocalApicHandle,
}

impl IoApic {
    pub fn new(destination: LocalApicHandle) -> Self {
        Self(Arc::new(Mutex::new(State {
            selector: 0,
            low: MASK,
            high_line: false,
            destination,
        })))
    }
    pub fn handle(&self) -> IoApicHandle {
        IoApicHandle(Arc::clone(&self.0))
    }
}

impl IoApicHandle {
    /// Reset this device's selector, route and input level, without resetting the Local APIC.
    pub fn reset(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.selector = 0;
        state.low = MASK;
        state.high_line = false;
    }

    /// Set INTIN4. A failed delivery leaves the input level unchanged, allowing retry.
    /// The caller must serialize operations on this device and its destination APIC,
    /// including reset and global-enable changes, for their complete duration.
    pub fn set_intin4(&self, high: bool) -> Result<(), DeviceError> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if high && !state.high_line && state.low & MASK == 0 {
            state.destination.request_fixed_edge(state.low as u8)?;
        }
        state.high_line = high;
        Ok(())
    }
}

fn validate(access: DeviceAccess) -> Result<u64, DeviceError> {
    if access.space != IoSpace::Mmio {
        return Err(DeviceError::UnsupportedSpace);
    }
    if access.width != AccessWidth::Dword {
        return Err(DeviceError::UnsupportedWidth);
    }
    if access.offset % 4 != 0 {
        return Err(DeviceError::Misaligned);
    }
    match access.offset {
        0 | 0x10 => Ok(access.offset),
        _ => Err(DeviceError::UnknownOffset),
    }
}

impl BusDevice for IoApic {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = validate(access)?;
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match (offset, state.selector) {
            (0, _) => Ok(u64::from(state.selector)),
            (0x10, 0x18) => Ok(u64::from(state.low)),
            (0x10, 0x19) => Ok(0),
            _ => Err(DeviceError::UnknownOffset),
        }
    }
    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = validate(access)?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match (offset, state.selector) {
            (0, _) if value <= 0xff => {
                state.selector = value as u8;
                Ok(())
            }
            (0x10, 0x18)
                if value & !(u64::from(MASK | READ_ONLY_STATUS) | 0xff) == 0
                    && (0x10..=0xfe).contains(&(value & 0xff)) =>
            {
                state.low = value as u32 & !READ_ONLY_STATUS;
                Ok(())
            }
            (0x10, 0x19) if value == 0 => Ok(()),
            (0, _) | (0x10, 0x18 | 0x19) => Err(DeviceError::UnsupportedConfiguration),
            _ => Err(DeviceError::UnknownOffset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, LOCAL_APIC_MMIO_BASE,
        LOCAL_APIC_MMIO_SIZE, LocalApic, MachineBus, MachineMemoryMap, MemoryRegionKind,
        MemoryRegionSpec, MmioBinding, RamLimits,
    };

    fn setup() -> (MachineBus, IoApicHandle, LocalApicHandle) {
        let local = LocalApic::new();
        let apic = local.handle();
        apic.set_global_enabled(true);
        let io = IoApic::new(apic.clone());
        let handle = io.handle();
        let regions = [
            (IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE),
            (LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE),
        ];
        let map = MachineMemoryMap::new(
            36,
            regions
                .into_iter()
                .map(|(base, size)| {
                    MemoryRegionSpec::new(
                        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), size).unwrap(),
                        MemoryRegionKind::Mmio,
                    )
                })
                .collect(),
        )
        .unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(0),
            vec![Box::new(io), Box::new(local)],
            regions
                .into_iter()
                .enumerate()
                .map(|(i, (base, _))| MmioBinding {
                    region_start: GuestPhysicalAddress::new(base),
                    device_index: i,
                })
                .collect(),
            vec![],
        )
        .unwrap();
        (bus, handle, apic)
    }
    fn write(
        bus: &mut MachineBus,
        base: u64,
        offset: u64,
        value: u64,
    ) -> Result<(), BusAccessError> {
        bus.write_physical(
            GuestPhysicalAddress::new(base + offset),
            AccessWidth::Dword,
            value,
            |_| {},
        )
    }
    fn read(bus: &mut MachineBus, offset: u64) -> Result<u64, BusAccessError> {
        bus.read_physical(
            GuestPhysicalAddress::new(IO_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
        )
    }
    fn select(bus: &mut MachineBus, selector: u64) {
        write(bus, IO_APIC_MMIO_BASE, 0, selector).unwrap();
    }
    fn route(bus: &mut MachineBus, value: u64) {
        select(bus, 0x18);
        write(bus, IO_APIC_MMIO_BASE, 0x10, value).unwrap();
    }
    fn device_error(source: DeviceError) -> BusAccessError {
        BusAccessError::Device {
            device_index: 0,
            source,
        }
    }

    #[test]
    fn reset_registers_and_rejected_accesses_are_atomic() {
        let (mut bus, io, apic) = setup();
        assert_eq!(read(&mut bus, 0), Ok(0));
        select(&mut bus, 0x18);
        assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK)));
        route(&mut bus, u64::from(MASK) | 0x51);
        for invalid in [0, 15, 0xff, 0x151, 0x851, 0x2051, 0x8051, 0x20051] {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, invalid),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK) | 0x51));
        }
        select(&mut bus, 0x19);
        assert_eq!(read(&mut bus, 0x10), Ok(0));
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 1 << 24),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(read(&mut bus, 0x10), Ok(0));
        select(&mut bus, 1);
        assert_eq!(
            read(&mut bus, 0x10),
            Err(device_error(DeviceError::UnknownOffset))
        );
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0, 0x100),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(read(&mut bus, 0), Ok(1));
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 2, 0),
            Err(device_error(DeviceError::Misaligned))
        );
        assert_eq!(
            bus.write_physical(
                GuestPhysicalAddress::new(IO_APIC_MMIO_BASE),
                AccessWidth::Byte,
                0,
                |_| {}
            ),
            Err(device_error(DeviceError::UnsupportedWidth))
        );
        assert_eq!(read(&mut bus, 0), Ok(1));
        io.set_intin4(true).unwrap();
        io.reset();
        assert_eq!(read(&mut bus, 0), Ok(0));
        select(&mut bus, 0x18);
        assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK)));
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn edges_mask_vector_priority_and_eoi() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(MASK) | 0x51);
        io.set_intin4(true).unwrap();
        route(&mut bus, 0x51);
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), None);
        io.set_intin4(false).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), Some(0x51));
        io.set_intin4(false).unwrap();
        io.set_intin4(true).unwrap();
        route(&mut bus, u64::from(MASK) | 0x61);
        assert_eq!(apic.pending(), Some(0x51));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x80, 0x50).unwrap();
        assert_eq!(apic.eligible(), None);
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x80, 0x40).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        assert_eq!(apic.pending(), None);
        route(&mut bus, 0x61);
        io.set_intin4(false).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x61));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge(), None);
    }

    #[test]
    fn read_only_status_writes_are_ignored_and_route_still_delivers() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        for status in [1 << 12, 1 << 14, READ_ONLY_STATUS] {
            route(&mut bus, u64::from(MASK | status) | 0x51);
            assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK) | 0x51));
            route(&mut bus, u64::from(status) | 0x51);
            assert_eq!(read(&mut bus, 0x10), Ok(0x51));
            io.set_intin4(true).unwrap();
            assert_eq!(apic.acknowledge(), Some(0x51));
            assert_eq!(read(&mut bus, 0x10), Ok(0x51));
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
            io.set_intin4(false).unwrap();
        }
    }

    #[test]
    fn disabled_destination_fails_without_consuming_the_edge() {
        let (mut bus, io, apic) = setup();
        route(&mut bus, 0x70);
        apic.set_global_enabled(false);
        assert_eq!(
            io.set_intin4(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10), Ok(0x70));
        apic.set_global_enabled(true);
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), Some(0x70));
    }
}
