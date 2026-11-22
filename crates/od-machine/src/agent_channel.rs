//! The agent channel device: the host side of the second UART (COM2) of the board (S11-21,
//! ADR-0007, `docs/specs/AGENT_CHANNEL_DEVICE.md`).
//!
//! The guest sees an ordinary 16450-profile [`Uart16550PollingTx`] at `0x2F8..0x2FF`
//! ([`UartPortSpec::COM2`], no FIFO). This module adds the bounded host queues around it:
//!
//! * host to guest: bytes are queued in an RX queue and fed one at a time to the existing
//!   [`Uart16550Handle::inject_received_byte`] whenever the receive register is empty;
//! * guest to host: bytes the guest wrote to THR leave through the two-stage TX path
//!   (`transfer_holding_to_shift`, `complete_transmission`) into a TX queue.
//!
//! Both queues are bounded. A byte that does not fit is lost and counted: a write to the guest
//! accepts only what fits (the caller sees the number) and counts the refused rest in
//! `rx_overflow_bytes`; a guest byte that completes while the TX queue is full is dropped and
//! counted in `tx_overflow_bytes`. The agent protocol tolerates lost bytes
//! (`protocols/guest/AGENT_PROTOCOL_V1.md`). The model has no baud timing, so the transfer speed
//! is the speed of [`AgentChannel::pump`].
//!
//! # Pumping
//!
//! The UART moves bytes only when the owner calls [`AgentChannel::pump`], like the COM1 THR pump
//! of the board loop: the session calls it after every guest quantum, and the host calls of
//! [`AgentChannel::read_from_guest`] and [`AgentChannel::write_to_guest`] call it too. A session
//! should end the quantum right after a THR write ([`AgentChannel::holding_occupied`]), as for
//! COM1. The board loops of `firmware_board` do not pump this channel yet: the session that runs
//! guest code with the devices (S10-09) owns that call.
//!
//! # Locking
//!
//! The queue lock is taken before the UART lock; the bus never takes the queue lock, so a guest
//! register access cannot deadlock with a host call.

use crate::{Uart16550Handle, Uart16550PollingTx, UartPortSpec};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// Capacity of the host-to-guest queue in bytes. A hypothesis until measured (ADR-0007, the
/// measurement threshold): one maximal agent frame is 61 464 bytes, so 64 KiB holds a frame.
pub const AGENT_CHANNEL_RX_QUEUE_BYTES: usize = 65_536;
/// Capacity of the guest-to-host queue in bytes (same hypothesis).
pub const AGENT_CHANNEL_TX_QUEUE_BYTES: usize = 65_536;

/// Capacities, fill levels and counters of one channel. Totals only grow (`reset` clears the queues, not the
/// totals).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentChannelStats {
    /// Capacity of the host-to-guest queue in bytes (fixed per channel).
    pub rx_capacity: u64,
    /// Capacity of the guest-to-host queue in bytes (fixed per channel).
    pub tx_capacity: u64,
    /// Bytes waiting in the host-to-guest queue (not counting the one in the receive register).
    pub rx_queued: u64,
    /// Bytes waiting in the guest-to-host queue.
    pub tx_queued: u64,
    /// Host-to-guest bytes accepted into the queue.
    pub rx_bytes_accepted: u64,
    /// Host-to-guest bytes moved into the UART receive register.
    pub rx_bytes_delivered: u64,
    /// Host-to-guest bytes refused because the queue was full (lost, counted).
    pub rx_overflow_bytes: u64,
    /// Guest-to-host bytes accepted into the queue.
    pub tx_bytes_accepted: u64,
    /// Guest-to-host bytes the host has read.
    pub tx_bytes_read: u64,
    /// Guest-to-host bytes dropped because the queue was full (lost, counted).
    pub tx_overflow_bytes: u64,
}

#[derive(Debug)]
struct Queues {
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
    rx_capacity: usize,
    tx_capacity: usize,
    stats: AgentChannelStats,
}

/// The host handle of the agent channel. Cloning gives another handle on the same queues and
/// device; any thread may use any clone.
#[derive(Clone, Debug)]
pub struct AgentChannel {
    uart: Uart16550Handle,
    queues: Arc<Mutex<Queues>>,
}

impl AgentChannel {
    /// Creates the bus device (a COM2 UART, to bind at [`UartPortSpec::COM2`]) and its host
    /// handle, with the default queue capacities.
    pub fn new() -> (Uart16550PollingTx, Self) {
        Self::with_capacities(AGENT_CHANNEL_RX_QUEUE_BYTES, AGENT_CHANNEL_TX_QUEUE_BYTES)
    }

    /// As [`Self::new`] with explicit capacities (at least 1 byte each; 0 is raised to 1).
    pub fn with_capacities(rx_capacity: usize, tx_capacity: usize) -> (Uart16550PollingTx, Self) {
        let (device, uart) = Uart16550PollingTx::new_with_handle_at(UartPortSpec::COM2);
        let queues = Queues {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            rx_capacity: rx_capacity.max(1),
            tx_capacity: tx_capacity.max(1),
            stats: AgentChannelStats::default(),
        };
        (
            device,
            Self {
                uart,
                queues: Arc::new(Mutex::new(queues)),
            },
        )
    }

    fn queues(&self) -> MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The handle of the UART behind the channel (register-level observation).
    pub fn uart(&self) -> Uart16550Handle {
        self.uart.clone()
    }

    /// Whether the guest's THR byte waits for a pump: the caller-side STOP predicate that ends a
    /// quantum right after a THR write.
    pub fn holding_occupied(&self) -> bool {
        self.uart.holding_occupied()
    }

    /// Moves what can move: completed guest bytes into the TX queue, then queued host bytes into
    /// an empty receive register.
    pub fn pump(&self) {
        let mut queues = self.queues();
        self.pump_locked(&mut queues);
    }

    fn pump_locked(&self, queues: &mut Queues) {
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                Self::push_guest_byte(queues, byte);
            }
        }
        while let Some(&byte) = queues.rx.front() {
            match self.uart.inject_received_byte(byte) {
                Ok(()) => {
                    queues.rx.pop_front();
                    queues.stats.rx_bytes_delivered += 1;
                }
                // The receive register still holds an unread byte (Busy).
                Err(_) => break,
            }
        }
    }

    fn push_guest_byte(queues: &mut Queues, byte: u8) {
        if queues.tx.len() < queues.tx_capacity {
            queues.tx.push_back(byte);
            queues.stats.tx_bytes_accepted += 1;
        } else {
            queues.stats.tx_overflow_bytes += 1;
        }
    }

    /// Host to guest: queues as many bytes of `bytes` as fit and returns that number; the rest
    /// is refused and counted in `rx_overflow_bytes`. Never blocks.
    pub fn write_to_guest(&self, bytes: &[u8]) -> usize {
        let mut queues = self.queues();
        let room = queues.rx_capacity - queues.rx.len();
        let accepted = bytes.len().min(room);
        queues.rx.extend(&bytes[..accepted]);
        queues.stats.rx_bytes_accepted += accepted as u64;
        queues.stats.rx_overflow_bytes += (bytes.len() - accepted) as u64;
        self.pump_locked(&mut queues);
        accepted
    }

    /// Guest to host: copies up to `buf.len()` queued bytes into `buf` and returns the number.
    /// Never blocks; 0 means nothing is queued.
    pub fn read_from_guest(&self, buf: &mut [u8]) -> usize {
        let mut queues = self.queues();
        self.pump_locked(&mut queues);
        let count = buf.len().min(queues.tx.len());
        for slot in &mut buf[..count] {
            *slot = queues.tx.pop_front().unwrap_or_default();
        }
        queues.stats.tx_bytes_read += count as u64;
        count
    }

    /// TEST-ONLY. Queues bytes as if the guest had sent them (they enter the TX queue with the
    /// same overflow rule as a completed THR byte), without a CPU or register access. Returns the
    /// number that was queued; the rest was dropped and counted.
    pub fn test_inject_from_guest(&self, bytes: &[u8]) -> usize {
        let mut queues = self.queues();
        let before = queues.stats.tx_bytes_accepted;
        for &byte in bytes {
            Self::push_guest_byte(&mut queues, byte);
        }
        (queues.stats.tx_bytes_accepted - before) as usize
    }

    /// The fill levels and counters.
    pub fn stats(&self) -> AgentChannelStats {
        let queues = self.queues();
        AgentChannelStats {
            rx_capacity: queues.rx_capacity as u64,
            tx_capacity: queues.tx_capacity as u64,
            rx_queued: queues.rx.len() as u64,
            tx_queued: queues.tx.len() as u64,
            ..queues.stats
        }
    }

    /// Board reset: empties both queues (the bytes are not counted as lost) and resets the UART
    /// registers. The totals are kept.
    pub fn reset(&self) {
        let mut queues = self.queues();
        queues.rx.clear();
        queues.tx.clear();
        self.uart.reset();
    }
}

#[cfg(test)]
mod tests;
