import Darwin
import Foundation
import ODAtomics
import ODHostABI
import XCTest
import ODShared
@testable import od_vmhost

// S10-24: failures of the frame transport as the UI reader sees them (FRAME_TRANSPORT_V1.md 5.2, 6.3, 6.4, 7.2, 8.1; documented in
// docs/testing/DISPLAY.md section 6). The real writer (apps/vmhost/Sources/FrameTransport.swift) against the real reader
// (apps/macos/Sources/Shared/FrameReader.swift) in one process over a socketpair; where a failure cannot be made by the real
// writer in one process (a slot rewritten while the reader copies it, a peer that dies in the middle of a message) the test plays
// the writer through the shared memory or the socket. The same failures in separate processes, with SIGKILL, are
// tests/system/display_failure_smoke.py. From apps/ (the Rust staticlib first, `cargo build -p od-host --offline` from the repository root):
//
//   swift test --filter DisplayFailureXCTests
//
// The words of the window for each state (`MachineWindowModel.overlay`) are checked in apps/macos/Tests/MachineWindowTests.swift.

private func socketPair() -> (host: Int32, ui: Int32) {
    var fds: [Int32] = [0, 0]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    return (fds[0], fds[1])
}

/// Every pixel of a frame of generation `g` is `[g, g >> 8, g >> 16, 0]`: a frame that mixes two generations is detectable.
private func tagged(_ generation: UInt64, count: Int) -> [UInt8] {
    let pixel: [UInt8] = [UInt8(truncatingIfNeeded: generation), UInt8(truncatingIfNeeded: generation >> 8), UInt8(truncatingIfNeeded: generation >> 16), 0]
    return (0..<count).map { pixel[$0 % 4] }
}

/// One writer, one reader, the host end of their socket. The reader has accepted ANNOUNCE and the first MODE (64 x 48, count 1).
private final class Rig {
    let writer: FrameTransportWriter
    let queue = NotificationQueue()
    let reader: FrameReader
    let hostFD: Int32
    /// Frames the reader copied, in order: (generation, pixels). A copy the reader discards afterwards is in this list too.
    var copies: [(generation: UInt64, pixels: [UInt8])] = []
    var copyCalls = 0
    /// Runs between g1 and g2 of the slot read (section 5.2, steps 2 and 6): the place where a writer rewrites a slot.
    var duringCopy: ((AcceptedFrame) -> Void)?

    init(announce: Bool = true, headerVersion: UInt16? = nil) throws {
        let sockets = socketPair()
        hostFD = sockets.host
        reader = FrameReader(fd: sockets.ui)
        reader.start()
        var hello = [UInt8](repeating: 0, count: 16)
        precondition(recv(hostFD, &hello, 16, 0) == 16)
        writer = try FrameTransportWriter(maxWidth: 64, maxHeight: 48, queue: queue)
        let flags = fcntl(hostFD, F_GETFL)
        precondition(fcntl(hostFD, F_SETFL, flags | O_NONBLOCK) == 0)
        if let headerVersion { writer.base.storeBytes(of: headerVersion, toByteOffset: 4, as: UInt16.self) }
        guard announce else { return }
        try writer.sendAnnounce(on: hostFD, screenCount: 1)
        try writer.queueMode(count: 1, active: true, width: 64, height: 48, stride: 256, format: 1)
        pump(rounds: 3)
    }

    deinit {
        reader.teardown()
        Darwin.close(hostFD)
    }

    func pump(rounds: Int = 2) {
        for _ in 0..<rounds {
            try? queue.flush(fd: hostFD)
            reader.pump(timeoutMs: 20, copy: { [unowned self] _, base, frame in
                copyCalls += 1
                copies.append((frame.generation, Array(UnsafeRawBufferPointer(start: base, count: frame.stride * frame.height))))
                duringCopy?(frame)
                return true
            })
        }
    }

    /// A synthetic frame through the real publication; true when the writer published it.
    @discardableResult
    func publish(generation: UInt64, count: UInt64 = 1, width: UInt32 = 64, height: UInt32 = 48) throws -> Bool {
        let mode = od_display_mode(abi_version: vmhost_display_abi_version_v1(), struct_size: UInt32(OD_DISPLAY_MODE_SIZE_V1),
                                   width: width, height: height, stride: width * 4, format: 1, flags: 3, reserved: 0,
                                   generation: generation, mode_change_count: count)
        return try writer.publishFrame(mode: mode) { destination, length, info in
            let pixels = tagged(generation, count: length)
            pixels.withUnsafeBufferPointer { destination.update(from: $0.baseAddress!, count: length) }
            info.width = width
            info.height = height
            info.stride = width * 4
            info.format = 1
            info.generation = generation
            info.mode_change_count = count
            return OD_DISPLAY_STATUS_OK.rawValue
        }
    }

    /// The slot that holds `generation`, or nil.
    func slot(of generation: UInt64) -> Int? {
        (0..<3).first { writer.base.load(fromByteOffset: 128 + 64 * $0, as: UInt64.self) == generation }
    }

    /// What a writer that starts to rewrite `slot` does first (5.1 steps 2-4): the invalid mark, then pixels.
    func startRewrite(slot: Int) {
        writer.base.storeBytes(of: UInt64(0), toByteOffset: 128 + 64 * slot, as: UInt64.self)
        writer.base.storeBytes(of: UInt8(0xA5), toByteOffset: 65_536 + slot * 16_384, as: UInt8.self)
    }
}

final class DisplayFailureXCTests: XCTestCase {
    // MARK: 1. the writer dies or is rewriting a slot; the UI never shows a half-written frame

    func testSlotRewrittenBetweenTheTwoGenerationReadsIsDiscardedAndTheStateIsKept() throws {
        let rig = try Rig()
        try rig.publish(generation: 1)
        rig.pump()
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.generation, 1)

        try rig.publish(generation: 2)
        let slot = try XCTUnwrap(rig.slot(of: 2))
        rig.duringCopy = { _ in rig.startRewrite(slot: slot) } // the writer is back in this slot while the reader copies it
        rig.pump()
        rig.duringCopy = nil
        XCTAssertEqual(rig.copyCalls, 2, "the reader did copy the slot: the discard is by g2, not by g1")
        XCTAssertEqual(rig.reader.acceptedCount, 1, "the half-rewritten frame was accepted")
        XCTAssertEqual(rig.reader.lastFrame?.generation, 1, "the shown frame is still the previous good one")
        XCTAssertEqual(rig.reader.state, .showing, "a discarded frame does not change the state (7.2)")
        XCTAssertFalse(rig.reader.isTerminal, "a discarded frame is not an error")

        // The next frame of the writer is accepted whole.
        try rig.publish(generation: 3)
        rig.pump()
        XCTAssertEqual(rig.reader.lastFrame?.generation, 3)
        XCTAssertEqual(rig.copies.last?.pixels, tagged(3, count: 64 * 48 * 4))
    }

    func testNotificationForASlotThatIsBeingWrittenIsDiscardedWithoutACopy() throws {
        // The real writer never announces a slot it has not published; the reader must still refuse generation 0 and any other
        // value than the announced one (5.2 step 2) without copying and without an error (7.3: 0 is a legal overwrite).
        let rig = try Rig()
        try rig.publish(generation: 1)
        rig.pump()
        let copiesBefore = rig.copyCalls
        let slot = (rig.slot(of: 1)! + 1) % 3
        rig.startRewrite(slot: slot) // generation 0, half a frame
        var frame = Array("ODFT".utf8) + [3, 0, 4, 0, 0, 0, 0, 0, 24, 0, 0, 0]
        frame += withUnsafeBytes(of: UInt64(2).littleEndian) { Array($0) } + withUnsafeBytes(of: UInt64(1).littleEndian) { Array($0) }
        frame += withUnsafeBytes(of: UInt32(slot).littleEndian) { Array($0) } + [0, 0, 0, 0]
        XCTAssertEqual(send(rig.hostFD, frame, frame.count, 0), frame.count)
        rig.pump()
        XCTAssertEqual(rig.copyCalls, copiesBefore, "a slot of generation 0 was copied")
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.generation, 1)
        XCTAssertFalse(rig.reader.isTerminal)
    }

    func testPeerDeathIsDisconnectedWhateverWasInFlight() throws {
        // (a) between two messages, (b) in the middle of a message header, (c) in the middle of a payload: the process was killed.
        for cut in [0, 10, 20] {
            let rig = try Rig()
            try rig.publish(generation: 1)
            rig.pump()
            XCTAssertEqual(rig.reader.state, .showing)
            if cut > 0 {
                let message = Array("ODFT".utf8) + [3, 0, 4, 0, 0, 0, 0, 0, 24, 0, 0, 0] + [UInt8](repeating: 0, count: 24)
                XCTAssertEqual(send(rig.hostFD, message, cut, 0), cut)
            }
            shutdown(rig.hostFD, SHUT_RDWR) // what the kernel does when the process dies
            for _ in 0..<5 { rig.reader.pump(timeoutMs: 20, copy: { _, _, _ in true }) }
            guard case .disconnected = rig.reader.state else { return XCTFail("cut \(cut): expected DISCONNECTED, got \(rig.reader.state)") }
            XCTAssertTrue(rig.reader.isTerminal)
            XCTAssertEqual(rig.reader.state.name, "DISCONNECTED")
        }
    }

    func testWriterWithoutAReaderDoesNotFailAndSeesTheEndAsAnError() throws {
        // The reader never reads: the writer publishes without waiting (6.2) and without an error.
        let rig = try Rig()
        for generation in 1...200 { try rig.publish(generation: UInt64(generation)); try rig.queue.flush(fd: rig.hostFD) }
        // The reader is gone: the write fails with an error (SO_NOSIGPIPE, as in DisplaySmoke.run), not with a signal.
        var one: Int32 = 1
        setsockopt(rig.hostFD, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        rig.reader.teardown()
        try rig.publish(generation: 201)
        XCTAssertThrowsError(try rig.queue.flush(fd: rig.hostFD)) { error in
            guard case FrameTransportError.channel = error else { return XCTFail("\(error)") }
        }
    }

    // MARK: 2. the reader is late

    func testReaderThatReadsLateAcceptsOnlyTheLatestFrameWhole() throws {
        let rig = try Rig()
        for generation in 1...300 {
            try rig.publish(generation: UInt64(generation))
            try rig.queue.flush(fd: rig.hostFD)
        }
        rig.pump(rounds: 6)
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.generation, 300, "the first frame the reader shows is the latest one, not generation 1")
        XCTAssertEqual(rig.reader.acceptedCount, 1, "the reader joined the notifications late and read one slot (7.3)")
        XCTAssertEqual(rig.copies.map(\.generation), [300])
        XCTAssertEqual(rig.copies.last?.pixels, tagged(300, count: 64 * 48 * 4))
    }

    func testStaleNotificationOfAnOverwrittenSlotIsDiscarded() throws {
        // Generation 1 is announced, then three more frames come before the reader looks at slot 1: it is rewritten (g1 differs).
        let rig = try Rig(announce: true)
        try rig.publish(generation: 1)
        let firstSlot = try XCTUnwrap(rig.slot(of: 1))
        for generation in 2...5 { try rig.publish(generation: UInt64(generation)) } // never flushed: the reader hears only the stale FRAME below
        XCTAssertNil(rig.slot(of: 1), "the old slot was not reused: the scenario does not hold (slot \(firstSlot))")
        var stale = Array("ODFT".utf8) + [3, 0, 4, 0, 0, 0, 0, 0, 24, 0, 0, 0]
        stale += withUnsafeBytes(of: UInt64(1).littleEndian) { Array($0) } + withUnsafeBytes(of: UInt64(1).littleEndian) { Array($0) }
        stale += withUnsafeBytes(of: UInt32(firstSlot).littleEndian) { Array($0) } + [0, 0, 0, 0]
        XCTAssertEqual(send(rig.hostFD, stale, stale.count, 0), stale.count)
        var copyCalls = 0
        for _ in 0..<2 { rig.reader.pump(timeoutMs: 20, copy: { _, _, _ in copyCalls += 1; return true }) }
        XCTAssertFalse(rig.reader.isTerminal, "a stale notification is a legal overwrite, not a protocol error: \(rig.reader.state)")
        XCTAssertEqual(rig.reader.acceptedCount, 0)
        XCTAssertEqual(copyCalls, 0, "a slot of another generation was copied")
    }

    // MARK: 3. a mode change between two reads

    func testModeChangeBetweenTwoReadsDiscardsTheFrameAndTheUIShowsAModeChange() throws {
        let rig = try Rig()
        try rig.publish(generation: 1)
        rig.pump()
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.modeChangeCount, 1)

        // Frame 2 of mode 1 is announced; while the reader copies it, the device switches mode: the writer rewrites the slot
        // for the new mode and queues MODE 2 (32 x 24).
        try rig.publish(generation: 2)
        let slot = try XCTUnwrap(rig.slot(of: 2))
        rig.duringCopy = { _ in
            rig.startRewrite(slot: slot)
            try? rig.writer.queueMode(count: 2, active: true, width: 32, height: 24, stride: 128, format: 1)
        }
        rig.pump(rounds: 1) // one read of the socket: MODE is queued during the copy and not yet on the wire
        rig.duringCopy = nil
        XCTAssertEqual(rig.reader.acceptedCount, 1, "the frame of the replaced mode was accepted")
        XCTAssertEqual(rig.reader.state, .showing, "until MODE arrives the old frame is still the current one")

        // MODE arrives (the next pump): the UI says "mode change" and shows no frame, though the last frame is still in memory.
        rig.pump()
        XCTAssertEqual(rig.reader.state, .modeChanging)
        XCTAssertEqual(rig.reader.state.name, "MODE_CHANGING")
        XCTAssertFalse(rig.reader.isTerminal)

        // The writer refuses a frame of the replaced mode (6.3) and publishes the first frame of the new one.
        XCTAssertFalse(try rig.publish(generation: 3, count: 1), "a frame of mode 1 was published after MODE 2")
        rig.pump()
        XCTAssertEqual(rig.reader.state, .modeChanging, "no frame of mode 2 yet")
        XCTAssertTrue(try rig.publish(generation: 4, count: 2, width: 32, height: 24))
        rig.pump()
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.width, 32)
        XCTAssertEqual(rig.reader.lastFrame?.modeChangeCount, 2)
        XCTAssertEqual(rig.copies.last?.pixels, tagged(4, count: 32 * 24 * 4))
    }

    func testFrameOfAnOlderModeAfterTheModeIsAProtocolErrorNotAShownFrame() throws {
        let rig = try Rig()
        try rig.publish(generation: 1)
        rig.pump()
        try rig.writer.queueMode(count: 2, active: true, width: 32, height: 24, stride: 128, format: 1)
        rig.pump()
        XCTAssertEqual(rig.reader.state, .modeChanging)
        // A writer that breaks 6.3 and sends FRAME of mode 1 after MODE 2.
        var frame = Array("ODFT".utf8) + [3, 0, 4, 0, 0, 0, 0, 0, 24, 0, 0, 0]
        frame += withUnsafeBytes(of: UInt64(2).littleEndian) { Array($0) } + withUnsafeBytes(of: UInt64(1).littleEndian) { Array($0) }
        frame += [0, 0, 0, 0, 0, 0, 0, 0]
        XCTAssertEqual(send(rig.hostFD, frame, frame.count, 0), frame.count)
        rig.pump()
        guard case .disconnected = rig.reader.state else { return XCTFail("expected DISCONNECTED, got \(rig.reader.state)") }
        XCTAssertEqual(rig.reader.acceptedCount, 1)
    }

    // MARK: 4. an incompatible version

    func testMessageOfAnotherVersionIsIncompatibleBeforeAnyMemoryIsMapped() throws {
        for version: UInt16 in [1, 2, 4, 9] {
            let rig = try Rig(announce: false)
            let header = Array("ODFT".utf8) + [UInt8(version), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] // VERSION-like: kind 0, nothing else is read
            XCTAssertEqual(send(rig.hostFD, header, header.count, 0), header.count)
            rig.pump()
            guard case let .incompatible(reason) = rig.reader.state else { return XCTFail("version \(version): got \(rig.reader.state)") }
            XCTAssertTrue(reason.contains("v\(version)"), "the reason names the VMHost version: \(reason)")
            XCTAssertEqual(rig.reader.state.name, "INCOMPATIBLE")
            XCTAssertEqual(rig.copyCalls, 0)
            XCTAssertNil(rig.reader.lastFrame)
        }
    }

    func testMemoryOfAnotherVersionIsIncompatibleAndNoSlotIsRead() throws {
        // The message is version 3 and valid; the object says version 2. The notifications behind the ANNOUNCE are never read.
        let rig = try Rig(announce: false, headerVersion: 2)
        try rig.writer.sendAnnounce(on: rig.hostFD, screenCount: 1)
        try rig.writer.queueMode(count: 1, active: true, width: 64, height: 48, stride: 256, format: 1)
        try rig.publish(generation: 1) // a perfectly good frame behind the bad header
        rig.pump(rounds: 4)
        guard case let .incompatible(reason) = rig.reader.state else { return XCTFail("got \(rig.reader.state)") }
        XCTAssertTrue(reason.contains("version 2"), reason)
        XCTAssertEqual(rig.copyCalls, 0, "a slot was read before the header was accepted")
        XCTAssertEqual(rig.reader.acceptedCount, 0)
        XCTAssertNil(rig.reader.lastFrame)
        XCTAssertEqual(rig.writer.base.load(fromByteOffset: 64, as: UInt32.self), 0, "reader_claim was written: a slot was claimed")
        XCTAssertTrue(rig.reader.isTerminal)
    }

    // MARK: 5. the notification queue overflows

    func testOverflowKeepsTheLatestStateCountsTheLossAndStaysBounded() throws {
        let rig = try Rig()
        let total: UInt64 = 3_000
        var mode: UInt64 = 1
        var maxBuffered = 0
        for generation in 1...total {
            // Every 100th frame starts a new mode: 64 x 48 and 32 x 24 alternate.
            let serial = (generation - 1) / 100
            let half = serial % 2 == 1
            mode = 1 + serial
            try rig.publish(generation: generation, count: mode, width: half ? 32 : 64, height: half ? 24 : 48)
            try rig.queue.flush(fd: rig.hostFD) // the reader reads nothing: the socket fills and the queue has to cope
            maxBuffered = max(maxBuffered, rig.queue.bufferedBytes)
        }
        XCTAssertLessThanOrEqual(maxBuffered, 256, "the queue grew past its bound (11.5)")
        XCTAssertLessThanOrEqual(rig.queue.highWaterBytes, 256)
        XCTAssertGreaterThan(rig.queue.sendWouldBlock, 0, "send never answered EAGAIN: the socket never filled")
        XCTAssertGreaterThan(rig.queue.supersededFrames, 0, "no overflow happened: the socket never filled")
        XCTAssertGreaterThan(rig.queue.supersededModes, 0)
        // Now the reader catches up; the state it ends in is the writer's latest.
        for _ in 0..<400 {
            rig.pump(rounds: 1)
            if rig.reader.lastFrame?.generation == total { break }
        }
        XCTAssertFalse(rig.reader.isTerminal, "the overflow ended the connection: \(rig.reader.state)")
        XCTAssertEqual(rig.reader.state, .showing)
        XCTAssertEqual(rig.reader.lastFrame?.generation, total)
        XCTAssertEqual(rig.reader.lastFrame?.modeChangeCount, mode)
        XCTAssertEqual(rig.copies.last?.pixels, tagged(total, count: rig.copies.last?.pixels.count ?? 0), "the last frame is not whole")
        // Everything that was published and did not reach the reader is in the counters.
        XCTAssertGreaterThan(rig.queue.supersededFrames, Int(total) - 500)
        XCTAssertTrue(rig.queue.statistics.contains("superseded_frames=\(rig.queue.supersededFrames)"))
    }

    func testMessagesStayWholeAndInOrderWhenTheSocketFillsUp() throws {
        // A socket with a tiny buffer, filled until EAGAIN, then freed a few bytes at a time: the stream the reader gets is whole
        // messages in order (6.4). On this kernel `send` never took a message in parts (`partialSends` stays 0 here), so the
        // partial-message path of the queue is NOT exercised by this test; it only has to keep the order if a kernel does that.
        let sockets = socketPair()
        defer { Darwin.close(sockets.host); Darwin.close(sockets.ui) }
        var small: Int32 = 1_024
        setsockopt(sockets.host, SOL_SOCKET, SO_SNDBUF, &small, socklen_t(MemoryLayout<Int32>.size))
        precondition(fcntl(sockets.host, F_SETFL, fcntl(sockets.host, F_GETFL) | O_NONBLOCK) == 0)
        var junk = 0
        var one: UInt8 = 0
        while send(sockets.host, &one, 1, 0) == 1 { junk += 1 }
        XCTAssertEqual(errno, EAGAIN)
        let queue = NotificationQueue()
        var expected: [[UInt8]] = []
        for generation in 1...200 {
            let message = Array("ODFT".utf8) + [3, 0, 4, 0, 0, 0, 0, 0, 24, 0, 0, 0]
                + withUnsafeBytes(of: UInt64(generation).littleEndian) { Array($0) } + [UInt8](repeating: 0, count: 16)
            queue.queueFrame(screen: 0, message)
            try queue.flush(fd: sockets.host)
            // The reader frees a few bytes; remember the messages once they are on the wire.
            var sink = [UInt8](repeating: 0, count: 7)
            _ = recv(sockets.ui, &sink, 7, MSG_DONTWAIT)
            expected.append(message)
        }
        XCTAssertGreaterThan(queue.sendWouldBlock, 0)
        // Drain everything: after the junk bytes the stream is a sequence of whole messages whose generations only grow.
        var stream: [UInt8] = []
        var chunk = [UInt8](repeating: 0, count: 4_096)
        while true {
            try queue.flush(fd: sockets.host)
            let n = recv(sockets.ui, &chunk, chunk.count, MSG_DONTWAIT)
            if n <= 0 { break }
            stream += chunk[0..<n]
        }
        XCTAssertEqual(queue.bufferedBytes, 0)
        guard let start = stream.firstRange(of: Array("ODFT".utf8))?.lowerBound else { return XCTFail("no message in the stream") }
        // Junk bytes were zeros and were mostly consumed above; resynchronise on the first magic and check 40-byte frames.
        var generations: [UInt64] = []
        var offset = start
        while offset + 40 <= stream.count, Array(stream[offset..<(offset + 4)]) == Array("ODFT".utf8) {
            generations.append(stream[(offset + 16)..<(offset + 24)].enumerated().reduce(0) { $0 | UInt64($1.element) << UInt64(8 * $1.offset) })
            offset += 40
        }
        XCTAssertEqual(offset, stream.count, "the stream is not whole messages: \(stream.count - offset) bytes left over")
        XCTAssertEqual(generations, generations.sorted())
        XCTAssertEqual(generations.last, 200, "the newest FRAME was lost")
    }

    func testQueueCountersCountEveryReplacedNotification() {
        let queue = NotificationQueue()
        queue.queueFrame(screen: 0, [1])
        queue.queueFrame(screen: 0, [2]) // replaces a FRAME
        queue.queueMode(screen: 0, [3]) // drops the pending FRAME
        queue.queueMode(screen: 0, [4]) // replaces a MODE
        queue.queueCursor([5])
        queue.queueCursor([6]) // replaces a CURSOR
        XCTAssertEqual(queue.supersededFrames, 2)
        XCTAssertEqual(queue.supersededModes, 1)
        XCTAssertEqual(queue.supersededCursors, 1)
        XCTAssertEqual(queue.bufferedBytes, 2)
        XCTAssertEqual(queue.highWaterBytes, 2, "the largest content after a queue call: the MODE and the CURSOR (one byte each here)")
    }
}
