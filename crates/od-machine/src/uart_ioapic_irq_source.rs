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
use std::sync::{Arc, Mutex, MutexGuard};

struct Inner {
    pit_ioapic: PitIoapicIrqSource,
    uart: Uart16550Handle,
    io_apic: IoApicHandle,
    /// The sample state. Its lock is taken before the UART and APIC locks (MACHINE_CONTRACT.md,
    /// S6-33 lock order) and is held across the sample.
    line: Mutex<Line>,
    /// The COM2 agent channel UART on INTIN3 (S11-21), with its own sample state.
    com2: Option<Com2>,
}

struct Com2 {
    uart: Uart16550Handle,
    line: Mutex<Line>,
}

#[derive(Default)]
struct Line {
    /// The UART interrupt level the IOAPIC accepted last.
    level: bool,
    /// Rising levels accepted by INTIN4 (a masked one included).
    edges: u64,
    /// Rising levels the IOAPIC refused (counted once per rise).
    refused: u64,
    /// A rise was refused and is being retried.
    refusing: bool,
}

/// The board source with the PIT on INTIN2 and the UART on INTIN4. Cloning gives a second handle
/// on the same source (the board owns one, a test keeps another to read the counters).
#[derive(Clone)]
pub(crate) struct UartIoapicIrqSource(Arc<Inner>);

impl UartIoapicIrqSource {
    /// Wraps the PIT -> INTIN2 source; `uart` and `io_apic` are the handles of the same board.
    ///
    /// `uart` is COM1 and drives INTIN4, so it must be placed on IRQ4; another line is a
    /// programming error. The COM2 line (IRQ3, INTIN3, S11-21) is added by [`Self::new_with_com2`].
    pub(crate) fn new(
        pit_ioapic: PitIoapicIrqSource,
        uart: Uart16550Handle,
        io_apic: IoApicHandle,
    ) -> Self {
        Self::build(pit_ioapic, uart, io_apic, None)
    }

    /// As [`Self::new`], and the COM2 UART (the agent channel, IRQ3) is presented to INTIN3 by the
    /// same samples. COM1 behaves exactly as without it.
    pub(crate) fn new_with_com2(
        pit_ioapic: PitIoapicIrqSource,
        uart: Uart16550Handle,
        com2: Uart16550Handle,
        io_apic: IoApicHandle,
    ) -> Self {
        assert_eq!(
            com2.port_spec().irq_line,
            3,
            "the second IOAPIC UART drives INTIN3 only"
        );
        let com2 = Com2 {
            uart: com2,
            line: Mutex::new(Line::default()),
        };
        Self::build(pit_ioapic, uart, io_apic, Some(com2))
    }

    fn build(
        pit_ioapic: PitIoapicIrqSource,
        uart: Uart16550Handle,
        io_apic: IoApicHandle,
        com2: Option<Com2>,
    ) -> Self {
        assert_eq!(
            uart.port_spec().irq_line,
            4,
            "the IOAPIC UART source drives INTIN4 with COM1"
        );
        Self(Arc::new(Inner {
            pit_ioapic,
            uart,
            io_apic,
            line: Mutex::new(Line::default()),
            com2,
        }))
    }

    fn line(&self) -> MutexGuard<'_, Line> {
        self.0.line.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Rising levels of the UART line presented to INTIN4 and accepted so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_edges(&self) -> u64 {
        self.line().edges
    }

    /// Rising levels INTIN4 refused (the Local APIC was disabled or the vector was busy).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_refused(&self) -> u64 {
        self.line().refused
    }

    /// A second handle on the wrapped PIT -> INTIN2 source (its counters).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pit_source(&self) -> PitIoapicIrqSource {
        self.0.pit_ioapic.clone()
    }

    /// Rising levels of the COM2 line accepted by INTIN3 (zero without COM2).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn com2_edges(&self) -> u64 {
        self.0.com2.as_ref().map_or(0, |com2| {
            com2.line.lock().unwrap_or_else(|e| e.into_inner()).edges
        })
    }

    /// Samples the UART interrupt outputs and presents a change of each to its input: COM1 to
    /// INTIN4, the COM2 agent channel (when present) to INTIN3.
    pub(crate) fn sample(&self) {
        let inner = &*self.0;
        Self::sample_line(&mut self.line(), inner.uart.interrupt_line(), |level| {
            inner.io_apic.set_intin4(level)
        });
        if let Some(com2) = &inner.com2 {
            let mut state = com2.line.lock().unwrap_or_else(|e| e.into_inner());
            Self::sample_line(&mut state, com2.uart.interrupt_line(), |level| {
                inner.io_apic.set_intin3(level)
            });
        }
    }

    fn sample_line(
        state: &mut Line,
        line: bool,
        present: impl FnOnce(bool) -> Result<(), crate::DeviceError>,
    ) {
        if line == state.level {
            if !line {
                state.refusing = false;
            }
            return;
        }
        match present(line) {
            Ok(()) => {
                state.level = line;
                state.refusing = false;
                if line {
                    state.edges += 1;
                }
            }
            Err(_) => {
                // Nothing changed in the IOAPIC; the next sample tries again.
                if !std::mem::replace(&mut state.refusing, true) {
                    state.refused += 1;
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
