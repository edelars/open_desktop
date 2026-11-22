//! The xHCI INTA line as a board interrupt source: controller event to IOAPIC INTIN17 to the
//! Local APIC, and back when the driver acknowledges (S8-42, board wiring).

use super::*;
use crate::irq_source::{BoardIrqSource, NoIrqSource};
use crate::xhci::irq_source::XhciIoapicIrqSource;
use crate::{BusDevice, DeviceAccess, IoApic, IoSpace, LocalApic};

const VECTOR: u64 = 0x41;
const XHCI_ENTRY_LOW: u64 = 0x10 + 2 * 17;
/// Level (bit 15), active low (bit 13), unmasked.
const LEVEL_ACTIVE_LOW: u64 = (1 << 15) | (1 << 13);

fn dword(offset: u64) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Mmio,
        offset,
        width: AccessWidth::Dword,
    }
}

#[test]
fn an_event_raises_gsi_17_and_clearing_ip_lowers_it_with_the_eoi() {
    let mut local = LocalApic::new();
    let apic = local.handle();
    apic.set_global_enabled(true).unwrap();
    local.write(dword(0xF0), 0x1FF).unwrap();
    let mut io = IoApic::new(apic.clone());
    io.write(dword(0), XHCI_ENTRY_LOW).unwrap();
    io.write(dword(0x10), LEVEL_ACTIVE_LOW | VECTOR).unwrap();
    let mut rig = running_rig();
    let source = XhciIoapicIrqSource::new(Box::new(NoIrqSource), rig.xhci.clone(), io.handle());
    source.refresh_device_lines();
    assert_eq!(apic.pending(), None, "no event, no interrupt");
    rig.command(no_op());
    assert!(rig.xhci.sample_pci_interrupts());
    source.refresh_device_lines();
    assert_eq!(
        apic.pending(),
        Some(VECTOR as u8),
        "INTA reaches the Local APIC"
    );
    assert_eq!(apic.acknowledge(), Some(VECTOR as u8));
    // The driver clears IP; the next sample lowers the line, the EOI ends the request.
    rig.set_dword(IMAN, IP | IE);
    source.refresh_device_lines();
    local.write(dword(0xB0), 0).unwrap();
    assert_eq!(apic.pending(), None, "the line is low: no redelivery");
}

#[test]
fn a_masked_input_holds_the_line_until_the_driver_unmasks_it() {
    let mut local = LocalApic::new();
    let apic = local.handle();
    apic.set_global_enabled(true).unwrap();
    local.write(dword(0xF0), 0x1FF).unwrap();
    let mut io = IoApic::new(apic.clone());
    io.write(dword(0), XHCI_ENTRY_LOW).unwrap();
    io.write(dword(0x10), LEVEL_ACTIVE_LOW | VECTOR | (1 << 16))
        .unwrap();
    let mut rig = running_rig();
    let source = XhciIoapicIrqSource::new(Box::new(NoIrqSource), rig.xhci.clone(), io.handle());
    rig.command(no_op());
    source.refresh_device_lines();
    assert_eq!(apic.pending(), None, "masked");
    io.write(dword(0), XHCI_ENTRY_LOW).unwrap();
    io.write(dword(0x10), LEVEL_ACTIVE_LOW | VECTOR).unwrap();
    assert_eq!(
        apic.pending(),
        Some(VECTOR as u8),
        "unmasking delivers the held line"
    );
}

#[test]
fn command_intx_disable_keeps_the_line_off() {
    let mut local = LocalApic::new();
    let apic = local.handle();
    apic.set_global_enabled(true).unwrap();
    local.write(dword(0xF0), 0x1FF).unwrap();
    let mut io = IoApic::new(apic.clone());
    io.write(dword(0), XHCI_ENTRY_LOW).unwrap();
    io.write(dword(0x10), LEVEL_ACTIVE_LOW | VECTOR).unwrap();
    let mut rig = running_rig();
    let source = XhciIoapicIrqSource::new(Box::new(NoIrqSource), rig.xhci.clone(), io.handle());
    rig.cfg_write(
        0x04,
        AccessWidth::Word,
        COMMAND_MSE | COMMAND_BME | COMMAND_INTX_DISABLE,
    );
    rig.command(no_op());
    source.refresh_device_lines();
    assert_eq!(apic.pending(), None);
    assert!(
        source.try_refresh_device_lines(),
        "a free controller is sampled"
    );
}
