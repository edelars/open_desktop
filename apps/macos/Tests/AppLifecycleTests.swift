import Foundation
import XCTest
@testable import OpenDesktop

// Tests of the quit of the application (S11-11, Sources/App/AppLifecycle.swift). No window and no process: the decision is a pure function and
// the waiting runs against fake machines and a manual clock. Every machine here is test-only, without a guest.
// Run: `swift test --filter AppLifecycle` from apps/.

private func candidate(_ name: String, _ state: MachineRunState, session: Bool = true) -> QuitCandidate {
    QuitCandidate(name: name, state: state, hasSession: session)
}

final class QuitRulesXCTests: XCTestCase {
    func testNoMachinesQuitsAtOnce() {
        XCTAssertEqual(QuitRules.decision([]), .terminateNow)
    }

    func testStoppedMachinesWithoutProcessesQuitAtOnce() {
        XCTAssertEqual(QuitRules.decision([candidate("A", .stopped, session: false), candidate("B", .failed, session: false)]), .terminateNow)
    }

    func testARunningMachineAsksAndIsNamed() {
        XCTAssertEqual(QuitRules.decision([candidate("A", .running)]), .confirm(names: ["A"]))
    }

    func testPausedMachinesAskToo() {
        XCTAssertEqual(
            QuitRules.decision([candidate("A", .paused), candidate("B", .stopped, session: false), candidate("C", .running)]),
            .confirm(names: ["A", "C"]), "only the machines that run or are paused are named, in the order of the library")
    }

    func testNoRunningMachineMeansNoQuestion() {
        // A start that has not finished is ended without asking: there is nothing of a guest to lose yet.
        // A machine that is shutting down is asked about too: its guest may be in the middle of a clean shutdown.
        XCTAssertEqual(QuitRules.decision([candidate("A", .starting)]), .endProcesses)
        XCTAssertEqual(QuitRules.decision([candidate("A", .stopped), candidate("B", .shuttingDown)]), .confirm(names: ["B"]))
        XCTAssertEqual(QuitRules.decision([candidate("A", .failed)]), .endProcesses)
    }

    func testARunningStateWithoutAProcessIsNotAskedAbout() {
        // The row says `running` only while the controller has a session; a leftover state without one cannot make the quit wait.
        XCTAssertEqual(QuitRules.decision([candidate("A", .running, session: false)]), .terminateNow)
    }

    func testTheSheetNamesTheMachinesAndNeverPromisesToSave() {
        let one = QuitRules.message(names: ["Тест"])
        XCTAssertTrue(one.contains("«Тест»"))
        XCTAssertTrue(one.contains("не сохраняется"), "the state of the guest is not saved")
        let many = QuitRules.message(names: ["A", "B"])
        XCTAssertTrue(many.contains("«A», «B»"))
        for text in [one, many, QuitRules.title(names: ["A"]), QuitRules.title(names: ["A", "B"]), QuitRules.stopButton, QuitRules.cancelButton] {
            XCTAssertFalse(text.lowercased().contains("сохранить"), text)
            XCTAssertFalse(text.lowercased().contains("сохранение"), text)
        }
        XCTAssertEqual(QuitRules.stopButton, "Остановить и выйти")
        XCTAssertEqual(QuitRules.cancelButton, "Отмена")
    }

    func testTimeLimits() {
        XCTAssertEqual(QuitRules.stopTimeout, 10)
        XCTAssertGreaterThan(QuitRules.forceGrace, 0.5, "room for SIGTERM, 500 ms and SIGKILL")
    }
}

private final class FakeTarget: QuitTarget {
    var hasSession = true
    var canStop = true
    var stops = 0
    var forces = 0
    private var handlers: [() -> Void] = []

    func stopForQuit() -> Bool {
        if canStop { stops += 1 }
        return canStop
    }
    func forceTerminate() { forces += 1 }
    func notifyWhenProcessEnded(_ handler: @escaping () -> Void) {
        if hasSession { handlers.append(handler) } else { handler() }
    }
    /// The process ends: the session goes first, then the handlers run (as in `MachineController.finish`).
    func end() {
        hasSession = false
        let run = handlers
        handlers = []
        run.forEach { $0() }
    }
}

private final class ManualClock {
    private(set) var scheduled: [(delay: TimeInterval, work: () -> Void)] = []
    var schedule: QuitTeardown.Schedule { { [self] delay, work in scheduled.append((delay, work)) } }
    func fire(upTo delay: TimeInterval) {
        for item in scheduled.sorted(by: { $0.delay < $1.delay }) where item.delay <= delay { item.work() }
    }
}

final class QuitTeardownXCTests: XCTestCase {
    private func make(_ targets: [FakeTarget], clock: ManualClock, completions: @escaping (Int) -> Void) -> QuitTeardown {
        QuitTeardown(targets: targets, schedule: clock.schedule, completion: completions)
    }

    func testEveryMachineGetsStopAndTheQuitWaitsForAllOfThem() {
        let a = FakeTarget(), b = FakeTarget()
        let clock = ManualClock()
        var results: [Int] = []
        make([a, b], clock: clock) { results.append($0) }.run(stopTimeout: 10, forceGrace: 3)

        XCTAssertEqual([a.stops, b.stops], [1, 1])
        XCTAssertEqual([a.forces, b.forces], [0, 0], "nothing is ended by force while STOP can still work")
        a.end()
        XCTAssertTrue(results.isEmpty, "one machine is still stopping")
        b.end()
        XCTAssertEqual(results, [0])
        clock.fire(upTo: 100) // the timers of a finished quit do nothing
        XCTAssertEqual(results, [0], "the completion runs once")
        XCTAssertEqual([a.forces, b.forces], [0, 0])
    }

    func testTimeoutEndsTheStragglerByForceAndThenFinishes() {
        let quick = FakeTarget(), slow = FakeTarget()
        let clock = ManualClock()
        var results: [Int] = []
        make([quick, slow], clock: clock) { results.append($0) }.run(stopTimeout: 10, forceGrace: 3)
        quick.end()
        XCTAssertEqual(clock.scheduled.map { $0.delay }.sorted(), [10, 13])

        clock.fire(upTo: 10)
        XCTAssertEqual([quick.forces, slow.forces], [0, 1], "after the time limit only the machine that has not ended gets SIGTERM/SIGKILL")
        XCTAssertTrue(results.isEmpty)
        slow.end() // the client reaped it
        XCTAssertEqual(results, [0])
    }

    func testAMachineThatCannotTakeStopIsEndedAtOnce() {
        let starting = FakeTarget()
        starting.canStop = false
        let clock = ManualClock()
        var results: [Int] = []
        make([starting], clock: clock) { results.append($0) }.run()

        XCTAssertEqual(starting.stops, 0)
        XCTAssertEqual(starting.forces, 1, "SIGTERM, 500 ms, SIGKILL by the client, without waiting for the time limit")
        starting.end()
        XCTAssertEqual(results, [0])
    }

    func testAProcessThatSurvivesEverythingIsReportedAndDoesNotHoldTheQuit() {
        let stuck = FakeTarget()
        let clock = ManualClock()
        var results: [Int] = []
        make([stuck], clock: clock) { results.append($0) }.run(stopTimeout: 10, forceGrace: 3)
        clock.fire(upTo: 13)
        XCTAssertEqual(results, [1], "the application ends anyway; the number of processes left is reported")
        XCTAssertEqual(stuck.forces, 1)
    }

    func testNothingAliveFinishesAtOnce() {
        let gone = FakeTarget()
        gone.hasSession = false
        let clock = ManualClock()
        var results: [Int] = []
        make([gone], clock: clock) { results.append($0) }.run()
        XCTAssertEqual(results, [0])
        XCTAssertEqual(gone.stops, 0)
        XCTAssertTrue(clock.scheduled.isEmpty)
    }
}
