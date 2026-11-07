import Darwin
import Foundation

// The pure rules of `odctl` (S11-13, docs/specs/CLI.md): the grammar of a script, the name of a bundle directory and the deletion of the
// files of a bundle. They live here, and not in the `odctl` target, so that the unit tests (`OdctlRulesTests`) can reach them: an
// executable target is not a dependency of a test target in this package. No output and no exit statuses here: the failures are values.
//
// The rule of deletion (`BundleRemoval`) repeats `LibraryModel.removeBundleFiles` of the application (S11-06b) and adds what the CLI
// needs; the application does not use this copy yet (one rule in two places, the move of the application to it is the next slice).

// MARK: script grammar (CLI.md section 6)

public enum ScriptCommand: Equatable {
    case start
    case pause
    case resume
    case stop
    case query
    case wait(state: ControlV2State, timeoutMs: Int)
    case sleep(ms: Int)

    public var name: String {
        switch self {
        case .start: return "start"
        case .pause: return "pause"
        case .resume: return "resume"
        case .stop: return "stop"
        case .query: return "query"
        case .wait: return "wait"
        case .sleep: return "sleep"
        }
    }
}

public struct ScriptLine: Equatable {
    public var number: Int
    /// The line without the comment and with single spaces (for the output).
    public var text: String
    public var command: ScriptCommand

    public init(number: Int, text: String, command: ScriptCommand) {
        self.number = number
        self.text = text
        self.command = command
    }
}

public enum ScriptLimits {
    public static let maxFileBytes = 64 << 10
    public static let maxLineBytes = 256
    public static let maxCommands = 1_000
    public static let defaultWaitMs = 10_000
    public static let maxWaitMs = 600_000
    public static let maxSleepMs = 600_000
}

/// A mistake in the text of a script. `line` is the number of the line (1-based); `nil` for a mistake of the whole script.
public struct ScriptError: Error, Equatable {
    public var line: Int?
    public var message: String

    public var text: String {
        line.map { "Сценарий, строка \($0): \(message)" } ?? "Сценарий: \(message)"
    }
}

/// The names of the states in a script and in the output (CONTROL_PROTOCOL_V2.md section 7.1).
public func stateName(_ state: ControlV2State) -> String {
    switch state {
    case .stopped: return "stopped"
    case .running: return "running"
    case .paused: return "paused"
    case .shuttingDown: return "shutting_down"
    case .failed: return "failed"
    }
}

public func parseStateName(_ text: String) -> ControlV2State? {
    [ControlV2State.stopped, .running, .paused, .shuttingDown, .failed].first { stateName($0) == text }
}

/// `text` is the content of the file. The limit of 256 bytes is for the line as written (a comment counts); the whole file is
/// checked before a VMHost is started, so a mistake in line 12 does not leave a machine running.
public func parseScript(_ text: String) throws -> [ScriptLine] {
    var lines: [ScriptLine] = []
    var stopSeen = false
    // Split by scalars: "\r\n" is ONE character in Swift and a split by the character "\n" would not see it.
    let rawLines = text.unicodeScalars.split(separator: "\n", omittingEmptySubsequences: false).map { String(String.UnicodeScalarView($0)) }
    for (index, rawLine) in rawLines.enumerated() {
        let number = index + 1
        func fail(_ message: String) -> ScriptError { ScriptError(line: number, message: message) }
        let raw = rawLine.replacingOccurrences(of: "\r", with: "")
        if raw.utf8.count > ScriptLimits.maxLineBytes { throw fail("строка длиннее \(ScriptLimits.maxLineBytes) байт") }
        var content = raw
        if let hash = content.firstIndex(of: "#") { content = String(content[..<hash]) }
        let words = content.split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
        guard let name = words.first else { continue }
        if stopSeen { throw fail("после stop команд быть не должно") }
        let arguments = Array(words.dropFirst())
        let command: ScriptCommand
        switch name {
        case "start", "pause", "resume", "stop", "query":
            guard arguments.isEmpty else { throw fail("команда \(name) не принимает аргументов") }
            command = [
                "start": ScriptCommand.start, "pause": .pause, "resume": .resume, "stop": .stop, "query": .query,
            ][name]!
        case "wait":
            guard arguments.count == 1 || arguments.count == 2 else { throw fail("ожидается: wait <состояние> [тайм-аут_мс]") }
            guard let state = parseStateName(arguments[0]) else {
                throw fail("неизвестное состояние «\(arguments[0])» (stopped, running, paused, shutting_down, failed)")
            }
            var timeout = ScriptLimits.defaultWaitMs
            if arguments.count == 2 { timeout = try milliseconds(arguments[1], maximum: ScriptLimits.maxWaitMs, minimum: 1, fail: fail) }
            command = .wait(state: state, timeoutMs: timeout)
        case "sleep":
            guard arguments.count == 1 else { throw fail("ожидается: sleep <мс>") }
            command = .sleep(ms: try milliseconds(arguments[0], maximum: ScriptLimits.maxSleepMs, minimum: 0, fail: fail))
        default:
            throw fail("неизвестная команда «\(name)» (start, wait, pause, resume, sleep, query, stop)")
        }
        if command == .stop { stopSeen = true }
        lines.append(ScriptLine(number: number, text: words.joined(separator: " "), command: command))
        if lines.count > ScriptLimits.maxCommands { throw fail("команд больше \(ScriptLimits.maxCommands)") }
    }
    if lines.isEmpty { throw ScriptError(line: nil, message: "нет ни одной команды") }
    return lines
}

private func milliseconds(_ text: String, maximum: Int, minimum: Int, fail: (String) -> ScriptError) throws -> Int {
    guard text.allSatisfy({ $0.isASCII && $0.isNumber }), let value = Int(text) else {
        throw fail("«\(text)» не целое число миллисекунд")
    }
    guard value >= minimum, value <= maximum else { throw fail("\(value) мс вне диапазона от \(minimum) до \(maximum)") }
    return value
}

// MARK: the name of a bundle directory (VM_BUNDLE section 2, item 4)

public enum BundleNaming {
    /// Without the numeric suffix: `/`, `:` and control characters become `_`, a leading `.` becomes `_`, a name of only `_` and spaces
    /// is `Machine`. The caller does not add a suffix for a taken name: an existing directory is an error (CLI.md section 7).
    public static func directoryName(for name: String) -> String {
        var scalars = String.UnicodeScalarView()
        for scalar in name.unicodeScalars {
            if scalar == "/" || scalar == ":" || scalar.value < 0x20 || scalar.value == 0x7F {
                scalars.append("_")
            } else {
                scalars.append(scalar)
            }
        }
        var text = String(scalars)
        if text.hasPrefix(".") { text = "_" + text.dropFirst() }
        if text.allSatisfy({ $0 == "_" || $0 == " " }) { text = "Machine" }
        return text + CatalogLimits.bundleSuffix
    }
}

// MARK: deletion of the files of a bundle (VM_BUNDLE sections 2, 2.1; ISSUE-055)

public enum BundleRemovalRefusal: Error, Equatable {
    /// The path is not a directory, or it is a link (nothing behind a link is followed).
    case notADirectory
    /// No `.odvm` suffix, or no regular `config.plist`.
    case notABundle
    /// The lock of the machine is held by another process.
    case inUse
    /// Files of disks are not deleted before S9-06 (ISSUE-055): the configuration lists `disks`, or `disks/` is not empty, or the
    /// configuration cannot be read and `disks/` cannot be shown to be empty.
    case unsupportedDurability
    case io(Int32)
}

public enum BundleRemoval {
    /// VM_BUNDLE section 2: the name ends with `.odvm` (and is not only the suffix) AND `config.plist` is a regular file, not a link.
    /// Only the shape: whether the file is valid is the status of the machine.
    public static func hasBundleShape(_ url: URL) -> Bool {
        let name = url.lastPathComponent
        guard name.hasSuffix(CatalogLimits.bundleSuffix), name.count > CatalogLimits.bundleSuffix.count else { return false }
        var info = stat()
        guard lstat(url.appendingPathComponent(BundleConfigLimits.fileName).path, &info) == 0 else { return false }
        return (info.st_mode & S_IFMT) == S_IFREG
    }

    /// Whether the files of the bundle include disk data that must stay. It does not depend on the configuration being readable:
    /// `config` is what was read (`nil` when it cannot be), and `disks/` is looked at in any case.
    public static func holdsDiskFiles(at url: URL, config: BundleConfig?) throws -> Bool {
        if let config, !config.disks.isEmpty { return true }
        let path = url.appendingPathComponent("disks", isDirectory: true).path
        var info = stat()
        guard lstat(path, &info) == 0 else {
            if errno == ENOENT { return false }
            throw BundleRemovalRefusal.io(errno)
        }
        // A link or a file named `disks` is removed as itself: what it points to is not touched by the removal.
        guard (info.st_mode & S_IFMT) == S_IFDIR else { return false }
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: path) else { throw BundleRemovalRefusal.io(EACCES) }
        return !names.isEmpty
    }

    /// Deletes the directory of a bundle. Refuses (nothing is deleted) a link, a non-directory, a directory that is not a bundle, a bundle
    /// with disk data (`holdsDiskFiles`) and a bundle whose lock is held. The check of the lock and the deletion are under ONE lock: it is
    /// taken here and held until the directory is gone, so that another process cannot start the machine in between. A lock that cannot be
    /// taken for another reason than `busy` (a `lock` that is not a regular file: no VMHost can take it either) does not stop the deletion.
    /// A directory that is gone already is a success. `config` is what the caller read from `config.plist`, `nil` when it cannot be read.
    public static func removeFiles(at url: URL, config: BundleConfig?) throws {
        var info = stat()
        guard lstat(url.path, &info) == 0 else {
            if errno == ENOENT { return }
            throw BundleRemovalRefusal.io(errno)
        }
        guard (info.st_mode & S_IFMT) == S_IFDIR else { throw BundleRemovalRefusal.notADirectory }
        guard hasBundleShape(url) else { throw BundleRemovalRefusal.notABundle }
        if try holdsDiskFiles(at: url, config: config) { throw BundleRemovalRefusal.unsupportedDurability }
        var held: BundleWriterLock?
        do {
            held = try BundleWriterLock.acquire(bundle: url)
        } catch BundleWriterLockFailure.busy {
            throw BundleRemovalRefusal.inUse
        } catch {
            held = nil
        }
        defer { held?.release() }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            var after = stat()
            if lstat(url.path, &after) != 0, errno == ENOENT { return } // it is gone anyway
            throw BundleRemovalRefusal.io(posixCode(of: error as NSError) ?? EIO)
        }
    }

    private static func posixCode(of error: NSError) -> Int32? {
        if error.domain == NSPOSIXErrorDomain { return Int32(truncatingIfNeeded: error.code) }
        if let underlying = error.userInfo[NSUnderlyingErrorKey] as? NSError { return posixCode(of: underlying) }
        return nil
    }
}
