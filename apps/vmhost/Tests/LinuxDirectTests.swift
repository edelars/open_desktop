import Darwin
import Foundation
import XCTest
import ODShared
@testable import od_vmhost

// S4-41b: VMHost runs the Linux direct-boot machine as a child process (`LinuxDirectRunner.swift`). The tests never start a guest and
// never need the real `od-runner`: the child is a small shell script that plays the runner (it writes the report named by `--out`
// and exits with the code the case asks for), so the process handling, the validation before spawn, the status, stop, the refusal of
// pause/resume and the absence of leaked descriptors, temp directories and zombies are checked on a real `posix_spawn`.
// Run with `swift test --filter LinuxDirectXCTests` from apps/.

private final class UnsupportedPauseRunner: ProfileRunner {
    var calls: [String] = []
    func start(profile: UInt32, screens: Int) throws { calls.append("start") }
    func pause() throws { throw ProfileUnsupportedError(operation: "pause") }
    func resume(reason: ControlV2Reason) throws { throw ProfileUnsupportedError(operation: "resume") }
    func stop() throws { calls.append("stop") }
}

final class LinuxDirectXCTests: XCTestCase {
    private var directory: String!

    override func setUpWithError() throws {
        var template = Array((NSTemporaryDirectory() + "/od-linux-direct-test-XXXXXX").utf8CString)
        XCTAssertNotNil(mkdtemp(&template))
        directory = String(cString: template)
        for name in ["vmlinuz", "initramfs", "acpi"] { try Data(name.utf8).write(to: URL(fileURLWithPath: path(name))) }
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(atPath: directory)
    }

    private func path(_ name: String) -> String { directory + "/" + name }

    private func refusal(_ code: String, _ stage: String) -> String {
        "{\"schema_version\": 1, \"command\": \"linux-direct\", \"status\": \"refused\", "
            + "\"error\": {\"code\": \"\(code)\", \"stage\": \"\(stage)\", \"detail\": \"d-\(code)\"}}"
    }

    private func report(exitKind: String, retired: Int, com1: String, userMode: Bool = false) -> String {
        var stop = "\"exit_kind\": \"\(exitKind)\", \"retired\": \(retired), \"com1_text\": \"\(com1)\", \"rip\": \"0x1000\""
        if userMode {
            stop += ", \"user_mode\": true, \"cpl3_retired\": 7, \"syscall_journal\": {\"capacity\": 8, \"entered\": 2, \"returned\": 1, "
                + "\"entries\": [{\"nr\": 1, \"rip\": \"0x401000\", \"rax_return\": \"0x1\"}, {\"nr\": 60, \"rip\": \"0x401010\", \"rax_return\": null}]}"
        }
        return "{\"schema_version\": 1, \"command\": \"linux-direct\", \"status\": \"loaded\", \"guest_started\": true, "
            + "\"retired\": \(retired), \"stop\": {\(stop)}}"
    }

    /// Writes an executable script that plays `od-runner`. It appends one line to `<dir>/spawns`, stores its arguments in
    /// `<dir>/args`, then runs `body` (`$out` is the `--out` path).
    private func script(_ name: String, _ body: String) throws -> String {
        let text = """
        #!/bin/sh
        echo spawn >> "\(path("spawns"))"
        printf '%s\\n' "$@" > "\(path("args"))"
        out=""
        while [ $# -gt 0 ]; do if [ "$1" = "--out" ]; then out="$2"; fi; shift; done
        \(body)
        """
        try text.write(toFile: path(name), atomically: true, encoding: .utf8)
        XCTAssertEqual(chmod(path(name), 0o755), 0)
        return path(name)
    }

    private func reportScript(_ name: String, json: String, exitCode: Int32 = 0) throws -> String {
        try script(name, "cat > \"$out\" <<'JSON'\n\(json)\nJSON\nexit \(exitCode)")
    }

    private func config(runner: String?, setCPL0: Bool = false) -> LinuxDirectConfig {
        let hash = String(repeating: "ab", count: 32)
        return LinuxDirectConfig(
            runner: runner, vmlinuz: path("vmlinuz"), initramfs: path("initramfs"), acpiBlob: path("acpi"), nonce: "N0nce-1",
            expectedVmlinuzSHA256: hash, expectedInitramfsSHA256: hash, expectedAcpiManifestHash: hash, ramMiB: 128, budget: 1000,
            setCPL0: setCPL0)
    }

    private var spawnCount: Int {
        ((try? String(contentsOfFile: path("spawns"), encoding: .utf8)) ?? "").split(separator: "\n").count
    }

    private func openDescriptorCount() -> Int { (0..<512).filter { fcntl(Int32($0), F_GETFD) != -1 }.count }

    private func leftoverSessionDirectories() -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: NSTemporaryDirectory())) ?? []).filter { $0.hasPrefix("od-vmhost-linux-direct-") }
    }

    private func assertGone(_ pid: pid_t, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertGreaterThan(pid, 0, file: file, line: line)
        errno = 0
        // A zombie would still answer `kill(pid, 0)`; a reaped process is ESRCH.
        XCTAssertEqual(kill(pid, 0), -1, "process \(pid) is still there (zombie or running)", file: file, line: line)
        XCTAssertEqual(errno, ESRCH, file: file, line: line)
    }

    // MARK: start and status

    func testStartRunsTheChildAndHandsOutTheStopFieldsAsStatus() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(
            runner: try reportScript("ok", json: report(exitKind: "halted", retired: 4242, com1: "hi"))))
        let status = try runner.runSession()
        XCTAssertEqual(status.exitKind, "halted")
        XCTAssertEqual(status.retired, 4242)
        XCTAssertEqual(status.com1Text, "hi")
        XCTAssertTrue(status.guestStarted)
        XCTAssertNil(status.cpl3Retired)
        XCTAssertNil(status.syscallJournal)
        XCTAssertEqual(runner.lastStatus, status)
        XCTAssertEqual(spawnCount, 1)
        // The arguments are the validated configuration, the guest always runs.
        let arguments = try String(contentsOfFile: path("args"), encoding: .utf8).split(separator: "\n").map(String.init)
        XCTAssertEqual(arguments.first, "linux-direct")
        XCTAssertEqual(Array(arguments[arguments.firstIndex(of: "--run")!...]), ["--run", "--budget", "1000"])
        XCTAssertTrue(arguments.contains("--vmlinuz") && arguments.contains(path("vmlinuz")))
        XCTAssertEqual(arguments[arguments.firstIndex(of: "--nonce")! + 1], "N0nce-1")
        XCTAssertFalse(arguments.contains("--set-cpl0"))
    }

    func testUserModeStatusCarriesCpl3AndSyscallJournal() throws {
        var value = config(runner: try reportScript("user", json: report(exitKind: "user_mode_fault", retired: 13, com1: "", userMode: true)),
                           setCPL0: true)
        value.cpuidProfile = "stage4a"
        value.userMode = true
        let status = try LinuxDirectProfileRunner(configuration: value).runSession()
        XCTAssertEqual(status.cpl3Retired, 7)
        XCTAssertEqual(status.syscallJournal, LinuxDirectStatus.SyscallJournal(entered: 2, returned: 1, entries: [
            .init(nr: 1, rip: "0x401000", raxReturn: "0x1"), .init(nr: 60, rip: "0x401010", raxReturn: nil),
        ]))
        let arguments = try String(contentsOfFile: path("args"), encoding: .utf8)
        XCTAssertTrue(arguments.contains("--set-cpl0\n") && arguments.contains("--cpuid-profile\nstage4a\n") && arguments.contains("--user-mode\n"))
    }

    func testGuestFaultIsAResultNotACrash() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(
            runner: try reportScript("fault", json: report(exitKind: "fault", retired: 3, com1: ""))))
        let status = try runner.runSession()
        XCTAssertEqual(status.exitKind, "fault")
        XCTAssertEqual(status.retired, 3)
        XCTAssertTrue(status.guestStarted)
    }

    func testStartProfileNumberMustBeTheLinuxProfile() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("ok", json: report(exitKind: "halted", retired: 1, com1: ""))))
        XCTAssertThrowsError(try runner.start(profile: 1, screens: 1)) {
            XCTAssertEqual($0 as? LinuxDirectError, .invalidConfig(field: "machine_profile", detail: "1 is not the linux-direct profile"))
        }
        XCTAssertEqual(spawnCount, 0)
        try runner.start(profile: LinuxDirectProfile.number, screens: 1)
        XCTAssertEqual(runner.lastStatus?.exitKind, "halted")
    }

    // MARK: refusals

    func testRunnerRefusalsAreReportedWithTheirReasonAndLeaveNoStatus() throws {
        for (exitCode, code, stage) in [(Int32(4), "vmlinuz_sha256_mismatch", "vmlinuz_sha256"), (3, "invalid_input", "vmlinuz_file")] {
            let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("refuse\(exitCode)", json: refusal(code, stage), exitCode: exitCode)))
            XCTAssertThrowsError(try runner.runSession()) {
                XCTAssertEqual($0 as? LinuxDirectError, .runnerRefused(exitCode: exitCode, code: code, stage: stage, detail: "d-\(code)"))
            }
            XCTAssertNil(runner.lastStatus)
            XCTAssertFalse(runner.isRunning)
            assertGone(runner.lastChildPID)
        }
        // What the real runner does on a refusal of its arguments (exit 3): the JSON of the refusal goes to STDOUT, `--out` is not written.
        let stdoutOnly = try script("args3", "cat <<'JSON'\n\(refusal("invalid_input", "arguments"))\nJSON\nexit 3")
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: stdoutOnly)).runSession()) {
            XCTAssertEqual($0 as? LinuxDirectError, .runnerRefused(exitCode: 3, code: "invalid_input", stage: "arguments", detail: "d-invalid_input"))
        }
        // No JSON anywhere: the stderr text is the detail.
        let noJSON = try script("args3b", "echo 'bad nonce' >&2\nexit 3")
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: noJSON)).runSession()) {
            XCTAssertEqual($0 as? LinuxDirectError, .runnerRefused(exitCode: 3, code: nil, stage: nil, detail: "bad nonce\n"))
        }
        // An unexpected exit code, a signal and a report that is not a linux-direct report are failures, not statuses.
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: try script("code9", "exit 9"))).runSession()) {
            XCTAssertEqual($0 as? LinuxDirectError, .runnerFailed("exit code 9"))
        }
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: try script("sig", "kill -KILL $$"))).runSession()) {
            XCTAssertEqual($0 as? LinuxDirectError, .runnerFailed("terminated by signal 9"))
        }
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: try reportScript("junk", json: "[1]"))).runSession())
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(
            runner: try reportScript("nostop", json: "{\"command\": \"linux-direct\", \"status\": \"loaded\", \"guest_started\": false}"))).runSession())
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    func testInvalidConfigIsRefusedBeforeAnyProcessIsSpawned() throws {
        let good = try reportScript("ok", json: report(exitKind: "halted", retired: 1, com1: ""))
        XCTAssertNil(config(runner: good).validate())
        var cases: [(String, LinuxDirectConfig)] = []
        func make(_ name: String, _ edit: (inout LinuxDirectConfig) -> Void) {
            var value = config(runner: good)
            edit(&value)
            cases.append((name, value))
        }
        make("profile") { $0.profile = "display-test-pattern" }
        make("vmlinuz") { $0.vmlinuz = self.path("missing") }
        make("initramfs") { $0.initramfs = self.directory }
        make("acpi_blob") { $0.acpiBlob = self.path("missing") }
        make("nonce") { $0.nonce = "a b" }
        make("nonce") { $0.nonce = "" }
        make("nonce") { $0.nonce = String(repeating: "n", count: 65) }
        make("ram_mib") { $0.ramMiB = 0 }
        make("ram_mib") { $0.ramMiB = 8193 }
        make("expected_vmlinuz_sha256") { $0.expectedVmlinuzSHA256 = "abc" }
        make("expected_initramfs_sha256") { $0.expectedInitramfsSHA256 = String(repeating: "g", count: 64) }
        make("expected_acpi_manifest_hash") { $0.expectedAcpiManifestHash = "" }
        make("cpuid_profile") { $0.cpuidProfile = "nope"; $0.setCPL0 = true }
        make("cpuid_profile") { $0.cpuidProfile = "stage4a" }
        make("user_mode") { $0.userMode = true; $0.setCPL0 = true }
        make("runner") { $0.runner = self.path("missing-runner") }
        make("runner") { $0.runner = self.path("vmlinuz") }
        for (field, value) in cases {
            guard case .invalidConfig(let reported, _)? = value.validate() else { XCTFail("\(field) was accepted"); continue }
            XCTAssertEqual(reported, field)
            XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: value).runSession()) {
                XCTAssertEqual(($0 as? LinuxDirectError).flatMap { error -> String? in
                    if case .invalidConfig(let name, _) = error { return name }
                    return nil
                }, field)
            }
        }
        XCTAssertEqual(spawnCount, 0, "a refused configuration spawned the runner")
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    func testConfigFileParsing() throws {
        func json(_ edit: (inout [String: Any]) -> Void = { _ in }) throws -> Data {
            var object: [String: Any] = [
                "profile": "linux-direct", "vmlinuz": "/k", "initramfs": "/i", "acpi_blob": "/a", "nonce": "n1",
                "expected_vmlinuz_sha256": String(repeating: "0", count: 64), "expected_initramfs_sha256": String(repeating: "1", count: 64),
                "expected_acpi_manifest_hash": String(repeating: "2", count: 64), "ram_mib": 128, "budget": 5,
            ]
            edit(&object)
            return try JSONSerialization.data(withJSONObject: object)
        }
        let parsed = try LinuxDirectConfig.parse(json: try json { $0["set_cpl0"] = true; $0["cpuid_profile"] = "stage4a"; $0["user_mode"] = true })
        XCTAssertEqual(parsed.ramMiB, 128)
        XCTAssertEqual(parsed.budget, 5)
        XCTAssertTrue(parsed.setCPL0 && parsed.userMode)
        XCTAssertEqual(parsed.cpuidProfile, "stage4a")
        XCTAssertNil(parsed.runner)
        for (name, edit) in [
            ("budget", { (o: inout [String: Any]) in o["budget"] = -1 }),
            ("budget", { (o: inout [String: Any]) in o["budget"] = "5" }),
            ("ram_mib", { (o: inout [String: Any]) in o["ram_mib"] = 1.5 }),
            ("nonce", { (o: inout [String: Any]) in o.removeValue(forKey: "nonce") }),
            ("set_cpl0", { (o: inout [String: Any]) in o["set_cpl0"] = 1 }),
            ("vmlinuz", { (o: inout [String: Any]) in o["vmlinuz"] = 3 }),
        ] {
            XCTAssertThrowsError(try LinuxDirectConfig.parse(json: try json(edit))) {
                guard case .invalidConfig(let field, _)? = $0 as? LinuxDirectError else { return XCTFail("\($0)") }
                XCTAssertEqual(field, name)
            }
        }
        XCTAssertThrowsError(try LinuxDirectConfig.parse(json: Data("[]".utf8)))
    }

    // MARK: pause, resume, stop

    func testPauseAndResumeAreRefusedAsUnsupportedWithoutChangingState() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("ok", json: report(exitKind: "halted", retired: 9, com1: ""))))
        let before = try runner.runSession()
        XCTAssertThrowsError(try runner.pause()) { XCTAssertEqual($0 as? ProfileUnsupportedError, ProfileUnsupportedError(operation: "pause of the linux-direct board")) }
        XCTAssertThrowsError(try runner.resume(reason: .user)) { XCTAssertTrue($0 is ProfileUnsupportedError) }
        XCTAssertEqual(runner.lastStatus, before)
        XCTAssertFalse(runner.isRunning)
        XCTAssertEqual(spawnCount, 1)
        // Through the state machine: ERROR `unsupported`, the state stays `running`, the runner is not stopped.
        let stub = UnsupportedPauseRunner()
        let lifecycle = Lifecycle(runner: stub, lockFailure: nil, hostMemoryBytes: 64 << 30)
        func send(_ request: ControlV2Request, id: UInt32) throws -> ControlV2ErrorReport? {
            let output = try lifecycle.handle(ControlV2Received(frame: ControlV2Frame(request: request, requestID: id), descriptor: nil))
            let all = try output.frames.map { try ControlV2Response.parse($0) }
            if case .error(let report)? = all.first { return report }
            return nil
        }
        XCTAssertNil(try send(.hello, id: 0))
        XCTAssertNil(try send(.configure(ControlV2Configure()), id: 1))
        XCTAssertNil(try send(.start(operationID: 0), id: 2))
        let paused = try XCTUnwrap(try send(.pause(reason: 1), id: 3))
        XCTAssertEqual(paused.code, ControlV2ErrorCode.unsupported.rawValue)
        XCTAssertEqual(paused.failedKind, 7)
        XCTAssertEqual(lifecycle.state, .running)
        // `resume` while running is `invalid_state` (the table of the state machine); the refusal is reachable only from `paused`.
        XCTAssertEqual(try XCTUnwrap(try send(.resume(reason: 1), id: 4)).code, ControlV2ErrorCode.invalidState.rawValue)
        XCTAssertEqual(lifecycle.state, .running)
        XCTAssertEqual(stub.calls, ["start"])
    }

    func testStopIsIdempotentWithoutAndAfterASession() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("ok", json: report(exitKind: "halted", retired: 5, com1: ""))))
        try runner.stop()
        let status = try runner.runSession()
        try runner.stop()
        try runner.stop()
        XCTAssertEqual(runner.lastStatus, status)
        assertGone(runner.lastChildPID)
        let again = try runner.runSession()
        XCTAssertEqual(again, status)
    }

    func testStopEndsARunningChildWithSigtermAndReapsIt() throws {
        let pidFile = path("child.pid")
        let sleeper = try script("sleeper", "echo $$ > \"\(pidFile).tmp\"; mv \"\(pidFile).tmp\" \"\(pidFile)\"\nexec sleep 30")
        let runner = LinuxDirectProfileRunner(configuration: config(runner: sleeper))
        let finished = expectation(description: "session ends")
        var outcome: Result<LinuxDirectStatus, Error>?
        let thread = Thread {
            outcome = Result { try runner.runSession() }
            finished.fulfill()
        }
        thread.start()
        let deadline = Date().addingTimeInterval(10)
        while !FileManager.default.fileExists(atPath: pidFile), Date() < deadline { usleep(5_000) }
        let pid = pid_t(try String(contentsOfFile: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines))!
        XCTAssertTrue(runner.isRunning)
        XCTAssertThrowsError(try runner.runSession()) { XCTAssertEqual($0 as? LinuxDirectError, .sessionRunning) }
        try runner.stop()
        wait(for: [finished], timeout: 10)
        XCTAssertFalse(runner.isRunning)
        guard case .failure(let error)? = outcome else { return XCTFail("a stopped session returned a status") }
        XCTAssertEqual(error as? LinuxDirectError, .terminatedByStop)
        XCTAssertNil(runner.lastStatus)
        assertGone(pid)
        try runner.stop()
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    func testStopKillsAChildThatIgnoresSigterm() throws {
        let pidFile = path("stubborn.pid")
        let stubborn = try script("stubborn", "trap '' TERM\necho $$ > \"\(pidFile).tmp\"; mv \"\(pidFile).tmp\" \"\(pidFile)\"\nwhile :; do sleep 1; done")
        let runner = LinuxDirectProfileRunner(configuration: config(runner: stubborn), terminationGraceSeconds: 0.3)
        let finished = expectation(description: "session ends")
        var outcome: Result<LinuxDirectStatus, Error>?
        Thread {
            outcome = Result { try runner.runSession() }
            finished.fulfill()
        }.start()
        let deadline = Date().addingTimeInterval(10)
        while !FileManager.default.fileExists(atPath: pidFile), Date() < deadline { usleep(5_000) }
        usleep(200_000) // the trap is set before the pid file is written; the sleep in the loop only has to start
        try runner.stop()
        wait(for: [finished], timeout: 10)
        guard case .failure(let error)? = outcome else { return XCTFail("a stopped session returned a status") }
        XCTAssertEqual(error as? LinuxDirectError, .terminatedByStop)
        assertGone(runner.lastChildPID)
    }

    func testReportNumbersAreStrictAndTheGuestMustHaveStarted() throws {
        func parse(_ stop: String, started: String = "true") throws -> LinuxDirectStatus {
            try LinuxDirectStatus.parse(report: Data(("{\"command\": \"linux-direct\", \"status\": \"loaded\", \"guest_started\": \(started), "
                + "\"stop\": {\"exit_kind\": \"halted\", \"com1_text\": \"\", \(stop)}}").utf8))
        }
        XCTAssertEqual(try parse("\"retired\": 18446744073709551615").retired, UInt64.max)
        for bad in ["true", "1.5", "-1", "\"7\"", "null"] {
            XCTAssertThrowsError(try parse("\"retired\": \(bad)"), "retired \(bad)")
        }
        XCTAssertThrowsError(try parse("\"retired\": 1, \"cpl3_retired\": true"))
        XCTAssertThrowsError(try parse("\"retired\": 1", started: "false"))
    }

    // MARK: races

    func testASecondSessionIsRefusedWhileTheFirstIsStillBeforeItsSpawn() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("ok", json: report(exitKind: "halted", retired: 1, com1: ""))))
        let inWindow = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        runner.stageHook = { stage in
            guard stage == .beforeSpawn else { return }
            inWindow.signal()
            release.wait()
        }
        let finished = expectation(description: "first session ends")
        var outcome: Result<LinuxDirectStatus, Error>?
        Thread {
            outcome = Result { try runner.runSession() }
            finished.fulfill()
        }.start()
        XCTAssertEqual(inWindow.wait(timeout: .now() + 10), .success)
        // The first session has been accepted but has not spawned: the phase is already `running`.
        XCTAssertTrue(runner.isRunning)
        XCTAssertThrowsError(try runner.runSession()) { XCTAssertEqual($0 as? LinuxDirectError, .sessionRunning) }
        XCTAssertEqual(spawnCount, 0)
        runner.stageHook = nil
        release.signal()
        wait(for: [finished], timeout: 10)
        guard case .success(let status)? = outcome else { return XCTFail("the first session failed: \(String(describing: outcome))") }
        XCTAssertEqual(status.exitKind, "halted")
        XCTAssertEqual(spawnCount, 1)
        // A failed validation gives the session back: the error of the second call is the validation, not `sessionRunning`.
        var broken = config(runner: nil)
        broken.nonce = "a b"
        let failing = LinuxDirectProfileRunner(configuration: broken)
        for _ in 0..<2 {
            XCTAssertThrowsError(try failing.runSession()) {
                guard case .invalidConfig(let field, _)? = $0 as? LinuxDirectError else { return XCTFail("\($0)") }
                XCTAssertEqual(field, "nonce")
            }
            XCTAssertFalse(failing.isRunning)
        }
    }

    func testStopBeforeSpawnIsNotLostAndNothingIsSpawned() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("ok", json: report(exitKind: "halted", retired: 1, com1: ""))))
        let stopper = DispatchQueue(label: "stopper")
        let stopped = expectation(description: "stop returns")
        runner.stageHook = { stage in
            guard stage == .beforeSpawn else { return }
            stopper.async {
                try? runner.stop()
                stopped.fulfill()
            }
            while !runner.isStopRequested { usleep(1_000) }
        }
        XCTAssertThrowsError(try runner.runSession()) { XCTAssertEqual($0 as? LinuxDirectError, .terminatedByStop) }
        wait(for: [stopped], timeout: 10)
        XCTAssertEqual(spawnCount, 0)
        XCTAssertFalse(runner.isRunning)
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    func testStopBetweenSpawnAndRegistrationOfThePidStillEndsTheChild() throws {
        let sleeper = try script("sleeper2", "exec sleep 30")
        let runner = LinuxDirectProfileRunner(configuration: config(runner: sleeper), terminationGraceSeconds: 0.3)
        let stopper = DispatchQueue(label: "stopper")
        let stopped = expectation(description: "stop returns")
        runner.stageHook = { stage in
            guard stage == .afterSpawn else { return }
            stopper.async {
                try? runner.stop()
                stopped.fulfill()
            }
            while !runner.isStopRequested { usleep(1_000) }
        }
        XCTAssertThrowsError(try runner.runSession()) { XCTAssertEqual($0 as? LinuxDirectError, .terminatedByStop) }
        wait(for: [stopped], timeout: 10)
        // The child may be killed before its shell script wrote anything, so `spawnCount` is not checked; the PID is what counts.
        assertGone(runner.lastChildPID)
        XCTAssertFalse(runner.isRunning)
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    // MARK: signals of VMHost itself

    /// `od-vmhost` as built next to the test bundle by SwiftPM; nil when it is not there.
    private var vmhostExecutable: String? {
        let candidate = Bundle(for: LinuxDirectXCTests.self).bundleURL.deletingLastPathComponent().appendingPathComponent("od-vmhost").path
        return FileManager.default.isExecutableFile(atPath: candidate) ? candidate : nil
    }

    func testSigtermOfVMHostStopsTheChildAndRemovesTheDirectory() throws {
        guard let vmhost = vmhostExecutable else { throw XCTSkip("od-vmhost is not built next to the test bundle") }
        let pidFile = path("signal.pid")
        let sleeper = try script("sleeper3", "echo $$ > \"\(pidFile).tmp\"; mv \"\(pidFile).tmp\" \"\(pidFile)\"\nexec sleep 30")
        let value = config(runner: sleeper)
        let document: [String: Any] = [
            "profile": value.profile, "runner": sleeper, "vmlinuz": value.vmlinuz, "initramfs": value.initramfs, "acpi_blob": value.acpiBlob,
            "nonce": value.nonce, "expected_vmlinuz_sha256": value.expectedVmlinuzSHA256,
            "expected_initramfs_sha256": value.expectedInitramfsSHA256, "expected_acpi_manifest_hash": value.expectedAcpiManifestHash,
            "ram_mib": 128, "budget": 10,
        ]
        try JSONSerialization.data(withJSONObject: document).write(to: URL(fileURLWithPath: path("vmhost.json")))
        let process = Process()
        process.executableURL = URL(fileURLWithPath: vmhost)
        process.arguments = ["--linux-direct", path("vmhost.json")]
        let output = Pipe()
        process.standardOutput = output
        try process.run()
        let deadline = Date().addingTimeInterval(15)
        while !FileManager.default.fileExists(atPath: pidFile), Date() < deadline { usleep(5_000) }
        let child = pid_t(try String(contentsOfFile: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines))!
        XCTAssertEqual(kill(child, 0), 0)
        kill(process.processIdentifier, SIGTERM)
        process.waitUntilExit()
        XCTAssertEqual(process.terminationReason, .exit)
        XCTAssertEqual(process.terminationStatus, VMHostExitStatus.failed)
        let printed = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        XCTAssertTrue(printed.contains("\"status\":\"refused\""), printed)
        errno = 0
        XCTAssertEqual(kill(child, 0), -1, "the child outlived VMHost")
        XCTAssertEqual(errno, ESRCH)
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    // MARK: the real runner

    /// `target/debug/od-runner` of this repository (the test file is `<repo>/apps/vmhost/Tests/`), built by
    /// `cargo build -p od-runner --bin od-runner`; the test is skipped, not failed, when it is not there.
    func testTheRealRunnerRefusesAForeignKernelAndVMHostReportsItsCode() throws {
        let repository = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let binary = repository.appendingPathComponent("target/debug/od-runner").path
        guard FileManager.default.isExecutableFile(atPath: binary) else {
            throw XCTSkip("SKIPPED: \(binary) is not built (cargo build -p od-runner --bin od-runner)")
        }
        let runner = LinuxDirectProfileRunner(configuration: config(runner: binary))
        // The files are not a kernel and the hash is not the pinned one: exit 4 `vmlinuz_sha256_mismatch`, the guest never starts.
        XCTAssertThrowsError(try runner.runSession()) {
            guard case .runnerRefused(let exitCode, let code, let stage, _)? = $0 as? LinuxDirectError else { return XCTFail("\($0)") }
            XCTAssertEqual(exitCode, 4)
            XCTAssertEqual(code, "vmlinuz_sha256_mismatch")
            XCTAssertEqual(stage, "vmlinuz_sha256")
        }
        XCTAssertNil(runner.lastStatus)
        assertGone(runner.lastChildPID)
        // A missing kernel is refused by VMHost itself, before the runner is spawned.
        var missing = config(runner: binary)
        missing.vmlinuz = path("missing")
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: missing).runSession())
        XCTAssertEqual(leftoverSessionDirectories(), [])
    }

    // MARK: repeated sessions and leaks

    func testTwoSessionsInOneProcessGiveTheSameStatusAndLeakNothing() throws {
        let runner = LinuxDirectProfileRunner(configuration: config(
            runner: try reportScript("ok", json: report(exitKind: "user_mode_fault", retired: 13, com1: "x", userMode: true))))
        let descriptorsBefore = openDescriptorCount()
        let first = try runner.runSession()
        let firstPID = runner.lastChildPID
        let second = try runner.runSession()
        let secondPID = runner.lastChildPID
        XCTAssertEqual(second, first, "the reports of two sessions differ")
        XCTAssertEqual(second.report, first.report)
        XCTAssertNotEqual(firstPID, secondPID)
        assertGone(firstPID)
        assertGone(secondPID)
        XCTAssertEqual(spawnCount, 2)
        XCTAssertEqual(openDescriptorCount(), descriptorsBefore, "descriptors leaked by two sessions")
        XCTAssertEqual(leftoverSessionDirectories(), [], "session directories were not removed")
        XCTAssertFalse(runner.isRunning)
        // A refusal between two sessions does not poison the next one.
        let refused = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("r4", json: "{}", exitCode: 4)))
        XCTAssertThrowsError(try refused.runSession())
        XCTAssertEqual(try runner.runSession(), first)
        XCTAssertEqual(openDescriptorCount(), descriptorsBefore)
    }

    func testTheChildDoesNotInheritDescriptorsOfVMHost() throws {
        // A descriptor of this process that is not close-on-exec must not reach the runner (`POSIX_SPAWN_CLOEXEC_DEFAULT`).
        // A high number, so that the descriptor `ls` opens for its own listing in the child cannot be mistaken for it.
        let opened = open("/dev/null", O_RDONLY | O_CLOEXEC)
        let probe = fcntl(opened, F_DUPFD, 100)
        close(opened)
        XCTAssertGreaterThanOrEqual(probe, 100)
        defer { close(probe) }
        XCTAssertEqual(fcntl(probe, F_GETFD) & FD_CLOEXEC, 0)
        let inherited = path("inherited")
        let runner = LinuxDirectProfileRunner(configuration: config(
            runner: try script("fds", "ls /dev/fd | sort -n | tr '\\n' ' ' > \"\(inherited)\"\ncat > \"$out\" <<'JSON'\n\(report(exitKind: "halted", retired: 1, com1: ""))\nJSON")))
        _ = try runner.runSession()
        let listed = try String(contentsOfFile: inherited, encoding: .utf8).split(separator: " ").compactMap { Int($0) }
        XCTAssertFalse(listed.contains(Int(probe)), "descriptor \(probe) leaked into the child: \(listed)")
        XCTAssertTrue(listed.contains(0) && listed.contains(1) && listed.contains(2))
    }
}
