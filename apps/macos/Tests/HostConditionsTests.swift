import AppKit
import Darwin
import Foundation
@testable import ODShared
import XCTest
@testable import OpenDesktop

// Tests of the conditions of the host (S11-12, Sources/App/HostConditions.swift and the S11-12 part of MachineController.swift).
// No real sleep, no real memory pressure, no real change of the displays: the handlers take the events as plain calls, the notification
// centers are private ones, free space is injected. No window and no process: the controller gets a fake session. Every machine here is
// test-only, without a guest. Run: `swift test --filter HostConditions` from apps/.

// MARK: fakes

private final class HCSession: MachineSession {
    private(set) var calls: [String] = []
    private(set) var reasons: [ControlV2Reason] = []
    private var pending: [String: ControlClientV2.Completion] = [:]

    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion) { pending["configure"] = completion }
    func requestStart(completion: @escaping ControlClientV2.Completion) { pending["start"] = completion }
    func requestPause(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion) {
        calls.append("pause"); reasons.append(reason); pending["pause"] = completion
    }
    func requestResume(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion) {
        calls.append("resume"); reasons.append(reason); pending["resume"] = completion
    }
    func requestStop(completion: @escaping ControlClientV2.Completion) { calls.append("stop"); pending["stop"] = completion }
    func requestQuery(completion: @escaping ControlClientV2.Completion) { pending["query"] = completion }
    func terminateSession() { calls.append("terminate") }

    func ack(_ name: String, kind: ControlV2Kind, state: ControlV2State) {
        guard let completion = pending.removeValue(forKey: name) else { return XCTFail("\(name) was not requested") }
        completion(.success(.ack(ControlV2Ack(ackedKind: kind.rawValue, state: state.rawValue))))
    }

    func refuse(_ name: String, code: ControlV2ErrorCode, kind: ControlV2Kind, state: ControlV2State) {
        guard let completion = pending.removeValue(forKey: name) else { return XCTFail("\(name) was not requested") }
        let report = ControlV2ErrorReport(code: code, failedKind: UInt16(kind.rawValue), state: state.rawValue)
        completion(.failure(.refused(report)))
    }
}

private final class HCLauncher {
    var launches = 0
    var sessions: [HCSession] = []
    var handlers: [ControlClientV2.EventHandler] = []

    var launcher: MachineLauncher {
        { [self] _, handler in
            launches += 1
            handlers.append(handler)
            let session = HCSession()
            sessions.append(session)
            return session
        }
    }

    var session: HCSession { sessions[sessions.count - 1] }
    func emit(_ event: ControlClientEvent) { handlers[handlers.count - 1](event) }
}

private let hostVMHost = VMHostLocation(url: URL(fileURLWithPath: "/x/od-vmhost"), source: .argument, isExecutable: true)

private func makeMachine(
    _ launcher: HCLauncher, name: String = "Тест", bundle: URL? = nil, sleepPauseAudit: @escaping (UInt32) -> Void = { _ in }
) -> MachineController {
    let entry = CatalogEntry(bundleURL: bundle ?? URL(fileURLWithPath: "/x/\(name).odvm"))
    let machine = MachineController(
        entry: entry, status: .config(.valid(BundleConfig(name: name))), vmhost: hostVMHost, launcher: launcher.launcher,
        hostMemoryBytes: 64 << 30, sleepPauseAudit: sleepPauseAudit)
    // Plenty of space unless a test says otherwise: no test depends on the disk of the machine that runs it.
    machine.availableSpace = { _ in 100 << 30 }
    return machine
}

private func changed(_ from: ControlV2State, _ to: ControlV2State, reason: ControlV2Reason = .user) -> ControlClientEvent {
    .stateChanged(from: from, to: to, reason: reason.rawValue, requestID: 0)
}

/// Brings a machine to `running` the way the events do.
private func runToRunning(_ machine: MachineController, _ launcher: HCLauncher) {
    machine.start()
    launcher.emit(.ready)
    launcher.session.ack("configure", kind: .configure, state: .stopped)
    launcher.emit(changed(.stopped, .running))
    launcher.session.ack("start", kind: .start, state: .running)
}

/// A machine that the VMHost reports `paused` for `reason`, with its PAUSE answered.
private func pausedBy(_ reason: ControlV2Reason, _ machine: MachineController, _ launcher: HCLauncher) {
    launcher.emit(changed(.running, .paused, reason: reason))
    launcher.session.ack("pause", kind: .pause, state: .paused)
}

private func conditions(_ machines: [MachineController], store: SettingsStore = MemorySettingsStore()) -> HostConditions {
    HostConditions(machines: { machines }, store: store, workspaceCenter: NotificationCenter(), appCenter: NotificationCenter())
}

// MARK: sleep and wake

final class HostConditionsSleepXCTests: XCTestCase {
    func testSleepPausesOnlyRunningMachinesWithTheSystemSleepReason() {
        let a = HCLauncher(), b = HCLauncher()
        let running = makeMachine(a, name: "A")
        let stopped = makeMachine(b, name: "B")
        runToRunning(running, a)
        conditions([running, stopped]).handleWillSleep()
        XCTAssertEqual(a.session.calls, ["pause"])
        XCTAssertEqual(a.session.reasons, [.systemSleep])
        XCTAssertEqual(b.launches, 0, "a machine without a process gets nothing")
    }

    func testSleepDuringStartPausesAfterStartCompletes() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        machine.start()
        launcher.emit(.ready)
        launcher.session.ack("configure", kind: .configure, state: .stopped)
        let host = conditions([machine])

        host.handleWillSleep()
        launcher.emit(changed(.stopped, .running))
        XCTAssertEqual(launcher.session.calls, [], "do not overlap PAUSE with START")
        launcher.session.ack("start", kind: .start, state: .running)

        XCTAssertEqual(launcher.session.calls, ["pause"])
        XCTAssertEqual(launcher.session.reasons, [.systemSleep])
    }

    func testSleepDuringResumePausesAfterResumeCompletes() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        machine.pause()
        pausedBy(.user, machine, launcher)
        machine.resume()
        let host = conditions([machine])

        host.handleWillSleep()
        launcher.emit(changed(.paused, .running, reason: .user))
        XCTAssertEqual(launcher.session.calls, ["pause", "resume"], "do not overlap PAUSE with RESUME")
        launcher.session.ack("resume", kind: .resume, state: .running)

        XCTAssertEqual(launcher.session.calls, ["pause", "resume", "pause"])
        XCTAssertEqual(launcher.session.reasons, [.user, .user, .systemSleep])
    }

    func testSleepDuringRefusedDiskPauseRetriesAsSystemPause() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        machine.availableSpace = { _ in 0 }
        machine.checkDiskSpace()
        let host = conditions([machine])

        host.handleWillSleep()
        XCTAssertEqual(launcher.session.calls, ["pause"], "wait for the disk-space PAUSE already in flight")
        launcher.session.refuse("pause", code: .pauseTimeout, kind: .pause, state: .running)

        XCTAssertEqual(launcher.session.calls, ["pause", "pause"])
        XCTAssertEqual(launcher.session.reasons, [.diskSpace, .systemSleep])
    }

    func testWakeResumesAMachinePausedForTheSleepWithTheWakeReason() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        pausedBy(.systemSleep, machine, launcher)
        XCTAssertTrue(machine.isPausedBySystemSleep)
        host.handleDidWake()
        XCTAssertEqual(launcher.session.calls, ["pause", "resume"])
        XCTAssertEqual(launcher.session.reasons, [.systemSleep, .systemWake])
        launcher.emit(changed(.paused, .running, reason: .systemWake))
        launcher.session.ack("resume", kind: .resume, state: .running)
        XCTAssertEqual(machine.runState, .running)
        XCTAssertNil(machine.pauseReason)
        XCTAssertNil(machine.notice)
    }

    func testAPauseOfTheUserIsNeverResumedByTheWake() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        machine.pause() // the user
        pausedBy(.user, machine, launcher)
        XCTAssertEqual(machine.pauseReason, .user)
        XCTAssertFalse(machine.isPausedBySystemSleep)
        let host = conditions([machine])
        host.handleWillSleep()
        host.handleDidWake()
        XCTAssertEqual(launcher.session.calls, ["pause"], "no second PAUSE and no RESUME")
        XCTAssertEqual(launcher.session.reasons, [.user])
        XCTAssertEqual(machine.runState, .paused)
    }

    func testAPauseForFreeSpaceIsNotResumedByTheWake() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        machine.pause(reason: .diskSpace)
        pausedBy(.diskSpace, machine, launcher)
        conditions([machine]).handleDidWake()
        XCTAssertEqual(launcher.session.calls, ["pause"], "only a pause of the sleep is resumed")
    }

    func testTheWakeResumesOnlyTheMachinesThatTheSleepPaused() {
        let a = HCLauncher(), b = HCLauncher()
        let bySleep = makeMachine(a, name: "A")
        let byUser = makeMachine(b, name: "B")
        runToRunning(byUser, b)
        byUser.pause()
        pausedBy(.user, byUser, b)
        runToRunning(bySleep, a)
        let host = conditions([bySleep, byUser])
        host.handleWillSleep()
        pausedBy(.systemSleep, bySleep, a)
        host.handleDidWake()
        XCTAssertEqual(a.session.calls, ["pause", "resume"])
        XCTAssertEqual(b.session.calls, ["pause"], "the user's pause stays")
    }

    func testTheSettingOffSendsNoPauseAndIsStored() {
        let store = MemorySettingsStore()
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine], store: store)
        XCTAssertTrue(host.pauseOnSleep, "on by default")
        host.pauseOnSleep = false
        XCTAssertEqual(store.bool(forKey: HostConditions.pauseOnSleepKey), false)
        host.handleWillSleep()
        XCTAssertEqual(launcher.session.calls, [])
        XCTAssertFalse(conditions([], store: store).pauseOnSleep, "the next start reads the stored value")
    }

    func testAnUnsetSettingIsOn() {
        XCTAssertNil(MemorySettingsStore().bool(forKey: HostConditions.pauseOnSleepKey))
        XCTAssertTrue(conditions([]).pauseOnSleep)
    }

    func testARefusedPauseIsKeptForTheWakeNotSwallowed() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        launcher.session.refuse("pause", code: .pauseTimeout, kind: .pause, state: .running)
        XCTAssertNil(machine.notice, "nobody looks while the Mac sleeps: the line waits for the wake")
        XCTAssertEqual(machine.runState, .running, "a refusal does not change the state")
        host.handleDidWake()
        let notice = try XCTUnwrap(machine.notice)
        XCTAssertTrue(notice.text.contains("Пауза перед сном"), notice.text)
        XCTAssertTrue(notice.text.contains(ErrorPresentation.lowercasedFirst(ErrorCatalog.entry(for: ControlV2ErrorCode.pauseTimeout).title)), notice.text)
        XCTAssertTrue(notice.details?.contains("pause_timeout") == true, "the details name the code")
        XCTAssertEqual(launcher.session.calls, ["pause"], "nothing to resume: the machine never paused")
    }

    func testSystemPauseRefusalIsAuditedWithoutMachineDetails() throws {
        let launcher = HCLauncher()
        var auditedCodes: [UInt32] = []
        let machine = makeMachine(launcher, name: "Private machine name", sleepPauseAudit: { auditedCodes.append($0) })
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        launcher.session.refuse("pause", code: .pauseTimeout, kind: .pause, state: .running)

        XCTAssertEqual(auditedCodes, [ControlV2ErrorCode.pauseTimeout.rawValue])
        XCTAssertNil(machine.notice, "the refusal is still presented after wake")
        host.handleDidWake()
        XCTAssertNotNil(machine.notice, "the refusal is still shown after wake")
    }

    func testAPauseRefusalThatArrivesAfterTheWakeIsShownAtOnce() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        host.handleDidWake()
        XCTAssertNil(machine.notice)
        launcher.session.refuse("pause", code: .pauseTimeout, kind: .pause, state: .running)
        XCTAssertNotNil(machine.notice)
        XCTAssertEqual(launcher.session.calls, ["pause"])
    }

    func testAWakeBeforeThePauseLandedResumesWhenItLands() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        host.handleDidWake() // the Mac woke at once: PAUSE is still unanswered
        XCTAssertEqual(launcher.session.calls, ["pause"])
        pausedBy(.systemSleep, machine, launcher)
        XCTAssertEqual(launcher.session.calls, ["pause", "resume"], "the machine is not left paused by the system")
        XCTAssertEqual(launcher.session.reasons, [.systemSleep, .systemWake])
    }

    func testARefusedResumeAfterTheWakeIsShown() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        pausedBy(.systemSleep, machine, launcher)
        host.handleDidWake()
        launcher.session.refuse("resume", code: .invalidState, kind: .resume, state: .paused)
        let notice = try XCTUnwrap(machine.notice)
        XCTAssertTrue(notice.text.contains("Продолжение после сна"), notice.text)
        XCTAssertEqual(machine.runState, .paused)
    }

    func testASleepDuringAnotherRequestSaysThatNoPauseWasSent() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        machine.pause() // the user: the request is in flight
        let host = conditions([machine])
        host.handleWillSleep()
        XCTAssertEqual(launcher.session.calls, ["pause"], "no second request while one is in flight")
        host.handleDidWake()
        XCTAssertEqual(machine.notice?.text, ErrorCatalog.entry(for: HostConditionIssue.sleepPauseSkipped).title)
    }

    func testTheWakeIgnoresTheSettingSoAnEarlierSystemPauseStillContinues() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        host.handleWillSleep()
        pausedBy(.systemSleep, machine, launcher)
        host.pauseOnSleep = false // turned off while the Mac sleeps
        host.handleDidWake()
        XCTAssertEqual(launcher.session.calls, ["pause", "resume"])
    }
}

// MARK: notifications

final class HostConditionsNotificationXCTests: XCTestCase {
    private func spin(_ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(2)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
    }

    /// The names and the centers of the documentation: the sleep and the wake on the workspace center, the displays on the other one.
    func testSystemNotificationsReachTheHandlers() {
        let workspace = NotificationCenter(), app = NotificationCenter()
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = HostConditions(machines: { [machine] }, store: MemorySettingsStore(), workspaceCenter: workspace, appCenter: app)
        var screens = 0
        host.onScreenParametersChange = { screens += 1 }
        host.subscribeToNotifications()
        defer { host.stop() }

        workspace.post(name: NSWorkspace.willSleepNotification, object: nil)
        spin { launcher.session.calls.contains("pause") }
        XCTAssertEqual(launcher.session.reasons, [.systemSleep])

        pausedBy(.systemSleep, machine, launcher)
        workspace.post(name: NSWorkspace.didWakeNotification, object: nil)
        spin { launcher.session.calls.contains("resume") }
        XCTAssertEqual(launcher.session.reasons, [.systemSleep, .systemWake])

        app.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        spin { screens == 1 }
        XCTAssertEqual(screens, 1)

        // The wrong center receives nothing (the documentation: a different center does not get the workspace notifications).
        app.post(name: NSWorkspace.willSleepNotification, object: nil)
        workspace.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        XCTAssertEqual(screens, 1)
        XCTAssertEqual(launcher.session.calls, ["pause", "resume"])
    }

    func testStopRemovesTheObservers() {
        let workspace = NotificationCenter()
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = HostConditions(machines: { [machine] }, store: MemorySettingsStore(), workspaceCenter: workspace, appCenter: NotificationCenter())
        host.subscribeToNotifications()
        host.stop()
        workspace.post(name: NSWorkspace.willSleepNotification, object: nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        XCTAssertEqual(launcher.session.calls, [])
    }
}

// MARK: free space

final class HostConditionsDiskSpaceXCTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("host-conditions-\(getpid())-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private let gib: UInt64 = 1 << 30

    func testThresholdsAreStrictlyBelow() {
        let t = DiskSpaceThresholds.hypothesis
        XCTAssertEqual(t.warningBytes, 5 * gib)
        XCTAssertEqual(t.hardBytes, 1 * gib)
        XCTAssertEqual(t.level(availableBytes: 100 * gib), .normal)
        XCTAssertEqual(t.level(availableBytes: 5 * gib), .normal, "exactly the warning threshold is still normal")
        XCTAssertEqual(t.level(availableBytes: 5 * gib - 1), .warning)
        XCTAssertEqual(t.level(availableBytes: 1 * gib), .warning, "exactly the hard threshold is still a warning")
        XCTAssertEqual(t.level(availableBytes: 1 * gib - 1), .critical)
        XCTAssertEqual(t.level(availableBytes: 0), .critical)
    }

    func testTheRealVolumeAnswersForATemporaryDirectoryAndItsMissingChild() throws {
        let existing = try XCTUnwrap(CreateMachineModel.volumeAvailableBytes(at: root))
        XCTAssertGreaterThan(existing, 0)
        let missing = root.appendingPathComponent("not/created/yet.odvm")
        XCTAssertNotNil(CreateMachineModel.volumeAvailableBytes(at: missing), "the nearest existing ancestor tells the volume")
    }

    /// The injected capacity decides, and it is asked for the bundle of the machine.
    func testTheLevelComesFromTheInjectedCapacityOfTheBundleVolume() throws {
        let bundle = root.appendingPathComponent("Диск.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        let launcher = HCLauncher()
        let machine = makeMachine(launcher, name: "Диск", bundle: bundle)
        var asked: [URL] = []
        var free = 500 * (1 << 20) as UInt64
        machine.availableSpace = { asked.append($0); return free }
        machine.start()
        XCTAssertEqual(launcher.launches, 0, "below the hard threshold nothing is spawned")
        XCTAssertEqual(asked, [bundle])
        free = 3 * gib
        machine.start()
        XCTAssertEqual(launcher.launches, 1, "a warning does not stop the start")
        XCTAssertEqual(machine.notice?.text, ErrorCatalog.entry(for: HostConditionIssue.diskSpaceLow).title)
    }

    func testStartBelowTheHardThresholdIsRefusedWithTheWordsOfTheCatalog() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        machine.availableSpace = { _ in 100 << 20 }
        machine.start()
        XCTAssertEqual(launcher.launches, 0)
        XCTAssertEqual(machine.runState, .stopped)
        XCTAssertFalse(machine.hasSession)
        let notice = try XCTUnwrap(machine.notice)
        XCTAssertEqual(notice.text, "Не удалось запустить: не хватает места для запуска машины.")
        XCTAssertEqual(notice.hint, ErrorPresentation.host(.diskSpaceStartRefused).message)
        XCTAssertTrue(notice.details?.contains("Свободно на томе: 100") == true, notice.details ?? "")
        XCTAssertTrue(machine.allows(.start), "the user may start again after freeing the space")
    }

    func testStartWithUnknownSpaceIsNotBlocked() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        machine.availableSpace = { _ in nil }
        machine.start()
        XCTAssertEqual(launcher.launches, 1)
        XCTAssertNil(machine.notice)
    }

    func testRunningMachineBelowTheHardThresholdIsPausedWithTheDiskSpaceReason() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        machine.availableSpace = { _ in 512 << 20 }
        host.handleDiskCheck()
        XCTAssertEqual(launcher.session.calls, ["pause"])
        XCTAssertEqual(launcher.session.reasons, [.diskSpace])
        XCTAssertEqual(machine.notice?.text, ErrorCatalog.entry(for: HostConditionIssue.diskSpacePaused).title)
        pausedBy(.diskSpace, machine, launcher)
        XCTAssertEqual(machine.pauseReason, .diskSpace)
        XCTAssertEqual(machine.notice?.text, ErrorCatalog.entry(for: HostConditionIssue.diskSpacePaused).title, "the line stays while it is paused")
        host.handleDiskCheck()
        XCTAssertEqual(launcher.session.calls, ["pause"], "a paused machine is not paused again")
    }

    func testRefusedDiskPauseReplacesTheLineAndTheNextCheckTriesAgain() throws {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        machine.availableSpace = { _ in 512 << 20 }
        host.handleDiskCheck()
        launcher.session.refuse("pause", code: .pauseTimeout, kind: .pause, state: .running)
        XCTAssertTrue(try XCTUnwrap(machine.notice).text.hasPrefix("Пауза:"), "the refusal is shown, not swallowed")
        host.handleDiskCheck()
        XCTAssertEqual(launcher.session.calls, ["pause", "pause"])
    }

    func testWarningLevelShowsTheLineOnceAndNormalResetsIt() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        machine.availableSpace = { _ in 3 << 30 }
        host.handleDiskCheck()
        XCTAssertEqual(machine.notice?.text, ErrorCatalog.entry(for: HostConditionIssue.diskSpaceLow).title)
        XCTAssertEqual(launcher.session.calls, [], "a warning pauses nothing")
        machine.dismissNotice()
        host.handleDiskCheck()
        XCTAssertNil(machine.notice, "once per low period, not every 30 s")
        machine.availableSpace = { _ in 50 << 30 }
        host.handleDiskCheck()
        machine.availableSpace = { _ in 3 << 30 }
        host.handleDiskCheck()
        XCTAssertNotNil(machine.notice, "after the space was normal again the warning is shown again")
    }

    func testNormalSpaceSaysAndDoesNothing() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        conditions([machine]).handleDiskCheck()
        XCTAssertEqual(launcher.session.calls, [])
        XCTAssertNil(machine.notice)
    }

    func testAStoppedMachineIsNotChecked() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        var asked = 0
        machine.availableSpace = { _ in asked += 1; return 0 }
        conditions([machine]).handleDiskCheck()
        XCTAssertEqual(asked, 0, "the check of 30 s is for running machines only")
    }

    func testTheOnlyTimerIsThirtySeconds() {
        XCTAssertEqual(HostConditions.diskCheckInterval, 30)
    }
}

// MARK: memory

final class HostConditionsMemoryXCTests: XCTestCase {
    func testTheWorstLevelOfTheMaskWins() {
        XCTAssertEqual(MemoryPressureLevel(event: .warning), .warning)
        XCTAssertEqual(MemoryPressureLevel(event: .critical), .critical)
        XCTAssertEqual(MemoryPressureLevel(event: .normal), .normal)
        XCTAssertEqual(MemoryPressureLevel(event: [.warning, .critical]), .critical)
        XCTAssertNil(MemoryPressureLevel(event: []))
    }

    func testBannerOffersToStopMachinesAndStopsNothing() throws {
        let warning = try XCTUnwrap(MemoryBanner.presentation(level: .warning, hasLiveMachines: true, dismissed: false))
        XCTAssertEqual(warning.title, ErrorCatalog.entry(for: HostConditionIssue.memoryPressureWarning).title)
        XCTAssertTrue(warning.message.contains("Остановите машины"))
        XCTAssertTrue(warning.message.contains("не остановит их само"))
        let critical = try XCTUnwrap(MemoryBanner.presentation(level: .critical, hasLiveMachines: true, dismissed: false))
        XCTAssertEqual(critical.title, ErrorCatalog.entry(for: HostConditionIssue.memoryPressureCritical).title)
        XCTAssertNil(MemoryBanner.presentation(level: .normal, hasLiveMachines: true, dismissed: false))
        XCTAssertNil(MemoryBanner.presentation(level: .critical, hasLiveMachines: false, dismissed: false), "no machine, no offer")
        XCTAssertNil(MemoryBanner.presentation(level: .critical, hasLiveMachines: true, dismissed: true))
    }

    func testPressureReachesTheLibraryAndTheMachinesAreLeftAlone() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let index = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("hc-lib-\(getpid())-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: index) }
        let library = LibraryModel(
            catalog: Catalog(indexURL: index.appendingPathComponent("Library.plist"), defaultDirectory: index), vmhost: hostVMHost,
            launcher: launcher.launcher, hostMemoryBytes: 64 << 30)
        let host = conditions([machine])
        host.onMemoryPressure = { library.setMemoryPressure($0) }
        host.handleMemoryPressure(.critical)
        XCTAssertEqual(library.memoryPressure, .critical)
        XCTAssertEqual(launcher.session.calls, [], "the application stops and pauses nothing by itself")
        XCTAssertEqual(machine.runState, .running)
        library.dismissMemoryBanner()
        host.handleMemoryPressure(.warning)
        XCTAssertEqual(library.memoryPressure, .warning)
        host.handleMemoryPressure(.normal)
        XCTAssertEqual(library.memoryPressure, .normal)
        XCTAssertNil(library.memoryBanner)
    }
}

// MARK: windows on displays

final class HostConditionsWindowPlacementXCTests: XCTestCase {
    private let primary = NSRect(x: 0, y: 25, width: 1440, height: 850)
    private let external = NSRect(x: 1440, y: 0, width: 1920, height: 1055)

    func testAWindowOnAnExistingDisplayStays() {
        let frame = NSRect(x: 100, y: 100, width: 800, height: 600)
        XCTAssertEqual(WindowPlacement.adjusted(frame, visibleFrames: [primary, external], primary: primary), frame)
        let onExternal = NSRect(x: 1600, y: 100, width: 800, height: 600)
        XCTAssertEqual(WindowPlacement.adjusted(onExternal, visibleFrames: [primary, external], primary: primary), onExternal)
    }

    func testAWindowOnADisplayThatIsGoneMovesToThePrimaryOne() {
        let onExternal = NSRect(x: 1600, y: 100, width: 800, height: 600)
        let moved = WindowPlacement.adjusted(onExternal, visibleFrames: [primary], primary: primary)
        XCTAssertEqual(moved.size, onExternal.size, "the size is kept when it fits")
        XCTAssertEqual(moved.midX, primary.midX, accuracy: 0.5)
        XCTAssertEqual(moved.midY, primary.midY, accuracy: 0.5)
        XCTAssertTrue(primary.contains(moved))
    }

    func testAWindowLargerThanThePrimaryDisplayIsCutToItsVisibleArea() {
        let huge = NSRect(x: 1500, y: 0, width: 3000, height: 2000)
        let moved = WindowPlacement.adjusted(huge, visibleFrames: [primary], primary: primary)
        XCTAssertEqual(moved, primary)
    }

    func testAPartlyVisibleWindowStaysAndAHairlineOfItDoesNot() {
        // 200 x 300 points inside the primary display: reachable.
        let partly = NSRect(x: 1240, y: 100, width: 800, height: 600)
        XCTAssertEqual(WindowPlacement.adjusted(partly, visibleFrames: [primary], primary: primary), partly)
        // 10 points inside: the title bar cannot be grabbed, the window moves.
        let hairline = NSRect(x: 1430, y: 100, width: 800, height: 600)
        XCTAssertNotEqual(WindowPlacement.adjusted(hairline, visibleFrames: [primary], primary: primary), hairline)
        XCTAssertFalse(WindowPlacement.isReachable(hairline, in: primary))
        XCTAssertTrue(WindowPlacement.isReachable(partly, in: primary))
    }

    func testADisplayLeftOfThePrimaryOneHasNegativeCoordinates() {
        let left = NSRect(x: -1920, y: 0, width: 1920, height: 1055)
        let frame = NSRect(x: -1500, y: 100, width: 800, height: 600)
        XCTAssertEqual(WindowPlacement.adjusted(frame, visibleFrames: [left, primary], primary: primary), frame)
        let moved = WindowPlacement.adjusted(frame, visibleFrames: [primary], primary: primary)
        XCTAssertTrue(primary.contains(moved))
    }

    func testAWindowSmallerThanTheMinimumCountsAsVisibleWhenWhole() {
        let tiny = NSRect(x: 50, y: 100, width: 80, height: 40)
        XCTAssertTrue(WindowPlacement.isReachable(tiny, in: primary))
    }
}

// MARK: simulated events (--simulate-host-event)

final class HostConditionsSimulationXCTests: XCTestCase {
    func testTheArgumentNamesTheFiveEvents() {
        XCTAssertEqual(HostEventKind.allCases.map(\.rawValue), ["sleep", "wake", "screens", "low-space", "memory"])
    }

    func testSimulatedEventsRunTheSameHandlers() {
        let launcher = HCLauncher()
        let machine = makeMachine(launcher)
        runToRunning(machine, launcher)
        let host = conditions([machine])
        var screens = 0
        var levels: [MemoryPressureLevel] = []
        host.onScreenParametersChange = { screens += 1 }
        host.onMemoryPressure = { levels.append($0) }
        host.simulate(.screens)
        host.simulate(.memory)
        XCTAssertEqual(screens, 1)
        XCTAssertEqual(levels, [.critical])
        host.simulate(.sleep)
        XCTAssertEqual(launcher.session.reasons, [.systemSleep])
        pausedBy(.systemSleep, machine, launcher)
        host.simulate(.wake)
        XCTAssertEqual(launcher.session.reasons, [.systemSleep, .systemWake])
        launcher.emit(changed(.paused, .running, reason: .systemWake))
        launcher.session.ack("resume", kind: .resume, state: .running)
        host.simulate(.lowSpace)
        XCTAssertEqual(launcher.session.reasons.last, .diskSpace)
    }
}
