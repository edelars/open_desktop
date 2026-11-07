use crate::apic_pair::{LEVEL, MASK, PairState, REMOTE_IRR};
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, LocalApicHandle};
use std::sync::{Arc, Mutex};

pub const IO_APIC_MMIO_BASE: u64 = 0xfec0_0000;
pub const IO_APIC_MMIO_SIZE: u64 = 0x1000;
const READ_ONLY_STATUS: u32 = (1 << 12) | REMOTE_IRR;

/// One fixed, physical, active-high INTIN4 route to APIC ID 0.
pub struct IoApic(Arc<Mutex<PairState>>);
#[derive(Clone)]
pub struct IoApicHandle(Arc<Mutex<PairState>>);

impl IoApic {
    pub fn new(destination: LocalApicHandle) -> Self {
        Self(destination.0)
    }
    pub fn handle(&self) -> IoApicHandle {
        IoApicHandle(Arc::clone(&self.0))
    }
}
impl IoApicHandle {
    /// Reset only the IOAPIC; a level request awaiting EOI vetoes reset.
    pub fn reset(&self) -> Result<(), DeviceError> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).reset_io()
    }
    /// Caller serializes complete device, reset and CPU helper operations.
    pub fn set_intin4(&self, high: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_line(high)
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
        match (offset, state.route.selector) {
            (0, _) => Ok(u64::from(state.route.selector)),
            (0x10, 0x18) => Ok(u64::from(
                state.route.low
                    | if state.route.remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    },
            )),
            (0x10, 0x19) => Ok(0),
            _ => Err(DeviceError::UnknownOffset),
        }
    }
    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = validate(access)?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match (offset, state.route.selector) {
            (0, _) if value <= 0xff => {
                state.route.selector = value as u8;
                Ok(())
            }
            (0x10, 0x18)
                if value & !(u64::from(MASK | LEVEL | READ_ONLY_STATUS) | 0xff) == 0
                    && (0x10..=0xfe).contains(&(value & 0xff)) =>
            {
                state.set_low(value as u32 & !READ_ONLY_STATUS)
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
        apic.set_global_enabled(true).unwrap();
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
        for invalid in [0, 15, 0xff, 0x151, 0x851, 0x2051, 0x20051] {
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
        io.reset().unwrap();
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
        apic.set_global_enabled(false).unwrap();
        assert_eq!(
            io.set_intin4(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10), Ok(0x70));
        apic.set_global_enabled(true).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), Some(0x70));
    }
    fn local_read(bus: &mut MachineBus, offset: u64) -> u64 {
        bus.read_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
        )
        .unwrap()
    }

    #[test]
    fn level_acceptance_eoi_mask_priority_and_nested_edge() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(LEVEL) | 0x51);
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x80, 0x50).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(apic.eligible(), None);
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert_ne!(local_read(&mut bus, 0x1a0) & (1 << 17), 0);
        io.set_intin4(true).unwrap();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x80, 0).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        apic.request_fixed_edge(0x71).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x71));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert_eq!(apic.pending(), None);
        for _ in 0..2 {
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
            assert_eq!(apic.pending(), Some(0x51));
            assert_eq!(apic.acknowledge(), Some(0x51));
        }
        route(&mut bus, u64::from(LEVEL | MASK) | 0x51);
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        route(&mut bus, u64::from(LEVEL) | 0x51);
        assert_eq!(apic.pending(), Some(0x51));
        io.set_intin4(false).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        assert_ne!(local_read(&mut bus, 0x1a0) & (1 << 17), 0);
    }

    #[test]
    fn level_deassert_after_ack_and_masked_assert_unmask() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(LEVEL | MASK) | 0x51);
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), None);
        route(&mut bus, u64::from(LEVEL) | 0x51);
        assert_eq!(apic.acknowledge(), Some(0x51));
        route(&mut bus, u64::from(LEVEL | READ_ONLY_STATUS) | 0x51);
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        io.set_intin4(false).unwrap();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
    }

    #[test]
    fn masked_inactive_level_reserves_vector_against_edges() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(LEVEL | MASK) | 0x51);
        let before = io.0.lock().unwrap().clone();
        assert!(!before.route.high_line);
        assert_eq!(
            apic.request_fixed_edge(0x51),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(*io.0.lock().unwrap(), before);
        assert_eq!(
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x300, 0x4_4051),
            Err(BusAccessError::Device {
                device_index: 1,
                source: DeviceError::UnsupportedConfiguration
            })
        );
        assert_eq!(*io.0.lock().unwrap(), before);
    }

    #[test]
    fn level_delivery_requires_software_enable_and_vetoes_disable() {
        let (mut bus, io, apic) = setup();
        route(&mut bus, u64::from(LEVEL) | 0x51);
        let before = io.0.lock().unwrap().clone();
        assert_eq!(
            io.set_intin4(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(*io.0.lock().unwrap(), before);
        route(&mut bus, u64::from(LEVEL | MASK) | 0x51);
        io.set_intin4(true).unwrap();
        let before = io.0.lock().unwrap().clone();
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(*io.0.lock().unwrap(), before);
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(LEVEL) | 0x51);
        assert_eq!(apic.pending(), Some(0x51));
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        let before = io.0.lock().unwrap().clone();
        assert_eq!(
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0xff),
            Err(BusAccessError::Device {
                device_index: 1,
                source: DeviceError::UnsupportedConfiguration
            })
        );
        assert_eq!(*io.0.lock().unwrap(), before);
    }

    #[test]
    fn independent_resets_preserve_other_controller() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x80, 0x20).unwrap();
        apic.request_fixed_edge(0x61).unwrap();
        route(&mut bus, 0x51);
        io.set_intin4(true).unwrap();
        let local_before = io.0.lock().unwrap().local.clone();
        io.reset().unwrap();
        assert_eq!(io.0.lock().unwrap().local, local_before);
        assert_eq!(apic.pending(), Some(0x61));
        assert_eq!(apic.acknowledge(), Some(0x61));

        route(&mut bus, u64::from(LEVEL | MASK) | 0x71);
        io.set_intin4(true).unwrap();
        select(&mut bus, 0x19);
        let route_before = io.0.lock().unwrap().route.clone();
        apic.reset().unwrap();
        assert_eq!(io.0.lock().unwrap().route, route_before);
        assert_eq!(read(&mut bus, 0).unwrap(), 0x19);
        select(&mut bus, 0x18);
        assert_eq!(
            read(&mut bus, 0x10).unwrap(),
            u64::from(LEVEL | MASK) | 0x71
        );
        assert!(io.0.lock().unwrap().route.high_line);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn independent_resets_veto_accepted_level_without_mutation() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, u64::from(LEVEL) | 0x51);
        io.set_intin4(true).unwrap();
        for acknowledged in [false, true] {
            if acknowledged {
                assert_eq!(apic.acknowledge(), Some(0x51));
                io.set_intin4(false).unwrap();
            }
            let before = io.0.lock().unwrap().clone();
            assert_eq!(io.reset(), Err(DeviceError::UnsupportedConfiguration));
            assert_eq!(*io.0.lock().unwrap(), before);
            assert_eq!(apic.reset(), Err(DeviceError::UnsupportedConfiguration));
            assert_eq!(*io.0.lock().unwrap(), before);
        }
        apic.reset_pair().unwrap();
        assert_eq!(*io.0.lock().unwrap(), PairState::default());
    }

    #[test]
    fn level_configuration_conflicts_and_coordinated_reset() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        apic.request_fixed_edge(0x51).unwrap();
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(apic.acknowledge(), Some(0x51));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        route(&mut bus, u64::from(LEVEL | READ_ONLY_STATUS) | 0x51);
        assert_eq!(read(&mut bus, 0x10).unwrap(), u64::from(LEVEL) | 0x51);
        assert_eq!(
            apic.request_fixed_edge(0x51),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0x300, 0x4_4051),
            Err(BusAccessError::Device {
                device_index: 1,
                source: DeviceError::UnsupportedConfiguration
            })
        );
        io.set_intin4(true).unwrap();
        assert_eq!(
            apic.set_global_enabled(false),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0xff),
            Err(BusAccessError::Device {
                device_index: 1,
                source: DeviceError::UnsupportedConfiguration
            })
        );
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x61),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(apic.pending(), Some(0x51));
        apic.reset_pair().unwrap();
        assert_eq!(apic.pending(), None);
        select(&mut bus, 0x18);
        assert_eq!(read(&mut bus, 0x10).unwrap(), u64::from(MASK));
        assert_eq!(local_read(&mut bus, 0x1a0), 0);
    }
}
