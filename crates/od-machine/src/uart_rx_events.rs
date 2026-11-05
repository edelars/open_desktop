//! Bounded caller-driven delivery of scheduled single-byte UART RX events.

use crate::{DeviceError, EventGeneration, EventQueue, EventQueueError, Uart16550Handle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UartRxDispatchExit {
    Stopped,
    BudgetExhausted,
    NoEvent,
    DeadlinePending,
    ReceiverBusy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UartRxDispatchReport {
    pub delivered: usize,
    pub exit: UartRxDispatchExit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UartRxPumpError {
    Queue(EventQueueError),
    Device(DeviceError),
    QueueInvariant,
}

impl From<EventQueueError> for UartRxPumpError {
    fn from(error: EventQueueError) -> Self {
        Self::Queue(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Payload {
    key: u64,
    value: u8,
}

/// Synchronously dispatches a fixed number of scheduled bytes to one UART RX
/// register. Event ordering and lifecycle are owned by [`EventQueue`].
#[derive(Debug)]
pub struct UartRxEventPump {
    events: EventQueue,
    payloads: Vec<Payload>,
    uart: Uart16550Handle,
}

impl UartRxEventPump {
    pub fn new(capacity: usize, uart: Uart16550Handle) -> Self {
        Self {
            events: EventQueue::new(capacity),
            payloads: Vec::with_capacity(capacity),
            uart,
        }
    }

    pub const fn capacity(&self) -> usize {
        self.events.capacity()
    }

    pub const fn generation(&self) -> EventGeneration {
        self.events.generation()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.events.next_deadline()
    }

    pub fn schedule(
        &mut self,
        generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
        value: u8,
    ) -> Result<(), EventQueueError> {
        self.events.schedule(generation, key, deadline_ns)?;
        // Capacity is reserved at construction and EventQueue admits no more
        // than this same logical capacity.
        self.payloads.push(Payload { key, value });
        Ok(())
    }

    pub fn cancel(&mut self, generation: EventGeneration, key: u64) -> Result<(), EventQueueError> {
        let Some(index) = self.payloads.iter().position(|payload| payload.key == key) else {
            self.events.cancel(generation, key)?;
            return Err(EventQueueError::NotFound);
        };
        self.events.cancel(generation, key)?;
        self.payloads.remove(index);
        Ok(())
    }

    pub fn reschedule(
        &mut self,
        generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
    ) -> Result<(), EventQueueError> {
        self.events.reschedule(generation, key, deadline_ns)
    }

    /// Reset pending deliveries and invalidate old generation tokens. UART RX
    /// state and caller-owned virtual time are not touched.
    pub fn reset(
        &mut self,
        generation: EventGeneration,
    ) -> Result<EventGeneration, EventQueueError> {
        let next = self.events.reset(generation)?;
        self.payloads.clear();
        Ok(next)
    }

    /// Deliver ready events in deadline/insertion order until STOP, the finite
    /// event budget, the next future deadline, an empty queue, or a full RX
    /// register is reached. STOP precedes budget, and budget precedes readiness.
    pub fn dispatch_due(
        &mut self,
        expected_generation: EventGeneration,
        now_ns: u64,
        budget: usize,
        mut stop: impl FnMut() -> bool,
    ) -> Result<UartRxDispatchReport, UartRxPumpError> {
        self.events.validate_generation(expected_generation)?;
        let mut delivered = 0;
        loop {
            if stop() {
                return Ok(UartRxDispatchReport {
                    delivered,
                    exit: UartRxDispatchExit::Stopped,
                });
            }
            if delivered >= budget {
                return Ok(UartRxDispatchReport {
                    delivered,
                    exit: UartRxDispatchExit::BudgetExhausted,
                });
            }

            let Some(event) = self.events.peek_due(expected_generation, now_ns)? else {
                let exit = if self.events.is_empty() {
                    UartRxDispatchExit::NoEvent
                } else {
                    UartRxDispatchExit::DeadlinePending
                };
                return Ok(UartRxDispatchReport { delivered, exit });
            };
            let payload = self
                .payloads
                .iter()
                .find(|payload| payload.key == event.key)
                .copied()
                .ok_or(UartRxPumpError::QueueInvariant)?;

            match self.uart.inject_received_byte(payload.value) {
                Ok(()) => {
                    let removed = self.events.remove_due(expected_generation, now_ns)?;
                    if removed != Some(event) || self.remove_payload(event.key).is_none() {
                        return Err(UartRxPumpError::QueueInvariant);
                    }
                    delivered += 1;
                }
                Err(DeviceError::Busy) => {
                    return Ok(UartRxDispatchReport {
                        delivered,
                        exit: UartRxDispatchExit::ReceiverBusy,
                    });
                }
                Err(error) => return Err(UartRxPumpError::Device(error)),
            }
        }
    }

    fn remove_payload(&mut self, key: u64) -> Option<Payload> {
        let index = self
            .payloads
            .iter()
            .position(|payload| payload.key == key)?;
        Some(self.payloads.remove(index))
    }
}
