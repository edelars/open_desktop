import Darwin
import Dispatch
import Foundation
#if canImport(ODShared)
import ODShared
#endif
#if canImport(ODAtomics)
import ODAtomics
#endif
#if canImport(ODHostABI)
import ODHostABI
#endif

@main
struct VmHostSmoke {
    private static let maxV1QueuedOutputBytes = 16_416
    private static let v1ReadAndTerminalReserveBytes = 8_192

    static func main() {
        var socketFD: Int32 = -1
        do {
            let args = Array(CommandLine.arguments.dropFirst())
            // Control protocol v2 (S11-04): selected only by the explicit flag; v0, v1 and the display smoke are not touched.
            if args.contains("--control-v2") {
                do {
                    if let options = try ControlV2Options.parse(args) { exit(ControlV2Host.run(options)) }
                } catch {
                    fputs("od-vmhost: \(error)\n", stderr)
                    exit(VMHostExitStatus.usage)
                }
            }
            // S4-41b: the Linux direct-boot machine as a child process of VMHost (`od-runner linux-direct`); explicit flag only.
            if args.first == "--linux-direct" { exit(LinuxDirectCommand.run(args)) }
            // TEST-ONLY display smoke (S10-06): selected only by the explicit flag, never implicitly.
            if let smoke = try DisplaySmokeOptions.parse(args) {
                socketFD = smoke.socketFD
                try DisplaySmoke.run(smoke)
            } else {
                socketFD = try parseSocketFD(args)
                try serveOneFragment(on: socketFD)
            }
        } catch {
            fputs("od-vmhost-smoke: \(error)\n", stderr)
            exit(EXIT_FAILURE)
        }
        if socketFD >= 0 { Darwin.close(socketFD) }
    }

    private static func parseSocketFD(_ args: [String]) throws -> Int32 {
        guard args.count == 2, args[0] == "--socket-fd", let fd = Int32(args[1]), fd >= 0 else {
            throw VmHostError.usage
        }
        return fd
    }

    private static func serveOneFragment(on fd: Int32) throws {
        let hello = try ControlFrame.readInitial(from: fd)
        guard hello.kind == .hello, hello.requestID == 0, hello.payload.isEmpty else {
            throw VmHostError.badHandshake
        }
        if hello.protocolVersion == ControlFrame.versionV1 {
            try serveV1(on: fd)
            return
        }
        var readyPayload = Data()
        readyPayload.appendLE(od_host_abi_version())
        try ControlFrame(kind: .ready, requestID: 0, payload: readyPayload).write(to: fd)

        let run = try ControlFrame.read(from: fd)
        guard run.kind == .run else { throw VmHostError.expectedRun }
        let request = try RunRequest(payload: run.payload)
        var handle: UnsafeMutableRawPointer?
        let createStatus = request.code.withUnsafeBufferPointer { bytes in
            od_fragment_create(bytes.baseAddress, bytes.count, &handle)
        }
        guard createStatus == 0, let handle else {
            try sendStatus(.error, requestID: run.requestID, status: createStatus, fd: fd)
            return
        }
        defer { _ = od_fragment_destroy(handle) }

        let worker = FragmentWorker(handle: handle, state: request.state, budget: request.budget)
        worker.start()
        try sendStatus(.runAck, requestID: run.requestID, status: 0, fd: fd)

        do {
            try serviceStopRequests(fd: fd, requestID: run.requestID, worker: worker, handle: handle)
        } catch {
            _ = od_fragment_request_stop(handle)
            worker.waitUntilFinished()
            throw error
        }

        worker.waitUntilFinished()
        let completion = worker.snapshot()
        guard completion.status == 0 else {
            try sendStatus(.error, requestID: run.requestID, status: completion.status, fd: fd)
            return
        }
        try ControlFrame(kind: .result, requestID: run.requestID, payload: serialize(completion.result))
            .write(to: fd)
    }

    private static func serviceStopRequests(
        fd: Int32,
        requestID: UInt32,
        worker: FragmentWorker,
        handle: UnsafeMutableRawPointer
    ) throws {
        var decoder = ControlFrameDecoder()
        while !worker.isFinished {
            var descriptors = [
                pollfd(fd: fd, events: Int16(POLLIN), revents: 0),
                pollfd(fd: worker.completionFileDescriptor, events: Int16(POLLIN), revents: 0),
            ]
            let ready = Darwin.poll(&descriptors, nfds_t(descriptors.count), -1)
            if ready < 0 {
                if errno == EINTR { continue }
                throw VmHostError.channelFailure
            }
            if descriptors[1].revents & Int16(POLLIN) != 0 {
                return
            }
            if descriptors[0].revents & Int16(POLLERR | POLLNVAL) != 0 {
                throw VmHostError.channelFailure
            }
            if descriptors[0].revents & Int16(POLLHUP) != 0,
               descriptors[0].revents & Int16(POLLIN) == 0 {
                throw VmHostError.channelFailure
            }
            guard descriptors[0].revents & Int16(POLLIN) != 0 else { continue }
            let frames = try readAvailableFrames(from: fd, decoder: &decoder)
            for frame in frames {
                guard frame.kind == .stop, frame.requestID == requestID, frame.payload.isEmpty else {
                    try sendStatus(.error, requestID: frame.requestID, status: 1, fd: fd)
                    continue
                }
                let status = od_fragment_request_stop(handle)
                try sendStatus(.stopAck, requestID: requestID, status: status, fd: fd)
            }
        }
    }

    private static func readAvailableFrames(from fd: Int32, decoder: inout ControlFrameDecoder) throws -> [ControlFrame] {
        var frames: [ControlFrame] = []
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = buffer.withUnsafeMutableBytes { raw in
                Darwin.recv(fd, raw.baseAddress, raw.count, Int32(MSG_DONTWAIT))
            }
            if count > 0 {
                frames.append(contentsOf: try decoder.append(Data(buffer[..<count])))
                continue
            }
            if count == 0 { throw VmHostError.channelFailure }
            if errno == EINTR { continue }
            if errno == EAGAIN || errno == EWOULDBLOCK { return frames }
            throw VmHostError.channelFailure
        }
    }

    private static func sendStatus(_ kind: ControlKind, requestID: UInt32, status: UInt32, fd: Int32) throws {
        var payload = Data()
        payload.appendLE(status)
        try ControlFrame(kind: kind, requestID: requestID, payload: payload).write(to: fd)
    }

    private static func serialize(_ result: od_execution_result_v0) -> Data {
        var payload = Data()
        payload.appendLE(result.state.gpr.0)
        payload.appendLE(result.state.gpr.1)
        payload.appendLE(result.state.gpr.2)
        payload.appendLE(result.state.gpr.3)
        payload.appendLE(result.state.rip)
        payload.appendLE(result.state.rflags)
        payload.appendLE(result.reason)
        payload.appendLE(result.reserved)
        payload.appendLE(result.instructions_executed)
        return payload
    }

    private static func serveV1(on fd: Int32) throws {
        var noSigpipe: Int32 = 1
        guard Darwin.setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigpipe,
                               socklen_t(MemoryLayout<Int32>.size)) == 0 else {
            throw VmHostError.channelFailure
        }
        guard od_session_abi_version() == vmhost_session_abi_version_v1(),
              (od_session_abi_capabilities() & vmhost_session_required_capabilities_v1()) != 0 else {
            throw VmHostError.sessionAbiUnavailable
        }

        var readyPayload = Data()
        readyPayload.appendLE(UInt32(1))
        readyPayload.appendLE(UInt32(0))
        try ControlFrame(kind: .ready, requestID: 0, payload: readyPayload,
                         protocolVersion: ControlFrame.versionV1).write(to: fd)

        // Keep polling run completion even while the peer is not reading.
        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0 else {
            throw VmHostError.channelFailure
        }

        var outgoing = Data()
        var session: od_session_handle_v1?
        var boundCode: [UInt8]?
        var highWatermark: UInt32 = 0
        var active: ActiveSessionRun?
        var closeAfterTerminal = false
        defer {
            if let active, !active.terminalQueued, let session {
                _ = od_session_request_stop_v1(session, active.requestID)
                var result = od_session_run_result_v1()
                result.struct_size = UInt32(MemoryLayout<od_session_run_result_v1>.size)
                result.request_id = active.requestID
                _ = od_session_wait_run_v1(
                    session, active.requestID, &result,
                    UInt32(MemoryLayout<od_session_run_result_v1>.size)
                )
                if active.completionFD >= 0 { Darwin.close(active.completionFD) }
            } else if let active, active.completionFD >= 0 {
                Darwin.close(active.completionFD)
            }
            if let session { _ = od_session_destroy_v1(session) }
        }

        var decoder = ControlFrameDecoder(expectedVersion: ControlFrame.versionV1)
        var socketEOF = false
        while !socketEOF {
            var socketEvents: Int16 = 0
            if !closeAfterTerminal && outgoing.count < Self.maxV1QueuedOutputBytes - Self.v1ReadAndTerminalReserveBytes {
                socketEvents |= Int16(POLLIN)
            }
            if !outgoing.isEmpty { socketEvents |= Int16(POLLOUT) }
            var descriptors = [pollfd(fd: fd, events: socketEvents, revents: 0)]
            var completionIndex: Int?
            if let active, active.completionFD >= 0 {
                completionIndex = descriptors.count
                descriptors.append(pollfd(fd: active.completionFD, events: Int16(POLLIN), revents: 0))
            }
            let ready = Darwin.poll(&descriptors, nfds_t(descriptors.count), -1)
            if ready < 0 {
                if errno == EINTR { continue }
                throw VmHostError.channelFailure
            }

            var frames: [ControlFrame] = []
            if descriptors[0].revents & Int16(POLLERR | POLLNVAL) != 0 {
                throw VmHostError.channelFailure
            }
            if descriptors[0].revents & Int16(POLLHUP) != 0,
               descriptors[0].revents & Int16(POLLIN) == 0 {
                socketEOF = true
            }
            if descriptors[0].revents & Int16(POLLIN | POLLHUP) != 0 {
                let received = try readAvailableV1Frames(from: fd, decoder: &decoder)
                frames = received.frames
                socketEOF = received.eof
                if socketEOF && !decoder.isEmpty { throw VmHostError.badV1Frame }
            }

            if socketEOF { break }
            frameLoop: for frame in frames {
                switch frame.kind {
                case .run:
                    let request = try SessionRunRequest(frame: frame)
                    let idStatus: UInt32
                    if frame.requestID == 0 || frame.requestID <= highWatermark {
                        idStatus = UInt32(OD_SESSION_STATUS_INVALID_REQUEST_ID_V1)
                    } else {
                        highWatermark = frame.requestID
                        idStatus = 0
                    }
                    if idStatus != 0 {
                        try queueV1Error(requestID: frame.requestID, status: idStatus, scope: 1,
                                         outgoing: &outgoing)
                        continue
                    }
                    if active != nil {
                        try queueV1Error(requestID: frame.requestID,
                                         status: UInt32(OD_SESSION_STATUS_BUSY_V1), scope: 1,
                                         outgoing: &outgoing)
                        continue
                    }
                    if let boundCode, boundCode != request.code {
                        try queueV1Error(requestID: frame.requestID,
                                         status: UInt32(OD_SESSION_STATUS_CODE_MISMATCH_V1), scope: 1,
                                         outgoing: &outgoing)
                        continue
                    }

                    var createdSession = session
                    if createdSession == nil {
                        var options = makeCreateOptions(code: request.code, firstEntryRIP: request.state.rip)
                        var output: od_session_handle_v1?
                        let status = od_session_create_v1(
                            &options, UInt32(MemoryLayout<od_session_create_options_v1>.size), &output
                        )
                        if status != 0 {
                            let mapped = mapSessionStatus(status)
                            try queueV1Error(requestID: frame.requestID, status: mapped, scope: 1,
                                             outgoing: &outgoing)
                            if mapped == UInt32(OD_SESSION_STATUS_INVALID_STATE_V1) {
                                closeAfterTerminal = true
                                break frameLoop
                            }
                            continue
                        }
                        guard let output else { throw VmHostError.sessionAbiFailure }
                        createdSession = output
                    }

                    guard let runSession = createdSession else { throw VmHostError.sessionAbiFailure }
                    var abiRequest = makeSessionRequest(request)
                    var token = od_session_run_token_v1()
                    token.struct_size = UInt32(MemoryLayout<od_session_run_token_v1>.size)
                    token.request_id = frame.requestID
                    token.completion_read_fd = -1
                    token.reserved = 0
                    let beginStatus = od_session_begin_run_v1(
                        runSession, &abiRequest,
                        UInt32(MemoryLayout<od_session_run_request_v1>.size), &token,
                        UInt32(MemoryLayout<od_session_run_token_v1>.size)
                    )
                    guard beginStatus == 0 else {
                        let mapped = mapSessionStatus(beginStatus)
                        if session == nil, od_session_destroy_v1(runSession) != 0 {
                            throw VmHostError.sessionAbiFailure
                        }
                        try queueV1Error(requestID: frame.requestID, status: mapped, scope: 1,
                                         outgoing: &outgoing)
                        if mapped == UInt32(OD_SESSION_STATUS_INVALID_STATE_V1) {
                            closeAfterTerminal = true
                            break frameLoop
                        }
                        continue
                    }
                    if session == nil {
                        session = runSession
                        boundCode = request.code
                    }
                    active = ActiveSessionRun(requestID: frame.requestID,
                                              completionFD: token.completion_read_fd)
                    guard token.request_id == frame.requestID, token.completion_read_fd >= 0 else {
                        throw VmHostError.sessionAbiFailure
                    }
                    try queueV1Status(.runAck, requestID: frame.requestID, status: 0,
                                      outgoing: &outgoing)

                case .stop:
                    guard frame.requestID != 0, frame.payload.isEmpty else { throw VmHostError.badV1Frame }
                    let status: UInt32
                    if let session {
                        status = od_session_request_stop_v1(session, frame.requestID)
                    } else {
                        status = UInt32(OD_SESSION_STATUS_NO_ACTIVE_RUN_V1)
                    }
                    try queueV1Status(.stopAck, requestID: frame.requestID,
                                      status: mapSessionStatus(status), outgoing: &outgoing)

                default:
                    throw VmHostError.badV1Frame
                }
                if closeAfterTerminal { break }
            }

            if let current = active, let completionIndex,
               descriptors[completionIndex].revents & Int16(POLLIN | POLLHUP | POLLERR) != 0 {
                let notificationOK = consumeCompletionNotification(current.completionFD)
                guard let session else { throw VmHostError.sessionAbiFailure }
                if !notificationOK { _ = od_session_request_stop_v1(session, current.requestID) }
                var result = od_session_run_result_v1()
                result.struct_size = UInt32(MemoryLayout<od_session_run_result_v1>.size)
                result.request_id = current.requestID
                let status = od_session_wait_run_v1(
                    session, current.requestID, &result,
                    UInt32(MemoryLayout<od_session_run_result_v1>.size)
                )
                Darwin.close(current.completionFD)
                active = ActiveSessionRun(requestID: current.requestID, completionFD: -1, terminalQueued: true)
                guard notificationOK else { throw VmHostError.sessionAbiFailure }
                if status == 0 {
                    guard result.request_id == current.requestID, result.reserved == 0 else {
                        throw VmHostError.sessionAbiFailure
                    }
                    try queueV1Frame(ControlFrame(kind: .result, requestID: current.requestID,
                                                  payload: serialize(result),
                                                  protocolVersion: ControlFrame.versionV1), outgoing: &outgoing)
                } else {
                    let mapped = mapSessionStatus(status)
                    guard (1...6).contains(mapped) else { throw VmHostError.sessionAbiFailure }
                    try queueV1Error(requestID: current.requestID, status: mapped, scope: 2,
                                     outgoing: &outgoing)
                    closeAfterTerminal = true
                }
            }

            try flushV1Output(fd: fd, outgoing: &outgoing)
            if outgoing.isEmpty, active?.terminalQueued == true { active = nil }
            if closeAfterTerminal && outgoing.isEmpty { break }
        }
    }

    private static func makeCreateOptions(code: [UInt8], firstEntryRIP: UInt64) -> od_session_create_options_v1 {
        var options = od_session_create_options_v1()
        options.struct_size = UInt32(MemoryLayout<od_session_create_options_v1>.size)
        options.code_length = UInt32(code.count)
        options.first_entry_rip = firstEntryRIP
        let offset = MemoryLayout<od_session_create_options_v1>.offset(of: \.code)!
        withUnsafeMutableBytes(of: &options) { raw in
            if !code.isEmpty {
                code.withUnsafeBytes { source in
                    raw.baseAddress!.advanced(by: offset).copyMemory(from: source.baseAddress!, byteCount: code.count)
                }
            }
        }
        return options
    }

    private static func makeSessionRequest(_ request: SessionRunRequest) -> od_session_run_request_v1 {
        var value = od_session_run_request_v1()
        value.struct_size = UInt32(MemoryLayout<od_session_run_request_v1>.size)
        value.request_id = request.requestID
        value.instruction_budget = request.budget
        value.initial_state = od_session_cpu_state_v1(
            gpr: (request.state.gpr.0, request.state.gpr.1, request.state.gpr.2, request.state.gpr.3),
            rip: request.state.rip,
            rflags: request.state.rflags
        )
        value.code_length = UInt32(request.code.count)
        value.reserved = 0
        let offset = MemoryLayout<od_session_run_request_v1>.offset(of: \.code)!
        withUnsafeMutableBytes(of: &value) { raw in
            if !request.code.isEmpty {
                request.code.withUnsafeBytes { source in
                    raw.baseAddress!.advanced(by: offset).copyMemory(
                        from: source.baseAddress!, byteCount: request.code.count
                    )
                }
            }
        }
        return value
    }

    private static func mapSessionStatus(_ status: UInt32) -> UInt32 {
        switch status {
        case 0,
             UInt32(OD_SESSION_STATUS_INVALID_ARGUMENT_V1),
             UInt32(OD_SESSION_STATUS_INVALID_CODE_V1),
             UInt32(OD_SESSION_STATUS_BUSY_V1),
             UInt32(OD_SESSION_STATUS_UNSUPPORTED_HOST_V1),
             UInt32(OD_SESSION_STATUS_ALLOCATION_FAILED_V1),
             UInt32(OD_SESSION_STATUS_INVALID_STATE_V1),
             UInt32(OD_SESSION_STATUS_INVALID_REQUEST_ID_V1),
             UInt32(OD_SESSION_STATUS_CODE_MISMATCH_V1),
             UInt32(OD_SESSION_STATUS_NO_ACTIVE_RUN_V1): return status
        default: return UInt32(OD_SESSION_STATUS_INVALID_STATE_V1)
        }
    }

    private static func queueV1Status(
        _ kind: ControlKind,
        requestID: UInt32,
        status: UInt32,
        outgoing: inout Data
    ) throws {
        var payload = Data()
        payload.appendLE(status)
        try queueV1Frame(ControlFrame(kind: kind, requestID: requestID, payload: payload,
                                      protocolVersion: ControlFrame.versionV1), outgoing: &outgoing)
    }

    private static func queueV1Error(
        requestID: UInt32,
        status: UInt32,
        scope: UInt32,
        outgoing: inout Data
    ) throws {
        var payload = Data()
        payload.appendLE(status)
        payload.appendLE(scope)
        payload.appendLE(UInt32(ControlKind.run.rawValue))
        try queueV1Frame(ControlFrame(kind: .error, requestID: requestID, payload: payload,
                                      protocolVersion: ControlFrame.versionV1), outgoing: &outgoing)
    }

    private static func queueV1Frame(_ frame: ControlFrame, outgoing: inout Data) throws {
        let encoded = try frame.encoded()
        guard encoded.count <= Self.maxV1QueuedOutputBytes - outgoing.count else {
            throw VmHostError.channelFailure
        }
        outgoing.append(encoded)
    }

    private static func flushV1Output(fd: Int32, outgoing: inout Data) throws {
        while !outgoing.isEmpty {
            let count = outgoing.withUnsafeBytes { raw in
                Darwin.send(fd, raw.baseAddress, raw.count, Int32(MSG_DONTWAIT))
            }
            if count > 0 {
                outgoing.removeSubrange(0..<count)
                continue
            }
            if count < 0 && errno == EINTR { continue }
            if count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { return }
            throw VmHostError.channelFailure
        }
    }

    private static func readAvailableV1Frames(
        from fd: Int32,
        decoder: inout ControlFrameDecoder
    ) throws -> (frames: [ControlFrame], eof: Bool) {
        var frames: [ControlFrame] = []
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = buffer.withUnsafeMutableBytes { raw in
                Darwin.recv(fd, raw.baseAddress, raw.count, Int32(MSG_DONTWAIT))
            }
            if count > 0 {
                frames = try decoder.append(Data(buffer[..<count]))
                return (frames, false)
            }
            if count == 0 { return (frames, true) }
            if errno == EINTR { continue }
            if errno == EAGAIN || errno == EWOULDBLOCK { return (frames, false) }
            throw VmHostError.channelFailure
        }
    }

    private static func consumeCompletionNotification(_ fd: Int32) -> Bool {
        var byte: UInt8 = 0
        while true {
            let count = Darwin.read(fd, &byte, 1)
            if count == 1 { break }
            if count < 0 && errno == EINTR { continue }
            return false
        }
        guard byte == UInt8(OD_SESSION_COMPLETION_BYTE_V1) else { return false }
        var extra: UInt8 = 0
        while true {
            let count = Darwin.read(fd, &extra, 1)
            if count == 0 { return true }
            if count < 0 && errno == EINTR { continue }
            return false
        }
    }

    private static func serialize(_ result: od_session_run_result_v1) -> Data {
        var payload = Data()
        payload.appendLE(result.state.gpr.0)
        payload.appendLE(result.state.gpr.1)
        payload.appendLE(result.state.gpr.2)
        payload.appendLE(result.state.gpr.3)
        payload.appendLE(result.state.rip)
        payload.appendLE(result.state.rflags)
        payload.appendLE(result.reason)
        payload.appendLE(result.reserved)
        payload.appendLE(result.instructions_executed)
        return payload
    }
}

private struct ActiveSessionRun {
    let requestID: UInt32
    let completionFD: Int32
    var terminalQueued = false
}

private struct SessionRunRequest {
    let requestID: UInt32
    let code: [UInt8]
    let state: od_cpu_state_v0
    let budget: UInt64

    init(frame: ControlFrame) throws {
        let bytes = [UInt8](frame.payload)
        guard frame.requestID != 0, bytes.count >= 60 else { throw VmHostError.badV1Frame }
        let codeLength = Int(bytes.readLE(UInt32.self, at: 0))
        guard codeLength <= Int(OD_SESSION_MAX_CODE_BYTES_V1), bytes.count == 60 + codeLength else {
            throw VmHostError.badV1Frame
        }
        requestID = frame.requestID
        budget = bytes.readLE(UInt64.self, at: 4)
        state = od_cpu_state_v0(
            gpr: (bytes.readLE(UInt64.self, at: 12), bytes.readLE(UInt64.self, at: 20),
                  bytes.readLE(UInt64.self, at: 28), bytes.readLE(UInt64.self, at: 36)),
            rip: bytes.readLE(UInt64.self, at: 44),
            rflags: bytes.readLE(UInt64.self, at: 52)
        )
        code = Array(bytes[60...])
    }
}

private struct RunRequest {
    let code: [UInt8]
    let state: od_cpu_state_v0
    let budget: UInt64

    init(payload: Data) throws {
        let bytes = [UInt8](payload)
        guard bytes.count >= 60 else { throw VmHostError.invalidRun }
        let codeLength = Int(bytes.readLE(UInt32.self, at: 0))
        guard codeLength <= Int(OD_FRAGMENT_MAX_LENGTH_V0), bytes.count == 60 + codeLength else {
            throw VmHostError.invalidRun
        }
        budget = bytes.readLE(UInt64.self, at: 4)
        state = od_cpu_state_v0(
            gpr: (
                bytes.readLE(UInt64.self, at: 12),
                bytes.readLE(UInt64.self, at: 20),
                bytes.readLE(UInt64.self, at: 28),
                bytes.readLE(UInt64.self, at: 36)
            ),
            rip: bytes.readLE(UInt64.self, at: 44),
            rflags: bytes.readLE(UInt64.self, at: 52)
        )
        code = Array(bytes[60...])
    }
}

private final class FragmentWorker: @unchecked Sendable {
    private let handle: UnsafeMutableRawPointer
    private let state: od_cpu_state_v0
    private let budget: UInt64
    private let lock = NSLock()
    private let finished = DispatchSemaphore(value: 0)
    private let completionSignal = Pipe()
    private var done = false
    private var status: UInt32 = 0
    private var result = od_execution_result_v0(
        state: od_cpu_state_v0(gpr: (0, 0, 0, 0), rip: 0, rflags: 2),
        reason: 0,
        reserved: 0,
        instructions_executed: 0
    )

    init(handle: UnsafeMutableRawPointer, state: od_cpu_state_v0, budget: UInt64) {
        self.handle = handle
        self.state = state
        self.budget = budget
    }

    var isFinished: Bool {
        lock.lock()
        defer { lock.unlock() }
        return done
    }

    var completionFileDescriptor: Int32 {
        Int32(completionSignal.fileHandleForReading.fileDescriptor)
    }

    func start() {
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            var initial = state
            var output = result
            let executionStatus = withUnsafePointer(to: &initial) { initialPointer in
                od_fragment_execute(handle, initialPointer, budget, &output)
            }
            lock.lock()
            status = executionStatus
            result = output
            done = true
            lock.unlock()
            finished.signal()
            var byte: UInt8 = 1
            _ = withUnsafePointer(to: &byte) {
                Darwin.write(completionSignal.fileHandleForWriting.fileDescriptor, $0, 1)
            }
        }
    }

    func waitUntilFinished() {
        finished.wait()
        finished.signal()
    }

    func snapshot() -> (status: UInt32, result: od_execution_result_v0) {
        lock.lock()
        defer { lock.unlock() }
        return (status, result)
    }
}

private enum VmHostError: Error, CustomStringConvertible {
    case usage
    case badHandshake
    case badV1Frame
    case expectedRun
    case invalidRun
    case channelFailure
    case sessionAbiUnavailable
    case sessionAbiFailure

    var description: String {
        switch self {
        case .usage: return "usage: od-vmhost-smoke --socket-fd FD [--display-smoke pattern|probe [--input-fd FD [--input-trace 1]]]"
        case .badHandshake: return "invalid launcher handshake"
        case .badV1Frame: return "invalid control v1 frame"
        case .expectedRun: return "expected RUN request"
        case .invalidRun: return "invalid RUN payload"
        case .channelFailure: return "control channel poll failed"
        case .sessionAbiUnavailable: return "control v1 session ABI is unavailable"
        case .sessionAbiFailure: return "control v1 session ABI invariant failed"
        }
    }
}
