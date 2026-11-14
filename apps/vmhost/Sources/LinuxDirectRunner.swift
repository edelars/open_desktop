import Darwin
import Foundation
#if canImport(ODShared)
import ODShared
#endif

// S4-41b: VMHost runs the Linux direct-boot machine as a CHILD PROCESS `od-runner linux-direct --run ...` and hands its result back as
// a status. The MMU and the CPU stay in Rust; this file embeds nothing of them, adds no function to the C ABI of `od-host` and does
// not change the wire of control protocol v2 (its CONFIGURE carries no kernel paths and still accepts only the display profiles
// 1 and 4, so `LinuxDirectProfileRunner` is reachable through the `--linux-direct <config.json>` mode of VMHost, not over control v2).
//
// One session is one run of the child: the runner has no start/stop methods (docs/specs/CPU_CONTRACT.md, S4-41a). The session is
// synchronous: `start` returns when the child has exited and its JSON report is read. `stop` ends a running child (SIGTERM, then
// SIGKILL after a grace period) from another thread and is idempotent. `pause` and `resume` answer `ProfileUnsupportedError`: the
// Linux board has no pause, and a silent no-op would be a lie about the machine.
//
// Resources of a session: a private temp directory (the `--out` report and the stderr log) and the child process. Standard streams of
// the child are `/dev/null` and a file, so no pipe exists; every other descriptor of VMHost is closed in the child
// (`POSIX_SPAWN_CLOEXEC_DEFAULT`). The child is always reaped with `waitpid` before the session returns, and the temp directory is
// always removed. There are no termination handlers.

/// Profile name of the Linux direct-boot machine in the configuration file.
enum LinuxDirectProfile {
    static let name = "linux-direct"
    /// Number that would stand in `CONFIGURE.machine_profile` if the wire accepted this profile; `start(profile:screens:)` takes it.
    static let number: UInt32 = 5
}

enum LinuxDirectError: Error, CustomStringConvertible, Equatable {
    /// The configuration is refused before any process is spawned.
    case invalidConfig(field: String, detail: String)
    /// `start` called while a session is running.
    case sessionRunning
    /// `posix_spawn` failed (errno).
    case spawnFailed(Int32)
    /// The runner refused its input: exit 3 (arguments or unreadable file) or 4 (hash or machine refusal); fields of its `error` object.
    case runnerRefused(exitCode: Int32, code: String?, stage: String?, detail: String?)
    /// The runner ended in a way that has no report: a signal, another exit code, or a report that is not the expected document.
    case runnerFailed(String)
    /// `stop` ended the child before it produced a report.
    case terminatedByStop

    var description: String {
        switch self {
        case .invalidConfig(let field, let detail): return "invalid config: \(field): \(detail)"
        case .sessionRunning: return "a linux-direct session is already running"
        case .spawnFailed(let code): return "cannot spawn od-runner, errno \(code)"
        case .runnerRefused(let exitCode, let code, let stage, let detail):
            return "od-runner refused (exit \(exitCode)): \(code ?? "?") at \(stage ?? "?"): \(detail ?? "")"
        case .runnerFailed(let detail): return "od-runner failed: \(detail)"
        case .terminatedByStop: return "the session was stopped before the runner reported"
        }
    }
}

/// The validated arguments of one session. `validate()` runs before anything is spawned and mirrors the argument checks of
/// `od-runner linux-direct` (tools/runner/src/linux_direct.rs `parse`), so a refusal here is the one the runner would give with exit 3.
struct LinuxDirectConfig: Equatable {
    var profile: String = LinuxDirectProfile.name
    /// Path of the `od-runner` binary; nil means `od-runner` next to the VMHost executable.
    var runner: String?
    var vmlinuz: String
    var initramfs: String
    var acpiBlob: String
    var nonce: String
    var expectedVmlinuzSHA256: String
    var expectedInitramfsSHA256: String
    var expectedAcpiManifestHash: String
    var ramMiB: UInt64
    var budget: UInt64
    var setCPL0 = false
    /// `diagnostic` or `stage4a`; nil leaves the runner default.
    var cpuidProfile: String?
    var userMode = false

    static let maxNonceCharacters = 64
    static let maxRamMiB: UInt64 = 8192

    static func siblingRunnerPath() -> String? {
        guard let executable = Bundle.main.executableURL else { return nil }
        return executable.deletingLastPathComponent().appendingPathComponent("od-runner").path
    }

    var resolvedRunner: String? { runner ?? Self.siblingRunnerPath() }

    private static func isRegularFile(_ path: String, access mode: Int32) -> Bool {
        var info = stat()
        return stat(path, &info) == 0 && (info.st_mode & S_IFMT) == S_IFREG && Darwin.access(path, mode) == 0
    }

    private static func isHex256(_ text: String) -> Bool {
        text.utf8.count == 64 && text.utf8.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) || ($0 >= 0x41 && $0 <= 0x46) }
    }

    /// The first violation, nil when the configuration may be run.
    func validate() -> LinuxDirectError? {
        func bad(_ field: String, _ detail: String) -> LinuxDirectError { .invalidConfig(field: field, detail: detail) }
        guard profile == LinuxDirectProfile.name else { return bad("profile", "must be \"\(LinuxDirectProfile.name)\", got \"\(profile)\"") }
        guard !nonce.isEmpty, nonce.count <= Self.maxNonceCharacters,
              nonce.utf8.allSatisfy({ ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x41 && $0 <= 0x5a) || ($0 >= 0x61 && $0 <= 0x7a)
                                      || $0 == 0x2d || $0 == 0x5f || $0 == 0x2e }) else {
            return bad("nonce", "1...\(Self.maxNonceCharacters) characters from [A-Za-z0-9._-]")
        }
        guard ramMiB > 0, ramMiB <= Self.maxRamMiB else { return bad("ram_mib", "1...\(Self.maxRamMiB)") }
        for (field, value) in [("expected_vmlinuz_sha256", expectedVmlinuzSHA256), ("expected_initramfs_sha256", expectedInitramfsSHA256),
                               ("expected_acpi_manifest_hash", expectedAcpiManifestHash)] where !Self.isHex256(value) {
            return bad(field, "not a 64-digit hex string")
        }
        if let cpuidProfile {
            guard cpuidProfile == "diagnostic" || cpuidProfile == "stage4a" else { return bad("cpuid_profile", "diagnostic or stage4a") }
            guard setCPL0 else { return bad("cpuid_profile", "needs set_cpl0") }
        }
        if userMode {
            guard setCPL0, cpuidProfile == "stage4a" else { return bad("user_mode", "needs set_cpl0 and cpuid_profile stage4a") }
        }
        for (field, path) in [("vmlinuz", vmlinuz), ("initramfs", initramfs), ("acpi_blob", acpiBlob)]
        where !Self.isRegularFile(path, access: R_OK) {
            return bad(field, "not a readable regular file: \(path)")
        }
        guard let runner = resolvedRunner, Self.isRegularFile(runner, access: X_OK) else {
            return bad("runner", "od-runner is not an executable regular file: \(resolvedRunner ?? "no path")")
        }
        return nil
    }

    /// Arguments after the program name, for a validated configuration. The guest always runs (`--run --budget N`).
    func arguments(out: String) -> [String] {
        var arguments = ["linux-direct", "--vmlinuz", vmlinuz, "--initramfs", initramfs, "--acpi-blob", acpiBlob, "--nonce", nonce,
                         "--expected-vmlinuz-sha256", expectedVmlinuzSHA256, "--expected-initramfs-sha256", expectedInitramfsSHA256,
                         "--expected-acpi-manifest-hash", expectedAcpiManifestHash, "--ram-mib", String(ramMiB), "--out", out,
                         "--run", "--budget", String(budget)]
        if setCPL0 { arguments.append("--set-cpl0") }
        if let cpuidProfile { arguments += ["--cpuid-profile", cpuidProfile] }
        if userMode { arguments.append("--user-mode") }
        return arguments
    }

    /// Reads the JSON file of the `--linux-direct` mode (snake_case keys of the runner flags; `runner`, `set_cpl0`, `cpuid_profile` and
    /// `user_mode` are optional). A missing or mistyped key is `invalidConfig`.
    static func parse(json data: Data) throws -> LinuxDirectConfig {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw LinuxDirectError.invalidConfig(field: "config", detail: "not a JSON object")
        }
        func string(_ key: String, optional: Bool = false) throws -> String? {
            guard let value = object[key] else {
                if optional { return nil }
                throw LinuxDirectError.invalidConfig(field: key, detail: "missing")
            }
            guard let text = value as? String else { throw LinuxDirectError.invalidConfig(field: key, detail: "not a string") }
            return text
        }
        func unsigned(_ key: String) throws -> UInt64 {
            guard let value = object[key] else { throw LinuxDirectError.invalidConfig(field: key, detail: "missing") }
            guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
                  number.int64Value >= 0, number.doubleValue == Double(number.int64Value) else {
                throw LinuxDirectError.invalidConfig(field: key, detail: "not a non-negative integer")
            }
            return UInt64(number.int64Value)
        }
        func flag(_ key: String) throws -> Bool {
            guard let value = object[key] else { return false }
            guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else {
                throw LinuxDirectError.invalidConfig(field: key, detail: "not a boolean")
            }
            return number.boolValue
        }
        return LinuxDirectConfig(
            profile: try string("profile")!, runner: try string("runner", optional: true),
            vmlinuz: try string("vmlinuz")!, initramfs: try string("initramfs")!, acpiBlob: try string("acpi_blob")!,
            nonce: try string("nonce")!, expectedVmlinuzSHA256: try string("expected_vmlinuz_sha256")!,
            expectedInitramfsSHA256: try string("expected_initramfs_sha256")!,
            expectedAcpiManifestHash: try string("expected_acpi_manifest_hash")!,
            ramMiB: try unsigned("ram_mib"), budget: try unsigned("budget"), setCPL0: try flag("set_cpl0"),
            cpuidProfile: try string("cpuid_profile", optional: true), userMode: try flag("user_mode"))
    }
}

/// The stop fields of the runner's report that VMHost hands out as the status of the machine.
struct LinuxDirectStatus: Equatable {
    struct SyscallEntry: Equatable {
        var nr: UInt64
        var rip: String
        var raxReturn: String?
    }

    struct SyscallJournal: Equatable {
        var entered: UInt64
        var returned: UInt64
        var entries: [SyscallEntry]
    }

    var exitKind: String
    var retired: UInt64
    /// Only under `user_mode` (the runner writes the key only then).
    var cpl3Retired: UInt64?
    var com1Text: String
    var syscallJournal: SyscallJournal?
    var guestStarted: Bool
    /// The whole report, re-serialized with sorted keys: two sessions of the same input compare equal byte for byte.
    var report: Data

    /// Reads the report of a run that ended with exit 0. A guest fault is a normal report (`exit_kind` `fault`).
    static func parse(report data: Data) throws -> LinuxDirectStatus {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw LinuxDirectError.runnerFailed("the report is not a JSON object")
        }
        guard object["command"] as? String == "linux-direct", object["status"] as? String == "loaded" else {
            throw LinuxDirectError.runnerFailed("the report is not a loaded linux-direct report")
        }
        guard let stop = object["stop"] as? [String: Any], let exitKind = stop["exit_kind"] as? String,
              let retired = (stop["retired"] as? NSNumber)?.uint64Value, let com1 = stop["com1_text"] as? String,
              let started = object["guest_started"] as? Bool else {
            throw LinuxDirectError.runnerFailed("the report has no stop record of a started guest")
        }
        var journal: SyscallJournal?
        if let raw = stop["syscall_journal"] as? [String: Any] {
            guard let entered = (raw["entered"] as? NSNumber)?.uint64Value, let returned = (raw["returned"] as? NSNumber)?.uint64Value,
                  let rawEntries = raw["entries"] as? [[String: Any]] else {
                throw LinuxDirectError.runnerFailed("syscall_journal is malformed")
            }
            let entries = try rawEntries.map { entry -> SyscallEntry in
                guard let nr = (entry["nr"] as? NSNumber)?.uint64Value, let rip = entry["rip"] as? String else {
                    throw LinuxDirectError.runnerFailed("a syscall_journal entry is malformed")
                }
                return SyscallEntry(nr: nr, rip: rip, raxReturn: entry["rax_return"] as? String)
            }
            journal = SyscallJournal(entered: entered, returned: returned, entries: entries)
        }
        let normalized = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        return LinuxDirectStatus(exitKind: exitKind, retired: retired, cpl3Retired: (stop["cpl3_retired"] as? NSNumber)?.uint64Value,
                                 com1Text: com1, syscallJournal: journal, guestStarted: started, report: normalized)
    }

    /// The status as the JSON that the `--linux-direct` mode prints.
    var jsonObject: [String: Any] {
        var result: [String: Any] = ["status": "ok", "exit_kind": exitKind, "retired": retired, "com1_text": com1Text,
                                     "guest_started": guestStarted]
        if let cpl3Retired { result["cpl3_retired"] = cpl3Retired }
        if let syscallJournal {
            result["syscall_journal"] = [
                "entered": syscallJournal.entered, "returned": syscallJournal.returned,
                "entries": syscallJournal.entries.map { entry -> [String: Any] in
                    ["nr": entry.nr, "rip": entry.rip, "rax_return": entry.raxReturn.map { $0 as Any } ?? NSNull()]
                },
            ] as [String: Any]
        }
        return result
    }
}

final class LinuxDirectProfileRunner: ProfileRunner {
    private enum Phase { case idle, running, finished }

    private let configuration: LinuxDirectConfig
    private let terminationGraceSeconds: Double
    /// Guards `phase`, `childPID` and `stopRequested`: `stop` may come from another thread while `start` waits for the child.
    private let lock = NSLock()
    private var phase = Phase.idle
    private var childPID: pid_t = 0
    private var stopRequested = false
    private(set) var lastStatus: LinuxDirectStatus?
    /// PID of the last child, kept after it was reaped so that a test can look for a zombie.
    private(set) var lastChildPID: pid_t = 0

    init(configuration: LinuxDirectConfig, terminationGraceSeconds: Double = 2) {
        self.configuration = configuration
        self.terminationGraceSeconds = terminationGraceSeconds
    }

    var isRunning: Bool {
        lock.lock()
        defer { lock.unlock() }
        return phase == .running
    }

    // MARK: ProfileRunner

    func start(profile: UInt32, screens: Int) throws {
        guard profile == LinuxDirectProfile.number else {
            throw LinuxDirectError.invalidConfig(field: "machine_profile", detail: "\(profile) is not the linux-direct profile")
        }
        _ = try runSession()
    }

    func pause() throws { throw ProfileUnsupportedError(operation: "pause of the linux-direct board") }

    func resume(reason: ControlV2Reason) throws { throw ProfileUnsupportedError(operation: "resume of the linux-direct board") }

    func stop() throws {
        lock.lock()
        guard phase == .running, childPID > 0 else {
            lock.unlock()
            return
        }
        stopRequested = true
        let pid = childPID
        kill(pid, SIGTERM)
        lock.unlock()
        // The thread that waits in `runSession` reaps the child; give it the grace period, then kill.
        let deadline = Date().addingTimeInterval(terminationGraceSeconds)
        while Date() < deadline {
            if !isRunning { return }
            usleep(2_000)
        }
        lock.lock()
        if phase == .running, childPID == pid { kill(pid, SIGKILL) }
        lock.unlock()
        while isRunning { usleep(2_000) }
    }

    // MARK: session

    /// Validates, spawns `od-runner linux-direct --run`, waits for the child, reads the report. Returns the status of the machine.
    @discardableResult
    func runSession() throws -> LinuxDirectStatus {
        lock.lock()
        guard phase != .running else {
            lock.unlock()
            throw LinuxDirectError.sessionRunning
        }
        lock.unlock()
        // Everything is checked before the first system call that makes anything: no directory, no process.
        if let violation = configuration.validate() { throw violation }
        guard let runner = configuration.resolvedRunner else { throw LinuxDirectError.invalidConfig(field: "runner", detail: "no path") }

        var template = Array((NSTemporaryDirectory() + "/od-vmhost-linux-direct-XXXXXX").utf8CString)
        guard mkdtemp(&template) != nil else { throw LinuxDirectError.spawnFailed(errno) }
        let directory = String(cString: template)
        defer { try? FileManager.default.removeItem(atPath: directory) }
        let reportPath = directory + "/report.json"
        let stderrPath = directory + "/stderr.log"

        lock.lock()
        phase = .running
        stopRequested = false
        lastStatus = nil
        lock.unlock()

        let pid: pid_t
        do {
            pid = try spawn(runner: runner, arguments: configuration.arguments(out: reportPath), stderrPath: stderrPath)
        } catch {
            lock.lock()
            phase = .idle
            lock.unlock()
            throw error
        }
        lock.lock()
        childPID = pid
        lastChildPID = pid
        lock.unlock()

        let waitStatus = reap(pid)
        let wasStopped = finish()
        if wasStopped, !(waitStatus.exited && waitStatus.code == 0) { throw LinuxDirectError.terminatedByStop }
        let result = try interpret(waitStatus, reportPath: reportPath, stderrPath: stderrPath)
        lock.lock()
        lastStatus = result
        lock.unlock()
        return result
    }

    private struct WaitOutcome {
        var exited: Bool
        var code: Int32
        var signal: Int32
    }

    private func spawn(runner: String, arguments: [String], stderrPath: String) throws -> pid_t {
        var actions: posix_spawn_file_actions_t?
        var attributes: posix_spawnattr_t?
        posix_spawn_file_actions_init(&actions)
        posix_spawnattr_init(&attributes)
        defer {
            posix_spawn_file_actions_destroy(&actions)
            posix_spawnattr_destroy(&attributes)
        }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, "/dev/null", O_WRONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 2, stderrPath, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
        var defaults = sigset_t()
        var mask = sigset_t()
        sigfillset(&defaults)
        sigemptyset(&mask)
        posix_spawnattr_setsigdefault(&attributes, &defaults)
        posix_spawnattr_setsigmask(&attributes, &mask)
        // Only descriptors 0, 1 and 2 of the file actions reach the child (the control socket, locks and shm stay in VMHost).
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK))
        let argv: [UnsafeMutablePointer<CChar>?] = ([runner] + arguments).map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        var pid: pid_t = 0
        let code = posix_spawn(&pid, runner, &actions, &attributes, argv, environ)
        guard code == 0 else { throw LinuxDirectError.spawnFailed(code) }
        return pid
    }

    /// Waits for the child with `waitpid(WNOHANG)` under the lock that `stop` takes before it signals: the PID is never signalled
    /// after it was reaped.
    private func reap(_ pid: pid_t) -> WaitOutcome {
        while true {
            lock.lock()
            var raw: Int32 = 0
            let result = waitpid(pid, &raw, WNOHANG)
            if result == pid {
                childPID = 0
                lock.unlock()
                let signal = raw & 0x7f
                return WaitOutcome(exited: signal == 0, code: (raw >> 8) & 0xff, signal: signal == 0 ? 0 : signal)
            }
            if result < 0, errno != EINTR {
                childPID = 0
                lock.unlock()
                return WaitOutcome(exited: false, code: -1, signal: 0)
            }
            lock.unlock()
            usleep(2_000)
        }
    }

    private func finish() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        phase = .finished
        return stopRequested
    }

    private func interpret(_ outcome: WaitOutcome, reportPath: String, stderrPath: String) throws -> LinuxDirectStatus {
        let report = FileManager.default.contents(atPath: reportPath)
        guard outcome.exited else {
            throw LinuxDirectError.runnerFailed(outcome.signal != 0 ? "terminated by signal \(outcome.signal)" : "waitpid failed")
        }
        switch outcome.code {
        case 0:
            guard let report else { throw LinuxDirectError.runnerFailed("exit 0 without a report file") }
            return try LinuxDirectStatus.parse(report: report)
        case 3, 4:
            let error = report.flatMap { (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any] }
                .flatMap { $0["error"] as? [String: Any] }
            var detail = error?["detail"] as? String
            if detail == nil, let log = FileManager.default.contents(atPath: stderrPath), !log.isEmpty {
                detail = String(decoding: log.prefix(512), as: UTF8.self)
            }
            throw LinuxDirectError.runnerRefused(exitCode: outcome.code, code: error?["code"] as? String,
                                                 stage: error?["stage"] as? String, detail: detail)
        default:
            throw LinuxDirectError.runnerFailed("exit code \(outcome.code)")
        }
    }
}

/// `od-vmhost --linux-direct <config.json>`: one session, the status as one JSON line on stdout.
/// Exit 0: the machine ran and the status is printed (a guest fault included); 2: usage or invalid config (no process spawned);
/// 6: the runner refused or failed.
enum LinuxDirectCommand {
    static func run(_ args: [String]) -> Int32 {
        guard args.count == 2, args[0] == "--linux-direct" else {
            fputs("od-vmhost: usage: --linux-direct <config.json>\n", stderr)
            return VMHostExitStatus.usage
        }
        do {
            guard let data = FileManager.default.contents(atPath: args[1]) else {
                throw LinuxDirectError.invalidConfig(field: "config", detail: "cannot read \(args[1])")
            }
            let runner = LinuxDirectProfileRunner(configuration: try LinuxDirectConfig.parse(json: data))
            let status = try runner.runSession()
            print(String(decoding: (try? JSONSerialization.data(withJSONObject: status.jsonObject, options: [.sortedKeys])) ?? Data(), as: UTF8.self))
            return VMHostExitStatus.ok
        } catch let error as LinuxDirectError {
            var refusal: [String: Any] = ["status": "refused", "reason": "\(error)"]
            if case .runnerRefused(let exitCode, let code, let stage, _) = error {
                refusal["runner_exit"] = exitCode
                refusal["error_code"] = code ?? NSNull()
                refusal["error_stage"] = stage ?? NSNull()
            }
            print(String(decoding: (try? JSONSerialization.data(withJSONObject: refusal, options: [.sortedKeys])) ?? Data(), as: UTF8.self))
            if case .invalidConfig = error { return VMHostExitStatus.usage }
            return VMHostExitStatus.failed
        } catch {
            fputs("od-vmhost: \(error)\n", stderr)
            return VMHostExitStatus.failed
        }
    }
}
