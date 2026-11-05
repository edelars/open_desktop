use od_machine::{EventQueue, ScheduledEvent, VirtualClock};

#[test]
fn deadlines_are_ready_against_virtual_clock_without_advancing_it() {
    let mut clock = VirtualClock::new(1_000_000_000).unwrap();
    let mut queue = EventQueue::new(2);
    queue.schedule(7, 5).unwrap();

    assert_eq!(clock.now_ns(), 0);
    assert_eq!(queue.next_deadline(), Some(5));
    assert_eq!(queue.pop_due(clock.now_ns()), None);
    assert_eq!(clock.now_ns(), 0);

    clock.advance_time_ns(5).unwrap();
    assert_eq!(
        queue.pop_due(clock.now_ns()),
        Some(ScheduledEvent {
            key: 7,
            deadline_ns: 5,
        })
    );
    assert_eq!(clock.now_ns(), 5);
    assert_eq!(queue.next_deadline(), None);
}
