import Darwin
import Foundation

// VMHost end of the input channel (protocols/control/INPUT_TRANSPORT_V1.md, task S10-16).
//
// Enabled ONLY by `--input-fd N` (together with the test-only `--display-smoke`); without it there is no input channel and
// the normal path is unchanged. The channel reads ODIT messages from the inherited socketpair end, checks every header before
// its payload, runs the bounded queue of section 7 (InputQueue.swift, shared with the UI), answers HELLO with ACCEPT and
// reports counters with STATUS (at most every 100 ms).
//
// THERE IS NO CONSUMER: the `od-host` C ABI has no input functions (section 10), so records stay in the queue; it fills up to
// 256 records and then merges motions and refuses DOWNs by the rules of 7.3. `GUEST_DELIVERY = 0` in ACCEPT. The only consumer is
// the TEST-ONLY trace (`--input-trace 1`, section 10): it takes records from the head through `popForConsumer`, which repeats the
// `mode_change_count` check at the queue exit, and prints them to stderr.
//
// An error on this channel (EOF, socket error, protocol error) closes only this channel (section 8.3): FOCUS LOST is queued if the
// state was FOCUSED, the descriptor is closed, frame transport goes on.

enum InputChannelHostError: Error, CustomStringConvertible {
    case setup(String)

    var description: String {
        switch self {
        case let .setup(what): return "input channel: \(what)"
        }
    }
}

final class InputHostChannel {
    enum Phase: Equatable {
        case waitHello
        case running
        case closed(String)
    }

    /// Section 7.5: STATUS at most every 100 ms, the last change not later than 100 ms after it.
    static let statusIntervalNs: UInt64 = 100_000_000
    /// Bound of the work for one `service` call: messages are read in pieces of one largest message.
    private static let maxReadsPerService = 64

    /// Section 4.2 `GUEST_DELIVERY`: the board has no input device yet, so this is always false.
    static let guestDelivery = false

    private(set) var fd: Int32
    private(set) var phase = Phase.waitHello
    private(set) var queue = InputQueue()
    private var decoder = InputDecoder(direction: .uiToHost, requiredFirstKind: .hello)
    private var partial: [UInt8] = []
    private var pendingStatus: [UInt8]?
    private var reportedCounters: (UInt64, UInt64, UInt64) = (0, 0, 0)
    private var lastStatusNs: UInt64?
    private let trace: Bool
    private var protocolErrors = 0

    init(fd: Int32, trace: Bool = false) throws {
        self.fd = fd
        self.trace = trace
        // Section 2.2 item 3: other children of VMHost must not inherit the channel; non-blocking mode on this end.
        guard fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw InputChannelHostError.setup("FD_CLOEXEC errno \(errno)") }
        guard InputSocket.makeNonBlocking(fd) else { throw InputChannelHostError.setup("O_NONBLOCK errno \(errno)") }
        guard InputSocket.setNoSigpipe(fd) else { throw InputChannelHostError.setup("SO_NOSIGPIPE errno \(errno)") }
    }

    deinit { if fd >= 0 { Darwin.close(fd) } }

    var isOpen: Bool {
        if case .closed = phase { return false }
        return true
    }

    var closeReason: String? {
        if case let .closed(reason) = phase { return reason }
        return nil
    }

    /// Events to poll on the socket: always readable; writable while an output message waits.
    var pollEvents: Int16 {
        guard isOpen else { return 0 }
        return Int16(POLLIN) | (partial.isEmpty && pendingStatus == nil ? 0 : Int16(POLLOUT))
    }

    var status: InputStatus {
        InputStatus(
            rejectedDowns: queue.rejectedDowns, mergedMotions: queue.mergedMotions,
            droppedUnfocused: queue.droppedUnfocused, queueDepth: UInt32(queue.count))
    }

    /// Milliseconds until the channel needs attention for a STATUS that is due, or nil when nothing is due.
    func pollTimeoutMs(nowNs: UInt64) -> Int32? {
        guard isOpen, countersChanged else { return nil }
        guard let last = lastStatusNs, nowNs < last + InputHostChannel.statusIntervalNs else { return 0 }
        return Int32((last + InputHostChannel.statusIntervalNs - nowNs + 999_999) / 1_000_000)
    }

    private var countersChanged: Bool {
        let now = (queue.rejectedDowns, queue.mergedMotions, queue.droppedUnfocused)
        return now != reportedCounters
    }

    // MARK: Service

    /// One step: read what is there, run the queue rules, send what the socket takes, report counters and (test-only) trace.
    /// `mode` is the current display mode (nil without an active display); it is read lazily per message.
    func service(revents: Int16, nowNs: UInt64, mode: () -> InputDisplayMode?) {
        if isOpen {
            if revents & Int16(POLLNVAL) != 0 {
                closeChannel(reason: "poll reported an invalid descriptor")
            } else {
                if revents & Int16(POLLOUT) != 0 { flushOutput() }
                if isOpen, revents & Int16(POLLIN | POLLHUP | POLLERR) != 0 { readAvailable(mode: mode) }
            }
        }
        reportStatusIfDue(nowNs: nowNs)
        if isOpen { flushOutput() }
        if trace { drainTrace(mode: mode()) }
    }

    private func readAvailable(mode: () -> InputDisplayMode?) {
        var buffer = [UInt8](repeating: 0, count: InputProtocol.maxMessageToHost)
        var reads = 0
        while isOpen, reads < InputHostChannel.maxReadsPerService {
            reads += 1
            switch InputSocket.receive(fd: fd, into: &buffer, maxBytes: buffer.count) {
            case let .data(count):
                do {
                    for message in try decoder.append(Array(buffer[0..<count])) {
                        try handle(message, mode: mode)
                        if !isOpen { return }
                    }
                } catch InputProtocolError.incompatibleVersion(let version) {
                    // Section 9.1: one VERSION with our version, then close.
                    protocolErrors += 1
                    sendVersionReply()
                    closeChannel(reason: "peer protocol version \(version)")
                    return
                } catch InputProtocolError.badMagic {
                    protocolErrors += 1
                    closeChannel(reason: "foreign magic")
                    return
                } catch {
                    protocolErrors += 1
                    closeChannel(reason: "\(error)")
                    return
                }
            case .wouldBlock:
                return
            case .eof:
                closeChannel(reason: "EOF")
                return
            case let .error(code):
                closeChannel(reason: "socket error \(code)")
                return
            case .ancillary:
                protocolErrors += 1
                closeChannel(reason: "protocol error: descriptor or control data in a message")
                return
            }
        }
    }

    private func handle(_ message: InputMessage, mode: () -> InputDisplayMode?) throws {
        switch phase {
        case .waitHello:
            // Section 8.2 item 1: the first message must be HELLO; ACCEPT goes out at once, then UNFOCUSED.
            guard case .hello = message else { throw InputProtocolError.malformed("first message is \(message.kind), not HELLO") }
            partial = InputMessage.accept(flags: InputHostChannel.guestDelivery ? InputProtocol.acceptGuestDeliveryFlag : 0).encoded()
            phase = .running
            flushOutput()
        case .running:
            switch message {
            case .hello:
                throw InputProtocolError.malformed("second HELLO")
            case let .focus(gained):
                try expect(queue.focus(gained: gained))
            case let .usage(usage):
                try expect(queue.usage(page: usage.page, id: usage.id, down: usage.down))
            case let .motion(motion):
                // Section 8.2 item 4, on receipt: a stale absolute part is reset (not an error); the same count with another
                // size is an error; a record that has nothing left is dropped. In UNFOCUSED the queue reports the error.
                guard queue.state != .unfocused else { throw InputProtocolError.malformed("MOTION while unfocused") }
                // GAINED_PENDING (7.4): every MOTION is dropped and counted in `dropped_unfocused`, also one that the
                // reconciliation would drop as empty. In FOCUSED a MOTION dropped as empty is counted nowhere: it is not
                // merged (`merged_motions`, section 4.2) and not a refusal.
                if queue.state == .gainedPending {
                    try expect(queue.motion(motion))
                    break
                }
                switch try motion.reconciled(with: mode()) {
                case let .keep(value): try expect(queue.motion(value))
                case .dropEmpty: break
                }
            case .version, .accept, .status:
                throw InputProtocolError.malformed("kind \(message.kind) is not sent by the UI")
            }
        case .closed:
            break
        }
    }

    private func expect(_ outcome: InputQueue.Outcome) throws {
        if case let .violation(reason) = outcome { throw InputProtocolError.malformed(reason) }
    }

    // MARK: Output

    /// Section 7.5: one partially sent message and one waiting STATUS; a new STATUS replaces an unsent one.
    private func flushOutput() {
        while isOpen {
            if partial.isEmpty {
                guard let next = pendingStatus else { return }
                pendingStatus = nil
                partial = next
            }
            switch InputSocket.send(fd: fd, partial) {
            case let .written(count):
                partial.removeFirst(count)
            case .wouldBlock:
                return
            case let .error(code):
                closeChannel(reason: "socket write error \(code)")
                return
            }
        }
    }

    private func reportStatusIfDue(nowNs: UInt64) {
        guard isOpen, phase == .running, countersChanged else { return }
        if let last = lastStatusNs, nowNs < last + InputHostChannel.statusIntervalNs { return }
        pendingStatus = InputMessage.status(status).encoded()
        reportedCounters = (queue.rejectedDowns, queue.mergedMotions, queue.droppedUnfocused)
        lastStatusNs = nowNs
    }

    private func sendVersionReply() {
        guard partial.isEmpty else { return } // a message is cut in the middle: nothing may follow it
        _ = InputSocket.send(fd: fd, InputMessage.version.encoded())
    }

    // MARK: Closing (section 8.3)

    /// Closes only this channel: FOCUS LOST is queued when the state was FOCUSED (the invariant keeps room for it),
    /// a pending GAINED is cancelled, the descriptor is closed and nothing is read afterwards. The queue stays for the trace.
    func closeChannel(reason: String) {
        guard isOpen else { return }
        switch queue.state {
        case .focused: _ = queue.focus(gained: false)
        case .gainedPending: _ = queue.focus(gained: false)
        case .unfocused: break
        }
        partial = []
        pendingStatus = nil
        Darwin.close(fd)
        fd = -1
        phase = .closed(reason)
        fputs("od-vmhost-smoke: input channel closed: \(reason)\n", stderr)
    }

    // MARK: Test-only trace consumer (section 10)

    private func drainTrace(mode: InputDisplayMode?) {
        while let record = queue.popForConsumer(currentMode: mode) {
            fputs("od-vmhost-smoke: input-trace \(InputHostChannel.describe(record))\n", stderr)
        }
    }

    static func describe(_ record: InputRecord) -> String {
        switch record {
        case let .focus(gained):
            return "FOCUS \(gained ? "GAINED" : "LOST")"
        case let .usage(usage):
            return "USAGE page=\(usage.page) id=\(usage.id) state=\(usage.down ? "DOWN" : "UP")"
        case let .motion(m):
            var text = "MOTION dx=\(m.dx) dy=\(m.dy) wheel=\(m.wheel) pan=\(m.pan)"
            if m.absValid {
                text += " abs=\(m.absX),\(m.absY) size=\(m.absWidth)x\(m.absHeight) mode=\(m.modeChangeCount)"
            } else {
                text += " abs=none"
            }
            return text
        }
    }
}
