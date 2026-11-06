import AppKit
import Foundation

// Quit of the application with machines that run (S11-11). `applicationShouldTerminate` asks when a machine runs or is paused, sends `STOP`
// to every machine that can take it, waits for the processes to end (or for the time limit), ends what is left by the policy of the client
// (SIGTERM, 500 ms, SIGKILL: `ControlClientV2.terminate`) and only then lets the application end. A quit without a live machine process
// is not delayed.
//
// The words never say "сохранить": the state of the guest is not saved (there is no suspend, STAGE_11.md 4.2). `STOP` of a running machine
// is a stop, not a save.
//
// The decision and the waiting are pure types (`QuitRules`, `QuitTeardown`) tested without a window (AppLifecycleTests.swift); `AppLifecycle`
// only puts the sheet on the screen and answers AppKit.

// MARK: decision

/// What the quit rule needs to know about one machine.
struct QuitCandidate: Equatable {
    var name: String
    var state: MachineRunState
    /// A VMHost process of this machine exists (the controller has a session).
    var hasSession: Bool
}

enum QuitDecision: Equatable {
    /// No machine process: the application ends at once.
    case terminateNow
    /// Processes exist but none runs or is paused (a start that has not finished, a process that is going away): end them without a question.
    case endProcesses
    /// At least one machine runs or is paused: ask, naming these machines.
    case confirm(names: [String])
}

enum QuitRules {
    /// Time that the machines get to answer `STOP` and exit before the client ends their processes.
    static let stopTimeout: TimeInterval = 10
    /// After `stopTimeout` the client needs SIGTERM, 500 ms and SIGKILL; this is the room for it before the application ends anyway.
    static let forceGrace: TimeInterval = 3

    static func decision(_ machines: [QuitCandidate]) -> QuitDecision {
        let alive = machines.filter { $0.hasSession }
        guard !alive.isEmpty else { return .terminateNow }
        // A machine that is shutting down right now is not stopped silently: the question is asked, as for a running one.
        let working = alive.filter { $0.state == .running || $0.state == .paused || $0.state == .shuttingDown }
        guard !working.isEmpty else { return .endProcesses }
        return .confirm(names: working.map { $0.name })
    }

    static let stopButton = "Остановить и выйти"
    static let cancelButton = "Отмена"

    static func title(names: [String]) -> String {
        names.count == 1 ? "Остановить машину и выйти из Open Desktop?" : "Остановить машины и выйти из Open Desktop?"
    }

    /// The list of names and the consequence. No word "сохранить": the state of the guest is not saved.
    static func message(names: [String]) -> String {
        let list = names.map { "«\($0)»" }.joined(separator: ", ")
        let verb = names.count == 1 ? "Работает машина" : "Работают машины"
        return "\(verb) \(list). Перед выходом они получат команду остановки. "
            + "Состояние гостевой системы не сохраняется: несохранённые данные в ней будут потеряны."
    }
}

// MARK: waiting for the processes

/// A machine as the quit sees it. `MachineController` is the one implementation; the tests use a fake.
protocol QuitTarget: AnyObject {
    var hasSession: Bool { get }
    /// Sends `STOP` when the machine can take it; `false` when it cannot now (the caller ends the process instead).
    @discardableResult func stopForQuit() -> Bool
    /// SIGTERM, 500 ms, SIGKILL by the client.
    func forceTerminate()
    /// Runs the handler once, when the process has ended (at once when there is none).
    func notifyWhenProcessEnded(_ handler: @escaping () -> Void)
}

extension MachineController: QuitTarget {}

/// Stops the machines and waits. The completion runs once, with the number of processes that were still alive at the end (0 on a clean run).
final class QuitTeardown {
    typealias Schedule = (TimeInterval, @escaping () -> Void) -> Void

    private let targets: [QuitTarget]
    private let schedule: Schedule
    private let completion: (Int) -> Void
    private var remaining = 0
    private var finished = false

    /// `schedule` runs a closure after a delay on the queue where the targets live (the main queue); the tests pass their own clock.
    init(targets: [QuitTarget], schedule: @escaping Schedule, completion: @escaping (Int) -> Void) {
        self.targets = targets.filter { $0.hasSession }
        self.schedule = schedule
        self.completion = completion
    }

    func run(stopTimeout: TimeInterval = QuitRules.stopTimeout, forceGrace: TimeInterval = QuitRules.forceGrace) {
        guard !targets.isEmpty else { return finish() }
        remaining = targets.count
        for target in targets {
            target.notifyWhenProcessEnded { [self] in processEnded() }
            // A machine that cannot take `STOP` now (it has not started, a request is in flight) is ended at once: its guest state is
            // not saved either way, and the question said so.
            if !target.stopForQuit() { target.forceTerminate() }
        }
        schedule(stopTimeout) { [self] in
            guard !finished else { return }
            for target in targets where target.hasSession { target.forceTerminate() }
        }
        schedule(stopTimeout + forceGrace) { [self] in finish() }
    }

    private func processEnded() {
        remaining -= 1
        if remaining <= 0 { finish() }
    }

    private func finish() {
        guard !finished else { return }
        finished = true
        completion(targets.filter { $0.hasSession }.count)
    }
}

// MARK: application delegate

final class AppLifecycle: NSObject, NSApplicationDelegate {
    /// The machines of the library at the time of the quit. Set by the application before the first window; a delegate has no other way
    /// to reach the model (SwiftUI creates it).
    static var machines: () -> [MachineController] = { [] }

    private var quitInProgress = false
    private var teardown: QuitTeardown?

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // A second quit while the sheet is open or the machines are stopping waits for the first one.
        if quitInProgress { return .terminateLater }
        let machines = Self.machines()
        let candidates = machines.map { QuitCandidate(name: $0.displayName, state: $0.runState, hasSession: $0.hasSession) }
        switch QuitRules.decision(candidates) {
        case .terminateNow:
            return .terminateNow
        case .endProcesses:
            quitInProgress = true
            stopAndQuit(machines)
            return .terminateLater
        case .confirm(let names):
            quitInProgress = true
            ask(names: names) { [self] confirmed in
                if confirmed {
                    stopAndQuit(machines)
                } else {
                    quitInProgress = false
                    sender.reply(toApplicationShouldTerminate: false)
                }
            }
            return .terminateLater
        }
    }

    private func stopAndQuit(_ machines: [MachineController]) {
        let teardown = QuitTeardown(
            targets: machines,
            schedule: { delay, work in DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work) },
            completion: { leftAlive in
                if leftAlive > 0 {
                    FileHandle.standardError.write(Data("OpenDesktop: при выходе остались процессы машин: \(leftAlive)\n".utf8))
                }
                NSApp.reply(toApplicationShouldTerminate: true)
            })
        self.teardown = teardown
        teardown.run()
    }

    private func ask(names: [String], answer: @escaping (Bool) -> Void) {
        let alert = NSAlert()
        alert.messageText = QuitRules.title(names: names)
        alert.informativeText = QuitRules.message(names: names)
        alert.alertStyle = .warning
        alert.addButton(withTitle: QuitRules.stopButton)
        alert.addButton(withTitle: QuitRules.cancelButton)
        if let window = NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first(where: { $0.isVisible }) {
            alert.beginSheetModal(for: window) { response in answer(response == .alertFirstButtonReturn) }
        } else {
            answer(alert.runModal() == .alertFirstButtonReturn)
        }
    }
}
