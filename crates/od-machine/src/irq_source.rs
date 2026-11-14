//! The interrupt source hook of the Linux CPL0-set board (S4-38L8b).
//!
//! The board asks its source, on every instruction boundary of the set loop, whether a
//! maskable external vector is pending; the loop delivers it through the guest IDT
//! (`od_isa::deliver_external_interrupt_cpl0_set`) and only then tells the source. This slice
//! adds the hook and no source: the board owns [`NoIrqSource`], which never has a vector, so a run
//! behaves exactly as without the hook. A PIC, a PIT output or an IOAPIC is a later slice.

/// A source of maskable external interrupts on the board.
///
/// The three questions are cheap and allocation-free: `pending_vector` is asked once on every
/// instruction boundary of the set loop.
pub(crate) trait BoardIrqSource {
    /// The vector of the interrupt to deliver now: the line is active and not masked at the
    /// source. `None` when nothing is pending. The source is not changed by the question.
    fn pending_vector(&self) -> Option<u8>;

    /// The delivery of `vector` succeeded (`Delivered`): the frame is on the guest stack. Called
    /// only then, never after a refusal or an abort, and only for the vector `pending_vector`
    /// gave.
    fn acknowledge(&mut self, vector: u8);

    /// Absolute virtual time (ns, the board clock) of the next edge of the source, or `None` when
    /// nothing limits the quantum. A quantum never executes more instructions than are left until
    /// this time (at least one), because the clock advances only after a quantum.
    fn next_deadline_ns(&self) -> Option<u64>;

    /// Whether a guest write to the PIT ports `0x40..=0x43` ends the quantum, so that the next
    /// deadline is asked again after the new count took effect. `false` for a source that does not
    /// depend on the PIT.
    fn pit_write_ends_quantum(&self) -> bool {
        false
    }

    /// Edges of the PIT channel 0 line the source has seen so far (`irq0_edges` of the stop
    /// record). `0` for a source without IRQ 0.
    fn irq0_edges(&self) -> u64 {
        0
    }

    /// S4-39g: the absolute virtual time (ns, the board clock) at which a CPU halted by `HLT`
    /// with IF = 1 can next be woken: the time of now when a vector is deliverable now, the time
    /// of the next edge when that edge would be deliverable, `None` when nothing can wake it
    /// (the line is masked or in service, no edge is scheduled). `None` by default: a source that
    /// does not model the wake keeps a halted CPU halted (the run ends as `halted`, as before).
    fn wake_ns(&self) -> Option<u64> {
        None
    }
}

/// The source of a board without any interrupt controller: never a vector, never a deadline.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoIrqSource;

impl BoardIrqSource for NoIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        None
    }

    fn acknowledge(&mut self, _vector: u8) {}

    fn next_deadline_ns(&self) -> Option<u64> {
        None
    }
}

/// What the hook counted during one run of the set loop (the stop record carries them).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct IrqCounters {
    /// External interrupts delivered through the guest IDT.
    pub(crate) irqs_delivered: u64,
    /// IRQ 0 edges the source reported (copied from the source at the end of the run).
    pub(crate) irq0_edges: u64,
    /// Instruction boundaries at which a pending vector was held back because RFLAGS.IF = 0.
    pub(crate) irq_refused_masked: u64,
    /// The same because the one-instruction STI shadow was not `Clear`.
    pub(crate) irq_refused_shadow: u64,
    /// Quanta the board shortened to the deadline of the source.
    pub(crate) quantum_cut_by_deadline: u64,
    /// Quanta that ended on a guest write to the PIT ports (only for a source that asked for it).
    pub(crate) pit_write_quantum_ends: u64,
    /// S4-39g: times a halted CPU (`HLT`, IF = 1) was woken by the source (`wake_ns`): the clock
    /// moved to the wake time and the CPU went back to `Running`.
    pub(crate) halt_wakes: u64,
    /// S4-39g: virtual nanoseconds the clock skipped while the CPU was halted.
    pub(crate) halt_idle_ns: u64,
}

/// The hook the set loop gets for one call: the source, the counters and whether the first
/// boundary is inside a partial REP.
pub(crate) struct IrqBoundaryHook<'a> {
    pub(crate) source: &'a mut dyn BoardIrqSource,
    pub(crate) counters: &'a mut IrqCounters,
    /// The call resumes a partial REP (`RepIterationBudgetExhausted`): the first boundary is in
    /// the middle of the REP, nothing is delivered there.
    pub(crate) skip_first_boundary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_source_never_has_a_vector_nor_a_deadline() {
        let mut source = NoIrqSource;
        assert_eq!(source.pending_vector(), None);
        assert_eq!(source.next_deadline_ns(), None);
        assert!(!source.pit_write_ends_quantum());
        assert_eq!(source.irq0_edges(), 0);
        source.acknowledge(0x20);
        assert_eq!(source.pending_vector(), None);
    }

    #[test]
    fn counters_start_at_zero() {
        assert_eq!(
            IrqCounters::default(),
            IrqCounters {
                irqs_delivered: 0,
                irq0_edges: 0,
                irq_refused_masked: 0,
                irq_refused_shadow: 0,
                quantum_cut_by_deadline: 0,
                pit_write_quantum_ends: 0,
                halt_wakes: 0,
                halt_idle_ns: 0,
            }
        );
    }
}
