import Darwin
import Foundation
import XCTest
@testable import ODShared

// Tests of Shared/ControlClientV2.swift (S11-06a): the client of protocols/control/CONTROL_PROTOCOL_V2.md.
//   1. the core (no I/O) against synthetic frames made by `ControlV2Frame` (the codec of ControlV2.swift);
//   2. the client over a socketpair with an in-process fake VMHost (frames, SCM_RIGHTS, violations, no polling);
//   3. the process layer with shell scripts in place of the VMHost (exit status, signals, `busy`, teardown policy);
//   4. the real `od-vmhost` of the same build (skipped when the binary is not next to the test bundle).
// Run: `swift test --filter ControlClientV2XCTests` from apps/.

// MARK: helpers

private func frame(_ response: ControlV2Response, id: UInt32 = 0) -> ControlV2Frame { ControlV2Frame(response: response, requestID: id) }

private func ack(_ kind: ControlV2Kind, state: ControlV2State, id: UInt32) -> ControlV2Frame {
    frame(.ack(ControlV2Ack(ackedKind: kind.rawValue, state: state.rawValue)), id: id)
}

private func changed(_ from: ControlV2State, _ to: ControlV2State, _ reason: ControlV2Reason = .user, id: UInt32 = 0) -> ControlV2Frame {
    frame(.stateChanged(from: from.rawValue, to: to.rawValue, reason: reason.rawValue), id: id)
}

private func errorFrame(_ code: ControlV2ErrorCode, kind: ControlV2Kind?, id: UInt32, fatal: Bool = false, state: ControlV2State = .stopped) -> ControlV2Frame {
    frame(.error(ControlV2ErrorReport(code: code, failedKind: kind?.rawValue ?? 0, flags: fatal ? ControlV2ErrorReport.flagFatal : 0,
                                       state: state.rawValue, detail: "detail")), id: id)
}

private let readyFrame = frame(.ready(profile: 2))

private extension Array where Element == ControlV2ClientCore.Action {
    var events: [ControlClientEvent] { compactMap { if case .event(let event) = $0 { return event } else { return nil } } }
    var completions: [(UInt32, Result<ControlV2Reply, ControlClientError>)] {
        compactMap { if case .complete(let id, let result) = $0 { return (id, result) } else { return nil } }
    }
    var violations: [String] { compactMap { if case .violation(let text) = $0 { return text } else { return nil } } }
}

/// Collects the events of a client on its delivery queue; `wait` blocks the test thread, never the queue.
private final class EventLog {
    let queue = DispatchQueue(label: "test.event-log")
    private let condition = NSCondition()
    private var stored: [ControlClientEvent] = []

    var events: [ControlClientEvent] {
        condition.lock()
        defer { condition.unlock() }
        return stored
    }

    func append(_ event: ControlClientEvent) {
        condition.lock()
        stored.append(event)
        condition.broadcast()
        condition.unlock()
    }

    @discardableResult
    func wait(timeout: TimeInterval = 60, until predicate: ([ControlClientEvent]) -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while !predicate(stored) {
            if !condition.wait(until: deadline) { return predicate(stored) }
        }
        return true
    }

    func waitForExit(timeout: TimeInterval = 60) -> ControlClientExit? {
        wait(timeout: timeout) { $0.contains { if case .exited = $0 { return true } else { return false } } }
        for event in events { if case .exited(let exit) = event { return exit } }
        return nil
    }
}

private func reply(_ send: (@escaping ControlClientV2.Completion) -> Void, timeout: TimeInterval = 60) -> Result<ControlV2Reply, ControlClientError>? {
    let semaphore = DispatchSemaphore(value: 0)
    var result: Result<ControlV2Reply, ControlClientError>?
    send { reply in
        result = reply
        semaphore.signal()
    }
    return semaphore.wait(timeout: .now() + timeout) == .success ? result : nil
}

/// A descriptor whose every copy can be counted: when all copies of the write end are closed the read end sees EOF.
private func makePipe() -> (read: Int32, write: Int32) {
    var ends: [Int32] = [0, 0]
    precondition(pipe(&ends) == 0)
    return (ends[0], ends[1])
}

private func seesEOF(_ readEnd: Int32) -> Bool {
    var item = pollfd(fd: readEnd, events: Int16(POLLIN), revents: 0)
    guard poll(&item, 1, 3_000) == 1 else { return false }
    var byte: UInt8 = 0
    return read(readEnd, &byte, 1) == 0
}

private func waitForFrames(_ host: FakeHost, _ count: Int, timeout: TimeInterval = 60) {
    let deadline = Date().addingTimeInterval(timeout)
    while host.received.count < count, Date() < deadline { Thread.sleep(forTimeInterval: 0.005) }
}

private func waitForFile(_ path: String, timeout: TimeInterval = 60) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while !FileManager.default.fileExists(atPath: path) {
        if Date() > deadline { return false }
        Thread.sleep(forTimeInterval: 0.01)
    }
    return true
}

private func temporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("od-client-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

// MARK: fake VMHost over a socketpair

/// The VMHost side of a control socketpair on its own thread: decodes requests with `ControlV2Decoder(direction: .towardHost)`
/// (descriptors with SCM_RIGHTS included) and answers as the test says.
private final class FakeHost {
    struct Received {
        var frame: ControlV2Frame
        var descriptor: Int32?
        var messageLength: Int
    }

    let fd: Int32
    private let lock = NSLock()
    private var frames: [Received] = []
    private var decoder = ControlV2Decoder(direction: .towardHost)
    /// Called on the host thread for every request frame.
    var onRequest: ((FakeHost, Received) -> Void)?
    var answerHello = true

    init(fd: Int32) { self.fd = fd }

    var received: [Received] {
        lock.lock()
        defer { lock.unlock() }
        return frames
    }

    func start() {
        let thread = Thread { [self] in run() }
        thread.start()
    }

    func send(_ frame: ControlV2Frame) { sendRaw(frame.encoded()) }

    func sendRaw(_ bytes: [UInt8]) {
        var offset = 0
        while offset < bytes.count {
            let written = bytes.withUnsafeBytes { Darwin.send(fd, $0.baseAddress! + offset, $0.count - offset, 0) }
            if written <= 0 { return }
            offset += written
        }
    }

    func sendWithDescriptor(_ bytes: [UInt8], descriptor: Int32) {
        var control = [UInt8](repeating: 0, count: 16)
        control.withUnsafeMutableBytes { raw in
            raw.storeBytes(of: UInt32(16), toByteOffset: 0, as: UInt32.self)
            raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
            raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
            raw.storeBytes(of: descriptor, toByteOffset: 12, as: Int32.self)
        }
        var data = bytes
        data.withUnsafeMutableBytes { dataBuffer in
            control.withUnsafeMutableBytes { controlBuffer in
                var iov = iovec(iov_base: dataBuffer.baseAddress, iov_len: dataBuffer.count)
                withUnsafeMutablePointer(to: &iov) { pointer in
                    var header = msghdr()
                    header.msg_iov = pointer
                    header.msg_iovlen = 1
                    header.msg_control = controlBuffer.baseAddress
                    header.msg_controllen = socklen_t(controlBuffer.count)
                    _ = Darwin.sendmsg(fd, &header, 0)
                }
            }
        }
    }

    func close() { _ = Darwin.shutdown(fd, SHUT_RDWR) }

    private func run() {
        var buffer = [UInt8](repeating: 0, count: ControlV2.maxFrameLength)
        while true {
            var control = [UInt8](repeating: 0, count: 64)
            var descriptor: Int32?
            let count = buffer.withUnsafeMutableBytes { dataBuffer in
                control.withUnsafeMutableBytes { controlBuffer -> Int in
                    var iov = iovec(iov_base: dataBuffer.baseAddress, iov_len: dataBuffer.count)
                    return withUnsafeMutablePointer(to: &iov) { pointer -> Int in
                        var header = msghdr()
                        header.msg_iov = pointer
                        header.msg_iovlen = 1
                        header.msg_control = controlBuffer.baseAddress
                        header.msg_controllen = socklen_t(controlBuffer.count)
                        let result = Darwin.recvmsg(fd, &header, 0)
                        if result > 0, header.msg_controllen >= 16,
                           controlBuffer.loadUnaligned(fromByteOffset: 8, as: Int32.self) == SCM_RIGHTS {
                            descriptor = controlBuffer.loadUnaligned(fromByteOffset: 12, as: Int32.self)
                        }
                        return result
                    }
                }
            }
            if count < 0, errno == EINTR { continue }
            if count <= 0 { break }
            let length = count
            if let descriptor { _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC) }
            decoder.append(Array(buffer[..<count]), descriptor: descriptor)
            while let item = try? decoder.nextFrame() {
                let entry = Received(frame: item.frame, descriptor: item.descriptor, messageLength: length)
                lock.lock()
                frames.append(entry)
                lock.unlock()
                if item.frame.kind == .hello {
                    if answerHello { send(readyFrame) }
                } else {
                    onRequest?(self, entry)
                }
            }
        }
        Darwin.close(fd)
    }
}

private struct Rig {
    let client: ControlClientV2
    let host: FakeHost
    let log: EventLog
}

private func makeRig(answerHello: Bool = true, onRequest: ((FakeHost, FakeHost.Received) -> Void)? = nil) throws -> Rig {
    let pair = try VMHostLaunch.makeSocketPair()
    let host = FakeHost(fd: pair.child)
    host.answerHello = answerHello
    host.onRequest = onRequest
    host.start()
    let log = EventLog()
    let client = ControlClientV2(socketFD: pair.parent, pid: nil, deliveryQueue: log.queue) { log.append($0) }
    client.begin()
    return Rig(client: client, host: host, log: log)
}

// MARK: scripts in place of the VMHost

private func writeScript(_ body: String, in directory: URL) throws -> String {
    let url = directory.appendingPathComponent("fake-vmhost-\(UUID().uuidString).sh")
    try Data("#!/bin/sh\n\(body)\n".utf8).write(to: url)
    chmod(url.path, 0o755)
    return url.path
}

/// `printf` of bytes to descriptor 3 (the control socket of the child).
private func printfToControl(_ bytes: [UInt8]) -> String {
    "printf '" + bytes.map { String(format: "\\%03o", $0) }.joined() + "' >&3"
}

private func launch(script: String, bundle: String, log: EventLog, display: Bool = false, grace: TimeInterval = 2.0) throws -> ControlClientV2 {
    try ControlClientV2.launchWithGrace(ControlClientLaunch(vmhostPath: script, bundlePath: bundle, displayChannel: display),
                                        deliveryQueue: log.queue, exitGrace: grace) { log.append($0) }
}

// MARK: 1. core

final class ControlClientV2XCTests: XCTestCase {
    private func readyCore() -> ControlV2ClientCore {
        var core = ControlV2ClientCore()
        _ = core.ingest(readyFrame.encoded())
        return core
    }

    func testHandshakeAndTheOrderBeforeReady() throws {
        var core = ControlV2ClientCore()
        XCTAssertThrowsError(try core.prepare(kind: .query)) { XCTAssertEqual($0 as? ControlClientError, .notReady) }
        let actions = core.ingest(readyFrame.encoded())
        XCTAssertEqual(actions.events, [.ready])
        XCTAssertEqual(core.phase, .ready)
        XCTAssertEqual(core.state, .stopped, "after READY the machine is stopped (section 4 item 5)")
        // Anything but READY or the start ERROR before READY breaks the order.
        var early = ControlV2ClientCore()
        XCTAssertEqual(early.ingest(changed(.stopped, .running).encoded()).violations.count, 1)
        // A repeated READY is a violation.
        var again = readyCore()
        XCTAssertEqual(again.ingest(readyFrame.encoded()).violations.count, 1)
        // READY with a request id.
        var withID = ControlV2ClientCore()
        XCTAssertEqual(withID.ingest(frame(.ready(profile: 2), id: 3).encoded()).violations.count, 1)
    }

    func testStartRejectionBusyAndTheOtherLockOutcomes() {
        func rejection(_ code: ControlV2ErrorCode, aux0: UInt64 = 0) -> StartRejection? {
            var core = ControlV2ClientCore()
            let report = ControlV2ErrorReport(code: code, failedKind: 0, flags: ControlV2ErrorReport.flagFatal, state: 1, aux0: aux0)
            let actions = core.ingest(frame(.error(report)).encoded())
            XCTAssertEqual(actions.violations, [])
            return core.startRejection
        }
        XCTAssertEqual(rejection(.busy), .busy)
        XCTAssertEqual(rejection(.lockChanged), .lockChanged)
        XCTAssertEqual(rejection(.lockInvalid), .lockInvalid)
        XCTAssertEqual(rejection(.ioError, aux0: 13), .ioError(errno: 13))
        XCTAssertEqual(rejection(.hostFailure), .other(code: 25))
        var core = ControlV2ClientCore()
        XCTAssertEqual(core.ingest(errorFrame(.busy, kind: .hello, id: 0, fatal: true).encoded()).violations.count, 1, "failed_kind is 0 at the start")
        var notFatal = ControlV2ClientCore()
        XCTAssertEqual(notFatal.ingest(errorFrame(.busy, kind: nil, id: 0, fatal: false).encoded()).violations.count, 1)
        var withID = ControlV2ClientCore()
        XCTAssertEqual(withID.ingest(errorFrame(.busy, kind: nil, id: 4, fatal: true).encoded()).violations.count, 1)
    }

    func testRequestIDsAndTheRulesOfTheSender() throws {
        var core = readyCore()
        XCTAssertEqual(try core.prepare(kind: .configure), 1)
        XCTAssertThrowsError(try core.prepare(kind: .start)) { XCTAssertEqual($0 as? ControlClientError, .requestInFlight) }
        XCTAssertEqual(try core.prepare(kind: .query), 2, "QUERY may be sent beside a request in flight")
        _ = core.ingest(ack(.configure, state: .stopped, id: 1).encoded())
        XCTAssertEqual(try core.prepare(kind: .start), 3, "IDs go 1, 2, 3 without gaps")
        core.cancel(3)
        // ATTACH_RESOURCE only when nothing else is in flight, and then nothing until its answer (section 8).
        XCTAssertThrowsError(try core.prepare(kind: .attachResource)) { XCTAssertEqual($0 as? ControlClientError, .requestInFlight) }
        _ = core.ingest(frame(.state(ControlV2StateReport(state: 1, flags: 1, lastReason: 0)), id: 2).encoded())
        let attach = try core.prepare(kind: .attachResource)
        XCTAssertEqual(attach, 4, "a cancelled request still used its ID")
        XCTAssertThrowsError(try core.prepare(kind: .query)) { XCTAssertEqual($0 as? ControlClientError, .requestInFlight) }
        _ = core.ingest(ack(.attachResource, state: .stopped, id: attach).encoded())
        XCTAssertEqual(try core.prepare(kind: .start), 5)
    }

    func testAckAndStateAnswerTheRequestInFlightOnly() throws {
        var core = readyCore()
        let configure = try core.prepare(kind: .configure)
        let actions = core.ingest(ack(.configure, state: .stopped, id: configure).encoded())
        XCTAssertEqual(actions.completions.count, 1)
        XCTAssertEqual(actions.completions[0].0, configure)
        XCTAssertEqual(actions.completions[0].1, .success(.ack(ControlV2Ack(ackedKind: 3, state: 1))))
        XCTAssertTrue(core.pending.isEmpty)
        // An answer for a request that is not in flight, of another kind, `in_progress` (the client never repeats), STATE for a non-QUERY.
        XCTAssertEqual(core.ingest(ack(.configure, state: .stopped, id: configure).encoded()).violations.count, 1)
        var other = readyCore()
        let start = try other.prepare(kind: .start)
        XCTAssertEqual(other.ingest(ack(.pause, state: .stopped, id: start).encoded()).violations.count, 1)
        var progress = readyCore()
        let id = try progress.prepare(kind: .start)
        XCTAssertEqual(progress.ingest(frame(.ack(ControlV2Ack(code: ControlV2Ack.codeInProgress, ackedKind: 6, state: 1)), id: id).encoded()).violations.count, 1)
        var wrongState = readyCore()
        let startID = try wrongState.prepare(kind: .start)
        XCTAssertEqual(wrongState.ingest(frame(.state(ControlV2StateReport(state: 1, flags: 0, lastReason: 0)), id: startID).encoded()).violations.count, 1)
    }

    func testStateChangedFollowsTheTableOfTransitions() throws {
        var core = readyCore()
        let start = try core.prepare(kind: .start)
        var actions = core.ingest(changed(.stopped, .running, .user, id: start).encoded())
        XCTAssertEqual(actions.events, [.stateChanged(from: .stopped, to: .running, reason: 1, requestID: start)])
        XCTAssertEqual(core.state, .running)
        _ = core.ingest(ack(.start, state: .running, id: start).encoded())
        actions = core.ingest(changed(.running, .paused, .systemSleep).encoded())
        XCTAssertEqual(actions.events, [.stateChanged(from: .running, to: .paused, reason: 2, requestID: 0)])
        actions = core.ingest(changed(.paused, .running, .systemWake).encoded())
        XCTAssertEqual(core.state, .running)
        actions = core.ingest(changed(.running, .stopped, .guestShutdown).encoded())
        XCTAssertEqual(core.state, .stopped)
        // The sequence of events: the frames arrive in one read and in single bytes with the same result.
        let frames = [changed(.stopped, .running), changed(.running, .paused), changed(.paused, .stopped), changed(.stopped, .running),
                      changed(.running, .failed, .hostFailure)].flatMap { $0.encoded() }
        var whole = readyCore(), bytewise = readyCore()
        let all = whole.ingest(frames).events
        var split: [ControlClientEvent] = []
        for byte in frames { split += bytewise.ingest([byte]).events }
        XCTAssertEqual(all.count, 5)
        XCTAssertEqual(all, split)
        XCTAssertEqual(whole.state, .failed)
    }

    func testAnImpossibleTransitionBreaksTheProtocol() throws {
        for (from, to) in [(ControlV2State.stopped, ControlV2State.paused), (.paused, .shuttingDown), (.stopped, .stopped), (.failed, .stopped)] {
            XCTAssertFalse(ControlV2Transitions.isAllowed(from: from, to: to), "\(from) -> \(to)")
        }
        var core = readyCore()
        XCTAssertEqual(core.ingest(changed(.stopped, .paused).encoded()).violations.count, 1)
        var mismatch = readyCore() // the client knows `stopped`; the VMHost says it left `running`
        XCTAssertEqual(mismatch.ingest(changed(.running, .paused).encoded()).violations.count, 1)
        var failed = readyCore()
        _ = failed.ingest(changed(.stopped, .failed, .hostFailure).encoded())
        // After a violation nothing more is read.
        XCTAssertEqual(failed.ingest(changed(.failed, .stopped).encoded()).violations.count, 1)
        XCTAssertEqual(failed.ingest(changed(.stopped, .running).encoded()).events, [])
    }

    func testErrorAnswersTheRequestAndIsAnEventToo() throws {
        var core = readyCore()
        let start = try core.prepare(kind: .start)
        let actions = core.ingest(errorFrame(.notConfigured, kind: .start, id: start).encoded())
        XCTAssertEqual(actions.violations, [])
        guard case .error(let report, let id)? = actions.events.first else { return XCTFail("expected an error event") }
        XCTAssertEqual(id, start)
        XCTAssertEqual(report.code, ControlV2ErrorCode.notConfigured.rawValue)
        guard case .failure(.refused(let refused)) = actions.completions[0].1 else { return XCTFail("expected refused") }
        XCTAssertEqual(refused, report)
        XCTAssertTrue(core.pending.isEmpty)
        // An error that no request caused (id 0) is only an event; a fatal one says that the VMHost is going away.
        let unsolicited = core.ingest(errorFrame(.hostFailure, kind: nil, id: 0, fatal: true, state: .failed).encoded())
        guard case .error(let fatal, 0)? = unsolicited.events.first else { return XCTFail("expected an error event") }
        XCTAssertTrue(fatal.fatal)
        XCTAssertEqual(unsolicited.completions.count, 0)
        // A code that this client does not know is still delivered (section 6.7).
        let unknown = core.ingest(frame(.error(ControlV2ErrorReport(code: 999, failedKind: 0, state: 1))).encoded())
        guard case .error(let report999, _)? = unknown.events.first else { return XCTFail("expected an error event") }
        XCTAssertNil(ControlV2ErrorCode(rawValue: report999.code))
    }

    func testStatsProgressAndClosingTheConnection() throws {
        var core = readyCore()
        let first = try core.prepare(kind: .start)
        let second = try core.prepare(kind: .query)
        var actions = core.ingest(frame(.stats(ControlV2Stats(state: 2, pollCount: 5, framesPublished: 1, rssBytes: 99))).encoded())
        XCTAssertEqual(actions.events, [.stats(ControlV2Stats(state: 2, pollCount: 5, framesPublished: 1, rssBytes: 99))])
        actions = core.ingest(frame(.progress(requestKind: 5, completed: 1, total: 2), id: 7).encoded())
        XCTAssertEqual(actions.events, [.progress(requestKind: 5, completed: 1, total: 2)])
        // The connection ends: every request in flight fails, in the order of the IDs.
        let closed = core.close()
        XCTAssertEqual(closed.completions.map { $0.0 }, [first, second])
        for item in closed.completions { XCTAssertEqual(item.1, .failure(.connectionClosed)) }
        XCTAssertThrowsError(try core.prepare(kind: .query)) { XCTAssertEqual($0 as? ControlClientError, .connectionClosed) }
    }

    func testStructureViolationsOfTheStreamAreReported() throws {
        // Garbage, a request kind from the VMHost, an unknown kind, a wrong version, a descriptor, CLIPBOARD_CHANGED, a truncated frame.
        let hello = ControlV2Frame(kind: .hello, requestID: 0).encoded()
        var wrongVersion = readyFrame.encoded()
        wrongVersion[4] = 1
        var unknownKind = readyFrame.encoded()
        unknownKind[6] = 99
        let cases: [[UInt8]] = [Array("not a frame at all.".utf8), hello, unknownKind, wrongVersion,
                                frame(.clipboardChanged([1])).encoded()]
        for bytes in cases {
            var core = readyCore()
            XCTAssertEqual(core.ingest(bytes).violations.count, 1, "\(bytes.prefix(8))")
        }
        var truncated = readyCore()
        XCTAssertEqual(truncated.ingest(Array(changed(.stopped, .running).encoded().dropLast(3))).violations, [])
        XCTAssertEqual(truncated.endOfStream().violations.count, 1, "EOF inside a frame")
        var clean = readyCore()
        XCTAssertEqual(clean.endOfStream().violations, [], "EOF between frames is not a violation")
        // READY with a profile other than 2 and non-zero reserved bytes.
        var badReady = ControlV2ClientCore()
        XCTAssertEqual(badReady.ingest(frame(.ready(profile: 3)).encoded()).violations.count, 1)
        // A complete frame before the broken one is still processed.
        var partial = readyCore()
        let mixed = partial.ingest(changed(.stopped, .running).encoded() + Array("garbage garbage garbage garbage".utf8))
        XCTAssertEqual(mixed.events.count, 1)
        XCTAssertEqual(mixed.violations.count, 1)
    }

    func testExitStatusMeaning() {
        func exit(_ termination: ControlClientExit.Termination, _ rejection: StartRejection? = nil) -> ControlClientExit {
            ControlClientExit(termination: termination, startRejection: rejection)
        }
        XCTAssertEqual(exit(.exited(0)).disposition, .stopped)
        XCTAssertEqual(exit(.exited(2)).disposition, .usageError)
        XCTAssertEqual(exit(.exited(3), .busy).disposition, .startRejected)
        XCTAssertTrue(exit(.exited(3), .busy).isBusy)
        XCTAssertFalse(exit(.exited(3), .lockChanged).isBusy)
        XCTAssertEqual(exit(.exited(4)).disposition, .protocolError)
        XCTAssertEqual(exit(.exited(5)).disposition, .transportLost)
        XCTAssertEqual(exit(.exited(6)).disposition, .hostFailed)
        XCTAssertEqual(exit(.exited(77)).disposition, .unexpected)
        XCTAssertEqual(exit(.signaled(9)).disposition, .killedBySignal)
        XCTAssertNotEqual(exit(.signaled(5)), exit(.exited(133)), "a signal is not folded into an exit code")
        XCTAssertEqual(exit(.unavailable).disposition, .unavailable)
    }

    // MARK: 2. client over a socketpair with a fake VMHost

    func testLifecycleOverTheSocketAndTheOrderOfEventsAndAnswers() throws {
        let rig = try makeRig { host, request in
            let id = request.frame.requestID
            switch request.frame.kind {
            case .configure: host.send(ack(.configure, state: .stopped, id: id))
            case .start:
                host.send(changed(.stopped, .running, .user, id: id))
                host.send(ack(.start, state: .running, id: id))
            case .pause:
                host.send(changed(.running, .paused, .user, id: id))
                host.send(ack(.pause, state: .paused, id: id))
            case .resume:
                host.send(changed(.paused, .running, .user, id: id))
                host.send(ack(.resume, state: .running, id: id))
            case .query:
                host.send(frame(.state(ControlV2StateReport(state: 2, flags: 1, lastReason: 1)), id: id))
            case .stop:
                host.send(changed(.running, .stopped, .user, id: id))
                host.send(ack(.stop, state: .stopped, id: id))
                host.close()
            default: break
            }
        }
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        XCTAssertEqual(reply { rig.client.configure(BundleConfig(name: "m").controlConfigure(), completion: $0) },
                       .success(.ack(ControlV2Ack(ackedKind: 3, state: 1))))
        XCTAssertEqual(reply { rig.client.start(completion: $0) }, .success(.ack(ControlV2Ack(ackedKind: 6, state: 2))))
        XCTAssertEqual(reply { rig.client.query(completion: $0) }, .success(.state(ControlV2StateReport(state: 2, flags: 1, lastReason: 1))))
        XCTAssertEqual(reply { rig.client.pause(completion: $0) }, .success(.ack(ControlV2Ack(ackedKind: 7, state: 3))))
        XCTAssertEqual(reply { rig.client.resume(completion: $0) }, .success(.ack(ControlV2Ack(ackedKind: 8, state: 2))))
        XCTAssertEqual(reply { rig.client.stop(completion: $0) }, .success(.ack(ControlV2Ack(ackedKind: 9, state: 1))))
        let exit = rig.log.waitForExit()
        XCTAssertEqual(exit, ControlClientExit(termination: .unavailable, startRejection: nil))
        // The events, in order: the state of the row comes from these, then the end of the connection.
        XCTAssertEqual(rig.log.events, [
            .ready,
            .stateChanged(from: .stopped, to: .running, reason: 1, requestID: 2),
            .stateChanged(from: .running, to: .paused, reason: 1, requestID: 4),
            .stateChanged(from: .paused, to: .running, reason: 1, requestID: 5),
            .stateChanged(from: .running, to: .stopped, reason: 1, requestID: 6),
            .exited(ControlClientExit(termination: .unavailable, startRejection: nil)),
        ])
        // On the wire: HELLO with id 0, then the requests with the IDs 1, 2, 3, ... and the right payloads.
        let wire = rig.host.received.map { ($0.frame.kind, $0.frame.requestID) }
        XCTAssertEqual(wire.map { $0.0 }, [.hello, .configure, .start, .query, .pause, .resume, .stop])
        XCTAssertEqual(wire.map { $0.1 }, [0, 1, 2, 3, 4, 5, 6])
        XCTAssertEqual(rig.host.received[1].frame.payload.count, 1588)
        XCTAssertEqual(rig.host.received[4].frame.payload, [1, 0, 0, 0, 0, 0, 0, 0], "PAUSE reason user")
        XCTAssertEqual(rig.host.received[2].frame.payload, [UInt8](repeating: 0, count: 8), "operation id 0")
    }

    func testTheClientNeverPollsTheVMHost() throws {
        let rig = try makeRig()
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        Thread.sleep(forTimeInterval: 0.6) // no timer asks for the state
        XCTAssertEqual(rig.host.received.map { $0.frame.kind }, [.hello])
        rig.host.close()
        XCTAssertNotNil(rig.log.waitForExit())
    }

    func testARefusalIsAFailureAndAnEvent() throws {
        let rig = try makeRig { host, request in
            host.send(errorFrame(.notConfigured, kind: request.frame.kind, id: request.frame.requestID))
        }
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        let result = reply { rig.client.start(completion: $0) }
        guard case .failure(.refused(let report))? = result else { return XCTFail("expected refused, got \(String(describing: result))") }
        XCTAssertEqual(report.code, ControlV2ErrorCode.notConfigured.rawValue)
        XCTAssertEqual(report.failedKind, ControlV2Kind.start.rawValue)
        XCTAssertTrue(rig.log.wait { $0.contains { if case .error(_, 1) = $0 { return true } else { return false } } })
        // The connection lives on: the next request is sent with the next ID.
        _ = reply { rig.client.query(completion: $0) }
        XCTAssertEqual(rig.host.received.last?.frame.requestID, 2)
        rig.client.terminate()
    }

    func testASecondRequestWhileOneIsInFlightIsRefusedLocally() throws {
        let rig = try makeRig() // the fake host never answers
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        var first: Result<ControlV2Reply, ControlClientError>?
        rig.client.start { first = $0 }
        let second = reply { rig.client.pause(completion: $0) }
        XCTAssertEqual(second, .failure(.requestInFlight))
        waitForFrames(rig.host, 2)
        XCTAssertEqual(rig.host.received.map { $0.frame.kind }, [.hello, .start], "the refused request is not sent")
        let notReady = try makeRig(answerHello: false)
        XCTAssertEqual(reply { notReady.client.start(completion: $0) }, .failure(.notReady))
        // The end of the connection fails the request in flight.
        rig.host.close()
        XCTAssertNotNil(rig.log.waitForExit())
        let deadline = Date().addingTimeInterval(5)
        while first == nil, Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
        XCTAssertEqual(first, .failure(.connectionClosed))
        notReady.client.terminate()
    }

    func testConfigureIsCheckedBeforeItIsSent() throws {
        let rig = try makeRig()
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        var bad = BundleConfig(name: "x").controlConfigure()
        bad.memoryMiB = 100
        guard case .failure(.invalidArgument)? = reply({ rig.client.configure(bad, completion: $0) }) else { return XCTFail("expected invalidArgument") }
        XCTAssertEqual(rig.host.received.map { $0.frame.kind }, [.hello])
        XCTAssertEqual(reply { rig.client.send(.hello, completion: $0) }, .failure(.invalidArgument("HELLO is sent by the client itself")))
        rig.client.terminate()
    }

    func testAttachResourceSendsOneFrameWithExactlyOneDescriptorAndClosesItsCopy() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("boot.iso")
        try Data("iso".utf8).write(to: file)
        let rig = try makeRig { host, request in
            host.send(ack(.attachResource, state: .stopped, id: request.frame.requestID))
        }
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        let descriptor = Darwin.open(file.path, O_RDONLY)
        XCTAssertGreaterThanOrEqual(descriptor, 0)
        let result = reply { rig.client.attachResource(role: .bootMedia, index: 0, descriptor: descriptor, completion: $0) }
        XCTAssertEqual(result, .success(.ack(ControlV2Ack(ackedKind: 4, state: 1))))
        let attach = rig.host.received.last
        XCTAssertEqual(attach?.frame.kind, .attachResource)
        XCTAssertEqual(attach?.messageLength, 24, "the whole frame is one message with the descriptor")
        XCTAssertEqual(attach?.frame.payload, [1, 0, 0, 0, 0, 0, 0, 0], "role boot_media, index 0, reserved 0")
        let received = try XCTUnwrap(attach?.descriptor)
        var sent = stat(), arrived = stat()
        XCTAssertEqual(stat(file.path, &sent), 0)
        XCTAssertEqual(fstat(received, &arrived), 0)
        XCTAssertEqual(sent.st_ino, arrived.st_ino, "the VMHost end holds the same file")
        Darwin.close(received)
        // The sender closes its copy after the frame was sent (section 8): once the receiver's copy is closed too, the pipe is at EOF.
        let pipeEnds = makePipe()
        XCTAssertEqual(reply { rig.client.attachResource(role: .disk, index: 1, descriptor: pipeEnds.write, completion: $0) },
                       .success(.ack(ControlV2Ack(ackedKind: 4, state: 1))))
        XCTAssertEqual(rig.host.received.last?.frame.payload, [2, 0, 1, 0, 0, 0, 0, 0], "role disk, index 1")
        Darwin.close(try XCTUnwrap(rig.host.received.last?.descriptor))
        XCTAssertTrue(seesEOF(pipeEnds.read), "the client closed its copy")
        Darwin.close(pipeEnds.read)
        // A descriptor with another kind, or none with ATTACH_RESOURCE, is refused locally, and the descriptor is closed.
        let spare = makePipe()
        guard case .failure(.invalidArgument)? = reply({ rig.client.send(.query, descriptor: spare.write, completion: $0) }) else { return XCTFail() }
        XCTAssertTrue(seesEOF(spare.read))
        Darwin.close(spare.read)
        guard case .failure(.invalidArgument)? = reply({ rig.client.send(.attachResource(ControlV2Attach(role: .disk, index: 0)), completion: $0) }) else {
            return XCTFail()
        }
        XCTAssertEqual(rig.host.received.count, 3, "HELLO and the two attaches; the refused ones were not sent")
        rig.client.terminate()
    }

    func testAttachResourceAfterARequestInFlightIsRefusedAndTheDescriptorClosed() throws {
        let rig = try makeRig() // no answers
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        rig.client.start { _ in }
        let pipeEnds = makePipe()
        XCTAssertEqual(reply { rig.client.attachResource(role: .disk, index: 1, descriptor: pipeEnds.write, completion: $0) }, .failure(.requestInFlight))
        XCTAssertTrue(seesEOF(pipeEnds.read), "the descriptor of a refused ATTACH_RESOURCE is closed")
        Darwin.close(pipeEnds.read)
        waitForFrames(rig.host, 2)
        XCTAssertEqual(rig.host.received.map { $0.frame.kind }, [.hello, .start])
        rig.client.terminate()
    }

    func testALostConnectionEndsRequestsWithConnectionClosedNeverWithIO() throws {
        let rig = try makeRig() // no answers
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        var inFlight: Result<ControlV2Reply, ControlClientError>?
        let done = DispatchSemaphore(value: 0)
        rig.client.start { inFlight = $0; done.signal() }
        waitForFrames(rig.host, 2)
        _ = Darwin.shutdown(rig.host.fd, SHUT_RDWR)
        XCTAssertEqual(done.wait(timeout: .now() + 60), .success)
        XCTAssertEqual(inFlight, .failure(.connectionClosed))
        XCTAssertNotNil(rig.log.waitForExit())
        // A request after the loss: whether the write fails or the client is finished already, the answer is the same.
        XCTAssertEqual(reply { rig.client.query(completion: $0) }, .failure(.connectionClosed))
        let pipeEnds = makePipe()
        XCTAssertEqual(reply { rig.client.attachResource(role: .disk, index: 0, descriptor: pipeEnds.write, completion: $0) }, .failure(.connectionClosed))
        XCTAssertTrue(seesEOF(pipeEnds.read), "the descriptor is closed on this path too")
        Darwin.close(pipeEnds.read)
    }

    func testStatsAndProgressAreEvents() throws {
        let rig = try makeRig()
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        rig.host.send(frame(.stats(ControlV2Stats(state: 2, pollCount: 10, framesPublished: 3, rssBytes: 4))))
        rig.host.send(frame(.progress(requestKind: 5, completed: 1, total: 2), id: 9))
        XCTAssertTrue(rig.log.wait { $0.count >= 3 })
        XCTAssertEqual(Array(rig.log.events.prefix(3)), [
            .ready, .stats(ControlV2Stats(state: 2, pollCount: 10, framesPublished: 3, rssBytes: 4)),
            .progress(requestKind: 5, completed: 1, total: 2),
        ])
        XCTAssertNil(rig.client.takeDisplaySocket(), "no display channel without a process")
        rig.client.terminate()
    }

    func testAProtocolViolationClosesTheConnectionAndFailsTheRequests() throws {
        let rig = try makeRig { host, _ in
            host.sendRaw(Array("garbage garbage garbage garbage!".utf8)) // answer to the first request
        }
        XCTAssertTrue(rig.log.wait { $0.contains(.ready) })
        let result = reply { rig.client.start(completion: $0) }
        XCTAssertEqual(result, .failure(.connectionClosed))
        let exit = rig.log.waitForExit()
        XCTAssertNotNil(exit)
        let events = rig.log.events
        XCTAssertTrue(events.contains { if case .protocolViolation = $0 { return true } else { return false } })
        XCTAssertEqual(events.filter { if case .exited = $0 { return true } else { return false } }.count, 1, "exited comes once")
        if case .exited? = events.last {} else { XCTFail("exited is the last event") }
    }

    func testADescriptorFromTheVMHostAndATruncatedFrameAreViolations() throws {
        let withDescriptor = try makeRig()
        XCTAssertTrue(withDescriptor.log.wait { $0.contains(.ready) })
        let spare = Darwin.open("/dev/null", O_RDONLY)
        withDescriptor.host.sendWithDescriptor(changed(.stopped, .running).encoded(), descriptor: spare)
        Darwin.close(spare)
        XCTAssertTrue(withDescriptor.log.wait { $0.contains { if case .protocolViolation = $0 { return true } else { return false } } })
        XCTAssertFalse(withDescriptor.log.events.contains { if case .stateChanged = $0 { return true } else { return false } })
        XCTAssertNotNil(withDescriptor.log.waitForExit())

        let truncated = try makeRig()
        XCTAssertTrue(truncated.log.wait { $0.contains(.ready) })
        truncated.host.sendRaw(Array(changed(.stopped, .running).encoded().dropLast(4)))
        truncated.host.close()
        XCTAssertTrue(truncated.log.wait { $0.contains { if case .protocolViolation = $0 { return true } else { return false } } })
        XCTAssertNotNil(truncated.log.waitForExit())
    }

    // MARK: 3. process layer with scripts

    func testBusyIsAStartRejectionWithExitStatus3() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let busy = ControlV2Frame(response: .error(ControlV2ErrorReport(
            code: .busy, failedKind: 0, flags: ControlV2ErrorReport.flagFatal, state: 1, detail: "bundle lock is held")), requestID: 0).encoded()
        let script = try writeScript("\(printfToControl(busy))\nexit 3", in: directory)
        let log = EventLog()
        let client = try launch(script: script, bundle: directory.path, log: log)
        let exit = log.waitForExit()
        XCTAssertEqual(exit, ControlClientExit(termination: .exited(3), startRejection: .busy))
        XCTAssertEqual(exit?.isBusy, true)
        XCTAssertEqual(exit?.disposition, .startRejected)
        XCTAssertEqual(log.events, [.startRejected(.busy), .exited(ControlClientExit(termination: .exited(3), startRejection: .busy))],
                       "no `ready`, no violation: busy is a result of its own")
        XCTAssertEqual(reply { client.start(completion: $0) }, .failure(.connectionClosed))
    }

    func testExitStatusBySignalIsNotAnExitCode() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let script = try writeScript("kill -KILL $$", in: directory)
        let log = EventLog()
        _ = try launch(script: script, bundle: directory.path, log: log)
        XCTAssertEqual(log.waitForExit(), ControlClientExit(termination: .signaled(SIGKILL), startRejection: nil))
        XCTAssertEqual(log.waitForExit()?.disposition, .killedBySignal)
    }

    func testExitCodesOfTheProcessAreReported() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        for (code, disposition) in [(0, ControlClientExit.Disposition.stopped), (2, .usageError), (5, .transportLost), (6, .hostFailed), (77, .unexpected)] {
            let log = EventLog()
            _ = try launch(script: try writeScript("exit \(code)", in: directory), bundle: directory.path, log: log)
            let exit = log.waitForExit()
            XCTAssertEqual(exit?.termination, .exited(Int32(code)))
            XCTAssertEqual(exit?.disposition, disposition)
        }
    }

    func testTerminateFollowsTheTeardownPolicyAndSIGKILLsAProcessThatIgnoresSIGTERM() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        // The shell ignores SIGTERM and keeps running; its short `sleep` children end by themselves after the shell was killed.
        // (The first start of a new script can take a few hundred ms on macOS: the script says when it is ready.)
        let marker = directory.appendingPathComponent("ignoring").path
        let script = try writeScript("trap '' TERM\n: > '\(marker)'\nwhile :; do sleep 1; done", in: directory)
        let log = EventLog()
        let client = try launch(script: script, bundle: directory.path, log: log)
        XCTAssertTrue(waitForFile(marker))
        let started = Date()
        client.terminate()
        let exit = log.waitForExit()
        let elapsed = Date().timeIntervalSince(started)
        XCTAssertEqual(exit?.termination, .signaled(SIGKILL))
        XCTAssertGreaterThanOrEqual(elapsed, 0.45, "SIGTERM first, SIGKILL after 500 ms")
        XCTAssertLessThan(elapsed, 5)
        // A process that obeys SIGTERM ends at once.
        let plainMarker = directory.appendingPathComponent("plain").path
        let plain = try writeScript(": > '\(plainMarker)'\nexec sleep 30", in: directory)
        let second = EventLog()
        let other = try launch(script: plain, bundle: directory.path, log: second)
        XCTAssertTrue(waitForFile(plainMarker))
        other.terminate()
        XCTAssertEqual(second.waitForExit()?.termination, .signaled(SIGTERM))
    }

    func testAProcessThatDoesNotExitAfterEOFIsTerminatedAfterTheGrace() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let script = try writeScript("exec 3>&-\nexec sleep 30", in: directory) // closes the control socket, stays alive
        let log = EventLog()
        _ = try launch(script: script, bundle: directory.path, log: log, grace: 0.3)
        XCTAssertEqual(log.waitForExit()?.termination, .signaled(SIGTERM))
    }

    func testLaunchFailureIsAThrowAndLeaksNoDescriptor() throws {
        func openDescriptors() -> Int { (0..<256).filter { fcntl(Int32($0), F_GETFD) != -1 }.count }
        let before = openDescriptors()
        XCTAssertThrowsError(try ControlClientV2.launch(
            ControlClientLaunch(vmhostPath: "/nonexistent/od-vmhost", bundlePath: "/tmp"), deliveryQueue: EventLog().queue) { _ in })
        XCTAssertEqual(openDescriptors(), before)
    }

    func testOnlyTheNamedDescriptorsReachTheVMHost() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        // The child lists the descriptors 3...63 that it has open. A holder keeps another machine's parent ends alive in this process.
        let output = directory.appendingPathComponent("fds.txt").path
        let script = try writeScript("i=3; while [ $i -le 63 ]; do [ -e /dev/fd/$i ] && echo $i >> '\(output)'; i=$((i+1)); done\nexit 0", in: directory)
        let marker = directory.appendingPathComponent("holder").path
        let holder = try launch(script: try writeScript(": > '\(marker)'\nexec sleep 30", in: directory), bundle: directory.path, log: EventLog(), display: true)
        XCTAssertTrue(waitForFile(marker))
        let log = EventLog()
        _ = try launch(script: script, bundle: directory.path, log: log, display: true)
        XCTAssertNotNil(log.waitForExit())
        let listed = try String(contentsOfFile: output, encoding: .utf8).split(separator: "\n").compactMap { Int($0) }.sorted()
        XCTAssertEqual(listed, [3, 5], "control on 3 and the frame transport on 5, nothing else (no input channel was asked for)")
        holder.terminate()
    }

    // MARK: 4. the real VMHost

    private func vmhostPath() -> String? {
        let candidates = [
            Bundle(for: Self.self).bundleURL.deletingLastPathComponent().appendingPathComponent("od-vmhost").path,
            URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent(".build/debug/od-vmhost").path,
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    func testRealVMHostLifecycleBusyAndExitStatus() throws {
        guard let vmhost = vmhostPath() else { throw XCTSkip("od-vmhost is not built next to the tests (run `swift build` first)") }
        let bundle = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: bundle) }

        let log = EventLog()
        let client = try ControlClientV2.launch(ControlClientLaunch(vmhostPath: vmhost, bundlePath: bundle.path, displayChannel: true),
                                                deliveryQueue: log.queue) { log.append($0) }
        XCTAssertTrue(log.wait { $0.contains(.ready) }, "no READY: \(log.events)")
        XCTAssertNotNil(client.takeDisplaySocket(), "the frame-transport socket of descriptor 5")
        XCTAssertNil(client.takeDisplaySocket(), "ownership moves once")

        // The bundle lock is held by this VMHost: a second one on the same bundle answers `busy` and exits with status 3.
        let secondLog = EventLog()
        _ = try ControlClientV2.launch(ControlClientLaunch(vmhostPath: vmhost, bundlePath: bundle.path, displayChannel: false),
                                       deliveryQueue: secondLog.queue) { secondLog.append($0) }
        let busyExit = secondLog.waitForExit()
        XCTAssertEqual(busyExit, ControlClientExit(termination: .exited(3), startRejection: .busy), "\(secondLog.events)")
        XCTAssertEqual(BundleWriterLock.probeFailure(bundle: bundle), .busy, "the first VMHost holds the lock")

        // The lifecycle on a test-only profile. The row of a library changes by these events only.
        XCTAssertEqual(reply { client.start(completion: $0) }.flatMapError, ControlV2ErrorCode.notConfigured.rawValue)
        XCTAssertEqual(reply { client.configure(BundleConfig(name: "m").controlConfigure(), completion: $0) },
                       .success(.ack(ControlV2Ack(ackedKind: 3, state: 1))))
        guard case .success(.ack(let started))? = reply({ client.start(completion: $0) }) else { return XCTFail("START failed") }
        XCTAssertEqual(started.state, ControlV2State.running.rawValue)
        guard case .success(.state(let running))? = reply({ client.query(completion: $0) }) else { return XCTFail("QUERY failed") }
        XCTAssertEqual(running.state, ControlV2State.running.rawValue)
        XCTAssertTrue(log.wait { $0.contains { if case .stats = $0 { return true } else { return false } } }, "STATS arrive by themselves, once a second")
        guard case .success? = reply({ client.pause(completion: $0) }) else { return XCTFail("PAUSE failed") }
        guard case .success? = reply({ client.resume(completion: $0) }) else { return XCTFail("RESUME failed") }
        guard case .success? = reply({ client.stop(completion: $0) }) else { return XCTFail("STOP failed") }
        XCTAssertEqual(log.waitForExit(), ControlClientExit(termination: .exited(0), startRejection: nil))
        let states = log.events.compactMap { event -> [ControlV2State]? in
            if case .stateChanged(let from, let to, _, _) = event { return [from, to] } else { return nil }
        }
        XCTAssertEqual(states, [[.stopped, .running], [.running, .paused], [.paused, .running], [.running, .stopped]])
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundle), "the lock is free after the exit")
    }

    func testRealVMHostRefusesAMachineWithADiskAndKilledHostIsASignal() throws {
        guard let vmhost = vmhostPath() else { throw XCTSkip("od-vmhost is not built next to the tests (run `swift build` first)") }
        let bundle = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: bundle) }
        let log = EventLog()
        let client = try ControlClientV2.launch(ControlClientLaunch(vmhostPath: vmhost, bundlePath: bundle.path, displayChannel: false),
                                                deliveryQueue: log.queue) { log.append($0) }
        XCTAssertTrue(log.wait { $0.contains(.ready) })
        var config = BundleConfig(name: "m")
        config.disks = [BundleDisk(location: .inBundle(name: "disk0.img"))]
        XCTAssertEqual(reply { client.configure(config.controlConfigure(), completion: $0) }, .success(.ack(ControlV2Ack(ackedKind: 3, state: 1))))
        // VM_BUNDLE 3.4 item 4: a non-empty `disks` answers `unsupported_durability` until S9-06.
        XCTAssertEqual(reply { client.start(completion: $0) }.flatMapError, ControlV2ErrorCode.unsupportedDurability.rawValue)
        XCTAssertFalse(log.events.contains { if case .stateChanged = $0 { return true } else { return false } }, "a refusal changes no state")
        client.terminate()
        let exit = log.waitForExit()
        XCTAssertNotNil(exit)
        if case .signaled(let number)? = exit?.termination {
            XCTAssertTrue(number == SIGTERM || number == SIGKILL)
        } else if case .exited(let code)? = exit?.termination {
            XCTAssertTrue([0, 5].contains(code), "VMHost ended by itself with status \(code)")
        } else {
            XCTFail("no exit status")
        }
    }
}

private extension Optional where Wrapped == Result<ControlV2Reply, ControlClientError> {
    /// The `ERROR` code of a refusal, nil for anything else.
    var flatMapError: UInt32? {
        if case .failure(.refused(let report))? = self { return report.code }
        return nil
    }
}
