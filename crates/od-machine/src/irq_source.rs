//! The interrupt source hook of the Linux CPL0-set board (S4-38L8b).
//!
//! The board asks its source, on every instruction boundary of the set loop, whether a
//! maskable external vector is pending; the loop delivers it through the guest IDT
//! (`od_isa::deliver_external_interrupt_cpl0_set`) and only then tells the source. This slice
//! adds the hook and no source: the board owns [`NoIrqSource`], which never has a vector, so a run
//! behaves exactly as without the hook. A PIC, a PIT output or an IOAPIC is a later slice.

/// A source of maskable external interrupts on the board.
///
/// `Send` (S6-33): a source moves to the thread of the vCPU that asks it. Its own state sits behind
/// a lock that is taken before any device lock it reaches (MACHINE_CONTRACT.md, lock order).
///
/// The three questions are cheap and allocation-free: `pending_vector` is asked once on every
/// instruction boundary of the set loop.
pub(crate) trait BoardIrqSource: Send {
    /// The vector of the interrupt to deliver now: the line is active and not masked at the
    /// source. `None` when nothing is pending. The source is not changed by the question.
    fn pending_vector(&self) -> Option<u8>;

    /// The delivery of `vector` succeeded (`Delivered`): the frame is on the guest stack. Called
    /// only then, never after a refusal or an abort, and only for the vector `pending_vector`
    /// gave.
    fn acknowledge(&mut self, vector: u8);

    /// Absolute virtual time (ns, the board clock) of the next edge of the source, or `None` when
    /// nothing limits the quantum. A quantum never executes more instructions than are left until
    /// this time (at least one), because the clock advances only after a quantum. Invariant: after
    /// the source has caught up with the clock (it applies every due edge when asked), it never
    /// returns a moment at or before now.
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

    /// Publish device line levels after a parallel quantum. A busy device can defer sampling
    /// to a continuation, returning false rather than waiting for in-flight device work.
    fn try_refresh_device_lines(&self) -> bool {
        true
    }
    /// Publish current device line levels while the scheduler owns the machine exclusively.
    fn refresh_device_lines(&self) {}

    /// Whether this source needs a post-quantum device line refresh.
    fn needs_device_line_refresh(&self) -> bool {
        false
    }

    /// S4-39g: the absolute virtual time (ns, the board clock) at which a CPU halted by `HLT`
    /// with IF = 1 can next be woken: the time of now when a vector is deliverable now, the time
    /// of the next edge when that edge would be deliverable, `None` when nothing can wake it
    /// (the line is masked or in service, no edge is scheduled). `None` by default: a source that
    /// does not model the wake keeps a halted CPU halted (the run ends as `halted`, as before).
    fn wake_ns(&self) -> Option<u64> {
        None
    }

    /// PERF-05: whether the answer of [`Self::pending_vector`] stays the same, with no change to
    /// the source or the devices it reads, as long as no guest access reaches a device through the
    /// bus (the device-access count of the bus does not move) and no vector was acknowledged. Then
    /// the board loop may reuse the last answer at the next instruction boundary of the same call.
    ///
    /// `true` requires that (a) the board clock does not move inside a call of the set loop, (b)
    /// the state `pending_vector` reads changes only by a guest port or MMIO access or by
    /// [`Self::acknowledge`], and (c) asking again without such a change changes nothing in the
    /// source (counters and statistics included). `false` by default: the source is asked at every
    /// boundary.
    fn stable_between_device_accesses(&self) -> bool {
        false
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

/// PERF-05: the last answer of `pending_vector` and the device-access count of the bus at the
/// moment it was asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PolledVector {
    pub(crate) vector: Option<u8>,
    pub(crate) device_accesses: u64,
}

/// The hook the set loop gets for one call: the source, the counters and whether the first
/// boundary is inside a partial REP.
pub(crate) struct IrqBoundaryHook<'a> {
    pub(crate) source: &'a mut dyn BoardIrqSource,
    pub(crate) counters: &'a mut IrqCounters,
    /// Optional callback-visible mirror, updated at the exact successful delivery boundary.
    pub(crate) delivered_mirror: Option<&'a std::cell::Cell<u64>>,
    /// The per-boundary flags: [`BOUNDARY_SKIP_FIRST`] — the call resumes a partial REP
    /// (`RepIterationBudgetExhausted`): the first boundary is in the middle of the REP, nothing is
    /// delivered there; [`BOUNDARY_CR8_LINKED`] — `cr8` is set (S7-32). One byte, so a boundary of
    /// a hook with neither costs one test ([`boundary_flags`] builds it).
    pub(crate) boundary_flags: u8,
    /// PERF-05: the count of guest device accesses of the bus
    /// ([`crate::bus::MachineBus::device_access_counter`]). `None`: the source is asked at every
    /// boundary.
    pub(crate) device_accesses: Option<&'a std::sync::atomic::AtomicU64>,
    /// PERF-05: the last answer of the source in this call; `None` before the first question and
    /// after a delivery.
    pub(crate) polled: Option<PolledVector>,
    /// S7-32: the CR8 cell of the CPL0 set bound to the TPR of the boot vCPU's local APIC, synced
    /// at every boundary before the source is asked. `None`: the set has no CR8 member.
    pub(crate) cr8: Option<&'a Cr8TprLink<'a>>,
}

/// [`IrqBoundaryHook::boundary_flags`]: the first boundary of the call is inside a partial REP.
pub(crate) const BOUNDARY_SKIP_FIRST: u8 = 1;
/// [`IrqBoundaryHook::boundary_flags`]: the hook carries a CR8 link (S7-32).
pub(crate) const BOUNDARY_CR8_LINKED: u8 = 2;

/// The flags of a hook that skips its first boundary or not and has `cr8` or not.
pub(crate) fn boundary_flags(skip_first_boundary: bool, cr8: Option<&Cr8TprLink<'_>>) -> u8 {
    u8::from(skip_first_boundary) * BOUNDARY_SKIP_FIRST
        | if cr8.is_some() {
            BOUNDARY_CR8_LINKED
        } else {
            0
        }
}

/// S7-32: CR8 of the CPL0 set (`Cpl0ProfileSet::with_cr8_tpr`) bound to the local APIC TPR, which
/// stays the only state (MACHINE_CONTRACT: "TPR remains the sole state source in `LocalApic`").
/// `MOV CR8, r64` writes the cell (0..=15); at the next instruction boundary [`Self::sync`] makes
/// it TPR = value << 4 (TPR[3:0] cleared, SDM Vol. 3A 11.8.6.5) before any interrupt is selected,
/// so the processor priority of that boundary already sees the new value. A guest write of the
/// TPR register through MMIO is a device access; after one, `sync` reads TPR[7:4] back into the
/// cell, so the next `MOV r64, CR8` reads it. One instruction cannot do both.
///
/// The cell holds what the link last put there; a CR8 write is a change of the cell. When the TPR
/// has bits 3:0 set (an MMIO write such as 0x2B) the link puts the class with bit 7 set into the
/// cell (`MOV r64, CR8` reads the low four bits only): a later CR8 write of the same class still
/// changes the cell and clears TPR[3:0]. A CR8 write retired as the last instruction of a run is
/// applied by [`Self::flush`] after the run.
pub(crate) struct Cr8TprLink<'a> {
    cell: &'a std::cell::Cell<u8>,
    lapic: crate::LocalApicHandle,
    /// The cell value the link last stored (or applied).
    synced: std::cell::Cell<u8>,
    /// The device-access count at the last read of the TPR.
    devices_seen: std::cell::Cell<Option<u64>>,
}

/// The cell value that stands for a TPR read back from the device (bit 7: TPR[3:0] not zero).
fn cell_for_tpr(tpr: u8) -> u8 {
    if tpr & 0xf == 0 {
        tpr >> 4
    } else {
        0x80 | (tpr >> 4)
    }
}

impl<'a> Cr8TprLink<'a> {
    /// Bind `cell` to the TPR of `lapic`; the cell starts at TPR[7:4].
    pub(crate) fn new(cell: &'a std::cell::Cell<u8>, lapic: crate::LocalApicHandle) -> Self {
        let value = cell_for_tpr(lapic.tpr());
        cell.set(value);
        Self {
            cell,
            lapic,
            synced: std::cell::Cell::new(value),
            devices_seen: std::cell::Cell::new(None),
        }
    }

    /// Apply a CR8 write the cell holds and the TPR does not have yet. `true`: the TPR moved.
    pub(crate) fn flush(&self) -> bool {
        let value = self.cell.get();
        if value == self.synced.get() {
            return false;
        }
        self.lapic.set_tpr_from_cr8(value & 0xf);
        let applied = value & 0xf;
        self.cell.set(applied);
        self.synced.set(applied);
        true
    }

    /// Reconcile the cell and the TPR at a boundary. `true`: a CR8 write moved the TPR (a source
    /// answer kept from before is stale). `device_accesses` is the bus counter of guest device
    /// accesses (PERF-05); without it the TPR is read at every call.
    pub(crate) fn sync(&self, device_accesses: Option<&std::sync::atomic::AtomicU64>) -> bool {
        if self.flush() {
            return true;
        }
        let accesses =
            device_accesses.map(|counter| counter.load(std::sync::atomic::Ordering::Relaxed));
        if accesses.is_some() && accesses == self.devices_seen.get() {
            return false;
        }
        self.devices_seen.set(accesses);
        let value = cell_for_tpr(self.lapic.tpr());
        if value != self.synced.get() {
            self.cell.set(value);
            self.synced.set(value);
        }
        false
    }
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

    /// S7-32: CR8 of the CPL0 set and the LAPIC TPR are one state. A CR8 write of 15 takes effect
    /// at the next boundary (TPR = 0xF0: a pending vector 0x50 is held), lowering it releases the
    /// vector at the next boundary although no device was reached (the answer kept by PERF-05 is
    /// dropped), and a TPR written through the device (an MMIO access) is read back as CR8.
    #[test]
    fn s7_32_cr8_and_the_tpr_are_one_state_at_every_boundary() {
        let apic = crate::LocalApic::new();
        let handle = apic.handle();
        handle.set_global_enabled(true).unwrap();
        handle.0.lock().unwrap().cpus[0].local.svr = 0x1ff;
        handle.request_fixed_edge(0x50).unwrap();
        let cell = std::cell::Cell::new(9u8);
        let link = Cr8TprLink::new(&cell, handle.clone());
        assert_eq!(cell.get(), 0, "the cell starts at TPR[7:4]");
        let mut source = crate::lapic_irq_source::LapicIrqSource::new(handle.clone());
        let mut counters = IrqCounters::default();
        let accesses = std::sync::atomic::AtomicU64::new(7);
        let mut hook = IrqBoundaryHook {
            source: &mut source,
            counters: &mut counters,
            delivered_mirror: None,
            boundary_flags: boundary_flags(false, Some(&link)),
            device_accesses: Some(&accesses),
            polled: None,
            cr8: Some(&link),
        };
        let mut state = od_isa::LongModeState::default();
        state.rflags = 0x202;
        state.sti_shadow = Some(od_isa::StiShadow::Clear);
        let poll = |hook: &mut IrqBoundaryHook<'_>, state: &od_isa::LongModeState| {
            crate::paged_memory_adapter::poll_pending_irq(hook, state)
        };
        assert_eq!(poll(&mut hook, &state), Some(0x50));
        // MOV CR8, 15 (the set's member writes the cell).
        cell.set(15);
        assert_eq!(poll(&mut hook, &state), None);
        assert_eq!(handle.tpr(), 0xf0);
        // MOV CR8, 0 with no device access in between.
        cell.set(0);
        assert_eq!(poll(&mut hook, &state), Some(0x50));
        assert_eq!(handle.tpr(), 0);
        // A TPR write through the device (counted as a device access) is CR8 at the next
        // boundary.
        handle.0.lock().unwrap().cpus[0].local.tpr = 0x6b;
        accesses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(poll(&mut hook, &state), None, "0x50 is below class 6");
        assert_eq!(cell.get() & 0xf, 6);
        // TPR 0x2B through the device, then `MOV CR8, 2` (the same class): the CR8 write clears
        // TPR[3:0] (SDM Vol. 3A 11.8.6.5), TPR = 0x20.
        handle.0.lock().unwrap().cpus[0].local.tpr = 0x2b;
        accesses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        poll(&mut hook, &state);
        assert_eq!(cell.get() & 0xf, 2, "MOV r64, CR8 reads TPR[7:4]");
        cell.set(2);
        poll(&mut hook, &state);
        assert_eq!(handle.tpr(), 0x20);
        // `flush` applies a write without a boundary.
        cell.set(9);
        assert!(link.flush());
        assert_eq!(handle.tpr(), 0x90);
        assert!(!link.flush());
    }
}
