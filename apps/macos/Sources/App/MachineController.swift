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

/// A line for the row and its details. The catalog of user texts for every error code is S11-09; this is the minimum that the library needs
/// without inventing a state.
struct MachineNotice: Equatable {
    var text: String
    var details: String?
    /// The line names the cause of the end of the process (a protocol violation, `failed`, a refused start): when the process ends it stays
    /// the headline and the exit status goes to the details. A line about something else (a refused pause) goes to the details instead.
    var explainsExit = false

    /// The text that the plan asks for when the VMHost answers `busy` (the bundle lock is held).
    static let busyText = "машина уже запущена другим процессом"

    static func startFailed(_ reason: String, details: String? = nil) -> MachineNotice {
        MachineNotice(text: "Не удалось запустить: \(reason).", details: details)
    }

    static func rejection(_ rejection: StartRejection) -> MachineNotice {
        switch rejection {
        case .busy:
            return .startFailed(busyText, details: "busy (CONTROL_PROTOCOL_V2.md 12.1): блокировка bundle занята. Возможна и ложная занятость на мгновение: повторите запуск.")
        case .lockChanged:
            return .startFailed("файл блокировки bundle был заменён", details: "lock_changed")
        case .lockInvalid:
            return .startFailed("файл блокировки bundle не является обычным файлом", details: "lock_invalid")
        case .ioError(let errno):
            return .startFailed("ошибка ввода-вывода при открытии bundle", details: "io_error, errno \(errno)")
        case .other(let code):
            return .startFailed("VMHost отказался запускаться", details: "неизвестный код \(code)")
        }
    }

    static func refusal(_ refusal: LaunchRefusal) -> MachineNotice {
        switch refusal {
        case .unsupportedDurability:
            return .startFailed(
                "у машины есть диски, а сохранность данных до закрытия S9-06 не гарантируется",
                details: "unsupported_durability (VM_BUNDLE.md 3.4 п. 4): VMHost не запускался")
        case .unsupported(let field):
            return .startFailed("конфигурация требует устройства, которого нет у профиля (\(field))", details: "unsupported: \(field)")
        case .hostMemoryExceeded(let requested, let limit):
            return .startFailed(
                "памяти машины (\(requested) МиБ) больше половины памяти компьютера (\(limit) МиБ)",
                details: "host_memory_exceeded")
        }
    }

    /// The words for a refusal of a request (`ERROR`): the minimum of texts, the rest is S11-09.
    static func reason(_ report: ControlV2ErrorReport) -> String {
        switch ControlV2ErrorCode(rawValue: report.code) {
        case .pauseTimeout?: return "машина не дошла до безопасной точки за 5 с и продолжает работать"
        case .invalidState?: return "операция недоступна в этом состоянии"
        case .operationInProgress?: return "предыдущая операция ещё выполняется"
        case .unsupportedDurability?: return "до закрытия S9-06 операция недоступна для машины с дисками"
        case .hostMemoryExceeded?: return "памяти машины больше половины памяти компьютера"
        case .unsupported?: return "конфигурация требует устройства, которого нет у профиля"
        case .invalidConfig?, .invalidName?: return "VMHost отверг конфигурацию"
        case .busy?: return busyText
        default: return "ошибка VMHost (код \(report.code))"
        }
    }

    static func details(_ report: ControlV2ErrorReport) -> String {
        let name = ControlV2ErrorCode(rawValue: report.code).map { String(describing: $0) } ?? "неизвестный код"
        return "\(name), код \(report.code)"
    }

    static func refused(_ report: ControlV2ErrorReport, operation: String) -> MachineNotice {
        MachineNotice(text: "\(operation): \(reason(report)).", details: details(report))
    }

    /// The last fact about a process: what its end says. `nil` for a normal end (status 0) that the user asked for or that came after
    /// `stopped`. Never a "success" for a status that is not 0 and never a message without the status in the details.
    static func exit(_ exit: ControlClientExit) -> MachineNotice? {
        if let rejection = exit.startRejection { return .rejection(rejection) }
        switch exit.termination {
        case .signaled(let signal):
            let name = String(cString: strsignal(signal))
            return MachineNotice(text: "VMHost завершён сигналом \(signal).", details: "\(name); статус процесса: сигнал \(signal)")
        case .unavailable:
            return nil
        case .exited(let code):
            switch exit.disposition {
            case .stopped:
                return nil
            case .usageError:
                return MachineNotice(text: "VMHost не принял аргументы запуска.", details: "код выхода \(code)")
            case .startRejected:
                return MachineNotice(text: "VMHost отказался запускаться.", details: "код выхода \(code)")
            case .protocolError:
                return MachineNotice(text: "VMHost закрыл соединение из-за нарушения протокола.", details: "код выхода \(code)")
            case .transportLost:
                return MachineNotice(text: "VMHost потерял связь с приложением.", details: "код выхода \(code)")
            case .hostFailed:
                return MachineNotice(text: "VMHost сообщил о сбое и завершился.", details: "код выхода \(code)")
            case .killedBySignal, .unavailable, .unexpected:
                return MachineNotice(text: "VMHost завершился с неожиданным кодом \(code).", details: "код выхода \(code)")
            }
        }
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
    func requestPause(completion: @escaping ControlClientV2.Completion)
    func requestResume(completion: @escaping ControlClientV2.Completion)
    func requestStop(completion: @escaping ControlClientV2.Completion)
    func requestQuery(completion: @escaping ControlClientV2.Completion)
    func terminateSession()
}

extension ControlClientV2: MachineSession {
    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping Completion) { self.configure(configure, completion: completion) }
    func requestStart(completion: @escaping Completion) { start(completion: completion) }
    func requestPause(completion: @escaping Completion) { pause(completion: completion) }
    func requestResume(completion: @escaping Completion) { resume(completion: completion) }
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

    private let vmhost: VMHostLocation
    private let launcher: MachineLauncher
    private let hostMemoryBytes: UInt64
    private var session: MachineSession?
    /// Events and completions of an earlier session (a process that is already gone) must not touch a newer one.
    private var generation = 0

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

    var config: BundleConfig? { bundleStatus.config }

    var displayName: String { MachinePresentation.displayName(entry: entry, status: bundleStatus) }

    var allowedActions: Set<MachineAction> {
        MachineActionTable.allowed(
            state: runState, hasSession: hasSession, requestInFlight: requestInFlight, bundleUsable: config != nil)
    }

    func allows(_ action: MachineAction) -> Bool { allowedActions.contains(action) }

    /// "Обновить" is not an action of the table: it reads (`QUERY` is valid in every state and may be sent while another request is in flight).
    var canRefresh: Bool { session != nil }

    /// A new reading of `config.plist` and the directory. Not while a process runs: the VMHost got its configuration at the start.
    func updateStatus(_ status: CatalogBundleStatus) {
        guard !hasSession else { return }
        if status != bundleStatus { bundleStatus = status }
    }

    func dismissNotice() { notice = nil }

    // MARK: actions

    func start() {
        guard allows(.start), let config else { return }
        notice = nil
        lastQueryText = nil
        if let refusal = config.launchRefusal(hostMemoryBytes: hostMemoryBytes) {
            // Section 7.3 and VM_BUNDLE 3.5: the controlling process applies the same order locally and does not spawn a VMHost.
            notice = .refusal(refusal)
            return
        }
        guard vmhost.isExecutable else {
            notice = .startFailed("VMHost не найден", details: vmhost.url.path)
            return
        }
        generation += 1
        let current = generation
        let launch = ControlClientLaunch(vmhostPath: vmhost.url.path, bundlePath: entry.bundleURL.path, displayChannel: false)
        do {
            // The row is `starting` before the process exists: a launcher may deliver an event only on the main queue, after this returns.
            runState = .starting
            session = try launcher(launch) { [weak self] event in self?.handle(event, generation: current) }
        } catch {
            runState = .stopped
            session = nil
            notice = .startFailed("не удалось запустить процесс VMHost", details: "\(error)")
        }
    }

    func pause() {
        guard allows(.pause), let session else { return }
        send(operation: "Пауза") { completion in session.requestPause(completion: completion) }
    }

    func resume() {
        guard allows(.resume), let session else { return }
        send(operation: "Продолжение") { completion in session.requestResume(completion: completion) }
    }

    func stop() {
        guard allows(.stop), let session else { return }
        send(operation: "Остановка") { completion in session.requestStop(completion: completion) }
    }

    /// "Прервать": the process does not answer (no READY, no `stateChanged`). SIGTERM, then SIGKILL (the client); `exited` follows and
    /// returns the row to `stopped`.
    func abort() {
        guard allows(.abort), let session else { return }
        session.terminateSession()
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
                self.notice = .refused(report, operation: "Обновление")
            case .failure:
                break // the connection ended: `exited` follows and says why
            }
        }
    }

    private func send(operation: String, _ body: (@escaping ControlClientV2.Completion) -> Void) {
        let current = generation
        requestInFlight = true
        notice = nil
        body { [weak self] result in
            guard let self, self.generation == current else { return }
            self.requestInFlight = false
            if case .failure(.refused(let report)) = result {
                // The state does not change by a refusal (7.2): the row stays what the last event made it.
                self.notice = .refused(report, operation: operation)
            }
            // `connectionClosed` and the like: the process is going away, `exited` follows with the real reason.
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
            notice = .rejection(rejection)
        case .stateChanged(_, let to, _, _):
            runState = MachineRunState(to)
            if to == .failed {
                notice = MachineNotice(text: "VMHost не может установить состояние ресурсов машины и завершается.", details: "failed (CONTROL_PROTOCOL_V2.md 7.1)", explainsExit: true)
            }
        case .error(let report, let requestID):
            // An `ERROR` that answers a request is handled by the completion of that request; this one is not caused by a request.
            if requestID == 0 {
                var line = MachineNotice.refused(report, operation: "VMHost")
                line.explainsExit = true
                notice = line
            }
        case .protocolViolation(let text):
            notice = MachineNotice(text: "VMHost нарушил протокол, соединение закрыто.", details: text, explainsExit: true)
        case .stats, .progress:
            break
        case .exited(let exit):
            finish(with: exit)
        }
    }

    private func finish(with exit: ControlClientExit) {
        session = nil
        requestInFlight = false
        lastExit = exit
        runState = .stopped
        guard let ended = MachineNotice.exit(exit) else { return } // a normal end keeps the line that explains an earlier refusal
        guard let earlier = notice, exit.startRejection == nil else {
            notice = ended
            return
        }
        if earlier.explainsExit {
            // An earlier line (a protocol violation, `failed`, a refused start) is the cause, the status is the last fact: both are kept.
            notice = MachineNotice(
                text: earlier.text, details: [earlier.details, ended.text, ended.details].compactMap { $0 }.joined(separator: "; "),
                explainsExit: true)
        } else {
            // An unexpected end (the process was not asked to stop): its reason is the headline. An unrelated earlier line (a refused
            // pause) is not the cause and moves to the details.
            notice = MachineNotice(
                text: ended.text,
                details: [ended.details, "Ранее: \(earlier.text)", earlier.details].compactMap { $0 }.joined(separator: "; "))
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
            var line = MachineNotice.startFailed(MachineNotice.reason(report), details: MachineNotice.details(report))
            line.explainsExit = true
            notice = line
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
            var line = MachineNotice.startFailed("ошибка клиента control v2", details: "\(error)")
            line.explainsExit = true
            notice = line
            session.terminateSession()
        }
    }
}
