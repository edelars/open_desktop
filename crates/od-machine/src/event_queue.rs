//! Fixed-capacity deterministic deadlines for caller-driven virtual time.

/// An event removed from an [`EventQueue`] after its deadline is due.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEvent {
    pub key: u64,
    pub deadline_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    key: u64,
    deadline_ns: u64,
    sequence: u64,
}

/// Explicit errors returned when a queue operation cannot be completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventQueueError {
    DuplicateKey,
    Full,
    NotFound,
}

/// A bounded collection of absolute deadlines in the virtual clock's ns domain.
///
/// Events are ordered by deadline and then by insertion order. Rescheduling an
/// event assigns it a new insertion order. This queue does not advance time or
/// dispatch events; its owner decides when to call [`pop_due`](Self::pop_due).
#[derive(Debug)]
pub struct EventQueue {
    entries: Vec<Entry>,
    capacity: usize,
    next_sequence: u64,
}

impl EventQueue {
    /// Create an empty queue with a fixed logical capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
            next_sequence: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a unique key and its absolute deadline.
    pub fn schedule(&mut self, key: u64, deadline_ns: u64) -> Result<(), EventQueueError> {
        if self.entries.iter().any(|entry| entry.key == key) {
            return Err(EventQueueError::DuplicateKey);
        }
        if self.entries.len() == self.capacity {
            return Err(EventQueueError::Full);
        }

        let sequence = self.take_sequence();
        self.entries.push(Entry {
            key,
            deadline_ns,
            sequence,
        });
        Ok(())
    }

    /// Remove a pending key, whether its deadline is due or still in the future.
    pub fn cancel(&mut self, key: u64) -> Result<(), EventQueueError> {
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return Err(EventQueueError::NotFound);
        };
        self.entries.remove(index);
        Ok(())
    }

    /// Change a pending deadline and assign the event a new insertion order.
    pub fn reschedule(&mut self, key: u64, deadline_ns: u64) -> Result<(), EventQueueError> {
        if !self.entries.iter().any(|entry| entry.key == key) {
            return Err(EventQueueError::NotFound);
        }

        let sequence = self.take_sequence();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.deadline_ns = deadline_ns;
            entry.sequence = sequence;
        }
        Ok(())
    }

    /// Return the earliest absolute deadline, regardless of whether it is due.
    pub fn next_deadline(&self) -> Option<u64> {
        self.earliest_index()
            .map(|index| self.entries[index].deadline_ns)
    }

    /// Remove and return at most one event whose deadline is at or before `now_ns`.
    pub fn pop_due(&mut self, now_ns: u64) -> Option<ScheduledEvent> {
        let index = self.earliest_index()?;
        let entry = self.entries[index];
        if entry.deadline_ns > now_ns {
            return None;
        }
        self.entries.remove(index);
        Some(ScheduledEvent {
            key: entry.key,
            deadline_ns: entry.deadline_ns,
        })
    }

    fn earliest_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| (entry.deadline_ns, entry.sequence))
            .map(|(index, _)| index)
    }

    fn take_sequence(&mut self) -> u64 {
        if self.next_sequence == u64::MAX {
            // Keep ordering intact without allowing an unbounded counter to wrap.
            self.entries.sort_unstable_by_key(|entry| entry.sequence);
            for (sequence, entry) in self.entries.iter_mut().enumerate() {
                entry.sequence = sequence as u64;
            }
            self.next_sequence = self.entries.len() as u64;
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(queue: &mut EventQueue, now_ns: u64) -> Vec<ScheduledEvent> {
        let mut events = Vec::new();
        while let Some(event) = queue.pop_due(now_ns) {
            events.push(event);
        }
        events
    }

    #[test]
    fn empty_full_and_duplicate_operations_preserve_contents() {
        let mut queue = EventQueue::new(1);
        assert!(queue.is_empty());
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(queue.pop_due(u64::MAX), None);

        queue.schedule(1, 5).unwrap();
        assert_eq!(queue.schedule(1, 6), Err(EventQueueError::DuplicateKey));
        assert_eq!(queue.schedule(2, 4), Err(EventQueueError::Full));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.next_deadline(), Some(5));
    }

    #[test]
    fn equal_deadlines_follow_insertion_order_and_pop_one_at_a_time() {
        let mut queue = EventQueue::new(3);
        queue.schedule(10, 7).unwrap();
        queue.schedule(20, 7).unwrap();
        queue.schedule(30, 7).unwrap();

        assert_eq!(queue.pop_due(6), None);
        for key in [10, 20, 30] {
            assert_eq!(queue.pop_due(7).unwrap().key, key);
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn exact_boundary_past_deadline_and_due_cancellation() {
        let mut queue = EventQueue::new(3);
        queue.schedule(1, 10).unwrap();
        queue.schedule(2, 4).unwrap();
        queue.schedule(3, 8).unwrap();

        assert_eq!(queue.cancel(2), Ok(()));
        assert_eq!(queue.pop_due(7), None);
        assert_eq!(queue.pop_due(8).unwrap().key, 3);
        assert_eq!(queue.pop_due(10).unwrap().key, 1);

        queue.schedule(4, 2).unwrap();
        assert_eq!(queue.pop_due(3).unwrap().key, 4);
    }

    #[test]
    fn reschedule_changes_order_for_earlier_later_and_equal_deadlines() {
        let mut queue = EventQueue::new(4);
        queue.schedule(1, 10).unwrap();
        queue.schedule(2, 10).unwrap();
        queue.schedule(3, 20).unwrap();

        queue.reschedule(3, 5).unwrap();
        queue.reschedule(1, 20).unwrap();
        queue.reschedule(2, 20).unwrap();
        assert_eq!(
            drain(&mut queue, 20)
                .into_iter()
                .map(|event| event.key)
                .collect::<Vec<_>>(),
            [3, 1, 2]
        );

        queue.schedule(4, 30).unwrap();
        queue.reschedule(4, 30).unwrap();
        assert_eq!(queue.pop_due(30).unwrap().key, 4);

        let mut equal = EventQueue::new(2);
        equal.schedule(1, 7).unwrap();
        equal.schedule(2, 7).unwrap();
        equal.reschedule(1, 7).unwrap();
        assert_eq!(equal.pop_due(7).unwrap().key, 2);
        assert_eq!(equal.pop_due(7).unwrap().key, 1);
    }

    #[test]
    fn sequence_rebases_before_counter_wrap() {
        let mut queue = EventQueue::new(2);
        queue.next_sequence = u64::MAX;
        queue.schedule(1, 5).unwrap();
        queue.schedule(2, 5).unwrap();
        assert_eq!(queue.pop_due(5).unwrap().key, 1);
        assert_eq!(queue.pop_due(5).unwrap().key, 2);
    }

    #[test]
    fn failed_operations_leave_queue_unchanged() {
        let mut queue = EventQueue::new(1);
        queue.schedule(1, 9).unwrap();
        let before = (queue.len(), queue.next_deadline());

        assert_eq!(queue.cancel(2), Err(EventQueueError::NotFound));
        assert_eq!(queue.reschedule(2, 1), Err(EventQueueError::NotFound));
        assert_eq!(queue.schedule(2, 1), Err(EventQueueError::Full));
        assert_eq!(queue.schedule(1, 1), Err(EventQueueError::DuplicateKey));
        assert_eq!((queue.len(), queue.next_deadline()), before);
        assert_eq!(queue.pop_due(8), None);
        assert_eq!(queue.pop_due(9).unwrap().key, 1);
    }

    #[test]
    fn deadline_order_handles_u64_extremes() {
        let mut queue = EventQueue::new(3);
        queue.schedule(1, u64::MAX).unwrap();
        queue.schedule(2, 0).unwrap();
        queue.schedule(3, u64::MAX - 1).unwrap();

        assert_eq!(queue.next_deadline(), Some(0));
        assert_eq!(queue.pop_due(0).unwrap().key, 2);
        assert_eq!(queue.pop_due(u64::MAX - 2), None);
        assert_eq!(queue.pop_due(u64::MAX - 1).unwrap().key, 3);
        assert_eq!(queue.pop_due(u64::MAX).unwrap().key, 1);
    }

    #[test]
    fn repeated_fill_and_drain_keeps_allocated_storage_fixed() {
        let mut queue = EventQueue::new(8);
        let storage_capacity = queue.entries.capacity();
        for _ in 0..100 {
            for key in 0..8 {
                queue.schedule(key, key).unwrap();
            }
            assert_eq!(queue.entries.capacity(), storage_capacity);
            assert_eq!(drain(&mut queue, u64::MAX).len(), 8);
            assert_eq!(queue.entries.capacity(), storage_capacity);
        }
    }
}
