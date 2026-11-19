use super::NvmeHandle;
use crate::IoApicHandle;
use crate::irq_source::BoardIrqSource;
use std::cell::Cell;

/// Samples PCI INTA at the existing APIC interrupt boundaries. The controller
/// commits the CQ entry before it is visible here. A failed APIC delivery does
/// not consume the transition, so the source can retry when the route is ready.
pub(crate) struct NvmeIoapicIrqSource {
    inner: Box<dyn BoardIrqSource>,
    nvme: NvmeHandle,
    io_apic: IoApicHandle,
    accepted_level: Cell<bool>,
}

impl NvmeIoapicIrqSource {
    pub(crate) fn new(
        inner: Box<dyn BoardIrqSource>,
        nvme: NvmeHandle,
        io_apic: IoApicHandle,
    ) -> Self {
        Self {
            inner,
            nvme,
            io_apic,
            accepted_level: Cell::new(false),
        }
    }

    fn sample(&self) -> bool {
        let Some(level) = self.nvme.try_pci_interrupt_line() else {
            return false;
        };
        // Re-present an asserted level even after it was accepted: a masked
        // IOAPIC may have been reset since the last sample.
        if (level || self.accepted_level.get()) && self.io_apic.set_nvme_line(level).is_ok() {
            self.accepted_level.set(level);
        }
        true
    }
}

impl BoardIrqSource for NvmeIoapicIrqSource {
    fn needs_device_line_refresh(&self) -> bool {
        true
    }

    fn try_refresh_device_lines(&self) -> bool {
        self.sample()
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
