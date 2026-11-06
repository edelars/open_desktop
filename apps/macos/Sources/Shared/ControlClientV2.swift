import Darwin
import Foundation

// Client of control protocol v2 for the controlling process (S11-06a): protocols/control/CONTROL_PROTOCOL_V2.md.
//
// It spawns `od-vmhost --control-v2 --socket-fd 3 --bundle PATH [--display-fd 5] [--input-fd 4]` over socketpairs (the same
// descriptors as `tests/system/vmhost_control_v2_lifecycle_smoke.py`), runs HELLO/READY, sends requests with strictly increasing
// Request IDs and reads frames on one reader thread. The frames are decoded by `ControlV2Decoder`/`ControlV2Response` (ControlV2.swift);
// there is no second codec. Responses and events are delivered on a `DispatchQueue` (the main queue by default), in the order they
// arrived. The client NEVER polls: no timer asks the VMHost for its state; `query()` is only called when the caller asks.
// The only timers are the teardown policy of the protocol (section 11: SIGTERM, 500 ms, SIGKILL) and one grace period after EOF.
//
// What the client does not do: it does not decide what the machine is doing. The state of a machine is what `stateChanged` events
// and the exit status say; a live pid is not a state. It does not hold a bundle lock (the VMHost does) and it does not check the
// launch refusals of VM_BUNDLE 3.5 (`BundleConfig.launchRefusal`); it reports `busy` when the VMHost does.

// MARK: transitions (section 7.2)

public enum ControlV2Transitions {
    /// Transitions of section 7.2 that the VMHost may report in `STATE_CHANGED`: `stopped -> running`, `running <-> paused`,
    /// `running` or `paused` or `shutting_down -> stopped`, `running -> shutting_down` (not reachable before S9-06 and S11-22), and
    /// any state other than `failed` to `failed`. `failed` is terminal.
    public static func isAllowed(from: ControlV2State, to: ControlV2State) -> Bool {
        switch (from, to) {
        case (.stopped, .running), (.running, .paused), (.paused, .running), (.running, .stopped), (.paused, .stopped),
             (.shuttingDown, .stopped), (.running, .shuttingDown):
            return true
        case (.failed, _):
            return false
        case (_, .failed):
            return true
        default:
            return false
        }
    }
}

// MARK: results and events

/// The terminal answer of one request: `ACK` (code `ok`) or `STATE` (answer of `QUERY`). A refusal is `ControlClientError.refused`.
public enum ControlV2Reply: Equatable {
    case ack(ControlV2Ack)
    case state(ControlV2StateReport)
}

public enum ControlClientError: Error, Equatable, CustomStringConvertible {
    /// No `READY` yet.
    case notReady
    /// Another request is in flight. The client keeps at most one request besides `QUERY`, and `ATTACH_RESOURCE` alone (section 5, 8).
    case requestInFlight
    case connectionClosed
    /// The VMHost answered with `ERROR` (not fatal for the connection unless `report.fatal`).
    case refused(ControlV2ErrorReport)
    case protocolViolation(String)
    case io(Int32)
    case invalidArgument(String)
    /// Request IDs are not reused: after `0xffffffff` a new VMHost process is needed (section 5).
    case requestIDsExhausted

    public var description: String {
        switch self {
        case .notReady: return "control v2: not ready"
        case .requestInFlight: return "control v2: another request is in flight"
        case .connectionClosed: return "control v2: connection closed"
        case .refused(let report): return "control v2: refused with code \(report.code)"
        case .protocolViolation(let text): return "control v2: protocol violation (\(text))"
        case .io(let code): return "control v2: I/O error \(code)"
        case .invalidArgument(let text): return "control v2: invalid argument (\(text))"
        case .requestIDsExhausted: return "control v2: request ids exhausted"
        }
    }
}

/// Why the VMHost refused to start, answered with `ERROR` instead of `READY` (section 4 item 2): `failed_kind` 0, fatal, then exit status 3.
public enum StartRejection: Equatable {
    /// `busy`: the bundle lock is held (the machine runs in another process, or a writer or a probe holds it for a moment: a false
    /// `busy` is possible, VM_BUNDLE 2.1). The user repeats the start; there are no automatic retries.
    case busy
    case lockChanged
    case lockInvalid
    case ioError(errno: UInt64)
    case other(code: UInt32)

    init(report: ControlV2ErrorReport) {
        switch ControlV2ErrorCode(rawValue: report.code) {
        case .busy?: self = .busy
        case .lockChanged?: self = .lockChanged
        case .lockInvalid?: self = .lockInvalid
        case .ioError?: self = .ioError(errno: report.aux0)
        default: self = .other(code: report.code)
        }
    }
}

/// What the process did (the last fact about a machine) and, when the VMHost refused to start, why.
public struct ControlClientExit: Equatable {
    public enum Termination: Equatable {
        case exited(Int32)
        /// Killed by a signal (the number of the signal); not folded into an exit code.
        case signaled(Int32)
        /// There was no process to wait for (a client over a connected socket).
        case unavailable
    }

    /// Meaning of the exit status (CONTROL_PROTOCOL_V2.md 12.3).
    public enum Disposition: Equatable, CaseIterable {
        case stopped
        case usageError
        case startRejected
        case protocolError
        case transportLost
        case hostFailed
        case killedBySignal
        case unexpected
        case unavailable
    }

    public var termination: Termination
    /// Set when the VMHost sent `ERROR` instead of `READY`.
    public var startRejection: StartRejection?

    public var isBusy: Bool { startRejection == .busy }

    public var disposition: Disposition {
        switch termination {
        case .unavailable: return .unavailable
        case .signaled: return .killedBySignal
        case .exited(let code):
            switch code {
            case 0: return .stopped
            case 2: return .usageError
            case 3: return .startRejected
            case 4: return .protocolError
            case 5: return .transportLost
            case 6: return .hostFailed
            default: return .unexpected
            }
        }
    }
}

/// Events in the order they arrive. `exited` is the last event and comes once.
public enum ControlClientEvent: Equatable {
    /// `READY` arrived: requests may be sent.
    case ready
    /// The VMHost refused to start (instead of `READY`). The connection ends; `exited` follows.
    case startRejected(StartRejection)
    /// `STATE_CHANGED`. `requestID` is the request that caused it, `0` when no request did.
    case stateChanged(from: ControlV2State, to: ControlV2State, reason: UInt32, requestID: UInt32)
    case stats(ControlV2Stats)
    case progress(requestKind: UInt32, completed: UInt64, total: UInt64)
    /// Every `ERROR` frame of the VMHost after the handshake (also the one that answers a request; that request's completion gets it too).
    case error(ControlV2ErrorReport, requestID: UInt32)
    /// The client closed the connection: the VMHost broke the protocol. The client terminates the process.
    case protocolViolation(String)
    case exited(ControlClientExit)
}

// MARK: core (no I/O: testable with synthetic frames)

struct ControlV2ClientCore {
    enum Phase: Equatable {
        case awaitingReady
        case ready
        case closed
    }

    enum Action {
        case event(ControlClientEvent)
        case complete(requestID: UInt32, Result<ControlV2Reply, ControlClientError>)
        case violation(String)
    }

    private(set) var phase: Phase = .awaitingReady
    /// The state that the last `STATE_CHANGED` reported; `stopped` after `READY` (section 4 item 5).
    private(set) var state: ControlV2State?
    private(set) var startRejection: StartRejection?
    private(set) var pending: [UInt32: ControlV2Kind] = [:]
    private var nextRequestID: UInt32 = 1
    private var idsExhausted = false
    private var decoder = ControlV2Decoder(direction: .towardController)
    private var broken = false

    /// Registers a request and returns its Request ID. The rules of the protocol that bind the sender: strictly increasing IDs without
    /// gaps, one request besides `QUERY`, and `ATTACH_RESOURCE` only when nothing else is in flight and with nothing sent after it
    /// before its answer (section 8).
    mutating func prepare(kind: ControlV2Kind) throws -> UInt32 {
        switch phase {
        case .awaitingReady: throw ControlClientError.notReady
        case .closed: throw ControlClientError.connectionClosed
        case .ready: break
        }
        guard !idsExhausted else { throw ControlClientError.requestIDsExhausted }
        if pending.values.contains(.attachResource) { throw ControlClientError.requestInFlight }
        if kind == .attachResource {
            guard pending.isEmpty else { throw ControlClientError.requestInFlight }
        } else if kind != .query {
            guard !pending.values.contains(where: { $0 != .query }) else { throw ControlClientError.requestInFlight }
        }
        let id = nextRequestID
        if id == UInt32.max { idsExhausted = true } else { nextRequestID = id + 1 }
        pending[id] = kind
        return id
    }

    /// Forgets a request (core tests only: the client does not roll IDs back, a write failure ends the connection).
    mutating func cancel(_ requestID: UInt32) {
        pending[requestID] = nil
    }

    /// Bytes read from the socket. Complete frames before a structure violation are still processed, then one `violation` ends it.
    mutating func ingest(_ bytes: [UInt8]) -> [Action] {
        guard !broken, phase != .closed else { return [] }
        decoder.append(bytes)
        return drain()
    }

    /// EOF between reads. A frame in progress is a truncated frame (section 11).
    mutating func endOfStream() -> [Action] {
        guard !broken else { return [] }
        do {
            try decoder.endOfStream()
        } catch {
            broken = true
            return [.violation("\(error)")]
        }
        return []
    }

    /// The connection is over: every request in flight fails with `connectionClosed`.
    mutating func close() -> [Action] {
        guard phase != .closed else { return [] }
        phase = .closed
        let ids = pending.keys.sorted()
        pending.removeAll()
        return ids.map { .complete(requestID: $0, .failure(.connectionClosed)) }
    }

    private mutating func drain() -> [Action] {
        var actions: [Action] = []
        while true {
            let received: ControlV2Received?
            do {
                received = try decoder.nextFrame()
            } catch {
                broken = true
                actions.append(.violation("\(error)"))
                return actions
            }
            guard let received else { return actions }
            if let descriptor = received.descriptor {
                Darwin.close(descriptor)
                broken = true
                actions.append(.violation("a descriptor arrived from the VMHost"))
                return actions
            }
            do {
                let response = try ControlV2Response.parse(received.frame)
                if let violation = handle(response, requestID: received.frame.requestID, into: &actions) {
                    broken = true
                    actions.append(.violation(violation))
                    return actions
                }
            } catch {
                broken = true
                actions.append(.violation("\(error)"))
                return actions
            }
        }
    }

    /// Returns a text when the frame breaks the order of the protocol.
    private mutating func handle(_ response: ControlV2Response, requestID: UInt32, into actions: inout [Action]) -> String? {
        if phase == .awaitingReady {
            switch response {
            case .ready:
                guard requestID == 0 else { return "READY with a request id" }
                phase = .ready
                state = .stopped
                actions.append(.event(.ready))
                return nil
            case .error(let report):
                // Section 4 item 2: instead of READY, request id 0, failed_kind 0, fatal.
                guard requestID == 0, report.failedKind == 0, report.fatal else { return "ERROR instead of READY is not fatal or not unsolicited" }
                let rejection = StartRejection(report: report)
                startRejection = rejection
                actions.append(.event(.startRejected(rejection)))
                return nil
            default:
                return "a frame before READY"
            }
        }
        switch response {
        case .ready:
            return "READY repeated"
        case .ack(let ack):
            guard let kind = pending[requestID] else { return "ACK for request \(requestID) that is not in flight" }
            guard ack.ackedKind == kind.rawValue else { return "ACK of kind \(ack.ackedKind) answers a request of kind \(kind.rawValue)" }
            // `in_progress` answers a repeat of a request in flight and is not terminal (6.2); the client never repeats one.
            guard ack.code == ControlV2Ack.codeOK else { return "ACK in_progress for a request that was not repeated" }
            pending[requestID] = nil
            actions.append(.complete(requestID: requestID, .success(.ack(ack))))
        case .state(let report):
            guard pending[requestID] == .query else { return "STATE for request \(requestID) that is not a QUERY in flight" }
            pending[requestID] = nil
            actions.append(.complete(requestID: requestID, .success(.state(report))))
        case .stateChanged(let from, let to, let reason):
            guard let fromState = ControlV2State(rawValue: from), let toState = ControlV2State(rawValue: to) else {
                return "STATE_CHANGED with an unknown state"
            }
            guard fromState == state, ControlV2Transitions.isAllowed(from: fromState, to: toState) else {
                return "STATE_CHANGED \(from) -> \(to) is not a transition of section 7.2 from \(state.map { String($0.rawValue) } ?? "?")"
            }
            state = toState
            actions.append(.event(.stateChanged(from: fromState, to: toState, reason: reason, requestID: requestID)))
        case .progress(let requestKind, let completed, let total):
            actions.append(.event(.progress(requestKind: requestKind, completed: completed, total: total)))
        case .stats(let stats):
            actions.append(.event(.stats(stats)))
        case .clipboardChanged:
            return "CLIPBOARD_CHANGED before S11-24"
        case .error(let report):
            actions.append(.event(.error(report, requestID: requestID)))
            if pending[requestID] != nil {
                pending[requestID] = nil
                actions.append(.complete(requestID: requestID, .failure(.refused(report))))
            }
        }
        return nil
    }
}

// MARK: launch

public struct ControlClientLaunch {
    public var vmhostPath: String
    /// Absolute path of the bundle directory (`--bundle`, VM_BUNDLE 7 item 1).
    public var bundlePath: String
    /// Create the frame-transport socketpair (`--display-fd 5`); the parent end is taken with `takeDisplaySocket()`.
    public var displayChannel: Bool
    /// Create the input socketpair (`--input-fd 4`); the parent end is taken with `takeInputSocket()`.
    public var inputChannel: Bool

    public init(vmhostPath: String, bundlePath: String, displayChannel: Bool = true, inputChannel: Bool = false) {
        self.vmhostPath = vmhostPath
        self.bundlePath = bundlePath
        self.displayChannel = displayChannel
        self.inputChannel = inputChannel
    }
}

// MARK: client

/// One control connection and one VMHost process. Not reusable: after `exited` a new client is needed (section 1).
public final class ControlClientV2 {
    public typealias EventHandler = (ControlClientEvent) -> Void
    public typealias Completion = (Result<ControlV2Reply, ControlClientError>) -> Void

    /// Serial queue of all state; the reader thread only hands bytes over to it.
    private let queue = DispatchQueue(label: "dev.opendesk.control-client-v2")
    private let deliveryQueue: DispatchQueue
    private let eventHandler: EventHandler
    private let socket: SocketBox
    private let pid: pid_t?
    private var displayFD: Int32?
    private var inputFD: Int32?
    private var core = ControlV2ClientCore()
    private var completions: [UInt32: Completion] = [:]
    private var finished = false
    private var violationReported = false
    /// A frame could not be written whole: no further request may be sent.
    private var transportLost = false
    /// Guards `childExited` and the signals: a signal is only sent to a process that has not exited (and so is not reaped, `WNOWAIT`).
    private let signalLock = NSLock()
    private var childExited = false
    /// Time after EOF in which the process should be gone before the teardown policy starts. Internal for tests.
    var exitGrace: TimeInterval = 2.0

    // MARK: construction

    /// Spawns the VMHost and starts the handshake. Throws only when the process cannot be started; every later failure is an event.
    public static func launch(
        _ launch: ControlClientLaunch,
        deliveryQueue: DispatchQueue = .main,
        eventHandler: @escaping EventHandler
    ) throws -> ControlClientV2 {
        try launchWithGrace(launch, deliveryQueue: deliveryQueue, exitGrace: 2.0, eventHandler: eventHandler)
    }

    /// `launch` with the grace period after EOF as a parameter (tests).
    static func launchWithGrace(
        _ launch: ControlClientLaunch,
        deliveryQueue: DispatchQueue,
        exitGrace: TimeInterval,
        eventHandler: @escaping EventHandler
    ) throws -> ControlClientV2 {
        var opened: [Int32] = []
        func closeAll() { opened.forEach { Darwin.close($0) } }
        let control: (parent: Int32, child: Int32)
        var display: (parent: Int32, child: Int32)?
        var input: (parent: Int32, child: Int32)?
        do {
            control = try VMHostLaunch.makeSocketPair()
            opened += [control.parent, control.child]
            if launch.displayChannel {
                display = try VMHostLaunch.makeSocketPair()
                opened += [display!.parent, display!.child]
            }
            if launch.inputChannel {
                input = try VMHostLaunch.makeSocketPair()
                opened += [input!.parent, input!.child]
            }
        } catch {
            closeAll()
            throw error
        }
        // The parent ends must not leak into the VMHost of another machine: a copy there would hide our EOF from this VMHost.
        for descriptor in [control.parent, display?.parent, input?.parent].compactMap({ $0 }) {
            _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC)
        }
        let child: pid_t
        do {
            child = try VMHostLaunch.spawnHelper(
                path: launch.vmhostPath, extraArguments: ["--control-v2", "--bundle", launch.bundlePath],
                parentFD: control.parent, childFD: control.child, input: input, display: display,
                closeOtherDescriptors: true)
        } catch {
            closeAll()
            throw error
        }
        // The UI closes its copies of the VMHost ends after the spawn.
        Darwin.close(control.child)
        if let display { Darwin.close(display.child) }
        if let input { Darwin.close(input.child) }
        let client = ControlClientV2(
            socketFD: control.parent, pid: child, displayFD: display?.parent, inputFD: input?.parent,
            deliveryQueue: deliveryQueue, eventHandler: eventHandler)
        client.exitGrace = exitGrace
        client.begin()
        return client
    }

    /// A client over a connected socket without a process (unit tests). Takes ownership of `socketFD`.
    init(
        socketFD: Int32, pid: pid_t?, displayFD: Int32? = nil, inputFD: Int32? = nil,
        deliveryQueue: DispatchQueue, eventHandler: @escaping EventHandler
    ) {
        self.socket = SocketBox(fd: socketFD)
        self.pid = pid
        self.displayFD = displayFD
        self.inputFD = inputFD
        self.deliveryQueue = deliveryQueue
        self.eventHandler = eventHandler
    }

    /// Starts the reader and sends `HELLO`. `READY` or a start rejection arrives as an event.
    func begin() {
        let thread = Thread { [self] in readLoop() }
        thread.name = "dev.opendesk.control-client-v2.reader"
        thread.start()
        queue.async { [self] in
            // A failed write is not reported here: the reader sees what the VMHost sent before it went away (for example `busy`).
            _ = writeFrame(ControlV2Frame(request: .hello, requestID: 0).encoded(), descriptor: nil)
        }
    }

    /// The pid of the VMHost this client spawned; `nil` for a client over a connected socket. A number, not a handle: signals go only
    /// through `terminate()`, which does not signal a process that has already exited (no pid reuse).
    public var processID: pid_t? { pid }

    // MARK: channels

    /// The parent end of the frame-transport socket (descriptor 5 of the VMHost). The caller owns it from here on; a socket that is
    /// not taken is closed when the process has exited.
    public func takeDisplaySocket() -> Int32? {
        queue.sync {
            let descriptor = displayFD
            displayFD = nil
            return descriptor
        }
    }

    /// The parent end of the input socket (descriptor 4 of the VMHost); ownership as `takeDisplaySocket()`.
    public func takeInputSocket() -> Int32? {
        queue.sync {
            let descriptor = inputFD
            inputFD = nil
            return descriptor
        }
    }

    // MARK: requests

    /// Sends one request. `completion` runs once on the delivery queue with the terminal answer; a refusal is `.failure(.refused)`.
    /// A request of `ATTACH_RESOURCE` carries a descriptor, the client takes it over and closes it after the frame was sent
    /// (section 8: the sender closes its copy); every other kind carries none. `HELLO` is the client's own.
    public func send(_ request: ControlV2Request, descriptor: Int32? = nil, completion: @escaping Completion) {
        queue.async { [self] in
            func fail(_ error: ControlClientError) {
                if let descriptor { Darwin.close(descriptor) }
                deliveryQueue.async { completion(.failure(error)) }
            }
            if finished || transportLost || violationReported { return fail(.connectionClosed) }
            let kind = request.kind
            if kind == .hello { return fail(.invalidArgument("HELLO is sent by the client itself")) }
            if (kind == .attachResource) != (descriptor != nil) {
                return fail(.invalidArgument("only ATTACH_RESOURCE carries a descriptor, and it must"))
            }
            if case .configure(let configure) = request, let violation = configure.validate() {
                return fail(.invalidArgument("CONFIGURE: \(violation.detail) at offset \(violation.aux0)"))
            }
            let id: UInt32
            do {
                id = try core.prepare(kind: kind)
            } catch let error as ControlClientError {
                return fail(error)
            } catch {
                return fail(.connectionClosed)
            }
            completions[id] = completion
            let written = writeFrame(ControlV2Frame(request: request, requestID: id).encoded(), descriptor: descriptor)
            // The sender closes its copy of the descriptor, whether the frame went out or not.
            if let descriptor { Darwin.close(descriptor) }
            if written != nil {
                // The request ID was issued and the frame may have been written in part: the stream cannot be continued (the next ID
                // would leave a gap, which the VMHost treats as fatal). The transport is lost; the reader sees EOF, and every request
                // in flight, this one included, ends with `connectionClosed` (`finish`).
                transportLost = true
                terminate()
            }
        }
    }

    public func configure(_ configure: ControlV2Configure, completion: @escaping Completion) {
        send(.configure(configure), completion: completion)
    }

    /// `operationID` is 0: a non-zero one is `unsupported_durability` before S9-06 (section 5).
    public func start(operationID: UInt64 = 0, completion: @escaping Completion) {
        send(.start(operationID: operationID), completion: completion)
    }

    public func pause(reason: ControlV2Reason = .user, completion: @escaping Completion) {
        send(.pause(reason: reason.rawValue), completion: completion)
    }

    public func resume(reason: ControlV2Reason = .user, completion: @escaping Completion) {
        send(.resume(reason: reason.rawValue), completion: completion)
    }

    public func stop(operationID: UInt64 = 0, completion: @escaping Completion) {
        send(.stop(operationID: operationID), completion: completion)
    }

    /// Only on an explicit request of the user ("Обновить"): the client does not call it by itself.
    public func query(completion: @escaping Completion) {
        send(.query, completion: completion)
    }

    /// `descriptor` is taken over (closed after the send, also on failure). The role is from the closed list of section 8.
    public func attachResource(role: ControlV2Role, index: UInt16 = 0, descriptor: Int32, completion: @escaping Completion) {
        send(.attachResource(ControlV2Attach(role: role, index: index)), descriptor: descriptor, completion: completion)
    }

    /// Ends the connection and stops the process by the policy of the protocol (section 11): the socket is shut down, SIGTERM, up to
    /// 500 ms, SIGKILL; the status is collected. Not a normal stop: use `stop()` for that.
    public func terminate() {
        socket.shutdown()
        terminateProcess()
    }

    // MARK: delivery

    private func process(_ actions: [ControlV2ClientCore.Action]) {
        for action in actions {
            switch action {
            case .event(let event):
                let handler = eventHandler
                deliveryQueue.async { handler(event) }
            case .complete(let id, let result):
                if let completion = completions.removeValue(forKey: id) { deliveryQueue.async { completion(result) } }
            case .violation(let text):
                guard !violationReported else { continue }
                violationReported = true
                let handler = eventHandler
                deliveryQueue.async { handler(.protocolViolation(text)) }
                terminate()
            }
        }
    }

    // MARK: reader thread

    private func readLoop() {
        var buffer = [UInt8](repeating: 0, count: ControlV2.maxFrameLength)
        while true {
            let received = Self.receive(fd: socket.fd, into: &buffer)
            if received.count < 0 {
                if received.errnoValue == EINTR { continue }
                break
            }
            if received.count == 0 && received.descriptors.isEmpty { break }
            if let violation = received.violation {
                // The frame that came with a descriptor (or with truncated ancillary data) is the violation: it is not processed.
                queue.async { [self] in process([.violation(violation)]) }
            } else if received.count > 0 {
                let bytes = Array(buffer[..<received.count])
                queue.async { [self] in
                    guard !finished, !violationReported else { return }
                    process(core.ingest(bytes))
                }
            }
        }
        queue.async { [self] in
            socket.close()
            process(core.endOfStream())
            if pid != nil {
                // The VMHost closes the connection and exits by itself; if it does not, the teardown policy takes over.
                queue.asyncAfter(deadline: .now() + exitGrace) { [self] in
                    if !finished { terminateProcess() }
                }
            }
        }
        let status = reapChild()
        queue.async { [self] in finish(status: status) }
    }

    /// One `recvmsg` with room for ancillary data: a descriptor is never expected from the VMHost (section 8), every one that arrives is closed.
    private static func receive(fd: Int32, into buffer: inout [UInt8]) -> (count: Int, descriptors: [Int32], violation: String?, errnoValue: Int32) {
        var control = [UInt8](repeating: 0, count: 12 + 16 * MemoryLayout<Int32>.size)
        var flags: Int32 = 0
        var descriptors: [Int32] = []
        let count = buffer.withUnsafeMutableBytes { dataBuffer in
            control.withUnsafeMutableBytes { controlBuffer in
                var iov = iovec(iov_base: dataBuffer.baseAddress, iov_len: dataBuffer.count)
                return withUnsafeMutablePointer(to: &iov) { iovPointer -> Int in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = controlBuffer.baseAddress
                    header.msg_controllen = socklen_t(controlBuffer.count)
                    let result = Darwin.recvmsg(fd, &header, 0)
                    flags = header.msg_flags
                    if result >= 0 {
                        // Darwin cmsghdr: len u32, level i32, type i32, data at 12; 4-byte alignment.
                        var offset = 0
                        let length = min(Int(header.msg_controllen), controlBuffer.count)
                        while offset + 12 <= length {
                            let size = Int(controlBuffer.loadUnaligned(fromByteOffset: offset, as: UInt32.self))
                            let level = controlBuffer.loadUnaligned(fromByteOffset: offset + 4, as: Int32.self)
                            let type = controlBuffer.loadUnaligned(fromByteOffset: offset + 8, as: Int32.self)
                            guard size >= 12 else { break }
                            if level == SOL_SOCKET, type == SCM_RIGHTS {
                                var position = offset + 12
                                while position + 4 <= min(offset + size, length) {
                                    descriptors.append(controlBuffer.loadUnaligned(fromByteOffset: position, as: Int32.self))
                                    position += 4
                                }
                            }
                            offset += (size + 3) & ~3
                        }
                    }
                    return result
                }
            }
        }
        let savedErrno = errno
        for descriptor in descriptors { Darwin.close(descriptor) }
        var violation: String?
        if count >= 0, !descriptors.isEmpty { violation = "a descriptor arrived from the VMHost" }
        if count >= 0, flags & Int32(MSG_CTRUNC) != 0 { violation = violation ?? "ancillary data truncated" }
        return (count, descriptors, violation, savedErrno)
    }

    // MARK: process

    /// Waits for the exit without reaping (`WNOWAIT`), forbids further signals, then reaps. nil: no process, or no status.
    private func reapChild() -> Int32? {
        guard let pid else { return nil }
        var info = siginfo_t()
        while waitid(P_PID, id_t(pid), &info, WEXITED | WNOWAIT) != 0 {
            if errno == EINTR { continue }
            break
        }
        signalLock.lock()
        childExited = true
        signalLock.unlock()
        var status: Int32 = 0
        while true {
            let waited = waitpid(pid, &status, 0)
            if waited == pid { return status }
            if waited < 0 && errno == EINTR { continue }
            return nil
        }
    }

    private func signalChild(_ signal: Int32) {
        guard let pid else { return }
        signalLock.lock()
        if !childExited { _ = kill(pid, signal) }
        signalLock.unlock()
    }

    /// Section 11: SIGTERM, up to 500 ms, SIGKILL. The status is collected by the reader thread.
    private func terminateProcess() {
        guard pid != nil else { return }
        signalChild(SIGTERM)
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + .milliseconds(500)) { [self] in
            signalChild(SIGKILL)
        }
    }

    private func finish(status: Int32?) {
        guard !finished else { return }
        finished = true
        process(core.close())
        let termination: ControlClientExit.Termination
        if let status {
            if status & 0x7F == 0 {
                termination = .exited((status >> 8) & 0xFF)
            } else {
                termination = .signaled(status & 0x7F)
            }
        } else {
            termination = .unavailable
        }
        let exit = ControlClientExit(termination: termination, startRejection: core.startRejection)
        let handler = eventHandler
        deliveryQueue.async { handler(.exited(exit)) }
        if let descriptor = displayFD { Darwin.close(descriptor) }
        if let descriptor = inputFD { Darwin.close(descriptor) }
        displayFD = nil
        inputFD = nil
    }

    // MARK: writing

    /// Writes one whole frame; with a descriptor, in one `sendmsg` with exactly one `SCM_RIGHTS` descriptor (section 8). Returns an errno.
    private func writeFrame(_ bytes: [UInt8], descriptor: Int32?) -> Int32? {
        let fd = socket.fd
        guard fd >= 0 else { return EBADF }
        var sent = 0
        if let descriptor {
            // CMSG_LEN(4) = 16 on Darwin: header 12 bytes and one int.
            var control = [UInt8](repeating: 0, count: 16)
            control.withUnsafeMutableBytes { raw in
                raw.storeBytes(of: UInt32(16), toByteOffset: 0, as: UInt32.self)
                raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
                raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
                raw.storeBytes(of: descriptor, toByteOffset: 12, as: Int32.self)
            }
            var data = bytes
            var failure: Int32?
            data.withUnsafeMutableBytes { dataBuffer in
                control.withUnsafeMutableBytes { controlBuffer in
                    var iov = iovec(iov_base: dataBuffer.baseAddress, iov_len: dataBuffer.count)
                    withUnsafeMutablePointer(to: &iov) { iovPointer in
                        var header = msghdr()
                        header.msg_iov = iovPointer
                        header.msg_iovlen = 1
                        header.msg_control = controlBuffer.baseAddress
                        header.msg_controllen = socklen_t(controlBuffer.count)
                        while true {
                            let result = Darwin.sendmsg(fd, &header, 0)
                            if result > 0 {
                                sent = result
                                return
                            }
                            if result < 0, errno == EINTR { continue }
                            failure = result == 0 ? EIO : errno
                            return
                        }
                    }
                }
            }
            if let failure { return failure }
        }
        // The rest goes without ancillary data (whole frame in one call in practice).
        while sent < bytes.count {
            let written = bytes.withUnsafeBytes { Darwin.send(fd, $0.baseAddress! + sent, $0.count - sent, 0) }
            if written < 0 {
                if errno == EINTR { continue }
                return errno
            }
            if written == 0 { return EIO }
            sent += written
        }
        return nil
    }
}

/// The descriptor of the control socket. It is closed once, on the serial queue (writes run there too), after the reader ended;
/// `shutdown` may come from any thread and only makes the reader see EOF.
final class SocketBox {
    private let lock = NSLock()
    private var descriptor: Int32
    private var open = true

    init(fd: Int32) { descriptor = fd }

    var fd: Int32 {
        lock.lock()
        defer { lock.unlock() }
        return open ? descriptor : -1
    }

    func shutdown() {
        lock.lock()
        defer { lock.unlock() }
        if open { _ = Darwin.shutdown(descriptor, SHUT_RDWR) }
    }

    func close() {
        lock.lock()
        defer { lock.unlock() }
        if open {
            open = false
            Darwin.close(descriptor)
        }
    }
}
