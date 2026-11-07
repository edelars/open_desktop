import Darwin
import Foundation
import ODShared

// Output and failures of `odctl` (S11-13, docs/specs/CLI.md sections 3 and 4).
// Words for the user come from `ErrorCatalog` (S11-09, docs/specs/USER_FACING_ERRORS.md): the title, the explanation and the action.
// A raw code is never part of the text of a human failure; it is in the `--json` output (`error.code`, `error.details`).

/// Exit statuses of CLI.md section 3.
enum ExitCode: Int32 {
    case success = 0
    case failure = 1
    case usage = 2
    case busy = 3
    case unsupportedDurability = 4
}

/// The version of every JSON object that `odctl` prints (CLI.md section 4). Changed only for an incompatible change of a field.
let jsonSchemaVersion = 1

enum Output {
    private static let lock = NSLock()

    static func stdout(_ text: String) { write(text, to: FileHandle.standardOutput) }

    static func stderr(_ text: String) { write(text, to: FileHandle.standardError) }

    private static func write(_ text: String, to handle: FileHandle) {
        lock.lock()
        defer { lock.unlock() }
        handle.write(Data((text + "\n").utf8))
    }

    /// One JSON object on one line, keys sorted, with the schema version first-class (`"schema": 1`).
    static func json(_ object: [String: Any]) {
        var object = object
        object["schema"] = jsonSchemaVersion
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]),
              let text = String(data: data, encoding: .utf8) else { return }
        stdout(text)
    }
}

/// A failure that ends the command: the exit status, a code for `--json`, the words, the technical lines.
struct CLIFailure: Error {
    var exit: ExitCode
    /// The wire name of a control code (`busy`, `unsupported_durability`) or the key of the catalog entry; for `--json` only.
    var code: String
    var title: String
    /// The explanation and the action of the catalog entry (empty for a usage failure).
    var message: String
    var details: [String]

    // MARK: builders

    static func entry(_ entry: ErrorEntry, code: String? = nil, exit: ExitCode = .failure, details: [String] = []) -> CLIFailure {
        CLIFailure(exit: exit, code: code ?? entry.key, title: entry.title, message: entry.explanation + " " + entry.action, details: details)
    }

    /// A mistake in the command line or in the script: exit 2. The words are the program's own (the catalog has no entry for them).
    static func usage(_ text: String, details: [String] = []) -> CLIFailure {
        CLIFailure(exit: .usage, code: "usage", title: text, message: "", details: details)
    }

    static func library(_ issue: LibraryIssue, exit: ExitCode = .failure, details: [String] = []) -> CLIFailure {
        entry(ErrorCatalog.entry(for: issue), exit: exit, details: details)
    }

    static func client(_ issue: ClientIssue, details: [String] = []) -> CLIFailure {
        entry(ErrorCatalog.entry(for: issue), code: "client", details: details)
    }

    /// An `ERROR` of the VMHost. `busy` is exit 3, `unsupported_durability` is exit 4.
    static func control(_ report: ControlV2ErrorReport) -> CLIFailure {
        let known = ControlV2ErrorCode(rawValue: report.code)
        var details = ["Состояние процесса при ошибке: \(report.state)"]
        if report.aux0 != 0 || report.aux1 != 0 { details.append("Числа ошибки: \(report.aux0), \(report.aux1)") }
        if !report.detail.isEmpty, let text = String(bytes: report.detail, encoding: .utf8) { details.append("Сообщение процесса: \(text)") }
        return entry(ErrorCatalog.entry(forErrorCode: report.code), code: known?.wireName ?? "unknown_\(report.code)", exit: exitCode(for: known),
                     details: details)
    }

    /// The VMHost answered `ERROR` instead of `READY`.
    static func rejection(_ rejection: StartRejection) -> CLIFailure {
        let code: String
        var details: [String] = []
        switch rejection {
        case .busy: code = ControlV2ErrorCode.busy.wireName
        case .lockChanged: code = ControlV2ErrorCode.lockChanged.wireName
        case .lockInvalid: code = ControlV2ErrorCode.lockInvalid.wireName
        case .ioError(let errno):
            code = ControlV2ErrorCode.ioError.wireName
            details.append("errno: \(errno) (\(String(cString: strerror(Int32(truncatingIfNeeded: errno)))))")
        case .other(let raw):
            code = ControlV2ErrorCode(rawValue: raw)?.wireName ?? "unknown_\(raw)"
        }
        return entry(ErrorCatalog.entry(for: rejection), code: code, exit: rejection == .busy ? .busy : .failure, details: details)
    }

    /// The end of a process; `nil` for a normal one (status 0 after a stop, or no process to wait for).
    static func processEnd(_ end: ControlClientExit) -> CLIFailure? {
        if end.startRejection == nil {
            switch end.termination {
            case .unavailable: return nil
            case .exited: if end.disposition == .stopped { return nil }
            case .signaled: break
            }
        }
        if let rejection = end.startRejection { return Self.rejection(rejection) }
        var details: [String] = []
        switch end.termination {
        case .exited(let status): details.append("Статус процесса: \(status)")
        case .signaled(let signal): details.append("Сигнал: \(signal) (\(String(cString: strsignal(signal))))")
        case .unavailable: break
        }
        return entry(ErrorCatalog.entry(for: end), code: "process_exit", details: details)
    }

    /// A refusal that the controlling process applies before it starts a VMHost (VM_BUNDLE 3.5).
    static func refusal(_ refusal: LaunchRefusal) -> CLIFailure {
        switch refusal {
        case .unsupportedDurability:
            return entry(ErrorCatalog.entry(for: refusal), code: ControlV2ErrorCode.unsupportedDurability.wireName, exit: .unsupportedDurability,
                         details: ["Процесс машины не запускался"])
        case .unsupported(let field):
            return entry(ErrorCatalog.entry(for: refusal), code: ControlV2ErrorCode.unsupported.wireName,
                         details: ["Поле конфигурации: \(field)", "Процесс машины не запускался"])
        case .hostMemoryExceeded(let requested, let limit):
            return entry(ErrorCatalog.entry(for: refusal), code: ControlV2ErrorCode.hostMemoryExceeded.wireName,
                         details: ["Запрошено: \(requested) МиБ, предел: \(limit) МиБ", "Процесс машины не запускался"])
        }
    }

    private static func exitCode(for code: ControlV2ErrorCode?) -> ExitCode {
        switch code {
        case .busy?: return .busy
        case .unsupportedDurability?: return .unsupportedDurability
        default: return .failure
        }
    }

    // MARK: printing

    /// The text of a failure for a person: no code, no number, no path (those are the details of `--json`).
    var text: String {
        message.isEmpty ? title : title + ". " + message
    }

    /// The object of `--json` for a failure (`error` of CLI.md section 4).
    var jsonObject: [String: Any] {
        var error: [String: Any] = ["code": code, "title": title, "message": message, "exit_code": Int(exit.rawValue)]
        if !details.isEmpty { error["details"] = details }
        return error
    }

    func report(json: Bool, command: String) {
        if json {
            Output.json(["command": command, "ok": false, "error": jsonObject])
        } else if exit == .usage {
            Output.stderr("odctl: \(text)")
            for line in details { Output.stderr("  \(line)") }
            Output.stderr("Справка: odctl --help")
        } else {
            Output.stderr("odctl: \(text)")
        }
    }
}

/// Text for the sentence "Предупреждение: ..." about a warning of the catalog (a damaged index, a limit).
func warningText(_ warning: CatalogWarning) -> (code: String, text: String) {
    let entry = ErrorCatalog.entry(for: LibraryIssue(warning))
    return (entry.key, entry.title + ". " + entry.explanation + " " + entry.action)
}
