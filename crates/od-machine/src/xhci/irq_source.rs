//! The xHCI INTA line (GSI 17, level, active low) as a board interrupt source: it samples the
//! controller at the existing APIC interrupt boundaries and drives IOAPIC INTIN17, on top of
//! another source (the NVMe or UART chain).

use super::XhciHandle;
use crate::IoApicHandle;
use crate::irq_source::BoardIrqSource;
use std::cell::Cell;

/// Samples PCI INTA of the xHCI function. The controller has committed its event before the level
/// is visible here. A failed APIC delivery does not consume the transition, so the source can
/// retry when the route is ready.
pub(crate) struct XhciIoapicIrqSource {
    inner: Box<dyn BoardIrqSource>,
    xhci: XhciHandle,
    io_apic: IoApicHandle,
    accepted_level: Cell<bool>,
}

impl XhciIoapicIrqSource {
    pub(crate) fn new(
        inner: Box<dyn BoardIrqSource>,
        xhci: XhciHandle,
        io_apic: IoApicHandle,
    ) -> Self {
        Self {
            inner,
            xhci,
            io_apic,
            accepted_level: Cell::new(false),
        }
    }

    fn sample(&self) -> bool {
        let Some(level) = self.xhci.try_sample_pci_interrupts() else {
            return false;
        };
        self.publish(level);
        true
    }

    fn publish(&self, level: bool) {
        // Re-present an asserted level even after it was accepted: a masked IOAPIC may have
        // been reset since the last sample.
        if (level || self.accepted_level.get()) && self.io_apic.set_xhci_line(level).is_ok() {
            self.accepted_level.set(level);
        }
    }
}

impl BoardIrqSource for XhciIoapicIrqSource {
    fn needs_device_line_refresh(&self) -> bool {
        true
    }

    fn try_refresh_device_lines(&self) -> bool {
        self.sample() && self.inner.try_refresh_device_lines()
    }

    fn refresh_device_lines(&self) {
        self.publish(self.xhci.sample_pci_interrupts());
        self.inner.refresh_device_lines();
    }

    fn pending_vector(&self) -> Option<u8> {
        self.sample();
        self.inner.pending_vector()
    }

    fn acknowledge(&mut self, vector: u8) {
        self.inner.acknowledge(vector);
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.inner.next_deadline_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        self.inner.pit_write_ends_quantum()
    }

    fn irq0_edges(&self) -> u64 {
        self.inner.irq0_edges()
    }

    fn wake_ns(&self) -> Option<u64> {
        self.sample();
        self.inner.wake_ns()
    }
}
