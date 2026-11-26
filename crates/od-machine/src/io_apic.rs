use crate::apic_pair::{
    COM2_PIN, DELIVERY_LOWEST_PRIORITY, DEST_LOGICAL, LEVEL, MASK, NIC_PIN, NVME_PIN, POLARITY_LOW,
    PairState, REMOTE_IRR, SCI_LEVEL_LOW, SCI_PIN, XHCI_PIN,
};
use crate::lock_wait_metrics::MeasuredMutex;
use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, LocalApicHandle};
use std::sync::Arc;

pub const IO_APIC_MMIO_BASE: u64 = 0xfec0_0000;
pub const IO_APIC_MMIO_SIZE: u64 = 0x1000;
const READ_ONLY_STATUS: u32 = (1 << 12) | REMOTE_IRR;
/// Version 0x20 (as on ICH/QEMU-style IOAPICs), MRE (bits 23:16) = 23 (24 inputs), as in the
/// MADT. The EOI register at offset 0x40 is not modeled, nor is register 0x03 (UnknownOffset).
const IO_APIC_VERSION: u32 = 0x0017_0020;

/// IOAPIC register window; INTIN4 and INTIN3 (edge/level), INTIN2 (edge only) and the SCI on INTIN9 (level,
/// active low, S4-38L8e) route to the vCPU whose
/// APIC ID the entry's destination field holds (physical mode; APIC ID 0 at reset).
pub struct IoApic(Arc<MeasuredMutex<PairState>>);
#[derive(Clone)]
pub struct IoApicHandle(Arc<MeasuredMutex<PairState>>);

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
    /// Connects the COM2 UART line to INTIN3 (the board has the agent channel, S11-21): from then
    /// on INTIN3 follows the INTIN4 rules. A board without the channel never calls this and INTIN3
    /// stays an input without a line. Idempotent.
    pub fn connect_com2_line(&self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .connect_com2_line();
    }
    /// The COM2 UART line on INTIN3 (GSI 3, S11-21): the INTIN4 rules; refused while the line is
    /// not connected. Caller serializes complete device operations.
    pub fn set_intin3(&self, high: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_line3(high)
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
    pub(crate) fn set_nvme_line(&self, asserted: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_nvme_line(asserted)
    }

    /// The xHCI INTA line on INTIN17 (GSI 17): level, active low.
    pub(crate) fn set_xhci_line(&self, asserted: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_xhci_line(asserted)
    }

    /// The NIC INTA line on INTIN18 (GSI 18): level, active low.
    pub(crate) fn set_nic_line(&self, asserted: bool) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_nic_line(asserted)
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
/// active-high only, and so is INTIN3 once the COM2 line is connected (`com2_wired`, the
/// `agent_channel` board option; without the line INTIN3 is an input like any other). Active-low
/// polarity (bit 13) is accepted on entries other than INTIN4 (and connected INTIN3) while masked
/// (the MADT declares the SCI on GSI 9 as level/active-low), and unmasked only on the SCI entry
/// together with the level trigger (bit 15): exactly the entry form the MADT interrupt source
/// override describes (S4-38L8e). Every other unmasked active-low entry is refused.
///
/// S7-32: the logical destination mode (bit 11) and the lowest-priority delivery mode (bits 10:8
/// = 001) are accepted on every entry (the Windows HAL programs them); with one target they
/// deliver like the fixed physical form. SMI, NMI, INIT and ExtINT stay refused.
fn valid_low(pin: usize, value: u64, com2_wired: bool) -> bool {
    let masked = value & u64::from(MASK) != 0;
    let mut allowed =
        u64::from(MASK | LEVEL | READ_ONLY_STATUS | DEST_LOGICAL | DELIVERY_LOWEST_PRIORITY) | 0xff;
    let level_low_form = matches!(pin, SCI_PIN | NVME_PIN | XHCI_PIN | NIC_PIN)
        && value & u64::from(SCI_LEVEL_LOW) == u64::from(SCI_LEVEL_LOW);
    if pin != 4 && !(pin == COM2_PIN && com2_wired) && (masked || level_low_form) {
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
            Register::Redirection { pin, high: true } => u32::from(state.route.dest_raw[pin]) << 24,
            Register::Redirection { pin: 4, .. } => {
                state.route.low
                    | if state.route.remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    }
            }
            Register::Redirection { pin: COM2_PIN, .. } => {
                state.route.pins[COM2_PIN]
                    | if state.route.com2_remote_irr {
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
            Register::Redirection { pin: NVME_PIN, .. } => {
                state.route.pins[NVME_PIN]
                    | if state.route.nvme_remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    }
            }
            Register::Redirection { pin: XHCI_PIN, .. } => {
                state.route.pins[XHCI_PIN]
                    | if state.route.xhci_remote_irr {
                        REMOTE_IRR
                    } else {
                        0
                    }
            }
            Register::Redirection { pin: NIC_PIN, .. } => {
                state.route.pins[NIC_PIN]
                    | if state.route.nic_remote_irr {
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
            // Bits 31:24 are the destination, the rest is reserved. Physical: the APIC ID of an
            // existing vCPU, another one is refused before any change; logical (S7-32): a message
            // destination address resolved through the LDR/DFR of the vCPUs.
            // S7-32: both redirection writes resolve the destination and then apply the entry;
            // a refusal in either step restores the whole state, so a refused write changes
            // nothing (the route, the LAPIC request state).
            Register::Redirection { pin, high: true } if value & !(0xff << 24) == 0 => {
                let saved = state.clone();
                let result = state.set_dest_raw(pin, (value >> 24) as u8);
                if result.is_err() {
                    *state = saved;
                }
                result
            }
            Register::Redirection { pin, high: false }
                if valid_low(pin, value, state.com2_wired) =>
            {
                let low = value as u32 & !READ_ONLY_STATUS;
                let saved = state.clone();
                // S7-32: the destination field is read under the destination mode of this write.
                let raw = state.route.dest_raw[pin];
                let result = state.resolve_entry(pin, raw, low).and_then(|()| {
                    if pin == 4 {
                        state.set_low(low)
                    } else if pin == 2 {
                        state.set_low2(low)
                    } else if pin == SCI_PIN {
                        state.set_sci_low(low)
                    } else if (pin == COM2_PIN && state.com2_wired)
                        || pin == NVME_PIN
                        || pin == XHCI_PIN
                        || pin == NIC_PIN
                    {
                        state.set_level_low(pin, low)
                    } else {
                        state.route.pins[pin] = low;
                        Ok(())
                    }
                });
                if result.is_err() {
                    *state = saved;
                }
                result
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
    use crate::apic_pair::{DELIVERY_LOWEST_PRIORITY, DEST_LOGICAL, logical_match};
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
        // 0x151 (lowest priority) is accepted since S7-32; 0x851 (logical, unmasked) still
        // names no vCPU here (LDR 0).
        for invalid in [0, 15, 0xff, 0x851, 0x2051, 0x20051] {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, invalid),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
            assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK) | 0x51));
        }
        select(&mut bus, 0x19);
        assert_eq!(read(&mut bus, 0x10), Ok(0));
        // S7-32: the masked entry stores the destination 1 (no vCPU has it) and reads it back;
        // unmasking it is refused and leaves the entry masked; destination 0 again.
        assert_eq!(write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 1 << 24), Ok(()));
        assert_eq!(read(&mut bus, 0x10), Ok(1 << 24));
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(read(&mut bus, 0x10), Ok(u64::from(MASK) | 0x51));
        select(&mut bus, 0x19);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0).unwrap();
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
            // 0x100 | 0x51 (lowest priority) is accepted since S7-32.
            let mut invalid: Vec<u64> = vec![0, 1, 15, 0xff];
            // Every delivery mode other than fixed and lowest priority (S7-32: 001 accepted;
            // 010..111, reserved 011 and 110 included), masked or not.
            for mode in 2..8u64 {
                invalid.push(mode << 8 | 0x51);
                invalid.push(mode << 8 | 0x51 | u64::from(MASK));
            }
            // Logical destination mode naming no vCPU while unmasked (S7-32: a masked one is
            // stored), active-low polarity, reserved bits.
            invalid.extend([0x851, 0x2051, 0x20051, 0x8000_0051]);
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
    fn smp_ioapic_id_is_outside_the_cpu_id_range_after_reset() {
        for count in [2, 4] {
            let (mut bus, io, _) = setup_with(LocalApic::with_cpus(count).unwrap());
            let id = (count as u64) << 24;
            assert_eq!(window(&mut bus, 0).unwrap(), id);
            assert_eq!(window(&mut bus, 2).unwrap(), id);
            window_write(&mut bus, 0, 7 << 24).unwrap();
            io.reset().unwrap();
            assert_eq!(window(&mut bus, 0).unwrap(), id);
        }
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

    // ---- S8-42: the xHCI INTA on INTIN17 (GSI 17), level/active low like the NVMe route -------

    const XHCI_ENTRY: u64 = 0x10 + 2 * 17;
    const NVME_ENTRY: u64 = 0x10 + 2 * 16;

    #[test]
    fn xhci_gsi_17_is_a_level_active_low_input_with_remote_irr_and_eoi_redelivery() {
        let (mut bus, io, apic) = sci_setup();
        select(&mut bus, XHCI_ENTRY);
        let entry = u64::from(SCI_LEVEL_LOW) | 0x31;
        // The entry form of a PCI INTA route is accepted and read back.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, entry).unwrap();
        assert_eq!(read(&mut bus, 0x10), Ok(entry));
        // A masked entry only records the line; unmasking delivers it once.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, entry | u64::from(MASK)).unwrap();
        io.set_xhci_line(true).unwrap();
        assert_eq!(apic.pending(), None);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, entry).unwrap();
        assert_eq!(apic.pending(), Some(0x31));
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert_eq!(apic.acknowledge(), Some(0x31));
        // Still asserted: the EOI redelivers; released: it does not.
        lapic_eoi(&mut bus);
        assert_eq!(apic.pending(), Some(0x31));
        assert_eq!(apic.acknowledge(), Some(0x31));
        io.set_xhci_line(false).unwrap();
        lapic_eoi(&mut bus);
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
    }

    #[test]
    fn xhci_and_nvme_lines_are_independent_inputs() {
        let (mut bus, io, apic) = sci_setup();
        select(&mut bus, XHCI_ENTRY);
        write(
            &mut bus,
            IO_APIC_MMIO_BASE,
            0x10,
            u64::from(SCI_LEVEL_LOW) | 0x41,
        )
        .unwrap();
        select(&mut bus, NVME_ENTRY);
        write(
            &mut bus,
            IO_APIC_MMIO_BASE,
            0x10,
            u64::from(SCI_LEVEL_LOW) | 0x30,
        )
        .unwrap();
        io.set_nvme_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x30));
        io.set_xhci_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x41));
        // Remote IRR of each entry is its own.
        select(&mut bus, XHCI_ENTRY);
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        select(&mut bus, NVME_ENTRY);
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        io.set_nvme_line(false).unwrap();
        io.set_xhci_line(false).unwrap();
        lapic_eoi(&mut bus);
        lapic_eoi(&mut bus);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn xhci_pin_refuses_an_unmasked_edge_entry_with_the_line_asserted() {
        let (mut bus, io, _apic) = sci_setup();
        select(&mut bus, XHCI_ENTRY);
        // Unmasked, edge, active high: not the form of the route.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x41).unwrap();
        assert_eq!(
            io.set_xhci_line(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }

    const NIC_ENTRY: u64 = 0x10 + 2 * 18;

    #[test]
    fn nic_gsi_18_is_a_level_active_low_input_independent_of_the_other_pci_lines() {
        let (mut bus, io, apic) = sci_setup();
        let entry = u64::from(SCI_LEVEL_LOW);
        select(&mut bus, XHCI_ENTRY);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, entry | 0x41).unwrap();
        select(&mut bus, NIC_ENTRY);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, entry | 0x51).unwrap();
        assert_eq!(read(&mut bus, 0x10), Ok(entry | 0x51));
        io.set_nic_line(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        // The xHCI entry is untouched.
        select(&mut bus, XHCI_ENTRY);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        // Still asserted: the EOI redelivers; released: it does not.
        lapic_eoi(&mut bus);
        assert_eq!(apic.acknowledge(), Some(0x51));
        io.set_nic_line(false).unwrap();
        lapic_eoi(&mut bus);
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn nic_pin_refuses_an_unmasked_edge_entry_with_the_line_asserted() {
        let (mut bus, io, _apic) = sci_setup();
        select(&mut bus, NIC_ENTRY);
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x51).unwrap();
        assert_eq!(
            io.set_nic_line(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
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
        // Lowest priority (001) is accepted since S7-32.
        for mode in [2u64, 4, 5, 7] {
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
            0x800 | 0x52,       // logical destination naming no vCPU (LDR 0), unmasked
            0x2000 | 0x52,      // active-low unmasked
            0x2000 | 0x800 | 1, // several at once
        ];
        // Lowest priority (001) is accepted since S7-32.
        for mode in 2..8u64 {
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
        // High dword: reserved bits 23:0 and a wide value are refused. S7-32: the masked entry
        // stores a destination no vCPU has (1, 0xFF) and unmasking it is refused; see the S7-32
        // tests.
        window_write(&mut bus, ENTRY2_HIGH, 0).unwrap();
        for dest in [1u64, u64::MAX] {
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
        }
        assert_unchanged(&io, before);
        // S7-32: a masked entry (pin 9) stores such a destination, delivers to no missing vCPU
        // (its route keeps vCPU 0) and refuses to be unmasked with it.
        for apic_id in [2u64, 3, 0xff] {
            set_destination(&mut bus, 9, apic_id).unwrap();
            assert_eq!(destination(&mut bus, 9), apic_id);
            assert_eq!(io.0.lock().unwrap().route.dest[9], 0);
            select(&mut bus, 0x10 + 2 * 9);
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x49),
                Err(device_error(DeviceError::UnsupportedConfiguration)),
                "APIC ID {apic_id}"
            );
        }
        // Control: with destination 0 the same unmasking write is accepted.
        set_destination(&mut bus, 9, 0).unwrap();
        route_pin(&mut bus, 9, 0x49);
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
        // S7-32: the masked entry stores 1 (read back) and refuses to be unmasked with it.
        assert!(set_destination(&mut bus, 4, 1).is_ok());
        assert_eq!(destination(&mut bus, 4), 1);
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x51),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        // Unmasked with destination 0, writing 1 is refused and changes nothing.
        set_destination(&mut bus, 4, 0).unwrap();
        route_pin(&mut bus, 4, 0x51);
        assert_eq!(
            set_destination(&mut bus, 4, 1),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(destination(&mut bus, 4), 0);
    }

    #[test]
    fn a_logical_destination_naming_no_vcpu_and_nmi_stay_refused_with_several_vcpus() {
        let (mut bus, io, _cpu0, _cpu1) = smp_setup();
        let before = snapshot(&io);
        // Destination mode (bit 11) logical with no LDR programmed, NMI (100). Lowest priority
        // (001) is accepted since S7-32 (see the S7-32 tests below).
        for low in [0x41 | (1 << 11), 0x41 | (4 << 8)] {
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

    // S6-24 (the decision recorded for the broadcast destination `0xFF`).

    #[test]
    fn the_broadcast_destination_0xff_is_refused_on_every_entry_and_changes_nothing() {
        // Version 1 routes only physical APIC IDs `0..N-1` (SMP_CONTRACT.md 6); the broadcast ID
        // `0xFF` is not among them whatever the number of vCPUs, so the write is a device error
        // (the same rule as the ICR: not listed, an error), not a delivery to all.
        let (mut bus, io, _cpu0, _cpu1) = smp_setup();
        set_destination(&mut bus, 4, 1).unwrap();
        route_pin(&mut bus, 4, 0x41);
        let before = snapshot(&io);
        // The unmasked entry (INTIN4) refuses them before any change.
        for apic_id in [0xffu64, 2, 3] {
            assert_eq!(
                set_destination(&mut bus, 4, apic_id),
                refused(),
                "pin 4 id {apic_id:#x}"
            );
        }
        assert_unchanged(&io, before);
        assert_eq!(destination(&mut bus, 4), 1);
        // S7-32: masked entries store them (a later unmasking low write resolves them), but no
        // route ever names a missing vCPU and unmasking with them is refused: `0xFF` is still no
        // delivery to all.
        for pin in [2u64, 9, 23] {
            for apic_id in [0xffu64, 2, 3] {
                set_destination(&mut bus, pin, apic_id).unwrap();
                assert!(usize::from(io.0.lock().unwrap().route.dest[pin as usize]) < 2);
                select(&mut bus, 0x10 + 2 * pin);
                assert_eq!(
                    write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x48),
                    refused(),
                    "pin {pin} id {apic_id:#x}"
                );
            }
        }
        // Control: with destination 1 the same unmasking writes are accepted.
        for pin in [2u64, 9, 23] {
            set_destination(&mut bus, pin, 1).unwrap();
            route_pin(&mut bus, pin, 0x48 + pin);
            assert_eq!(io.0.lock().unwrap().route.dest[pin as usize], 1);
        }
        // The same on a board with one vCPU: only ID 0 exists, `0xFF` is no exception.
        let (mut one, io_one, _apic) = setup();
        route_pin(&mut one, 4, 0x51);
        let before = snapshot(&io_one);
        assert_eq!(set_destination(&mut one, 4, 0xff), refused());
        assert_unchanged(&io_one, before);
    }

    fn route3(bus: &mut MachineBus, value: u64) {
        select(bus, 0x16);
        write(bus, IO_APIC_MMIO_BASE, 0x10, value).unwrap();
    }

    /// S11-21: INTIN3 carries the COM2 UART line. An unmasked edge entry requests its vector on
    /// each rising level; INTIN4 (COM1) keeps its own entry and line.
    #[test]
    fn intin3_edge_entry_requests_its_vector_and_leaves_intin4_alone() {
        let (mut bus, io, apic) = setup();
        io.connect_com2_line();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route(&mut bus, 0x51);
        // A masked entry takes no edge; unmasking never replays it.
        io.set_intin3(true).unwrap();
        assert_eq!(apic.pending(), None);
        io.set_intin3(false).unwrap();
        route3(&mut bus, 0x61);
        io.set_intin3(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x61));
        io.set_intin3(true).unwrap();
        assert_eq!(apic.pending(), None);
        io.set_intin3(false).unwrap();
        select(&mut bus, 0x16);
        assert_eq!(read(&mut bus, 0x10), Ok(0x61));
        // INTIN4 still requests its own vector and does not touch INTIN3.
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x51));
        assert!(!io.0.lock().unwrap().route.com2_asserted);
    }

    #[test]
    fn intin3_level_entry_waits_for_eoi_and_redelivers_while_the_line_is_high() {
        let (mut bus, io, apic) = setup();
        io.connect_com2_line();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        route3(&mut bus, u64::from(LEVEL) | 0x61);
        io.set_intin3(true).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x61));
        select(&mut bus, 0x16);
        assert_ne!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        assert!(!io.0.lock().unwrap().route.remote_irr);
        // The EOI redelivers while the UART line stays high, then stops once it falls.
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x61));
        io.set_intin3(false).unwrap();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xb0, 0).unwrap();
        assert_eq!(apic.pending(), None);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
    }

    #[test]
    fn intin3_is_active_high_and_its_level_vector_is_reserved_against_intin4() {
        let (mut bus, io, _apic) = setup();
        io.connect_com2_line();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        select(&mut bus, 0x16);
        for value in [
            u64::from(POLARITY_LOW) | 0x61,
            u64::from(MASK | POLARITY_LOW) | 0x61,
        ] {
            assert_eq!(
                write(&mut bus, IO_APIC_MMIO_BASE, 0x10, value),
                Err(device_error(DeviceError::UnsupportedConfiguration))
            );
        }
        route3(&mut bus, u64::from(LEVEL) | 0x61);
        // INTIN4 may not take the vector a level INTIN3 route reserves.
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x61),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        route(&mut bus, u64::from(LEVEL) | 0x62);
    }

    /// S11-21: without the COM2 line (the `agent_channel` option off, the default) INTIN3 is an
    /// input without a line, as on a board that never had the channel: a level-form entry
    /// reserves no vector against INTIN4 or edges, a masked active-low entry is accepted, no line
    /// request is taken and nothing is ever delivered. With the line connected the same writes
    /// follow the INTIN4 rules.
    #[test]
    fn intin3_has_no_line_semantics_unless_the_com2_line_is_connected() {
        let (mut bus, io, apic) = setup();
        write(&mut bus, LOCAL_APIC_MMIO_BASE, 0xf0, 0x1ff).unwrap();
        // A level-form entry reserves nothing: INTIN4 takes the same vector.
        route3(&mut bus, u64::from(LEVEL) | 0x61);
        route(&mut bus, u64::from(LEVEL) | 0x61);
        route(&mut bus, u64::from(LEVEL | MASK) | 0x62);
        // A masked active-low entry is accepted (the generic input rule) and reads back.
        route3(&mut bus, u64::from(MASK | POLARITY_LOW) | 0x61);
        select(&mut bus, 0x16);
        assert_eq!(
            read(&mut bus, 0x10),
            Ok(u64::from(MASK | POLARITY_LOW) | 0x61)
        );
        // No line: the request is refused and nothing is delivered.
        route3(&mut bus, 0x71);
        assert_eq!(
            io.set_intin3(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(apic.pending(), None);
        assert!(!io.0.lock().unwrap().route.com2_asserted);
        // A level-form entry never takes a Remote IRR.
        route3(&mut bus, u64::from(LEVEL) | 0x71);
        select(&mut bus, 0x16);
        assert_eq!(read(&mut bus, 0x10).unwrap() & u64::from(REMOTE_IRR), 0);
        // With the line connected, the level form reserves its vector and polarity is refused.
        io.connect_com2_line();
        select(&mut bus, 0x18);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x71),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        select(&mut bus, 0x16);
        assert_eq!(
            write(
                &mut bus,
                IO_APIC_MMIO_BASE,
                0x10,
                u64::from(MASK | POLARITY_LOW) | 0x61
            ),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
    }

    /// S11-21: whether the COM2 line is connected belongs to the board; restoring a snapshot keeps
    /// the board's own state in both directions.
    #[test]
    fn restoring_a_snapshot_keeps_the_boards_own_com2_line_connection() {
        let (_bus, io, apic) = setup();
        let without_channel = apic.snapshot_pair();
        assert!(!without_channel.com2_wired);
        io.connect_com2_line();
        let with_channel = apic.snapshot_pair();
        assert!(with_channel.com2_wired);
        // A snapshot without the channel restored onto a board with it: the line stays connected.
        apic.restore_pair(&without_channel).unwrap();
        assert!(io.0.lock().unwrap().com2_wired);
        io.set_intin3(true).unwrap();
        io.set_intin3(false).unwrap();
        // A snapshot with the channel restored onto a board without it: INTIN3 stays a plain input.
        let (_bus2, io2, apic2) = setup();
        apic2.restore_pair(&with_channel).unwrap();
        assert!(!io2.0.lock().unwrap().com2_wired);
        assert_eq!(
            io2.set_intin3(true),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }

    /// S7-32 (SDM Vol. 3A 11.6.2.2, 11.6.2.4 and the 82093AA redirection entry): an entry with the
    /// logical destination mode and the lowest-priority delivery mode, as the Windows HAL programs
    /// it, delivers to the vCPU whose LDR the destination names under the flat model; the raw
    /// destination reads back. Unmasked, a destination that names no vCPU is refused before any
    /// change; masked, it is stored.
    #[test]
    fn s7_32_a_logical_lowest_priority_entry_delivers_to_the_vcpu_its_ldr_names() {
        let (mut bus, io, apic) = setup();
        {
            let mut pair = apic.0.lock().unwrap();
            pair.cpus[0].local.svr = 0x1ff;
            pair.cpus[0].local.ldr = 0x01 << 24; // flat model (DFR reset 0xFFFF_FFFF), bit 0
        }
        const ENTRY2: u64 = 0x14;
        let logical_lowest = u64::from(DEST_LOGICAL | DELIVERY_LOWEST_PRIORITY);
        // Masked first, then the destination 0x01, then unmask.
        window_write(&mut bus, ENTRY2, u64::from(MASK) | logical_lowest | 0x52).unwrap();
        window_write(&mut bus, ENTRY2 + 1, 0x01 << 24).unwrap();
        assert_eq!(window(&mut bus, ENTRY2 + 1), Ok(0x01 << 24));
        window_write(&mut bus, ENTRY2, logical_lowest | 0x52).unwrap();
        assert_eq!(window(&mut bus, ENTRY2), Ok(logical_lowest | 0x52));
        io.0.lock().unwrap().set_line2(true).unwrap();
        assert_eq!(apic.pending(), Some(0x52));
        // A destination that names no vCPU (bit 1; LDR has bit 0 only): refused unmasked...
        select(&mut bus, ENTRY2 + 1);
        let before = snapshot(&io);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x02 << 24),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&io), before);
        // ...stored masked (and read back as written).
        window_write(&mut bus, ENTRY2, u64::from(MASK) | logical_lowest | 0x52).unwrap();
        window_write(&mut bus, ENTRY2 + 1, 0x02 << 24).unwrap();
        assert_eq!(window(&mut bus, ENTRY2 + 1), Ok(0x02 << 24));
        // Unmasking it with that destination is refused.
        assert_eq!(
            window_write(&mut bus, ENTRY2, logical_lowest | 0x52),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        // The physical lowest-priority form delivers to the physical ID.
        window_write(&mut bus, ENTRY2 + 1, 0).unwrap();
        window_write(&mut bus, ENTRY2, u64::from(DELIVERY_LOWEST_PRIORITY) | 0x53).unwrap();
        io.0.lock().unwrap().set_line2(false).unwrap();
        io.0.lock().unwrap().set_line2(true).unwrap();
        assert_eq!(apic.vector_state(0x53).0, true);
    }

    /// S7-32: the HAL order "destination first, then the low dword with the logical mode": while
    /// the entry is masked (its reset state) any destination is stored and nothing is resolved,
    /// so 0x01 and the flat broadcast 0xFF are not read as physical IDs; the unmasking low write
    /// resolves them under its own mode. An unmasked logical low written first, while the stored
    /// destination names no vCPU, is refused and changes nothing.
    #[test]
    fn s7_32_a_destination_written_before_the_logical_low_dword_is_resolved_at_unmasking() {
        let (mut bus, io, apic) = setup();
        {
            let mut pair = apic.0.lock().unwrap();
            pair.cpus[0].local.svr = 0x1ff;
            pair.cpus[0].local.ldr = 0x01 << 24;
        }
        const ENTRY4: u64 = 0x18;
        let logical = u64::from(DEST_LOGICAL | DELIVERY_LOWEST_PRIORITY);
        // Unmasked logical low first, stored destination 0: names no vCPU, refused.
        select(&mut bus, ENTRY4);
        let before = snapshot(&io);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, logical | 0x61),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&io), before);
        for destination in [0x01u64, 0xff] {
            window_write(&mut bus, ENTRY4, u64::from(MASK)).unwrap();
            window_write(&mut bus, ENTRY4 + 1, destination << 24).unwrap();
            assert_eq!(window(&mut bus, ENTRY4 + 1), Ok(destination << 24));
            window_write(&mut bus, ENTRY4, logical | 0x61).unwrap();
            assert_eq!(window(&mut bus, ENTRY4), Ok(logical | 0x61));
        }
    }

    /// S7-32: a low write whose destination move succeeds but whose entry is then refused (an
    /// unmasked level entry on INTIN2) restores the whole state: the route keeps its vCPU.
    #[test]
    fn s7_32_a_refused_low_write_leaves_the_destination_and_the_entry_unchanged() {
        let (mut bus, io, _cpu0, _cpu1) = smp_setup();
        const ENTRY2: u64 = 0x14;
        window_write(&mut bus, ENTRY2, u64::from(MASK) | 0x52).unwrap();
        // Physical ID 1 stored while masked; the entry still delivers to vCPU 0.
        window_write(&mut bus, ENTRY2 + 1, 1 << 24).unwrap();
        assert_eq!(io.0.lock().unwrap().route.dest[2], 0);
        select(&mut bus, ENTRY2);
        let before = snapshot(&io);
        assert_eq!(
            write(&mut bus, IO_APIC_MMIO_BASE, 0x10, u64::from(LEVEL) | 0x52),
            Err(device_error(DeviceError::UnsupportedConfiguration))
        );
        assert_eq!(snapshot(&io), before);
        // The edge form unmasks and moves the entry to vCPU 1.
        write(&mut bus, IO_APIC_MMIO_BASE, 0x10, 0x52).unwrap();
        assert_eq!(io.0.lock().unwrap().route.dest[2], 1);
    }

    /// S7-32: the cluster model (DFR[31:28] = 0000b) matches the same cluster (bits 7:4) and a
    /// member bit in common (bits 3:0) (SDM Vol. 3A 11.6.2.2); a logical entry reaches vCPU 1.
    #[test]
    fn s7_32_the_cluster_model_names_the_vcpu_of_the_cluster_and_member() {
        let (mut bus, io, cpu0, cpu1) = smp_setup();
        {
            let mut pair = cpu0.0.lock().unwrap();
            for (cpu, ldr) in [(0usize, 0x11u32), (1, 0x12)] {
                pair.cpus[cpu].local.dfr = 0x0fff_ffff;
                pair.cpus[cpu].local.ldr = ldr << 24;
            }
        }
        const ENTRY2: u64 = 0x14;
        window_write(&mut bus, ENTRY2, u64::from(MASK | DEST_LOGICAL) | 0x54).unwrap();
        window_write(&mut bus, ENTRY2 + 1, 0x12 << 24).unwrap();
        window_write(&mut bus, ENTRY2, u64::from(DEST_LOGICAL) | 0x54).unwrap();
        io.0.lock().unwrap().set_line2(true).unwrap();
        assert_eq!(cpu1.vector_state(0x54).0, true);
        assert_eq!(cpu0.vector_state(0x54).0, false);
        // Another cluster with the same member bit names no vCPU.
        assert!(!logical_match(0x0fff_ffff, 0x11 << 24, 0x21));
        assert!(logical_match(0xffff_ffff, 0x01 << 24, 0xff));
        assert!(
            !logical_match(0x5fff_ffff, 0x01 << 24, 0xff),
            "undefined model"
        );
    }
}
