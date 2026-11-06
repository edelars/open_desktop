import Darwin
import Foundation

// Input transport v1 (protocols/control/INPUT_TRANSPORT_V1.md, task S10-16): messages of the ODIT channel.
// Pure encoding, decoding and checks; no AppKit. This file is shared by the UI (apps/macos) and VMHost (apps/vmhost):
// both sides compile the same source, so the byte layout and the checks cannot diverge.
// Numbers are little-endian fixed width, there is no padding (section 4.1).
//
// There is NO delivery to the guest: the board has no input device yet (section 11); `GUEST_DELIVERY` stays 0.

enum InputProtocol {
    static let magic: [UInt8] = [0x4f, 0x44, 0x49, 0x54] // "ODIT"
    static let version: UInt16 = 1
    static let headerLength = 16
    /// Section 7.1: capacity of both queues and the size of one record (the largest message, MOTION).
    static let queueCapacity = 256
    static let recordSize = 64
    /// Largest message each side buffers (section 4.1): VMHost reads MOTION (64), the UI reads STATUS (48).
    static let maxMessageToHost = 64
    static let maxMessageToUI = 48
    /// Device limits (FRAME_TRANSPORT_V1.md 3.1), section 4.2.
    static let maxWidth: UInt32 = 4096
    static let maxHeight: UInt32 = 2304
    static let acceptGuestDeliveryFlag: UInt32 = 1
}

enum InputKind: UInt16, CaseIterable {
    case version = 0, hello = 1, accept = 2, focus = 3, usage = 4, motion = 5, status = 6

    /// Fixed payload length of the kind (table 4.2).
    var payloadLength: Int {
        switch self {
        case .version, .hello: return 0
        case .accept, .focus, .usage: return 8
        case .motion: return 48
        case .status: return 32
        }
    }

    var totalLength: Int { InputProtocol.headerLength + payloadLength }

    /// UI -> VMHost kinds; the others are sent by VMHost.
    var sentByUI: Bool {
        switch self {
        case .hello, .focus, .usage, .motion: return true
        case .version, .accept, .status: return false
        }
    }
}

/// Which side reads: decides which kinds are legal (a kind of the other direction is a protocol error).
enum InputDirection {
    /// The reader is VMHost: HELLO, FOCUS, USAGE, MOTION.
    case uiToHost
    /// The reader is the UI: ACCEPT, STATUS (VERSION is legal on the wire but never with version 1).
    case hostToUI

    var maxMessage: Int { self == .uiToHost ? InputProtocol.maxMessageToHost : InputProtocol.maxMessageToUI }
}

enum InputProtocolError: Error, Equatable, CustomStringConvertible {
    case badMagic
    /// Header `ODIT` with `version != 1` (section 9.1): the UI shows INCOMPATIBLE, VMHost answers VERSION.
    case incompatibleVersion(UInt16)
    case malformed(String)

    var description: String {
        switch self {
        case .badMagic: return "foreign magic"
        case let .incompatibleVersion(version): return "incompatible input protocol version \(version)"
        case let .malformed(reason): return "input protocol error: \(reason)"
        }
    }
}

// MARK: Usage rules (section 6.1)

enum InputUsagePage {
    static let keyboard: UInt16 = 0x07
    static let button: UInt16 = 0x09
}

struct InputUsage: Equatable, Hashable {
    var page: UInt16
    var id: UInt16
    var down: Bool

    /// Section 6.1 (HUT 1.7): keyboard 0x04...0xA4, 0xB0...0xDD, 0xE0...0xE7; buttons 0x01...0x05.
    static func isValid(page: UInt16, id: UInt16) -> Bool {
        switch page {
        case InputUsagePage.keyboard:
            return (0x04...0xA4).contains(id) || (0xB0...0xDD).contains(id) || (0xE0...0xE7).contains(id)
        case InputUsagePage.button:
            return (0x01...0x05).contains(id)
        default:
            return false
        }
    }

    /// Dense index for bitmaps: keyboard usages 0...255, buttons 256...260 (section 7.2: 256 + 5), nil for invalid usages.
    static func slot(page: UInt16, id: UInt16) -> Int? {
        guard isValid(page: page, id: id) else { return nil }
        return page == InputUsagePage.keyboard ? Int(id) : 256 + Int(id) - 1
    }

    static let slotCount = 261
}

// MARK: Messages

struct InputMotion: Equatable {
    var modeChangeCount: UInt64 = 0
    var dx: Int32 = 0
    var dy: Int32 = 0
    var wheel: Int32 = 0
    var pan: Int32 = 0
    var absX: UInt32 = 0
    var absY: UInt32 = 0
    var absWidth: UInt32 = 0
    var absHeight: UInt32 = 0
    var absValid = false

    var hasRelativePart: Bool { dx != 0 || dy != 0 || wheel != 0 || pan != 0 }

    /// Section 4.2: the checks that need no state. An empty MOTION (no delta, `ABS_VALID = 0`) is an error.
    func validate() throws {
        if absValid {
            guard absWidth >= 1, absWidth <= InputProtocol.maxWidth, absHeight >= 1, absHeight <= InputProtocol.maxHeight else {
                throw InputProtocolError.malformed("MOTION abs size \(absWidth)x\(absHeight) out of range")
            }
            guard absX < absWidth, absY < absHeight else {
                throw InputProtocolError.malformed("MOTION abs position \(absX),\(absY) outside \(absWidth)x\(absHeight)")
            }
        } else {
            guard modeChangeCount == 0, absX == 0, absY == 0, absWidth == 0, absHeight == 0 else {
                throw InputProtocolError.malformed("MOTION abs fields without ABS_VALID")
            }
            guard hasRelativePart else { throw InputProtocolError.malformed("empty MOTION") }
        }
    }

    mutating func clearAbsolute() {
        modeChangeCount = 0
        absX = 0
        absY = 0
        absWidth = 0
        absHeight = 0
        absValid = false
    }

    /// Section 7.3: sums with saturation, takes the absolute part and the mode count from `newer`.
    mutating func merge(_ newer: InputMotion) {
        dx = InputMotion.saturatingAdd(dx, newer.dx)
        dy = InputMotion.saturatingAdd(dy, newer.dy)
        wheel = InputMotion.saturatingAdd(wheel, newer.wheel)
        pan = InputMotion.saturatingAdd(pan, newer.pan)
        modeChangeCount = newer.modeChangeCount
        absX = newer.absX
        absY = newer.absY
        absWidth = newer.absWidth
        absHeight = newer.absHeight
        absValid = newer.absValid
    }

    static func saturatingAdd(_ a: Int32, _ b: Int32) -> Int32 {
        let sum = Int64(a) + Int64(b)
        return Int32(max(Int64(Int32.min), min(Int64(Int32.max), sum)))
    }
}

/// Display mode of VMHost as input checks see it (section 8.2 item 4).
struct InputDisplayMode: Equatable {
    var modeChangeCount: UInt64
    var width: UInt32
    var height: UInt32
}

extension InputMotion {
    enum Reconciled: Equatable {
        case keep(InputMotion)
        /// The absolute part was stale and nothing else is left: the record is dropped.
        case dropEmpty
    }

    /// Section 8.2 item 4, used on receipt and at the queue exit. A stale mode count resets the absolute part (not an error);
    /// the same count with another size is a protocol error. Without a display the absolute part is always reset.
    func reconciled(with mode: InputDisplayMode?) throws -> Reconciled {
        guard absValid else { return .keep(self) }
        var value = self
        if let mode, mode.modeChangeCount == modeChangeCount {
            guard absWidth == mode.width, absHeight == mode.height else {
                throw InputProtocolError.malformed(
                    "MOTION abs size \(absWidth)x\(absHeight) differs from the mode \(mode.width)x\(mode.height)")
            }
            return .keep(value)
        }
        value.clearAbsolute()
        return value.hasRelativePart ? .keep(value) : .dropEmpty
    }
}

struct InputStatus: Equatable {
    var rejectedDowns: UInt64 = 0
    var mergedMotions: UInt64 = 0
    var droppedUnfocused: UInt64 = 0
    var queueDepth: UInt32 = 0
}

enum InputMessage: Equatable {
    /// Encoded by VMHost only (answer to a HELLO of another version); never decoded (VERSION with version 1 is an error).
    case version
    case hello
    case accept(flags: UInt32)
    case focus(gained: Bool)
    case usage(InputUsage)
    case motion(InputMotion)
    case status(InputStatus)

    var kind: InputKind {
        switch self {
        case .version: return .version
        case .hello: return .hello
        case .accept: return .accept
        case .focus: return .focus
        case .usage: return .usage
        case .motion: return .motion
        case .status: return .status
        }
    }

    func encoded() -> [UInt8] {
        var out = InputProtocol.magic
        out.inAppendLE(InputProtocol.version)
        out.inAppendLE(kind.rawValue)
        out.inAppendLE(UInt32(0))
        out.inAppendLE(UInt32(kind.payloadLength))
        switch self {
        case .version, .hello:
            break
        case let .accept(flags):
            out.inAppendLE(flags)
            out.inAppendLE(UInt32(0))
        case let .focus(gained):
            out.inAppendLE(UInt32(gained ? 1 : 0))
            out.inAppendLE(UInt32(0))
        case let .usage(usage):
            out.inAppendLE(usage.page)
            out.inAppendLE(usage.id)
            out.inAppendLE(UInt32(usage.down ? 1 : 0))
        case let .motion(motion):
            out.inAppendLE(motion.modeChangeCount)
            out.inAppendLE(motion.dx)
            out.inAppendLE(motion.dy)
            out.inAppendLE(motion.wheel)
            out.inAppendLE(motion.pan)
            out.inAppendLE(motion.absX)
            out.inAppendLE(motion.absY)
            out.inAppendLE(motion.absWidth)
            out.inAppendLE(motion.absHeight)
            out.inAppendLE(UInt32(motion.absValid ? 1 : 0))
            out.inAppendLE(UInt32(0))
        case let .status(status):
            out.inAppendLE(status.rejectedDowns)
            out.inAppendLE(status.mergedMotions)
            out.inAppendLE(status.droppedUnfocused)
            out.inAppendLE(status.queueDepth)
            out.inAppendLE(UInt32(0))
        }
        return out
    }
}

/// Stream decoder: the header is checked as soon as its 16 bytes are there, before any payload (section 4.1);
/// at most one incomplete message is buffered.
struct InputDecoder {
    let direction: InputDirection
    private var buffer: [UInt8] = []
    /// When set, the first message must be of this kind; a header of another kind is an error as soon as its 16 bytes are
    /// there (VMHost sets `.hello`: section 8.2 item 1 decides by the header, without waiting for a payload).
    var requiredFirstKind: InputKind?

    init(direction: InputDirection, requiredFirstKind: InputKind? = nil) {
        self.direction = direction
        self.requiredFirstKind = requiredFirstKind
    }

    var bufferedByteCount: Int { buffer.count }

    /// Header check; returns the kind. Order: magic, version (a foreign version stops all other checks), kind, direction,
    /// reserved, payload length.
    static func parseHeader(_ bytes: [UInt8], direction: InputDirection) throws -> InputKind {
        guard bytes.count >= InputProtocol.headerLength else { throw InputProtocolError.malformed("short header") }
        guard Array(bytes[0..<4]) == InputProtocol.magic else { throw InputProtocolError.badMagic }
        let version = bytes.inReadLE(UInt16.self, at: 4)
        guard version == InputProtocol.version else { throw InputProtocolError.incompatibleVersion(version) }
        let rawKind = bytes.inReadLE(UInt16.self, at: 6)
        guard let kind = InputKind(rawValue: rawKind) else { throw InputProtocolError.malformed("unknown kind \(rawKind)") }
        switch direction {
        case .uiToHost:
            guard kind.sentByUI else { throw InputProtocolError.malformed("kind \(kind) is not sent by the UI") }
        case .hostToUI:
            guard !kind.sentByUI else { throw InputProtocolError.malformed("kind \(kind) is not sent by VMHost") }
            // VERSION carries the sender's version; with version 1 it is an error (section 9.1).
            guard kind != .version else { throw InputProtocolError.malformed("VERSION with version 1") }
        }
        guard bytes.inReadLE(UInt32.self, at: 8) == 0 else { throw InputProtocolError.malformed("reserved is not 0") }
        guard Int(bytes.inReadLE(UInt32.self, at: 12)) == kind.payloadLength else {
            throw InputProtocolError.malformed("payload_length does not match kind \(kind)")
        }
        return kind
    }

    /// Feeds bytes; returns the complete messages. Throws on the first violation; the decoder is then unusable.
    mutating func append(_ bytes: [UInt8]) throws -> [InputMessage] {
        buffer.append(contentsOf: bytes)
        var messages: [InputMessage] = []
        while buffer.count >= InputProtocol.headerLength {
            let kind = try InputDecoder.parseHeader(Array(buffer[0..<InputProtocol.headerLength]), direction: direction)
            if let required = requiredFirstKind, kind != required {
                throw InputProtocolError.malformed("first message is \(kind), not \(required)")
            }
            guard buffer.count >= kind.totalLength else { break }
            let payload = Array(buffer[InputProtocol.headerLength..<kind.totalLength])
            buffer.removeFirst(kind.totalLength)
            requiredFirstKind = nil
            messages.append(try InputDecoder.decodePayload(kind, payload))
        }
        return messages
    }

    static func decodePayload(_ kind: InputKind, _ p: [UInt8]) throws -> InputMessage {
        switch kind {
        case .version:
            throw InputProtocolError.malformed("VERSION is not decoded")
        case .hello:
            return .hello
        case .accept:
            let flags = p.inReadLE(UInt32.self, at: 0)
            guard flags & ~InputProtocol.acceptGuestDeliveryFlag == 0 else { throw InputProtocolError.malformed("ACCEPT flags") }
            guard p.inReadLE(UInt32.self, at: 4) == 0 else { throw InputProtocolError.malformed("ACCEPT reserved") }
            return .accept(flags: flags)
        case .focus:
            let state = p.inReadLE(UInt32.self, at: 0)
            guard state <= 1 else { throw InputProtocolError.malformed("FOCUS state \(state)") }
            guard p.inReadLE(UInt32.self, at: 4) == 0 else { throw InputProtocolError.malformed("FOCUS reserved") }
            return .focus(gained: state == 1)
        case .usage:
            let page = p.inReadLE(UInt16.self, at: 0)
            let id = p.inReadLE(UInt16.self, at: 2)
            let state = p.inReadLE(UInt32.self, at: 4)
            guard state <= 1 else { throw InputProtocolError.malformed("USAGE state \(state)") }
            guard InputUsage.isValid(page: page, id: id) else {
                throw InputProtocolError.malformed("USAGE page \(page) id \(id) is not allowed (section 6.1)")
            }
            return .usage(InputUsage(page: page, id: id, down: state == 1))
        case .motion:
            var motion = InputMotion()
            motion.modeChangeCount = p.inReadLE(UInt64.self, at: 0)
            motion.dx = Int32(bitPattern: p.inReadLE(UInt32.self, at: 8))
            motion.dy = Int32(bitPattern: p.inReadLE(UInt32.self, at: 12))
            motion.wheel = Int32(bitPattern: p.inReadLE(UInt32.self, at: 16))
            motion.pan = Int32(bitPattern: p.inReadLE(UInt32.self, at: 20))
            motion.absX = p.inReadLE(UInt32.self, at: 24)
            motion.absY = p.inReadLE(UInt32.self, at: 28)
            motion.absWidth = p.inReadLE(UInt32.self, at: 32)
            motion.absHeight = p.inReadLE(UInt32.self, at: 36)
            let flags = p.inReadLE(UInt32.self, at: 40)
            guard flags & ~UInt32(1) == 0 else { throw InputProtocolError.malformed("MOTION flags") }
            guard p.inReadLE(UInt32.self, at: 44) == 0 else { throw InputProtocolError.malformed("MOTION reserved") }
            motion.absValid = flags & 1 == 1
            try motion.validate()
            return .motion(motion)
        case .status:
            var status = InputStatus()
            status.rejectedDowns = p.inReadLE(UInt64.self, at: 0)
            status.mergedMotions = p.inReadLE(UInt64.self, at: 8)
            status.droppedUnfocused = p.inReadLE(UInt64.self, at: 16)
            status.queueDepth = p.inReadLE(UInt32.self, at: 24)
            guard p.inReadLE(UInt32.self, at: 28) == 0 else { throw InputProtocolError.malformed("STATUS reserved") }
            return .status(status)
        }
    }
}

// MARK: Byte helpers

extension Array where Element == UInt8 {
    mutating func inAppendLE<T: FixedWidthInteger>(_ value: T) {
        var le = value.littleEndian
        Swift.withUnsafeBytes(of: &le) { append(contentsOf: $0) }
    }

    func inReadLE<T: FixedWidthInteger>(_ type: T.Type, at offset: Int) -> T {
        var value: T = 0
        for index in 0..<MemoryLayout<T>.size { value |= T(truncatingIfNeeded: self[offset + index]) << T(8 * index) }
        return value
    }
}

// MARK: Socket helpers (sections 2.2, 4.1)

/// Non-blocking socket access shared by both ends: `recvmsg` with an ancillary buffer (any descriptor or `cmsghdr` is a
/// protocol error and a received descriptor is closed at once), `send` that never raises SIGPIPE.
enum InputSocket {
    enum ReadResult: Equatable {
        case data(Int)
        case wouldBlock
        case eof
        case error(Int32)
        /// Control data or `MSG_CTRUNC`: protocol error; descriptors were closed.
        case ancillary
    }

    enum WriteResult: Equatable {
        case written(Int)
        case wouldBlock
        case error(Int32)
    }

    static func makeNonBlocking(_ fd: Int32) -> Bool {
        let flags = fcntl(fd, F_GETFL)
        return flags >= 0 && fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0
    }

    static func setNoSigpipe(_ fd: Int32) -> Bool {
        var one: Int32 = 1
        return setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size)) == 0
    }

    static func receive(fd: Int32, into buffer: inout [UInt8], maxBytes: Int) -> ReadResult {
        var control = [UInt8](repeating: 0, count: 64)
        var received = 0
        var controlLength = 0
        var flags: Int32 = 0
        var failure: Int32 = 0
        buffer.withUnsafeMutableBytes { dataRaw in
            control.withUnsafeMutableBytes { controlRaw in
                var iov = iovec(iov_base: dataRaw.baseAddress, iov_len: min(maxBytes, dataRaw.count))
                withUnsafeMutablePointer(to: &iov) { iovPointer in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = controlRaw.baseAddress
                    header.msg_controllen = socklen_t(controlRaw.count)
                    while true {
                        let n = Darwin.recvmsg(fd, &header, 0)
                        if n < 0 && errno == EINTR { continue }
                        if n < 0 { failure = errno } else { received = n }
                        break
                    }
                    controlLength = Int(header.msg_controllen)
                    flags = header.msg_flags
                }
            }
        }
        if failure != 0 {
            return failure == EAGAIN || failure == EWOULDBLOCK ? .wouldBlock : .error(failure)
        }
        if controlLength > 0 || flags & MSG_CTRUNC != 0 {
            closeReceivedDescriptors(control, length: controlLength)
            return .ancillary
        }
        return received == 0 ? .eof : .data(received)
    }

    /// Closes every descriptor of an SCM_RIGHTS message (Darwin cmsghdr: len u32, level i32, type i32, data at 12).
    private static func closeReceivedDescriptors(_ control: [UInt8], length: Int) {
        var offset = 0
        while offset + 12 <= min(length, control.count) {
            let len = Int(control.inReadLE(UInt32.self, at: offset))
            guard len >= 12, offset + len <= control.count else { return }
            let level = Int32(bitPattern: control.inReadLE(UInt32.self, at: offset + 4))
            let type = Int32(bitPattern: control.inReadLE(UInt32.self, at: offset + 8))
            if level == SOL_SOCKET, type == SCM_RIGHTS {
                var cursor = offset + 12
                while cursor + 4 <= offset + len {
                    let fd = Int32(bitPattern: control.inReadLE(UInt32.self, at: cursor))
                    if fd >= 0 { Darwin.close(fd) }
                    cursor += 4
                }
            }
            offset += (len + 3) & ~3
        }
    }

    static func send(fd: Int32, _ bytes: [UInt8]) -> WriteResult {
        while true {
            let n = bytes.withUnsafeBytes { Darwin.send(fd, $0.baseAddress, $0.count, 0) }
            if n >= 0 { return .written(n) }
            if errno == EINTR { continue }
            if errno == EAGAIN || errno == EWOULDBLOCK { return .wouldBlock }
            return .error(errno)
        }
    }
}
