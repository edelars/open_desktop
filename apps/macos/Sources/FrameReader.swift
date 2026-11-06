import Darwin
import Foundation

// Reader (UI) side of frame transport v2: protocols/control/FRAME_TRANSPORT_V1.md (section 10 is the cursor, S10-14).
//
// Receives ANNOUNCE with SCM_RIGHTS (recvmsg only), checks fstat before mmap, maps the header page
// read-write and the slot data read-only, and reads slots with the seqlock protocol of section 5.2:
// every descriptor field is read exactly once into a local copy, all checks and the copy length use
// only those locals, and the frame is accepted only when the generation is unchanged after the copy.
//
// S10-14: the CURSOR notification is read the same way from the separate cursor area (section 10.6): own descriptor
// seqlock word `cursor_seq`, `cursor_claim`, image slots of 16 KiB; the image is copied only when `shape_seq` changes.
//
// Atomics go through the shared C shim apps/vmhost/Sources/FrameTransportAtomics.h (section 5.3),
// imported with -import-objc-header; the header is not copied.

/// Reader states of section 7.2.
enum FrameReaderState: Equatable {
    case connecting
    case noSignal
    case modeChanging
    case showing
    /// EOF, socket error, protocol error, timeout waiting for ANNOUNCE.
    case disconnected(String)
    /// `magic = ODFT` with a foreign version, or an incompatible memory header.
    case incompatible(String)

    var isTerminal: Bool {
        switch self {
        case .disconnected, .incompatible: return true
        default: return false
        }
    }

    var name: String {
        switch self {
        case .connecting: return "CONNECTING"
        case .noSignal: return "NO_SIGNAL"
        case .modeChanging: return "MODE_CHANGING"
        case .showing: return "SHOWING"
        case .disconnected: return "DISCONNECTED"
        case .incompatible: return "INCOMPATIBLE"
        }
    }
}

/// A frame that passed the seqlock check; `pixels` of the copy callback are valid only inside the callback.
struct AcceptedFrame: Equatable {
    let generation: UInt64
    let modeChangeCount: UInt64
    let width: Int
    let height: Int
    let stride: Int
}

/// The cursor state of the last descriptor that passed the seqlock check and the validation (section 10.6).
/// Without a shape (`shapeSeq == 0`) the size, hotspot and `enabled` are zero/false.
struct AcceptedCursor: Equatable {
    let cursorSeq: UInt64
    let enabled: Bool
    let shapeSeq: UInt64
    let width: Int
    let height: Int
    let hotX: Int
    let hotY: Int
    let posX: Int
    let posY: Int
}

/// Called between `g1` and `g2` (section 10.6, step 5) with a pointer to `width * height * 4` bytes of the cursor slot,
/// only when the shape differs from the accepted one. The copy goes to a staging texture that is not used until accepted.
/// Returns false when the copy could not be made (for example no texture could be allocated): the update is then not accepted.
typealias CursorCopy = (_ pixels: UnsafeRawPointer, _ cursor: AcceptedCursor) -> Bool

/// Called between `g1` and `g2` (section 5.2, step 5) with a pointer to `stride * height` bytes of the slot.
/// The copy must go to private storage that is not shown until the reader accepts the frame.
typealias FrameCopy = (_ pixels: UnsafeRawPointer, _ frame: AcceptedFrame) -> Void

private struct ProtocolFailure: Error {
    let reason: String
    init(_ reason: String) { self.reason = reason }
}

private struct IncompatibleFailure: Error {
    let reason: String
    init(_ reason: String) { self.reason = reason }
}

private struct ModeMessage {
    let modeChangeCount: UInt64
    let active: Bool
    let width: Int
    let height: Int
}

final class FrameReader {
    // Constants of FRAME_TRANSPORT_V1.md sections 3 and 3.5.
    private static let granule = 16384
    /// Version 2 (section 10.3): frame slots start at 65536, after the cursor image slots.
    private static let slotsOffset = 65536
    private static let messageVersion: UInt16 = 2
    private static let minTotal = 65536 + 2 * 16384
    private static let maxTotal = 65536 + 4 * 37_748_736
    private static let slotSizeMax = 37_748_736
    private static let pixelFormatBGRX8888: UInt32 = 1
    private static let readerClaimOffset = 64
    private static let cursorClaimOffset = 68
    private static let descriptorsOffset = 128
    private static let descriptorSize = 64
    // Cursor area (10.3.2, 10.3.3).
    private static let cursorHeaderOffset = 384
    private static let cursorDescriptorsOffset = 448
    private static let cursorSlotsOffset = 16384
    private static let cursorSlotSize = 16384
    private static let cursorSlotCount = 3
    private static let cursorMaxSide = 64
    private static let cursorFormatBGRA8888Premul: UInt32 = 2

    private static let kindVersion: UInt16 = 0
    private static let kindHello: UInt16 = 1
    private static let kindAnnounce: UInt16 = 2
    private static let kindMode: UInt16 = 3
    private static let kindFrame: UInt16 = 4
    private static let kindCursor: UInt16 = 5
    private static let payloadLength: [UInt16: Int] = [2: 16, 3: 32, 4: 24, 5: 16]

    private var fd: Int32
    private(set) var state: FrameReaderState = .connecting
    /// `shown_once` of section 7.2: true after the first entry into `showing`, never reset.
    private(set) var shownOnce = false
    /// The last accepted frame; nil until the first `showing`.
    private(set) var lastFrame: AcceptedFrame?
    /// Incremented on every accepted frame; the UI compares it to know that a new frame is ready.
    private(set) var acceptedCount = 0
    /// The last accepted MODE, for the UI.
    private(set) var modeWidth = 0
    private(set) var modeHeight = 0
    /// The accepted cursor state (section 10.6); nil until the first accepted CURSOR and after the connection ends.
    /// It is kept in every reader state; the UI draws it only while `showing`.
    private(set) var cursor: AcceptedCursor?
    /// Incremented on every accepted cursor update.
    private(set) var cursorAcceptedCount = 0

    // Inbound byte stream and descriptor handling (section 4.3).
    private var inbound = [UInt8]()
    private var firstReceive = true
    private var announced = false
    private var announceDeadline: UInt64 = 0

    // Mappings, valid after a successful 7.1.
    private var headerMap: UnsafeMutableRawPointer?
    private var dataMap: UnsafeMutableRawPointer?
    private var dataMapLength = 0

    // Private copy of the validated header (section 3.1).
    private var slotCount = 0
    private var slotSize = 0
    private var maxWidth = 0
    private var maxHeight = 0

    private var lastMode: ModeMessage?
    private var lastFrameNotificationGeneration: UInt64 = 0
    private var sawFrameNotification = false
    private var lastCursorNotificationSeq: UInt64 = 0
    private var sawCursorNotification = false

    /// `fd` is the UI end of the socketpair; the reader owns it and closes it in `teardown()`.
    init(fd: Int32) {
        self.fd = fd
    }

    deinit { teardown() }

    // MARK: Connection

    /// Section 7.1 step 1: send HELLO. `announceTimeoutMs` bounds the wait for ANNOUNCE.
    func start(announceTimeoutMs: UInt64 = 10_000) {
        guard 16384 % getpagesize() == 0 else {
            fail(.disconnected("host page size \(getpagesize()) does not divide 16384"))
            return
        }
        announceDeadline = FrameReader.nowMs() + announceTimeoutMs
        var hello = [UInt8]()
        hello.append(contentsOf: Array("ODFT".utf8))
        FrameReader.appendLE(&hello, FrameReader.messageVersion)
        FrameReader.appendLE(&hello, FrameReader.kindHello)
        FrameReader.appendLE(&hello, UInt32(0))
        FrameReader.appendLE(&hello, UInt32(0))
        var sent = 0
        while sent < hello.count {
            let n = hello.withUnsafeBytes { raw in
                Darwin.send(fd, raw.baseAddress! + sent, hello.count - sent, 0)
            }
            if n > 0 { sent += n; continue }
            if n < 0 && errno == EINTR { continue }
            fail(.disconnected("could not send HELLO (errno \(errno))"))
            return
        }
    }

    /// Releases both mappings and the socket. The reader is terminal afterwards.
    func teardown() {
        if let headerMap { munmap(headerMap, FrameReader.granule) }
        if let dataMap { munmap(dataMap, dataMapLength) }
        headerMap = nil
        dataMap = nil
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
    }

    // MARK: Pump

    /// Waits up to `timeoutMs` for the socket, processes every complete message and reads the slot of the last
    /// pending FRAME. Returns true when the state or the shown frame changed. Single-threaded: call from one thread.
    @discardableResult
    func pump(timeoutMs: Int32, copy: FrameCopy, cursorCopy: CursorCopy = { _, _ in true }) -> Bool {
        if state.isTerminal || fd < 0 { return false }
        let before = state
        let beforeFrame = lastFrame
        let beforeCursor = cursor
        if state == .connecting {
            let now = FrameReader.nowMs()
            if now >= announceDeadline {
                fail(.disconnected("timeout waiting for ANNOUNCE"))
                return true
            }
        }
        var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let ready = Darwin.poll(&descriptor, 1, timeoutMs)
        if ready < 0 {
            if errno != EINTR { fail(.disconnected("poll failed (errno \(errno))")) }
            return state != before
        }
        if ready == 0 {
            return false
        }
        // POLLHUP with pending data still reads the data first; EOF is seen by recvmsg.
        do {
            try receiveAvailable()
            let pending = try processMessages()
            if let frame = pending.frame {
                try readSlot(frame, copy: copy)
            }
            if let notification = pending.cursor {
                try readCursor(notification, copy: cursorCopy)
            }
        } catch let failure as ProtocolFailure {
            fail(.disconnected("protocol error: \(failure.reason)"))
        } catch let failure as IncompatibleFailure {
            fail(.incompatible(failure.reason))
        } catch let failure as ConnectionFailure {
            fail(.disconnected(failure.reason))
        } catch {
            fail(.disconnected("\(error)"))
        }
        return state != before || lastFrame != beforeFrame || cursor != beforeCursor
    }

    // MARK: Receive (section 4.3)

    private struct ConnectionFailure: Error {
        let reason: String
    }

    private func receiveAvailable() throws {
        var data = [UInt8](repeating: 0, count: 4096)
        // cmsghdr on Darwin: len u32, level i32, type i32, data at offset 12; CMSG_SPACE(4) = 16.
        var control = [UInt8](repeating: 0, count: 16)
        var received = 0
        var controlLength = 0
        var flags: Int32 = 0
        var failedErrno: Int32 = 0
        data.withUnsafeMutableBytes { dataRaw in
            control.withUnsafeMutableBytes { controlRaw in
                var iov = iovec(iov_base: dataRaw.baseAddress, iov_len: dataRaw.count)
                withUnsafeMutablePointer(to: &iov) { iovPointer in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = controlRaw.baseAddress
                    header.msg_controllen = socklen_t(controlRaw.count)
                    while true {
                        let n = Darwin.recvmsg(fd, &header, 0)
                        if n < 0 && errno == EINTR { continue }
                        if n < 0 { failedErrno = errno }
                        received = n
                        break
                    }
                    controlLength = Int(header.msg_controllen)
                    flags = header.msg_flags
                }
            }
        }
        if received < 0 { throw ConnectionFailure(reason: "recvmsg failed (errno \(failedErrno))") }

        // Collect descriptors first and mark them close-on-exec before anything else (section 4.3).
        var descriptors: [Int32] = []
        var ancillaryError: String?
        var offset = 0
        let controlEnd = min(controlLength, control.count) // msg_controllen is the real extent of the ancillary data
        while offset + 12 <= controlEnd {
            let length = Int(control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: UInt32.self) })
            let level = control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset + 4, as: Int32.self) }
            let type = control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset + 8, as: Int32.self) }
            if length < 12 || offset + length > controlEnd {
                ancillaryError = "malformed control message"
                break
            }
            if level == SOL_SOCKET && type == SCM_RIGHTS {
                var cursor = offset + 12
                while cursor + 4 <= offset + length {
                    descriptors.append(control.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: cursor, as: Int32.self) })
                    cursor += 4
                }
            } else {
                ancillaryError = "unexpected control message level \(level) type \(type)"
            }
            offset += (length + 3) & ~3
        }
        for descriptor in descriptors {
            _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC)
        }
        func closeAll() { for descriptor in descriptors { Darwin.close(descriptor) } }

        if flags & MSG_CTRUNC != 0 {
            closeAll()
            throw ProtocolFailure("MSG_CTRUNC: more descriptors than expected")
        }
        if let ancillaryError {
            closeAll()
            throw ProtocolFailure(ancillaryError)
        }
        if received == 0 {
            closeAll()
            throw ConnectionFailure(reason: "VMHost closed the connection")
        }
        // The descriptor is accepted only in the recvmsg that returned the first byte of the stream.
        let wasFirst = firstReceive
        firstReceive = false
        if !descriptors.isEmpty && !(wasFirst && !announced && pendingDescriptor == nil) {
            closeAll()
            throw ProtocolFailure("descriptor received outside the first recvmsg of the stream")
        }
        if descriptors.count > 1 {
            closeAll()
            throw ProtocolFailure("more than one descriptor")
        }
        if let first = descriptors.first { pendingDescriptor = first }
        inbound.append(contentsOf: data[0..<received])
    }

    private var pendingDescriptor: Int32?

    // MARK: Messages (sections 4.1, 4.2, 7.1, 7.3)

    private struct PendingNotifications {
        var frame: (generation: UInt64, modeChangeCount: UInt64, slot: Int)?
        var cursor: (seq: UInt64, slot: Int)?
    }

    /// Processes complete messages; returns the last FRAME and the last CURSOR notification that still need a slot read.
    /// A MODE drops an unread FRAME of the old mode; it does not drop an unread CURSOR (section 10.6).
    private func processMessages() throws -> PendingNotifications {
        var pending = PendingNotifications()
        while true {
            // Bytes 0-5 first: magic ODFT and version (section 4.1 invariant, 7.1 step 2).
            if inbound.count >= 6 {
                guard inbound[0] == 0x4f, inbound[1] == 0x44, inbound[2] == 0x46, inbound[3] == 0x54 else {
                    closePendingDescriptor()
                    throw ProtocolFailure("bad message magic")
                }
                let version = UInt16(inbound[4]) | UInt16(inbound[5]) << 8
                if version != FrameReader.messageVersion {
                    closePendingDescriptor()
                    throw IncompatibleFailure("incompatible version: VMHost speaks frame transport v\(version), this viewer v\(FrameReader.messageVersion)")
                }
            }
            guard inbound.count >= 16 else { return pending }
            let kind = UInt16(inbound[6]) | UInt16(inbound[7]) << 8
            let reserved = FrameReader.readLE(UInt32.self, inbound, 8)
            let length = Int(FrameReader.readLE(UInt32.self, inbound, 12))
            guard let expectedLength = FrameReader.payloadLength[kind] else {
                closePendingDescriptor()
                if kind == FrameReader.kindVersion { throw ProtocolFailure("VERSION with version \(FrameReader.messageVersion)") }
                throw ProtocolFailure("unknown or misdirected message kind \(kind)")
            }
            guard reserved == 0, length == expectedLength else {
                closePendingDescriptor()
                throw ProtocolFailure("message kind \(kind) has a bad header (reserved \(reserved), length \(length))")
            }
            guard inbound.count >= 16 + length else { return pending }
            let payload = Array(inbound[16..<(16 + length)])
            inbound.removeFirst(16 + length)

            if !announced {
                guard kind == FrameReader.kindAnnounce else {
                    closePendingDescriptor()
                    throw ProtocolFailure("first message is kind \(kind), not ANNOUNCE")
                }
                try handleAnnounce(payload)
                continue
            }
            // After ANNOUNCE a descriptor never arrives (checked in receiveAvailable); a second ANNOUNCE is an error.
            switch kind {
            case FrameReader.kindAnnounce:
                throw ProtocolFailure("second ANNOUNCE")
            case FrameReader.kindMode:
                try handleMode(payload)
                pending.frame = nil // a MODE removes an unread FRAME of the old mode
            case FrameReader.kindFrame:
                pending.frame = try handleFrame(payload)
            case FrameReader.kindCursor:
                pending.cursor = try handleCursor(payload)
            default:
                throw ProtocolFailure("unexpected kind \(kind)")
            }
        }
    }

    private func closePendingDescriptor() {
        if let descriptor = pendingDescriptor {
            Darwin.close(descriptor)
            pendingDescriptor = nil
        }
    }

    /// Section 7.1 steps 2-5 for the payload of ANNOUNCE.
    private func handleAnnounce(_ payload: [UInt8]) throws {
        guard let descriptor = pendingDescriptor else {
            throw ProtocolFailure("ANNOUNCE without a descriptor")
        }
        pendingDescriptor = nil
        defer { Darwin.close(descriptor) }

        let totalSize = FrameReader.readLE(UInt64.self, payload, 0)
        let announcedSlots = Int(FrameReader.readLE(UInt32.self, payload, 8))
        let announceReserved = FrameReader.readLE(UInt32.self, payload, 12)
        guard announceReserved == 0 else { throw ProtocolFailure("ANNOUNCE reserved is not zero") }
        guard totalSize >= UInt64(FrameReader.minTotal), totalSize <= UInt64(FrameReader.maxTotal),
              totalSize % UInt64(FrameReader.granule) == 0 else {
            throw ProtocolFailure("ANNOUNCE total_size \(totalSize) out of range")
        }
        guard (2...4).contains(announcedSlots) else {
            throw ProtocolFailure("ANNOUNCE slot_count \(announcedSlots) out of range")
        }

        // Step 3: fstat before mmap.
        var info = stat()
        guard fstat(descriptor, &info) == 0 else { throw ProtocolFailure("fstat failed (errno \(errno))") }
        guard info.st_size >= 0, UInt64(info.st_size) == totalSize else {
            throw ProtocolFailure("fstat size \(info.st_size) differs from ANNOUNCE total_size \(totalSize)")
        }
        let total = Int(totalSize)

        // Step 4: header page read-write, data read-only.
        let header = mmap(nil, FrameReader.granule, PROT_READ | PROT_WRITE, MAP_SHARED, descriptor, 0)
        guard let header, header != MAP_FAILED else { throw ProtocolFailure("mmap of the header page failed (errno \(errno))") }
        let dataLength = total - FrameReader.granule // includes the cursor image slots (section 10.3)
        let data = mmap(nil, dataLength, PROT_READ, MAP_SHARED, descriptor, off_t(FrameReader.granule))
        guard let data, data != MAP_FAILED else {
            _ = munmap(header, FrameReader.granule)
            throw ProtocolFailure("mmap of the slot data failed (errno \(errno))")
        }
        headerMap = header
        dataMap = data
        dataMapLength = dataLength
        odft_fence_acquire()
        // The mappings keep the object alive; the descriptor is no longer needed (closed by the defer).

        // Step 5: validate the header in the order of section 3.5; read every field once.
        do {
            try validateHeader(totalSize: totalSize, announcedSlots: announcedSlots)
        } catch {
            _ = munmap(header, FrameReader.granule)
            _ = munmap(data, dataLength)
            headerMap = nil
            dataMap = nil
            throw error
        }
        odft_store_relaxed_u32(
            (header + FrameReader.readerClaimOffset).assumingMemoryBound(to: UInt32.self), 0)
        announced = true
        state = .noSignal
    }

    private func headerU32(_ offset: Int) -> UInt32 {
        odft_load_relaxed_u32((headerMap! + offset).assumingMemoryBound(to: UInt32.self))
    }

    private func headerU64(_ offset: Int) -> UInt64 {
        odft_load_relaxed_u64((headerMap! + offset).assumingMemoryBound(to: UInt64.self))
    }

    private func validateHeader(totalSize: UInt64, announcedSlots: Int) throws {
        // Fields are read once into locals. The identity fields decide INCOMPATIBLE.
        let magic = headerU32(0)
        let versionAndHeaderSize = headerU32(4)
        let version = UInt16(versionAndHeaderSize & 0xFFFF)
        let headerSize = UInt16(versionAndHeaderSize >> 16)
        let readerBlockOffset = headerU32(8)
        let descriptorsOffset = headerU32(12)
        let descriptorSize = headerU32(16)
        let slots = headerU32(20)
        let slotsOffset = headerU32(24)
        let slotSizeValue = headerU32(28)
        let pixelFormat = headerU32(32)
        let maxW = headerU32(36)
        let maxH = headerU32(40)
        let flags = headerU32(44)
        let headerTotal = headerU64(48)
        let cursorOffset = headerU32(56)
        let reserved = headerU32(60)
        // Cursor area header (10.3.2), every field once.
        let cursorHeader = (0..<8).map { headerU32(FrameReader.cursorHeaderOffset + 4 * $0) }
        var cursorReserved: UInt64 = 0
        for index in 0..<4 { cursorReserved |= headerU64(FrameReader.cursorHeaderOffset + 32 + 8 * index) }

        guard magic == 0x5346_444F else { // "ODFS" little-endian
            throw IncompatibleFailure("memory header magic is not ODFS")
        }
        guard version == FrameReader.messageVersion else { throw IncompatibleFailure("memory header version \(version)") }
        guard headerSize == 64, readerBlockOffset == 64, descriptorsOffset == 128, descriptorSize == 64,
              slotsOffset == UInt32(FrameReader.slotsOffset), cursorOffset == UInt32(FrameReader.cursorHeaderOffset) else {
            throw IncompatibleFailure("memory header layout differs from v2")
        }
        let expectedCursorHeader: [UInt32] = [
            UInt32(FrameReader.cursorSlotCount), UInt32(FrameReader.descriptorSize), UInt32(FrameReader.cursorDescriptorsOffset),
            UInt32(FrameReader.cursorSlotsOffset), UInt32(FrameReader.cursorSlotSize), UInt32(FrameReader.cursorMaxSide),
            UInt32(FrameReader.cursorMaxSide), FrameReader.cursorFormatBGRA8888Premul,
        ]
        guard cursorHeader == expectedCursorHeader else {
            throw IncompatibleFailure("cursor area header differs from v2")
        }
        guard pixelFormat == FrameReader.pixelFormatBGRX8888 else {
            throw IncompatibleFailure("memory header pixel_format \(pixelFormat) is not BGRX8888")
        }
        guard flags == 0 else { throw IncompatibleFailure("memory header flags \(flags)") }

        // Section 3.5 order: ranges first, then products, then slot_size, then total_size in 64 bits.
        guard (2...4).contains(Int(slots)) else { throw ProtocolFailure("memory header slot_count \(slots)") }
        guard (1...4096).contains(Int(maxW)) else { throw ProtocolFailure("memory header max_width \(maxW)") }
        guard (1...2304).contains(Int(maxH)) else { throw ProtocolFailure("memory header max_height \(maxH)") }
        let maxFrame = Int(maxW) * 4 * Int(maxH)
        guard maxFrame <= FrameReader.slotSizeMax else { throw ProtocolFailure("max frame size out of range") }
        let slotSize = Int(slotSizeValue)
        guard slotSize % FrameReader.granule == 0, slotSize >= maxFrame, slotSize <= FrameReader.slotSizeMax else {
            throw ProtocolFailure("memory header slot_size \(slotSizeValue) out of range")
        }
        let expectedTotal = UInt64(FrameReader.slotsOffset) + UInt64(slots) * UInt64(slotSize)
        guard headerTotal == expectedTotal else { throw ProtocolFailure("memory header total_size does not match its layout") }
        guard headerTotal == totalSize, Int(slots) == announcedSlots else {
            throw ProtocolFailure("memory header disagrees with ANNOUNCE")
        }
        guard reserved == 0, cursorReserved == 0 else { throw ProtocolFailure("memory header reserved is not zero") }

        slotCount = Int(slots)
        self.slotSize = slotSize
        maxWidth = Int(maxW)
        maxHeight = Int(maxH)
    }

    private func handleMode(_ payload: [UInt8]) throws {
        let modeChangeCount = FrameReader.readLE(UInt64.self, payload, 0)
        let flags = FrameReader.readLE(UInt32.self, payload, 8)
        let width = Int(FrameReader.readLE(UInt32.self, payload, 12))
        let height = Int(FrameReader.readLE(UInt32.self, payload, 16))
        let stride = Int(FrameReader.readLE(UInt32.self, payload, 20))
        let format = FrameReader.readLE(UInt32.self, payload, 24)
        let reserved = FrameReader.readLE(UInt32.self, payload, 28)
        guard reserved == 0, flags & ~UInt32(1) == 0 else { throw ProtocolFailure("MODE flags/reserved") }
        let active = flags & 1 != 0
        if active {
            guard (1...maxWidth).contains(width), (1...maxHeight).contains(height),
                  stride == width * 4, format == FrameReader.pixelFormatBGRX8888 else {
                throw ProtocolFailure("MODE geometry \(width)x\(height) stride \(stride) format \(format) is invalid")
            }
        } else {
            guard width == 0, height == 0, stride == 0, format == 0 else {
                throw ProtocolFailure("inactive MODE with non-zero geometry")
            }
        }
        if let previous = lastMode, modeChangeCount <= previous.modeChangeCount {
            throw ProtocolFailure("MODE mode_change_count \(modeChangeCount) does not grow")
        }
        lastMode = ModeMessage(modeChangeCount: modeChangeCount, active: active, width: width, height: height)
        modeWidth = width
        modeHeight = height
        // Section 7.2: no frame of the new mode yet.
        lastFrame = nil
        if !active {
            state = .noSignal
        } else {
            state = shownOnce ? .modeChanging : .noSignal
        }
    }

    private func handleFrame(_ payload: [UInt8]) throws -> (generation: UInt64, modeChangeCount: UInt64, slot: Int) {
        let generation = FrameReader.readLE(UInt64.self, payload, 0)
        let modeChangeCount = FrameReader.readLE(UInt64.self, payload, 8)
        let slot = Int(FrameReader.readLE(UInt32.self, payload, 16))
        let reserved = FrameReader.readLE(UInt32.self, payload, 20)
        guard reserved == 0 else { throw ProtocolFailure("FRAME reserved is not zero") }
        guard generation > 0 else { throw ProtocolFailure("FRAME generation is 0") }
        if sawFrameNotification {
            guard generation > lastFrameNotificationGeneration else {
                throw ProtocolFailure("FRAME generation \(generation) does not grow")
            }
        }
        guard let mode = lastMode else { throw ProtocolFailure("FRAME before any MODE") }
        guard mode.active else { throw ProtocolFailure("FRAME while the mode is inactive") }
        guard modeChangeCount == mode.modeChangeCount else {
            throw ProtocolFailure("FRAME mode_change_count differs from the last MODE")
        }
        guard slot < slotCount else { throw ProtocolFailure("FRAME slot_index \(slot) out of range") }
        sawFrameNotification = true
        lastFrameNotificationGeneration = generation
        return (generation, modeChangeCount, slot)
    }

    private func handleCursor(_ payload: [UInt8]) throws -> (seq: UInt64, slot: Int) {
        let seq = FrameReader.readLE(UInt64.self, payload, 0)
        let slot = Int(FrameReader.readLE(UInt32.self, payload, 8))
        let reserved = FrameReader.readLE(UInt32.self, payload, 12)
        guard reserved == 0 else { throw ProtocolFailure("CURSOR reserved is not zero") }
        guard seq > 0 else { throw ProtocolFailure("CURSOR cursor_seq is 0") }
        if sawCursorNotification {
            guard seq > lastCursorNotificationSeq else {
                throw ProtocolFailure("CURSOR cursor_seq \(seq) does not grow")
            }
        }
        guard slot < FrameReader.cursorSlotCount else { throw ProtocolFailure("CURSOR slot_index \(slot) out of range") }
        sawCursorNotification = true
        lastCursorNotificationSeq = seq
        return (seq, slot)
    }

    // MARK: Cursor read (section 10.6)

    private func readCursor(_ notification: (seq: UInt64, slot: Int), copy: CursorCopy) throws {
        guard let header = headerMap, let data = dataMap else { return }
        let slot = notification.slot
        let n = notification.seq
        let descriptor = header + FrameReader.cursorDescriptorsOffset + FrameReader.descriptorSize * slot
        let claim = (header + FrameReader.cursorClaimOffset).assumingMemoryBound(to: UInt32.self)
        let seqPointer = descriptor.assumingMemoryBound(to: UInt64.self)

        // 1. claim the cursor slot (a hint, not a lock).
        odft_store_relaxed_u32(claim, UInt32(slot + 1))
        defer { odft_store_relaxed_u32(claim, 0) } // 7. release the claim

        // 2. g1 with acquire.
        let g1 = odft_load_acquire_u64(seqPointer)
        if g1 != n {
            if g1 != 0 && g1 < n {
                throw ProtocolFailure("cursor slot cursor_seq \(g1) is older than the announced \(n)")
            }
            return // rewritten or being written: discard
        }

        // 3. every descriptor field exactly once into locals.
        func u32(_ offset: Int) -> UInt32 { odft_load_relaxed_u32((descriptor + offset).assumingMemoryBound(to: UInt32.self)) }
        func u64(_ offset: Int) -> UInt64 { odft_load_relaxed_u64((descriptor + offset).assumingMemoryBound(to: UInt64.self)) }
        let shapeSeq = u64(8)
        let flags = u32(16)
        let widthValue = u32(20)
        let heightValue = u32(24)
        let formatValue = u32(28)
        let hotXValue = u32(32)
        let hotYValue = u32(36)
        let posXValue = u32(40)
        let posYValue = u32(44)
        let reservedA = u64(48)
        let reservedB = u64(56)

        // 4. check the locals (width and height before any multiplication; the position against the frame limits
        // of the memory header, not against the 64 pixel image limit).
        let width = Int(widthValue)
        let height = Int(heightValue)
        var valid = (flags == 0 || flags == 1) && reservedA == 0 && reservedB == 0
            && Int(posXValue) < maxWidth && Int(posYValue) < maxHeight
        if shapeSeq == 0 {
            valid = valid && flags == 0 && widthValue == 0 && heightValue == 0 && formatValue == 0
                && hotXValue == 0 && hotYValue == 0
        } else {
            valid = valid && width >= 1 && width <= FrameReader.cursorMaxSide
                && height >= 1 && height <= FrameReader.cursorMaxSide
                && formatValue == FrameReader.cursorFormatBGRA8888Premul
                && Int(hotXValue) < width && Int(hotYValue) < height
        }

        var accepted: AcceptedCursor?
        if valid {
            let candidate = AcceptedCursor(
                cursorSeq: g1, enabled: flags == 1, shapeSeq: shapeSeq, width: width, height: height,
                hotX: Int(hotXValue), hotY: Int(hotYValue), posX: Int(posXValue), posY: Int(posYValue))
            // 5. the image is copied only when the shape differs from the accepted one; not shown before step 6.
            if shapeSeq != 0 && shapeSeq != cursor?.shapeSeq {
                let slotBase = UnsafeRawPointer(data + (FrameReader.cursorSlotsOffset - FrameReader.granule)
                    + slot * FrameReader.cursorSlotSize)
                if copy(slotBase, candidate) {
                    accepted = candidate
                } // else: no image copy, the shape is not taken as accepted; the next CURSOR repeats the copy
            } else {
                accepted = candidate
            }
        }
        // 6. fence(acquire), then g2 with a relaxed load.
        odft_fence_acquire()
        let g2 = odft_load_relaxed_u64(seqPointer)
        if g2 != g1 { return } // torn or overwritten: discard, the accepted state stays
        guard let candidate = accepted else {
            if valid { return } // the copy failed: not a protocol error, the accepted state stays
            throw ProtocolFailure("cursor descriptor violates the protocol while cursor_seq is stable")
        }
        cursor = candidate
        cursorAcceptedCount += 1
    }

    // MARK: Slot read (section 5.2)

    private func readSlot(_ notification: (generation: UInt64, modeChangeCount: UInt64, slot: Int), copy: FrameCopy) throws {
        guard let header = headerMap, let data = dataMap, let mode = lastMode else { return }
        let slot = notification.slot
        let n = notification.generation
        let descriptor = header + FrameReader.descriptorsOffset + FrameReader.descriptorSize * slot
        let claim = (header + FrameReader.readerClaimOffset).assumingMemoryBound(to: UInt32.self)
        let generationPointer = descriptor.assumingMemoryBound(to: UInt64.self)

        // 1. claim the slot (a hint to the writer, not a lock).
        odft_store_relaxed_u32(claim, UInt32(slot + 1))
        defer { odft_store_relaxed_u32(claim, 0) } // 7. release the claim

        // 2. g1 with acquire.
        let g1 = odft_load_acquire_u64(generationPointer)
        if g1 != n {
            if g1 != 0 && g1 < n {
                throw ProtocolFailure("slot generation \(g1) is older than the announced \(n)")
            }
            return // rewritten or being written: discard
        }

        // 3. every descriptor field exactly once into locals.
        let mcc = odft_load_relaxed_u64((descriptor + 8).assumingMemoryBound(to: UInt64.self))
        let widthValue = odft_load_relaxed_u32((descriptor + 16).assumingMemoryBound(to: UInt32.self))
        let heightValue = odft_load_relaxed_u32((descriptor + 20).assumingMemoryBound(to: UInt32.self))
        let strideValue = odft_load_relaxed_u32((descriptor + 24).assumingMemoryBound(to: UInt32.self))
        let formatValue = odft_load_relaxed_u32((descriptor + 28).assumingMemoryBound(to: UInt32.self))
        let stateValue = odft_load_relaxed_u32((descriptor + 32).assumingMemoryBound(to: UInt32.self))
        let reserved0 = odft_load_relaxed_u32((descriptor + 36).assumingMemoryBound(to: UInt32.self))
        let reserved1a = odft_load_relaxed_u64((descriptor + 40).assumingMemoryBound(to: UInt64.self))
        let reserved1b = odft_load_relaxed_u64((descriptor + 48).assumingMemoryBound(to: UInt64.self))
        let reserved1c = odft_load_relaxed_u64((descriptor + 56).assumingMemoryBound(to: UInt64.self))

        // 4. check the locals (width and height against the maxima before any multiplication).
        let width = Int(widthValue)
        let height = Int(heightValue)
        let stride = Int(strideValue)
        let valid = width >= 1 && width <= maxWidth && height >= 1 && height <= maxHeight
            && stride == width * 4 && stride * height <= slotSize
            && formatValue == FrameReader.pixelFormatBGRX8888 && stateValue == 1
            && reserved0 == 0 && reserved1a == 0 && reserved1b == 0 && reserved1c == 0
            && mcc == notification.modeChangeCount
            && mode.active && width == mode.width && height == mode.height

        var accepted: AcceptedFrame?
        if valid {
            let frame = AcceptedFrame(
                generation: g1, modeChangeCount: mcc, width: width, height: height, stride: stride)
            // 5. the copy uses only the local values; the frame is not shown yet.
            let slotBase = UnsafeRawPointer(data + (FrameReader.slotsOffset - FrameReader.granule) + slot * slotSize)
            copy(slotBase, frame)
            accepted = frame
        }
        // 6. fence(acquire), then g2 with a relaxed load.
        odft_fence_acquire()
        let g2 = odft_load_relaxed_u64(generationPointer)
        if g2 != g1 { return } // torn or overwritten: discard
        guard let frame = accepted else {
            throw ProtocolFailure("slot descriptor violates the protocol while the generation is stable")
        }
        lastFrame = frame
        acceptedCount += 1
        shownOnce = true
        state = .showing
    }

    // MARK: Helpers

    private func fail(_ newState: FrameReaderState) {
        state = newState
        cursor = nil // the accepted cursor state dies with the connection (10.6)
        closePendingDescriptor()
        teardown()
    }

    private static func nowMs() -> UInt64 {
        DispatchTime.now().uptimeNanoseconds / 1_000_000
    }

    private static func readLE<T: FixedWidthInteger>(_ type: T.Type, _ bytes: [UInt8], _ offset: Int) -> T {
        var value: T = 0
        for index in 0..<MemoryLayout<T>.size {
            value |= T(truncatingIfNeeded: bytes[offset + index]) << T(index * 8)
        }
        return value
    }

    private static func appendLE<T: FixedWidthInteger>(_ bytes: inout [UInt8], _ value: T) {
        for index in 0..<MemoryLayout<T>.size {
            bytes.append(UInt8(truncatingIfNeeded: value >> T(index * 8)))
        }
    }
}
