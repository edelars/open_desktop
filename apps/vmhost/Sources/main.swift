import Darwin
import Dispatch
import Foundation

@main
struct VmHostSmoke {
    static func main() {
        var socketFD: Int32 = -1
        do {
            socketFD = try parseSocketFD(Array(CommandLine.arguments.dropFirst()))
            try serveOneFragment(on: socketFD)
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
        let hello = try ControlFrame.read(from: fd)
        guard hello.kind == .hello, hello.requestID == 0, hello.payload.isEmpty else {
            throw VmHostError.badHandshake
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
    case expectedRun
    case invalidRun
    case channelFailure

    var description: String {
        switch self {
        case .usage: return "usage: od-vmhost-smoke --socket-fd FD"
        case .badHandshake: return "invalid launcher handshake"
        case .expectedRun: return "expected RUN request"
        case .invalidRun: return "invalid RUN payload"
        case .channelFailure: return "control channel poll failed"
        }
    }
}
