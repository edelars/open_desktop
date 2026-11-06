import Darwin
import Foundation

// Shared VMHost launch code (control v1 launcher and the S10-07 display viewer):
// socketpair with SO_NOSIGPIPE on both ends, posix_spawn with the VMHost end duplicated
// onto descriptor 3, child wait and the stop policy SIGTERM -> up to 500 ms -> SIGKILL -> reap
// (protocols/control/CONTROL_PROTOCOL_V1.md "EOF, teardown and failures").

enum VMHostLaunchError: Error, CustomStringConvertible {
    case socketPair
    case spawnFailed
    case waitFailed

    var description: String {
        switch self {
        case .socketPair: return "could not create or configure control socketpair"
        case .spawnFailed: return "could not start VMHost helper"
        case .waitFailed: return "could not collect VMHost exit status"
        }
    }
}

enum VMHostLaunch {
    /// The VMHost end is always descriptor 3 in the child.
    static let childSocketFD: Int32 = 3
    /// S10-16 (INPUT_TRANSPORT_V1.md 2.2): the VMHost end of the separate input socketpair is descriptor 4 (`--input-fd 4`).
    static let childInputFD: Int32 = 4

    static func configureSocket(_ fd: Int32) throws {
        var noSignal: Int32 = 1
        let status = setsockopt(
            fd,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &noSignal,
            socklen_t(MemoryLayout<Int32>.size)
        )
        if status != 0 { throw VMHostLaunchError.socketPair }
    }

    /// Creates the socketpair and configures both ends. `child` is left inheritable across exec.
    /// On failure nothing is left open.
    static func makeSocketPair() throws -> (parent: Int32, child: Int32) {
        var sockets: [Int32] = [0, 0]
        guard socketpair(AF_UNIX, SOCK_STREAM, 0, &sockets) == 0 else {
            throw VMHostLaunchError.socketPair
        }
        do {
            try configureSocket(sockets[0])
            try configureSocket(sockets[1])
        } catch {
            Darwin.close(sockets[0])
            Darwin.close(sockets[1])
            throw error
        }
        _ = fcntl(sockets[1], F_SETFD, 0) // keep the helper endpoint across exec
        return (sockets[0], sockets[1])
    }

    /// `posix_spawn` of `path --socket-fd 3 <extraArguments>` with `childFD` on descriptor 3. With `input` (S10-16) the VMHost end of a
    /// second socketpair is also put on descriptor 4 and `--input-fd 4` is appended; without it the spawn is exactly as before.
    static func spawnHelper(
        path: String,
        extraArguments: [String] = [],
        parentFD: Int32,
        childFD: Int32,
        input: (parent: Int32, child: Int32)? = nil
    ) throws -> pid_t {
        var actions: posix_spawn_file_actions_t?
        guard posix_spawn_file_actions_init(&actions) == 0 else { throw VMHostLaunchError.spawnFailed }
        defer { posix_spawn_file_actions_destroy(&actions) }

        let targetFD = childSocketFD
        var temporaries: [Int32] = []
        defer { temporaries.forEach { Darwin.close($0) } }
        var arguments = [path, "--socket-fd", String(targetFD)] + extraArguments
        if let input {
            // Both child ends could sit on 3 or 4 and be overwritten by the first dup2: move them above 9 first. The copies
            // are closed in the child after the dup2 and in this process after the spawn.
            let frameSource = fcntl(childFD, F_DUPFD, 10)
            guard frameSource >= 0 else { throw VMHostLaunchError.spawnFailed }
            temporaries.append(frameSource)
            let inputSource = fcntl(input.child, F_DUPFD, 10)
            guard inputSource >= 0 else { throw VMHostLaunchError.spawnFailed }
            temporaries.append(inputSource)
            guard posix_spawn_file_actions_adddup2(&actions, frameSource, targetFD) == 0,
                  posix_spawn_file_actions_adddup2(&actions, inputSource, childInputFD) == 0 else {
                throw VMHostLaunchError.spawnFailed
            }
            _ = posix_spawn_file_actions_addclose(&actions, frameSource)
            _ = posix_spawn_file_actions_addclose(&actions, inputSource)
            for descriptor in [childFD, parentFD, input.child, input.parent]
            where descriptor != targetFD && descriptor != childInputFD {
                _ = posix_spawn_file_actions_addclose(&actions, descriptor)
            }
            arguments += ["--input-fd", String(childInputFD)]
        } else {
            guard posix_spawn_file_actions_adddup2(&actions, childFD, targetFD) == 0 else {
                throw VMHostLaunchError.spawnFailed
            }
            if childFD != targetFD {
                _ = posix_spawn_file_actions_addclose(&actions, childFD)
            }
            if parentFD != targetFD {
                _ = posix_spawn_file_actions_addclose(&actions, parentFD)
            }
        }

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
        guard status == 0 else { throw VMHostLaunchError.spawnFailed }
        return pid
    }

    /// Waits for the child; returns its exit status, or 128 + signal when it was killed by a signal.
    static func waitForChild(_ pid: pid_t) throws -> Int32 {
        var status: Int32 = 0
        while true {
            let waited = waitpid(pid, &status, 0)
            if waited == pid { break }
            if waited < 0 && errno == EINTR { continue }
            throw VMHostLaunchError.waitFailed
        }
        if status & 0x7F != 0 { return 128 + (status & 0x7F) }
        return (status >> 8) & 0xFF
    }

    static func terminateChild(_ pid: pid_t) {
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
}
