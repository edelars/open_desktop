use crate::apic_pair::{LEVEL, MASK, PairState, REMOTE_IRR};
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, LocalApicHandle};
use std::sync::{Arc, Mutex};

pub const IO_APIC_MMIO_BASE: u64 = 0xfec0_0000;
pub const IO_APIC_MMIO_SIZE: u64 = 0x1000;
const READ_ONLY_STATUS: u32 = (1 << 12) | REMOTE_IRR;
const POLARITY_LOW: u32 = 1 << 13;
/// Version 0x20 (as on ICH/QEMU-style IOAPICs), MRE (bits 23:16) = 23 (24 inputs), as in the
/// MADT. The EOI register at offset 0x40 is not modeled, nor is register 0x03 (UnknownOffset).
const IO_APIC_VERSION: u32 = 0x0017_0020;

/// IOAPIC register window; INTIN4 (edge/level) and INTIN2 (edge only) route to APIC ID 0.
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
    /// INTIN2 (GSI 2), edge only (S4-29e). Caller serializes complete device operations.
    pub fn set_intin2(&self, high: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_line2(high)
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

/// Indirect register selected through IOREGSEL.
enum Register {
    Id,
    Version,
    ArbitrationId,
    /// Low (`high == false`) or high dword of redirection entry `pin`.
    Redirection {
        pin: usize,
        high: bool,
    },
}

fn register(selector: u8) -> Option<Register> {
    match selector {
        0 => Some(Register::Id),
        1 => Some(Register::Version),
        2 => Some(Register::ArbitrationId),
        0x10..=0x3f => Some(Register::Redirection {
            pin: usize::from(selector - 0x10) / 2,
            high: selector & 1 != 0,
        }),
        _ => None,
    }
}

/// Fixed, physical entry: mask, level bit, vector. Masked entries may hold any vector (reset
/// and Linux `clear_IO_APIC` leave vector 0); unmasked ones need 0x10..=0xFE. INTIN4 is
/// active-high only; entries without a line (all others) may additionally hold the active-low
/// polarity bit while masked (the MADT declares the SCI on GSI 9 as level/active-low), and
/// unmasking such an entry is refused because nothing could be delivered.
fn valid_low(pin: usize, value: u64) -> bool {
    let masked = value & u64::from(MASK) != 0;
    let mut allowed = u64::from(MASK | LEVEL | READ_ONLY_STATUS) | 0xff;
    if pin != 4 && masked {
        allowed |= u64::from(POLARITY_LOW);
    }
    let vector = value & 0xff;
    value & !allowed == 0 && (masked || (0x10..=0xfe).contains(&vector))
}

impl BusDevice for IoApic {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = validate(access)?;
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if offset == 0 {
            return Ok(u64::from(state.route.selector));
        }
        let value = match register(state.route.selector).ok_or(DeviceError::UnknownOffset)? {
            Register::Id | Register::ArbitrationId => u32::from(state.route.id) << 24,
            Register::Version => IO_APIC_VERSION,
            Register::Redirection { high: true, .. } => 0,
            Register::Redirection { pin: 4, .. } => {
                state.route.low
                    | if state.route.remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    }
            }
            Register::Redirection { pin, .. } => state.route.pins[pin],
        };
        Ok(u64::from(value))
    }
    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = validate(access)?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if offset == 0 {
            if value > 0xff {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            state.route.selector = value as u8;
            return Ok(());
        }
        match register(state.route.selector).ok_or(DeviceError::UnknownOffset)? {
            Register::Id if value & !(0xf << 24) == 0 => {
                state.route.id = (value >> 24) as u8;
                Ok(())
            }
            Register::Redirection { high: true, .. } if value == 0 => Ok(()),
            Register::Redirection { pin, high: false } if valid_low(pin, value) => {
                let low = value as u32 & !READ_ONLY_STATUS;
                if pin == 4 {
                    state.set_low(low)
                } else if pin == 2 {
                    state.set_low2(low)
                } else {
                    state.route.pins[pin] = low;
                    Ok(())
                }
            }
            // Read-only Version/Arbitration ID, reserved ID bits, reserved or unsupported
            // redirection bits: refused before any state change.
            _ => Err(DeviceError::UnsupportedConfiguration),
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
        select(&mut bus, 3); // S4-26c: selector 1 is now Version
        assert_eq!(
            read(&mut bus, 0x10),
            Err(device_error(DeviceError::UnknownOffset))
        );
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0, 0x100),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(read(&mut bus, 0), Ok(3));
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
        assert_eq!(read(&mut bus, 0), Ok(3));
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

    // ---- S4-26c: ID/Version/Arbitration ID, window, invalid entries ----------------------

    fn window(bus: &mut MachineBus, selector: u64) -> Result<u64, BusAccessError> {
        select(bus, selector);
        read(bus, 0x10)
    }
    fn window_write(bus: &mut MachineBus, selector: u64, value: u64) -> Result<(), BusAccessError> {
        select(bus, selector);
        write(bus, IO_APIC_MMIO_BASE, 0x10, value)
    }
    fn snapshot(io: &IoApicHandle) -> PairState {
        io.0.lock().unwrap().clone()
    }

    #[test]
    fn id_version_and_arbitration_id_through_the_window() {
        let (mut bus, _io, _apic) = setup();
        assert_eq!(window(&mut bus, 0), Ok(0x0100_0000));
        assert_eq!(window(&mut bus, 1), Ok(0x0017_0020));
        assert_eq!(window(&mut bus, 2), Ok(0x0100_0000));
        let version = window(&mut bus, 1).unwrap();
        assert_eq!(version & 0xff, 0x20);
        assert_eq!((version >> 16) & 0xff, 23);
        // IOREGSEL still reads back the selector.
        assert_eq!(read(&mut bus, 0), Ok(1));
    }

    #[test]
    fn id_is_writable_in_27_24_only_and_version_arbitration_are_read_only() {
        let (mut bus, io, _apic) = setup();
        window_write(&mut bus, 0, 0x0500_0000).unwrap();
        assert_eq!(window(&mut bus, 0), Ok(0x0500_0000));
        assert_eq!(
            window(&mut bus, 2),
            Ok(0x0500_0000),
            "arbitration ID follows ID"
        );
        for (selector, value) in [
            (0, 0x1),
            (0, 0x0100_0001),
            (0, 0x1000_0000),
            (0, 0x8000_0000),
            (1, 0x0017_0020),
            (1, 0),
            (2, 0x0600_0000),
            (2, 0),
        ] {
            select(&mut bus, selector);
            let before = snapshot(&io);
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                Err(device_error(DeviceError::UnsupportedConfiguration)),
                "selector {selector} value {value:#x}"
            );
            assert_eq!(snapshot(&io), before);
        }
        assert_eq!(window(&mut bus, 1), Ok(0x0017_0020));
        assert_eq!(window(&mut bus, 0), Ok(0x0500_0000));
        io.reset().unwrap();
        assert_eq!(window(&mut bus, 0), Ok(0x0100_0000), "reset reloads ID 1");
    }

    #[test]
    fn window_reads_and_writes_all_24_redirection_entries() {
        let (mut bus, io, apic) = setup();
        for pin in 0..24u64 {
            let (low, high) = (0x10 + 2 * pin, 0x11 + 2 * pin);
            assert_eq!(
                window(&mut bus, low),
                Ok(u64::from(MASK)),
                "reset pin {pin}"
            );
            assert_eq!(window(&mut bus, high), Ok(0));
        }
        for pin in 0..24u64 {
            let trigger = if pin % 2 == 0 { 0 } else { u64::from(LEVEL) };
            window_write(&mut bus, 0x10 + 2 * pin, trigger | (0x40 + pin)).unwrap();
        }
        for pin in 0..24u64 {
            let trigger = if pin % 2 == 0 { 0 } else { u64::from(LEVEL) };
            assert_eq!(window(&mut bus, 0x10 + 2 * pin), Ok(trigger | (0x40 + pin)));
            window_write(&mut bus, 0x11 + 2 * pin, 0).unwrap();
            assert_eq!(
                window_write(&mut bus, 0x11 + 2 * pin, 1 << 24),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(window(&mut bus, 0x11 + 2 * pin), Ok(0));
        }
        // Entries 24.. do not exist.
        for selector in [0x40, 0x41, 0x7e, 0xff] {
            assert_eq!(
                window(&mut bus, selector),
                Err(device_error(DeviceError::UnknownOffset))
            );
        }
        // Only INTIN4 has a line: other unmasked entries never deliver.
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        window_write(&mut bus, 0x18, u64::from(MASK) | 0x44).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), None);
        // Mask, vector and trigger written through the window change INTIN4 behavior.
        window_write(&mut bus, 0x18, 0x45).unwrap();
        io.set_intin4(false).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x45));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        io.set_intin4(false).unwrap();
        window_write(&mut bus, 0x18, u64::from(LEVEL) | 0x46).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x46));
        assert_ne!(window(&mut bus, 0x18).unwrap() & u64::from(REMOTE_IRR), 0);
        for pin in [3u64, 5] {
            assert_eq!(
                window(&mut bus, 0x10 + 2 * pin).unwrap() & u64::from(REMOTE_IRR),
                0,
                "Remote IRR belongs to INTIN4 only"
            );
        }
    }

    #[test]
    fn unknown_select_does_not_change_state() {
        let (mut bus, io, _apic) = setup();
        window_write(&mut bus, 0x18, u64::from(MASK) | 0x51).unwrap();
        let unknown = (3..=0x0f).chain([0x40, 0x41, 0x80, 0xfe, 0xff]);
        for selector in unknown {
            select(&mut bus, selector);
            let before = snapshot(&io);
            assert_eq!(
                read(&mut bus, 0x10),
                Err(device_error(DeviceError::UnknownOffset)),
                "read selector {selector:#x}"
            );
            for value in [0, 0x51, 0x0200_0000, u64::MAX] {
                assert_eq!(
                    write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                    Err(device_error(DeviceError::UnknownOffset)),
                    "write selector {selector:#x}"
                );
            }
            assert_eq!(snapshot(&io), before);
            assert_eq!(read(&mut bus, 0), Ok(selector));
        }
        assert_eq!(window(&mut bus, 0x18), Ok(u64::from(MASK) | 0x51));
    }

    #[test]
    fn invalid_vector_delivery_mode_and_destination_are_refused_atomically() {
        let (mut bus, io, _apic) = setup();
        for pin in [0u64, 4, 23] {
            let low = 0x10 + 2 * pin;
            window_write(&mut bus, low, 0x51).unwrap();
            select(&mut bus, low);
            let before = snapshot(&io);
            let mut invalid: Vec<u64> = vec![0, 1, 15, 0xff, 0x100 | 0x51];
            // Every non-Fixed delivery mode (001..111, reserved 011 and 110 included),
            // masked or not.
            for mode in 1..8u64 {
                invalid.push(mode << 8 | 0x51);
                invalid.push(mode << 8 | 0x51 | u64::from(MASK));
            }
            // Logical destination mode, active-low polarity, reserved bits.
            invalid.extend([0x851, 0x851 | u64::from(MASK), 0x2051, 0x20051, 0x8000_0051]);
            for value in invalid {
                assert_eq!(
                    write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                    Err(device_error(DeviceError::UnsupportedConfiguration)),
                    "pin {pin} value {value:#x}"
                );
                assert_eq!(snapshot(&io), before);
            }
            // Destination: only physical ID 0; any other or logical destination refused.
            select(&mut bus, low + 1);
            let before = snapshot(&io);
            for dest in [
                1u64 << 24,
                0xff << 24,
                1,
                0x100,
                0x0100_0000 | 0x800,
                u64::MAX,
            ] {
                assert_eq!(
                    write(&mut bus, IO_APIC_MMIO_BASE, 0x10, dest),
                    Err(device_error(DeviceError::UnsupportedConfiguration)),
                    "pin {pin} dest {dest:#x}"
                );
                assert_eq!(snapshot(&io), before);
            }
            // Logical destination: bit 11 with destination 0 is refused as well.
            assert_eq!(
                window_write(&mut bus, low, 0x800 | 0x51),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(window(&mut bus, low), Ok(0x51));
        }
        // Policy: a masked entry may hold any vector (reset state is MASK, vector 0).
        window_write(&mut bus, 0x18, u64::from(MASK)).unwrap();
        window_write(&mut bus, 0x18, u64::from(MASK) | 15).unwrap();
        assert_eq!(window(&mut bus, 0x18), Ok(u64::from(MASK) | 15));
        assert_eq!(
            window_write(&mut bus, 0x18, 15),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
    }

    /// The firmware MADT (S4-31f) declares IOAPIC ID 1 and 24 inputs; the model must answer
    /// the same through its registers. The ID is read from the committed real blob; the pin
    /// count is not a MADT field, so it is read from the firmware constant itself.
    #[test]
    fn model_agrees_with_the_firmware_madt() {
        use crate::{FirmwareBoardConfig, validate_acpi_blob};
        const BLOB: &[u8] =
            include_bytes!("../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
        const MADT_SRC: &str = include_str!("../../../firmware/uefi/src/acpi/madt.rs");
        let board = FirmwareBoardConfig {
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            ..FirmwareBoardConfig::default()
        };
        let info = validate_acpi_blob(BLOB, &board.acpi_manifest_hash().unwrap()).unwrap();
        let table = info
            .tables
            .iter()
            .find(|t| &t.signature == b"APIC")
            .unwrap();
        let madt = info.table_bytes(BLOB, table);
        let (mut at, mut ioapic) = (44, None);
        while at + 2 <= madt.len() {
            if madt[at] == 1 {
                assert_eq!(madt[at + 1], 12);
                let address = u32::from_le_bytes(madt[at + 4..at + 8].try_into().unwrap());
                assert_eq!(u64::from(address), IO_APIC_MMIO_BASE);
                assert!(ioapic.replace(madt[at + 2]).is_none(), "one I/O APIC");
            }
            assert_ne!(madt[at + 1], 0, "zero-length MADT structure");
            at += usize::from(madt[at + 1]);
        }
        let madt_id = u64::from(ioapic.expect("MADT has an I/O APIC"));
        let pins: u64 = MADT_SRC
            .lines()
            .find_map(|l| l.strip_prefix("pub const IOAPIC_PIN_COUNT: u32 = "))
            .and_then(|v| v.trim_end_matches(';').parse().ok())
            .expect("IOAPIC_PIN_COUNT constant");
        let src_id: u64 = MADT_SRC
            .lines()
            .find_map(|l| l.strip_prefix("pub const IOAPIC_ID: u8 = "))
            .and_then(|v| v.trim_end_matches(';').parse().ok())
            .expect("IOAPIC_ID constant");
        assert_eq!(madt_id, src_id);
        let (mut bus, _io, _apic) = setup();
        assert_eq!(window(&mut bus, 0).unwrap() >> 24 & 0xf, madt_id);
        assert_eq!(window(&mut bus, 2).unwrap() >> 24 & 0xf, madt_id);
        let version = window(&mut bus, 1).unwrap();
        assert_eq!((version >> 16) & 0xff, pins - 1, "MRE = MADT pin count - 1");
        assert_eq!(version & 0xff, 0x20);
        // The last MADT input has a redirection entry; the next one does not.
        assert!(window(&mut bus, 0x10 + 2 * (pins - 1)).is_ok());
        assert!(window(&mut bus, 0x10 + 2 * pins).is_err());
    }

    #[test]
    fn masked_active_low_is_stored_on_entries_without_a_line() {
        let (mut bus, _io, _apic) = setup();
        let low = u64::from(MASK | LEVEL | POLARITY_LOW);
        // Pin 9 (SCI GSI 9, level/active-low in the MADT): stored while masked.
        window_write(&mut bus, 0x10 + 2 * 9, low | 0x59).unwrap();
        assert_eq!(window(&mut bus, 0x10 + 2 * 9), Ok(low | 0x59));
        window_write(&mut bus, 0x10 + 2 * 9, u64::from(MASK | POLARITY_LOW)).unwrap();
        assert_eq!(
            window(&mut bus, 0x10 + 2 * 9),
            Ok(u64::from(MASK | POLARITY_LOW))
        );
    }

    #[test]
    fn unmasking_active_low_is_refused_without_state_change() {
        let (mut bus, io, apic) = setup();
        window_write(
            &mut bus,
            0x10 + 2 * 9,
            u64::from(MASK | LEVEL | POLARITY_LOW),
        )
        .unwrap();
        // Unmasking while active-low is refused with no state change.
        select(&mut bus, 0x10 + 2 * 9);
        let before = snapshot(&io);
        for value in [
            u64::from(LEVEL | POLARITY_LOW) | 0x59,
            u64::from(POLARITY_LOW) | 0x59,
        ] {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(snapshot(&io), before);
        }
        // Unmasked active-high on the same pin still works, and clears the polarity bit.
        window_write(&mut bus, 0x10 + 2 * 9, u64::from(LEVEL) | 0x59).unwrap();
        assert_eq!(window(&mut bus, 0x10 + 2 * 9), Ok(u64::from(LEVEL) | 0x59));
        // INTIN4 is unchanged: active-low is refused even while masked.
        select(&mut bus, 0x18);
        let before = snapshot(&io);
        assert_eq!(
            write(
                &mut bus,
                IO_APIC_MMIO_BASE,
                0x10,
                u64::from(MASK | POLARITY_LOW)
            ),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&io), before);
        assert_eq!(apic.pending(), None);
    }

    // ---- S4-29e: INTIN2 (GSI 2), edge only -----------------------------------------------

    const ENTRY2: u64 = 0x14;
    const ENTRY2_HIGH: u64 = 0x15;
    fn refused() -> Result<(), BusAccessError> {
        Err(device_error(DeviceError::UnsupportedConfiguration))
    }

    #[test]
    fn intin2_masked_edge_is_lost_and_unmask_does_not_deliver() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        // Reset state: masked, line low.
        assert_eq!(window(&mut bus, ENTRY2), Ok(u64::from(MASK)));
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), None);
        io.set_intin2(false).unwrap();
        window_write(&mut bus, ENTRY2, u64::from(MASK) | 0x52).unwrap();
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), None);
        // Unmask with the line high: the lost edge is not replayed.
        window_write(&mut bus, ENTRY2, 0x52).unwrap();
        assert_eq!(apic.pending(), None);
        // High-to-high is no edge.
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), None);
        // A new rising edge delivers.
        io.set_intin2(false).unwrap();
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), Some(0x52));
        assert_eq!(apic.acknowledge(), Some(0x52));
        // Remote IRR is never set for entry 2; INTIN4 is not involved.
        assert_eq!(window(&mut bus, ENTRY2), Ok(0x52));
        assert!(!snapshot(&io).route.remote_irr);
        assert_eq!(window(&mut bus, 0x18), Ok(u64::from(MASK)));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        // Masking again loses further edges.
        window_write(&mut bus, ENTRY2, u64::from(MASK) | 0x52).unwrap();
        io.set_intin2(false).unwrap();
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn intin2_refused_configurations_leave_the_snapshot_unchanged() {
        let (mut bus, io, _apic) = setup();
        window_write(&mut bus, ENTRY2, 0x52).unwrap();
        select(&mut bus, ENTRY2);
        let before = snapshot(&io);
        let mut invalid: Vec<u64> = vec![
            u64::from(LEVEL) | 0x52, // level unmasked
            0x0f,
            0xff,
            0,
            0x100 | 0x52,       // non-Fixed delivery
            0x800 | 0x52,       // logical destination
            0x2000 | 0x52,      // active-low unmasked
            0x2000 | 0x800 | 1, // several at once
        ];
        for mode in 1..8u64 {
            invalid.push(mode << 8 | 0x52);
        }
        for value in invalid {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                refused(),
                "value {value:#x}"
            );
            assert_eq!(snapshot(&io), before);
        }
        // Masked level (as the SCI entry) is stored; unmasking it stays refused.
        window_write(&mut bus, ENTRY2, u64::from(MASK | LEVEL) | 0x52).unwrap();
        assert_eq!(window(&mut bus, ENTRY2), Ok(u64::from(MASK | LEVEL) | 0x52));
        select(&mut bus, ENTRY2);
        let before = snapshot(&io);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x52),
            refused()
        );
        assert_eq!(snapshot(&io), before);
        // High dword: only 0 (physical APIC ID 0) is accepted, as for every entry.
        window_write(&mut bus, ENTRY2_HIGH, 0).unwrap();
        for dest in [1u64 << 24, 0xff << 24, 1, u64::MAX] {
            let before = snapshot(&io);
            assert_eq!(window_write(&mut bus, ENTRY2_HIGH, dest), refused());
            assert_eq!(snapshot(&io), before);
        }
        assert_eq!(window(&mut bus, ENTRY2_HIGH), Ok(0));
    }

    #[test]
    fn intin2_vector_boundaries_unmasked() {
        let (mut bus, _io, _apic) = setup();
        for vector in [0x10u64, 0x11, 0xfd, 0xfe] {
            window_write(&mut bus, ENTRY2, vector).unwrap();
            assert_eq!(window(&mut bus, ENTRY2), Ok(vector));
        }
        for vector in [0x0fu64, 0xff] {
            assert_eq!(window_write(&mut bus, ENTRY2, vector), refused());
            assert_eq!(window(&mut bus, ENTRY2), Ok(0xfe));
        }
    }

    #[test]
    fn intin2_conflicts_with_a_level_intin4_vector() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        window_write(&mut bus, 0x18, u64::from(LEVEL) | 0x61).unwrap();
        // Same vector as the level route: unmasking entry 2 is refused (S4-26b policy).
        select(&mut bus, ENTRY2);
        let before = snapshot(&io);
        assert_eq!(write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x61), refused());
        assert_eq!(snapshot(&io), before);
        // Masked entry 2 may hold the vector; a different vector is accepted.
        window_write(&mut bus, ENTRY2, u64::from(MASK) | 0x61).unwrap();
        window_write(&mut bus, ENTRY2, 0x62).unwrap();
        io.set_intin2(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x62));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        // Reverse direction: INTIN4 level onto entry 2's unmasked vector is refused.
        window_write(&mut bus, 0x18, u64::from(MASK)).unwrap();
        select(&mut bus, 0x18);
        let before = snapshot(&io);
        for value in [u64::from(LEVEL) | 0x62, u64::from(LEVEL | MASK) | 0x62] {
            assert_eq!(write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value), refused());
            assert_eq!(snapshot(&io), before);
        }
        // Edge INTIN4 on the same vector is not a conflict (edges coalesce).
        window_write(&mut bus, 0x18, 0x62).unwrap();
    }

    #[test]
    fn intin2_delivery_refused_without_enabled_destination_and_not_consumed() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        window_write(&mut bus, ENTRY2, 0x62).unwrap();
        window_write(&mut bus, 0x18, u64::from(MASK | LEVEL)).unwrap();
        // INTIN4 is masked/level with vector 0; unrelated, so entry 2 delivers.
        io.set_intin2(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x62));
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        io.set_intin2(false).unwrap();
        // Disabled destination: the edge is refused and not consumed.
        apic.set_global_enabled(false).unwrap();
        assert_eq!(
            io.set_intin2(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert!(!snapshot(&io).route.intin2_high);
        apic.set_global_enabled(true).unwrap();
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), Some(0x62));
    }

    #[test]
    fn intin2_reset_masks_entry_and_drops_the_line() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        window_write(&mut bus, ENTRY2, 0x52).unwrap();
        io.set_intin2(true).unwrap();
        assert!(snapshot(&io).route.intin2_high);
        apic.acknowledge();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        io.reset().unwrap();
        assert_eq!(window(&mut bus, ENTRY2), Ok(u64::from(MASK)));
        assert_eq!(window(&mut bus, ENTRY2_HIGH), Ok(0));
        let route = snapshot(&io).route;
        assert!(!route.intin2_high && !route.remote_irr);
        let expected = crate::apic_pair::Route {
            selector: route.selector,
            ..crate::apic_pair::Route::default()
        };
        assert_eq!(route, expected);
        // After reset a rising edge on the masked entry is lost.
        io.set_intin2(true).unwrap();
        assert_eq!(apic.pending(), None);
        // Pair reset does the same.
        window_write(&mut bus, ENTRY2, 0x52).unwrap();
        apic.reset_pair().unwrap();
        assert_eq!(window(&mut bus, ENTRY2), Ok(u64::from(MASK)));
        assert!(!snapshot(&io).route.intin2_high);
    }

    #[test]
    fn intin2_does_not_disturb_intin4() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        window_write(&mut bus, ENTRY2, 0x92).unwrap();
        window_write(&mut bus, 0x18, u64::from(LEVEL) | 0x71).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x71));
        assert_ne!(window(&mut bus, 0x18).unwrap() & u64::from(REMOTE_IRR), 0);
        // Entry 2 edge while INTIN4 level awaits EOI: independent vectors, no Remote IRR on 2.
        io.set_intin2(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x92));
        assert_eq!(window(&mut bus, ENTRY2).unwrap() & u64::from(REMOTE_IRR), 0);
        assert!(snapshot(&io).route.intin2_high);
        // INTIN2 line state is untouched by INTIN4 transitions and vice versa.
        io.set_intin4(false).unwrap();
        assert!(snapshot(&io).route.intin2_high && !snapshot(&io).route.high_line);
    }
}
