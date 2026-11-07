//! Fixed-capacity deterministic deadlines for caller-driven virtual time.

/// Opaque identity for one reset generation of an [`EventQueue`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventGeneration(u64);

/// An event removed from an [`EventQueue`] after its deadline is due.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEvent {
    pub generation: EventGeneration,
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
    StaleGeneration,
    GenerationExhausted,
}

/// Errors specific to the private single-slot operation.
#[allow(dead_code)] // The timer owner will use this operation in a later slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SingleSlotError {
    Queue(EventQueueError),
    WrongCapacity,
}

/// A bounded collection of absolute deadlines in the virtual clock's ns domain.
///
/// Events are ordered by deadline and then by insertion order. Rescheduling an
/// event assigns it a new insertion order. This queue does not advance time or
/// dispatch events; its owner decides when to call [`pop_due`](Self::pop_due).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EventQueue {
    entries: Vec<Entry>,
    capacity: usize,
    next_sequence: u64,
    generation: EventGeneration,
}

impl EventQueue {
    #[cfg(test)]
    pub(crate) fn exhaust_generation_for_test(&mut self) {
        self.generation = EventGeneration(u64::MAX);
    }
    /// Create an empty queue with a fixed logical capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
            next_sequence: 0,
            generation: EventGeneration(0),
        }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Return the token required to operate on the current queue generation.
    pub const fn generation(&self) -> EventGeneration {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a unique key and its absolute deadline.
    pub fn schedule(
        &mut self,
        generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
    ) -> Result<(), EventQueueError> {
        self.validate_generation(generation)?;
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
    pub fn cancel(&mut self, generation: EventGeneration, key: u64) -> Result<(), EventQueueError> {
        self.validate_generation(generation)?;
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return Err(EventQueueError::NotFound);
        };
        self.entries.remove(index);
        Ok(())
    }

    /// Change a pending deadline and assign the event a new insertion order.
    pub fn reschedule(
        &mut self,
        generation: EventGeneration,
        key: u64,
        deadline_ns: u64,
    ) -> Result<(), EventQueueError> {
        self.validate_generation(generation)?;
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
    pub fn pop_due(
        &mut self,
        generation: EventGeneration,
        now_ns: u64,
    ) -> Result<Option<ScheduledEvent>, EventQueueError> {
        self.validate_generation(generation)?;
        self.remove_due(generation, now_ns)
    }

    /// Inspect the earliest due event without changing its order or contents.
    pub(crate) fn peek_due(
        &self,
        generation: EventGeneration,
        now_ns: u64,
    ) -> Result<Option<ScheduledEvent>, EventQueueError> {
        self.validate_generation(generation)?;
        let Some(index) = self.earliest_index() else {
            return Ok(None);
        };
        let entry = self.entries[index];
        if entry.deadline_ns > now_ns {
            return Ok(None);
        }
        Ok(Some(ScheduledEvent {
            generation: self.generation,
            key: entry.key,
            deadline_ns: entry.deadline_ns,
        }))
    }

    /// Remove the earliest event when due. The owner may inspect first and call
    /// this only after its side effect succeeds, preserving the event on retry.
    pub(crate) fn remove_due(
        &mut self,
        generation: EventGeneration,
        now_ns: u64,
    ) -> Result<Option<ScheduledEvent>, EventQueueError> {
        self.validate_generation(generation)?;
        let Some(index) = self.earliest_index() else {
            return Ok(None);
        };
        let entry = self.entries[index];
        if entry.deadline_ns > now_ns {
            return Ok(None);
        }
        self.entries.remove(index);
        Ok(Some(ScheduledEvent {
            generation: self.generation,
            key: entry.key,
            deadline_ns: entry.deadline_ns,
        }))
    }

    /// Reject a token retained across a reset.
    pub fn validate_generation(&self, generation: EventGeneration) -> Result<(), EventQueueError> {
        if generation == self.generation {
            Ok(())
        } else {
            Err(EventQueueError::StaleGeneration)
        }
    }

    /// Clear all pending events and invalidate every token and extracted event.
    pub fn reset(
        &mut self,
        generation: EventGeneration,
    ) -> Result<EventGeneration, EventQueueError> {
        self.validate_generation(generation)?;
        let next = self
            .generation
            .0
            .checked_add(1)
            .ok_or(EventQueueError::GenerationExhausted)?;

        self.entries.clear();
        self.next_sequence = 0;
        self.generation = EventGeneration(next);
        Ok(self.generation)
    }

    /// Replace a dedicated one-entry slot and invalidate all prior events.
    /// The slot uses key zero; its generation identifies each incarnation.
    #[allow(dead_code)] // Awaiting the single-slot timer owner.
    pub(crate) fn replace_single(
        &mut self,
        generation: EventGeneration,
        deadline_ns: Option<u64>,
    ) -> Result<EventGeneration, SingleSlotError> {
        self.validate_generation(generation)
            .map_err(SingleSlotError::Queue)?;
        if self.capacity != 1 {
            return Err(SingleSlotError::WrongCapacity);
        }
        let next = self
            .generation
            .0
            .checked_add(1)
            .ok_or(SingleSlotError::Queue(EventQueueError::GenerationExhausted))?;

        self.entries.clear();
        self.next_sequence = 0;
        self.generation = EventGeneration(next);
        if let Some(deadline_ns) = deadline_ns {
            self.entries.push(Entry {
                key: 0,
                deadline_ns,
                sequence: 0,
            });
            self.next_sequence = 1;
        }
        Ok(self.generation)
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

    #[test]
    fn single_slot_replaces_full_and_invalidates_extracted_events() {
        let mut queue = EventQueue::new(1);
        let initial = queue.generation();
        queue.schedule(initial, 42, 10).unwrap();
        let next = queue.replace_single(initial, Some(20)).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.next_deadline(), Some(20));
        assert_eq!(
            queue.validate_generation(initial),
            Err(EventQueueError::StaleGeneration)
        );
        let event = queue.pop_due(next, 20).unwrap().unwrap();
        assert_eq!(
            (event.key, event.deadline_ns, event.generation),
            (0, 20, next)
        );
        let rearmed = queue.replace_single(next, Some(30)).unwrap();
        assert_eq!(
            queue.validate_generation(event.generation),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(queue.pop_due(rearmed, 30).unwrap().unwrap().deadline_ns, 30);
    }

    #[test]
    fn single_slot_cancel_invalidates_pending_and_extracted_events() {
        let mut queue = EventQueue::new(1);
        let start = queue.generation();
        let armed = queue.replace_single(start, Some(10)).unwrap();
        let canceled = queue.replace_single(armed, None).unwrap();
        assert!(queue.is_empty());
        assert_eq!(
            queue.validate_generation(armed),
            Err(EventQueueError::StaleGeneration)
        );
        let armed_again = queue.replace_single(canceled, Some(11)).unwrap();
        let extracted = queue.pop_due(armed_again, 11).unwrap().unwrap();
        let canceled_again = queue.replace_single(armed_again, None).unwrap();
        assert_eq!(
            queue.validate_generation(extracted.generation),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(queue.pop_due(canceled_again, u64::MAX), Ok(None));
    }

    #[test]
    fn single_slot_errors_leave_queue_and_sequence_unchanged() {
        for capacity in [0, 2] {
            let mut queue = EventQueue::new(capacity);
            let generation = queue.generation();
            if capacity == 2 {
                queue.schedule(generation, 9, 12).unwrap();
            }
            let before = (queue.entries.clone(), queue.next_sequence, queue.generation);
            assert_eq!(
                queue.replace_single(EventGeneration(99), Some(1)),
                Err(SingleSlotError::Queue(EventQueueError::StaleGeneration))
            );
            assert_eq!(
                queue.replace_single(generation, Some(1)),
                Err(SingleSlotError::WrongCapacity)
            );
            assert_eq!(
                (queue.entries.clone(), queue.next_sequence, queue.generation),
                before
            );
        }
        let mut queue = EventQueue::new(1);
        queue.generation = EventGeneration(u64::MAX);
        let generation = queue.generation();
        queue.schedule(generation, 7, 15).unwrap();
        let before = (queue.entries.clone(), queue.next_sequence, queue.generation);
        for deadline in [None, Some(1)] {
            assert_eq!(
                queue.replace_single(generation, deadline),
                Err(SingleSlotError::Queue(EventQueueError::GenerationExhausted))
            );
            assert_eq!(
                (queue.entries.clone(), queue.next_sequence, queue.generation),
                before
            );
        }
        assert_eq!(queue.pop_due(generation, 15).unwrap().unwrap().key, 7);
    }

    fn schedule(queue: &mut EventQueue, key: u64, deadline_ns: u64) -> Result<(), EventQueueError> {
        let generation = queue.generation();
        queue.schedule(generation, key, deadline_ns)
    }

    fn cancel(queue: &mut EventQueue, key: u64) -> Result<(), EventQueueError> {
        let generation = queue.generation();
        queue.cancel(generation, key)
    }

    fn reschedule(
        queue: &mut EventQueue,
        key: u64,
        deadline_ns: u64,
    ) -> Result<(), EventQueueError> {
        let generation = queue.generation();
        queue.reschedule(generation, key, deadline_ns)
    }

    fn pop_due(queue: &mut EventQueue, now_ns: u64) -> Option<ScheduledEvent> {
        let generation = queue.generation();
        queue.pop_due(generation, now_ns).unwrap()
    }

    fn drain(queue: &mut EventQueue, now_ns: u64) -> Vec<ScheduledEvent> {
        let mut events = Vec::new();
        while let Some(event) = pop_due(queue, now_ns) {
            events.push(event);
        }
        events
    }

    #[test]
    fn empty_full_and_duplicate_operations_preserve_contents() {
        let mut queue = EventQueue::new(1);
        assert!(queue.is_empty());
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(pop_due(&mut queue, u64::MAX), None);

        schedule(&mut queue, 1, 5).unwrap();
        assert_eq!(
            schedule(&mut queue, 1, 6),
            Err(EventQueueError::DuplicateKey)
        );
        assert_eq!(schedule(&mut queue, 2, 4), Err(EventQueueError::Full));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.next_deadline(), Some(5));
    }

    #[test]
    fn equal_deadlines_follow_insertion_order_and_pop_one_at_a_time() {
        let mut queue = EventQueue::new(3);
        schedule(&mut queue, 10, 7).unwrap();
        schedule(&mut queue, 20, 7).unwrap();
        schedule(&mut queue, 30, 7).unwrap();

        assert_eq!(pop_due(&mut queue, 6), None);
        for key in [10, 20, 30] {
            assert_eq!(pop_due(&mut queue, 7).unwrap().key, key);
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn exact_boundary_past_deadline_and_due_cancellation() {
        let mut queue = EventQueue::new(3);
        schedule(&mut queue, 1, 10).unwrap();
        schedule(&mut queue, 2, 4).unwrap();
        schedule(&mut queue, 3, 8).unwrap();

        assert_eq!(cancel(&mut queue, 2), Ok(()));
        assert_eq!(pop_due(&mut queue, 7), None);
        assert_eq!(pop_due(&mut queue, 8).unwrap().key, 3);
        assert_eq!(pop_due(&mut queue, 10).unwrap().key, 1);

        schedule(&mut queue, 4, 2).unwrap();
        assert_eq!(pop_due(&mut queue, 3).unwrap().key, 4);
    }

    #[test]
    fn reschedule_changes_order_for_earlier_later_and_equal_deadlines() {
        let mut queue = EventQueue::new(4);
        schedule(&mut queue, 1, 10).unwrap();
        schedule(&mut queue, 2, 10).unwrap();
        schedule(&mut queue, 3, 20).unwrap();

        reschedule(&mut queue, 3, 5).unwrap();
        reschedule(&mut queue, 1, 20).unwrap();
        reschedule(&mut queue, 2, 20).unwrap();
        assert_eq!(
            drain(&mut queue, 20)
                .into_iter()
                .map(|event| event.key)
                .collect::<Vec<_>>(),
            [3, 1, 2]
        );

        schedule(&mut queue, 4, 30).unwrap();
        reschedule(&mut queue, 4, 30).unwrap();
        assert_eq!(pop_due(&mut queue, 30).unwrap().key, 4);

        let mut equal = EventQueue::new(2);
        schedule(&mut equal, 1, 7).unwrap();
        schedule(&mut equal, 2, 7).unwrap();
        reschedule(&mut equal, 1, 7).unwrap();
        assert_eq!(pop_due(&mut equal, 7).unwrap().key, 2);
        assert_eq!(pop_due(&mut equal, 7).unwrap().key, 1);
    }

    #[test]
    fn sequence_rebases_before_counter_wrap() {
        let mut queue = EventQueue::new(2);
        queue.next_sequence = u64::MAX;
        schedule(&mut queue, 1, 5).unwrap();
        schedule(&mut queue, 2, 5).unwrap();
        assert_eq!(pop_due(&mut queue, 5).unwrap().key, 1);
        assert_eq!(pop_due(&mut queue, 5).unwrap().key, 2);
    }

    #[test]
    fn failed_operations_leave_queue_unchanged() {
        let mut queue = EventQueue::new(1);
        schedule(&mut queue, 1, 9).unwrap();
        let before = (queue.len(), queue.next_deadline());

        assert_eq!(cancel(&mut queue, 2), Err(EventQueueError::NotFound));
        assert_eq!(reschedule(&mut queue, 2, 1), Err(EventQueueError::NotFound));
        assert_eq!(schedule(&mut queue, 2, 1), Err(EventQueueError::Full));
        assert_eq!(
            schedule(&mut queue, 1, 1),
            Err(EventQueueError::DuplicateKey)
        );
        assert_eq!((queue.len(), queue.next_deadline()), before);
        assert_eq!(pop_due(&mut queue, 8), None);
        assert_eq!(pop_due(&mut queue, 9).unwrap().key, 1);
    }

    #[test]
    fn deadline_order_handles_u64_extremes() {
        let mut queue = EventQueue::new(3);
        schedule(&mut queue, 1, u64::MAX).unwrap();
        schedule(&mut queue, 2, 0).unwrap();
        schedule(&mut queue, 3, u64::MAX - 1).unwrap();

        assert_eq!(queue.next_deadline(), Some(0));
        assert_eq!(pop_due(&mut queue, 0).unwrap().key, 2);
        assert_eq!(pop_due(&mut queue, u64::MAX - 2), None);
        assert_eq!(pop_due(&mut queue, u64::MAX - 1).unwrap().key, 3);
        assert_eq!(pop_due(&mut queue, u64::MAX).unwrap().key, 1);
    }

    #[test]
    fn repeated_fill_and_drain_keeps_allocated_storage_fixed() {
        let mut queue = EventQueue::new(8);
        let storage_capacity = queue.entries.capacity();
        for _ in 0..100 {
            for key in 0..8 {
                schedule(&mut queue, key, key).unwrap();
            }
            assert_eq!(queue.entries.capacity(), storage_capacity);
            assert_eq!(drain(&mut queue, u64::MAX).len(), 8);
            assert_eq!(queue.entries.capacity(), storage_capacity);
        }
    }

    #[test]
    fn reset_empty_and_full_queue_preserves_capacity_and_restarts_sequence() {
        let mut queue = EventQueue::new(2);
        let storage_capacity = queue.entries.capacity();
        let initial = queue.generation();
        let after_empty_reset = queue.reset(initial).unwrap();
        assert_ne!(after_empty_reset, initial);
        assert_eq!(queue.entries.capacity(), storage_capacity);

        schedule(&mut queue, 1, 5).unwrap();
        schedule(&mut queue, 2, 5).unwrap();
        assert_eq!(queue.next_sequence, 2);
        let current = queue.generation();
        let after_full_reset = queue.reset(current).unwrap();
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.next_sequence, 0);
        assert_eq!(queue.entries.capacity(), storage_capacity);
        schedule(&mut queue, 2, 5).unwrap();
        schedule(&mut queue, 1, 5).unwrap();
        assert_eq!(queue.next_sequence, 2);
        assert_eq!(pop_due(&mut queue, 5).unwrap().generation, after_full_reset);
        assert_eq!(pop_due(&mut queue, 5).unwrap().key, 1);
    }

    #[test]
    fn stale_operations_are_rejected_before_key_or_capacity_checks() {
        let mut queue = EventQueue::new(1);
        let stale = queue.generation();
        schedule(&mut queue, 1, 10).unwrap();
        let previous = queue.generation();
        let current = queue.reset(previous).unwrap();
        schedule(&mut queue, 1, 10).unwrap();
        let before = (
            queue.len(),
            queue.next_deadline(),
            queue.next_sequence,
            queue.generation(),
        );

        assert_eq!(
            queue.schedule(stale, 1, 0),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.schedule(stale, 2, 0),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.cancel(stale, 1),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.cancel(stale, 99),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.reschedule(stale, 1, 0),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.reschedule(stale, 99, 0),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            queue.pop_due(stale, u64::MAX),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(queue.reset(stale), Err(EventQueueError::StaleGeneration));
        assert_eq!(
            queue.validate_generation(stale),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(
            (
                queue.len(),
                queue.next_deadline(),
                queue.next_sequence,
                queue.generation()
            ),
            before
        );
        assert_eq!(queue.generation(), current);
        assert_eq!(pop_due(&mut queue, 10).unwrap().key, 1);
    }

    #[test]
    fn reset_invalidates_extracted_event_even_when_queue_was_empty() {
        let mut queue = EventQueue::new(1);
        let old = queue.generation();
        schedule(&mut queue, 5, 1).unwrap();
        let event = pop_due(&mut queue, 1).unwrap();
        assert!(queue.is_empty());

        let new = queue.reset(old).unwrap();
        assert_eq!(
            queue.validate_generation(event.generation),
            Err(EventQueueError::StaleGeneration)
        );
        assert_eq!(queue.validate_generation(new), Ok(()));
    }

    #[test]
    fn generation_exhaustion_is_atomic_and_does_not_reuse_tokens() {
        let mut queue = EventQueue::new(2);
        queue.generation = EventGeneration(u64::MAX);
        queue.next_sequence = 17;
        schedule(&mut queue, 1, 3).unwrap();
        let generation = queue.generation();
        let before_entries = queue.entries.clone();
        let before_capacity = queue.entries.capacity();
        assert_eq!(
            queue.reset(generation),
            Err(EventQueueError::GenerationExhausted)
        );
        assert_eq!(queue.generation(), generation);
        assert_eq!(queue.entries, before_entries);
        assert_eq!(queue.entries.capacity(), before_capacity);
        assert_eq!(queue.next_sequence, 18);
        assert_eq!(pop_due(&mut queue, 3).unwrap().key, 1);
    }

    #[test]
    fn generation_is_independent_from_sequence_rebasing() {
        let mut queue = EventQueue::new(3);
        let generation = queue.generation();
        schedule(&mut queue, 1, 7).unwrap();
        schedule(&mut queue, 2, 7).unwrap();
        queue.next_sequence = u64::MAX;
        schedule(&mut queue, 3, 7).unwrap();
        assert_eq!(queue.generation(), generation);
        assert_eq!(
            drain(&mut queue, 7)
                .into_iter()
                .map(|event| event.key)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }
}
