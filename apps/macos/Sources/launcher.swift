import Darwin
import Dispatch
import Foundation

private enum Issue027Trace {
    private static let enabled = ProcessInfo.processInfo.environment["OD_ISSUE027_TRACE"] == "1"

    static func mark(_ phase: String, detail: String = "") {
        guard enabled else { return }
        let suffix = detail.isEmpty ? "" : " \(detail)"
        fputs(
            "issue027_trace role=launcher uptime_ns=\(DispatchTime.now().uptimeNanoseconds) phase=\(phase)\(suffix)\n",
            stderr
        )
        fflush(stderr)
    }
}

@main
struct OpenDesktopLauncherSmoke {
    static func main() {
        Issue027Trace.mark("launcher_started")
        var parentFD: Int32 = -1
        var childFD: Int32 = -1
        var childPID: pid_t?
        do {
            let config = try parseArguments(Array(CommandLine.arguments.dropFirst()))
            var sockets: [Int32] = [0, 0]
            guard socketpair(AF_UNIX, SOCK_STREAM, 0, &sockets) == 0 else {
                throw LauncherError.socketPair
            }
            parentFD = sockets[0]
            childFD = sockets[1]
            try configureSocket(parentFD)
            try configureSocket(childFD)
            _ = fcntl(childFD, F_SETFD, 0) // keep the helper endpoint across exec

            let pid = try spawnHelper(path: config.helper.path, parentFD: parentFD, childFD: childFD)
            childPID = pid
            Issue027Trace.mark("helper_spawned", detail: "pid=\(pid)")
            Darwin.close(childFD)
            childFD = -1

            try ControlFrame(kind: .hello, requestID: 0, payload: Data()).write(to: parentFD)
            Issue027Trace.mark("hello_sent")
            Issue027Trace.mark("waiting_ready")
            let ready = try ControlFrame.read(from: parentFD)
            Issue027Trace.mark("ready_received", detail: "kind=\(ready.kind) request_id=\(ready.requestID)")
            guard ready.kind == .ready,
                  ready.requestID == 0,
                  ready.payload.count == 4,
                  [UInt8](ready.payload).readLE(UInt32.self, at: 0) == OD_HOST_ABI_VERSION_V0 else {
                throw LauncherError.badHandshake
            }

            var runPayload = Data()
            runPayload.appendLE(UInt32(config.code.count))
            runPayload.appendLE(config.budget)
            for register in config.registers { runPayload.appendLE(register) }
            runPayload.appendLE(config.rip)
            runPayload.appendLE(UInt64(2)) // reserved RFLAGS bit 1
            runPayload.append(contentsOf: config.code)
            try ControlFrame(kind: .run, requestID: 1, payload: runPayload).write(to: parentFD)
            Issue027Trace.mark("run_sent", detail: "request_id=1 bytes=\(runPayload.count)")
            Issue027Trace.mark("waiting_run_ack")
            try expectStatusFrame(.runAck, requestID: 1, fd: parentFD)
            Issue027Trace.mark("run_ack_received")
            Issue027Trace.mark("waiting_result")
            let result = try waitForResult(
                fd: parentFD,
                requestID: 1,
                stopAfterMilliseconds: config.stopAfterMilliseconds
            )
            Issue027Trace.mark("result_received")
            printResult(result)
            Issue027Trace.mark("waiting_child_exit")
            let childStatus = try waitForChild(pid)
            childPID = nil
            Issue027Trace.mark("child_exited", detail: "status=\(childStatus)")
            guard childStatus == 0 else {
                throw LauncherError.childFailed(childStatus)
            }
            Darwin.close(parentFD)
            parentFD = -1
        } catch {
            var reportedError: Error = error
            Issue027Trace.mark("launcher_caught_error", detail: "error=\(error)")
            if let childPID {
                var status: Int32 = 0
                let waited = waitpid(childPID, &status, WNOHANG)
                Issue027Trace.mark("child_wait_probe", detail: "waited=\(waited) status=\(status)")
                if waited == childPID {
                    if status & 0x7F != 0 {
                        reportedError = LauncherError.childSignaled(status & 0x7F)
                        Issue027Trace.mark("child_signaled", detail: "signal=\(status & 0x7F)")
                    } else {
                        let exitStatus = (status >> 8) & 0xFF
                        if exitStatus != 0 {
                            reportedError = LauncherError.childFailed(exitStatus)
                            Issue027Trace.mark("child_failed", detail: "status=\(exitStatus)")
                        } else {
                            Issue027Trace.mark("child_exited", detail: "status=0")
                        }
                    }
                } else {
                    Issue027Trace.mark("terminating_child", detail: "pid=\(childPID)")
                    terminateChild(childPID)
                    Issue027Trace.mark("child_termination_complete", detail: "pid=\(childPID)")
                }
            }
            if parentFD >= 0 { Darwin.close(parentFD) }
            if childFD >= 0 { Darwin.close(childFD) }
            fputs("open-desktop-launcher-smoke: \(reportedError)\n", stderr)
            exit(EXIT_FAILURE)
        }
    }

    private static func configureSocket(_ fd: Int32) throws {
        var noSignal: Int32 = 1
        let status = setsockopt(
            fd,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &noSignal,
            socklen_t(MemoryLayout<Int32>.size)
        )
        if status != 0 { throw LauncherError.socketPair }
    }

    private static func spawnHelper(path: String, parentFD: Int32, childFD: Int32) throws -> pid_t {
        var actions: posix_spawn_file_actions_t?
        guard posix_spawn_file_actions_init(&actions) == 0 else { throw LauncherError.spawnFailed }
        defer { posix_spawn_file_actions_destroy(&actions) }

        let targetFD: Int32 = 3
        guard posix_spawn_file_actions_adddup2(&actions, childFD, targetFD) == 0 else {
            throw LauncherError.spawnFailed
        }
        if childFD != targetFD {
            _ = posix_spawn_file_actions_addclose(&actions, childFD)
        }
        if parentFD != targetFD {
            _ = posix_spawn_file_actions_addclose(&actions, parentFD)
        }

        let arguments = [path, "--socket-fd", String(targetFD)]
        var argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) }
        argv.append(nil)
        defer { argv.forEach { if let value = $0 { free(value) } } }
        var pid: pid_t = 0
        let status = withUnsafePointer(to: actions) { actionPointer in
            path.withCString { executable in
                argv.withUnsafeBufferPointer { pointers in
                    posix_spawn(&pid, executable, actionPointer, nil, pointers.baseAddress!, environ)
                }
            }
        }
        guard status == 0 else { throw LauncherError.spawnFailed }
        return pid
    }

    private static func waitForChild(_ pid: pid_t) throws -> Int32 {
        var status: Int32 = 0
        while true {
            let waited = waitpid(pid, &status, 0)
            if waited == pid { break }
            if waited < 0 && errno == EINTR { continue }
            throw LauncherError.waitFailed
        }
        if status & 0x7F != 0 { return 128 + (status & 0x7F) }
        return (status >> 8) & 0xFF
    }

    private static func terminateChild(_ pid: pid_t) {
        _ = kill(pid, SIGTERM)
        for _ in 0..<50 {
            var childStatus: Int32 = 0
            let waited = waitpid(pid, &childStatus, WNOHANG)
            if waited == pid || (waited < 0 && errno == ECHILD) { return }
            if waited < 0 && errno != EINTR { break }
            usleep(10_000)
        }
        _ = kill(pid, SIGKILL)
        _ = try? waitForChild(pid)
    }

    private static func expectStatusFrame(_ kind: ControlKind, requestID: UInt32, fd: Int32) throws {
        let frame = try ControlFrame.read(from: fd)
        try validateStatusFrame(frame, kind: kind, requestID: requestID)
    }

    private static func validateStatusFrame(_ frame: ControlFrame, kind: ControlKind, requestID: UInt32) throws {
        if frame.kind == .error, frame.requestID == requestID, frame.payload.count == 4 {
            let status = [UInt8](frame.payload).readLE(UInt32.self, at: 0)
            throw LauncherError.apiStatus(status)
        }
        guard frame.kind == kind, frame.requestID == requestID, frame.payload.count == 4 else {
            throw LauncherError.invalidStatus
        }
        let status = [UInt8](frame.payload).readLE(UInt32.self, at: 0)
        guard status == 0 else { throw LauncherError.apiStatus(status) }
    }

    private static func waitForResult(
        fd: Int32,
        requestID: UInt32,
        stopAfterMilliseconds: UInt64?
    ) throws -> Data {
        if let delay = stopAfterMilliseconds {
            let startedAt = DispatchTime.now().uptimeNanoseconds
            let delayNanoseconds = delay.multipliedReportingOverflow(by: 1_000_000)
            let (deadline, deadlineOverflow) = startedAt.addingReportingOverflow(
                delayNanoseconds.overflow ? UInt64.max : delayNanoseconds.partialValue
            )
            let stopDeadline = deadlineOverflow ? UInt64.max : deadline

            while try waitUntilReadable(fd: fd, deadline: stopDeadline) {
                let frame = try ControlFrame.read(from: fd)
                return try resultPayload(from: frame, requestID: requestID)
            }

            do {
                try ControlFrame(kind: .stop, requestID: requestID, payload: Data()).write(to: fd)
            } catch {
                // RESULT can be written just as the helper closes its endpoint.
                let now = DispatchTime.now().uptimeNanoseconds
                let (rescueDeadline, overflow) = now.addingReportingOverflow(1_000_000_000)
                if try waitUntilReadable(fd: fd, deadline: overflow ? UInt64.max : rescueDeadline) {
                    return try resultPayload(from: ControlFrame.read(from: fd), requestID: requestID)
                }
                throw error
            }
        }

        while true {
            let frame = try ControlFrame.read(from: fd)
            if frame.kind == .result {
                return try resultPayload(from: frame, requestID: requestID)
            }
            if frame.kind == .stopAck {
                try validateStatusFrame(frame, kind: .stopAck, requestID: requestID)
                continue
            }
            return try resultPayload(from: frame, requestID: requestID)
        }
    }

    private static func resultPayload(from frame: ControlFrame, requestID: UInt32) throws -> Data {
        if frame.kind == .error, frame.requestID == requestID, frame.payload.count == 4 {
            throw LauncherError.apiStatus([UInt8](frame.payload).readLE(UInt32.self, at: 0))
        }
        guard frame.kind == .result, frame.requestID == requestID, frame.payload.count == 64 else {
            throw LauncherError.invalidResult
        }
        return frame.payload
    }

    private static func waitUntilReadable(fd: Int32, deadline: UInt64) throws -> Bool {
        while true {
            let now = DispatchTime.now().uptimeNanoseconds
            let remaining = deadline > now ? deadline - now : 0
            let timeout = remaining / 1_000_000 + (remaining % 1_000_000 == 0 ? 0 : 1)
            var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            let ready = Darwin.poll(&descriptor, 1, Int32(min(timeout, UInt64(Int32.max))))
            if ready > 0 {
                if descriptor.revents & Int16(POLLIN) != 0 { return true }
                if descriptor.revents & Int16(POLLERR | POLLHUP | POLLNVAL) != 0 {
                    throw LauncherError.channelFailure
                }
                continue
            }
            if ready == 0 {
                if DispatchTime.now().uptimeNanoseconds >= deadline { return false }
                continue
            }
            if errno == EINTR { continue }
            throw LauncherError.channelFailure
        }
    }

    private static func printResult(_ data: Data) {
        let bytes = [UInt8](data)
        let gpr: [UInt64] = (0..<4).map { bytes.readLE(UInt64.self, at: $0 * 8) }
        let rip = bytes.readLE(UInt64.self, at: 32)
        let rflags = bytes.readLE(UInt64.self, at: 40)
        let reason = bytes.readLE(UInt32.self, at: 48)
        let instructions = bytes.readLE(UInt64.self, at: 56)
        let result: [String: Any] = [
            "gpr": gpr,
            "rip": rip,
            "rflags": rflags,
            "reason": reason,
            "instructions_executed": instructions,
        ]
        if let json = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            print(String(decoding: json, as: UTF8.self))
        }
    }

    private static func parseArguments(_ args: [String]) throws -> LauncherConfig {
        var values: [String: String] = [:]
        var index = 0
        while index < args.count {
            guard args[index].hasPrefix("--"), index + 1 < args.count else {
                throw LauncherError.usage
            }
            values[String(args[index].dropFirst(2))] = args[index + 1]
            index += 2
        }
        guard let helperPath = values["vmhost"], let hex = values["hex"] else {
            throw LauncherError.usage
        }
        let compact = hex.filter { !$0.isWhitespace && $0 != "_" }
        guard compact.count.isMultiple(of: 2) else { throw LauncherError.invalidHex }
        var code: [UInt8] = []
        var cursor = compact.startIndex
        while cursor < compact.endIndex {
            let end = compact.index(cursor, offsetBy: 2)
            guard let byte = UInt8(compact[cursor..<end], radix: 16) else {
                throw LauncherError.invalidHex
            }
            code.append(byte)
            cursor = end
        }
        guard code.count <= Int(OD_FRAGMENT_MAX_LENGTH_V0) else { throw LauncherError.invalidHex }
        return LauncherConfig(
            helper: URL(fileURLWithPath: helperPath),
            code: code,
            registers: try ["rax", "rcx", "rdx", "rbx"].map { try parseNumber(values[$0] ?? "0") },
            rip: try parseNumber(values["rip"] ?? "0"),
            budget: try parseNumber(values["budget"] ?? "100000"),
            stopAfterMilliseconds: try values["stop-after-ms"].map(parseNumber)
        )
    }

    private static func parseNumber(_ text: String) throws -> UInt64 {
        if text.hasPrefix("0x"), let number = UInt64(text.dropFirst(2), radix: 16) {
            return number
        }
        guard let number = UInt64(text) else { throw LauncherError.invalidNumber(text) }
        return number
    }
}

private struct LauncherConfig {
    let helper: URL
    let code: [UInt8]
    let registers: [UInt64]
    let rip: UInt64
    let budget: UInt64
    let stopAfterMilliseconds: UInt64?
}

private enum LauncherError: Error, CustomStringConvertible {
    case usage
    case socketPair
    case spawnFailed
    case waitFailed
    case channelFailure
    case badHandshake
    case invalidHex
    case invalidNumber(String)
    case invalidStatus
    case invalidResult
    case apiStatus(UInt32)
    case childFailed(Int32)
    case childSignaled(Int32)

    var description: String {
        switch self {
        case .usage:
            return "usage: launcher --vmhost PATH --hex HEX [--rax N --rcx N --rdx N --rbx N --rip N --budget N --stop-after-ms N]"
        case .socketPair: return "could not create or configure control socketpair"
        case .spawnFailed: return "could not start VMHost helper"
        case .waitFailed: return "could not collect VMHost exit status"
        case .channelFailure: return "control channel poll failed"
        case .badHandshake: return "VMHost protocol/ABI handshake failed"
        case .invalidHex: return "invalid or oversized hex fragment"
        case let .invalidNumber(value): return "invalid integer: \(value)"
        case .invalidStatus: return "invalid VMHost status frame"
        case .invalidResult: return "invalid VMHost result frame"
        case let .apiStatus(status): return "VMHost returned status \(status)"
        case let .childFailed(status): return "VMHost exited with status \(status)"
        case let .childSignaled(signal):
            let name: String
            switch signal {
            case SIGSEGV: name = "SIGSEGV"
            case SIGABRT: name = "SIGABRT"
            case SIGBUS: name = "SIGBUS"
            case SIGILL: name = "SIGILL"
            case SIGFPE: name = "SIGFPE"
            default: name = "signal \(signal)"
            }
            return "VMHost terminated by \(name)"
        }
    }
}
