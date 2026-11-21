//! The event ring of Interrupter 0: a single ERST segment written by the
//! controller (`docs/specs/XHCI_DEVICE.md` section 5, "Кольцо событий").

use super::trb::{TRB_BYTES, Trb};
use crate::nvme::{DmaError, GuestDma};

/// Valid segment sizes in TRBs (xHCI 6.5: 16..4096).
pub(super) const MIN_SEGMENT_TRBS: u32 = 16;
pub(super) const MAX_SEGMENT_TRBS: u32 = 4096;

/// Producer state of the event ring. `used` counts the events the driver has not
/// yet acknowledged by moving ERDP; the last free TRB is kept for the Event Ring
/// Full Error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EventRing {
    base: u64,
    size: u32,
    enqueue: u32,
    dequeue: u32,
    used: u32,
    cycle: bool,
    full_reported: bool,
}

impl EventRing {
    /// A ring over a validated segment: enqueue at the start, cycle bit 1.
    pub(super) fn new(base: u64, size: u32) -> Self {
        Self {
            base,
            size,
            enqueue: 0,
            dequeue: 0,
            used: 0,
            cycle: true,
            full_reported: false,
        }
    }

    pub(super) fn free(&self) -> u32 {
        self.size - self.used
    }

    /// Event Ring Full Error is already in the ring and ERDP has not moved since.
    /// Events written and not yet acknowledged by ERDP.
    pub(super) fn unconsumed(&self) -> u32 {
        self.used
    }

    /// Room for one event and the TRB reserved for the Event Ring Full Error.
    pub(super) fn has_room(&self) -> bool {
        !self.full_reported && self.free() >= 2
    }

    pub(super) fn full_reported(&self) -> bool {
        self.full_reported
    }

    pub(super) fn mark_full_reported(&mut self) {
        self.full_reported = true;
    }

    /// Writes `trb` at the enqueue pointer with the producer cycle bit; DW3 (with the
    /// cycle bit) goes last, so the guest never sees a half-written TRB.
    pub(super) fn write(&mut self, dma: &mut dyn GuestDma, trb: Trb) -> Result<(), DmaError> {
        let address = self.base + u64::from(self.enqueue) * TRB_BYTES;
        let mut head = [0u8; 12];
        for (index, chunk) in head.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&trb.0[index].to_le_bytes());
        }
        let control = (trb.0[3] & !1) | u32::from(self.cycle);
        dma.write(address, &head)?;
        dma.write(address + 12, &control.to_le_bytes())?;
        self.enqueue += 1;
        if self.enqueue == self.size {
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }
        self.used += 1;
        Ok(())
    }

    /// The driver wrote ERDP. A pointer outside the segment is ignored; moving the
    /// dequeue pointer acknowledges events and ends a full-ring episode. In a full ring the
    /// enqueue and dequeue pointers are equal, so a driver that consumed every event writes the
    /// pointer it already had: such a write that also clears EHB (`acknowledged`) counts as
    /// having consumed them all.
    pub(super) fn set_dequeue_pointer(&mut self, pointer: u64, acknowledged: bool) {
        let pointer = pointer & !0xF;
        let end = self.base + u64::from(self.size) * TRB_BYTES;
        if pointer < self.base || pointer >= end {
            return;
        }
        let index = ((pointer - self.base) / TRB_BYTES) as u32;
        let mut consumed = (index + self.size - self.dequeue) % self.size;
        if consumed == 0 && acknowledged && self.used == self.size {
            consumed = self.size;
        }
        if consumed > self.used {
            // ERDP past the enqueue pointer cannot be right: keep the old dequeue pointer.
            return;
        }
        self.dequeue = index;
        self.used = self.used.saturating_sub(consumed);
        if consumed > 0 {
            self.full_reported = false;
        }
    }
}
