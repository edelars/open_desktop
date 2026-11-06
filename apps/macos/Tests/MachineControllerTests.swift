import Combine
import Darwin
import Foundation
@testable import ODShared
import XCTest
@testable import OpenDesktop

// Tests of the library state of S11-06b (Sources/App/MachineController.swift, LibraryModel.swift). No window and no process: the controller
// takes a fake session in place of `ControlClientV2` and the tests deliver the events themselves, as the client would on the main queue.
// Every machine here is test-only, without a guest. Run: `swift test --filter MachineController` from apps/.

private final class FakeSession: MachineSession {
    enum Call: Equatable { case configure, start, pause, resume, stop, query, terminate }
    var calls: [Call] = []
    var configure: ControlV2Configure?
    private var pending: [Call: ControlClientV2.Completion] = [:]

    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion) {
        calls.append(.configure)
        self.configure = configure
        pending[.configure] = completion
    }
    func requestStart(completion: @escaping ControlClientV2.Completion) { calls.append(.start); pending[.start] = completion }
    func requestPause(completion: @escaping ControlClientV2.Completion) { calls.append(.pause); pending[.pause] = completion }
    func requestResume(completion: @escaping ControlClientV2.Completion) { calls.append(.resume); pending[.resume] = completion }
    func requestStop(completion: @escaping ControlClientV2.Completion) { calls.append(.stop); pending[.stop] = completion }
    func requestQuery(completion: @escaping ControlClientV2.Completion) { calls.append(.query); pending[.query] = completion }
    func terminateSession() { calls.append(.terminate) }

    func complete(_ call: Call, _ result: Result<ControlV2Reply, ControlClientError>) {
        guard let completion = pending.removeValue(forKey: call) else { return XCTFail("\(call) was not requested") }
        completion(result)
    }

    func ack(_ call: Call, kind: ControlV2Kind, state: ControlV2State) {
        complete(call, .success(.ack(ControlV2Ack(ackedKind: kind.rawValue, state: state.rawValue))))
    }
}

private final class FakeLauncher {
    var launches: [ControlClientLaunch] = []
    var sessions: [FakeSession] = []
    var handlers: [ControlClientV2.EventHandler] = []
    var failure: Error?

    var launcher: MachineLauncher {
        { [self] launch, handler in
            if let failure { throw failure }
            launches.append(launch)
            handlers.append(handler)
            let session = FakeSession()
            sessions.append(session)
            return session
        }
    }

    var session: FakeSession { sessions[sessions.count - 1] }
    func emit(_ event: ControlClientEvent, from index: Int? = nil) { handlers[index ?? handlers.count - 1](event) }
}

private func makeStatus(_ configure: (inout BundleConfig) -> Void = { _ in }) -> CatalogBundleStatus {
    var config = BundleConfig(name: "Тест")
    configure(&config)
    return .config(.valid(config))
}

private let vmhost = VMHostLocation(url: URL(fileURLWithPath: "/x/od-vmhost"), source: .argument, isExecutable: true)

private func makeController(
    _ launcher: FakeLauncher, status: CatalogBundleStatus = makeStatus(), executable: Bool = true
) -> MachineController {
    let location = executable ? vmhost : VMHostLocation(url: vmhost.url, source: .argument, isExecutable: false)
    let entry = CatalogEntry(bundleURL: URL(fileURLWithPath: "/x/Тест.odvm"))
    return MachineController(
        entry: entry, status: status, vmhost: location, launcher: launcher.launcher, hostMemoryBytes: 64 << 30)
}

private func changed(_ from: ControlV2State, _ to: ControlV2State, request: UInt32 = 0) -> ControlClientEvent {
    .stateChanged(from: from, to: to, reason: ControlV2Reason.user.rawValue, requestID: request)
}

private func exitEvent(_ termination: ControlClientExit.Termination, rejection: StartRejection? = nil) -> ControlClientEvent {
    .exited(ControlClientExit(termination: termination, startRejection: rejection))
}

/// Brings a controller to `running` the way the events do: start, READY, CONFIGURE, START, `stateChanged`, ACK.
private func runToRunning(_ machine: MachineController, _ launcher: FakeLauncher) {
    machine.start()
    launcher.emit(.ready)
    launcher.session.ack(.configure, kind: .configure, state: .stopped)
    launcher.emit(changed(.stopped, .running, request: 2))
    launcher.session.ack(.start, kind: .start, state: .running)
}

// MARK: 1. the table of actions (CONTROL_PROTOCOL_V2.md 7.2), a pure function

final class MachineActionTableXCTests: XCTestCase {
    private func allowed(_ state: MachineRunState, session: Bool = true, inFlight: Bool = false, usable: Bool = true) -> Set<MachineAction> {
        MachineActionTable.allowed(state: state, hasSession: session, requestInFlight: inFlight, bundleUsable: usable)
    }

    func testStoppedWithoutProcessMayStartAndBeDeleted() {
        XCTAssertEqual(allowed(.stopped, session: false), [.start, .delete])
    }

    func testBrokenBundleCannotStartButCanBeRemoved() {
        XCTAssertEqual(allowed(.stopped, session: false, usable: false), [.delete])
    }

    func testRunningMayPauseAndStop() {
        XCTAssertEqual(allowed(.running), [.pause, .stop])
    }

    func testPausedMayResumeAndStop() {
        XCTAssertEqual(allowed(.paused), [.resume, .stop])
    }

    func testShuttingDownAcceptsOnlyStop() {
        // 7.2 note 4: STOP is the one explicit exception in `shutting_down`.
        XCTAssertEqual(allowed(.shuttingDown), [.stop])
    }

    func testStoppedWithProcessAndFailedOfferNothing() {
        XCTAssertEqual(allowed(.stopped, session: true), [], "the process is going away: a new start waits for `exited`")
        XCTAssertEqual(allowed(.failed), [], "`failed` is terminal for the process (7.2: closed)")
    }

    func testStartingOffersOnlyAbortEvenWithARequestInFlight() {
        XCTAssertEqual(allowed(.starting), [.abort], "the way out of a VMHost that never says READY")
        XCTAssertEqual(allowed(.starting, inFlight: true), [.abort], "CONFIGURE or START may be unanswered too")
        for state in [MachineRunState.running, .paused, .shuttingDown, .stopped, .failed] {
            XCTAssertFalse(allowed(state, inFlight: false).contains(.abort), "\(state)")
            XCTAssertFalse(allowed(state, session: false).contains(.abort), "\(state) without a session")
        }
    }

    func testNothingIsActiveWhileARequestIsInFlight() {
        for state in [MachineRunState.running, .paused, .shuttingDown, .stopped, .failed] {
            XCTAssertEqual(allowed(state, inFlight: true), [], "\(state)")
        }
    }

    func testDeleteOnlyInStoppedWithoutProcess() {
        for state in [MachineRunState.running, .paused, .shuttingDown, .starting, .failed] {
            XCTAssertFalse(allowed(state, session: true).contains(.delete), "\(state)")
            XCTAssertFalse(allowed(state, session: false).contains(.delete), "\(state) without a session")
        }
        XCTAssertFalse(allowed(.stopped, session: true).contains(.delete))
    }

    func testEveryAllowedOperationIsAnOkCellOfTheTable() {
        // The operations the table gives as `ok` (7.2): PAUSE only in running, RESUME only in paused, START only in stopped (no process
        // yet: the application configures and starts in one action), STOP in running, paused and shutting_down.
        for state in [MachineRunState.stopped, .starting, .running, .paused, .shuttingDown, .failed] {
            for session in [true, false] {
                let set = allowed(state, session: session)
                if set.contains(.abort) { XCTAssertTrue(state == .starting && session) }
                if set.contains(.pause) { XCTAssertEqual(state, .running) }
                if set.contains(.resume) { XCTAssertEqual(state, .paused) }
                if set.contains(.start) { XCTAssertTrue(state == .stopped && !session) }
                if set.contains(.stop) { XCTAssertTrue([.running, .paused, .shuttingDown].contains(state) && session) }
            }
        }
    }

    func testWireStatesMapToRowStates() {
        XCTAssertEqual(MachineRunState(.stopped), .stopped)
        XCTAssertEqual(MachineRunState(.running), .running)
        XCTAssertEqual(MachineRunState(.paused), .paused)
        XCTAssertEqual(MachineRunState(.shuttingDown), .shuttingDown)
        XCTAssertEqual(MachineRunState(.failed), .failed)
    }
}

// MARK: 2. transitions by events

final class MachineControllerXCTests: XCTestCase {
    func testStartShowsRunningOnlyAfterTheStateChangedEvent() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertTrue(machine.allows(.start))

        machine.start()
        XCTAssertEqual(launcher.launches.count, 1)
        XCTAssertEqual(launcher.launches[0].vmhostPath, "/x/od-vmhost")
        XCTAssertEqual(launcher.launches[0].bundlePath, "/x/Тест.odvm")
        XCTAssertFalse(launcher.launches[0].displayChannel, "the display window is S11-10")
        XCTAssertEqual(machine.runState, .starting, "a live process is not `running`")
        XCTAssertTrue(machine.hasSession)
        XCTAssertEqual(machine.allowedActions, [.abort])
        XCTAssertEqual(launcher.session.calls, [], "nothing is sent before READY")

        launcher.emit(.ready)
        XCTAssertEqual(launcher.session.calls, [.configure])
        XCTAssertEqual(launcher.session.configure?.machineProfile, 1)
        XCTAssertEqual(launcher.session.configure?.memoryMiB, 4096)
        XCTAssertEqual(machine.runState, .starting, "READY and a sent CONFIGURE do not make a machine run")

        launcher.session.ack(.configure, kind: .configure, state: .stopped)
        XCTAssertEqual(launcher.session.calls, [.configure, .start])
        XCTAssertEqual(machine.runState, .starting, "an ACK is not a state: only `stateChanged` is")

        // 6.4: the state event comes before the terminal answer of the request that caused it.
        launcher.emit(changed(.stopped, .running, request: 2))
        XCTAssertEqual(machine.runState, .running)
        XCTAssertEqual(machine.allowedActions, [], "START is still in flight")
        launcher.session.ack(.start, kind: .start, state: .running)
        XCTAssertEqual(machine.allowedActions, [.pause, .stop])
    }

    func testAbortEndsAProcessThatNeverSaysReady() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.abort()
        XCTAssertEqual(launcher.launches.count, 0, "nothing to abort without a process")
        machine.start()
        XCTAssertEqual(machine.runState, .starting)
        machine.abort()
        XCTAssertEqual(launcher.session.calls, [.terminate], "SIGTERM, then SIGKILL, are the client's `terminate`")
        XCTAssertEqual(machine.runState, .starting, "the row changes at `exited`, not at the request to end")
        launcher.emit(exitEvent(.signaled(SIGTERM)))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.allowedActions, [.start, .delete])
    }

    func testAbortWorksWhileConfigureIsUnanswered() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.start()
        launcher.emit(.ready)
        XCTAssertTrue(machine.requestInFlight)
        XCTAssertEqual(machine.allowedActions, [.abort])
        machine.abort()
        XCTAssertEqual(launcher.session.calls, [.configure, .terminate])
        launcher.emit(exitEvent(.signaled(SIGKILL)))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.requestInFlight)
    }

    func testPauseAndResumeFollowTheEventsNotTheRequests() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)

        machine.pause()
        XCTAssertEqual(launcher.session.calls.last, .pause)
        XCTAssertEqual(machine.runState, .running, "a sent PAUSE changes nothing")
        XCTAssertEqual(machine.allowedActions, [])
        launcher.emit(changed(.running, .paused, request: 3))
        XCTAssertEqual(machine.runState, .paused)
        launcher.session.ack(.pause, kind: .pause, state: .paused)
        XCTAssertEqual(machine.allowedActions, [.resume, .stop])

        machine.resume()
        XCTAssertEqual(launcher.session.calls.last, .resume)
        XCTAssertEqual(machine.runState, .paused)
        launcher.emit(changed(.paused, .running, request: 4))
        launcher.session.ack(.resume, kind: .resume, state: .running)
        XCTAssertEqual(machine.runState, .running)
        XCTAssertEqual(machine.allowedActions, [.pause, .stop])
    }

    func testRefusedPauseKeepsTheStateAndSaysWhy() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        machine.pause()
        let report = ControlV2ErrorReport(code: ControlV2ErrorCode.pauseTimeout.rawValue, failedKind: ControlV2Kind.pause.rawValue, state: ControlV2State.running.rawValue)
        launcher.emit(.error(report, requestID: 3)) // the same ERROR reaches the event stream: the completion handles it
        launcher.session.complete(.pause, .failure(.refused(report)))
        XCTAssertEqual(machine.runState, .running, "7.2: `pause_timeout` leaves `running`")
        XCTAssertEqual(machine.allowedActions, [.pause, .stop])
        XCTAssertEqual(machine.notice?.text, "Пауза: машина не успела приостановиться.")
        XCTAssertEqual(machine.notice?.details?.contains("pause_timeout (10)"), true)
    }

    func testUnexpectedExitMakesItsReasonTheHeadlineNotTheEarlierRefusal() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        machine.pause()
        let report = ControlV2ErrorReport(code: ControlV2ErrorCode.pauseTimeout.rawValue, failedKind: ControlV2Kind.pause.rawValue, state: ControlV2State.running.rawValue)
        launcher.session.complete(.pause, .failure(.refused(report)))
        XCTAssertEqual(machine.notice?.text.contains("не успела приостановиться"), true)
        launcher.emit(exitEvent(.signaled(SIGKILL))) // nobody asked for a STOP
        XCTAssertEqual(machine.notice?.text, "Процесс машины аварийно завершился")
        XCTAssertEqual(machine.notice?.details?.contains("не успела приостановиться"), true, "the earlier line is kept in the details")
        XCTAssertEqual(machine.notice?.details?.contains("pause_timeout (10)"), true)
    }

    func testStopEndsWithStoppedEventThenExit() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)

        machine.stop()
        XCTAssertEqual(launcher.session.calls.last, .stop)
        XCTAssertEqual(machine.runState, .running, "a sent STOP changes nothing")
        // 6.4: `STATE_CHANGED -> stopped`, `ACK`, then the connection is closed.
        launcher.emit(changed(.running, .stopped, request: 3))
        launcher.session.ack(.stop, kind: .stop, state: .stopped)
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertTrue(machine.hasSession, "the process has not been reaped yet")
        XCTAssertEqual(machine.allowedActions, [], "no new start and no delete before `exited`")

        launcher.emit(exitEvent(.exited(0)))
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertNil(machine.notice, "status 0 after a STOP is the normal end")
        XCTAssertEqual(machine.lastExit?.disposition, .stopped)
        XCTAssertEqual(machine.allowedActions, [.start, .delete])
    }

    func testStartAgainAfterAnExit() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        launcher.emit(exitEvent(.exited(0)))
        machine.start()
        XCTAssertEqual(launcher.launches.count, 2)
        XCTAssertEqual(machine.runState, .starting)
    }

    func testBusyFromVMHostIsShownAsAnotherProcessHoldsTheMachine() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.start()
        launcher.emit(.startRejected(.busy))
        XCTAssertEqual(machine.runState, .starting, "the row changes at `exited`")
        XCTAssertEqual(machine.notice?.text.contains("машина уже запущена другим процессом"), true)

        launcher.emit(exitEvent(.exited(3), rejection: .busy))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.lastExit?.isBusy, true)
        XCTAssertEqual(machine.notice?.text, "Не удалось запустить: машина уже запущена другим процессом.")
        XCTAssertNotEqual(machine.runState, .running, "never `running` for a process that exists")
        XCTAssertEqual(machine.allowedActions, [.start, .delete], "the user may repeat the start: there are no automatic retries")
        XCTAssertEqual(launcher.launches.count, 1)
    }

    func testExitBySignalIsAStoppedMachineWithTheSignal() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        launcher.emit(exitEvent(.signaled(SIGKILL)))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.lastExit?.disposition, .killedBySignal)
        XCTAssertEqual(machine.notice?.text, "Процесс машины аварийно завершился")
        XCTAssertEqual(machine.notice?.details?.contains("Сигнал: 9"), true)
        XCTAssertEqual(machine.allowedActions, [.start, .delete])
    }

    func testExitStatusesOfSection123() {
        let cases: [(Int32, String)] = [
            (2, "не запустился"), (4, "Нарушен обмен"), (5, "Связь с процессом машины потеряна"), (6, "Сбой процесса машины"), (77, "завершился неожиданно"),
        ]
        for (code, fragment) in cases {
            let launcher = FakeLauncher()
            let machine = makeController(launcher)
            runToRunning(machine, launcher)
            launcher.emit(exitEvent(.exited(code)))
            XCTAssertEqual(machine.runState, .stopped, "status \(code)")
            XCTAssertEqual(machine.notice?.text.contains(fragment), true, "status \(code): \(machine.notice?.text ?? "nil")")
            XCTAssertEqual(machine.notice?.details?.contains("\(code)"), true, "the status is in the details")
        }
    }

    func testFailedStateIsTerminalAndTheExitMakesItStopped() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        launcher.emit(changed(.running, .failed))
        XCTAssertEqual(machine.runState, .failed)
        XCTAssertEqual(machine.allowedActions, [])
        XCTAssertNotNil(machine.notice)
        launcher.emit(exitEvent(.exited(6)))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertEqual(machine.lastExit?.disposition, .hostFailed)
        XCTAssertEqual(machine.notice?.text, "Сбой процесса машины", "the cause is kept next to the status")
        XCTAssertEqual(machine.notice?.details?.contains("Состояние: failed (по событию STATE_CHANGED)"), true, "no ERROR was sent: the state came from the event")
        XCTAssertEqual(machine.notice?.details?.contains("Статус процесса: 6"), true)
    }

    func testProtocolViolationKeepsItsTextNextToTheSignal() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        launcher.emit(.protocolViolation("bad frame"))
        launcher.emit(exitEvent(.signaled(SIGTERM)))
        XCTAssertEqual(machine.notice?.text, "Нарушен обмен с процессом машины")
        XCTAssertEqual(machine.notice?.details?.contains("bad frame"), true)
        XCTAssertEqual(machine.notice?.details?.contains("Сигнал: 15"), true)
    }

    func testLaunchRefusalForDisksDoesNotSpawnAVMHost() {
        let launcher = FakeLauncher()
        let status = makeStatus { $0.disks = [BundleDisk(location: .inBundle(name: "root.raw"))] }
        let machine = makeController(launcher, status: status)
        XCTAssertTrue(machine.allows(.start), "the button is active (the table); the refusal is the answer")
        machine.start()
        XCTAssertEqual(launcher.launches.count, 0)
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.notice?.details?.contains("unsupported_durability"), true)
    }

    func testMemoryAboveHalfOfTheHostIsRefusedLocally() {
        let launcher = FakeLauncher()
        let status = makeStatus { $0.memoryMiB = 40_000 }
        let machine = makeController(launcher, status: status) // the host has 64 GiB: the limit is 32768 MiB
        machine.start()
        XCTAssertEqual(launcher.launches.count, 0)
        XCTAssertEqual(machine.notice?.details?.contains("host_memory_exceeded (12)"), true)
        XCTAssertEqual(machine.notice?.details?.contains("40000"), true, "the requested size is in the details, not in the headline")
    }

    func testSpawnFailureLeavesAStoppedMachine() {
        let launcher = FakeLauncher()
        launcher.failure = POSIXError(.ENOENT)
        let machine = makeController(launcher)
        machine.start()
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        XCTAssertEqual(machine.notice?.text, "Не удалось запустить: не получилось создать процесс машины.")
    }

    func testMissingVMHostIsReportedWithoutSpawning() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher, executable: false)
        machine.start()
        XCTAssertEqual(launcher.launches.count, 0)
        XCTAssertEqual(machine.notice?.text, "Не удалось запустить: процесс машины не найден.")
    }

    func testIncompatibleOrDamagedBundleCannotStart() {
        let launcher = FakeLauncher()
        let issue = BundleConfigIssue(field: "schema_version", rule: "newer")
        let machine = makeController(launcher, status: .config(.incompatible(issue)))
        XCTAssertEqual(machine.allowedActions, [.delete])
        machine.start()
        XCTAssertEqual(launcher.launches.count, 0)
        XCTAssertEqual(MachinePresentation.bundleText(.config(.incompatible(issue))), "Несовместимая")
        XCTAssertEqual(MachinePresentation.bundleText(.config(.invalid(issue))), "Повреждённая")
    }

    func testRefusedConfigureClosesTheSessionWithStop() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.start()
        launcher.emit(.ready)
        let report = ControlV2ErrorReport(code: ControlV2ErrorCode.invalidConfig.rawValue, failedKind: ControlV2Kind.configure.rawValue, state: ControlV2State.stopped.rawValue)
        launcher.session.complete(.configure, .failure(.refused(report)))
        XCTAssertEqual(launcher.session.calls, [.configure, .stop], "STOP is valid in `stopped` and ends the process")
        XCTAssertEqual(machine.runState, .starting)
        XCTAssertEqual(machine.notice?.text.hasPrefix("Не удалось запустить:"), true)
        launcher.session.ack(.stop, kind: .stop, state: .stopped)
        launcher.emit(exitEvent(.exited(0)))
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertNotNil(machine.notice, "a normal end does not erase the reason")
        XCTAssertEqual(machine.allowedActions, [.start, .delete])
    }

    func testRefusedStartClosesTheSessionToo() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.start()
        launcher.emit(.ready)
        launcher.session.ack(.configure, kind: .configure, state: .stopped)
        let report = ControlV2ErrorReport(code: ControlV2ErrorCode.hostMemoryExceeded.rawValue, failedKind: ControlV2Kind.start.rawValue, state: ControlV2State.stopped.rawValue)
        launcher.session.complete(.start, .failure(.refused(report)))
        XCTAssertEqual(launcher.session.calls, [.configure, .start, .stop])
        XCTAssertEqual(machine.runState, .starting, "no `stateChanged`: the row does not claim `stopped` or `running`")
    }

    func testEventsOfAnEarlierSessionAreIgnored() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        launcher.emit(exitEvent(.exited(0)))
        machine.start()
        launcher.emit(changed(.stopped, .running), from: 0)
        XCTAssertEqual(machine.runState, .starting)
        launcher.emit(exitEvent(.signaled(SIGKILL)), from: 0)
        XCTAssertTrue(machine.hasSession, "the old process's exit is not the new process's")
        XCTAssertEqual(machine.runState, .starting)
    }

    func testRefreshAsksOnceAndDoesNotChangeTheState() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        machine.refresh()
        XCTAssertEqual(launcher.launches.count, 0, "no session: nothing to ask, nothing is spawned")
        runToRunning(machine, launcher)
        machine.refresh()
        XCTAssertEqual(launcher.session.calls.filter { $0 == .query }.count, 1)
        XCTAssertEqual(machine.allowedActions, [.pause, .stop], "QUERY may be sent while no other request is in flight and blocks nothing")
        launcher.session.complete(
            .query, .success(.state(ControlV2StateReport(state: ControlV2State.paused.rawValue, flags: 1, lastReason: 1))))
        XCTAssertEqual(machine.runState, .running, "only events change the row")
        XCTAssertEqual(machine.lastQueryText?.contains("paused"), true)
    }

    func testNoTimerAsksTheVMHost() {
        let launcher = FakeLauncher()
        let machine = makeController(launcher)
        runToRunning(machine, launcher)
        let expectation = expectation(description: "wait")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { expectation.fulfill() }
        wait(for: [expectation], timeout: 3)
        XCTAssertEqual(launcher.session.calls, [.configure, .start], "nothing was sent by itself")
    }

    func testPresentationTexts() {
        XCTAssertEqual(MachinePresentation.memoryText(mib: 4096), "4 ГиБ")
        XCTAssertEqual(MachinePresentation.memoryText(mib: 512), "512 МиБ")
        XCTAssertEqual(MachinePresentation.memoryText(mib: 1536), "1536 МиБ")
        XCTAssertEqual(MachinePresentation.stateText(.running), "Запущена")
        XCTAssertEqual(MachinePresentation.stateText(.starting), "Запускается…")
        XCTAssertTrue(MachinePresentation.profileText(.displayTestPattern).contains("test-only, без гостя"))
        XCTAssertTrue(MachinePresentation.profileText(.displayTwoScreensTest).contains("test-only, без гостя"))
        let entry = CatalogEntry(bundleURL: URL(fileURLWithPath: "/x/Моя.odvm"))
        XCTAssertEqual(MachinePresentation.displayName(entry: entry, status: .directoryMissing), "Моя")
        XCTAssertEqual(MachinePresentation.displayName(entry: entry, status: makeStatus()), "Тест")
    }
}

// MARK: 3. the library model: opening and deleting

final class LibraryModelXCTests: XCTestCase {
    private var root: URL!
    private var indexURL: URL { root.appendingPathComponent("support/Library.plist") }
    private var defaultDirectory: URL { root.appendingPathComponent("Documents", isDirectory: true) }

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("od-library-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: defaultDirectory, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: root)
        super.tearDown()
    }

    private func makeModel(_ launcher: FakeLauncher = FakeLauncher()) -> LibraryModel {
        LibraryModel(
            catalog: Catalog(indexURL: indexURL, defaultDirectory: defaultDirectory), vmhost: vmhost, launcher: launcher.launcher,
            hostMemoryBytes: 64 << 30)
    }

    @discardableResult
    private func makeBundle(_ name: String, configure: (inout BundleConfig) -> Void = { _ in }) throws -> URL {
        let bundle = defaultDirectory.appendingPathComponent("\(name).odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        var config = BundleConfig(name: name)
        configure(&config)
        try BundleConfigStore.write(config, bundle: bundle, mode: .create)
        return bundle
    }

    func testEmptyLibraryHasNoMachines() {
        let model = makeModel()
        model.load()
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertNil(model.selected)
    }

    func testOpenListsAndSelectsTheBundle() throws {
        let other = try makeBundle("Other")
        let bundle = try makeBundle("Open")
        let model = makeModel()
        model.load()
        XCTAssertEqual(model.machines.count, 2, "the default directory is scanned when there is no index")
        XCTAssertTrue(model.open(bundle: bundle))
        XCTAssertEqual(model.selected?.entry.bundleURL.standardizedFileURL, bundle.standardizedFileURL)
        XCTAssertEqual(model.machines.count, 2, "the same bundle is not listed twice")
        XCTAssertTrue(model.open(bundle: other))
        XCTAssertEqual(model.selected?.displayName, "Other")
    }

    func testOpenOfANonDirectoryIsReportedNotListed() throws {
        let file = root.appendingPathComponent("file")
        try Data().write(to: file)
        let model = makeModel()
        model.load()
        XCTAssertFalse(model.open(bundle: file))
        XCTAssertNotNil(model.banner)
        XCTAssertTrue(model.machines.isEmpty)
    }

    func testDeleteWithoutFilesKeepsTheBundleOnDisk() throws {
        let bundle = try makeBundle("Keep")
        let model = makeModel()
        model.load()
        let machine = try XCTUnwrap(model.machines.first)
        XCTAssertTrue(machine.allows(.delete))
        try model.delete(machine, deleteFiles: false)
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("config.plist").path), "the directory stays")
        // The index no longer lists it: a new model loads the index and does not bring the machine back.
        let again = makeModel()
        again.load()
        XCTAssertTrue(again.machines.isEmpty)
    }

    func testDeleteWithFilesRemovesTheDirectory() throws {
        let bundle = try makeBundle("Gone")
        let model = makeModel()
        model.load()
        try model.delete(try XCTUnwrap(model.machines.first), deleteFiles: true)
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testDeleteIsRefusedWhileAProcessExists() throws {
        let bundle = try makeBundle("Busy")
        let launcher = FakeLauncher()
        let model = makeModel(launcher)
        model.load()
        let machine = try XCTUnwrap(model.machines.first)
        machine.start()
        XCTAssertThrowsError(try model.delete(machine, deleteFiles: true)) { XCTAssertEqual($0 as? BundleRemovalError, .notStopped) }
        XCTAssertEqual(model.machines.count, 1)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testFilesOfAMachineWithDisksAreNotDeletedBeforeS906() throws {
        let bundle = try makeBundle("Disks") { $0.disks = [BundleDisk(location: .inBundle(name: "root.raw"))] }
        let model = makeModel()
        model.load()
        let machine = try XCTUnwrap(model.machines.first)
        XCTAssertFalse(LibraryModel.canDeleteFiles(of: machine))
        XCTAssertThrowsError(try model.delete(machine, deleteFiles: true)) {
            XCTAssertEqual($0 as? BundleRemovalError, .unsupportedDurability)
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
        XCTAssertEqual(model.machines.count, 1, "nothing changed")
        try model.delete(machine, deleteFiles: false) // the library entry alone may go
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testRemoveFilesRefusesALinkAFileAndAForeignDirectory() throws {
        let target = try makeBundle("Target")
        let link = root.appendingPathComponent("Link.odvm")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: link)) { XCTAssertEqual($0 as? BundleRemovalError, .notADirectory) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: target.path))

        let foreign = root.appendingPathComponent("Documents-of-the-user", isDirectory: true)
        try FileManager.default.createDirectory(at: foreign, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: foreign.appendingPathComponent("photo.jpg"))
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: foreign)) { XCTAssertEqual($0 as? BundleRemovalError, .notABundle) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: foreign.appendingPathComponent("photo.jpg").path))
    }

    func testRemoveFilesRefusesABundleWhoseLockIsHeld() throws {
        let bundle = try makeBundle("Locked")
        let held = try BundleWriterLock.acquire(bundle: bundle)
        defer { held.release() }
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: bundle)) { XCTAssertEqual($0 as? BundleRemovalError, .inUse) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    /// A directory with the given name under the test root; `config` puts a valid `config.plist` into it, and `photo.jpg` stands for a user's file.
    private func makeDirectory(_ name: String, config: Bool, in parent: URL? = nil) throws -> URL {
        let directory = (parent ?? root).appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: directory.appendingPathComponent("photo.jpg"))
        if config { try BundleConfigStore.write(BundleConfig(name: "Чужой"), bundle: directory, mode: .create) }
        return directory
    }

    /// A model whose index cannot be written: the directory of the index is a regular file.
    private func makeModelWithUnwritableIndex() throws -> LibraryModel {
        let blocker = root.appendingPathComponent("blocker")
        try Data("not a directory".utf8).write(to: blocker)
        return LibraryModel(
            catalog: Catalog(indexURL: blocker.appendingPathComponent("Library.plist"), defaultDirectory: defaultDirectory), vmhost: vmhost,
            launcher: FakeLauncher().launcher, hostMemoryBytes: 64 << 30)
    }

    func testDirectoryWithConfigButWithoutSuffixIsNeitherDeletedNorListed() throws {
        let foreign = try makeDirectory("Photos", config: true)
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: foreign)) { XCTAssertEqual($0 as? BundleRemovalError, .notABundle) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: foreign.appendingPathComponent("photo.jpg").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: foreign.appendingPathComponent("config.plist").path))

        let model = makeModel()
        model.load()
        XCTAssertFalse(model.open(bundle: foreign))
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertNil(model.selected)
        XCTAssertTrue(model.banner?.contains("не похож на машину") == true, model.banner ?? "nil")
        XCTAssertFalse(model.banner?.contains(root.path) == true, "the path is in the details, not in the headline")
        XCTAssertTrue(model.bannerDetails?.contains(foreign.path) == true)
        XCTAssertFalse(FileManager.default.fileExists(atPath: indexURL.path), "a refused directory writes nothing to the index")
        XCTAssertTrue(model.catalog.entries.isEmpty)
    }

    func testBundleWithoutConfigOrWithLinkedConfigIsNotDeletedNorOpened() throws {
        let empty = try makeDirectory("Empty.odvm", config: false)
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: empty)) { XCTAssertEqual($0 as? BundleRemovalError, .notABundle) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: empty.appendingPathComponent("photo.jpg").path))

        let linked = try makeDirectory("Linked.odvm", config: false)
        let real = root.appendingPathComponent("real-config.plist")
        try Data("x".utf8).write(to: real)
        try FileManager.default.createSymbolicLink(at: linked.appendingPathComponent("config.plist"), withDestinationURL: real)
        XCTAssertThrowsError(try LibraryModel.removeBundleFiles(at: linked)) { XCTAssertEqual($0 as? BundleRemovalError, .notABundle) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: linked.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: real.path))

        let model = makeModel()
        model.load()
        XCTAssertFalse(model.open(bundle: empty))
        XCTAssertFalse(model.open(bundle: linked))
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: indexURL.path))
    }

    func testDeleteOfAListedBundleWithoutConfigKeepsTheFiles() throws {
        let bundle = try makeDirectory("NoConfig.odvm", config: false, in: defaultDirectory) // the scan of the default directory lists it
        let model = makeModel()
        model.load()
        let machine = try XCTUnwrap(model.machines.first)
        XCTAssertThrowsError(try model.delete(machine, deleteFiles: true)) { XCTAssertEqual($0 as? BundleRemovalError, .notABundle) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("photo.jpg").path))
        XCTAssertEqual(model.machines.count, 1, "nothing changed")
        try model.delete(machine, deleteFiles: false)
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testRefreshKeepsABundleOpenedThisRunWhenTheIndexCannotBeWritten() throws {
        let away = try makeDirectory("Away.odvm", config: true, in: root.appendingPathComponent("Elsewhere", isDirectory: true))
        let model = try makeModelWithUnwritableIndex()
        model.load()
        XCTAssertTrue(model.open(bundle: away), "the machine is shown although the list is not saved")
        XCTAssertEqual(model.machines.count, 1)
        XCTAssertTrue(model.banner?.contains("не сохранён") == true)
        model.refresh()
        XCTAssertEqual(model.machines.count, 1, "`refresh` re-reads the index and must not lose a machine opened in this run")
        XCTAssertEqual(model.machines.first?.entry.bundleURL.standardizedFileURL, away.standardizedFileURL)
        model.refresh()
        XCTAssertEqual(model.machines.count, 1, "and does not list it twice")
    }

    func testRefreshKeepsABundleOpenedThisRunWhenTheIndexIsNewer() throws {
        try FileManager.default.createDirectory(at: indexURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        let newer = try PropertyListSerialization.data(fromPropertyList: ["schema_version": 2, "machines": []], format: .xml, options: 0)
        try newer.write(to: indexURL)
        let away = try makeDirectory("Away.odvm", config: true, in: root.appendingPathComponent("Elsewhere", isDirectory: true))
        let model = makeModel()
        model.load()
        XCTAssertTrue(model.open(bundle: away))
        XCTAssertEqual(model.machines.count, 1)
        model.refresh()
        XCTAssertEqual(model.machines.count, 1)
        XCTAssertEqual(try Data(contentsOf: indexURL), newer, "a newer index is not rewritten")
    }

    func testDeletedOpenedBundleDoesNotComeBackOnRefresh() throws {
        let away = try makeDirectory("Away.odvm", config: true, in: root.appendingPathComponent("Elsewhere", isDirectory: true))
        let model = makeModel()
        model.load()
        XCTAssertTrue(model.open(bundle: away))
        try model.delete(try XCTUnwrap(model.machines.first), deleteFiles: false)
        model.refresh()
        XCTAssertTrue(model.machines.isEmpty)
    }

    func testFilesDeletedButIndexNotWrittenIsNotAFailedDeletion() throws {
        let bundle = try makeBundle("Gone")
        let model = try makeModelWithUnwritableIndex()
        model.load()
        let machine = try XCTUnwrap(model.machines.first)
        XCTAssertNoThrow(try model.delete(machine, deleteFiles: true))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
        XCTAssertTrue(model.machines.isEmpty)
        XCTAssertTrue(model.banner?.contains("Файлы машины удалены") == true, model.banner ?? "nil")
        XCTAssertNotNil(model.bannerDetails)
    }

    func testRemovalOfADirectoryThatIsAlreadyGoneIsASuccess() throws {
        XCTAssertNoThrow(try LibraryModel.removeBundleFiles(at: root.appendingPathComponent("Nowhere.odvm")))
    }

    func testFileSystemErrorsAreInRussianWithTheCodeOnlyInTheDetails() {
        let underlying = NSError(domain: NSPOSIXErrorDomain, code: Int(EACCES))
        let wrapped = NSError(domain: NSCocoaErrorDomain, code: 513, userInfo: [NSUnderlyingErrorKey: underlying])
        let error = BundleRemovalError.from(wrapped)
        XCTAssertEqual(error, .io(EACCES), "the POSIX code, not the Cocoa code 513")
        XCTAssertTrue(error.description.contains("нет прав"))
        XCTAssertFalse(error.description.contains("513"))
        XCTAssertEqual(error.details?.contains("errno 13"), true)
        XCTAssertEqual(BundleRemovalError.from(NSError(domain: NSCocoaErrorDomain, code: 513)), .io(EACCES), "no underlying error: the Cocoa code is mapped")
        XCTAssertEqual(BundleRemovalError.from(NSError(domain: NSPOSIXErrorDomain, code: Int(EBUSY))).description, "файл занят другой программой")
        let unknown = BundleRemovalError.from(NSError(domain: "Other", code: 7))
        XCTAssertEqual(unknown.description, "ошибка файловой системы")
        XCTAssertEqual(unknown.details, "Other 7")
    }

    func testCatalogErrorsAreInRussianWithoutThePathInTheHeadline() {
        let notADirectory = LibraryFailure(CatalogError.notADirectory("/very/long/path"))
        XCTAssertFalse(notADirectory.text.contains("/very"))
        XCTAssertEqual(notADirectory.details?.contains("/very/long/path"), true)
        for error in [CatalogError.limitReached, .indexTooLarge, .notFound("/p"), .io(EACCES)] {
            let failure = LibraryFailure(error)
            XCTAssertNil(failure.text.range(of: "[A-Za-z]{4,}", options: .regularExpression), "the headline is in Russian: \(failure.text)")
        }
    }

    func testCopiedBundleIsMarkedAsAnIdentityConflict() throws {
        let identity = UUID().uuidString.lowercased()
        for name in ["A", "B"] {
            let bundle = defaultDirectory.appendingPathComponent("\(name).odvm", isDirectory: true)
            try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
            try BundleConfigStore.write(BundleConfig(name: name, identity: identity), bundle: bundle, mode: .create)
        }
        let model = makeModel()
        model.load()
        XCTAssertEqual(model.conflictingIDs.count, 2)
    }
}

// MARK: 4. the real VMHost behind a controller (test-only profile, no guest)

final class MachineControllerRealVMHostXCTests: XCTestCase {
    private func vmhostPath() -> String? {
        let candidates = [
            Bundle(for: Self.self).bundleURL.deletingLastPathComponent().appendingPathComponent("od-vmhost").path,
            URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent(".build/debug/od-vmhost").path,
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    /// Runs the main run loop (the client delivers on the main queue) until the condition holds.
    private func waitUntil(_ what: String, timeout: TimeInterval = 15, _ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        XCTAssertTrue(condition(), "timeout waiting for: \(what)")
    }

    func testLifecycleBusyAndExitOfATestOnlyMachine() throws {
        guard let path = vmhostPath() else { throw XCTSkip("od-vmhost is not built next to the tests (run `swift build` first)") }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("od-real-\(UUID().uuidString)", isDirectory: true)
        let bundleURL = root.appendingPathComponent("Real.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: bundleURL, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try BundleConfigStore.write(BundleConfig(name: "Real"), bundle: bundleURL, mode: .create)
        let location = VMHostLocation(url: URL(fileURLWithPath: path), source: .argument, isExecutable: true)
        let entry = CatalogEntry(bundleURL: bundleURL)
        let status = CatalogBundleStatus.config(BundleConfigCodec.read(bundle: bundleURL))
        let machine = MachineController(entry: entry, status: status, vmhost: location)
        var seen: [MachineRunState] = []
        let observer = machine.$runState.sink { if seen.last != $0 { seen.append($0) } }
        defer { observer.cancel() }

        machine.start()
        XCTAssertEqual(machine.runState, .starting)
        waitUntil("running") { machine.runState == .running && !machine.requestInFlight }
        XCTAssertEqual(machine.allowedActions, [.pause, .stop])

        // A second controller on the same bundle gets `busy` from the VMHost (the lock is held by the first one).
        let second = MachineController(entry: entry, status: status, vmhost: location)
        second.start()
        waitUntil("busy exit") { !second.hasSession && second.lastExit != nil }
        XCTAssertEqual(second.lastExit?.isBusy, true)
        XCTAssertEqual(second.runState, .stopped)
        XCTAssertEqual(second.notice?.text, "Не удалось запустить: машина уже запущена другим процессом.")
        XCTAssertEqual(machine.runState, .running, "the first machine is not touched")

        machine.pause()
        waitUntil("paused") { machine.runState == .paused && !machine.requestInFlight }
        XCTAssertEqual(machine.allowedActions, [.resume, .stop])
        machine.resume()
        waitUntil("running again") { machine.runState == .running && !machine.requestInFlight }
        machine.refresh()
        waitUntil("query answer") { machine.lastQueryText != nil }
        XCTAssertEqual(machine.lastQueryText?.contains("running"), true)
        machine.stop()
        waitUntil("exit") { !machine.hasSession }
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertEqual(machine.lastExit?.termination, .exited(0))
        XCTAssertNil(machine.notice)
        XCTAssertEqual(seen, [.stopped, .starting, .running, .paused, .running, .stopped])
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundleURL), "the lock is free after the exit")
        XCTAssertEqual(machine.allowedActions, [.start, .delete])
    }
}
