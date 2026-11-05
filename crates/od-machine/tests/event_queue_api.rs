use od_machine::{EventQueue, EventQueueError, ScheduledEvent, VirtualClock};

#[test]
fn deadlines_are_ready_against_virtual_clock_without_advancing_it() {
    let mut clock = VirtualClock::new(1_000_000_000).unwrap();
    let mut queue = EventQueue::new(2);
    let generation = queue.generation();
    queue.schedule(generation, 7, 5).unwrap();

    assert_eq!(clock.now_ns(), 0);
    assert_eq!(queue.next_deadline(), Some(5));
    assert_eq!(queue.pop_due(generation, clock.now_ns()), Ok(None));
    assert_eq!(clock.now_ns(), 0);

    clock.advance_time_ns(5).unwrap();
    assert_eq!(
        queue.pop_due(generation, clock.now_ns()),
        Ok(Some(ScheduledEvent {
            generation,
            key: 7,
            deadline_ns: 5,
        }))
    );
    assert_eq!(clock.now_ns(), 5);
    assert_eq!(queue.next_deadline(), None);
}

#[test]
fn reset_invalidates_old_token_and_extracted_events_without_advancing_clock() {
    let clock = VirtualClock::new(1_000_000_000).unwrap();
    let mut queue = EventQueue::new(2);
    let old = queue.generation();
    queue.schedule(old, 7, 0).unwrap();
    queue.schedule(old, 8, 100).unwrap();
    let extracted = queue.pop_due(old, 0).unwrap().unwrap();

    let new = queue.reset(old).unwrap();
    assert_ne!(new, old);
    assert_eq!(queue.capacity(), 2);
    assert_eq!(queue.len(), 0);
    assert_eq!(clock.now_ns(), 0);
    assert_eq!(
        queue.validate_generation(extracted.generation),
        Err(EventQueueError::StaleGeneration)
    );
    assert_eq!(
        queue.schedule(old, 7, 1),
        Err(EventQueueError::StaleGeneration)
    );
    assert_eq!(queue.cancel(old, 8), Err(EventQueueError::StaleGeneration));
    assert_eq!(
        queue.reschedule(old, 8, 1),
        Err(EventQueueError::StaleGeneration)
    );
    assert_eq!(
        queue.pop_due(old, u64::MAX),
        Err(EventQueueError::StaleGeneration)
    );
    assert_eq!(queue.reset(old), Err(EventQueueError::StaleGeneration));

    queue.schedule(new, 7, 2).unwrap();
    assert_eq!(queue.pop_due(new, 2).unwrap().unwrap().key, 7);
    assert_eq!(clock.now_ns(), 0);
}
