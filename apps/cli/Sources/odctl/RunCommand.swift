import Darwin
import Foundation
import ODShared

// `odctl run <name|path> --script FILE [--json-events]` (S11-13, ADR-0006 (в)): starts the VMHost, drives it by control v2 through the lines of
// the script and ends with it. No endpoint, no background process: when the command returns the VMHost has exited (`STOP`, or the
// teardown policy of the client). A machine that the application or another `odctl` runs has the bundle lock held: `busy`, exit 3.
// A machine with `disks` cannot be started or stopped by a request before S9-06 (`unsupported_durability`, CONTROL_PROTOCOL_V2.md 7.2/7.3,
// ISSUE-055): a script with `start` or `stop` for it is refused with exit 4 before a process is started, and any other script ends by
// closing the connection (EOF in `stopped` is a normal end, status 0).

/// `od-vmhost`: `--vmhost PATH`, then `$ODCTL_VMHOST`, then the file next to the executable of `odctl` (the build directory).
func locateVMHost(option: String?) throws -> String {
    let path: String
    if let option {
        path = option
    } else if let environment = ProcessInfo.processInfo.environment["ODCTL_VMHOST"], !environment.isEmpty {
        path = environment
    } else {
        let executable = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0])
        path = executable.deletingLastPathComponent().appendingPathComponent("od-vmhost").path
    }
    guard FileManager.default.isExecutableFile(atPath: path) else {
        throw CLIFailure.client(.vmhostMissing, details: ["Путь: \(path)"])
    }
    return path
}

func reasonName(_ raw: UInt32) -> String {
    switch ControlV2Reason(rawValue: raw) {
    case .user?: return "user"
    case .systemSleep?: return "system_sleep"
    case .systemWake?: return "system_wake"
    case .guestShutdown?: return "guest_shutdown"
    case .hostFailure?: return "host_failure"
    case .diskSpace?: return "disk_space"
    case nil: return "unknown_\(raw)"
    }
}

/// What `run` prints while it goes. With `--json-events` one JSON object per line (`event`), otherwise short lines of text.
struct RunReporter {
    var json: Bool

    func event(_ name: String, _ fields: [String: Any] = [:], text: String? = nil) {
        if json {
            var object = fields
            object["event"] = name
            object["command"] = "run"
            Output.json(object)
        } else if let text {
            Output.stdout(text)
        }
    }

    /// An event of the control connection (called on the delivery queue).
    func control(_ event: ControlClientEvent) {
        switch event {
        case .ready:
            self.event("ready", text: "Процесс машины готов")
        case .stateChanged(let from, let to, let reason, let requestID):
            self.event("state_changed", ["from": stateName(from), "to": stateName(to), "reason": reasonName(reason), "request_id": requestID],
                       text: "Состояние: \(stateName(from)) -> \(stateName(to))")
        case .error(let report, _):
            let failure = CLIFailure.control(report)
            self.event("error", ["error": failure.jsonObject, "fatal": report.fatal])
        case .exited(let end):
            var fields: [String: Any] = [:]
            switch end.termination {
            case .exited(let status): fields["status"] = Int(status)
            case .signaled(let signal): fields["signal"] = Int(signal)
            case .unavailable: break
            }
            self.event("exited", fields, text: "Процесс машины завершился")
        case .startRejected, .stats, .progress, .protocolViolation:
            break
        }
    }
}

/// The failure for an answer that was not `ok`.
func failure(of error: ControlClientError, session: HostSession) -> CLIFailure {
    switch error {
    case .refused(let report):
        return CLIFailure.control(report)
    case .connectionClosed:
        if let text = session.protocolViolationText {
            return CLIFailure.client(.protocolViolation, details: [text])
        }
        if let end = session.waitExit(timeout: 3), let failure = CLIFailure.processEnd(end) { return failure }
        return CLIFailure.client(.connectionClosed)
    case .protocolViolation(let text):
        return CLIFailure.client(.protocolViolation, details: [text])
    default:
        return CLIFailure.client(ClientIssue(error), details: ["\(error)"])
    }
}

/// The failure that ends a wait because the process ended.
func failure(ofEnd end: ControlClientExit) -> CLIFailure {
    CLIFailure.processEnd(end) ?? CLIFailure.client(.connectionClosed, details: ["Процесс машины завершился, пока сценарий не закончен"])
}

func runRun(_ parsed: ParsedArguments) throws -> Int32 {
    let jsonEvents = parsed.flag("--json-events")
    let reporter = RunReporter(json: jsonEvents)
    let script = try readScript(path: try require(parsed, "--script"))
    let resolved = try resolveBundle(parsed.positional[0], library: parsed.value("--library"))
    let status = CatalogBundleStatus.config(resolved.state)
    guard let config = resolved.state.config else {
        throw CLIFailure.library(LibraryIssue(status) ?? .bundleInvalid, details: ["Путь: \(resolved.url.path)"])
    }
    // The checks of VM_BUNDLE 3.5 that the controlling process applies before it starts a VMHost. Only a script that starts the machine
    // is refused with `launchRefusal` (`unsupported_durability` for a machine with a disk is told without a process, CONTROL_PROTOCOL_V2.md
    // 7.3). `stop` of such a machine is refused by the VMHost in every state (7.2): the script is refused the same way before the launch.
    let hasDisks = !config.disks.isEmpty
    if script.contains(where: { $0.command == .start }), let refusal = config.launchRefusal() {
        throw CLIFailure.refusal(refusal)
    }
    if hasDisks, script.contains(where: { $0.command == .stop }) {
        throw CLIFailure.refusal(.unsupportedDurability)
    }
    let vmhost = try locateVMHost(option: parsed.value("--vmhost"))

    let session: HostSession
    do {
        session = try HostSession(vmhost: vmhost, bundle: resolved.url, onEvent: reporter.control)
    } catch {
        throw CLIFailure.client(.spawnFailed, details: ["\(error)"])
    }
    // SIGINT and SIGTERM end the process of the machine by the policy of the client and then `odctl` itself: nothing stays behind. The last
    // line is `result` (CLI.md 4.3), exit 1.
    signal(SIGINT, SIG_IGN)
    signal(SIGTERM, SIG_IGN)
    let signals = [SIGINT, SIGTERM].map { number -> DispatchSourceSignal in
        let source = DispatchSource.makeSignalSource(signal: number, queue: .global())
        source.setEventHandler {
            session.terminate()
            let interrupted = CLIFailure(
                exit: .failure, code: "interrupted", title: "Команда прервана сигналом",
                message: "Процесс машины остановлен.", details: ["Сигнал: \(number) (\(String(cString: strsignal(number))))"])
            if jsonEvents {
                Output.json(["event": "result", "command": "run", "ok": false, "exit_code": Int(interrupted.exit.rawValue), "error": interrupted.jsonObject])
            } else {
                interrupted.report(json: false, command: "run")
            }
            Darwin.exit(interrupted.exit.rawValue)
        }
        source.resume()
        return source
    }
    defer { signals.forEach { $0.cancel() } }

    do {
        try drive(script: script, config: config, session: session, reporter: reporter)
    } catch let failure as CLIFailure {
        // A machine with disks cannot be stopped by `STOP`: the connection is closed (the VMHost exits in `stopped` with status 0).
        if hasDisks { session.finishByClosing() } else { session.finish() }
        throw failure
    }
    return ExitCode.success.rawValue
}

private func drive(script: [ScriptLine], config: BundleConfig, session: HostSession, reporter: RunReporter) throws {
    switch session.waitReady(timeout: 30) {
    case .ready?: break
    case .ended(let end)?: throw failure(ofEnd: end)
    case nil: throw CLIFailure.client(.notReady)
    }
    // `CONFIGURE` from `config.plist`, as the application does it; the machine is `stopped` and configured afterwards.
    switch session.send(.configure(config.controlConfigure())) {
    case .success: break
    case .failure(let error): throw failure(of: error, session: session)
    }
    var stopped = false
    for line in script {
        let fields: [String: Any] = ["line": line.number, "script": line.text]
        switch line.command {
        case .start:
            try request(.start(operationID: 0), line, session, reporter)
        case .pause:
            try request(.pause(reason: ControlV2Reason.user.rawValue), line, session, reporter)
        case .resume:
            try request(.resume(reason: ControlV2Reason.user.rawValue), line, session, reporter)
        case .query:
            try request(.query, line, session, reporter)
        case .stop:
            try request(.stop(operationID: 0), line, session, reporter)
            stopped = true
        case .sleep(let ms):
            Thread.sleep(forTimeInterval: Double(ms) / 1000)
            reporter.event("command", fields.merging(["ok": true], uniquingKeysWith: { $1 }), text: "[\(line.number)] \(line.text): ok")
        case .wait(let target, let timeoutMs):
            switch session.waitState(target, timeout: Double(timeoutMs) / 1000) {
            case .reached:
                reporter.event("command", fields.merging(["ok": true, "state": stateName(target)], uniquingKeysWith: { $1 }),
                               text: "[\(line.number)] \(line.text): ok")
            case .ended(let end):
                throw failure(ofEnd: end)
            case .timedOut:
                let current = session.state.map(stateName) ?? "неизвестно"
                throw CLIFailure(
                    exit: .failure, code: "wait_timeout", title: "Состояние «\(stateName(target))» не наступило за \(timeoutMs) мс",
                    message: "Сценарий остановлен, процесс машины будет остановлен. Сейчас: \(current).",
                    details: ["Строка сценария: \(line.number)"])
            }
        }
    }
    // A script that does not end with `stop` is ended by `odctl`: `STOP` ends it together with the process of the machine. A machine with
    // `disks` is ended by closing the connection instead (`STOP` would be `unsupported_durability`, CONTROL_PROTOCOL_V2.md 7.2).
    let end: ControlClientExit?
    if !config.disks.isEmpty {
        end = session.finishByClosing()
    } else {
        if !stopped {
            switch session.send(.stop(operationID: 0)) {
            case .success: break
            case .failure(let error): throw failure(of: error, session: session)
            }
        }
        end = session.finish(sendStop: false)
    }
    guard let end else { throw CLIFailure.client(.connectionClosed) }
    if let failure = CLIFailure.processEnd(end) { throw failure }
}

private func request(_ request: ControlV2Request, _ line: ScriptLine, _ session: HostSession, _ reporter: RunReporter) throws {
    switch session.send(request) {
    case .success(let reply):
        var fields: [String: Any] = ["line": line.number, "script": line.text, "ok": true]
        var state: ControlV2State?
        switch reply {
        case .ack(let ack): state = ControlV2State(rawValue: ack.state)
        case .state(let report):
            state = ControlV2State(rawValue: report.state)
            fields["configured"] = report.flags & ControlV2StateReport.flagConfigured != 0
            fields["disks_non_empty"] = report.flags & ControlV2StateReport.flagDisksNonEmpty != 0
            fields["last_reason"] = reasonName(report.lastReason)
        }
        if let state { fields["state"] = stateName(state) }
        reporter.event("command", fields, text: "[\(line.number)] \(line.text): ok" + (state.map { " (\(stateName($0)))" } ?? ""))
    case .failure(let error):
        throw failure(of: error, session: session)
    }
}
