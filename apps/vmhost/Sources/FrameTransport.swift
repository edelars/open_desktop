import Darwin
import Foundation

// Writer side of frame transport v1 (protocols/control/FRAME_TRANSPORT_V1.md).
//
// TEST-ONLY entry point: `--display-smoke pattern` runs a VMHost without a guest.
// It creates a display session with OD_DISPLAY_PROFILE_TEST_PATTERN, publishes
// the frame of that session into a shared-memory slot and notifies the reader.
// It is not part of the normal VMHost path and is never selected implicitly.
//
// Atomics go through the C shim FrameTransportAtomics.h (section 5.3). The frame
// is copied once: od_display_session_read_frame writes straight into the slot.

enum FrameTransportError: Error, CustomStringConvertible {
    case usage
    case unsupportedHostPage
    case sharedMemory(String)
    case channel(String)
    case protocolViolation(String)
    case displayABI(String, UInt32)
    case hostRefusal(String)

    var description: String {
        switch self {
        case .usage:
            return "usage: od-vmhost-smoke --socket-fd FD --display-smoke pattern [--display-poll-hz N] [--display-hold-until-claim 1] [--input-fd FD [--input-trace 1]]"
        case .unsupportedHostPage: return "host page size does not divide 16384"
        case .sharedMemory(let what): return "shared memory: \(what)"
        case .channel(let what): return "frame channel: \(what)"
        case .protocolViolation(let what): return "frame transport protocol: \(what)"
        case .displayABI(let call, let status): return "\(call) returned status \(status)"
        case .hostRefusal(let what): return "host refusal: \(what)"
        }
    }
}

/// Command-line options of the test-only display smoke mode.
struct DisplaySmokeOptions {
    let socketFD: Int32
    /// How often the publish loop checks the session; 60 Hz by default (S10-06).
    let pollHz: Int
    /// Test-only: hold publications back until the reader sets `reader_claim`, so a reader can
    /// observe generation 0 deterministically before the first frame.
    let holdUntilClaim: Bool
    /// S10-16: descriptor of the input channel (INPUT_TRANSPORT_V1.md 2.2); nil = no input channel.
    let inputFD: Int32?
    /// S10-16, test-only: print the records that leave the VMHost input queue (section 10) instead of leaving them in the queue.
    let inputTrace: Bool

    /// Returns nil when `--display-smoke` is absent (normal VMHost path).
    static func parse(_ args: [String]) throws -> DisplaySmokeOptions? {
        guard args.contains("--display-smoke") else { return nil }
        var socketFD: Int32 = -1
        var pollHz = 60
        var hold = false
        var inputFD: Int32?
        var inputTrace = false
        var sawPattern = false
        var index = 0
        while index < args.count {
            let name = args[index]
            guard index + 1 < args.count else { throw FrameTransportError.usage }
            let value = args[index + 1]
            switch name {
            case "--socket-fd":
                guard let fd = Int32(value), fd >= 0 else { throw FrameTransportError.usage }
                socketFD = fd
            case "--display-smoke":
                guard value == "pattern" else { throw FrameTransportError.usage }
                sawPattern = true
            case "--display-poll-hz":
                guard let hz = Int(value), (1...1000).contains(hz) else { throw FrameTransportError.usage }
                pollHz = hz
            case "--display-hold-until-claim":
                guard value == "1" else { throw FrameTransportError.usage }
                hold = true
            case "--input-fd":
                guard let fd = Int32(value), fd >= 0 else { throw FrameTransportError.usage }
                inputFD = fd
            case "--input-trace":
                guard value == "1" else { throw FrameTransportError.usage }
                inputTrace = true
            default:
                throw FrameTransportError.usage
            }
            index += 2
        }
        guard socketFD >= 0, sawPattern else { throw FrameTransportError.usage }
        // The input descriptor must be another one than the frame socket; the trace needs a channel.
        guard inputFD != socketFD, inputFD != nil || !inputTrace else { throw FrameTransportError.usage }
        return DisplaySmokeOptions(
            socketFD: socketFD, pollHz: pollHz, holdUntilClaim: hold, inputFD: inputFD, inputTrace: inputTrace)
    }
}

private extension Array where Element == UInt8 {
    mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        var le = value.littleEndian
        Swift.withUnsafeBytes(of: &le) { append(contentsOf: $0) }
    }
}

private func readLE<T: FixedWidthInteger>(_ bytes: [UInt8], _ offset: Int, as _: T.Type) -> T {
    var value: T = 0
    for i in 0..<MemoryLayout<T>.size { value |= T(truncatingIfNeeded: bytes[offset + i]) << T(8 * i) }
    return value
}

/// Shared memory layout constants (section 3).
private enum Layout {
    static let granule = 16_384
    static let headerSize = 64
    static let readerBlockOffset = 64
    static let descriptorsOffset = 128
    static let descriptorSize = 64
    static let slotsOffset = 16_384
    static let magic: [UInt8] = Array("ODFS".utf8)
    static let pixelFormatBGRX8888: UInt32 = 1
}

/// Message constants (section 4).
private enum Message {
    static let magic: [UInt8] = Array("ODFT".utf8)
    static let version: UInt16 = 1
    static let headerLength = 16
    static let kindVersion: UInt16 = 0
    static let kindHello: UInt16 = 1
    static let kindAnnounce: UInt16 = 2
    static let kindMode: UInt16 = 3
    static let kindFrame: UInt16 = 4

    static func header(kind: UInt16, payloadLength: UInt32, version: UInt16 = Message.version) -> [UInt8] {
        var bytes = magic
        bytes.appendLE(version)
        bytes.appendLE(kind)
        bytes.appendLE(UInt32(0))
        bytes.appendLE(payloadLength)
        return bytes
    }

    static func mode(count: UInt64, active: Bool, width: UInt32, height: UInt32, stride: UInt32, format: UInt32) -> [UInt8] {
        var bytes = header(kind: kindMode, payloadLength: 32)
        bytes.appendLE(count)
        bytes.appendLE(UInt32(active ? 1 : 0))
        // ACTIVE = 0 means "no signal": the remaining fields are zero (section 4.2).
        bytes.appendLE(active ? width : 0)
        bytes.appendLE(active ? height : 0)
        bytes.appendLE(active ? stride : 0)
        bytes.appendLE(active ? format : 0)
        bytes.appendLE(UInt32(0))
        return bytes
    }

    static func frame(generation: UInt64, modeChangeCount: UInt64, slot: UInt32) -> [UInt8] {
        var bytes = header(kind: kindFrame, payloadLength: 24)
        bytes.appendLE(generation)
        bytes.appendLE(modeChangeCount)
        bytes.appendLE(slot)
        bytes.appendLE(UInt32(0))
        return bytes
    }
}

private func wait(fd: Int32, events: Int16, timeoutMs: Int32) throws -> Int16 {
    var p = pollfd(fd: fd, events: events, revents: 0)
    while true {
        let r = Darwin.poll(&p, 1, timeoutMs)
        if r >= 0 { return r == 0 ? 0 : p.revents }
        if errno == EINTR { continue }
        throw FrameTransportError.channel("poll failed errno \(errno)")
    }
}

/// Polls several descriptors (S10-16 adds the input channel to the frame socket); EINTR is retried.
private func pollAll(_ descriptors: inout [pollfd], timeoutMs: Int32) throws {
    while true {
        let r = Darwin.poll(&descriptors, nfds_t(descriptors.count), timeoutMs)
        if r >= 0 { return }
        if errno == EINTR { continue }
        throw FrameTransportError.channel("poll failed errno \(errno)")
    }
}

/// Section 6.4: at most one partial message, one pending MODE, one pending FRAME.
private struct NotificationQueue {
    private var partial: [UInt8] = []
    private var pendingMode: [UInt8]?
    private var pendingFrame: [UInt8]?

    var isEmpty: Bool { partial.isEmpty && pendingMode == nil && pendingFrame == nil }

    /// A new MODE replaces an unsent MODE and drops the unsent FRAME.
    mutating func queueMode(_ message: [UInt8]) {
        pendingMode = message
        pendingFrame = nil
    }

    /// A new FRAME replaces an unsent FRAME.
    mutating func queueFrame(_ message: [UInt8]) { pendingFrame = message }

    /// Sends as much as the non-blocking socket accepts: partial message, then MODE, then FRAME.
    mutating func flush(fd: Int32) throws {
        while true {
            if partial.isEmpty {
                if let mode = pendingMode {
                    partial = mode
                    pendingMode = nil
                } else if let frame = pendingFrame {
                    partial = frame
                    pendingFrame = nil
                } else {
                    return
                }
            }
            let n = partial.withUnsafeBytes { Darwin.send(fd, $0.baseAddress, $0.count, 0) }
            if n > 0 {
                partial.removeFirst(n)
                continue
            }
            if n < 0 {
                if errno == EINTR { continue }
                if errno == EAGAIN || errno == EWOULDBLOCK { return }
            }
            throw FrameTransportError.channel("send failed errno \(errno)")
        }
    }
}

/// Writer: owns the shared memory object and the publication state (all private).
final class FrameTransportWriter {
    private let slotCount = 3
    private let base: UnsafeMutableRawPointer
    private let totalSize: Int
    private let slotSize: Int
    private let maxWidth: UInt32
    private let maxHeight: UInt32
    private var shmFD: Int32
    // Private writer state; the writer never reads descriptors or the header back (section 5.1).
    private var slotGeneration: [UInt64]
    private var front: Int?
    private var lastPublished: UInt64 = 0
    private var queuedModeCount: UInt64 = 0
    private var queue = NotificationQueue()

    /// Steps 1-7 of section 2.2 for a fixed maximum mode (test-only writer).
    init(maxWidth: UInt32, maxHeight: UInt32) throws {
        guard Self.pageSizeSupported() else { throw FrameTransportError.unsupportedHostPage }
        guard (1...4096).contains(maxWidth), (1...2304).contains(maxHeight) else {
            throw FrameTransportError.hostRefusal("mode \(maxWidth)x\(maxHeight) outside device limits")
        }
        let frameBytes = Int(maxWidth) * 4 * Int(maxHeight)
        let granule = Layout.granule
        let slotSize = (frameBytes + granule - 1) / granule * granule
        let total = Layout.slotsOffset + slotCount * slotSize
        self.slotSize = slotSize
        self.totalSize = total
        self.maxWidth = maxWidth
        self.maxHeight = maxHeight
        self.slotGeneration = Array(repeating: 0, count: slotCount)

        // 1-3: random name, O_EXCL create, immediate unlink; the name is never passed on.
        var fd: Int32 = -1
        for _ in 0..<8 {
            var random = [UInt8](repeating: 0, count: 8)
            arc4random_buf(&random, random.count)
            let name = "/odft." + random.map { String(format: "%02x", $0) }.joined()
            fd = vmhost_shm_create_excl(name)
            if fd >= 0 {
                if shm_unlink(name) != 0 {
                    let e = errno
                    Darwin.close(fd)
                    throw FrameTransportError.sharedMemory("shm_unlink errno \(e)")
                }
                break
            }
            if errno != EEXIST { throw FrameTransportError.sharedMemory("shm_open errno \(errno)") }
        }
        guard fd >= 0 else { throw FrameTransportError.sharedMemory("shm_open: name collisions") }
        self.shmFD = fd
        // 4: the single size change of the object.
        guard ftruncate(fd, off_t(total)) == 0 else {
            let e = errno
            Darwin.close(fd)
            throw FrameTransportError.sharedMemory("ftruncate errno \(e)")
        }
        // 5: other children of VMHost must not inherit the memory.
        guard fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else {
            let e = errno
            Darwin.close(fd)
            throw FrameTransportError.sharedMemory("fcntl FD_CLOEXEC errno \(e)")
        }
        // 6: map.
        guard let mapped = mmap(nil, total, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0),
              mapped != UnsafeMutableRawPointer(bitPattern: -1) else {
            let e = errno
            Darwin.close(fd)
            throw FrameTransportError.sharedMemory("mmap errno \(e)")
        }
        self.base = mapped
        // 7: header (plain stores, readable by the peer only after ANNOUNCE), then fence(release).
        base.storeBytes(of: Layout.magic[0], toByteOffset: 0, as: UInt8.self)
        base.storeBytes(of: Layout.magic[1], toByteOffset: 1, as: UInt8.self)
        base.storeBytes(of: Layout.magic[2], toByteOffset: 2, as: UInt8.self)
        base.storeBytes(of: Layout.magic[3], toByteOffset: 3, as: UInt8.self)
        base.storeBytes(of: UInt16(1).littleEndian, toByteOffset: 4, as: UInt16.self)
        base.storeBytes(of: UInt16(Layout.headerSize).littleEndian, toByteOffset: 6, as: UInt16.self)
        base.storeBytes(of: UInt32(Layout.readerBlockOffset).littleEndian, toByteOffset: 8, as: UInt32.self)
        base.storeBytes(of: UInt32(Layout.descriptorsOffset).littleEndian, toByteOffset: 12, as: UInt32.self)
        base.storeBytes(of: UInt32(Layout.descriptorSize).littleEndian, toByteOffset: 16, as: UInt32.self)
        base.storeBytes(of: UInt32(slotCount).littleEndian, toByteOffset: 20, as: UInt32.self)
        base.storeBytes(of: UInt32(Layout.slotsOffset).littleEndian, toByteOffset: 24, as: UInt32.self)
        base.storeBytes(of: UInt32(slotSize).littleEndian, toByteOffset: 28, as: UInt32.self)
        base.storeBytes(of: Layout.pixelFormatBGRX8888.littleEndian, toByteOffset: 32, as: UInt32.self)
        base.storeBytes(of: maxWidth.littleEndian, toByteOffset: 36, as: UInt32.self)
        base.storeBytes(of: maxHeight.littleEndian, toByteOffset: 40, as: UInt32.self)
        base.storeBytes(of: UInt32(0), toByteOffset: 44, as: UInt32.self)
        base.storeBytes(of: UInt64(total).littleEndian, toByteOffset: 48, as: UInt64.self)
        base.storeBytes(of: UInt64(0), toByteOffset: 56, as: UInt64.self)
        odft_fence_release()
    }

    deinit {
        munmap(base, totalSize)
        if shmFD >= 0 { Darwin.close(shmFD) }
    }

    private static func pageSizeSupported() -> Bool {
        let page = Int(getpagesize())
        return page > 0 && Layout.granule % page == 0
    }

    // MARK: pointers into the mapping

    private func word32(_ offset: Int) -> UnsafeMutablePointer<UInt32> {
        base.advanced(by: offset).assumingMemoryBound(to: UInt32.self)
    }

    private func word64(_ offset: Int) -> UnsafeMutablePointer<UInt64> {
        base.advanced(by: offset).assumingMemoryBound(to: UInt64.self)
    }

    private func descriptorOffset(_ slot: Int) -> Int { Layout.descriptorsOffset + Layout.descriptorSize * slot }

    private func slotPixels(_ slot: Int) -> UnsafeMutablePointer<UInt8> {
        base.advanced(by: Layout.slotsOffset + slot * slotSize).assumingMemoryBound(to: UInt8.self)
    }

    // MARK: ANNOUNCE (section 4.3)

    /// One `sendmsg` with the descriptor. On EAGAIN/EINTR the same `sendmsg` is repeated.
    func sendAnnounce(on fd: Int32) throws {
        var message = Message.header(kind: Message.kindAnnounce, payloadLength: 16)
        message.appendLE(UInt64(totalSize))
        message.appendLE(UInt32(slotCount))
        message.appendLE(UInt32(0))
        // cmsghdr on Darwin: len u32, level i32, type i32, data at offset 12; CMSG_LEN(4) = CMSG_SPACE(4) = 16.
        var control = [UInt8](repeating: 0, count: 16)
        control.withUnsafeMutableBytes { raw in
            raw.storeBytes(of: UInt32(16), toByteOffset: 0, as: UInt32.self)
            raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
            raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
            raw.storeBytes(of: shmFD, toByteOffset: 12, as: Int32.self)
        }
        var sent = 0
        try message.withUnsafeMutableBytes { data in
            try control.withUnsafeMutableBytes { ctl in
                var iov = iovec(iov_base: data.baseAddress, iov_len: data.count)
                try withUnsafeMutablePointer(to: &iov) { iovPointer in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = ctl.baseAddress
                    header.msg_controllen = socklen_t(ctl.count)
                    while true {
                        let n = Darwin.sendmsg(fd, &header, 0)
                        if n > 0 {
                            sent = n
                            return
                        }
                        if n == 0 { throw FrameTransportError.channel("sendmsg(ANNOUNCE) sent 0 bytes") }
                        if errno == EINTR { continue }
                        if errno == EAGAIN || errno == EWOULDBLOCK {
                            _ = try wait(fd: fd, events: Int16(POLLOUT), timeoutMs: -1)
                            continue
                        }
                        throw FrameTransportError.channel("sendmsg(ANNOUNCE) errno \(errno)")
                    }
                }
            }
        }
        // The descriptor travels with the first byte; the rest goes without ancillary data.
        while sent < message.count {
            let n = message.withUnsafeBytes { Darwin.send(fd, $0.baseAddress! + sent, $0.count - sent, 0) }
            if n > 0 {
                sent += n
            } else if n < 0, errno == EINTR {
                continue
            } else if n < 0, errno == EAGAIN || errno == EWOULDBLOCK {
                _ = try wait(fd: fd, events: Int16(POLLOUT), timeoutMs: -1)
            } else {
                throw FrameTransportError.channel("send(ANNOUNCE tail) errno \(errno)")
            }
        }
        // The reader holds its own reference now; the mapping stays valid.
        Darwin.close(shmFD)
        shmFD = -1
    }

    // MARK: notifications (sections 6.3, 6.4)

    /// True once the reader has written a non-zero `reader_claim` (test-only publication gate).
    var readerHasClaimed: Bool { odft_load_relaxed_u32(word32(Layout.readerBlockOffset)) != 0 }

    /// Section 6.3: an active mode beyond the announced limits or with another format cannot be
    /// announced on this connection; that is a host refusal.
    func queueMode(count: UInt64, active: Bool, width: UInt32, height: UInt32, stride: UInt32, format: UInt32) throws {
        if active {
            guard width >= 1, height >= 1, width <= maxWidth, height <= maxHeight,
                  format == Layout.pixelFormatBGRX8888, UInt64(stride) == UInt64(width) * 4 else {
                throw FrameTransportError.hostRefusal("mode \(width)x\(height) exceeds the announced layout")
            }
        }
        queue.queueMode(Message.mode(count: count, active: active, width: width, height: height, stride: stride, format: format))
        queuedModeCount = count
    }

    var hasPendingOutput: Bool { !queue.isEmpty }

    func flush(fd: Int32) throws { try queue.flush(fd: fd) }

    var queuedMode: UInt64 { queuedModeCount }
    var lastPublishedGeneration: UInt64 { lastPublished }

    // MARK: publication (sections 5.1, 6.2)

    /// Section 6.2: not `front`, not the slot the reader claims; the oldest of the rest.
    private func chooseSlot() -> Int? {
        let claim = Int(odft_load_relaxed_u32(word32(Layout.readerBlockOffset)))
        var best: Int?
        for s in 0..<slotCount {
            if s == front { continue }
            if claim >= 1, claim <= slotCount, s == claim - 1 { continue }
            if let b = best, slotGeneration[b] <= slotGeneration[s] { continue }
            best = s
        }
        return best
    }

    /// Reads the device frame straight into a free slot and publishes it.
    /// Returns true when a frame was published.
    func publishFrame(session: OpaquePointer, mode: od_display_mode) throws -> Bool {
        guard let slot = chooseSlot() else { return false }
        let descriptor = descriptorOffset(slot)
        // 5.1 steps 2-3: invalidate, make the mark visible before any pixel write.
        odft_store_relaxed_u64(word64(descriptor), 0)
        odft_fence_release()
        slotGeneration[slot] = 0
        // Section 3.5 order: width/height limits first, then stride, then the product (in UInt64).
        guard mode.width >= 1, mode.height >= 1, mode.width <= maxWidth, mode.height <= maxHeight,
              mode.format == Layout.pixelFormatBGRX8888, UInt64(mode.stride) == UInt64(mode.width) * 4,
              UInt64(mode.stride) * UInt64(mode.height) <= UInt64(slotSize) else {
            throw FrameTransportError.hostRefusal("mode \(mode.width)x\(mode.height) exceeds the announced layout")
        }
        let length = Int(mode.stride) * Int(mode.height)
        // 5.1 step 4: the single copy, device memory -> slot.
        var info = od_display_frame_info()
        info.abi_version = vmhost_display_abi_version_v1()
        info.struct_size = UInt32(OD_DISPLAY_FRAME_INFO_SIZE_V1)
        let status = od_display_session_read_frame(session, slotPixels(slot), length, &info)
        if status != OD_DISPLAY_STATUS_OK.rawValue {
            switch status {
            case OD_DISPLAY_STATUS_NO_FRAME.rawValue, OD_DISPLAY_STATUS_NO_ACTIVE_MODE.rawValue,
                 OD_DISPLAY_STATUS_MODE_CHANGED.rawValue, OD_DISPLAY_STATUS_BUFFER_LENGTH.rawValue:
                return false // slot stays invalid; retry on the next check
            default:
                throw FrameTransportError.displayABI("od_display_session_read_frame", status)
            }
        }
        // 6.3: a mode beyond the announced limits cannot be published on this connection.
        guard info.width >= 1, info.height >= 1, info.width <= maxWidth, info.height <= maxHeight,
              info.format == Layout.pixelFormatBGRX8888, UInt64(info.stride) == UInt64(info.width) * 4,
              UInt64(info.stride) * UInt64(info.height) <= UInt64(slotSize) else {
            throw FrameTransportError.hostRefusal("frame \(info.width)x\(info.height) exceeds the announced layout")
        }
        // 5.1 step 5.
        let generation = info.generation
        let modeCount = info.mode_change_count
        if generation <= lastPublished || modeCount < queuedModeCount { return false }
        if modeCount > queuedModeCount {
            try queueMode(count: modeCount, active: true, width: info.width, height: info.height,
                          stride: info.stride, format: info.format)
        }
        // Steps 6-7: relaxed field stores, then the release store of generation.
        odft_store_relaxed_u64(word64(descriptor + 8), modeCount)
        odft_store_relaxed_u32(word32(descriptor + 16), info.width)
        odft_store_relaxed_u32(word32(descriptor + 20), info.height)
        odft_store_relaxed_u32(word32(descriptor + 24), info.stride)
        odft_store_relaxed_u32(word32(descriptor + 28), info.format)
        odft_store_relaxed_u32(word32(descriptor + 32), 1) // READY
        odft_store_relaxed_u32(word32(descriptor + 36), 0)
        odft_store_relaxed_u64(word64(descriptor + 40), 0)
        odft_store_relaxed_u64(word64(descriptor + 48), 0)
        odft_store_relaxed_u64(word64(descriptor + 56), 0)
        odft_store_release_u64(word64(descriptor), generation)
        slotGeneration[slot] = generation
        front = slot
        lastPublished = generation
        // Step 8: the socket write happens after the publication.
        queue.queueFrame(Message.frame(generation: generation, modeChangeCount: modeCount, slot: UInt32(slot)))
        return true
    }
}

// MARK: test-only display smoke (VMHost without a guest)

enum DisplaySmoke {
    private static func currentMode(_ session: OpaquePointer) throws -> od_display_mode {
        var mode = od_display_mode()
        mode.abi_version = vmhost_display_abi_version_v1()
        mode.struct_size = UInt32(OD_DISPLAY_MODE_SIZE_V1)
        let status = od_display_session_mode(session, &mode)
        guard status == OD_DISPLAY_STATUS_OK.rawValue else {
            throw FrameTransportError.displayABI("od_display_session_mode", status)
        }
        return mode
    }

    /// Reads exactly 16 bytes of HELLO from a blocking socket. nil means EOF before a full header.
    private static func readHello(from fd: Int32) throws -> [UInt8]? {
        var buffer = [UInt8](repeating: 0, count: Message.headerLength)
        var got = 0
        while got < buffer.count {
            let n = buffer.withUnsafeMutableBytes { Darwin.recv(fd, $0.baseAddress! + got, $0.count - got, 0) }
            if n > 0 {
                got += n
            } else if n == 0 {
                return nil
            } else if errno == EINTR {
                continue
            } else {
                throw FrameTransportError.channel("recv(HELLO) errno \(errno)")
            }
        }
        return buffer
    }

    static func run(_ options: DisplaySmokeOptions) throws {
        let fd = options.socketFD
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        // S10-16: the input channel exists only with --input-fd (CLOEXEC and non-blocking are set here).
        let input = try options.inputFD.map { try InputHostChannel(fd: $0, trace: options.inputTrace) }

        var config = od_display_session_config()
        config.abi_version = vmhost_display_abi_version_v1()
        config.struct_size = UInt32(OD_DISPLAY_SESSION_CONFIG_SIZE_V1)
        config.profile = OD_DISPLAY_PROFILE_TEST_PATTERN.rawValue
        var created: OpaquePointer?
        let createStatus = od_display_session_create(&config, &created)
        guard createStatus == OD_DISPLAY_STATUS_OK.rawValue, let session = created else {
            throw FrameTransportError.displayABI("od_display_session_create", createStatus)
        }
        defer { _ = od_display_session_destroy(session) }

        let initial = try currentMode(session)
        guard initial.flags & vmhost_display_mode_flag_active() != 0 else {
            throw FrameTransportError.hostRefusal("test pattern profile has no active mode")
        }
        // Test-only writer with a fixed mode declares that mode as its maximum (section 3.1).
        let writer = try FrameTransportWriter(maxWidth: initial.width, maxHeight: initial.height)

        // 6.1: HELLO first (blocking), then everything is non-blocking.
        guard let hello = try readHello(from: fd) else { return }
        guard Array(hello[0..<4]) == Message.magic else {
            throw FrameTransportError.protocolViolation("HELLO has a foreign magic") // closed without reply
        }
        let peerVersion: UInt16 = readLE(hello, 4, as: UInt16.self)
        if peerVersion != Message.version {
            let reply = Message.header(kind: Message.kindVersion, payloadLength: 0)
            _ = reply.withUnsafeBytes { Darwin.send(fd, $0.baseAddress, $0.count, 0) }
            return
        }
        let kind: UInt16 = readLE(hello, 6, as: UInt16.self)
        let reserved: UInt32 = readLE(hello, 8, as: UInt32.self)
        let length: UInt32 = readLE(hello, 12, as: UInt32.self)
        guard kind == Message.kindHello, reserved == 0, length == 0 else {
            throw FrameTransportError.protocolViolation("malformed HELLO")
        }

        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0 else {
            throw FrameTransportError.channel("O_NONBLOCK errno \(errno)")
        }
        try writer.sendAnnounce(on: fd)
        try writer.queueMode(count: initial.mode_change_count, active: true, width: initial.width,
                             height: initial.height, stride: initial.stride, format: initial.format)

        let tickMs = Int32(max(1, 1000 / options.pollHz))
        var gateOpen = !options.holdUntilClaim
        while true {
            // One quantum, then a check no more often than the poll rate (the wait below).
            var report = od_display_step_report()
            report.abi_version = vmhost_display_abi_version_v1()
            report.struct_size = UInt32(OD_DISPLAY_STEP_REPORT_SIZE_V1)
            let stepStatus = od_display_session_step(session, 1_000_000, &report)
            guard stepStatus == OD_DISPLAY_STATUS_OK.rawValue else {
                throw FrameTransportError.displayABI("od_display_session_step", stepStatus)
            }
            let mode = try currentMode(session)
            if mode.mode_change_count != writer.queuedMode {
                try writer.queueMode(count: mode.mode_change_count,
                                     active: mode.flags & vmhost_display_mode_flag_active() != 0,
                                     width: mode.width, height: mode.height, stride: mode.stride, format: mode.format)
            }
            if !gateOpen, writer.readerHasClaimed { gateOpen = true } // latched
            if gateOpen,
               mode.flags & vmhost_display_mode_flag_active() != 0,
               mode.flags & vmhost_display_mode_flag_has_frame() != 0,
               mode.generation > writer.lastPublishedGeneration {
                if try writer.publishFrame(session: session, mode: mode) {
                    fputs("od-vmhost-smoke: display-smoke published generation \(writer.lastPublishedGeneration)\n", stderr)
                }
            }
            try writer.flush(fd: fd)

            // 6.5: EOF or any byte from the UI after HELLO ends the connection.
            let events = Int16(POLLIN) | (writer.hasPendingOutput ? Int16(POLLOUT) : 0)
            var descriptors = [pollfd(fd: fd, events: events, revents: 0)]
            if let input, input.isOpen { descriptors.append(pollfd(fd: input.fd, events: input.pollEvents, revents: 0)) }
            var timeout = tickMs
            if let input, let due = input.pollTimeoutMs(nowNs: DispatchTime.now().uptimeNanoseconds) {
                timeout = min(timeout, due)
            }
            try pollAll(&descriptors, timeoutMs: timeout)
            // The input channel is serviced first: its errors close only itself (section 8.3) and never end the frame loop.
            if let input {
                let inputEvents = descriptors.count > 1 ? descriptors[1].revents : 0
                input.service(revents: inputEvents, nowNs: DispatchTime.now().uptimeNanoseconds) {
                    guard let current = try? currentMode(session), current.flags & vmhost_display_mode_flag_active() != 0 else {
                        return nil
                    }
                    return InputDisplayMode(
                        modeChangeCount: current.mode_change_count, width: current.width, height: current.height)
                }
            }
            let revents = descriptors[0].revents
            if revents & Int16(POLLIN) != 0 {
                var byte: UInt8 = 0
                let n = Darwin.recv(fd, &byte, 1, Int32(MSG_DONTWAIT))
                if n > 0 { throw FrameTransportError.protocolViolation("bytes from the reader after HELLO") }
                if n == 0 { return } // EOF: normal teardown, status 0
                if errno != EAGAIN, errno != EWOULDBLOCK, errno != EINTR {
                    throw FrameTransportError.channel("recv errno \(errno)") // socket error: status 1
                }
            } else if revents & Int16(POLLERR | POLLNVAL) != 0 {
                throw FrameTransportError.channel("poll reported a socket error") // status 1
            } else if revents & Int16(POLLHUP) != 0 {
                return // peer closed: EOF, status 0
            }
        }
    }
}
