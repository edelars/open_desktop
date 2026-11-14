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
        let refusal = { (code: String, stage: String) in
            "{\"schema_version\": 1, \"command\": \"linux-direct\", \"status\": \"refused\", "
                + "\"error\": {\"code\": \"\(code)\", \"stage\": \"\(stage)\", \"detail\": \"d-\(code)\"}}"
        }
        for (exitCode, code, stage) in [(Int32(4), "vmlinuz_sha256_mismatch", "vmlinuz_sha256"), (3, "invalid_input", "vmlinuz_file")] {
            let runner = LinuxDirectProfileRunner(configuration: config(runner: try reportScript("refuse\(exitCode)", json: refusal(code, stage), exitCode: exitCode)))
            XCTAssertThrowsError(try runner.runSession()) {
                XCTAssertEqual($0 as? LinuxDirectError, .runnerRefused(exitCode: exitCode, code: code, stage: stage, detail: "d-\(code)"))
            }
            XCTAssertNil(runner.lastStatus)
            XCTAssertFalse(runner.isRunning)
            assertGone(runner.lastChildPID)
        }
        // Exit 3 for bad arguments leaves no report file: the stderr text is the detail.
        let noFile = try script("args3", "echo 'bad nonce' >&2\nexit 3")
        XCTAssertThrowsError(try LinuxDirectProfileRunner(configuration: config(runner: noFile)).runSession()) {
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
        let probe = fcntl(open("/dev/null", O_RDONLY | O_CLOEXEC), F_DUPFD, 100)
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
