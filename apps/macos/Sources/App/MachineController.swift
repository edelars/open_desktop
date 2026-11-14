import Foundation
import ODShared

// State of one machine row of the library (S11-06b). The rule of stage 11 (STAGE_11.md 4.1): the state of a machine is what the events of
// the control client (`stateChanged`, `exited`, `startRejected`) and the exit status of the VMHost say. A live pid, an open window or a
// reply to `QUERY` does not change the row; "running" is shown only after `stateChanged(to: running)`. Nothing here polls: `refresh()`
// sends one `QUERY` when the user asks for it.
//
// This file imports no UI framework: the controller and the pure functions are tested without windows (MachineControllerTests.swift).
// The controller works on the main queue (the client delivers events on `.main`). It is a local observable of the library, not a global
// store (ARCHITECTURE section 3). The application executes no guest code and links no engine: it only drives a VMHost process.

extension CatalogBundleStatus {
    /// The configuration, when the bundle is a directory with a valid `config.plist`.
    var config: BundleConfig? {
        if case .config(let state) = self { return state.config }
        return nil
    }
}

// MARK: states and actions

/// What the row shows. `starting` is the controller's own state between "the process was spawned" and the first `stateChanged`: the VMHost
/// is `stopped` at that time, and the row does not claim more.
enum MachineRunState: Equatable {
    case stopped
    case starting
    case running
    case paused
    case shuttingDown
    case failed

    init(_ wire: ControlV2State) {
        switch wire {
        case .stopped: self = .stopped
        case .running: self = .running
        case .paused: self = .paused
        case .shuttingDown: self = .shuttingDown
        case .failed: self = .failed
        }
    }
}

enum MachineAction: CaseIterable, Hashable {
    case start
    case pause
    case resume
    case stop
    /// Ends a process that has not yet reported its state (`starting`): SIGTERM, then SIGKILL. The way out of a VMHost that never says READY.
    case abort
    case delete
}

/// Which buttons are active: the table "state x operation" of CONTROL_PROTOCOL_V2.md 7.2 read for the requests the application sends.
/// A pure function of its arguments.
///
/// - `START` is valid in `stopped` (the application sends `CONFIGURE` first, in the same start action); `PAUSE` in `running`; `RESUME`
///   in `paused`; `STOP` in `running`, `paused` and (as the one exception to the rule "no request while another is in flight") in
///   `shutting_down`. Every other cell is `invalid_state`, `operation_in_progress` or "closed", so the button is not active.
/// - Only one request besides `QUERY` may be in flight (section 5): while one is, nothing is active.
/// - `failed` is terminal for the process and `exited` is gone: nothing is active until the process has exited and the row is `stopped`
///   without a session again.
/// - A row `stopped` WITH a session is the process that is going away (after `STATE_CHANGED -> stopped` the VMHost answers and closes):
///   a new start waits for `exited`.
/// - `abort` (a library action, not a request) exists only in `starting` and ignores a request in flight: the VMHost may never answer.
/// - `delete` (the library action) exists only in `stopped` without a session; it does not depend on the bundle being valid.
enum MachineActionTable {
    static func allowed(state: MachineRunState, hasSession: Bool, requestInFlight: Bool, bundleUsable: Bool) -> Set<MachineAction> {
        if !hasSession {
            guard state == .stopped else { return [] }
            return bundleUsable ? [.start, .delete] : [.delete]
        }
        if state == .starting { return [.abort] }
        guard !requestInFlight else { return [] }
        switch state {
        case .running: return [.pause, .stop]
        case .paused: return [.resume, .stop]
        case .shuttingDown: return [.stop]
        case .stopped, .starting, .failed: return []
        }
    }
}

// MARK: texts

/// A line for the row and its details. The words come from `ErrorCatalog` through `ErrorPresentation` (S11-09): the headline has no code and
/// no path, the explanation and the action are under it, the code, the request ID, the log and the time are in the details.
/// A button that a line of the row offers (S11-11). The row and the controller know the same three; there is no automatic restart.
enum MachineNoticeAction: Hashable {
    /// "Запустить снова": the same start as the toolbar's, after a crash. The user presses it; nothing restarts a machine by itself.
    case restart
    /// "Показать лог": `logs/vmhost.log` of the bundle in Finder, or the words that no log is kept.
    case showLog
    /// "Завершить процесс": SIGTERM, then SIGKILL, through the client that owns the pid. Offered only when the pid is known.
    case terminateProcess
}

struct MachineNotice: Equatable {
    var text: String
    var details: String?
    /// The line names the cause of the end of the process (a protocol violation, `failed`, a refused start): when the process ends it stays
    /// the headline and the exit status goes to the details. A line about something else (a refused pause) goes to the details instead.
    var explainsExit = false
    /// The explanation and the action of the catalog entry (the second line).
    var hint: String?
    var actions: [MachineNoticeAction] = []

    /// The title alone: a line about the process itself ("Процесс машины аварийно завершился").
    static func plain(_ presentation: ErrorPresentation, explainsExit: Bool = false) -> MachineNotice {
        MachineNotice(text: presentation.title, details: presentation.details, explainsExit: explainsExit, hint: presentation.message)
    }

    /// The process ended by a signal or with a status that is not 0 and nobody asked it to (S11-11). The headline is the title of the
    /// catalog entry (S11-09); the state is `stopped`; the user may start again or read the log. The signal or the status is in the details.
    /// The window says the same as "Процесс машины завершился: <причина>" (`windowText`).
    static func crashed(_ presentation: ErrorPresentation) -> MachineNotice {
        MachineNotice(
            text: presentation.title, details: presentation.details, explainsExit: true, hint: presentation.message, actions: crashActions)
    }

    /// "Процесс машины завершился: <причина>": the reason is the title of the entry, lower case.
    static func windowText(reason: String) -> String { "Процесс машины завершился: " + ErrorPresentation.lowercasedFirst(reason) }

    static let crashActions: [MachineNoticeAction] = [.restart, .showLog]

    /// "Машина ещё занята процессом (pid N)" with the offer to end that process, or, when no pid is known (the lock is held by a process of
    /// another application or of the command line), the same fact without a pid and without the button: this application cannot end a
    /// process that it did not start. The lock itself is never touched (VM_BUNDLE.md 2.1: the kernel drops it when the process dies).
    static func busy(pid: pid_t?, _ presentation: ErrorPresentation) -> MachineNotice {
        var details = presentation.details
        if let pid { details = [details, "Процесс машины: pid \(pid)"].compactMap { $0 }.joined(separator: "\n") }
        return MachineNotice(
            text: pid.map { "Машина ещё занята процессом (pid \($0))" } ?? "Машина ещё занята другим процессом",
            details: details, explainsExit: false, hint: presentation.message, actions: pid == nil ? [] : [.terminateProcess])
    }

    /// "Не удалось запустить: <title of the entry>."
    static func startFailed(_ presentation: ErrorPresentation, explainsExit: Bool = false) -> MachineNotice {
        MachineNotice(
            text: "Не удалось запустить: \(presentation.fragment).", details: presentation.details, explainsExit: explainsExit,
            hint: presentation.message)
    }

    /// "<operation>: <title of the entry>." for a refusal of a request (`ERROR`); the state does not change by it.
    static func refused(_ report: ControlV2ErrorReport, operation: String, requestID: UInt32? = nil, context: ErrorContext) -> MachineNotice {
        var context = context
        context.requestID = requestID ?? context.requestID
        let presentation = ErrorPresentation.control(report, context: context)
        return MachineNotice(text: "\(operation): \(presentation.fragment).", details: presentation.details, hint: presentation.message)
    }

    /// The last fact about a process: what its end says. `nil` for a normal end (status 0) that the user asked for or that came after
    /// `stopped`. Never a "success" for a status that is not 0 and never a message without the status in the details.
    static func exit(_ exit: ControlClientExit, context: ErrorContext) -> MachineNotice? {
        guard let presentation = ErrorPresentation.exit(exit, context: context) else { return nil }
        if exit.startRejection != nil { return .startFailed(presentation) }
        return .plain(presentation)
    }
}

/// What a row says about a machine, as pure functions.
enum MachinePresentation {
    static func stateText(_ state: MachineRunState) -> String {
        switch state {
        case .stopped: return "Остановлена"
        case .starting: return "Запускается…"
        case .running: return "Запущена"
        case .paused: return "Приостановлена"
        case .shuttingDown: return "Завершается…"
        case .failed: return "Сбой"
        }
    }

    static func bundleText(_ status: CatalogBundleStatus) -> String? {
        switch status {
        case .directoryMissing: return "Каталог машины не найден"
        case .notADirectory: return "Не каталог"
        case .config(let state):
            switch state {
            case .valid: return nil
            case .incompatible: return "Несовместимая"
            case .invalid: return "Повреждённая"
            }
        }
    }

    static func profileText(_ profile: MachineProfile) -> String {
        switch profile {
        case .displayTestPattern: return "Тестовый узор (test-only, без гостя)"
        case .displayTwoScreensTest: return "Два экрана, тест (test-only, без гостя)"
        }
    }

    static func memoryText(mib: Int) -> String {
        mib >= 1024 && mib % 1024 == 0 ? "\(mib / 1024) ГиБ" : "\(mib) МиБ"
    }

    /// The name of the machine: `name` of a valid config, else the bundle directory without its suffix.
    static func displayName(entry: CatalogEntry, status: CatalogBundleStatus) -> String {
        if case .config(.valid(let config)) = status { return config.name }
        let last = entry.bundleURL.lastPathComponent
        return last.hasSuffix(CatalogLimits.bundleSuffix) ? String(last.dropLast(CatalogLimits.bundleSuffix.count)) : last
    }
}

// MARK: session

/// The part of `ControlClientV2` that the controller uses. The tests put a fake in its place and feed the events themselves.
protocol MachineSession: AnyObject {
    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion)
    func requestStart(completion: @escaping ControlClientV2.Completion)
    /// `reason` is the field of `PAUSE`/`RESUME` (CONTROL_PROTOCOL_V2.md 6.8): `user`, `system_sleep`, `disk_space` for a pause; `user`,
    /// `system_wake` for a resume. S11-12 sends the system ones.
    func requestPause(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion)
    func requestResume(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion)
    func requestStop(completion: @escaping ControlClientV2.Completion)
    func requestQuery(completion: @escaping ControlClientV2.Completion)
    func terminateSession()
    /// S11-10: the parent end of the frame socket (VMHost descriptor 5) and of the input socket (descriptor 4), once per session.
    func takeDisplaySocket() -> Int32?
    func takeInputSocket() -> Int32?
    /// S11-11: the pid of the VMHost this session spawned, for "Машина ещё занята процессом (pid N)". `nil` when it is not known.
    var processID: pid_t? { get }
}

extension MachineSession {
    var processID: pid_t? { nil }
    /// A session that has no frame or input socket (the tests' fakes).
    func takeDisplaySocket() -> Int32? { nil }
    func takeInputSocket() -> Int32? { nil }
}

/// S11-10: whoever opens the machine window. The controller tells it every change of the row state; the host takes the sockets with
/// `takeDisplayChannels()` when it opens a window. A controller without a host asks for no frame and no input socket (as before S11-10).
protocol MachineWindowHost: AnyObject {
    func machine(_ machine: MachineController, didChange state: MachineRunState)
}

extension ControlClientV2: MachineSession {
    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping Completion) { self.configure(configure, completion: completion) }
    func requestStart(completion: @escaping Completion) { start(completion: completion) }
    func requestPause(reason: ControlV2Reason, completion: @escaping Completion) { pause(reason: reason, completion: completion) }
    func requestResume(reason: ControlV2Reason, completion: @escaping Completion) { resume(reason: reason, completion: completion) }
    func requestStop(completion: @escaping Completion) { stop(completion: completion) }
    func requestQuery(completion: @escaping Completion) { query(completion: completion) }
    func terminateSession() { terminate() }
}

/// Spawns the VMHost for a launch description; the events go to the handler on the main queue.
typealias MachineLauncher = (ControlClientLaunch, @escaping ControlClientV2.EventHandler) throws -> MachineSession

let defaultMachineLauncher: MachineLauncher = { launch, handler in
    try ControlClientV2.launch(launch, deliveryQueue: .main, eventHandler: handler)
}

// MARK: controller

final class MachineController: ObservableObject, Identifiable {
    let entry: CatalogEntry
    var id: String { entry.id }

    @Published private(set) var bundleStatus: CatalogBundleStatus
    @Published private(set) var runState: MachineRunState = .stopped
    @Published private(set) var requestInFlight = false
    @Published private(set) var notice: MachineNotice?
    /// What the VMHost answered to the last explicit "Обновить" (`QUERY`). Information only: it does not change `runState`.
    @Published private(set) var lastQueryText: String?
    /// The last end of a process of this machine (the last fact), `nil` before the first start.
    @Published private(set) var lastExit: ControlClientExit?
    /// S11-11: "Процесс машины завершился: <причина>" while the last end of the process was a crash (a signal, or a status that is not 0,
    /// that nobody asked for). The state is `stopped`; this is the reason. `nil` after a normal end and when a new start begins.
    @Published private(set) var crashText: String?
    /// S11-11: what "Показать лог" says when `logs/vmhost.log` does not exist ("журнал не ведётся"); `nil` otherwise.
    @Published private(set) var logNote: String?

    /// S11-12: the reason of the `STATE_CHANGED` that made the machine `paused` (`user`, `system_sleep`, `disk_space`); `nil` in every other
    /// state. It comes from the event, not from what the application asked: the VMHost says why it is paused.
    @Published private(set) var pauseReason: ControlV2Reason?

    private let vmhost: VMHostLocation
    private let launcher: MachineLauncher
    private let hostMemoryBytes: UInt64
    private var session: MachineSession?
    /// Opens the window of the machine (S11-10). Weak: the application owns the host.
    weak var windowHost: MachineWindowHost?
    /// Events and completions of an earlier session (a process that is already gone) must not touch a newer one.
    private var generation = 0
    /// The user (or the application at quit) asked this process to stop (`STOP`), or to end (`abort`, "Завершить процесс", quit): its end is
    /// not a crash. Reset by a new start.
    private var endRequested = false
    /// What runs when the process has ended (the quit of the application waits for it). Called once each, on the main queue.
    private var endHandlers: [() -> Void] = []
    /// Shows `logs/vmhost.log` (Finder); set by the application. `nil` in the tests and wherever there is nothing to show with.
    var logRevealer: ((URL) -> Void)?
    /// Whether a file exists; the tests replace it.
    var fileExists: (URL) -> Bool = { FileManager.default.fileExists(atPath: $0.path) }

    // S11-12: the conditions of the host (HostConditions.swift).
    /// Free bytes of the volume of the bundle; `nil` when it cannot be told. The tests replace it.
    var availableSpace: (URL) -> UInt64? = CreateMachineModel.volumeAvailableBytes(at:)
    var diskThresholds = DiskSpaceThresholds.hypothesis
    /// The Mac is going to sleep or sleeps (between `prepareForSystemSleep` and `resumeAfterSystemWake`): failures are kept for the wake.
    private var asleep = false
    /// The `PAUSE` of the sleep was sent and is not answered yet.
    private var sleepPauseInFlight = false
    /// The machine became running during sleep while another lifecycle request was still in flight.
    private var sleepPausePending = false
    /// A disk-space `PAUSE` is in flight: a sleep during it must not be reported as a skipped pause.
    private var diskPauseInFlight = false
    /// The Mac woke before the pause of the sleep landed (or while another request was in flight): the resume follows as soon as it can.
    private var wakeResumePending = false
    /// Refusals of the requests of the sleep, shown when the Mac wakes.
    private var sleepNotices: [MachineNotice] = []
    /// The warning about free space was shown for this low period; it is shown once, not every 30 s.
    private var diskWarningShown = false

    init(
        entry: CatalogEntry, status: CatalogBundleStatus, vmhost: VMHostLocation,
        launcher: @escaping MachineLauncher = defaultMachineLauncher,
        hostMemoryBytes: UInt64 = ProcessInfo.processInfo.physicalMemory
    ) {
        self.entry = entry
        self.bundleStatus = status
        self.vmhost = vmhost
        self.launcher = launcher
        self.hostMemoryBytes = hostMemoryBytes
    }

    var hasSession: Bool { session != nil }

    /// The pid of the live VMHost of this machine, when the session knows it (the real client does). For the text "(pid N)" and the tests.
    var processID: pid_t? { session?.processID }

    var config: BundleConfig? { bundleStatus.config }

    var displayName: String { MachinePresentation.displayName(entry: entry, status: bundleStatus) }

    var allowedActions: Set<MachineAction> {
        MachineActionTable.allowed(
            state: runState, hasSession: hasSession, requestInFlight: requestInFlight, bundleUsable: config != nil)
    }

    func allows(_ action: MachineAction) -> Bool { allowedActions.contains(action) }

    /// The parent ends of the frame and input sockets of the current session; each is handed out once, `nil` afterwards (or without a session).
    func takeDisplayChannels() -> (display: Int32?, input: Int32?) {
        guard let session else { return (nil, nil) }
        return (session.takeDisplaySocket(), session.takeInputSocket())
    }

    /// "Обновить" is not an action of the table: it reads (`QUERY` is valid in every state and may be sent while another request is in flight).
    var canRefresh: Bool { session != nil }

    /// A new reading of `config.plist` and the directory. Not while a process runs: the VMHost got its configuration at the start.
    func updateStatus(_ status: CatalogBundleStatus) {
        guard !hasSession else { return }
        if status != bundleStatus { bundleStatus = status }
    }

    func dismissNotice() { notice = nil }

    /// What the details of an error of this machine need besides the error: the bundle (the log is in it) and the request, when known.
    private func errorContext(requestID: UInt32? = nil) -> ErrorContext {
        ErrorContext(requestID: requestID, bundleURL: entry.bundleURL)
    }

    // MARK: actions

    func start() {
        // A process of this machine that has already said `stopped` or `failed` but has not exited: a new VMHost would meet its lock
        // (`busy`, VM_BUNDLE.md 2.1), so none is spawned. The row names the process and offers to end it; the lock is not touched.
        if hasSession, runState == .stopped || runState == .failed, config != nil {
            notice = .busy(pid: session?.processID, ErrorPresentation.rejection(.busy, context: errorContext()))
            return
        }
        guard allows(.start), let config else { return }
        notice = nil
        lastQueryText = nil
        crashText = nil
        logNote = nil
        endRequested = false
        if let refusal = config.launchRefusal(hostMemoryBytes: hostMemoryBytes) {
            // Section 7.3 and VM_BUNDLE 3.5: the controlling process applies the same order locally and does not spawn a VMHost.
            notice = .startFailed(ErrorPresentation.refusal(refusal, context: errorContext()))
            return
        }
        guard vmhost.isExecutable else {
            notice = .startFailed(ErrorPresentation.client(.vmhostMissing, technical: "Путь: \(vmhost.url.path)", context: errorContext()))
            return
        }
        // S11-12: free space of the volume of the bundle. Below the hard threshold nothing is spawned; below the warning one it starts.
        if let space = diskSpace() {
            switch space.level {
            case .critical:
                notice = .startFailed(ErrorPresentation.host(.diskSpaceStartRefused, technical: space.details, context: errorContext()))
                return
            case .warning:
                diskWarningShown = true
                notice = .plain(ErrorPresentation.host(.diskSpaceLow, technical: space.details, context: errorContext()))
            case .normal:
                diskWarningShown = false
            }
        }
        generation += 1
        let current = generation
        // With a window host the VMHost gets `--display-fd 5` and `--input-fd 4` (S11-10); without one, nothing of the kind.
        let withWindow = windowHost != nil
        let launch = ControlClientLaunch(
            vmhostPath: vmhost.url.path, bundlePath: entry.bundleURL.path, displayChannel: withWindow, inputChannel: withWindow)
        do {
            // The row is `starting` before the process exists: a launcher may deliver an event only on the main queue, after this returns.
            runState = .starting
            session = try launcher(launch) { [weak self] event in self?.handle(event, generation: current) }
        } catch {
            runState = .stopped
            session = nil
            notice = .startFailed(ErrorPresentation.client(.spawnFailed, technical: "\(error)", context: errorContext()))
        }
    }

    func pause(reason: ControlV2Reason = .user) {
        guard allows(.pause), let session else { return }
        send(operation: "Пауза") { completion in session.requestPause(reason: reason, completion: completion) }
    }

    func resume(reason: ControlV2Reason = .user) {
        guard allows(.resume), let session else { return }
        send(operation: "Продолжение") { completion in session.requestResume(reason: reason, completion: completion) }
    }

    // MARK: conditions of the host (S11-12)

    /// "Приостановлено системой": the pause that the application sent before the sleep (`reason = system_sleep`). Only this one is resumed
    /// when the Mac wakes: a pause of the user (`user`) and a pause for free space (`disk_space`) are not.
    var isPausedBySystemSleep: Bool { runState == .paused && pauseReason == .systemSleep }

    /// The Mac is going to sleep (`willSleepNotification`). A running machine gets `PAUSE reason=system_sleep`. A refusal (`pause_timeout`
    /// and the others) is not swallowed: it is kept and becomes the line of the row when the Mac wakes. The pause is asynchronous: this
    /// does not wait for the answer, and nothing here says that it lands before the sleep begins (the notification gives no such promise).
    func prepareForSystemSleep() {
        asleep = true
        sleepNotices = []
        wakeResumePending = false
        guard session != nil, runState == .running else { return }
        guard allows(.pause) else {
            if diskPauseInFlight {
                // The disk-space PAUSE can be refused. Preserve the sleep intent and retry it after that request completes.
                sleepPausePending = true
            } else {
                sleepNotices.append(.plain(ErrorPresentation.host(.sleepPauseSkipped, context: errorContext())))
            }
            return
        }
        sleepPausePending = true
        flushSystemSleepPause()
    }

    /// A START or RESUME can finish after `willSleep`. Defer the system PAUSE until the lifecycle request no longer owns the slot.
    private func flushSystemSleepPause() {
        guard asleep, sleepPausePending, runState == .running, let session else { return }
        guard allows(.pause) else { return }
        sleepPausePending = false
        sleepPauseInFlight = true
        send(
            operation: "Пауза перед сном", onRefusal: { [weak self] line in self?.recordSleepNotice(line) },
            completed: { [weak self] in self?.sleepPauseInFlight = false }
        ) { completion in session.requestPause(reason: .systemSleep, completion: completion) }
    }

    /// The Mac woke (`didWakeNotification`). Publishes what went wrong before the sleep, and sends `RESUME reason=system_wake` to a machine
    /// that the application paused for the sleep, and only to it. A machine that is paused by the user stays paused.
    func resumeAfterSystemWake() {
        asleep = false
        sleepPausePending = false
        if !sleepNotices.isEmpty {
            var line = sleepNotices[0]
            let rest = sleepNotices.dropFirst().map { [$0.text, $0.details].compactMap { $0 }.joined(separator: "\n") }
            if !rest.isEmpty { line.details = ([line.details] + rest).compactMap { $0 }.joined(separator: "\n") }
            notice = line
            sleepNotices = []
        }
        guard session != nil else { return }
        if isPausedBySystemSleep || sleepPauseInFlight { wakeResumePending = true }
        flushWakeResume()
    }

    private func recordSleepNotice(_ line: MachineNotice) {
        if asleep { sleepNotices.append(line) } else { notice = line }
    }

    /// Sends the resume of the wake as soon as the machine is paused for the sleep and no request is in flight.
    private func flushWakeResume() {
        guard wakeResumePending, let session else { return }
        if isPausedBySystemSleep {
            guard allows(.resume) else { return } // a request is in flight: its completion comes back here
            wakeResumePending = false
            send(operation: "Продолжение после сна", onRefusal: { [weak self] line in self?.notice = line }) { completion in
                session.requestResume(reason: .systemWake, completion: completion)
            }
        } else if !sleepPauseInFlight && !requestInFlight {
            wakeResumePending = false // the pause never landed: nothing to resume
        }
    }

    /// Free space of the volume of the bundle and what the level is; `nil` when the volume does not tell.
    private func diskSpace() -> (level: DiskSpaceLevel, details: String)? {
        guard let available = availableSpace(entry.bundleURL) else { return nil }
        let level = diskThresholds.level(availableBytes: available)
        return (level, "Свободно на томе: \(CreateMachineModel.format(available)); пороги: предупреждение \(CreateMachineModel.format(diskThresholds.warningBytes)), жёсткий \(CreateMachineModel.format(diskThresholds.hardBytes))")
    }

    /// The check of every 30 s for a running machine (`HostConditions` owns the timer). Below the warning threshold the line is shown once;
    /// below the hard one the machine gets `PAUSE reason=disk_space` and the line of the catalog; it does not continue by itself.
    func checkDiskSpace() {
        guard runState == .running, session != nil, let space = diskSpace() else { return }
        switch space.level {
        case .normal:
            diskWarningShown = false
        case .warning:
            guard !diskWarningShown, !requestInFlight else { return }
            diskWarningShown = true
            notice = .plain(ErrorPresentation.host(.diskSpaceLow, technical: space.details, context: errorContext()))
        case .critical:
            guard allows(.pause) else { return } // the next check tries again
            guard let session else { return }
            diskPauseInFlight = true
            // Clears the line; a refusal of the VMHost replaces the line below.
            send(operation: "Пауза", completed: { [weak self] in self?.diskPauseInFlight = false }) { completion in
                session.requestPause(reason: .diskSpace, completion: completion)
            }
            if requestInFlight {
                notice = .plain(ErrorPresentation.host(.diskSpacePaused, technical: space.details, context: errorContext()))
            }
        }
    }

    func stop() {
        guard allows(.stop), let session else { return }
        endRequested = true
        send(operation: "Остановка", isStop: true) { completion in session.requestStop(completion: completion) }
    }

    /// "Прервать": the process does not answer (no READY, no `stateChanged`). SIGTERM, then SIGKILL (the client); `exited` follows and
    /// returns the row to `stopped`.
    func abort() {
        guard allows(.abort), let session else { return }
        endRequested = true
        session.terminateSession()
    }

    // MARK: crash, busy, quit (S11-11)

    /// "Завершить процесс": the process of this machine has said `stopped` or `failed` and is still alive. SIGTERM, then SIGKILL, by the
    /// client that owns the pid (it does not signal a process that has exited); `exited` follows and the row is `stopped` without a session.
    var canTerminateProcess: Bool { session != nil && (runState == .stopped || runState == .failed) }

    func terminateProcess() {
        guard canTerminateProcess, let session else { return }
        endRequested = true
        session.terminateSession()
    }

    /// "Показать лог": `logs/vmhost.log` of the bundle. A bundle without that file says "журнал не ведётся" and no path is invented.
    var logURL: URL { entry.bundleURL.appendingPathComponent("logs").appendingPathComponent("vmhost.log") }

    func showLog() {
        let url = logURL
        guard fileExists(url) else {
            logNote = "Журнал не ведётся: файла журнала у этой машины нет."
            return
        }
        logNote = nil
        logRevealer?(url)
    }

    /// Quit of the application, step 1: send `STOP` to a machine that can take it. `false` when it cannot now (starting, a request is in
    /// flight, not running): the caller ends the process instead. A `STOP` that is already on its way counts as sent.
    @discardableResult
    func stopForQuit() -> Bool {
        guard session != nil else { return true }
        if endRequested { return true } // a `STOP` is on its way, or the process is already being ended
        guard allows(.stop) else { return false }
        stop()
        return true
    }

    /// Quit of the application, step 2 (after the time limit, or when `STOP` is not possible): SIGTERM, 500 ms, SIGKILL, by the client.
    func forceTerminate() {
        guard let session else { return }
        endRequested = true
        session.terminateSession()
    }

    /// Runs `handler` once, when the process of this machine has ended (at once when there is none).
    func notifyWhenProcessEnded(_ handler: @escaping () -> Void) {
        if session == nil { handler() } else { endHandlers.append(handler) }
    }

    /// One `QUERY`, only when the user asks. Without a session there is nothing to ask (the library re-reads the bundle instead).
    func refresh() {
        guard let session else { return }
        let current = generation
        session.requestQuery { [weak self] result in
            guard let self, self.generation == current else { return }
            switch result {
            case .success(.state(let report)):
                let name = ControlV2State(rawValue: report.state).map { String(describing: $0) } ?? "неизвестное (\(report.state))"
                self.lastQueryText = "VMHost по запросу: \(name)"
            case .success(.ack):
                self.lastQueryText = nil
            case .failure(.refused(let report)):
                self.notice = .refused(report, operation: "Обновление", context: self.errorContext())
            case .failure:
                break // the connection ended: `exited` follows and says why
            }
        }
    }

    /// `onRefusal` (S11-12): a request of the system (the sleep, the wake) does not clear the line of the row and tells its refusal to
    /// the caller instead of the row. `completed` runs after every answer of this request, before the next deferred request is sent.
    private func send(
        operation: String, isStop: Bool = false, onRefusal: ((MachineNotice) -> Void)? = nil, completed: (() -> Void)? = nil,
        _ body: (@escaping ControlClientV2.Completion) -> Void
    ) {
        let current = generation
        requestInFlight = true
        if onRefusal == nil { notice = nil }
        body { [weak self] result in
            guard let self, self.generation == current else { return }
            self.requestInFlight = false
            if case .failure(.refused(let report)) = result {
                if isStop { self.endRequested = false } // the process was not asked to end after all
                // The state does not change by a refusal (7.2): the row stays what the last event made it.
                let line = MachineNotice.refused(report, operation: operation, context: self.errorContext())
                if let onRefusal { onRefusal(line) } else { self.notice = line }
            }
            // `connectionClosed` and the like: the process is going away, `exited` follows with the real reason.
            completed?()
            self.flushSystemSleepPause()
            self.flushWakeResume()
        }
    }

    // MARK: events

    /// The only place where the state of the row changes after the start. Events of a session that is not the current one are dropped.
    func handle(_ event: ControlClientEvent, generation eventGeneration: Int) {
        guard eventGeneration == generation else { return }
        switch event {
        case .ready:
            beginConfigureAndStart()
        case .startRejected(let rejection):
            // The connection ends and `exited` follows; the reason is kept and shown at that point together with the exit status.
            // `busy` here: the pid of the holder is not known to this controller (the process that refused is the new one), so the line has
            // no pid and no button; a process of THIS machine that is still alive is met earlier, in `start()`.
            notice = .startFailed(ErrorPresentation.rejection(rejection, context: errorContext()))
        case .stateChanged(_, let to, let reason, _):
            runState = MachineRunState(to)
            pauseReason = to == .paused ? ControlV2Reason(rawValue: reason) : nil
            if asleep, to == .running, !sleepPauseInFlight { sleepPausePending = true }
            if to != .running { sleepPausePending = false }
            if to != .paused { wakeResumePending = false }
            windowHost?.machine(self, didChange: runState)
            flushSystemSleepPause()
            if to == .failed {
                // The VMHost sent no `ERROR` here: the state came from `STATE_CHANGED`, so the entry is the catalog's and the details say so.
                var line = MachineNotice.plain(
                    ErrorPresentation(entry: ErrorCatalog.entry(for: ControlV2ErrorCode.hostFailure), details: "Состояние: failed (по событию STATE_CHANGED)"),
                    explainsExit: true)
                if let pid = session?.processID {
                    // `failed` is terminal for the process, which closes and exits by itself; if it does not, the user may end it.
                    line.details = [line.details, "Процесс машины: pid \(pid)"].compactMap { $0 }.joined(separator: "\n")
                    line.actions = [.terminateProcess]
                }
                notice = line
            }
        case .error(let report, let requestID):
            // An `ERROR` that answers a request is handled by the completion of that request; this one is not caused by a request.
            if requestID == 0 {
                var line = MachineNotice.plain(ErrorPresentation.control(report, context: errorContext(requestID: requestID)))
                line.explainsExit = true
                notice = line
            }
        case .protocolViolation(let text):
            notice = .plain(ErrorPresentation.client(.protocolViolation, technical: text, context: errorContext()), explainsExit: true)
        case .stats, .progress:
            break
        case .exited(let exit):
            finish(with: exit)
        }
    }

    /// A process that ended by a signal or with a status that is not 0, with nobody having asked it to end and without a refused start.
    private func endedAbnormally(_ exit: ControlClientExit) -> Bool {
        guard !endRequested, exit.startRejection == nil else { return false }
        switch exit.termination {
        case .signaled: return true
        case .exited(let status): return status != 0
        case .unavailable: return false
        }
    }

    private func finish(with exit: ControlClientExit) {
        let abnormal = endedAbnormally(exit)
        // A line that offered to end the process is out of date now: the process is gone. The line that says why the process failed stays,
        // without the button; the line that only said "still busy" goes.
        if var offered = notice, offered.actions.contains(.terminateProcess) {
            offered.actions.removeAll { $0 == .terminateProcess }
            notice = offered.explainsExit ? offered : nil
        }
        session = nil
        requestInFlight = false
        sleepPauseInFlight = false
        sleepPausePending = false
        wakeResumePending = false
        diskWarningShown = false
        pauseReason = nil
        lastExit = exit
        runState = .stopped
        crashText = nil
        updateNotice(after: exit, abnormal: abnormal)
        // The window reads `crashText` when it learns that the machine has stopped, so the reason is set first.
        windowHost?.machine(self, didChange: .stopped)
        let handlers = endHandlers
        endHandlers = []
        handlers.forEach { $0() }
    }

    private func updateNotice(after exit: ControlClientExit, abnormal: Bool) {
        guard var ended = MachineNotice.exit(exit, context: errorContext()) else { return } // a normal end keeps the line that explains an earlier refusal
        if abnormal, let presentation = ErrorPresentation.exit(exit, context: errorContext()) {
            ended = .crashed(presentation)
            crashText = MachineNotice.windowText(reason: ended.text)
        }
        guard let earlier = notice, exit.startRejection == nil else {
            notice = ended
            return
        }
        if earlier.explainsExit {
            // An earlier line (a protocol violation, `failed`, a refused start) is the cause, the status is the last fact: both are kept.
            notice = MachineNotice(
                text: earlier.text, details: [earlier.details, ended.text, ended.details].compactMap { $0 }.joined(separator: "\n"),
                explainsExit: true, hint: earlier.hint, actions: abnormal ? MachineNotice.crashActions : [])
            if abnormal { crashText = MachineNotice.windowText(reason: earlier.text) }
        } else {
            // An unexpected end (the process was not asked to stop): its reason is the headline. An unrelated earlier line (a refused
            // pause) is not the cause and moves to the details.
            notice = MachineNotice(
                text: ended.text,
                details: [ended.details, "Ранее: \(earlier.text)", earlier.details].compactMap { $0 }.joined(separator: "\n"), hint: ended.hint,
                actions: ended.actions)
        }
    }

    // MARK: start sequence

    /// `READY` arrived: `CONFIGURE` from `config.plist`, then `START` (the VMHost is `stopped` until it answers). On a refusal of either the
    /// session is closed with `STOP` (valid in `stopped`: no transition, `ACK`, close) and the reason stays on the row.
    private func beginConfigureAndStart() {
        guard let session, let config else { return }
        let current = generation
        requestInFlight = true
        session.requestConfigure(config.controlConfigure()) { [weak self] result in
            guard let self, self.generation == current else { return }
            switch result {
            case .success:
                session.requestStart { [weak self] result in
                    guard let self, self.generation == current else { return }
                    self.requestInFlight = false
                    if case .failure(let error) = result { self.startSequenceFailed(error, session: session) }
                    self.flushSystemSleepPause()
                }
            case .failure(let error):
                self.requestInFlight = false
                self.startSequenceFailed(error, session: session)
            }
        }
    }

    private func startSequenceFailed(_ error: ControlClientError, session: MachineSession) {
        switch error {
        case .refused(let report):
            notice = .startFailed(ErrorPresentation.control(report, context: errorContext()), explainsExit: true)
            guard !report.fatal else { return } // the VMHost closes the connection itself
            let current = generation
            requestInFlight = true
            session.requestStop { [weak self] result in
                guard let self, self.generation == current else { return }
                self.requestInFlight = false
                if case .failure = result { session.terminateSession() }
            }
        case .connectionClosed:
            break // `exited` follows
        default:
            notice = .startFailed(
                ErrorPresentation.client(ClientIssue(error), technical: "\(error)", context: errorContext()), explainsExit: true)
            session.terminateSession()
        }
    }
}
