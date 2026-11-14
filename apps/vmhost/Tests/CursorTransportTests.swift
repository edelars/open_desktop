import Darwin
import Foundation
import ODAtomics
import ODHostABI
import XCTest
import ODShared
@testable import od_vmhost

// S10-14 unit test of the cursor path of frame transport v2 (FRAME_TRANSPORT_V1.md section 10): the real writer
// (apps/vmhost/Sources/FrameTransport.swift, target `od-vmhost`) against the real reader (apps/macos/Sources/Shared/FrameReader.swift,
// target `ODShared`) in one process, over a socketpair and one shared-memory object. XCTest case of the OpenDesktopVMHostTests
// target (S11-05a); it used to be an executable with `@main`, the checks are unchanged. From apps/ (the Rust staticlib first,
// `cargo build -p od-host --offline` from the repository root):
//
//   swift test --filter CursorTransportXCTests
//
// The cursor of the device cannot be driven from here without a test-only ABI call (DISPLAY_HOST_ABI_V1.md rule 0), so the writer is
// fed `CursorSnapshot` values directly; the ABI itself is covered by the od-host unit tests. The reader's `cursorCopy` callback
// runs between `g1` and `g2` of section 10.6, which is where these tests play the writer that rewrites a slot.

private var failures = 0

private func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition {
        failures += 1
        print("FAIL: \(message())")
    }
}

private func socketPair() -> (host: Int32, ui: Int32) {
    var fds: [Int32] = [0, 0]
    precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    return (fds[0], fds[1])
}

private func le32(_ value: UInt32) -> [UInt8] { (0..<4).map { UInt8(truncatingIfNeeded: value >> UInt32(8 * $0)) } }
private func le64(_ value: UInt64) -> [UInt8] { (0..<8).map { UInt8(truncatingIfNeeded: value >> UInt64(8 * $0)) } }

private func pattern(_ width: Int, _ height: Int, salt: UInt8 = 3) -> [UInt8] {
    (0..<(width * height * 4)).map { UInt8(truncatingIfNeeded: $0 &* 7 &+ Int(salt)) }
}

private func snapshot(
    seq: UInt64, shape: UInt64 = 1, enabled: Bool = true, width: UInt32 = 4, height: UInt32 = 4, hotX: UInt32 = 1,
    hotY: UInt32 = 1, posX: UInt32 = 5, posY: UInt32 = 6
) -> CursorSnapshot {
    CursorSnapshot(
        cursorSeq: seq, enabled: shape == 0 ? false : enabled, shapeSeq: shape, width: shape == 0 ? 0 : width,
        height: shape == 0 ? 0 : height, format: shape == 0 ? 0 : 2, hotX: shape == 0 ? 0 : hotX,
        hotY: shape == 0 ? 0 : hotY, posX: posX, posY: posY)
}

/// A writer, a reader and the host end of their socket; the reader has accepted ANNOUNCE and the first MODE.
private final class Rig {
    let writer: FrameTransportWriter
    let reader: FrameReader
    let hostFD: Int32
    var uploads = 0
    /// Plays a presenter that cannot allocate a texture (item 7 of the review).
    var failCopies = false
    var lastUpload: [UInt8] = []

    init() throws {
        let sockets = socketPair()
        hostFD = sockets.host
        reader = FrameReader(fd: sockets.ui)
        reader.start()
        var hello = [UInt8](repeating: 0, count: 16)
        precondition(recv(hostFD, &hello, 16, 0) == 16)
        precondition(Array(hello[0..<6]) == Array("ODFT".utf8) + [3, 0], "the reader must speak version 3")
        writer = try FrameTransportWriter(maxWidth: 64, maxHeight: 48)
        try writer.sendAnnounce(on: hostFD, screenCount: 1)
        try writer.queueMode(count: 1, active: true, width: 64, height: 48, stride: 256, format: 1)
        try writer.flush(fd: hostFD)
        pump()
        pump()
        precondition(reader.state == .noSignal, "reader state after ANNOUNCE and MODE: \(reader.state)")
    }

    deinit {
        reader.teardown()
        Darwin.close(hostFD)
    }

    /// Flush the writer queue and let the reader process what arrived.
    func pump(copy: @escaping (UnsafeRawPointer, AcceptedCursor) -> Void = { _, _ in }) {
        try? writer.flush(fd: hostFD)
        reader.pump(timeoutMs: 50, copy: { _, _, _ in true }, cursorCopy: { [unowned self] pixels, cursor in
            uploads += 1
            lastUpload = Array(UnsafeRawBufferPointer(start: pixels, count: cursor.width * cursor.height * 4))
            copy(pixels, cursor)
            return !failCopies
        })
    }

    /// Publishes `state` with the image `bytes`; returns how many times the writer had to copy the image.
    @discardableResult
    func publish(_ state: CursorSnapshot, bytes: [UInt8]? = nil) throws -> Int {
        var copies = 0
        let image = bytes ?? pattern(Int(state.width), Int(state.height))
        _ = try writer.publishCursor(state) { destination, length in
            copies += 1
            image.withUnsafeBytes { destination.update(from: $0.bindMemory(to: UInt8.self).baseAddress!, count: length) }
            return state
        }
        return copies
    }

    func descriptorSeq(_ slot: Int) -> UInt64 {
        writer.base.advanced(by: 448 + 64 * slot).load(as: UInt64.self)
    }

    /// The slot that holds `seq`, or nil.
    func slot(of seq: UInt64) -> Int? { (0..<3).first { descriptorSeq($0) == seq } }
}

struct CursorTransportTests {
    // MARK: 10.3 layout (written by the real writer, checked against numbers of the spec, not against code constants)

    static func layoutConstants() throws {
        let rig = try Rig()
        let base = rig.writer.base
        func u32(_ offset: Int) -> UInt32 { base.load(fromByteOffset: offset, as: UInt32.self) }
        check(u32(0) == 0x5346_444F, "magic ODFS")
        check(u32(4) == 3 | (64 << 16), "version 3, header_size 64")
        check(u32(24) == 65_536, "slots_offset 65536")
        check(base.load(fromByteOffset: 48, as: UInt64.self) == 65_536 + 3 * 16_384, "total_size = 65536 + 3 * 16384 for 64x48")
        check(u32(56) == 384 && u32(60) == 0, "cursor_offset 384, reserved 0")
        let area = (0..<8).map { u32(384 + 4 * $0) }
        check(area == [3, 64, 448, 16_384, 16_384, 64, 64, 2], "cursor area header \(area)")
        for slot in 0..<3 {
            check(rig.descriptorSeq(slot) == 0, "descriptor \(slot) starts invalid")
        }
        check(u32(68) == 0, "cursor_claim starts at 0")
        // total_size 114 688 for the 64x48 three-slot test writer (10.3.5).
        check(65_536 + 3 * 16_384 == 114_688, "table of 10.3.5")
        check(rig.reader.cursor == nil, "no cursor before the first CURSOR")
    }

    // MARK: 10.5 / 10.6 round trip

    static func roundTripAndShapeCopies() throws {
        let rig = try Rig()
        let frameGenerationBefore = rig.writer.lastPublishedGeneration

        // Shape + position + visibility, as the device would have published them one after another.
        let a = snapshot(seq: 1, enabled: false, posX: 0, posY: 0)
        check(try rig.publish(a) == 1, "first publication copies the image")
        rig.pump()
        check(rig.reader.cursor == AcceptedCursor(
            cursorSeq: 1, enabled: false, shapeSeq: 1, width: 4, height: 4, hotX: 1, hotY: 1, posX: 0, posY: 0), "first state")
        check(rig.uploads == 1 && rig.lastUpload == pattern(4, 4), "the reader copied the image bytes unchanged")

        // Moves: the writer rotates slots, so the next two publications copy the image into the other two slots;
        // the reader copies it only when shape_seq changes (once, above).
        var copies = 0
        for (seq, x) in [(UInt64(2), UInt32(7)), (3, 8), (4, 9), (5, 10)] {
            copies += try rig.publish(snapshot(seq: seq, posX: x, posY: 2))
            rig.pump()
            check(rig.reader.cursor?.cursorSeq == seq && rig.reader.cursor?.posX == Int(x) && rig.reader.cursor?.enabled == true,
                  "move \(seq): \(String(describing: rig.reader.cursor))")
        }
        check(copies == 2, "after one copy per slot the writer stops copying an unchanged shape: \(copies) copies for 4 moves")
        check(rig.uploads == 1, "the reader did not upload again for an unchanged shape_seq: \(rig.uploads)")
        check(rig.reader.cursorAcceptedCount == 5, "five accepted updates: \(rig.reader.cursorAcceptedCount)")

        // A new shape is copied by both.
        let bytes = pattern(2, 3, salt: 99)
        check(try rig.publish(snapshot(seq: 6, shape: 6, width: 2, height: 3, hotX: 0, hotY: 2, posX: 3, posY: 4), bytes: bytes) == 1,
              "new shape copied")
        rig.pump()
        check(rig.reader.cursor?.shapeSeq == 6 && rig.reader.cursor?.width == 2 && rig.reader.cursor?.height == 3, "new shape state")
        check(rig.uploads == 2 && rig.lastUpload == bytes, "the reader copied the new image")

        // Shape removed (device reset): seq grows, shape_seq 0, enabled 0, fields zero.
        check(try rig.publish(snapshot(seq: 7, shape: 0, posX: 0, posY: 0)) == 0, "no copy without a shape")
        rig.pump()
        check(rig.reader.cursor == AcceptedCursor(
            cursorSeq: 7, enabled: false, shapeSeq: 0, width: 0, height: 0, hotX: 0, hotY: 0, posX: 0, posY: 0), "no shape state")

        // Same cursor_seq again: nothing is published.
        check((try? rig.writer.publishCursor(snapshot(seq: 7, shape: 0)) { _, _ in nil }) == false, "unchanged cursor_seq is not republished")
        check(rig.writer.lastPublishedCursorSeq == 7, "last published cursor_seq")

        // The cursor never touched the frame side.
        check(rig.writer.lastPublishedGeneration == frameGenerationBefore, "frame generation unchanged")
        check(rig.reader.lastFrame == nil && rig.reader.acceptedCount == 0 && rig.reader.state == .noSignal,
              "the reader state and frames are unaffected by CURSOR (state \(rig.reader.state))")
        check(rig.writer.queuedBytes == 0, "queue empty after flush")
    }

    // MARK: 10.5 step 1: slot selection

    static func slotSelection() throws {
        let rig = try Rig()
        var front: Int?
        var seq: UInt64 = 0
        // Untrusted cursor_claim values, including those outside 1...3; a slot is always found and never front or claimed.
        for claim in [0, 1, 2, 3, 4, 0xFFFF_FFFF, 2, 1, 3, 0, 7, 1, 2, 3] as [UInt32] {
            rig.writer.base.storeBytes(of: claim, toByteOffset: 68, as: UInt32.self)
            seq += 1
            let published = try rig.publish(snapshot(seq: seq, posX: UInt32(seq % 60)))
            _ = published
            guard let slot = rig.slot(of: seq) else {
                check(false, "claim \(claim): cursor_seq \(seq) was not published")
                continue
            }
            if let front { check(slot != front, "claim \(claim): wrote into cursor_front \(front)") }
            if claim >= 1, claim <= 3 { check(slot != Int(claim) - 1, "claim \(claim): wrote into the claimed slot \(slot)") }
            front = slot
            rig.pump()
        }
        // The earlier publications are older than front and stay valid or are overwritten, but never the front one.
        check(rig.reader.cursor?.cursorSeq == seq, "reader follows the last update: \(String(describing: rig.reader.cursor))")
    }

    // MARK: 10.6 torn update

    static func tornUpdateIsDiscarded() throws {
        let rig = try Rig()
        try rig.publish(snapshot(seq: 1, posX: 1))
        rig.pump()
        check(rig.reader.cursor?.cursorSeq == 1, "baseline")
        let acceptedBefore = rig.reader.cursorAcceptedCount

        // A new shape: the writer rewrites the slot while the reader copies it (between g1 and g2).
        try rig.publish(snapshot(seq: 2, shape: 2, posX: 9), bytes: pattern(4, 4, salt: 50))
        let slot = rig.slot(of: 2)!
        var rewritten = false
        rig.pump(copy: { _, _ in
            // The writer starts a rewrite: invalid mark, then a newer publication in the same slot.
            rig.writer.base.advanced(by: 448 + 64 * slot).storeBytes(of: UInt64(3), as: UInt64.self)
            rewritten = true
        })
        check(rewritten, "the copy callback ran")
        check(rig.reader.cursor?.cursorSeq == 1 && rig.reader.cursorAcceptedCount == acceptedBefore,
              "a rewritten slot is discarded and the accepted state stays: \(String(describing: rig.reader.cursor))")
        check(!rig.reader.state.isTerminal, "a discarded update is not an error: \(rig.reader.state)")

        // The next, valid notification (a newer cursor_seq in another slot) is accepted and the shape is copied again,
        // because the discarded copy never became the accepted shape.
        let before = rig.uploads
        try rig.publish(snapshot(seq: 4, shape: 2, posX: 11), bytes: pattern(4, 4, salt: 50))
        rig.pump()
        check(rig.reader.cursor?.cursorSeq == 4 && rig.reader.cursor?.shapeSeq == 2, "recovered: \(String(describing: rig.reader.cursor))")
        check(rig.uploads == before + 1, "the shape was copied again after the discard")

        // Overwritten before g1: no copy at all, no error.
        try rig.publish(snapshot(seq: 5, posX: 12))
        let overwritten = rig.slot(of: 5)!
        rig.writer.base.advanced(by: 448 + 64 * overwritten).storeBytes(of: UInt64(0), as: UInt64.self)
        let uploads = rig.uploads
        rig.pump()
        check(rig.uploads == uploads && rig.reader.cursor?.cursorSeq == 4 && !rig.reader.state.isTerminal, "g1 differs: dropped quietly")
    }

    // MARK: 10.6 step 5 when the presenter cannot make a texture

    static func failedCopyIsNotAccepted() throws {
        let rig = try Rig()
        try rig.publish(snapshot(seq: 1, posX: 1))
        rig.pump()
        check(rig.reader.cursor?.shapeSeq == 1, "baseline")
        // A new shape whose copy fails: the update is not accepted, the shape is not remembered, it is not an error.
        rig.failCopies = true
        try rig.publish(snapshot(seq: 2, shape: 2, posX: 2), bytes: pattern(4, 4, salt: 20))
        rig.pump()
        check(rig.reader.cursor?.cursorSeq == 1 && rig.reader.cursor?.shapeSeq == 1, "failed copy: accepted state unchanged")
        check(!rig.reader.state.isTerminal, "failed copy is not a protocol error: \(rig.reader.state)")
        // The next update of the same shape copies again, because the shape was never accepted.
        rig.failCopies = false
        let before = rig.uploads
        try rig.publish(snapshot(seq: 3, shape: 2, posX: 3), bytes: pattern(4, 4, salt: 20))
        rig.pump()
        check(rig.reader.cursor?.cursorSeq == 3 && rig.reader.cursor?.shapeSeq == 2, "recovered after a failed copy")
        check(rig.uploads == before + 1 && rig.lastUpload == pattern(4, 4, salt: 20), "the image was copied again")
    }

    // MARK: 10.6 protocol errors, one by one

    static func protocolViolations() throws {
        func expectDisconnect(_ name: String, _ body: (Rig) throws -> Void) throws {
            let rig = try Rig()
            try body(rig)
            rig.pump()
            if case .disconnected = rig.reader.state {
                check(rig.reader.cursor == nil, "\(name): the cursor state dies with the connection")
            } else {
                check(false, "\(name): expected DISCONNECTED, got \(rig.reader.state)")
            }
        }
        func raw(_ rig: Rig, kind: UInt16 = 5, length: UInt32 = 16, reserved: UInt32 = 0, seq: UInt64, slot: UInt32, tail: UInt32 = 0) {
            var bytes = Array("ODFT".utf8) + [3, 0] + [UInt8(kind & 0xFF), UInt8(kind >> 8)] + le32(reserved) + le32(length)
            bytes += le64(seq) + le32(slot) + le32(tail)
            precondition(send(rig.hostFD, bytes, bytes.count, 0) == bytes.count)
        }
        func poke(_ rig: Rig, slot: Int, offset: Int, value: UInt32) {
            rig.writer.base.advanced(by: 448 + 64 * slot + offset).storeBytes(of: value, as: UInt32.self)
        }

        try expectDisconnect("slot index 3") { rig in raw(rig, seq: 1, slot: 3) }
        try expectDisconnect("slot index huge") { rig in raw(rig, seq: 1, slot: 0xFFFF_FFFF) }
        try expectDisconnect("cursor_seq 0") { rig in raw(rig, seq: 0, slot: 0) }
        try expectDisconnect("CURSOR reserved") { rig in raw(rig, seq: 1, slot: 0, tail: 1) }
        try expectDisconnect("header reserved") { rig in raw(rig, reserved: 1, seq: 1, slot: 0) }
        try expectDisconnect("wrong length") { rig in raw(rig, length: 24, seq: 1, slot: 0, tail: 0) }
        try expectDisconnect("cursor_seq does not grow") { rig in
            try rig.publish(snapshot(seq: 5))
            rig.pump()
            raw(rig, seq: 5, slot: 0)
        }
        // g1 older than N: the notification promises a publication that is not there.
        try expectDisconnect("g1 older than N") { rig in
            try rig.publish(snapshot(seq: 5))
            let slot = rig.slot(of: 5)!
            rig.writer.base.advanced(by: 448 + 64 * slot).storeBytes(of: UInt64(4), as: UInt64.self)
            raw(rig, seq: 5, slot: UInt32(slot))
        }
        // Descriptor violations with a stable cursor_seq (g1 = g2) are protocol errors; each field once.
        let mutations: [(String, Int, UInt32)] = [
            ("flags 2", 16, 2), ("width 0", 20, 0), ("width 65", 20, 65), ("height 0", 24, 0), ("height 65", 24, 65),
            ("format 1", 28, 1), ("format 0", 28, 0), ("hot_x == width", 32, 4), ("hot_y == height", 36, 4),
            ("pos_x == max_width", 40, 64), ("pos_y == max_height", 44, 48), ("pos_x huge", 40, 0xFFFF_FFFF),
            ("reserved", 48, 1), ("reserved tail", 60, 1),
        ]
        for (name, offset, value) in mutations {
            try expectDisconnect("descriptor \(name)") { rig in
                try rig.publish(snapshot(seq: 1))
                let slot = rig.slot(of: 1)!
                poke(rig, slot: slot, offset: offset, value: value)
                raw(rig, seq: 1, slot: UInt32(slot))
            }
        }
        // shape_seq = 0 with leftover shape fields; and ENABLED with no shape.
        try expectDisconnect("no shape but width") { rig in
            try rig.publish(snapshot(seq: 1, shape: 0))
            let slot = rig.slot(of: 1)!
            poke(rig, slot: slot, offset: 20, value: 4)
            raw(rig, seq: 1, slot: UInt32(slot))
        }
        try expectDisconnect("ENABLED without a shape") { rig in
            try rig.publish(snapshot(seq: 1, shape: 0))
            let slot = rig.slot(of: 1)!
            poke(rig, slot: slot, offset: 16, value: 1)
            raw(rig, seq: 1, slot: UInt32(slot))
        }
        // The valid extremes are accepted: 64x64, hotspot (63, 63), position at the frame limit - 1.
        let rig = try Rig()
        try rig.publish(snapshot(seq: 1, width: 64, height: 64, hotX: 63, hotY: 63, posX: 63, posY: 47))
        rig.pump()
        check(rig.reader.cursor?.width == 64 && rig.reader.cursor?.posX == 63 && rig.reader.cursor?.posY == 47, "extremes accepted")
        check(rig.lastUpload == pattern(64, 64), "a 16 KiB image slot is copied in full")
        // A position outside the announced frame limits is a host refusal in the writer, not a published descriptor.
        do {
            _ = try rig.writer.publishCursor(snapshot(seq: 2, posX: 64)) { _, _ in nil }
            check(false, "the writer must refuse a position beyond max_width")
        } catch {
            check(true, "refused: \(error)")
        }
        do {
            _ = try rig.writer.publishCursor(snapshot(seq: 2, width: 65, height: 1)) { _, _ in nil }
            check(false, "the writer must refuse a 65 pixel wide shape")
        } catch {
            check(true, "refused: \(error)")
        }
    }

    // MARK: 10.3.1, 10.3.2: each header value of v2 is checked by the reader

    /// A fake writer: its own shared memory and header, built here from the numbers of the spec, then mutated.
    static func headerOutcome(mutate: (UnsafeMutableRawPointer) -> Void, messageVersion: UInt16 = 3) -> FrameReaderState {
        let total = 65_536 + 2 * 16_384
        var fd: Int32 = -1
        var name = ""
        for _ in 0..<8 {
            name = "/odft.t" + String(UInt32.random(in: 0...UInt32.max), radix: 16)
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
        put(8, 64); put(12, 128); put(16, 64); put(20, 2); put(24, 65_536); put(28, 16_384)
        put(32, 1); put(36, 64); put(40, 48); put(44, 0)
        base.storeBytes(of: UInt64(total), toByteOffset: 48, as: UInt64.self)
        put(56, 384); put(60, 0)
        for (index, value) in [3, 64, 448, 16_384, 16_384, 64, 64, 2].enumerated() { put(384 + 4 * index, UInt32(value)) }
        mutate(base)

        let sockets = socketPair()
        let reader = FrameReader(fd: sockets.ui)
        reader.start()
        // ANNOUNCE with SCM_RIGHTS, built by hand.
        var message = Array("ODFT".utf8) + [UInt8(messageVersion & 0xFF), UInt8(messageVersion >> 8), 2, 0] + le32(0) + le32(24)
        // ANNOUNCE payload of section 11.4: total_size, slot_count, screen = 0, screen_count = 1, reserved = 0.
        message += le64(UInt64(total)) + le32(2) + le32(0) + le32(1) + le32(0)
        var control = [UInt8](repeating: 0, count: 16)
        control.withUnsafeMutableBytes { raw in
            raw.storeBytes(of: UInt32(16), toByteOffset: 0, as: UInt32.self)
            raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
            raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
            raw.storeBytes(of: fd, toByteOffset: 12, as: Int32.self)
        }
        message.withUnsafeMutableBytes { data in
            control.withUnsafeMutableBytes { ctl in
                var iov = iovec(iov_base: data.baseAddress, iov_len: data.count)
                withUnsafeMutablePointer(to: &iov) { iovPointer in
                    var header = msghdr()
                    header.msg_iov = iovPointer
                    header.msg_iovlen = 1
                    header.msg_control = ctl.baseAddress
                    header.msg_controllen = socklen_t(ctl.count)
                    precondition(sendmsg(sockets.host, &header, 0) == data.count)
                }
            }
        }
        Darwin.close(fd)
        // The viewer pumps in a loop; a fake ANNOUNCE needs two passes (header, then the 24-byte payload).
        for _ in 0..<4 where reader.state == .connecting {
            reader.pump(timeoutMs: 100, copy: { _, _, _ in true })
        }
        let state = reader.state
        reader.teardown()
        Darwin.close(sockets.host)
        return state
    }

    static func headerMutations() throws {
        check(headerOutcome(mutate: { _ in }) == .noSignal, "an untouched fake v3 writer is accepted")

        func incompatible(_ name: String, _ mutate: @escaping (UnsafeMutableRawPointer) -> Void) {
            if case .incompatible = headerOutcome(mutate: mutate) { return }
            check(false, "\(name): expected INCOMPATIBLE, got \(headerOutcome(mutate: mutate))")
        }
        func protocolError(_ name: String, _ mutate: @escaping (UnsafeMutableRawPointer) -> Void) {
            if case .disconnected = headerOutcome(mutate: mutate) { return }
            check(false, "\(name): expected DISCONNECTED, got \(headerOutcome(mutate: mutate))")
        }
        func set(_ offset: Int, _ value: UInt32) -> (UnsafeMutableRawPointer) -> Void {
            { $0.storeBytes(of: value, toByteOffset: offset, as: UInt32.self) }
        }
        incompatible("memory version 1") { $0.storeBytes(of: UInt16(1), toByteOffset: 4, as: UInt16.self) }
        incompatible("memory version 2") { $0.storeBytes(of: UInt16(2), toByteOffset: 4, as: UInt16.self) }
        incompatible("slots_offset 16384 (v1 layout)", set(24, 16_384))
        incompatible("cursor_offset 0", set(56, 0))
        incompatible("cursor_offset 448", set(56, 448))
        let names = ["cursor_slot_count", "cursor_descriptor_size", "cursor_descriptors_offset", "cursor_slots_offset",
                     "cursor_slot_size", "cursor_max_width", "cursor_max_height", "cursor_format"]
        for (index, name) in names.enumerated() {
            incompatible(name, set(384 + 4 * index, 1))
        }
        incompatible("cursor_format 1 (BGRX)", set(384 + 28, 1))
        protocolError("header reserved", set(60, 1))
        protocolError("cursor header reserved", set(384 + 32, 1))
        protocolError("cursor header reserved tail", set(384 + 60, 1))
        // Message version: a v1 VMHost (and any other) is INCOMPATIBLE by the bytes 0-5 rule (8.1), whatever follows.
        if case .incompatible = headerOutcome(mutate: { _ in }, messageVersion: 1) {} else {
            check(false, "ANNOUNCE with message version 1: expected INCOMPATIBLE")
        }
        if case .incompatible = headerOutcome(mutate: { _ in }, messageVersion: 2) {} else {
            check(false, "ANNOUNCE with message version 2: expected INCOMPATIBLE")
        }
    }

    // MARK: 10.5 notification queue

    static func notificationQueue() throws {
        // Replacement and order, through the real writer: an unsent CURSOR is replaced by the next one, MODE does not
        // delete a pending CURSOR and goes first; nothing else is sent.
        let rig = try Rig()
        try rig.publish(snapshot(seq: 1, posX: 1))
        try rig.publish(snapshot(seq: 2, posX: 2))
        try rig.publish(snapshot(seq: 3, posX: 3))
        check(rig.writer.queuedBytes == 32, "one pending CURSOR (32 bytes) after three publications: \(rig.writer.queuedBytes)")
        try rig.writer.queueMode(count: 2, active: true, width: 64, height: 48, stride: 256, format: 1)
        check(rig.writer.queuedBytes == 48 + 32, "MODE does not remove the pending CURSOR: \(rig.writer.queuedBytes)")
        // Flush to a plain socket pair so that the bytes can be read raw.
        let wire = socketPair()
        try rig.writer.flush(fd: wire.host)
        var received = [UInt8](repeating: 0, count: 200)
        let n = recv(wire.ui, &received, 200, 0)
        Darwin.close(wire.host)
        Darwin.close(wire.ui)
        // Order on the wire: MODE (kind 3, 48 bytes), then CURSOR (kind 5, 32 bytes) carrying cursor_seq 3.
        check(n == 80, "80 bytes arrived: \(n)")
        check(received[6] == 3 && received[7] == 0 && received[12] == 32, "first MODE")
        check(received[48 + 6] == 5 && received[48 + 12] == 16, "then CURSOR")
        check(Array(received[64..<72]) == le64(3), "the CURSOR that arrived carries cursor_seq 3 (older ones were replaced)")

        // The bound: partial 48 + MODE 48 + FRAME 40 + CURSOR 32 = 168 bytes.
        var queue = NotificationQueue()
        queue.queueMode(screen: 0, [UInt8](repeating: 0, count: 48))
        queue.queueFrame(screen: 0, [UInt8](repeating: 0, count: 40))
        queue.queueCursor([UInt8](repeating: 0, count: 32))
        check(queue.bufferedBytes == 120, "pending bytes: \(queue.bufferedBytes)")
        queue.queueCursor([UInt8](repeating: 0, count: 32))
        queue.queueFrame(screen: 0, [UInt8](repeating: 0, count: 40))
        queue.queueMode(screen: 0, [UInt8](repeating: 0, count: 48)) // drops the pending FRAME, keeps the CURSOR
        check(queue.bufferedBytes == 48 + 32, "MODE drops FRAME, keeps CURSOR: \(queue.bufferedBytes)")
        // A partially sent MODE (48) plus a full set of pending messages is the 168-byte bound.
        var bounded = NotificationQueue()
        let sockets = socketPair()
        var sndbuf: Int32 = 1
        setsockopt(sockets.host, SOL_SOCKET, SO_SNDBUF, &sndbuf, 4)
        let flags = fcntl(sockets.host, F_GETFL)
        _ = fcntl(sockets.host, F_SETFL, flags | O_NONBLOCK)
        // Fill the socket so the next message stays (partly) unsent.
        let filler = [UInt8](repeating: 0, count: 1024)
        while send(sockets.host, filler, filler.count, 0) > 0 {}
        bounded.queueMode(screen: 0, [UInt8](repeating: 1, count: 48))
        try bounded.flush(fd: sockets.host) // EAGAIN: nothing leaves, or part of it does
        bounded.queueMode(screen: 0, [UInt8](repeating: 2, count: 48))
        bounded.queueFrame(screen: 0, [UInt8](repeating: 3, count: 40))
        bounded.queueCursor([UInt8](repeating: 4, count: 32))
        check(bounded.bufferedBytes <= 168, "bound 168 bytes: \(bounded.bufferedBytes)")
        Darwin.close(sockets.host)
        Darwin.close(sockets.ui)
    }
}

/// XCTest wrapper (S11-05a): each former step of `main()` is a test case; the checks above are unchanged and still count failures.
final class CursorTransportXCTests: XCTestCase {
    override func setUp() {
        failures = 0
    }

    private func finish(_ name: String) {
        XCTAssertEqual(failures, 0, "cursor transport \(name): \(failures) check(s) failed (see FAIL lines above)")
    }

    func testLayoutConstants() throws {
        try CursorTransportTests.layoutConstants()
        finish("layoutConstants")
    }

    func testRoundTripAndShapeCopies() throws {
        try CursorTransportTests.roundTripAndShapeCopies()
        finish("roundTripAndShapeCopies")
    }

    func testSlotSelection() throws {
        try CursorTransportTests.slotSelection()
        finish("slotSelection")
    }

    func testTornUpdateIsDiscarded() throws {
        try CursorTransportTests.tornUpdateIsDiscarded()
        finish("tornUpdateIsDiscarded")
    }

    func testFailedCopyIsNotAccepted() throws {
        try CursorTransportTests.failedCopyIsNotAccepted()
        finish("failedCopyIsNotAccepted")
    }

    func testProtocolViolations() throws {
        try CursorTransportTests.protocolViolations()
        finish("protocolViolations")
    }

    func testHeaderMutations() throws {
        try CursorTransportTests.headerMutations()
        finish("headerMutations")
    }

    func testNotificationQueue() throws {
        try CursorTransportTests.notificationQueue()
        finish("notificationQueue")
    }
}
