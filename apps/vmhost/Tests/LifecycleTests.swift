import Darwin
import Foundation
import XCTest
import ODShared
@testable import od_vmhost

// Unit tests of the state machine of VMHost control v2 (apps/vmhost/Sources/Lifecycle.swift, CONTROL_PROTOCOL_V2.md section 7):
// the table "state x operation", the order of checks, request IDs and the replay ring, ATTACH_RESOURCE checks. The profile is a
// fake runner: no display session, no sockets. Run with `swift test --filter LifecycleXCTests` from apps/.

private final class FakeRunner: ProfileRunner {
    var calls: [String] = []
    var failStart = false
    var failStop = false

    struct Failure: Error {}

    func start(profile: UInt32, screens: Int) throws {
        calls.append("start(\(profile),\(screens))")
        if failStart { throw Failure() }
    }

    func pause() { calls.append("pause") }
    func resume(reason: ControlV2Reason) { calls.append("resume(\(reason.rawValue))") }

    func stop() throws {
        calls.append("stop")
        if failStop { throw Failure() }
    }
}

final class LifecycleXCTests: XCTestCase {
    private let gib: UInt64 = 1 << 30

    private func make(lockFailure: BundleLockFailure? = nil, memory: UInt64 = 64 << 30) -> (Lifecycle, FakeRunner) {
        let runner = FakeRunner()
        return (Lifecycle(runner: runner, lockFailure: lockFailure, hostMemoryBytes: memory), runner)
    }

    private func send(_ lifecycle: Lifecycle, _ request: ControlV2Request, id: UInt32, descriptor: Int32? = nil) throws -> LifecycleOutput {
        try lifecycle.handle(ControlV2Received(frame: ControlV2Frame(request: request, requestID: id), descriptor: descriptor))
    }

    private func responses(_ output: LifecycleOutput) throws -> [ControlV2Response] {
        try output.frames.map { try ControlV2Response.parse($0) }
    }

    private func error(_ output: LifecycleOutput) throws -> ControlV2ErrorReport {
        let all = try responses(output)
        guard all.count == 1, case .error(let report) = all[0] else {
            XCTFail("one ERROR expected, got \(all)")
            return ControlV2ErrorReport(code: 0, failedKind: 0, state: 0)
        }
        return report
    }

    /// HELLO and a valid CONFIGURE; returns the next request ID.
    private func ready(_ lifecycle: Lifecycle, configuration: ControlV2Configure = ControlV2Configure()) throws -> UInt32 {
        XCTAssertEqual(try responses(try send(lifecycle, .hello, id: 0)), [.ready(profile: 2)])
        _ = try send(lifecycle, .configure(configuration), id: 1)
        return 2
    }

    func testHandshakeAndLockFailureInsteadOfReady() throws {
        let (lifecycle, _) = make(lockFailure: .busy)
        let output = try send(lifecycle, .hello, id: 0)
        XCTAssertEqual(output.exitStatus, VMHostExitStatus.lockRejected)
        let report = try error(output)
        XCTAssertEqual(report.code, ControlV2ErrorCode.busy.rawValue)
        XCTAssertEqual(report.failedKind, 0)
        XCTAssertTrue(report.fatal)
        XCTAssertEqual(output.frames[0].requestID, 0)
        // A request before HELLO is fatal.
        XCTAssertThrowsError(try send(make().0, .query, id: 1)) { XCTAssertEqual($0 as? ControlV2FatalError, .helloExpected) }
        let (again, _) = make()
        _ = try send(again, .hello, id: 0)
        XCTAssertThrowsError(try send(again, .hello, id: 0)) { XCTAssertEqual($0 as? ControlV2FatalError, .helloRepeated) }
    }

    func testStartPauseResumeStopOrderOfFramesAndStates() throws {
        let (lifecycle, runner) = make()
        var id = try ready(lifecycle)
        func step(_ request: ControlV2Request) throws -> [ControlV2Response] {
            defer { id += 1 }
            return try responses(try send(lifecycle, request, id: id))
        }
        XCTAssertEqual(try step(.start(operationID: 0)), [
            .stateChanged(from: 1, to: 2, reason: 1), .ack(ControlV2Ack(ackedKind: 6, state: 2)),
        ])
        XCTAssertEqual(try step(.pause(reason: 2)), [
            .stateChanged(from: 2, to: 3, reason: 2), .ack(ControlV2Ack(ackedKind: 7, state: 3)),
        ])
        XCTAssertEqual(try step(.resume(reason: 3)), [
            .stateChanged(from: 3, to: 2, reason: 3), .ack(ControlV2Ack(ackedKind: 8, state: 2)),
        ])
        XCTAssertEqual(lifecycle.stateReport(), ControlV2StateReport(state: 2, flags: 1, lastReason: 3))
        let stop = try send(lifecycle, .stop(operationID: 0), id: id)
        XCTAssertEqual(try responses(stop), [
            .stateChanged(from: 2, to: 1, reason: 1), .ack(ControlV2Ack(ackedKind: 9, state: 1)),
        ])
        XCTAssertEqual(stop.exitStatus, 0)
        XCTAssertEqual(runner.calls, ["start(1,1)", "pause", "resume(3)", "stop"])
    }

    func testRefusalsDoNotChangeTheState() throws {
        let (lifecycle, runner) = make()
        _ = try send(lifecycle, .hello, id: 0)
        XCTAssertEqual(try error(try send(lifecycle, .start(operationID: 0), id: 1)).code, ControlV2ErrorCode.notConfigured.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .pause(reason: 1), id: 2)).code, ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .resume(reason: 1), id: 3)).code, ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .configure(ControlV2Configure(memoryMiB: 1)), id: 4)).aux0, 4)
        XCTAssertEqual(lifecycle.stateReport(), ControlV2StateReport(state: 1, flags: 0, lastReason: 0))
        _ = try send(lifecycle, .configure(ControlV2Configure()), id: 5)
        XCTAssertEqual(try error(try send(lifecycle, .configure(ControlV2Configure()), id: 6)).code, ControlV2ErrorCode.invalidState.rawValue)
        // A non-zero operation ID answers before the state is looked at.
        XCTAssertEqual(try error(try send(lifecycle, .start(operationID: 1), id: 7)).code, ControlV2ErrorCode.unsupportedDurability.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .shutdown(operationID: 0), id: 8)).code, ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .setClipboard([]), id: 9)).code, ControlV2ErrorCode.unsupported.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .reservedStage9(.restore), id: 10)).code, ControlV2ErrorCode.unsupported.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .prepareDisk(ControlV2PrepareDisk(capacityBytes: 4096, name: Array("d".utf8))), id: 11)).code,
                       ControlV2ErrorCode.unsupported.rawValue)
        _ = try send(lifecycle, .start(operationID: 0), id: 12)
        // While running: pause with a reason that belongs to RESUME, SHUTDOWN, PREPARE_DISK.
        XCTAssertEqual(try error(try send(lifecycle, .pause(reason: 3), id: 13)).code, ControlV2ErrorCode.invalidArgument.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .shutdown(operationID: 0), id: 14)).code, ControlV2ErrorCode.unsupportedDurability.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .prepareDisk(ControlV2PrepareDisk(capacityBytes: 4096, name: Array("d".utf8))), id: 15)).code,
                       ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(lifecycle.state, .running)
        XCTAssertEqual(runner.calls, ["start(1,1)"])
    }

    func testEndOfStreamBeforeHello() throws {
        // Lock taken: EOF before HELLO is an EOF in `stopped` without a request in flight (section 12.3): status 0.
        XCTAssertEqual(make().0.statusOnEndOfStream(), VMHostExitStatus.ok)
        // Lock failed: no READY and no reason were given, so EOF before HELLO is a protocol failure like garbage: status 4.
        for failure in [BundleLockFailure.busy, .invalid, .lockChanged, .io(5)] {
            XCTAssertEqual(make(lockFailure: failure).0.statusOnEndOfStream(), VMHostExitStatus.protocolError)
        }
    }

    func testMachineWithDiskAndUnsupportedDevices() throws {
        let (withDisk, runner) = make()
        var id = try ready(withDisk, configuration: ControlV2Configure(disks: [.init(name: Array("d".utf8))]))
        XCTAssertEqual(withDisk.stateReport().flags, 3)
        XCTAssertEqual(try error(try send(withDisk, .start(operationID: 0), id: id)).code, ControlV2ErrorCode.unsupportedDurability.rawValue)
        id += 1
        let stop = try send(withDisk, .stop(operationID: 0), id: id)
        XCTAssertEqual(try error(stop).code, ControlV2ErrorCode.unsupportedDurability.rawValue)
        XCTAssertNil(stop.exitStatus)
        XCTAssertTrue(runner.calls.isEmpty)
        XCTAssertEqual(withDisk.statusOnEndOfStream(), 0)

        let cases: [(ControlV2Configure, UInt64)] = [
            (ControlV2Configure(network: 1), 12), (ControlV2Configure(audio: 1), 13), (ControlV2Configure(clipboardSync: 1), 14),
            (ControlV2Configure(bootMedia: 1), 15), (ControlV2Configure(sharedFolders: [.init(guestName: Array("s".utf8))]), 17),
        ]
        for (configuration, offset) in cases {
            let (lifecycle, _) = make()
            let next = try ready(lifecycle, configuration: configuration)
            let report = try error(try send(lifecycle, .start(operationID: 0), id: next))
            XCTAssertEqual(report.code, ControlV2ErrorCode.unsupported.rawValue)
            XCTAssertEqual(report.aux0, offset)
        }
        // Memory above half of the host memory.
        let (small, _) = make(memory: 4 * gib)
        let next = try ready(small, configuration: ControlV2Configure(memoryMiB: 4096))
        let report = try error(try send(small, .start(operationID: 0), id: next))
        XCTAssertEqual(report.code, ControlV2ErrorCode.hostMemoryExceeded.rawValue)
        XCTAssertEqual(report.aux0, 4096)
        XCTAssertEqual(report.aux1, 2048)
    }

    func testStopOfStoppedMachineHasNoStateChanged() throws {
        let (lifecycle, runner) = make()
        _ = try send(lifecycle, .hello, id: 0)
        let output = try send(lifecycle, .stop(operationID: 0), id: 1)
        XCTAssertEqual(try responses(output), [.ack(ControlV2Ack(ackedKind: 9, state: 1))])
        XCTAssertEqual(output.exitStatus, 0)
        XCTAssertTrue(runner.calls.isEmpty)
    }

    func testFailedStateOnAbiFailure() throws {
        let (lifecycle, runner) = make()
        runner.failStart = true
        let id = try ready(lifecycle)
        let output = try send(lifecycle, .start(operationID: 0), id: id)
        XCTAssertEqual(output.exitStatus, VMHostExitStatus.failed)
        let all = try responses(output)
        XCTAssertEqual(all.count, 2)
        XCTAssertEqual(all[0], .stateChanged(from: 1, to: 5, reason: 5))
        guard case .error(let report) = all[1] else { return XCTFail("fatal ERROR expected") }
        XCTAssertEqual(report.code, ControlV2ErrorCode.hostFailure.rawValue)
        XCTAssertTrue(report.fatal)
        XCTAssertEqual(lifecycle.state, .failed)
    }

    func testRequestIDsReplayConflictStaleAndSkip() throws {
        let (lifecycle, _) = make()
        let first = try ready(lifecycle) // ID 1 was CONFIGURE
        // Replay of CONFIGURE: the stored ACK with the replayed flag, no second execution.
        let replay = try responses(try send(lifecycle, .configure(ControlV2Configure(memoryMiB: 1)), id: 1))
        XCTAssertEqual(replay, [.ack(ControlV2Ack(ackedKind: 3, flags: 1, state: 1))])
        // The same ID with another kind: request_id_conflict.
        XCTAssertEqual(try error(try send(lifecycle, .pause(reason: 1), id: 1)).code, ControlV2ErrorCode.requestIDConflict.rawValue)
        // A skipped ID and ID 0 are fatal.
        XCTAssertThrowsError(try send(lifecycle, .query, id: first + 1)) { XCTAssertEqual($0 as? ControlV2FatalError, .requestIDSkipped) }
        XCTAssertThrowsError(try send(lifecycle, .query, id: 0)) { XCTAssertEqual($0 as? ControlV2FatalError, .requestIDZero) }
        // 70 more requests push ID 1 out of the ring of 64.
        var id = first
        for _ in 0..<70 {
            _ = try send(lifecycle, .query, id: id)
            id += 1
        }
        XCTAssertEqual(try error(try send(lifecycle, .configure(ControlV2Configure()), id: 1)).code, ControlV2ErrorCode.requestIDStale.rawValue)
        // A replayed QUERY is a fresh STATE.
        XCTAssertEqual(try responses(try send(lifecycle, .query, id: id - 1)), [.state(lifecycle.stateReport())])
        XCTAssertEqual(lifecycle.highestRequestID, id - 1)
    }

    func testAttachResourceChecksAndStoredDescriptors() throws {
        let (lifecycle, _) = make()
        _ = try send(lifecycle, .hello, id: 0)
        func open(_ path: String, _ flags: Int32) -> Int32 { Darwin.open(path, flags) }
        // Before CONFIGURE.
        let early = open("/dev/null", O_RDONLY)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .bootMedia)), id: 1, descriptor: early)).code,
                       ControlV2ErrorCode.notConfigured.rawValue)
        XCTAssertEqual(fcntl(early, F_GETFD), -1, "a refused descriptor is closed")
        _ = try send(lifecycle, .configure(ControlV2Configure(bootMedia: 1, disks: [.init(source: 2, writable: 1, name: [])])), id: 2)
        let file = (NSTemporaryDirectory() as NSString).appendingPathComponent("lifecycle-test-\(getpid()).iso")
        FileManager.default.createFile(atPath: file, contents: Data(count: 2048))
        defer { try? FileManager.default.removeItem(atPath: file) }
        // The boot medium: read only, regular file.
        let wrongMode = open(file, O_RDWR)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .bootMedia)), id: 3, descriptor: wrongMode)).code,
                       ControlV2ErrorCode.invalidResource.rawValue)
        let directory = open(NSTemporaryDirectory(), O_RDONLY)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .bootMedia)), id: 4, descriptor: directory)).code,
                       ControlV2ErrorCode.invalidResource.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: 9, index: 0)), id: 5, descriptor: open(file, O_RDONLY))).code,
                       ControlV2ErrorCode.invalidResource.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .disk, index: 1)), id: 6, descriptor: open(file, O_RDWR))).code,
                       ControlV2ErrorCode.invalidResource.rawValue)
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .dropFile)), id: 7, descriptor: open(file, O_RDONLY))).code,
                       ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(lifecycle.attachedDescriptorCount, 0)
        XCTAssertEqual(try responses(try send(lifecycle, .attachResource(.init(role: .bootMedia)), id: 8, descriptor: open(file, O_RDONLY))),
                       [.ack(ControlV2Ack(ackedKind: 4, state: 1))])
        let duplicate = open(file, O_RDONLY)
        let conflict = try error(try send(lifecycle, .attachResource(.init(role: .bootMedia)), id: 9, descriptor: duplicate))
        XCTAssertEqual(conflict.code, ControlV2ErrorCode.resourceConflict.rawValue)
        XCTAssertEqual(fcntl(duplicate, F_GETFD), -1)
        // A writable disk needs a read-write descriptor.
        XCTAssertEqual(try error(try send(lifecycle, .attachResource(.init(role: .disk, index: 0)), id: 10, descriptor: open(file, O_RDONLY))).code,
                       ControlV2ErrorCode.invalidResource.rawValue)
        XCTAssertEqual(try responses(try send(lifecycle, .attachResource(.init(role: .disk, index: 0)), id: 11, descriptor: open(file, O_RDWR))),
                       [.ack(ControlV2Ack(ackedKind: 4, state: 1))])
        XCTAssertEqual(lifecycle.attachedDescriptorCount, 2)
        lifecycle.closeAttachedDescriptors()
        XCTAssertEqual(lifecycle.attachedDescriptorCount, 0)
    }

    func testDropFileOnlyWhileRunningWithLimitOfEight() throws {
        let (lifecycle, _) = make()
        var id = try ready(lifecycle)
        _ = try send(lifecycle, .start(operationID: 0), id: id)
        id += 1
        let file = (NSTemporaryDirectory() as NSString).appendingPathComponent("lifecycle-drop-\(getpid()).bin")
        FileManager.default.createFile(atPath: file, contents: Data(count: 16))
        defer { try? FileManager.default.removeItem(atPath: file) }
        for _ in 0..<8 {
            let descriptor = Darwin.open(file, O_RDONLY)
            XCTAssertEqual(try responses(try send(lifecycle, .attachResource(.init(role: .dropFile)), id: id, descriptor: descriptor)),
                           [.ack(ControlV2Ack(ackedKind: 4, state: 2))])
            id += 1
        }
        let report = try error(try send(lifecycle, .attachResource(.init(role: .dropFile)), id: id, descriptor: Darwin.open(file, O_RDONLY)))
        XCTAssertEqual(report.code, ControlV2ErrorCode.resourceLimit.rawValue)
        XCTAssertEqual(lifecycle.attachedDescriptorCount, 8)
        lifecycle.closeAttachedDescriptors()
    }
}
