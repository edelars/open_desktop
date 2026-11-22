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

// `--control-v2` mode of VMHost (S11-04): the I/O of control protocol v2 around `Lifecycle` (protocols/control/CONTROL_PROTOCOL_V2.md).
//
// One thread, one `poll(2)` loop over the control socket (fd 3), the optional frame socket (`--display-fd`, FRAME_TRANSPORT_V1.md)
// and the optional input channel (`--input-fd`). No read of the control socket blocks the polling of the profile session: the
// loop waits for readiness with a timeout equal to the next polling tick. The machine is a test-only profile without a guest:
// "running" means that the display session is stepped and its frames are published; the counter of polls in `STATS` is the
// observable sign of it.

struct ControlV2UsageError: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}

/// Command line of the v2 mode: `--control-v2 --socket-fd N --bundle PATH [--display-fd N] [--input-fd N]`.
struct ControlV2Options {
    let socketFD: Int32
    let bundlePath: String
    let displayFD: Int32?
    let inputFD: Int32?

    static let usage = "usage: od-vmhost --control-v2 --socket-fd FD --bundle PATH [--display-fd FD] [--input-fd FD]"

    /// Returns nil when `--control-v2` is absent. Any other problem is a usage error (exit status 2, no frames).
    static func parse(_ args: [String]) throws -> ControlV2Options? {
        guard args.contains("--control-v2") else { return nil }
        var socketFD: Int32?
        var bundle: String?
        var displayFD: Int32?
        var inputFD: Int32?
        var sawFlag = false
        var index = 0
        while index < args.count {
            let name = args[index]
            if name == "--control-v2" {
                guard !sawFlag else { throw ControlV2UsageError(message: usage) }
                sawFlag = true
                index += 1
                continue
            }
            guard index + 1 < args.count else { throw ControlV2UsageError(message: usage) }
            let value = args[index + 1]
            switch name {
            case "--socket-fd": socketFD = try descriptor(value)
            case "--display-fd": displayFD = try descriptor(value)
            case "--input-fd": inputFD = try descriptor(value)
            case "--bundle": bundle = value
            default: throw ControlV2UsageError(message: usage)
            }
            index += 2
        }
        guard let socketFD, let bundle else { throw ControlV2UsageError(message: usage) }
        let descriptors = [socketFD] + [displayFD, inputFD].compactMap { $0 }
        guard Set(descriptors).count == descriptors.count else {
            throw ControlV2UsageError(message: "descriptors of control, display and input must differ")
        }
        for descriptor in descriptors where fcntl(descriptor, F_GETFD) < 0 {
            throw ControlV2UsageError(message: "descriptor \(descriptor) is not open")
        }
        try validateBundlePath(bundle)
        return ControlV2Options(socketFD: socketFD, bundlePath: bundle, displayFD: displayFD, inputFD: inputFD)
    }

    private static func descriptor(_ text: String) throws -> Int32 {
        guard let value = Int32(text), value >= 0 else { throw ControlV2UsageError(message: usage) }
        return value
    }

    /// VM_BUNDLE 7 item 1: absolute path, UTF-8 without NUL, at most 1023 bytes, an existing directory.
    private static func validateBundlePath(_ path: String) throws {
        let bytes = Array(path.utf8)
        guard path.hasPrefix("/"), bytes.count <= 1023, !bytes.contains(0) else {
            throw ControlV2UsageError(message: "--bundle must be an absolute path of at most 1023 bytes")
        }
        var info = stat()
        guard stat(path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else {
            throw ControlV2UsageError(message: "--bundle is not an existing directory")
        }
    }
}

// MARK: profile runner

/// A display session of a test-only profile (1 or 4), stepped by `pollOnce` and published through the frame transport writer
/// (FrameTransport.swift as it is). Without `--display-fd` the session is stepped and counted, and nothing is published.
final class DisplayProfileRunner: ProfileRunner {
    private enum Link { case none, awaitingHello, streaming, closed }

    private let displayFD: Int32?
    private var link: Link
    private var helloBuffer: [UInt8] = []
    private var session: OpaquePointer?
    private var writers: [FrameTransportWriter] = []
    private(set) var isPolling = false
    private(set) var pollCount: UInt64 = 0
    private(set) var framesPublished: UInt64 = 0

    init(displayFD: Int32?) throws {
        self.displayFD = displayFD
        link = displayFD == nil ? .none : .awaitingHello
        if let displayFD {
            // The frame socket is serviced without blocking, like the control socket.
            var one: Int32 = 1
            _ = setsockopt(displayFD, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
            let flags = fcntl(displayFD, F_GETFL)
            guard flags >= 0, fcntl(displayFD, F_SETFL, flags | O_NONBLOCK) == 0,
                  fcntl(displayFD, F_SETFD, FD_CLOEXEC) == 0 else {
                throw FrameTransportError.channel("display socket flags errno \(errno)")
            }
        }
    }

    // MARK: ProfileRunner

    func start(profile: UInt32, screens: Int) throws {
        var config = od_display_session_config()
        config.abi_version = vmhost_display_abi_version_v1()
        config.struct_size = UInt32(OD_DISPLAY_SESSION_CONFIG_SIZE_V1)
        config.profile = profile == OD_DISPLAY_PROFILE_TWO_SCREENS_TEST.rawValue
            ? OD_DISPLAY_PROFILE_TWO_SCREENS_TEST.rawValue : OD_DISPLAY_PROFILE_TEST_PATTERN.rawValue
        var created: OpaquePointer?
        let status = od_display_session_create(&config, &created)
        guard status == OD_DISPLAY_STATUS_OK.rawValue, let created else {
            throw FrameTransportError.displayABI("od_display_session_create", status)
        }
        var built: [FrameTransportWriter] = []
        if displayFD != nil {
            do {
                // Section 11.3 of the frame transport: one object per screen, all created before the first ANNOUNCE.
                // Screen 0 of a test-only profile always has a mode; a screen without one (screen 1) is not a refusal: its object
                // gets the device limits and its window shows "no signal" (DISPLAY_DEVICE.md 16.2).
                guard try DisplaySmoke.screenMode(created, screen: 0).flags & vmhost_display_mode_flag_active() != 0 else {
                    throw FrameTransportError.hostRefusal("test pattern profile has no active mode")
                }
                built = try DisplaySmoke.makeWriters(session: created, screens: screens, queue: NotificationQueue())
            } catch {
                _ = od_display_session_destroy(created)
                throw error
            }
        }
        session = created
        writers = built
        pollCount = 0
        framesPublished = 0
        isPolling = true
    }

    func pause() { isPolling = false }

    func resume(reason: ControlV2Reason) {
        // S11-22: the guest clock is corrected here by `reason` (TIME_SYNC). No guest clock exists on these profiles.
        isPolling = true
    }

    func stop() throws {
        isPolling = false
        writers = []
        closeLink()
        guard let session else { return }
        self.session = nil
        let status = od_display_session_destroy(session)
        guard status == OD_DISPLAY_STATUS_OK.rawValue else { throw FrameTransportError.displayABI("od_display_session_destroy", status) }
    }

    /// Teardown after an error or EOF: destroys the session without reporting.
    func teardown() {
        isPolling = false
        writers = []
        closeLink()
        if let session {
            self.session = nil
            _ = od_display_session_destroy(session)
        }
    }

    // MARK: polling

    /// One quantum of the profile and, when a reader is connected, the publication of what changed (the body of the loop of
    /// `--display-smoke`). A throw is a failure of the host ABI; a failure of the frame socket only closes the frame link.
    func pollOnce() throws {
        guard isPolling, let session else { return }
        var report = od_display_step_report()
        report.abi_version = vmhost_display_abi_version_v1()
        report.struct_size = UInt32(OD_DISPLAY_STEP_REPORT_SIZE_V1)
        let status = od_display_session_step(session, 1_000_000, &report)
        guard status == OD_DISPLAY_STATUS_OK.rawValue else { throw FrameTransportError.displayABI("od_display_session_step", status) }
        pollCount += 1
        guard link == .streaming, let displayFD else { return }
        do {
            for (screen, current) in writers.enumerated() {
                let mode = try DisplaySmoke.screenMode(session, screen: screen)
                if mode.mode_change_count != current.queuedMode {
                    try current.queueMode(count: mode.mode_change_count, active: mode.flags & 1 != 0,
                                          disabled: mode.flags & 4 != 0, width: mode.width, height: mode.height,
                                          stride: mode.stride, format: mode.format)
                }
            }
            for (screen, current) in writers.enumerated() {
                let mode = try DisplaySmoke.screenMode(session, screen: screen)
                guard mode.flags & 1 != 0, mode.flags & 2 != 0, mode.generation > current.lastPublishedGeneration else { continue }
                let plain = od_display_mode(abi_version: vmhost_display_abi_version_v1(),
                                            struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1), width: mode.width,
                                            height: mode.height, stride: mode.stride, format: mode.format,
                                            flags: mode.flags, reserved: 0, generation: mode.generation,
                                            mode_change_count: mode.mode_change_count)
                if try current.publishFrame(session: session, mode: plain) { framesPublished += 1 }
            }
            if let cursor = try DisplaySmoke.cursorSnapshot(session) {
                _ = try writers[0].publishCursor(cursor, readImage: { destination, length in
                    try DisplaySmoke.readCursorImage(session, into: destination, length: length)
                })
            }
            try writers[0].flush(fd: displayFD)
        } catch let error as FrameTransportError {
            switch error {
            case .displayABI: throw error
            default:
                fputs("od-vmhost: control-v2: frame link closed: \(error)\n", stderr)
                closeLink()
            }
        }
    }

    // MARK: frame link

    /// `poll` events for the frame socket. Before START nothing is read: a `HELLO` that arrives early waits in the socket.
    var displayPollEvents: Int16 {
        guard let _ = displayFD, session != nil else { return 0 }
        switch link {
        case .none, .closed: return 0
        case .awaitingHello: return Int16(POLLIN)
        case .streaming:
            return Int16(POLLIN) | (writers.first?.hasPendingOutput == true ? Int16(POLLOUT) : 0)
        }
    }

    var displayDescriptor: Int32? { link == .none || link == .closed ? nil : displayFD }

    func serviceDisplay(revents: Int16) {
        guard let displayFD, revents != 0 else { return }
        switch link {
        case .none, .closed: return
        case .awaitingHello:
            readHello(fd: displayFD)
        case .streaming:
            if revents & Int16(POLLIN) != 0 {
                var byte: UInt8 = 0
                let count = Darwin.recv(displayFD, &byte, 1, Int32(MSG_DONTWAIT))
                if count > 0 {
                    fputs("od-vmhost: control-v2: frame link closed: bytes from the reader after HELLO\n", stderr)
                    closeLink()
                    return
                }
                if count == 0 { return closeLink() }
                if errno != EAGAIN, errno != EWOULDBLOCK, errno != EINTR { return closeLink() }
            } else if revents & Int16(POLLERR | POLLNVAL | POLLHUP) != 0 {
                return closeLink()
            }
            if revents & Int16(POLLOUT) != 0 {
                do { try writers.first?.flush(fd: displayFD) } catch { closeLink() }
            }
        }
    }

    /// FRAME_TRANSPORT_V1.md 6.1 without blocking: 16 bytes of HELLO, then ANNOUNCE of every screen and the first MODE of each.
    private func readHello(fd: Int32) {
        var buffer = [UInt8](repeating: 0, count: 16 - helloBuffer.count)
        let count = buffer.withUnsafeMutableBytes { Darwin.recv(fd, $0.baseAddress, $0.count, Int32(MSG_DONTWAIT)) }
        if count == 0 { return closeLink() }
        if count < 0 {
            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
            return closeLink()
        }
        helloBuffer.append(contentsOf: buffer[..<count])
        guard helloBuffer.count == 16 else { return }
        let hello = helloBuffer
        helloBuffer = []
        guard Array(hello[0..<4]) == Array("ODFT".utf8) else { return closeLink() }
        // Mirrors `Message.version` of FrameTransport.swift (3): a HELLO of another version gets a VERSION reply.
        if hello.readLE(UInt16.self, at: 4) != 3 {
            var reply = Array("ODFT".utf8)
            reply += [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
            _ = reply.withUnsafeBytes { Darwin.send(fd, $0.baseAddress, $0.count, 0) }
            return closeLink()
        }
        guard hello.readLE(UInt16.self, at: 6) == 1, hello.readLE(UInt32.self, at: 8) == 0,
              hello.readLE(UInt32.self, at: 12) == 0, let session else { return closeLink() }
        do {
            for announced in writers { try announced.sendAnnounce(on: fd, screenCount: writers.count) }
            for (screen, writer) in writers.enumerated() {
                let mode = try DisplaySmoke.screenMode(session, screen: screen)
                try writer.queueMode(count: mode.mode_change_count, active: mode.flags & 1 != 0,
                                     disabled: mode.flags & 4 != 0, width: mode.width, height: mode.height,
                                     stride: mode.stride, format: mode.format)
            }
            link = .streaming
            try writers.first?.flush(fd: fd)
        } catch {
            fputs("od-vmhost: control-v2: frame link closed: \(error)\n", stderr)
            closeLink()
        }
    }

    private func closeLink() {
        if let displayFD, link != .closed, link != .none {
            Darwin.close(displayFD)
        }
        if link != .none { link = .closed }
    }

    /// The mode of screen 0 for the input channel (INPUT_TRANSPORT_V1.md section 8); nil without a session or a signal.
    func currentInputMode() -> InputDisplayMode? {
        guard let session, let mode = try? DisplaySmoke.screenMode(session, screen: 0), mode.flags & 1 != 0 else { return nil }
        return InputDisplayMode(modeChangeCount: mode.mode_change_count, width: mode.width, height: mode.height)
    }
}

// MARK: connection

private func monotonicNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// `hw.memsize`.
private func physicalMemoryBytes() -> UInt64 {
    var value: UInt64 = 0
    var size = MemoryLayout<UInt64>.size
    guard sysctlbyname("hw.memsize", &value, &size, nil, 0) == 0 else { return 0 }
    return value
}

/// Resident set size of this process (`mach_task_basic_info.resident_size`, the current value, not the peak).
func residentSetSizeBytes() -> UInt64 {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
    let result = withUnsafeMutablePointer(to: &info) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    return result == KERN_SUCCESS ? UInt64(info.resident_size) : 0
}

enum ControlV2Host {
    /// Runs the mode to its end and returns the exit status of the process (section 12.3).
    static func run(_ options: ControlV2Options) -> Int32 {
        signal(SIGPIPE, SIG_IGN)
        let socketFD = options.socketFD
        var one: Int32 = 1
        _ = setsockopt(socketFD, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        let flags = fcntl(socketFD, F_GETFL)
        guard flags >= 0, fcntl(socketFD, F_SETFL, flags | O_NONBLOCK) == 0, fcntl(socketFD, F_SETFD, FD_CLOEXEC) == 0 else {
            fputs("od-vmhost: control-v2: control socket flags errno \(errno)\n", stderr)
            return VMHostExitStatus.usage
        }
        let runner: DisplayProfileRunner
        let input: InputHostChannel?
        do {
            runner = try DisplayProfileRunner(displayFD: options.displayFD)
            input = try options.inputFD.map { try InputHostChannel(fd: $0, trace: false) }
        } catch {
            fputs("od-vmhost: control-v2: \(error)\n", stderr)
            return VMHostExitStatus.usage
        }

        // VM_BUNDLE 2.1: the lock is taken at process start, before HELLO; the outcome is reported instead of READY.
        var lock: BundleLock?
        var lockFailure: BundleLockFailure?
        do {
            lock = try BundleLock.acquire(bundlePath: options.bundlePath)
        } catch let failure as BundleLockFailure {
            lockFailure = failure
        } catch {
            lockFailure = .io(EIO)
        }
        let lifecycle = Lifecycle(runner: runner, lockFailure: lockFailure, hostMemoryBytes: physicalMemoryBytes(),
                              bundlePath: options.bundlePath)
        let connection = ControlV2Connection(fd: socketFD, lifecycle: lifecycle, runner: runner, input: input)
        let status = connection.run()
        runner.teardown()
        lifecycle.closeAttachedDescriptors()
        lock?.release()
        return status
    }
}

private final class ControlV2Connection {
    /// Room kept free in the output queue before the next request is taken: the largest answer of one request is a
    /// STATE_CHANGED and a fatal ERROR (at most 28 + 304 bytes).
    private static let responseReserve = 1_024
    private static let pollTickNs: UInt64 = 1_000_000_000 / 60
    private static let statsIntervalNs = UInt64(ControlV2.statsIntervalMs) * 1_000_000
    /// `output_drain_timeout` (section 12.2).
    private static let drainTimeoutNs: UInt64 = 5_000_000_000

    private let fd: Int32
    private let lifecycle: Lifecycle
    private let runner: DisplayProfileRunner
    private let input: InputHostChannel?
    private var decoder = ControlV2Decoder(direction: .towardHost)
    private var outgoing: [UInt8] = []
    /// The 1000 ms of STATS has run out; the frame itself is built when it is put in the output queue (`flushOutput`).
    private var statsDue = false
    private var exitStatus: Int32?
    private var endOfStream = false
    private var nextTickNs: UInt64 = 0
    private var nextStatsNs: UInt64?
    private var drainStalledSinceNs: UInt64?

    init(fd: Int32, lifecycle: Lifecycle, runner: DisplayProfileRunner, input: InputHostChannel?) {
        self.fd = fd
        self.lifecycle = lifecycle
        self.runner = runner
        self.input = input
    }

    func run() -> Int32 {
        while true {
            let now = monotonicNs()
            if let status = processFrames() { return finish(status) }
            if let status = tickProfile(now: now) { return finish(status) }
            scheduleStats(now: now)
            if let status = flushOutput(now: now) { return finish(status) }
            if endOfStream, !decoder.hasPendingFrames, exitStatus == nil {
                do {
                    try decoder.endOfStream()
                    exitStatus = lifecycle.statusOnEndOfStream()
                } catch {
                    fputs("od-vmhost: control-v2: \(error)\n", stderr)
                    return finish(VMHostExitStatus.protocolError)
                }
            }
            if let status = exitStatus, outgoing.isEmpty { return finish(status) }

            var descriptors = [pollfd(fd: fd, events: controlPollEvents(), revents: 0)]
            var displayIndex: Int?
            if runner.displayDescriptor != nil, runner.displayPollEvents != 0, let displayFD = runner.displayDescriptor {
                displayIndex = descriptors.count
                descriptors.append(pollfd(fd: displayFD, events: runner.displayPollEvents, revents: 0))
            }
            var inputIndex: Int?
            if let input, input.isOpen {
                inputIndex = descriptors.count
                descriptors.append(pollfd(fd: input.fd, events: input.pollEvents, revents: 0))
            }
            let timeout = pollTimeoutMs(now: monotonicNs())
            let ready = Darwin.poll(&descriptors, nfds_t(descriptors.count), timeout)
            if ready < 0 {
                if errno == EINTR { continue }
                fputs("od-vmhost: control-v2: poll failed errno \(errno)\n", stderr)
                return finish(VMHostExitStatus.transportLost)
            }
            if let input {
                let events = inputIndex.map { descriptors[$0].revents } ?? 0
                input.service(revents: events, nowNs: monotonicNs()) { runner.currentInputMode() }
            }
            if let displayIndex { runner.serviceDisplay(revents: descriptors[displayIndex].revents) }
            if descriptors[0].revents & Int16(POLLIN | POLLHUP | POLLERR) != 0, canRead {
                if let status = receive() { return finish(status) }
            } else if descriptors[0].revents & Int16(POLLERR | POLLNVAL) != 0 {
                fputs("od-vmhost: control-v2: control socket error\n", stderr)
                return finish(VMHostExitStatus.transportLost)
            }
        }
    }

    /// Teardown at the end of the mode: a last non-blocking flush, then the caller destroys the session and releases the lock.
    private func finish(_ status: Int32) -> Int32 {
        _ = flushOutput(now: monotonicNs(), final: true)
        decoder.discard()
        return status
    }

    // MARK: requests

    private var canRead: Bool {
        exitStatus == nil && !endOfStream && !decoder.hasPendingFrames && outgoing.count + Self.responseReserve <= ControlV2.outputQueueLimit
    }

    private func controlPollEvents() -> Int16 {
        var events: Int16 = 0
        if canRead { events |= Int16(POLLIN) }
        if !outgoing.isEmpty { events |= Int16(POLLOUT) }
        return events
    }

    /// Takes complete frames while the output queue has room (backpressure, section 12.2): a full queue stops the reading of requests.
    private func processFrames() -> Int32? {
        while exitStatus == nil, outgoing.count + Self.responseReserve <= ControlV2.outputQueueLimit {
            do {
                guard let received = try decoder.nextFrame() else { return nil }
                let output = try lifecycle.handle(received)
                for frame in output.frames { outgoing.append(contentsOf: frame.encoded()) }
                if let status = output.exitStatus { exitStatus = status }
            } catch {
                fputs("od-vmhost: control-v2: \(error)\n", stderr)
                return VMHostExitStatus.protocolError
            }
        }
        return nil
    }

    /// One recvmsg of at most the room of one frame; a descriptor may arrive only with the first byte of an ATTACH_RESOURCE.
    ///
    /// The ancillary buffer holds many descriptors, and every descriptor that arrives is collected and closed unless exactly one
    /// came without truncation: no outcome leaves a descriptor open that the protocol did not ask for (section 8).
    private func receive() -> Int32? {
        var data = [UInt8](repeating: 0, count: ControlV2.maxFrameLength - decoder.bufferedPartialBytes)
        var control = [UInt8](repeating: 0, count: Self.ancillaryCapacity)
        var received = 0
        var flags: Int32 = 0
        var descriptors: [Int32] = []
        var violation: String?
        while true {
            descriptors.removeAll()
            violation = nil
            received = data.withUnsafeMutableBytes { dataBuffer in
                control.withUnsafeMutableBytes { controlBuffer in
                    var iov = iovec(iov_base: dataBuffer.baseAddress, iov_len: dataBuffer.count)
                    return withUnsafeMutablePointer(to: &iov) { iovPointer in
                        var header = msghdr()
                        header.msg_iov = iovPointer
                        header.msg_iovlen = 1
                        header.msg_control = controlBuffer.baseAddress
                        header.msg_controllen = socklen_t(controlBuffer.count)
                        let count = Darwin.recvmsg(fd, &header, Int32(MSG_DONTWAIT))
                        flags = header.msg_flags
                        if count >= 0 {
                            violation = Self.collectDescriptors(from: UnsafeRawBufferPointer(controlBuffer),
                                                                length: Int(header.msg_controllen), into: &descriptors)
                        }
                        return count
                    }
                }
            }
            if received < 0, errno == EINTR { continue }
            break
        }
        let receiveErrno = errno
        for descriptor in descriptors { _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC) } // before anything else (section 8)
        if received < 0 {
            for descriptor in descriptors { Darwin.close(descriptor) }
            if receiveErrno == EAGAIN || receiveErrno == EWOULDBLOCK { return nil }
            fputs("od-vmhost: control-v2: recvmsg errno \(receiveErrno)\n", stderr)
            return VMHostExitStatus.transportLost
        }
        if flags & Int32(MSG_CTRUNC) != 0 || violation != nil || descriptors.count > 1 {
            for descriptor in descriptors { Darwin.close(descriptor) }
            let reason = violation ?? (flags & Int32(MSG_CTRUNC) != 0 ? "ancillary data truncated" : "\(descriptors.count) descriptors in one message")
            fputs("od-vmhost: control-v2: \(ControlV2FatalError.descriptor(reason))\n", stderr)
            return VMHostExitStatus.protocolError
        }
        if received == 0 {
            for descriptor in descriptors { Darwin.close(descriptor) }
            endOfStream = true
            return nil
        }
        decoder.append(Array(data[..<received]), descriptor: descriptors.first)
        return nil
    }

    /// Room for 64 descriptors: far above the one the protocol allows, so that surplus descriptors are seen and closed here.
    private static let ancillaryCapacity = 12 + 64 * MemoryLayout<Int32>.size

    /// Walks every `cmsghdr` of the buffer (CMSG_FIRSTHDR / CMSG_NXTHDR of Darwin: header 12 bytes, 4-byte alignment) and appends
    /// all descriptors of `SCM_RIGHTS` messages. Returns a text when something else is there or the chain is malformed.
    private static func collectDescriptors(from buffer: UnsafeRawBufferPointer, length: Int, into descriptors: inout [Int32]) -> String? {
        let headerSize = 12
        var violation: String?
        var offset = 0
        while offset + headerSize <= min(length, buffer.count) {
            let size = Int(buffer.loadUnaligned(fromByteOffset: offset, as: UInt32.self))
            let level = buffer.loadUnaligned(fromByteOffset: offset + 4, as: Int32.self)
            let type = buffer.loadUnaligned(fromByteOffset: offset + 8, as: Int32.self)
            guard size >= headerSize, offset + size <= min(length, buffer.count) else {
                // Collect whatever whole descriptors are still visible, so that the caller closes them too.
                let limit = min(length, buffer.count)
                if level == SOL_SOCKET, type == SCM_RIGHTS {
                    var position = offset + headerSize
                    while position + MemoryLayout<Int32>.size <= limit {
                        descriptors.append(buffer.loadUnaligned(fromByteOffset: position, as: Int32.self))
                        position += MemoryLayout<Int32>.size
                    }
                }
                return violation ?? "malformed ancillary data"
            }
            if level == SOL_SOCKET, type == SCM_RIGHTS {
                var position = offset + headerSize
                while position + MemoryLayout<Int32>.size <= offset + size {
                    descriptors.append(buffer.loadUnaligned(fromByteOffset: position, as: Int32.self))
                    position += MemoryLayout<Int32>.size
                }
            } else {
                violation = "unexpected ancillary data"
            }
            offset += (size + 3) & ~3
        }
        return violation
    }

    // MARK: profile, stats, output

    private func tickProfile(now: UInt64) -> Int32? {
        guard exitStatus == nil, lifecycle.state == .running, runner.isPolling, now >= nextTickNs else { return nil }
        nextTickNs = now + Self.pollTickNs
        do {
            try runner.pollOnce()
        } catch {
            // A failed ABI call leaves the session in an unknown state: `failed`, a fatal ERROR, status 6 (section 7.1).
            fputs("od-vmhost: control-v2: \(error)\n", stderr)
            let output = lifecycle.hostFailure(detail: "\(error)")
            for frame in output.frames { outgoing.append(contentsOf: frame.encoded()) }
            exitStatus = output.exitStatus
        }
        return nil
    }

    /// STATS at most once per 1000 ms, only in `running` and `paused` (section 6.6). This only notes that the interval has run out:
    /// the frame is built, with the state of that moment, and the next 1000 ms are counted from the moment it is put in the output
    /// queue (`flushOutput`), so two STATS are never closer than 1000 ms in the queue.
    private func scheduleStats(now: UInt64) {
        guard exitStatus == nil, lifecycle.state == .running || lifecycle.state == .paused else {
            nextStatsNs = nil
            statsDue = false
            return
        }
        guard let due = nextStatsNs else {
            nextStatsNs = now + Self.statsIntervalNs
            return
        }
        if now >= due { statsDue = true }
    }

    /// Writes what the non-blocking socket takes. nil = go on; a status ends the mode (transport lost, or the queue did not drain
    /// for `output_drain_timeout`, section 12.2).
    private func flushOutput(now: UInt64, final: Bool = false) -> Int32? {
        if statsDue, outgoing.isEmpty, exitStatus == nil, lifecycle.state == .running || lifecycle.state == .paused {
            let stats = ControlV2Stats(state: lifecycle.state.rawValue, pollCount: runner.pollCount,
                                       framesPublished: runner.framesPublished, rssBytes: residentSetSizeBytes())
            outgoing = ControlV2Frame(response: .stats(stats), requestID: 0).encoded()
            statsDue = false
            nextStatsNs = monotonicNs() + Self.statsIntervalNs // counted from the moment of queueing
        }
        var progressed = false
        while !outgoing.isEmpty {
            let count = outgoing.withUnsafeBytes { Darwin.send(fd, $0.baseAddress, $0.count, Int32(MSG_DONTWAIT)) }
            if count > 0 {
                outgoing.removeFirst(count)
                progressed = true
                continue
            }
            if count < 0, errno == EINTR { continue }
            if count < 0, errno == EAGAIN || errno == EWOULDBLOCK { break }
            if !final { fputs("od-vmhost: control-v2: send errno \(errno)\n", stderr) }
            return VMHostExitStatus.transportLost
        }
        if outgoing.isEmpty || progressed {
            drainStalledSinceNs = outgoing.isEmpty ? nil : now
        } else if drainStalledSinceNs == nil {
            drainStalledSinceNs = now
        }
        if let since = drainStalledSinceNs, !final, now >= since + Self.drainTimeoutNs {
            fputs("od-vmhost: control-v2: output queue did not drain\n", stderr)
            return VMHostExitStatus.transportLost
        }
        return nil
    }

    private func pollTimeoutMs(now: UInt64) -> Int32 {
        var deadlines: [UInt64] = []
        if exitStatus == nil, lifecycle.state == .running, runner.isPolling { deadlines.append(nextTickNs) }
        if let nextStatsNs, !statsDue { deadlines.append(nextStatsNs) }
        if let since = drainStalledSinceNs { deadlines.append(since + Self.drainTimeoutNs) }
        var timeout: Int32 = -1
        if let earliest = deadlines.min() {
            let remaining = earliest > now ? (earliest - now + 999_999) / 1_000_000 : 0
            timeout = Int32(min(remaining, 1_000))
        }
        if let input, let due = input.pollTimeoutMs(nowNs: now) { timeout = timeout < 0 ? due : min(timeout, due) }
        return timeout
    }
}
