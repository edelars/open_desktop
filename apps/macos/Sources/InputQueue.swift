import Foundation

// Bounded input queue of INPUT_TRANSPORT_V1.md section 7 (task S10-16). The UI queue (between AppKit and the socket)
// and the VMHost queue (between the socket and the future `od-host` input boundary) are this same type: 256 records of
// 64 bytes accounted, storage allocated once and never grown.
//
// Notation (7.2): free = free records, h = usages whose DOWN is queued and UP is not, m = 0 when the tail is a MOTION
// and 1 otherwise (also for an empty queue), `suppressed` = usages whose DOWN was refused or dropped.
// Invariant in FOCUSED: free >= 2h + m + 1. The rules of 7.3 and 7.4 keep it; `invariantHolds` lets tests check it.

enum InputRecord: Equatable {
    case focus(gained: Bool)
    case usage(InputUsage)
    case motion(InputMotion)

    var message: InputMessage {
        switch self {
        case let .focus(gained): return .focus(gained: gained)
        case let .usage(usage): return .usage(usage)
        case let .motion(motion): return .motion(motion)
        }
    }
}

struct InputQueue {
    static let capacity = InputProtocol.queueCapacity

    enum State: Equatable {
        case unfocused
        case focused
        case gainedPending
    }

    /// What happened to one input. `violation` is a protocol error on the VMHost side and a programming error on the UI
    /// side (the UI filters such events before it calls the queue).
    enum Outcome: Equatable {
        case queued
        case merged
        /// `DOWN` refused (7.3): counted, the usage is in `suppressed`.
        case rejectedDown
        /// Dropped in GAINED_PENDING (7.4): counted.
        case droppedPending
        /// `UP` of a suppressed usage: removed from `suppressed`, nothing queued.
        case releasedSuppressed
        /// `FOCUS GAINED` could not be queued (free < 3): GAINED_PENDING.
        case focusPending
        /// `FOCUS LOST` in GAINED_PENDING: only cancels the wait.
        case cancelledPending
        case violation(String)
    }

    private var ring: [InputRecord?]
    private var head = 0
    private(set) var count = 0
    private(set) var state = State.unfocused
    private var pressed = [Bool](repeating: false, count: InputUsage.slotCount)
    private var suppressed = [Bool](repeating: false, count: InputUsage.slotCount)
    /// h of section 7.2.
    private(set) var pressedCount = 0
    private(set) var rejectedDowns: UInt64 = 0
    private(set) var mergedMotions: UInt64 = 0
    private(set) var droppedUnfocused: UInt64 = 0
    /// MOTION records dropped at the queue exit because their only content was a stale absolute position (8.2 item 4).
    private(set) var staleMotionsDropped: UInt64 = 0

    init() {
        ring = [InputRecord?](repeating: nil, count: InputQueue.capacity)
    }

    var free: Int { InputQueue.capacity - count }
    var isEmpty: Bool { count == 0 }
    /// Allocated storage in records; constant for the life of the queue.
    var storageRecordCount: Int { ring.count }
    var suppressedCount: Int { suppressed.reduce(0) { $0 + ($1 ? 1 : 0) } }

    func isPressed(page: UInt16, id: UInt16) -> Bool {
        InputUsage.slot(page: page, id: id).map { pressed[$0] } ?? false
    }

    func isSuppressed(page: UInt16, id: UInt16) -> Bool {
        InputUsage.slot(page: page, id: id).map { suppressed[$0] } ?? false
    }

    /// `m` of section 7.2.
    var tailMotionTerm: Int {
        if case .motion? = tail { return 0 }
        return 1
    }

    var invariantHolds: Bool {
        state != .focused || free >= 2 * pressedCount + tailMotionTerm + 1
    }

    private var tail: InputRecord? {
        count == 0 ? nil : ring[(head + count - 1) % InputQueue.capacity]
    }

    var peekTail: InputRecord? { tail }
    var peekHead: InputRecord? { count == 0 ? nil : ring[head] }

    /// All queued records, head first (tests and tracing).
    var records: [InputRecord] {
        (0..<count).compactMap { ring[(head + $0) % InputQueue.capacity] }
    }

    @discardableResult
    private mutating func append(_ record: InputRecord) -> Bool {
        guard count < InputQueue.capacity else { return false }
        ring[(head + count) % InputQueue.capacity] = record
        count += 1
        return true
    }

    private mutating func clearUsageState() {
        pressed = [Bool](repeating: false, count: InputUsage.slotCount)
        suppressed = [Bool](repeating: false, count: InputUsage.slotCount)
        pressedCount = 0
    }

    // MARK: Rules 7.3

    mutating func focus(gained: Bool) -> Outcome {
        if gained {
            guard state == .unfocused else { return .violation("repeated FOCUS GAINED") }
            if free >= 3 {
                append(.focus(gained: true))
                state = .focused
                return .queued
            }
            state = .gainedPending
            return .focusPending
        }
        switch state {
        case .unfocused:
            return .violation("repeated FOCUS LOST")
        case .gainedPending:
            clearUsageState()
            state = .unfocused
            return .cancelledPending
        case .focused:
            guard append(.focus(gained: false)) else { return .violation("queue invariant: no room for FOCUS LOST") }
            clearUsageState()
            state = .unfocused
            return .queued
        }
    }

    mutating func usage(page: UInt16, id: UInt16, down: Bool) -> Outcome {
        guard let slot = InputUsage.slot(page: page, id: id) else { return .violation("usage \(page):\(id) is not allowed") }
        switch state {
        case .unfocused:
            return .violation("USAGE while unfocused")
        case .gainedPending:
            if down {
                guard !suppressed[slot], !pressed[slot] else { return .violation("DOWN for a pressed or suppressed usage") }
                suppressed[slot] = true
                droppedUnfocused += 1
                return .droppedPending
            }
            guard suppressed[slot] else { return .violation("UP for a usage that is not pressed") }
            suppressed[slot] = false
            return .releasedSuppressed
        case .focused:
            if down {
                guard !pressed[slot], !suppressed[slot] else { return .violation("DOWN for a pressed or suppressed usage") }
                if free >= 2 * pressedCount + 5, pressedCount < 126 {
                    append(.usage(InputUsage(page: page, id: id, down: true)))
                    pressed[slot] = true
                    pressedCount += 1
                    return .queued
                }
                suppressed[slot] = true
                rejectedDowns += 1
                return .rejectedDown
            }
            if suppressed[slot] {
                suppressed[slot] = false
                return .releasedSuppressed
            }
            guard pressed[slot] else { return .violation("UP for a usage that is not pressed") }
            guard append(.usage(InputUsage(page: page, id: id, down: false))) else {
                return .violation("queue invariant: no room for UP")
            }
            pressed[slot] = false
            pressedCount -= 1
            return .queued
        }
    }

    /// `motion` must already satisfy `InputMotion.validate()` (the decoder and the UI producer guarantee that).
    mutating func motion(_ motion: InputMotion) -> Outcome {
        switch state {
        case .unfocused:
            return .violation("MOTION while unfocused")
        case .gainedPending:
            droppedUnfocused += 1
            return .droppedPending
        case .focused:
            if case var .motion(last)? = tail {
                last.merge(motion)
                ring[(head + count - 1) % InputQueue.capacity] = .motion(last)
                mergedMotions += 1
                return .merged
            }
            guard append(.motion(motion)) else { return .violation("queue invariant: no room for MOTION") }
            return .queued
        }
    }

    // MARK: Removal from the head (send or consume)

    /// Takes the head record. In GAINED_PENDING the check of 7.4 follows: with `free >= 3` the postponed GAINED is queued.
    mutating func pop() -> InputRecord? {
        guard count > 0, let record = ring[head] else { return nil }
        ring[head] = nil
        head = (head + 1) % InputQueue.capacity
        count -= 1
        if state == .gainedPending, free >= 3 {
            append(.focus(gained: true))
            state = .focused
        }
        return record
    }

    /// Exit of the VMHost queue (8.2 item 4, 10 item 1): the record is checked against the *current* display mode before it is
    /// handed on. A stale absolute part is cleared; a MOTION with nothing left is dropped and the next record is taken.
    mutating func popForConsumer(currentMode: InputDisplayMode?) -> InputRecord? {
        while let record = pop() {
            guard case let .motion(motion) = record else { return record }
            guard motion.absValid else { return record }
            // Only the count is compared here: the size was checked on receipt against the mode of that moment.
            if let mode = currentMode, mode.modeChangeCount == motion.modeChangeCount { return record }
            var cleared = motion
            cleared.clearAbsolute()
            if cleared.hasRelativePart { return .motion(cleared) }
            staleMotionsDropped += 1
        }
        return nil
    }
}
