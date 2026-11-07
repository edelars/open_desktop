import Darwin
import Foundation
import ODShared

// A blocking view of one VMHost session for a command-line process (S11-13, ADR-0006 (в)). `ControlClientV2` delivers events and answers
// on a queue; this class puts them behind a condition so that the main thread can write the command as a sequence of steps.
// One session = one VMHost process = one command. Nothing outlives `odctl`: the process is stopped (`STOP`), or terminated by the policy
// of the client (SIGTERM, up to 500 ms, SIGKILL), before the command returns.

final class HostSession {
    enum Readiness {
        case ready
        case ended(ControlClientExit)
    }

    private let client: ControlClientV2
    private let condition = NSCondition()
    private var isReady = false
    private var exit: ControlClientExit?
    private var lastState: ControlV2State?
    private var fatalError: ControlV2ErrorReport?
    private var violation: String?

    /// Spawns the VMHost. `onEvent` runs on the delivery queue, in the order of the events, before a waiter is woken.
    init(vmhost: String, bundle: URL, onEvent: ((ControlClientEvent) -> Void)? = nil) throws {
        let launch = ControlClientLaunch(vmhostPath: vmhost, bundlePath: bundle.path, displayChannel: false)
        let deliveryQueue = DispatchQueue(label: "dev.opendesk.odctl.events")
        // The handler needs `self`, which does not exist before the client does: a box that is filled right after. `launch` starts the
        // reader and sends HELLO, so an early event (`READY`, `exited` of a refused start) may be ready before the box is filled: the queue
        // is suspended until it is, and the box is guarded by a lock for the reader of the queue.
        let holder = Holder()
        deliveryQueue.suspend()
        do {
            client = try ControlClientV2.launch(launch, deliveryQueue: deliveryQueue) { event in
                holder.session?.handle(event)
                onEvent?(event)
            }
        } catch {
            deliveryQueue.resume() // a suspended queue must not be released
            throw error
        }
        holder.session = self
        deliveryQueue.resume()
    }

    private final class Holder {
        private let lock = NSLock()
        private weak var stored: HostSession?

        var session: HostSession? {
            get {
                lock.lock()
                defer { lock.unlock() }
                return stored
            }
            set {
                lock.lock()
                stored = newValue
                lock.unlock()
            }
        }
    }

    private func handle(_ event: ControlClientEvent) {
        condition.lock()
        switch event {
        case .ready: isReady = true
        case .stateChanged(_, let to, _, _): lastState = to
        case .exited(let value): exit = value
        case .error(let report, _): if report.fatal { fatalError = report }
        case .protocolViolation(let text): violation = text
        case .startRejected, .stats, .progress: break
        }
        condition.broadcast()
        condition.unlock()
    }

    var processID: pid_t? { client.processID }

    /// The state known from the last `STATE_CHANGED` or answer.
    var state: ControlV2State? {
        condition.lock()
        defer { condition.unlock() }
        return lastState
    }

    var protocolViolationText: String? {
        condition.lock()
        defer { condition.unlock() }
        return violation
    }

    var exited: ControlClientExit? {
        condition.lock()
        defer { condition.unlock() }
        return exit
    }

    /// The connection is alive: no exit and no fatal `ERROR`.
    var isAlive: Bool {
        condition.lock()
        defer { condition.unlock() }
        return exit == nil && fatalError == nil && violation == nil
    }

    // MARK: waiting

    /// `READY`, or the end of the process (a refused start: `busy`).
    func waitReady(timeout: TimeInterval) -> Readiness? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while !isReady && exit == nil {
            if !condition.wait(until: deadline) { return nil }
        }
        if let exit { return .ended(exit) }
        return .ready
    }

    /// The state `target`, or the end of the process (returned as `.failure`), or `nil` on a timeout.
    enum StateWait {
        case reached
        case ended(ControlClientExit)
        case timedOut
    }

    func waitState(_ target: ControlV2State, timeout: TimeInterval) -> StateWait {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while lastState != target {
            if let exit { return .ended(exit) }
            if !condition.wait(until: deadline) { return lastState == target ? .reached : .timedOut }
        }
        return .reached
    }

    func waitExit(timeout: TimeInterval) -> ControlClientExit? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while exit == nil {
            if !condition.wait(until: deadline) { return exit }
        }
        return exit
    }

    // MARK: requests

    /// Sends one request and waits for its terminal answer. The state of an answer is remembered.
    func send(_ request: ControlV2Request, timeout: TimeInterval = 30) -> Result<ControlV2Reply, ControlClientError> {
        let semaphore = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var answer: Result<ControlV2Reply, ControlClientError>?
        client.send(request) { result in
            lock.lock()
            answer = result
            lock.unlock()
            semaphore.signal()
        }
        if semaphore.wait(timeout: .now() + timeout) == .timedOut { return .failure(.io(ETIMEDOUT)) }
        lock.lock()
        let result = answer ?? .failure(.connectionClosed)
        lock.unlock()
        if case .success(let reply) = result {
            let raw: UInt32
            switch reply {
            case .ack(let ack): raw = ack.state
            case .state(let report): raw = report.state
            }
            condition.lock()
            lastState = ControlV2State(rawValue: raw) ?? lastState
            condition.broadcast()
            condition.unlock()
        }
        return result
    }

    /// Ends the process by the policy of the client (SIGTERM, up to 500 ms, SIGKILL) and waits for its end.
    func terminate() {
        client.terminate()
        _ = waitExit(timeout: 10)
    }

    /// The tail of a command on a machine with `disks`, which `STOP` cannot end before S9-06 (CONTROL_PROTOCOL_V2.md 7.2, explanation 5):
    /// the connection is closed, the VMHost sees EOF in `stopped` and exits with status 0. The policy of the client takes over when it
    /// does not exit. Returns the end that was observed.
    @discardableResult
    func finishByClosing() -> ControlClientExit? {
        if isAlive, exited == nil { client.closeConnection() }
        if let end = waitExit(timeout: 10) { return end }
        terminate()
        return exited
    }

    /// The tail of every command: `STOP` when the process is alive (valid in every state), the end of the process, and the policy of the
    /// client when it does not come. Returns the end that was observed.
    @discardableResult
    func finish(sendStop: Bool = true) -> ControlClientExit? {
        if sendStop, isAlive, exited == nil {
            if case .failure = send(.stop(operationID: 0), timeout: 10) { client.terminate() }
        }
        if let end = waitExit(timeout: 10) { return end }
        terminate()
        return exited
    }
}
