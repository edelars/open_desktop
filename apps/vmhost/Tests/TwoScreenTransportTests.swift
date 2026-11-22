import Darwin
import Foundation
import ODAtomics
import ODHostABI
import XCTest
import ODShared
@testable import od_vmhost

// S10-15 review: tests of frame transport v3 with two screens (protocols/control/FRAME_TRANSPORT_V1.md, section 11.10, items 1-7,
// and the separation of connection and screen states of 11.7). The real writer (apps/vmhost/Sources/FrameTransport.swift) against the
// real reader (apps/macos/Sources/Shared/FrameReader.swift) in one process over a socketpair, plus hand-built messages where the test plays
// a faulty writer. A synthetic writer (frames not taken from a device) is the internal `publishFrame(mode:read:)` of the writer.
// Frames of profile 4 come from the device through the display ABI and are compared with the reference files of
// tests/fixtures/guest/stage10 (test-only, no guest: DISPLAY_HOST_ABI_V1.md rule 0). From apps/ (the Rust staticlib first,
// `cargo build -p od-host --offline` from the repository root):
//
//   swift test --filter TwoScreenTransportXCTests

private func socketPair() -> (host: Int32, ui: Int32) {
    var fds: [Int32] = [0, 0]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    return (fds[0], fds[1])
}

private func le16(_ value: UInt16) -> [UInt8] { [UInt8(truncatingIfNeeded: value), UInt8(truncatingIfNeeded: value >> 8)] }
private func le32(_ value: UInt32) -> [UInt8] { (0..<4).map { UInt8(truncatingIfNeeded: value >> UInt32(8 * $0)) } }
private func le64(_ value: UInt64) -> [UInt8] { (0..<8).map { UInt8(truncatingIfNeeded: value >> UInt64(8 * $0)) } }

private let repositoryRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

private func fixture(_ name: String) -> [UInt8] {
    let url = repositoryRoot.appendingPathComponent("tests/fixtures/guest/stage10/" + name)
    return Array(try! Data(contentsOf: url))
}

// MARK: messages of section 11.4, built by hand (the test is the faulty writer)

private func header(kind: UInt16, length: UInt32, version: UInt16 = 3) -> [UInt8] {
    Array("ODFT".utf8) + le16(version) + le16(kind) + le32(0) + le32(length)
}

private func announceMessage(total: Int, slots: UInt32 = 2, screen: UInt32, count: UInt32, version: UInt16 = 3) -> [UInt8] {
    header(kind: 2, length: 24, version: version) + le64(UInt64(total)) + le32(slots) + le32(screen) + le32(count) + le32(0)
}

private func modeMessage(count: UInt64, flags: UInt32, width: UInt32 = 0, height: UInt32 = 0, screen: UInt32) -> [UInt8] {
    header(kind: 3, length: 32) + le64(count) + le32(flags) + le32(width) + le32(height) + le32(width * 4)
        + le32(width == 0 ? 0 : 1) + le32(screen)
}

private func frameMessage(generation: UInt64, count: UInt64, slot: UInt32, screen: UInt32) -> [UInt8] {
    header(kind: 4, length: 24) + le64(generation) + le64(count) + le32(slot) + le32(screen)
}

private func cursorMessage(seq: UInt64, slot: UInt32, screen: UInt32) -> [UInt8] {
    header(kind: 5, length: 16) + le64(seq) + le32(slot) + le32(screen)
}

/// A shared-memory object with a valid v3 header for a 64 x 48 screen (two 16 KiB slots), as the writer would create it.
private struct SharedObject {
    let fd: Int32
    let total: Int
}

private func makeObject(screen: UInt32, slots: Int = 2, width: UInt32 = 64, height: UInt32 = 48) -> SharedObject {
    let slotSize = (Int(width) * 4 * Int(height) + 16_383) / 16_384 * 16_384
    let total = 65_536 + slots * slotSize
    var fd: Int32 = -1
    var name = ""
    for _ in 0..<8 {
        name = "/odft.s" + String(UInt32.random(in: 0...UInt32.max), radix: 16)
        fd = vmhost_shm_create_excl(name)
        if fd >= 0 { break }
    }
    precondition(fd >= 0, "shm create failed")
    shm_unlink(name)
    precondition(ftruncate(fd, off_t(total)) == 0)
    let base = mmap(nil, total, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)!
    defer { munmap(base, total) }
    func put(_ offset: Int, _ value: UInt32) { base.storeBytes(of: value, toByteOffset: offset, as: UInt32.self) }
    put(0, 0x5346_444F)
    put(4, 3 | (64 << 16))
    put(8, 64); put(12, 128); put(16, 64); put(20, UInt32(slots)); put(24, 65_536); put(28, UInt32(slotSize))
    put(32, 1); put(36, width); put(40, height); put(44, 0)
    base.storeBytes(of: UInt64(total), toByteOffset: 48, as: UInt64.self)
    put(56, 384); put(60, screen)
    for (index, value) in [3, 64, 448, 16_384, 16_384, 64, 64, 2].enumerated() { put(384 + 4 * index, UInt32(value)) }
    return SharedObject(fd: fd, total: total)
}

/// One `sendmsg` with the given descriptors attached to its first byte (cmsghdr on Darwin: len u32, level i32, type i32, data at 12).
private func send(_ bytes: [UInt8], descriptors: [Int32] = [], on fd: Int32) {
    var data = bytes
    var control = [UInt8](repeating: 0, count: 12 + 4 * descriptors.count)
    control.withUnsafeMutableBytes { raw in
        raw.storeBytes(of: UInt32(12 + 4 * descriptors.count), toByteOffset: 0, as: UInt32.self)
        raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
        raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
        for (index, descriptor) in descriptors.enumerated() {
            raw.storeBytes(of: descriptor, toByteOffset: 12 + 4 * index, as: Int32.self)
        }
    }
    data.withUnsafeMutableBytes { dataRaw in
        control.withUnsafeMutableBytes { controlRaw in
            var iov = iovec(iov_base: dataRaw.baseAddress, iov_len: dataRaw.count)
            withUnsafeMutablePointer(to: &iov) { iovPointer in
                var message = msghdr()
                message.msg_iov = iovPointer
                message.msg_iovlen = 1
                if !descriptors.isEmpty {
                    message.msg_control = controlRaw.baseAddress
                    message.msg_controllen = socklen_t(controlRaw.count)
                }
                precondition(sendmsg(fd, &message, 0) == dataRaw.count)
            }
        }
    }
}

// MARK: a reader on the other end of a socketpair

private final class RawPeer {
    let hostFD: Int32
    let reader: FrameReader

    init() {
        let sockets = socketPair()
        hostFD = sockets.host
        reader = FrameReader(fd: sockets.ui)
        reader.start()
        var hello = [UInt8](repeating: 0, count: 16)
        precondition(recv(hostFD, &hello, 16, 0) == 16)
        precondition(Array(hello[0..<6]) == Array("ODFT".utf8) + [3, 0], "the reader must speak version 3")
    }

    deinit {
        reader.teardown()
        Darwin.close(hostFD)
    }

    /// Pumps until the connection is terminal, `until` holds, or the rounds are over (every pump is one `recvmsg`).
    func settle(rounds: Int = 30, until: (FrameReader) -> Bool = { _ in false }) {
        for _ in 0..<rounds {
            if reader.isTerminal || until(reader) { return }
            reader.pump(timeoutMs: 20, copy: { _, _, _ in true })
        }
    }

    /// ANNOUNCE of screen `screen` with its own object; the sender's copy of the descriptor is closed.
    func announce(screen: UInt32, count: UInt32 = 2, objectScreen: UInt32? = nil, version: UInt16 = 3) {
        let object = makeObject(screen: objectScreen ?? screen)
        send(announceMessage(total: object.total, screen: screen, count: count, version: version), descriptors: [object.fd], on: hostFD)
        Darwin.close(object.fd)
    }

    func announceBoth() {
        announce(screen: 0)
        announce(screen: 1)
        settle(until: { $0.connection != .connecting })
        precondition(reader.connection == .noSignal, "two valid ANNOUNCE messages: \(reader.connection)")
    }
}

// MARK: a writer pair on the other end

private func cursorSnapshot(seq: UInt64) -> CursorSnapshot {
    CursorSnapshot(cursorSeq: seq, enabled: true, shapeSeq: 1, width: 4, height: 4, format: 2, hotX: 1, hotY: 1, posX: 5, posY: 6)
}

private final class TwoScreenRig {
    let writers: [FrameTransportWriter]
    let queue: od_vmhost.NotificationQueue
    let reader: FrameReader
    let hostFD: Int32
    /// Pixels the reader accepted, per screen, in order.
    var frames: [[[UInt8]]] = [[], []]

    /// Synthetic writers (no device): screen 0 is 64 x 48 and screen 1 is 80 x 60 (the test sizes of 11.3), both with an active mode of count 1.
    /// `session` instead takes the objects and modes from a display session (profile 4).
    init(session: OpaquePointer? = nil, slowReader: Bool = false) throws {
        let sockets = socketPair()
        hostFD = sockets.host
        reader = FrameReader(fd: sockets.ui)
        reader.start()
        var hello = [UInt8](repeating: 0, count: 16)
        precondition(recv(hostFD, &hello, 16, 0) == 16)
        queue = od_vmhost.NotificationQueue()
        if let session {
            writers = try DisplaySmoke.makeWriters(session: session, screens: 2, queue: queue)
        } else {
            writers = [
                try FrameTransportWriter(maxWidth: 64, maxHeight: 48, screen: 0, queue: queue),
                try FrameTransportWriter(maxWidth: 80, maxHeight: 60, screen: 1, queue: queue),
            ]
        }
        for writer in writers { try writer.sendAnnounce(on: hostFD, screenCount: 2) }
        for (screen, writer) in writers.enumerated() {
            if let session {
                let mode = try DisplaySmoke.screenMode(session, screen: screen)
                try writer.queueMode(count: mode.mode_change_count, active: mode.flags & 1 != 0, disabled: mode.flags & 4 != 0,
                                     width: mode.width, height: mode.height, stride: mode.stride, format: mode.format)
            } else {
                let width: UInt32 = screen == 0 ? 64 : 80
                try writer.queueMode(count: 1, active: true, width: width, height: width * 3 / 4, stride: width * 4, format: 1)
            }
        }
        if slowReader { usleep(100_000) }
        drain()
        precondition(reader.screens.count == 2, "both screens announced")
    }

    deinit {
        reader.teardown()
        Darwin.close(hostFD)
    }

    func drain(rounds: Int = 8) {
        for _ in 0..<rounds {
            try? queue.flush(fd: hostFD)
            reader.pump(timeoutMs: 15, copy: { [unowned self] screen, base, frame in
                frames[screen].append(Array(UnsafeRawBufferPointer(start: base, count: frame.stride * frame.height)))
                return true
            })
        }
    }

    /// A frame of screen `screen` from a synthetic source; true when the writer published it.
    @discardableResult
    func publish(screen: Int, generation: UInt64, modeCount: UInt64, width: UInt32, height: UInt32) throws -> Bool {
        let stride = width * 4
        let plain = od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                                    width: width, height: height, stride: stride, format: 1, flags: 3, reserved: 0,
                                    generation: generation, mode_change_count: modeCount)
        return try writers[screen].publishFrame(mode: plain) { destination, length, info in
            for index in 0..<length { destination[index] = UInt8(truncatingIfNeeded: index &* 7 &+ Int(generation) &+ screen) }
            info.width = width
            info.height = height
            info.stride = stride
            info.format = 1
            info.generation = generation
            info.mode_change_count = modeCount
            return OD_DISPLAY_STATUS_OK.rawValue
        }
    }

    /// The `generation` word of every frame descriptor of the object of `screen` (descriptors start at 128, 64 bytes each).
    func descriptorGenerations(screen: Int) -> [UInt64] {
        (0..<3).map { writers[screen].base.load(fromByteOffset: 128 + 64 * $0, as: UInt64.self) }
    }
}

private func makeSession() -> OpaquePointer {
    var config = od_display_session_config()
    config.abi_version = vmhost_display_abi_version_v1()
    config.struct_size = UInt32(OD_DISPLAY_SESSION_CONFIG_SIZE_V1)
    config.profile = OD_DISPLAY_PROFILE_TWO_SCREENS_TEST.rawValue
    var created: OpaquePointer?
    precondition(od_display_session_create(&config, &created) == OD_DISPLAY_STATUS_OK.rawValue)
    return created!
}

final class TwoScreenTransportXCTests: XCTestCase {
    // MARK: 11.10 item 1: flags of MODE, versions, screen of CURSOR

    private func expectDisconnected(_ name: String, line: UInt = #line, _ script: (RawPeer) -> Void) {
        let peer = RawPeer()
        script(peer)
        peer.settle()
        if case .disconnected = peer.reader.connection { return }
        XCTFail("\(name): expected DISCONNECTED, got \(peer.reader.connection)", line: line)
    }

    func testModeFlagsAndScreenFieldsOfThePayloadsAreChecked() {
        // Good: flags 0, 1 and 2 (DISABLED on screen 1 only).
        let good = RawPeer()
        good.announceBoth()
        send(modeMessage(count: 1, flags: 1, width: 64, height: 48, screen: 0), on: good.hostFD)
        send(modeMessage(count: 1, flags: 2, screen: 1), on: good.hostFD)
        good.settle(until: { $0.windowState(screen: 1) == .disabled })
        XCTAssertEqual(good.reader.windowState(screen: 1), .disabled)
        XCTAssertEqual(good.reader.windowState(screen: 0), .noSignal, "screen 0 has a mode but no frame yet")
        send(modeMessage(count: 2, flags: 0, screen: 1), on: good.hostFD)
        good.settle(until: { $0.windowState(screen: 1) == .noSignal })
        XCTAssertEqual(good.reader.windowState(screen: 1), .noSignal, "DISABLED -> NO_SIGNAL by a MODE with flags 0 (reset)")

        func afterAnnounces(_ name: String, _ message: [UInt8], line: UInt = #line) {
            expectDisconnected(name, line: line) { peer in
                peer.announceBoth()
                send(message, on: peer.hostFD)
            }
        }
        afterAnnounces("flags 3 (ACTIVE and DISABLED)", modeMessage(count: 1, flags: 3, width: 80, height: 60, screen: 1))
        afterAnnounces("flags bit 2", modeMessage(count: 1, flags: 4, screen: 1))
        afterAnnounces("DISABLED on screen 0", modeMessage(count: 1, flags: 2, screen: 0))
        afterAnnounces("MODE of a screen that does not exist", modeMessage(count: 1, flags: 0, screen: 2))
        afterAnnounces("inactive MODE with geometry", modeMessage(count: 1, flags: 2, width: 64, height: 48, screen: 1))
        afterAnnounces("FRAME of a screen that does not exist", frameMessage(generation: 1, count: 1, slot: 0, screen: 2))
        afterAnnounces("CURSOR of screen 1", cursorMessage(seq: 1, slot: 0, screen: 1))
    }

    func testAMessageOfAnotherVersionIsIncompatibleNotAProtocolError() {
        let peer = RawPeer()
        peer.announce(screen: 0, version: 2) // VMHost v2 against this UI v3
        peer.settle()
        if case .incompatible = peer.reader.connection { return }
        XCTFail("expected INCOMPATIBLE, got \(peer.reader.connection)")
    }

    // MARK: 11.10 item 2 and 11.4.1: reception of the descriptors

    func testReceptionOfTheDescriptorsFollowsSection11_4_1() {
        expectDisconnected("screens in the order 1, 0") { peer in
            peer.announce(screen: 1)
        }
        expectDisconnected("screen_count differs between the two ANNOUNCE messages") { peer in
            peer.announce(screen: 0, count: 2)
            peer.announce(screen: 1, count: 1)
        }
        expectDisconnected("screen_count 3") { peer in
            peer.announce(screen: 0, count: 3)
        }
        expectDisconnected("a third ANNOUNCE with a descriptor") { peer in
            peer.announceBoth()
            peer.announce(screen: 0)
        }
        expectDisconnected("a third ANNOUNCE without a descriptor") { peer in
            peer.announceBoth()
            send(announceMessage(total: 65_536 + 2 * 16_384, screen: 1, count: 2), on: peer.hostFD)
        }
        expectDisconnected("MODE before the second ANNOUNCE") { peer in
            peer.announce(screen: 0)
            send(modeMessage(count: 1, flags: 1, width: 64, height: 48, screen: 0), on: peer.hostFD)
        }
        expectDisconnected("two descriptors in one ANNOUNCE") { peer in
            let first = makeObject(screen: 0)
            let second = makeObject(screen: 1)
            send(announceMessage(total: first.total, screen: 0, count: 2), descriptors: [first.fd, second.fd], on: peer.hostFD)
            Darwin.close(first.fd)
            Darwin.close(second.fd)
        }
        expectDisconnected("ANNOUNCE without a descriptor") { peer in
            send(announceMessage(total: 65_536 + 2 * 16_384, screen: 0, count: 2), on: peer.hostFD)
        }
        expectDisconnected("a descriptor on the payload of an ANNOUNCE, not on its first byte") { peer in
            let object = makeObject(screen: 0)
            let message = announceMessage(total: object.total, screen: 0, count: 2)
            send(Array(message[0..<16]), on: peer.hostFD)
            usleep(50_000)
            send(Array(message[16...]), descriptors: [object.fd], on: peer.hostFD)
            Darwin.close(object.fd)
        }
        expectDisconnected("a descriptor on a MODE after the last ANNOUNCE") { peer in
            peer.announceBoth()
            let stray = makeObject(screen: 0)
            send(modeMessage(count: 1, flags: 1, width: 64, height: 48, screen: 0), descriptors: [stray.fd], on: peer.hostFD)
            Darwin.close(stray.fd)
        }
        expectDisconnected("screen of the memory header differs from the ANNOUNCE") { peer in
            peer.announce(screen: 0, objectScreen: 1)
        }
    }

    // MARK: 11.10 item 3: the sum of the two objects

    func testTwoObjectsOfTheMaximumSizeAreAcceptedAndAnOversizedOrMismatchedSecondOneIsNot() {
        let maxTotal = 65_536 + 4 * 37_748_736 // 151 060 480: four slots of 4096 x 2304
        let accepted = RawPeer()
        let first = makeObject(screen: 0, slots: 4, width: 4096, height: 2304)
        let second = makeObject(screen: 1, slots: 4, width: 4096, height: 2304)
        XCTAssertEqual(first.total, maxTotal)
        send(announceMessage(total: first.total, slots: 4, screen: 0, count: 2), descriptors: [first.fd], on: accepted.hostFD)
        send(announceMessage(total: second.total, slots: 4, screen: 1, count: 2), descriptors: [second.fd], on: accepted.hostFD)
        Darwin.close(first.fd)
        Darwin.close(second.fd)
        accepted.settle(until: { $0.connection != .connecting })
        XCTAssertEqual(accepted.reader.connection, .noSignal, "2 x MAX_TOTAL is accepted")
        XCTAssertEqual(accepted.reader.screens.map(\.announced), [true, true])

        // ANNOUNCE of screen 1 with total_size = MAX_TOTAL + 16384: out of range, the connection ends.
        expectDisconnected("total_size above MAX_TOTAL") { peer in
            peer.announce(screen: 0)
            let big = makeObject(screen: 1, slots: 4, width: 4096, height: 2304)
            send(announceMessage(total: maxTotal + 16_384, slots: 4, screen: 1, count: 2), descriptors: [big.fd], on: peer.hostFD)
            Darwin.close(big.fd)
        }
        // ... and so does an ANNOUNCE whose total_size differs from what fstat says about the descriptor.
        expectDisconnected("fstat differs from the announced total_size") { peer in
            peer.announce(screen: 0)
            let object = makeObject(screen: 1)
            send(announceMessage(total: object.total + 16_384, screen: 1, count: 2), descriptors: [object.fd], on: peer.hostFD)
            Darwin.close(object.fd)
        }
    }

    func testTwoAnnounceMessagesInARowReachASlowReaderWithTheirOwnDescriptors() throws {
        // The assumption of 11.4.1: the writer sends both ANNOUNCE at once, the reader looks at the socket only afterwards.
        let rig = try TwoScreenRig(slowReader: true)
        XCTAssertEqual(rig.reader.connection, .noSignal)
        XCTAssertEqual(rig.reader.screens.map(\.screen), [0, 1])
        XCTAssertEqual(rig.reader.screens.map(\.announced), [true, true])
        // Each reader holds its own object: a frame of 80 x 60 fits screen 1 and would not fit the object of screen 0 (64 x 48).
        try rig.publish(screen: 0, generation: 1, modeCount: 1, width: 64, height: 48)
        try rig.publish(screen: 1, generation: 1, modeCount: 1, width: 80, height: 60)
        rig.drain()
        XCTAssertEqual(rig.frames[0].map(\.count), [64 * 48 * 4])
        XCTAssertEqual(rig.frames[1].map(\.count), [80 * 60 * 4])
        XCTAssertEqual(rig.reader.windowState(screen: 0), .showing)
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)
    }

    // MARK: 11.10 items 4: queue, order and the 256-byte bound

    func testQueueBoundOfTwoScreensIs256BytesAndModeOfOneScreenKeepsTheOthersMessages() throws {
        let sockets = socketPair()
        defer {
            Darwin.close(sockets.host)
            Darwin.close(sockets.ui)
        }
        let flags = fcntl(sockets.host, F_GETFL)
        _ = fcntl(sockets.host, F_SETFL, flags | O_NONBLOCK)
        var sndbuf: Int32 = 1
        setsockopt(sockets.host, SOL_SOCKET, SO_SNDBUF, &sndbuf, 4)
        // Fill the socket so that nothing more leaves: the first MODE stays whole as the partial message (48 bytes).
        let filler: [UInt8] = [0x01]
        while send(sockets.host, filler, 1, 0) > 0 {}
        let queue = od_vmhost.NotificationQueue()
        func message(_ tag: UInt8, _ length: Int) -> [UInt8] { [UInt8](repeating: tag, count: length) }
        queue.queueMode(screen: 0, message(0x99, 48))
        try queue.flush(fd: sockets.host)
        XCTAssertEqual(queue.bufferedBytes, 48, "the partial message")
        // Everything that can be pending at once: MODE(0), FRAME(0), CURSOR, MODE(1), FRAME(1) behind the partial message.
        queue.queueFrame(screen: 1, message(0xB1, 40))
        queue.queueMode(screen: 1, message(0xA1, 48))
        queue.queueFrame(screen: 1, message(0xB1, 40))
        queue.queueCursor(message(0xC0, 32))
        queue.queueFrame(screen: 0, message(0xB0, 40))
        queue.queueMode(screen: 0, message(0xA0, 48))
        queue.queueFrame(screen: 0, message(0xB0, 40))
        XCTAssertEqual(queue.bufferedBytes, 48 + 2 * 48 + 2 * 40 + 32)
        XCTAssertEqual(queue.bufferedBytes, 256, "the bound of 11.5")
        // More of the same only replaces: the bound holds.
        queue.queueCursor(message(0xC0, 32))
        queue.queueFrame(screen: 1, message(0xB1, 40))
        queue.queueFrame(screen: 0, message(0xB0, 40))
        XCTAssertEqual(queue.bufferedBytes, 256)
        // A new MODE(1) replaces MODE(1) and drops FRAME(1) only: FRAME(0) and CURSOR stay (and MODE(0) too).
        queue.queueMode(screen: 1, message(0xA2, 48))
        XCTAssertEqual(queue.bufferedBytes, 256 - 40)
        // Let it leave and read the wire: the filler first, then the partial message and the order of 11.5.
        let reading = fcntl(sockets.ui, F_GETFL)
        _ = fcntl(sockets.ui, F_SETFL, reading | O_NONBLOCK)
        var wire: [UInt8] = []
        for _ in 0..<2000 where !queue.isEmpty || wire.count < 8 {
            var chunk = [UInt8](repeating: 0, count: 4096)
            let n = recv(sockets.ui, &chunk, chunk.count, 0)
            if n > 0 { wire += chunk[0..<n] }
            try queue.flush(fd: sockets.host)
        }
        while true {
            var chunk = [UInt8](repeating: 0, count: 4096)
            let n = recv(sockets.ui, &chunk, chunk.count, 0)
            if n <= 0 { break }
            wire += chunk[0..<n]
        }
        let expected = message(0x99, 48) + message(0xA0, 48) + message(0xB0, 40) + message(0xC0, 32) + message(0xA2, 48)
        XCTAssertEqual(Array(wire.suffix(expected.count)), expected, "order: partial, MODE(0), FRAME(0), CURSOR, MODE(1)")
        XCTAssertTrue(wire.prefix(wire.count - expected.count).allSatisfy { $0 == 0x01 })
        XCTAssertTrue(queue.isEmpty)
    }

    func testRealWritersSendMode0Frame0CursorMode1Frame1InThatOrder() throws {
        let rig = try TwoScreenRig()
        // Drain what the handshake queued on the real socket first.
        rig.drain()
        let wire = socketPair()
        defer {
            Darwin.close(wire.host)
            Darwin.close(wire.ui)
        }
        try rig.writers[1].queueMode(count: 2, active: true, width: 80, height: 60, stride: 320, format: 1)
        try rig.publish(screen: 1, generation: 5, modeCount: 2, width: 80, height: 60)
        _ = try rig.writers[0].publishCursor(cursorSnapshot(seq: 1)) { destination, length in
            for index in 0..<length { destination[index] = UInt8(truncatingIfNeeded: index) }
            return cursorSnapshot(seq: 1)
        }
        try rig.publish(screen: 0, generation: 5, modeCount: 1, width: 64, height: 48)
        try rig.writers[0].queueMode(count: 2, active: true, width: 64, height: 48, stride: 256, format: 1)
        // The queue of the second MODE(0) dropped the FRAME(0): publish again so that all five kinds are pending.
        try rig.publish(screen: 0, generation: 6, modeCount: 2, width: 64, height: 48)
        XCTAssertEqual(rig.queue.bufferedBytes, 48 + 40 + 32 + 48 + 40)
        try rig.queue.flush(fd: wire.host)
        var received = [UInt8](repeating: 0, count: 400)
        let n = recv(wire.ui, &received, received.count, 0)
        XCTAssertEqual(n, 208)
        func kind(_ offset: Int) -> UInt16 { UInt16(received[offset + 6]) | UInt16(received[offset + 7]) << 8 }
        XCTAssertEqual([kind(0), kind(48), kind(88), kind(120), kind(168)], [3, 4, 5, 3, 4], "MODE, FRAME, CURSOR, MODE, FRAME")
        XCTAssertEqual(received[16 + 28], 0, "MODE(0)")
        XCTAssertEqual(received[48 + 16 + 20], 0, "FRAME(0)")
        XCTAssertEqual(received[120 + 16 + 28], 1, "MODE(1)")
        XCTAssertEqual(received[168 + 16 + 20], 1, "FRAME(1)")
        // Screen 0 cannot be disabled by the writer either (11.4).
        XCTAssertThrowsError(try rig.writers[0].queueMode(count: 3, active: false, disabled: true, width: 0, height: 0, stride: 0, format: 0))
    }

    // MARK: 11.10 item 5: independent generations

    func testFramesAndModeChangesOfOneScreenDoNotTouchTheOtherOne() throws {
        let rig = try TwoScreenRig()
        try rig.publish(screen: 0, generation: 1, modeCount: 1, width: 64, height: 48)
        try rig.publish(screen: 1, generation: 1, modeCount: 1, width: 80, height: 60)
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 0), .showing)
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)

        // N frames and a mode change on screen 1.
        let object0 = rig.descriptorGenerations(screen: 0)
        let accepted0 = rig.reader.screens[0].acceptedCount
        let last0 = rig.reader.screens[0].lastFrame
        let frames0 = rig.frames[0]
        for generation in UInt64(2)...9 {
            XCTAssertTrue(try rig.publish(screen: 1, generation: generation, modeCount: 1, width: 80, height: 60))
            rig.drain()
        }
        try rig.writers[1].queueMode(count: 2, active: true, width: 40, height: 30, stride: 160, format: 1)
        XCTAssertTrue(try rig.publish(screen: 1, generation: 10, modeCount: 2, width: 40, height: 30))
        rig.drain()
        XCTAssertEqual(rig.reader.screens[1].lastFrame?.generation, 10)
        XCTAssertEqual(rig.reader.screens[1].lastFrame?.width, 40)
        XCTAssertEqual(rig.reader.screens[1].acceptedCount, 10)
        // Screen 0: descriptors of its object, the frames it was shown, its state and its last frame are as before.
        XCTAssertEqual(rig.descriptorGenerations(screen: 0), object0)
        XCTAssertEqual(rig.writers[0].lastPublishedGeneration, 1)
        XCTAssertEqual(rig.reader.screens[0].acceptedCount, accepted0)
        XCTAssertEqual(rig.reader.screens[0].lastFrame, last0)
        XCTAssertEqual(rig.frames[0], frames0)
        XCTAssertEqual(rig.reader.windowState(screen: 0), .showing)

        // The other way round: frames and a mode change on screen 0 leave screen 1 alone.
        let object1 = rig.descriptorGenerations(screen: 1)
        let accepted1 = rig.reader.screens[1].acceptedCount
        let last1 = rig.reader.screens[1].lastFrame
        let frames1 = rig.frames[1]
        for generation in UInt64(2)...6 {
            XCTAssertTrue(try rig.publish(screen: 0, generation: generation, modeCount: 1, width: 64, height: 48))
            rig.drain()
        }
        try rig.writers[0].queueMode(count: 2, active: true, width: 32, height: 24, stride: 128, format: 1)
        XCTAssertTrue(try rig.publish(screen: 0, generation: 7, modeCount: 2, width: 32, height: 24))
        rig.drain()
        XCTAssertEqual(rig.reader.screens[0].lastFrame?.generation, 7)
        XCTAssertEqual(rig.descriptorGenerations(screen: 1), object1)
        XCTAssertEqual(rig.writers[1].lastPublishedGeneration, 10)
        XCTAssertEqual(rig.reader.screens[1].acceptedCount, accepted1)
        XCTAssertEqual(rig.reader.screens[1].lastFrame, last1)
        XCTAssertEqual(rig.frames[1], frames1)
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)
    }

    func testProfile4FramesReachTheReaderAndEqualTheReferenceFiles() throws {
        // End to end without a guest (test-only profile 4): device frames of both screens through the writer, the socket and the reader.
        let session = makeSession()
        defer { _ = od_display_session_destroy(session) }
        let rig = try TwoScreenRig(session: session)
        for screen in 0..<2 {
            let mode = try DisplaySmoke.screenMode(session, screen: screen)
            let plain = od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                                        width: mode.width, height: mode.height, stride: mode.stride, format: mode.format,
                                        flags: mode.flags, reserved: 0, generation: mode.generation,
                                        mode_change_count: mode.mode_change_count)
            XCTAssertTrue(try rig.writers[screen].publishFrame(session: session, mode: plain), "screen \(screen)")
        }
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 0), .showing)
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)
        XCTAssertEqual(rig.frames[0].count, 1)
        XCTAssertEqual(rig.frames[1].count, 1)
        XCTAssertEqual(rig.frames[0].first, fixture("bars_64x48_seed0.raw"), "screen 0: bars 64x48, tolerance 0")
        XCTAssertEqual(rig.frames[1].first, fixture("checker_80x60_seed0x204080.raw"), "screen 1: checker 80x60, tolerance 0")
        XCTAssertEqual(rig.reader.screens[1].lastFrame?.stride, 320)
    }

    // MARK: 11.10 item 6: DISABLED is an explicit state

    func testDisabledScreenIsAnExplicitStateAndNoFrameFollowsIt() throws {
        let rig = try TwoScreenRig()
        try rig.publish(screen: 0, generation: 1, modeCount: 1, width: 64, height: 48)
        try rig.publish(screen: 1, generation: 1, modeCount: 1, width: 80, height: 60)
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)
        let accepted0 = rig.reader.screens[0].acceptedCount

        try rig.writers[1].queueMode(count: 2, active: false, disabled: true, width: 0, height: 0, stride: 0, format: 0)
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .disabled)
        XCTAssertNil(rig.reader.screens[1].lastFrame, "no last frame is kept as the current one")
        // The writer does not publish a frame of a screen whose last MODE is inactive (11.5), even if a source offers one.
        XCTAssertFalse(try rig.publish(screen: 1, generation: 2, modeCount: 2, width: 80, height: 60))
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .disabled)
        // A second DISABLED MODE keeps the state.
        try rig.writers[1].queueMode(count: 3, active: false, disabled: true, width: 0, height: 0, stride: 0, format: 0)
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .disabled)
        // DISABLED -> MODE_CHANGING -> SHOWING (shown_once is true).
        try rig.writers[1].queueMode(count: 4, active: true, width: 80, height: 60, stride: 320, format: 1)
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .modeChanging)
        XCTAssertTrue(try rig.publish(screen: 1, generation: 2, modeCount: 4, width: 80, height: 60))
        rig.drain()
        XCTAssertEqual(rig.reader.windowState(screen: 1), .showing)
        // Screen 0 was showing all the time and received nothing.
        XCTAssertEqual(rig.reader.windowState(screen: 0), .showing)
        XCTAssertEqual(rig.reader.screens[0].acceptedCount, accepted0)
    }

    func testAFrameWithAnOldModeChangeCountIsAProtocolErrorOfTheReader() throws {
        // After MODE(DISABLED), a FRAME breaks the connection: this case is caught by "FRAME while the mode is inactive"
        // (the stale count is not what triggers it) ...
        let disabled = try TwoScreenRig()
        try disabled.publish(screen: 1, generation: 1, modeCount: 1, width: 80, height: 60)
        try disabled.writers[1].queueMode(count: 2, active: false, disabled: true, width: 0, height: 0, stride: 0, format: 0)
        disabled.drain()
        XCTAssertEqual(disabled.reader.windowState(screen: 1), .disabled)
        send(frameMessage(generation: 7, count: 1, slot: 0, screen: 1), on: disabled.hostFD)
        disabled.drain()
        if case .disconnected = disabled.reader.connection {} else { XCTFail("FRAME after MODE(DISABLED): \(disabled.reader.connection)") }
        // ... and so does a FRAME with the old count after a new active MODE: the mode is active here, so only the
        // counter check can fire, which isolates this case.
        let renewed = try TwoScreenRig()
        try renewed.writers[1].queueMode(count: 2, active: true, width: 40, height: 30, stride: 160, format: 1)
        renewed.drain()
        send(frameMessage(generation: 7, count: 1, slot: 0, screen: 1), on: renewed.hostFD)
        renewed.drain()
        if case .disconnected = renewed.reader.connection {} else { XCTFail("FRAME with a stale count: \(renewed.reader.connection)") }
    }

    // MARK: S10-15 review D and E: a screen without a mode, connection state apart from screen states

    func testTheWriterOfAScreenWithoutAModeAnnouncesTheDeviceLimitsAndTheConnectionStays() throws {
        var noMode = od_display_screen_mode()
        XCTAssertEqual(DisplaySmoke.writerLimits(of: noMode).width, 4096)
        XCTAssertEqual(DisplaySmoke.writerLimits(of: noMode).height, 2304)
        noMode.flags = 4 // DISABLED: no mode either
        XCTAssertEqual(DisplaySmoke.writerLimits(of: noMode).width, 4096)
        var active = od_display_screen_mode()
        active.flags = 1
        active.width = 80
        active.height = 60
        XCTAssertEqual(DisplaySmoke.writerLimits(of: active).width, 80)
        XCTAssertEqual(DisplaySmoke.writerLimits(of: active).height, 60)

        // Screen 1 without a mode at start: no refusal, its window shows "no signal" while screen 0 shows its frame.
        let sockets = socketPair()
        let reader = FrameReader(fd: sockets.ui)
        defer {
            reader.teardown()
            Darwin.close(sockets.host)
        }
        reader.start()
        var hello = [UInt8](repeating: 0, count: 16)
        precondition(recv(sockets.host, &hello, 16, 0) == 16)
        let queue = od_vmhost.NotificationQueue()
        let limits = DisplaySmoke.writerLimits(of: od_display_screen_mode())
        let writers = [
            try FrameTransportWriter(maxWidth: 64, maxHeight: 48, screen: 0, queue: queue),
            try FrameTransportWriter(maxWidth: limits.width, maxHeight: limits.height, screen: 1, queue: queue),
        ]
        for writer in writers { try writer.sendAnnounce(on: sockets.host, screenCount: 2) }
        try writers[0].queueMode(count: 1, active: true, width: 64, height: 48, stride: 256, format: 1)
        try writers[1].queueMode(count: 0, active: false, width: 0, height: 0, stride: 0, format: 0)
        let rig = (queue: queue, host: sockets.host)
        func drain() {
            for _ in 0..<10 {
                try? rig.queue.flush(fd: rig.host)
                reader.pump(timeoutMs: 15, copy: { _, _, _ in true })
            }
        }
        drain()
        XCTAssertFalse(reader.isTerminal, "the connection is not closed because screen 1 has no mode: \(reader.connection)")
        XCTAssertEqual(reader.windowState(screen: 1), .noSignal)
        XCTAssertEqual(writers[1].base.load(fromByteOffset: 36, as: UInt32.self), 4096)
        XCTAssertEqual(writers[1].base.load(fromByteOffset: 40, as: UInt32.self), 2304)

        // The window of screen 0 shows its frame; the window of screen 1 must not take over that state (11.7).
        let plain = od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                                    width: 64, height: 48, stride: 256, format: 1, flags: 3, reserved: 0, generation: 1,
                                    mode_change_count: 1)
        XCTAssertTrue(try writers[0].publishFrame(mode: plain) { destination, length, info in
            for index in 0..<length { destination[index] = UInt8(truncatingIfNeeded: index) }
            info.width = 64
            info.height = 48
            info.stride = 256
            info.format = 1
            info.generation = 1
            info.mode_change_count = 1
            return OD_DISPLAY_STATUS_OK.rawValue
        })
        drain()
        XCTAssertEqual(reader.state, .showing, "screen 0")
        XCTAssertEqual(reader.windowState(screen: 0), .showing)
        XCTAssertEqual(reader.windowState(screen: 1), .noSignal, "the window of screen 1 does not show the state of screen 0")
        XCTAssertEqual(reader.connection, .noSignal, "the connection alone: running (reported as NO_SIGNAL, never shown by a window)")

        // A mode set later fits the object of the device limits.
        try writers[1].queueMode(count: 1, active: true, width: 1920, height: 1080, stride: 7680, format: 1)
        let big = od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                                  width: 1920, height: 1080, stride: 7680, format: 1, flags: 3, reserved: 0, generation: 1,
                                  mode_change_count: 1)
        XCTAssertTrue(try writers[1].publishFrame(mode: big) { destination, length, info in
            destination.update(repeating: 0x5A, count: length)
            info.width = 1920
            info.height = 1080
            info.stride = 7680
            info.format = 1
            info.generation = 1
            info.mode_change_count = 1
            return OD_DISPLAY_STATUS_OK.rawValue
        })
        drain()
        XCTAssertEqual(reader.windowState(screen: 1), .showing)

        // A failure of the connection ends every window, whatever screen it came from.
        send(modeMessage(count: 9, flags: 2, screen: 0), on: sockets.host)
        drain()
        if case .disconnected = reader.windowState(screen: 0) {} else { XCTFail("window 0: \(reader.windowState(screen: 0))") }
        if case .disconnected = reader.windowState(screen: 1) {} else { XCTFail("window 1: \(reader.windowState(screen: 1))") }
    }

    func testTheSessionOfProfile4GivesEachScreenItsOwnMaximumFromItsMode() throws {
        let session = makeSession()
        defer { _ = od_display_session_destroy(session) }
        let writers = try DisplaySmoke.makeWriters(session: session, screens: 2, queue: od_vmhost.NotificationQueue())
        XCTAssertEqual(writers.map { $0.base.load(fromByteOffset: 36, as: UInt32.self) }, [64, 80])
        XCTAssertEqual(writers.map { $0.base.load(fromByteOffset: 40, as: UInt32.self) }, [48, 60])
        XCTAssertEqual(writers.map { $0.base.load(fromByteOffset: 60, as: UInt32.self) }, [0, 1], "screen of the header")
    }
}
