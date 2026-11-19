//! The interrupt source of one vCPU's own Local APIC (S6-16a): the vectors that wait in the IRR of
//! that processor's LAPIC (a self-IPI, an IPI from another vCPU, an IOAPIC line routed to it) are
//! delivered through that vCPU's guest IDT, and acknowledged in that LAPIC only.
//!
//! [`LapicIrqSource`] is the [`BoardIrqSource`] a vCPU other than the boot processor hands to the
//! set loop. The boot processor's source of the Linux board is [`crate::PitIoapicIrqSource`], which
//! owns the clock-driven part of the same question (PIT edges, the queue of events); this source has
//! none of it: it only reads and moves bits of the LAPIC its handle addresses.
//!
//! The three questions of the trait:
//! * `pending_vector` is `LocalApicHandle::eligible` of its own LAPIC: the highest IRR vector that
//!   the software-enabled LAPIC could accept (priority class above PPR). The question changes
//!   nothing.
//! * `acknowledge` moves the delivered vector of the same LAPIC from IRR to ISR once the frame is
//!   on the guest stack. A higher vector queued during delivery waits for the next boundary.
//!   The guest's `EOI` write (as that vCPU) clears this LAPIC's ISR.
//! * `next_deadline_ns` is always `None`: the LAPIC timer deadlines are the clock owner's
//!   (S4-23j, S6-15), not a limit of this source. A timer event that fires moves a vector into IRR,
//!   and the next boundary asks `pending_vector` again.
//!
//! `wake_ns` keeps the trait default `None`: a vCPU halted by `HLT` is woken by the scheduler of
//! S6-15 from the LAPIC, not by this source. The semantics of the Local APIC that the repository
//! does not hold are SDM facts from memory.

use crate::LocalApicHandle;
use crate::irq_source::BoardIrqSource;

/// The source of the vectors of one vCPU's Local APIC.
#[derive(Clone)]
pub(crate) struct LapicIrqSource {
    lapic: LocalApicHandle,
    stop_on_pit_write: bool,
}

impl LapicIrqSource {
    /// The source of the LAPIC `lapic` addresses (the handle carries the vCPU index).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(lapic: LocalApicHandle) -> Self {
        Self {
            lapic,
            stop_on_pit_write: false,
        }
    }

    pub(crate) fn with_pit_write_boundary(mut self, enabled: bool) -> Self {
        self.stop_on_pit_write = enabled;
        self
    }
}

impl BoardIrqSource for LapicIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        self.lapic.eligible()
    }

    fn acknowledge(&mut self, vector: u8) {
        self.lapic.acknowledge_delivered_vector(vector);
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        None
    }

    fn pit_write_ends_quantum(&self) -> bool {
        self.stop_on_pit_write
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{AccessWidth, BusDevice, DeviceAccess, IoSpace};
    use crate::{LocalApic, LocalApicHandle};

    const SELF_ASSERT: u64 = 0x4_4000;

    fn dword(offset: u64) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        }
    }

    /// A two-processor LAPIC with both software-enabled (SVR = 0x1FF).
    fn two_enabled() -> (LocalApic, LocalApicHandle, LocalApicHandle) {
        let mut device = LocalApic::with_cpus(2).unwrap();
        for cpu in 0..2 {
            device.write_from_cpu(cpu, dword(0xf0), 0x1ff).unwrap();
        }
        let (a, b) = (device.handle_for(0).unwrap(), device.handle_for(1).unwrap());
        (device, a, b)
    }

    #[test]
    fn it_reports_the_vector_of_its_own_lapic_only() {
        let (mut device, lapic0, lapic1) = two_enabled();
        let (source0, source1) = (
            LapicIrqSource::new(lapic0.clone()),
            LapicIrqSource::new(lapic1.clone()),
        );
        assert_eq!(
            (source0.pending_vector(), source1.pending_vector()),
            (None, None)
        );
        // Self-IPI of vCPU 1: vector 0x30 (class 3, above PPR 0).
        device
            .write_from_cpu(1, dword(0x300), SELF_ASSERT | 0x30)
            .unwrap();
        assert_eq!(source1.pending_vector(), Some(0x30));
        assert_eq!(source0.pending_vector(), None);
        // The question changed nothing.
        assert_eq!(lapic1.vector_state(0x30), (true, false));
    }

    #[test]
    fn acknowledge_moves_the_vector_in_its_own_lapic_and_leaves_the_other_alone() {
        let (mut device, lapic0, lapic1) = two_enabled();
        // The same vector waits in both processors.
        for cpu in 0..2 {
            device
                .write_from_cpu(cpu, dword(0x300), SELF_ASSERT | 0x30)
                .unwrap();
        }
        let mut source1 = LapicIrqSource::new(lapic1.clone());
        source1.acknowledge(0x30);
        assert_eq!(lapic1.vector_state(0x30), (false, true));
        assert_eq!(lapic0.vector_state(0x30), (true, false));
        // In service on vCPU 1: nothing of that class is eligible there any more, vCPU 0 is intact.
        assert_eq!(source1.pending_vector(), None);
        assert_eq!(LapicIrqSource::new(lapic0).pending_vector(), Some(0x30));
    }

    #[test]
    fn acknowledge_of_a_vector_that_is_not_pending_changes_nothing() {
        let (mut device, _lapic0, lapic1) = two_enabled();
        device
            .write_from_cpu(1, dword(0x300), SELF_ASSERT | 0x30)
            .unwrap();
        let mut source1 = LapicIrqSource::new(lapic1.clone());
        source1.acknowledge(0x31);
        assert_eq!(lapic1.vector_state(0x30), (true, false));
        assert_eq!(lapic1.vector_state(0x31), (false, false));
    }

    #[test]
    fn a_higher_ipi_queued_during_delivery_does_not_repeat_the_selected_vector() {
        let (mut device, lapic0, lapic1) = two_enabled();
        let mut source = LapicIrqSource::new(lapic1.clone());
        lapic0.write_icr(0, 1, 0x30).unwrap();
        let selected = source.pending_vector().unwrap();
        assert_eq!(selected, 0x30);

        // A remote sender can enqueue an IPI while the target writes the guest stack frame.
        lapic0.write_icr(0, 1, 0x50).unwrap();
        source.acknowledge(selected);
        assert_eq!(lapic1.vector_state(0x30), (false, true));
        assert_eq!(lapic1.vector_state(0x50), (true, false));

        let next = source.pending_vector().unwrap();
        assert_eq!(next, 0x50);
        source.acknowledge(next);
        device.write_from_cpu(1, dword(0xb0), 0).unwrap();
        assert_eq!(lapic1.vector_state(0x30), (false, true));
        assert_eq!(lapic1.vector_state(0x50), (false, false));
        device.write_from_cpu(1, dword(0xb0), 0).unwrap();
        assert_eq!(lapic1.vector_state(0x30), (false, false));
        assert_eq!(source.pending_vector(), None);
    }

    #[test]
    fn it_has_no_deadline_and_the_trait_defaults_elsewhere() {
        let (_device, _lapic0, lapic1) = two_enabled();
        let source = LapicIrqSource::new(lapic1);
        assert_eq!(source.next_deadline_ns(), None);
        assert_eq!(source.wake_ns(), None);
        assert!(!source.pit_write_ends_quantum());
        assert_eq!(source.irq0_edges(), 0);
    }

    #[test]
    fn a_software_disabled_lapic_offers_no_vector() {
        let mut device = LocalApic::with_cpus(2).unwrap();
        let lapic1 = device.handle_for(1).unwrap();
        // SVR left at its reset value (software disabled): the self-IPI is accepted into IRR
        // by the model only when enabled, so the source has nothing either way.
        let _ = device.write_from_cpu(1, dword(0x300), SELF_ASSERT | 0x30);
        assert_eq!(LapicIrqSource::new(lapic1).pending_vector(), None);
    }
}
