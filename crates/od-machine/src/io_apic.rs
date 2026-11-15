use crate::apic_pair::{LEVEL, MASK, POLARITY_LOW, PairState, REMOTE_IRR, SCI_LEVEL_LOW, SCI_PIN};
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, LocalApicHandle};
use std::sync::{Arc, Mutex};

pub const IO_APIC_MMIO_BASE: u64 = 0xfec0_0000;
pub const IO_APIC_MMIO_SIZE: u64 = 0x1000;
const READ_ONLY_STATUS: u32 = (1 << 12) | REMOTE_IRR;
/// Version 0x20 (as on ICH/QEMU-style IOAPICs), MRE (bits 23:16) = 23 (24 inputs), as in the
/// MADT. The EOI register at offset 0x40 is not modeled, nor is register 0x03 (UnknownOffset).
const IO_APIC_VERSION: u32 = 0x0017_0020;

/// IOAPIC register window; INTIN4 (edge/level), INTIN2 (edge only) and the SCI on INTIN9 (level,
/// active low, S4-38L8e) route to the vCPU whose
/// APIC ID the entry's destination field holds (physical mode; APIC ID 0 at reset).
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
    /// The SCI line on INTIN9 (GSI 9, S4-38L8e): `asserted` is the source's request, the physical
    /// level being low (MADT flags 0x000F). Level semantics: see `PairState::set_sci_line`.
    /// Caller serializes complete device operations.
    pub fn set_sci_line(&self, asserted: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_sci_line(asserted)
    }
    /// S4-38L8f: the route of INTIN2 as a question that changes nothing: `Some((cpu, vector))`
    /// when the entry is unmasked (an edge would request `vector` on the Local APIC of vCPU
    /// `cpu`), `None` when it is masked (an edge is lost).
    pub(crate) fn intin2_route(&self) -> Option<(usize, u8)> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let low = state.route.pins[2];
        (low & MASK == 0).then(|| (usize::from(state.route.dest[2]), low as u8))
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
/// active-high only. Active-low polarity (bit 13) is accepted on entries other than INTIN4
/// while masked (the MADT declares the SCI on GSI 9 as level/active-low), and unmasked only on
/// the SCI entry together with the level trigger (bit 15): exactly the entry form the MADT
/// interrupt source override describes (S4-38L8e). Every other unmasked active-low entry is
/// refused.
fn valid_low(pin: usize, value: u64) -> bool {
    let masked = value & u64::from(MASK) != 0;
    let mut allowed = u64::from(MASK | LEVEL | READ_ONLY_STATUS) | 0xff;
    let sci_form = pin == SCI_PIN && value & u64::from(SCI_LEVEL_LOW) == u64::from(SCI_LEVEL_LOW);
    if pin != 4 && (masked || sci_form) {
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
            Register::Redirection { pin, high: true } => u32::from(state.route.dest[pin]) << 24,
            Register::Redirection { pin: 4, .. } => {
                state.route.low
                    | if state.route.remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    }
            }
            Register::Redirection { pin: SCI_PIN, .. } => {
                state.route.pins[SCI_PIN]
                    | if state.route.sci_remote_irr {
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
            // Physical destination only: bits 31:24 are the APIC ID of an existing vCPU, the
            // rest is reserved. An ID no vCPU has is refused before any change.
            Register::Redirection { pin, high: true } if value & !(0xff << 24) == 0 => {
                state.set_dest(pin, (value >> 24) as u8)
            }
            Register::Redirection { pin, high: false } if valid_low(pin, value) => {
                let low = value as u32 & !READ_ONLY_STATUS;
                if pin == 4 {
                    state.set_low(low)
                } else if pin == 2 {
                    state.set_low2(low)
                } else if pin == SCI_PIN {
                    state.set_sci_low(low)
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
        setup_with(LocalApic::new())
    }
    fn setup_with(local: LocalApic) -> (MachineBus, IoApicHandle, LocalApicHandle) {
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
        let local_before = io.0.lock().unwrap().cpus[0].local.clone();
        io.reset().unwrap();
        assert_eq!(io.0.lock().unwrap().cpus[0].local, local_before);
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
    fn unmasking_other_active_low_forms_is_refused_without_state_change() {
        let (mut bus, io, apic) = setup();
        window_write(
            &mut bus,
            0x10 + 2 * 9,
            u64::from(MASK | LEVEL | POLARITY_LOW),
        )
        .unwrap();
        // S4-38L8e: the level/active-low form of the SCI is accepted unmasked (the entry the MADT
        // describes); active-low without the level trigger stays refused with no state change.
        select(&mut bus, 0x10 + 2 * 9);
        let before = snapshot(&io);
        assert_eq!(
            write(
                &mut bus,
                IO_APIC_MMIO_BASE,
                0x10,
                u64::from(POLARITY_LOW) | 0x59
            ),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&io), before);
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

    // ---- S4-38L8e: the SCI on INTIN9, level/active low ------------------------------------

    const SCI_ENTRY: u64 = 0x10 + 2 * 9;
    const SCI_VECTOR: u64 = 0x20;
    fn sci_entry(extra: u32) -> u64 {
        u64::from(SCI_LEVEL_LOW | extra) | SCI_VECTOR
    }
    fn sci_setup() -> (MachineBus, IoApicHandle, LocalApicHandle) {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        select(&mut bus, SCI_ENTRY);
        (bus, io, apic)
    }
    fn lapic_eoi(bus: &mut MachineBus) {
        write(bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
    }

    #[test]
    fn sci_level_active_low_entry_is_accepted_through_ioregsel_and_iowin() {
        let (mut bus, _io, apic) = sci_setup();
        // The write the Linux boot stops on (r9 = 0xa020 at IOREGSEL 0x22).
        assert_eq!(write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0xa020), Ok(()));
        assert_eq!(read(&mut bus, 0x10), Ok(0xa020));
        assert_eq!(sci_entry(0), 0xa020);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn sci_mask_blocks_delivery_and_unmask_with_the_line_asserted_delivers_once() {
        let (mut bus, io, apic) = sci_setup();
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(MASK)).unwrap();
        io.set_sci_line(true).unwrap();
        assert_eq!(apic.pending(), None, "masked: no delivery");
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        assert_eq!(
            apic.pending(),
            Some(0x20),
            "unmasking delivers the held line"
        );
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert_ne!(
            local_read(&mut bus, 0x190) & (1 << 0),
            0,
            "TMR bit 0x20 (level)"
        );
        // Rewriting the entry, or a second assertion, does not queue a second request.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        io.set_sci_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x20));
        assert_eq!(apic.acknowledge(), None);
        // Masking again before the EOI keeps Remote IRR until the EOI, and the EOI ends it.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(MASK)).unwrap();
        lapic_eoi(&mut bus);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn sci_level_redelivers_on_eoi_while_asserted_and_stops_when_released() {
        let (mut bus, io, apic) = sci_setup();
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        io.set_sci_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x20));
        for _ in 0..2 {
            lapic_eoi(&mut bus);
            assert_eq!(
                apic.pending(),
                Some(0x20),
                "line still asserted: redelivered"
            );
            assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
            assert_eq!(apic.acknowledge(), Some(0x20));
        }
        io.set_sci_line(false).unwrap();
        lapic_eoi(&mut bus);
        assert_eq!(apic.pending(), None, "line released: no redelivery");
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        // A new assertion after the release is a new request.
        io.set_sci_line(true).unwrap();
        assert_eq!(apic.pending(), Some(0x20));
    }

    #[test]
    fn sci_does_not_disturb_intin4_and_collisions_and_resets_are_refused() {
        let (mut bus, io, apic) = sci_setup();
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        // A level INTIN4 entry with the SCI vector collides.
        select(&mut bus, 0x18);
        let before = snapshot(&io);
        assert_eq!(
            write(
                &mut bus,
                IO_APIC_MMIO_BASE,
                0x10,
                u64::from(LEVEL) | SCI_VECTOR
            ),
            refused()
        );
        assert_eq!(snapshot(&io), before);
        select(&mut bus, ENTRY2);
        let before = snapshot(&io);
        // The SCI vector is reserved against edges (INTIN2 is an edge entry).
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, SCI_VECTOR),
            refused()
        );
        assert_eq!(snapshot(&io), before);
        // INTIN4 with another vector works while the SCI waits for its EOI.
        io.set_sci_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x20));
        window_write(&mut bus, 0x18, u64::from(LEVEL) | 0x51).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        // Remote IRR of the SCI vetoes the IOAPIC reset.
        assert_eq!(io.reset(), Err(DeviceError::UnsupportedConfiguration));
        io.set_intin4(false).unwrap();
        io.set_sci_line(false).unwrap();
        lapic_eoi(&mut bus); // EOI of 0x51 (highest in ISR)
        lapic_eoi(&mut bus); // EOI of 0x20
        assert_eq!(
            window(&mut bus, SCI_ENTRY).unwrap() & u64::from(REMOTE_IRR),
            0
        );
        assert_eq!(apic.pending(), None);
        io.reset().unwrap();
    }

    #[test]
    fn sci_remote_irr_vetoes_lapic_disable_svr_disable_and_lapic_reset() {
        let (mut bus, io, apic) = sci_setup();
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        io.set_sci_line(true).unwrap();
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        let before = snapshot(&io);
        assert_eq!(
            apic.set_global_enabled(false),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(apic.reset(), Err(DeviceError::UnsupportedConfiguration));
        assert_eq!(
            write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0xff),
            Err(BusAccessError::Device {
                device_index: 1,
                source: DeviceError::UnsupportedConfiguration
            })
        );
        assert_eq!(snapshot(&io), before);
        // After the EOI (line released first) the same operations succeed.
        assert_eq!(apic.acknowledge(), Some(0x20));
        io.set_sci_line(false).unwrap();
        lapic_eoi(&mut bus);
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0xff).unwrap();
        apic.set_global_enabled(false).unwrap();
    }

    #[test]
    fn a_masked_sci_form_reserves_its_vector_both_ways() {
        let (mut bus, io, apic) = sci_setup();
        // A masked SCI form takes the vector: a level or edge INTIN4 and entry 2 with it are refused.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(MASK)).unwrap();
        for (selector, value) in [(0x18, u64::from(LEVEL) | SCI_VECTOR), (ENTRY2, SCI_VECTOR)] {
            select(&mut bus, selector);
            let before = snapshot(&io);
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                refused(),
                "selector {selector:#x} value {value:#x}"
            );
            assert_eq!(snapshot(&io), before);
        }
        // An INTIN4 edge entry may hold the vector, but its edge is refused at delivery.
        window_write(&mut bus, 0x18, SCI_VECTOR).unwrap();
        assert_eq!(
            io.set_intin4(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(apic.pending(), None);
        window_write(&mut bus, 0x18, u64::from(MASK)).unwrap();
        // And the other way: a masked SCI form with a vector entry 2 or a level INTIN4 holds is refused.
        window_write(&mut bus, SCI_ENTRY, u64::from(MASK)).unwrap();
        window_write(&mut bus, ENTRY2, 0x52).unwrap();
        window_write(&mut bus, 0x18, u64::from(LEVEL) | 0x51).unwrap();
        for vector in [0x52u64, 0x51] {
            select(&mut bus, SCI_ENTRY);
            let before = snapshot(&io);
            assert_eq!(
                write(
                    &mut bus,
                    IO_APIC_MMIO_BASE,
                    0x10,
                    u64::from(SCI_LEVEL_LOW | MASK) | vector
                ),
                refused(),
                "vector {vector:#x}"
            );
            assert_eq!(snapshot(&io), before);
        }
    }

    #[test]
    fn other_unsupported_entry_fields_stay_refused_with_the_sci_form_accepted() {
        let (mut bus, io, _apic) = sci_setup();
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, sci_entry(0)).unwrap();
        let before = snapshot(&io);
        // Pin 9: logical destination mode, NMI/SMI/INIT/ExtINT/lowest-priority delivery modes,
        // reserved bits, the active-low edge form, a vector below 0x10.
        let mut invalid = vec![
            0x800 | sci_entry(0),
            0x8000_0000 | sci_entry(0),
            u64::from(POLARITY_LOW) | SCI_VECTOR,
            u64::from(SCI_LEVEL_LOW) | 0x05,
            u64::from(SCI_LEVEL_LOW) | 0xff,
            0x2_0000 | sci_entry(0),
        ];
        for mode in [1u64, 2, 4, 5, 7] {
            invalid.push(mode << 8 | sci_entry(0));
        }
        for value in invalid {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                refused(),
                "value {value:#x}"
            );
            assert_eq!(snapshot(&io), before);
        }
        // The same entry form on other inputs: unmasked level/active-low is refused (the MADT
        // overrides GSI 9 only), INTIN4 stays active-high only.
        for pin in [0u64, 2, 4, 8, 10, 23] {
            let selector = 0x10 + 2 * pin;
            select(&mut bus, selector);
            let before = snapshot(&io);
            for value in [sci_entry(0), u64::from(POLARITY_LOW) | 0x51] {
                assert_eq!(
                    write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                    refused(),
                    "pin {pin} value {value:#x}"
                );
                assert_eq!(snapshot(&io), before);
            }
        }
        assert_eq!(window_write(&mut bus, 0x18, sci_entry(MASK)), refused());
        // A destination no vCPU has is refused for the SCI entry as for any other.
        assert_eq!(window_write(&mut bus, SCI_ENTRY + 1, 1 << 24), refused());
        // An unmasked SCI entry of another form takes no line: typed refusal, no state change.
        window_write(&mut bus, SCI_ENTRY, u64::from(LEVEL) | 0x59).unwrap();
        let before = snapshot(&io);
        assert_eq!(
            io.set_sci_line(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(snapshot(&io), before);
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

    // S6-11: the destination field selects the vCPU whose LAPIC receives the request.

    /// Two vCPUs, both software-enabled; vcpu 1's handle is the last element.
    fn smp_setup() -> (MachineBus, IoApicHandle, LocalApicHandle, LocalApicHandle) {
        let local = LocalApic::with_cpus(2).unwrap();
        let cpu1 = local.handle_for(1).unwrap();
        let (bus, io, cpu0) = setup_with(local);
        for cpu in [&cpu0, &cpu1] {
            cpu.0.lock().unwrap().cpus[cpu.cpu_index()].local.svr = 0x1ff;
        }
        (bus, io, cpu0, cpu1)
    }
    /// High dword of entry `pin`: the APIC ID in bits 31:24.
    fn set_destination(bus: &mut MachineBus, pin: u64, apic_id: u64) -> Result<(), BusAccessError> {
        select(bus, 0x11 + 2 * pin);
        write(bus, IO_APIC_MMIO_BASE, 0x10, apic_id << 24)
    }
    fn destination(bus: &mut MachineBus, pin: u64) -> u64 {
        select(bus, 0x11 + 2 * pin);
        read(bus, 0x10).unwrap() >> 24
    }
    fn route_pin(bus: &mut MachineBus, pin: u64, low: u64) {
        select(bus, 0x10 + 2 * pin);
        write(bus, IO_APIC_MMIO_BASE, 0x10, low).unwrap();
    }

    /// The state is as it was, except for the register selector, which accesses move.
    fn assert_unchanged(io: &IoApicHandle, mut before: PairState) {
        let now = snapshot(io);
        before.route.selector = now.route.selector;
        assert_eq!(now, before);
    }

    #[test]
    fn an_edge_route_to_apic_id_1_raises_the_request_in_vcpu_1_only() {
        let (mut bus, io, cpu0, cpu1) = smp_setup();
        // Reset: both destinations read APIC ID 0.
        assert_eq!((destination(&mut bus, 4), destination(&mut bus, 2)), (0, 0));
        set_destination(&mut bus, 4, 1).unwrap();
        route_pin(&mut bus, 4, 0x41);
        assert_eq!(destination(&mut bus, 4), 1);
        io.set_intin4(true).unwrap();
        assert_eq!(cpu1.vector_state(0x41), (true, false));
        assert_eq!(cpu1.pending(), Some(0x41));
        assert_eq!(
            (cpu0.vector_state(0x41), cpu0.pending()),
            ((false, false), None)
        );
        // Back to APIC ID 0: the next edge goes to vcpu 0 and vcpu 1 keeps what it has.
        io.set_intin4(false).unwrap();
        set_destination(&mut bus, 4, 0).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(cpu0.vector_state(0x41), (true, false));
        assert_eq!(cpu1.vector_state(0x41), (true, false));
        // Taking the request on vcpu 1 does not take it from vcpu 0.
        assert_eq!(cpu1.acknowledge(), Some(0x41));
        assert_eq!(cpu0.vector_state(0x41), (true, false));
    }

    #[test]
    fn intin2_follows_its_own_destination_field() {
        let (mut bus, io, cpu0, cpu1) = smp_setup();
        set_destination(&mut bus, 2, 1).unwrap();
        route_pin(&mut bus, 2, 0x92);
        // INTIN4 still goes to vcpu 0: the destinations are per entry.
        assert_eq!(destination(&mut bus, 4), 0);
        io.set_intin2(true).unwrap();
        assert_eq!(cpu1.vector_state(0x92), (true, false));
        assert_eq!(cpu0.vector_state(0x92), (false, false));
    }

    #[test]
    fn a_level_route_to_apic_id_1_sets_remote_irr_until_that_vcpu_acknowledges_it() {
        let (mut bus, io, cpu0, cpu1) = smp_setup();
        set_destination(&mut bus, 4, 1).unwrap();
        route_pin(&mut bus, 4, u64::from(LEVEL) | 0x51);
        io.set_intin4(true).unwrap();
        assert_eq!(cpu1.vector_state(0x51), (true, false));
        assert_eq!(cpu0.vector_state(0x51), (false, false));
        assert_ne!(
            window(&mut bus, 0x18).unwrap() & u64::from(REMOTE_IRR),
            0,
            "Remote IRR"
        );
        // An EOI from the other LAPIC does not end the request.
        cpu0.0.lock().unwrap().eoi(0).unwrap();
        assert!(snapshot(&io).route.remote_irr);
        assert_eq!(cpu1.acknowledge(), Some(0x51));
        // Moving the destination while the request is outstanding is refused, and refused
        // atomically.
        let before = snapshot(&io);
        assert_eq!(
            set_destination(&mut bus, 4, 0),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_unchanged(&io, before);
        // With the line low again, the EOI of the destination LAPIC ends the request.
        io.set_intin4(false).unwrap();
        cpu1.0.lock().unwrap().eoi(1).unwrap();
        assert!(!snapshot(&io).route.remote_irr);
        assert_eq!(cpu1.vector_state(0x51), (false, false));
    }

    #[test]
    fn a_destination_no_vcpu_has_is_refused_before_any_change() {
        let (mut bus, io, _cpu0, cpu1) = smp_setup();
        set_destination(&mut bus, 4, 1).unwrap();
        route_pin(&mut bus, 4, 0x41);
        let before = snapshot(&io);
        for apic_id in [2u64, 3, 0xff] {
            assert_eq!(
                set_destination(&mut bus, 4, apic_id),
                Err(device_error(DeviceError::UnsupportedConfiguration)),
                "APIC ID {apic_id}"
            );
            // Entries without an input line refuse it too: no entry may name a missing vCPU.
            assert_eq!(
                set_destination(&mut bus, 9, apic_id),
                Err(device_error(DeviceError::UnsupportedConfiguration)),
                "APIC ID {apic_id}"
            );
        }
        assert_unchanged(&io, before);
        assert_eq!(destination(&mut bus, 4), 1);
        // The route still delivers to the vcpu it had.
        io.set_intin4(true).unwrap();
        assert_eq!(cpu1.vector_state(0x41), (true, false));
        // Reserved bits of the high dword are refused as before.
        select(&mut bus, 0x19);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, (1 << 24) | 1),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
    }

    #[test]
    fn one_vcpu_accepts_no_destination_but_apic_id_0() {
        let (mut bus, _io, _apic) = setup();
        assert!(set_destination(&mut bus, 4, 0).is_ok());
        assert_eq!(
            set_destination(&mut bus, 4, 1),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(destination(&mut bus, 4), 0);
    }

    #[test]
    fn logical_destination_and_lowest_priority_stay_refused_with_several_vcpus() {
        let (mut bus, io, _cpu0, _cpu1) = smp_setup();
        let before = snapshot(&io);
        // Destination mode (bit 11) logical, delivery mode lowest priority (001), NMI (100).
        for low in [0x41 | (1 << 11), 0x41 | (1 << 8), 0x41 | (4 << 8)] {
            select(&mut bus, 0x18);
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, low),
                Err(device_error(DeviceError::UnsupportedConfiguration)),
                "{low:#x}"
            );
        }
        assert_unchanged(&io, before);
    }

    #[test]
    fn a_level_vector_collides_with_a_timer_only_in_the_lapic_that_receives_it() {
        let clock = crate::VirtualClockHandle::new(1_000_000_000).unwrap();
        let local = LocalApic::with_shared_timers(clock, 1_000_000_000, 2).unwrap();
        let mut timer_owner = local.clone();
        let (mut bus, _io, cpu0, cpu1) = {
            let cpu1 = local.handle_for(1).unwrap();
            let (bus, io, cpu0) = setup_with(local);
            (bus, io, cpu0, cpu1)
        };
        for cpu in [&cpu0, &cpu1] {
            cpu.0.lock().unwrap().cpus[cpu.cpu_index()].local.svr = 0x1ff;
        }
        // vcpu 0's LVT Timer uses 0x51, unmasked.
        timer_owner
            .write_from_cpu(
                0,
                DeviceAccess {
                    space: IoSpace::Mmio,
                    offset: 0x320,
                    width: AccessWidth::Dword,
                },
                0x51,
            )
            .unwrap();
        // A level route to vcpu 0 may not reuse the vector; one to vcpu 1 may.
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        set_destination(&mut bus, 4, 1).unwrap();
        route_pin(&mut bus, 4, u64::from(LEVEL) | 0x51);
        // And the route cannot be moved back onto the colliding LAPIC.
        assert_eq!(
            set_destination(&mut bus, 4, 0),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(destination(&mut bus, 4), 1);
    }
}
