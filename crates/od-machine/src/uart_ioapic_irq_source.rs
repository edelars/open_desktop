//! The UART interrupt line through the IOAPIC INTIN4 (S4-38 "UART -> INTIN4"), on top of the PIT ->
//! IOAPIC INTIN2 source of the APIC profile (`X8664V1Stage4AApic`).
//!
//! The UART interrupt output ([`Uart16550Handle::interrupt_line`], a pure level: OUT2 and an
//! enabled pending condition, RDA or THRE) is sampled on every question the loop asks and is
//! presented to the IOAPIC as the level of INTIN4 ([`IoApicHandle::set_intin4`]), as the 8259
//! variant (`uart_pic_irq_source`, S4-39i) presents it to master IR4. Nothing of the IOAPIC and
//! the Local APIC semantics is new: the existing INTIN4 rules of `apic_pair` decide what the
//! level does. With the redirection entry 4 in the level form the request is IRR and TMR with a
//! Remote IRR until the guest's EOI, and the EOI redelivers it while the line stays high; with the
//! edge form a rising level is one edge on the vector (an edge on a set IRR bit coalesces); a
//! masked entry takes no request (the level is only recorded, unmasking a level entry whose line
//! is high delivers it). The Local APIC decides what is deliverable (`eligible`); the vector
//! delivery, the INTA-like `acknowledge` and the wake of a halted CPU are the ones of
//! [`PitIoapicIrqSource`], which this source wraps.
//!
//! The 8259 pair gets nothing from this source; the two UART sources (this one and
//! `UartPicIrqSource`) are exclusive because they wrap the two exclusive PIT sources.
//!
//! # A refused request
//!
//! `set_intin4(true)` is refused (nothing changes, the transition is not consumed) when the Local
//! APIC is disabled, or the level vector is still in IRR or ISR. The source keeps the level it
//! last presented, so the next sample tries again; the refusals are counted once per rising
//! level. The line falling is always accepted.
//!
//! # Sampling
//!
//! The sampling happens at each instruction boundary (`pending_vector`) and before a halted CPU is
//! woken (`wake_ns`), so a level that rises (IER write, an injected receive byte, the THR pump) is
//! seen between two guest instructions, and the wake of a halted CPU sees the request in IRR. The
//! board loop also samples right before and right after it pumps a THR byte to COM1, as for the
//! 8259 variant. Semantics follow the 16550 datasheet from memory and were not checked against a
//! primary source.

use crate::irq_source::BoardIrqSource;
use crate::pit_ioapic_irq_source::PitIoapicIrqSource;
use crate::{IoApicHandle, Uart16550Handle};
use std::cell::Cell;
use std::rc::Rc;

struct Inner {
    pit_ioapic: PitIoapicIrqSource,
    uart: Uart16550Handle,
    io_apic: IoApicHandle,
    /// The UART interrupt level the IOAPIC accepted last.
    level: Cell<bool>,
    /// Rising levels accepted by INTIN4 (a masked one included).
    edges: Cell<u64>,
    /// Rising levels the IOAPIC refused (counted once per rise).
    refused: Cell<u64>,
    /// A rise was refused and is being retried.
    refusing: Cell<bool>,
}

/// The board source with the PIT on INTIN2 and the UART on INTIN4. Cloning gives a second handle
/// on the same source (the board owns one, a test keeps another to read the counters).
#[derive(Clone)]
pub(crate) struct UartIoapicIrqSource(Rc<Inner>);

impl UartIoapicIrqSource {
    /// Wraps the PIT -> INTIN2 source; `uart` and `io_apic` are the handles of the same board.
    pub(crate) fn new(
        pit_ioapic: PitIoapicIrqSource,
        uart: Uart16550Handle,
        io_apic: IoApicHandle,
    ) -> Self {
        Self(Rc::new(Inner {
            pit_ioapic,
            uart,
            io_apic,
            level: Cell::new(false),
            edges: Cell::new(0),
            refused: Cell::new(0),
            refusing: Cell::new(false),
        }))
    }

    /// Rising levels of the UART line presented to INTIN4 and accepted so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_edges(&self) -> u64 {
        self.0.edges.get()
    }

    /// Rising levels INTIN4 refused (the Local APIC was disabled or the vector was busy).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_refused(&self) -> u64 {
        self.0.refused.get()
    }

    /// A second handle on the wrapped PIT -> INTIN2 source (its counters).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pit_source(&self) -> PitIoapicIrqSource {
        self.0.pit_ioapic.clone()
    }

    /// Samples the UART interrupt output and presents a change of it to INTIN4.
    pub(crate) fn sample(&self) {
        let inner = &*self.0;
        let line = inner.uart.interrupt_line();
        if line == inner.level.get() {
            if !line {
                inner.refusing.set(false);
            }
            return;
        }
        match inner.io_apic.set_intin4(line) {
            Ok(()) => {
                inner.level.set(line);
                inner.refusing.set(false);
                if line {
                    inner.edges.set(inner.edges.get() + 1);
                }
            }
            Err(_) => {
                // Nothing changed in the IOAPIC; the next sample tries again.
                if !inner.refusing.replace(true) {
                    inner.refused.set(inner.refused.get() + 1);
                }
            }
        }
    }
}

impl BoardIrqSource for UartIoapicIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        self.sample();
        self.0.pit_ioapic.pending_vector()
    }

    fn acknowledge(&mut self, vector: u8) {
        let mut source = self.0.pit_ioapic.clone();
        source.acknowledge(vector);
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.0.pit_ioapic.next_deadline_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        self.0.pit_ioapic.pit_write_ends_quantum()
    }

    fn irq0_edges(&self) -> u64 {
        self.0.pit_ioapic.irq0_edges()
    }

    fn wake_ns(&self) -> Option<u64> {
        self.sample();
        self.0.pit_ioapic.wake_ns()
    }
}
