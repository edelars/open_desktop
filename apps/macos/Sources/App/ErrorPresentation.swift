import Foundation
import ODShared

// How an error reaches the screen (S11-09, docs/specs/USER_FACING_ERRORS.md section 2): the words come from `ErrorCatalog`
// (ODShared), this file adds what only the application knows (the request ID, the bundle, the time) and builds the "Подробности".
// A pure value: no UI framework, tested without windows.
//
// The headline is the title of the entry (no code, no digits, no path). The details hold the code, the request ID, the path of
// `vmhost.log` (a machine error only), the time, and the technical part that the caller passes (errno, a field name). No stack trace.

/// What the application knows about the moment of a failure.
struct ErrorContext {
    /// The request ID of the failed request, when the event or the completion carries it. `nil` is written as "не определён".
    var requestID: UInt32?
    /// The bundle of the machine. `nil` for errors that are not about one machine (the library): no log line then.
    var bundleURL: URL?
    var date = Date()
    /// Whether `logs/vmhost.log` exists; the default asks the file system. Injected by the tests.
    var logExists: (URL) -> Bool = { FileManager.default.fileExists(atPath: $0.path) }
}

struct ErrorPresentation: Equatable {
    var entry: ErrorEntry
    /// `nil` when the entry does not offer "Подробности".
    var details: String?

    var title: String { entry.title }
    var explanation: String { entry.explanation }
    var action: String { entry.action }
    var severity: ErrorSeverity { entry.severity }

    /// The explanation and the action: the second line under the headline.
    var message: String { entry.explanation + " " + entry.action }
    /// A whole sentence for a place that has one line (a banner).
    var fullText: String { title + ". " + message }
    /// The title for a place where it continues a sentence ("Не удалось удалить: нет прав на изменение файлов машины.").
    var fragment: String { ErrorPresentation.lowercasedFirst(title) }

    static func == (lhs: ErrorPresentation, rhs: ErrorPresentation) -> Bool { lhs.entry == rhs.entry && lhs.details == rhs.details }

    static func lowercasedFirst(_ text: String) -> String {
        guard let first = text.first else { return text }
        return first.lowercased() + text.dropFirst()
    }

    // MARK: builders

    /// An `ERROR` of the VMHost. `report.detail` is the text for the log (the protocol keeps it out of the logic); it goes to the details.
    static func control(_ report: ControlV2ErrorReport, context: ErrorContext = ErrorContext()) -> ErrorPresentation {
        let entry = ErrorCatalog.entry(forErrorCode: report.code)
        var extra = ["Состояние процесса при ошибке: \(report.state)"]
        if report.aux0 != 0 || report.aux1 != 0 { extra.append("Числа ошибки: \(report.aux0), \(report.aux1)") }
        if !report.detail.isEmpty, let text = String(bytes: report.detail, encoding: .utf8) { extra.append("Сообщение процесса: \(text)") }
        if report.fatal { extra.append("Ошибка закрыла соединение") }
        return make(entry, code: codeText(report.code), context: context, extra: extra)
    }

    /// The end of a process. `nil` for a normal end (status 0 after a stop, or a process that was not ours to wait for). A refused start
    /// is told by the code of the refusal; the exit status or the signal is the last line of the details.
    static func exit(_ exit: ControlClientExit, context: ErrorContext = ErrorContext()) -> ErrorPresentation? {
        if exit.startRejection == nil {
            switch exit.termination {
            case .unavailable: return nil
            case .exited: if exit.disposition == .stopped { return nil }
            case .signaled: break
            }
        }
        var extra: [String] = []
        var code: String
        switch exit.termination {
        case .exited(let status): code = "статус \(status)"; extra.append("Статус процесса: \(status)")
        case .signaled(let signal):
            code = "сигнал \(signal)"
            extra.append("Сигнал: \(signal) (\(String(cString: strsignal(signal))))")
        case .unavailable: code = "статус неизвестен"
        }
        if let rejection = exit.startRejection {
            let info = rejectionInfo(rejection)
            code = info.code
            extra.insert(contentsOf: info.extra, at: 0)
        }
        return make(ErrorCatalog.entry(for: exit), code: code, context: context, extra: extra)
    }

    /// The VMHost answered `ERROR` instead of `READY` (the exit follows): the code of the refusal.
    static func rejection(_ rejection: StartRejection, context: ErrorContext = ErrorContext()) -> ErrorPresentation {
        let info = rejectionInfo(rejection)
        return make(ErrorCatalog.entry(for: rejection), code: info.code, context: context, extra: info.extra)
    }

    private static func rejectionInfo(_ rejection: StartRejection) -> (code: String, extra: [String]) {
        switch rejection {
        case .busy: return (codeText(ControlV2ErrorCode.busy.rawValue), [])
        case .lockChanged: return (codeText(ControlV2ErrorCode.lockChanged.rawValue), [])
        case .lockInvalid: return (codeText(ControlV2ErrorCode.lockInvalid.rawValue), [])
        case .ioError(let errno):
            return (codeText(ControlV2ErrorCode.ioError.rawValue),
                    ["errno: \(errno) (\(String(cString: strerror(Int32(truncatingIfNeeded: errno)))))"])
        case .other(let raw): return (codeText(raw), [])
        }
    }

    /// A refusal of the application before the VMHost was spawned.
    static func refusal(_ refusal: LaunchRefusal, context: ErrorContext = ErrorContext()) -> ErrorPresentation {
        let entry = ErrorCatalog.entry(for: refusal)
        switch refusal {
        case .unsupportedDurability:
            return make(entry, code: "unsupported_durability (5)", context: context, extra: ["Процесс машины не запускался"])
        case .unsupported(let field):
            return make(entry, code: "unsupported (4)", context: context, extra: ["Поле конфигурации: \(field)", "Процесс машины не запускался"])
        case .hostMemoryExceeded(let requested, let limit):
            return make(entry, code: "host_memory_exceeded (12)", context: context,
                        extra: ["Запрошено: \(requested) МиБ, предел: \(limit) МиБ", "Процесс машины не запускался"])
        }
    }

    /// A failure of the controlling side. `technical` is the description for the details only (an error text, a path).
    static func client(_ issue: ClientIssue, technical: String? = nil, context: ErrorContext = ErrorContext()) -> ErrorPresentation {
        make(ErrorCatalog.entry(for: issue), code: "client", context: context, extra: technical.map { [$0] } ?? [])
    }

    /// A failure or a warning of the library or of a bundle. No request and no log line: it is not about a running machine.
    static func library(_ issue: LibraryIssue, technical: String? = nil, context: ErrorContext = ErrorContext()) -> ErrorPresentation {
        var context = context
        context.bundleURL = nil
        return make(ErrorCatalog.entry(for: issue), code: nil, context: context, extra: technical.map { [$0] } ?? [])
    }

    // MARK: details

    /// `code` (name and number, or a status), the request ID, the log, the time, then the technical lines. Without a code or a request
    /// line, when the failure has none (the library).
    private static func make(_ entry: ErrorEntry, code: String?, context: ErrorContext, extra: [String]) -> ErrorPresentation {
        guard entry.showsDetails else { return ErrorPresentation(entry: entry, details: nil) }
        var lines: [String] = []
        if let code { lines.append("Код: \(code)") }
        if context.bundleURL != nil || context.requestID != nil {
            lines.append("Запрос: " + (context.requestID.map { String($0) } ?? "не определён"))
        }
        if let bundle = context.bundleURL {
            let log = bundle.appendingPathComponent("logs").appendingPathComponent("vmhost.log")
            lines.append(context.logExists(log) ? "Журнал: \(log.path)" : "Журнал: vmhost.log пока не ведётся")
        }
        lines.append("Время: " + timeText(context.date))
        lines.append(contentsOf: extra)
        return ErrorPresentation(entry: entry, details: lines.joined(separator: "\n"))
    }

    /// `busy (7)` for a known code, `неизвестный (99)` for an unknown one.
    static func codeText(_ raw: UInt32) -> String {
        if let code = ControlV2ErrorCode(rawValue: raw) { return "\(code.wireName) (\(raw))" }
        return "неизвестный (\(raw))"
    }

    /// Local time with the offset: the same moment is easy to find in a log.
    static func timeText(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss Z"
        return formatter.string(from: date)
    }
}
