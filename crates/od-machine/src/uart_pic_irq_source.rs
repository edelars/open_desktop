//! The UART THRE line through the 8259 master IR4 (S4-39i), on top of the PIT -> IR0 source.
//!
//! The kernel's 8250 driver sends `tty` output by enabling the transmitter-holding-register-empty
//! interrupt (IER bit 1) and writing the bytes to THR from its interrupt handler. Without a wire
//! from the UART to the 8259 the first byte is never written (S4-39g2: `write(1, ...)` returned,
//! COM1 stayed empty). This source adds that wire: the UART interrupt output
//! ([`Uart16550Handle::interrupt_line`], a pure level: OUT2 and an enabled pending condition) is
//! sampled on every question the loop asks, and a rising level is an edge on master IR4
//! ([`Pic8259Handle::raise_irq`]). The PIT -> IR0 source, the vector delivery, the INTA and the
//! guest EOI are the existing machinery of [`PitPicIrqSource`] and the 8259 model.
//!
//! The UART model itself is unchanged: the board loop already moves a THR byte to the COM1
//! transcript right after the quantum that wrote it (`transfer_holding_to_shift` and
//! `complete_transmission`), which empties THR again and raises the THRE condition when the guest
//! enabled it; the guest clears it by reading IIR (THRE source) or by writing THR. Semantics
//! follow the 16550 datasheet from memory and were not checked against a primary source.
//!
//! The line is edge-triggered towards the 8259 (the Linux init is edge mode): while the level
//! stays high no second edge is raised. The sampling happens at each instruction boundary
//! (`pending_vector`) and before a halted CPU is woken (`wake_ns`), so a level that rises
//! (IER write, THR pump) and falls (IIR read) is always seen between two guest instructions. The
//! board loop also samples right before and right after it pumps a THR byte to COM1: the THR write
//! drops the condition and the pump raises it again inside one quantum end, and both sides must be
//! seen to make an edge.

use crate::irq_source::BoardIrqSource;
use crate::pit_pic_irq_source::PitPicIrqSource;
use crate::{Pic8259Handle, Uart16550Handle};
use std::cell::Cell;
use std::rc::Rc;

/// The 8259 master input line the UART interrupt output is wired to (ISA IRQ 4, COM1).
pub(crate) const UART_IRQ_LINE: u8 = 4;

struct Inner {
    pit_pic: PitPicIrqSource,
    uart: Uart16550Handle,
    pic: Pic8259Handle,
    /// The UART interrupt level at the last sample.
    level: Cell<bool>,
    /// Rising edges presented to IR4 (a masked one included).
    edges: Cell<u64>,
}

/// The board source with the PIT on IR0 and the UART on IR4. Cloning gives a second handle on the
/// same source (the board owns one, a test keeps another to read the counter).
#[derive(Clone)]
pub(crate) struct UartPicIrqSource(Rc<Inner>);

impl UartPicIrqSource {
    /// Wraps the PIT -> IR0 source; `uart` and `pic` are the handles of the same board.
    pub(crate) fn new(pit_pic: PitPicIrqSource, uart: Uart16550Handle, pic: Pic8259Handle) -> Self {
        Self(Rc::new(Inner {
            pit_pic,
            uart,
            pic,
            level: Cell::new(false),
            edges: Cell::new(0),
        }))
    }

    /// Rising edges of the UART line presented to IR4 so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn uart_edges(&self) -> u64 {
        self.0.edges.get()
    }

    /// Samples the UART interrupt output; a rising level is an edge on IR4.
    pub(crate) fn sample(&self) {
        let inner = &*self.0;
        let level = inner.uart.interrupt_line();
        if level && !inner.level.get() {
            inner.pic.raise_irq(UART_IRQ_LINE);
            inner.edges.set(inner.edges.get() + 1);
        }
        inner.level.set(level);
    }
}

impl BoardIrqSource for UartPicIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        self.sample();
        self.0.pit_pic.pending_vector()
    }

    fn acknowledge(&mut self, vector: u8) {
        let mut source = self.0.pit_pic.clone();
        source.acknowledge(vector);
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.0.pit_pic.next_deadline_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        self.0.pit_pic.pit_write_ends_quantum()
    }

    fn irq0_edges(&self) -> u64 {
        self.0.pit_pic.irq0_edges()
    }

    fn wake_ns(&self) -> Option<u64> {
        self.sample();
        self.0.pit_pic.wake_ns()
    }
}

#[cfg(test)]
mod tests;
