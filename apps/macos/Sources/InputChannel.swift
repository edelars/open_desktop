import Darwin
import Dispatch
import Foundation
#if canImport(ODShared)
import ODShared
#endif

// UI end of the input channel (INPUT_TRANSPORT_V1.md, task S10-16): HELLO/ACCEPT handshake, the bounded UI queue of
// section 7, non-blocking writes from its head into the socket, reception of STATUS. No AppKit: DisplayWindow.swift feeds it
// with already translated events, the unit test feeds it directly.
//
// Everything runs on the main thread. The socket is polled by a 10 ms timer on the main queue (`pump`), which also ends the
// wait for ACCEPT; writes go out at once from `flush` after every event, and a full socket only leaves the rest in the queue.
// There is NO delivery to the guest: VMHost sets `GUEST_DELIVERY = 0` and the UI says so (section 4.2).

enum InputChannelError: Error, CustomStringConvertible {
    case socketSetup
    var description: String { "could not configure the input socket" }
}

final class InputChannelUI {
    enum State: Equatable {
        /// HELLO sent, waiting for ACCEPT.
        case handshake
        case ready
        /// EOF, a socket error, a protocol error or an ACCEPT timeout: final for the channel.
        case closed(String)
        /// A header `ODIT` with another version (section 9.1): final for the channel.
        case incompatible(UInt16)
    }

    /// Counted events that were not transmitted (sections 5.2, 6.2, 6.3, 6.4, 8.1).
    struct Counters: Equatable {
        /// Events while the channel was not READY (HANDSHAKE).
        var droppedNotReady = 0
        /// Events of an unfocused window.
        var droppedUnfocused = 0
        var unmappedKeys = 0
        var ignoredButtons = 0
        var ignoredRepeats = 0
        /// DOWN of an already pressed usage, UP of a usage that is not pressed (section 6.3).
        var ignoredDuplicates = 0
        /// A queue refusal that should not happen (a bug); never silent.
        var queueViolations = 0
    }

    /// Wait for ACCEPT (section 8.1 allows a limit; the value is chosen here).
    static let acceptTimeoutMs = 5000

    private let fd: Int32
    private(set) var state = State.handshake
    private(set) var queue = InputQueue()
    private var decoder = InputDecoder(direction: .hostToUI)
    private var partial: [UInt8] = []
    private var windowIsKey = false
    private(set) var acceptFlags: UInt32?
    private(set) var hostStatus: InputStatus?
    private(set) var counters = Counters()
    private var loggedUnmapped = Set<UInt16>()
    private var handshakeDeadline: DispatchTime?
    private var timer: DispatchSourceTimer?
    private var pointer = InputPointerState()

    /// Called on every change of what the window shows about input.
    var onChange: (() -> Void)?

    init(fd: Int32) throws {
        self.fd = fd
        guard InputSocket.makeNonBlocking(fd), InputSocket.setNoSigpipe(fd) else {
            Darwin.close(fd)
            throw InputChannelError.socketSetup
        }
    }

    deinit { shutdown() }

    // MARK: State for the window

    /// True while the window collects input events (HANDSHAKE and READY): they are queued or counted. In CLOSED and
    /// INCOMPATIBLE they are not collected at all (section 8.1).
    var isCollecting: Bool {
        switch state {
        case .handshake, .ready: return true
        case .closed, .incompatible: return false
        }
    }

    var isFocused: Bool { state == .ready && queue.state == .focused }

    /// `GUEST_DELIVERY` of ACCEPT; nil before ACCEPT.
    var guestDelivery: Bool? { acceptFlags.map { $0 & InputProtocol.acceptGuestDeliveryFlag != 0 } }

    /// Text for the window (section 4.2: show `GUEST_DELIVERY = 0` explicitly; section 8.1: reason when disconnected).
    var statusText: String {
        switch state {
        case .handshake:
            return "input: connecting"
        case let .closed(reason):
            return "input disconnected: \(reason)"
        case let .incompatible(version):
            return "input disconnected: incompatible protocol version \(version)"
        case .ready:
            var text = guestDelivery == true ? "input: delivered to the guest" : "input is not delivered to the guest (no input device yet)"
            let rejected = queue.rejectedDowns + (hostStatus?.rejectedDowns ?? 0)
            if rejected > 0 { text += "; \(rejected) key presses rejected" }
            return text
        }
    }

    // MARK: Start and stop

    /// Sends HELLO and, with `attachTimer`, starts the 10 ms pump on the main queue.
    func start(attachTimer: Bool = true) {
        partial = InputMessage.hello.encoded()
        handshakeDeadline = DispatchTime.now() + .milliseconds(InputChannelUI.acceptTimeoutMs)
        flush()
        guard attachTimer else { return }
        let source = DispatchSource.makeTimerSource(queue: .main)
        source.schedule(deadline: .now() + .milliseconds(10), repeating: .milliseconds(10))
        source.setEventHandler { [weak self] in self?.pump() }
        source.resume()
        timer = source
    }

    /// Closes the UI end: VMHost sees EOF (section 8.1: CLOSED releases the queue and clears the pressed usages).
    func shutdown() {
        if case .closed = state { return }
        if case .incompatible = state { return }
        close(reason: "shut down by the UI")
    }

    private func close(reason: String) {
        enterFinal(.closed(reason))
    }

    private func enterFinal(_ final: State) {
        guard state == .handshake || state == .ready else { return }
        timer?.cancel()
        timer = nil
        queue = InputQueue()
        partial = []
        pointer.reset()
        hostStatus = nil
        Darwin.close(fd)
        state = final
        onChange?()
    }

    // MARK: Socket

    /// One pump step: pending writes, reads (bounded), the ACCEPT timeout.
    func pump() {
        guard isCollecting else { return }
        flush()
        readAvailable()
        if state == .handshake, let deadline = handshakeDeadline, DispatchTime.now() >= deadline {
            close(reason: "no ACCEPT within \(InputChannelUI.acceptTimeoutMs) ms")
        }
    }

    private func readAvailable() {
        var buffer = [UInt8](repeating: 0, count: InputProtocol.maxMessageToUI)
        var reads = 0
        while isCollecting, reads < 64 {
            reads += 1
            switch InputSocket.receive(fd: fd, into: &buffer, maxBytes: buffer.count) {
            case let .data(count):
                do {
                    for message in try decoder.append(Array(buffer[0..<count])) {
                        try handle(message)
                        if !isCollecting { return }
                    }
                } catch InputProtocolError.incompatibleVersion(let version) {
                    enterFinal(.incompatible(version))
                    return
                } catch {
                    close(reason: "\(error)")
                    return
                }
            case .wouldBlock:
                return
            case .eof:
                close(reason: "EOF from VMHost")
                return
            case let .error(code):
                close(reason: "socket error \(code)")
                return
            case .ancillary:
                close(reason: "input protocol error: descriptor or control data in a message")
                return
            }
        }
    }

    private func handle(_ message: InputMessage) throws {
        switch (state, message) {
        case let (.handshake, .accept(flags)):
            acceptFlags = flags
            state = .ready
            handshakeDeadline = nil
            if windowIsKey {
                applyFocus(gained: true)
                flush()
            }
            onChange?()
        case let (.ready, .status(status)):
            hostStatus = status
            onChange?()
        default:
            throw InputProtocolError.malformed("unexpected \(message.kind) in \(state)")
        }
    }

    /// Writes from the head of the queue until the socket is full. A message cut by a short write stays in `partial`.
    func flush() {
        while isCollecting {
            if partial.isEmpty {
                guard let record = queue.pop() else { return }
                partial = record.message.encoded()
            }
            switch InputSocket.send(fd: fd, partial) {
            case let .written(count):
                partial.removeFirst(count)
            case .wouldBlock:
                return
            case let .error(code):
                close(reason: "socket write error \(code)")
                return
            }
        }
    }

    // MARK: Focus (section 6.4)

    /// The window became key (`gained`) or stopped being key, was minimised or closed, or the application resigned active.
    func windowFocus(gained: Bool) {
        windowIsKey = gained
        guard state == .ready else { return }
        applyFocus(gained: gained)
        flush()
        onChange?()
    }

    private func applyFocus(gained: Bool) {
        if gained {
            pointer.reset() // section 5.4: remainders start from zero in a new focus session as well
            if queue.state == .unfocused { note(queue.focus(gained: true)) }
        } else {
            // FOCUS LOST releases everything: the queue clears its pressed set; the remainders of 5.4 and 5.5 are reset.
            if queue.state != .unfocused { note(queue.focus(gained: false)) }
            pointer.reset()
        }
    }

    // MARK: Events

    /// Common gate of every key, button, motion or wheel event. false = the event is counted and must not be queued.
    private func admit() -> Bool {
        switch state {
        case .handshake:
            counters.droppedNotReady += 1
            return false
        case .ready:
            if queue.state == .unfocused {
                counters.droppedUnfocused += 1
                return false
            }
            return true // GAINED_PENDING: the queue drops and counts (7.4)
        case .closed, .incompatible:
            return false
        }
    }

    private func note(_ outcome: InputQueue.Outcome) {
        if case .violation = outcome { counters.queueViolations += 1 }
    }

    /// A `keyDown` with `isARepeat` (USB HID keyboards have no auto-repeat; the guest repeats, section 6.3).
    func noteRepeat() { counters.ignoredRepeats += 1 }

    /// A key code with no row in the table (section 6.2): counted, logged once per code.
    func noteUnmapped(keyCode: UInt16) {
        counters.unmappedKeys += 1
        if loggedUnmapped.insert(keyCode).inserted {
            fputs("display-viewer: input: key code 0x\(String(keyCode, radix: 16)) has no HID usage, not transmitted\n", stderr)
        }
    }

    func noteIgnoredButton() { counters.ignoredButtons += 1 }

    /// `DOWN` or `UP` of one usage. Duplicates (section 6.3) are counted and not sent.
    func usage(page: UInt16, id: UInt16, down: Bool) {
        guard admit() else { return }
        let known = queue.isPressed(page: page, id: id) || queue.isSuppressed(page: page, id: id)
        if down == known {
            counters.ignoredDuplicates += 1
            return
        }
        note(queue.usage(page: page, id: id, down: down))
        flush()
        onChange?()
    }

    /// Caps Lock: one `flagsChanged` per toggle becomes a `DOWN`, `UP` pair (section 6.3).
    func capsLockToggle() {
        usage(page: InputUsagePage.keyboard, id: 0x39, down: true)
        usage(page: InputUsagePage.keyboard, id: 0x39, down: false)
    }

    /// A mouse button. The position goes first as a MOTION with zero deltas and `ABS_VALID = 1` (section 5.2); without a
    /// computable position there is no such MOTION.
    func button(number: Int, down: Bool, absolute: InputAbsolute?) {
        guard let id = HIDUsage.buttonUsage(forButtonNumber: number) else {
            noteIgnoredButton()
            return
        }
        guard admit() else { return }
        let page = InputUsagePage.button
        let known = queue.isPressed(page: page, id: id) || queue.isSuppressed(page: page, id: id)
        if down == known {
            counters.ignoredDuplicates += 1
            return
        }
        if let absolute { enqueueMotion(dx: 0, dy: 0, wheel: 0, pan: 0, absolute: absolute) }
        note(queue.usage(page: page, id: id, down: down))
        flush()
        onChange?()
    }

    /// Relative motion, wheel, pan and an optional position. Nothing to send (all zero, no position) is dropped silently.
    func motion(dx: Int32, dy: Int32, wheel: Int32, pan: Int32, absolute: InputAbsolute?) {
        guard admit() else { return }
        guard dx != 0 || dy != 0 || wheel != 0 || pan != 0 || absolute != nil else { return }
        enqueueMotion(dx: dx, dy: dy, wheel: wheel, pan: pan, absolute: absolute)
        flush()
        onChange?()
    }

    private func enqueueMotion(dx: Int32, dy: Int32, wheel: Int32, pan: Int32, absolute: InputAbsolute?) {
        var motion = InputMotion()
        motion.dx = dx
        motion.dy = dy
        motion.wheel = wheel
        motion.pan = pan
        if let absolute {
            motion.absValid = true
            motion.modeChangeCount = absolute.modeChangeCount
            motion.absX = absolute.x
            motion.absY = absolute.y
            motion.absWidth = absolute.width
            motion.absHeight = absolute.height
        }
        note(queue.motion(motion))
    }

    /// Pointer remainders (5.4, 5.5) live here so that `FOCUS LOST` resets them together with the queue state.
    func relativeDelta(dxPoints: Double, dyPoints: Double, geometry: InputGeometry) -> (dx: Int32, dy: Int32) {
        pointer.relative(dxPoints: dxPoints, dyPoints: dyPoints, geometry: geometry)
    }

    func scrollUnits(deltaX: Double, deltaY: Double, precise: Bool, invertedFromDevice: Bool) -> (wheel: Int32, pan: Int32) {
        pointer.scroll(deltaX: deltaX, deltaY: deltaY, precise: precise, invertedFromDevice: invertedFromDevice)
    }
}
