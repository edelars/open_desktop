import Darwin
import Foundation
import XCTest
@testable import ODShared
@testable import ODDisplay

// Unit tests of the input transport (S10-16, protocols/control/INPUT_TRANSPORT_V1.md section 12): messages, queue rules and the
// invariant, the kVK_* table, point/pixel translation, focus release. No AppKit, no VMHost process. XCTest case of the
// OpenDesktopMacTests target (S11-05a); run with `swift test --filter InputXCTests` from apps/. It used to be an executable with
// `@main`; the checks are unchanged. The UI end of the channel (`InputChannel`) is in the viewer target, imported with @testable.

private var failures = 0
private var checks = 0

private func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    checks += 1
    if !condition {
        failures += 1
        print("FAIL: \(message())")
    }
}

/// Deterministic generator (xorshift64*), fixed seed in every use.
private struct Generator {
    var state: UInt64
    mutating func next() -> UInt64 {
        state ^= state >> 12
        state ^= state << 25
        state ^= state >> 27
        return state &* 2685821657736338717
    }
    mutating func below(_ bound: Int) -> Int { Int(next() % UInt64(bound)) }
}

struct InputTests {
    // MARK: Protocol

    static func hex(_ bytes: [UInt8]) -> String { bytes.map { String(format: "%02x", $0) }.joined(separator: " ") }

    static func messageLayout() {
        let sizes: [(InputMessage, Int)] = [
            (.version, 16), (.hello, 16), (.accept(flags: 0), 24), (.focus(gained: true), 24),
            (.usage(InputUsage(page: 7, id: 4, down: true)), 24),
            (.motion(motion(dx: 1)), 64), (.status(InputStatus()), 48),
        ]
        for (message, size) in sizes {
            let bytes = message.encoded()
            check(bytes.count == size, "\(message.kind) is \(bytes.count) bytes, expected \(size)")
            check(bytes.count == message.kind.totalLength, "\(message.kind) total length")
            check(Array(bytes[0..<4]) == [0x4f, 0x44, 0x49, 0x54], "\(message.kind) magic")
        }
        // Byte-exact samples (section 4): header, then payload, little-endian.
        check(hex(InputMessage.hello.encoded()) == "4f 44 49 54 01 00 01 00 00 00 00 00 00 00 00 00", "HELLO bytes")
        check(
            hex(InputMessage.usage(InputUsage(page: 0x09, id: 2, down: true)).encoded())
                == "4f 44 49 54 01 00 04 00 00 00 00 00 08 00 00 00 09 00 02 00 01 00 00 00", "USAGE bytes")
        var m = InputMotion()
        m.modeChangeCount = 0x0102030405060708
        m.dx = -2
        m.dy = 3
        m.wheel = 120
        m.pan = -120
        m.absX = 5
        m.absY = 6
        m.absWidth = 64
        m.absHeight = 48
        m.absValid = true
        let motionBytes = InputMessage.motion(m).encoded()
        check(
            hex(Array(motionBytes[16...]))
                == "08 07 06 05 04 03 02 01 fe ff ff ff 03 00 00 00 78 00 00 00 88 ff ff ff 05 00 00 00 06 00 00 00 40 00 00 00 30 00 00 00 01 00 00 00 00 00 00 00",
            "MOTION payload \(hex(Array(motionBytes[16...])))")
        let status = InputMessage.status(InputStatus(rejectedDowns: 1, mergedMotions: 2, droppedUnfocused: 3, queueDepth: 4)).encoded()
        check(hex(Array(status[16...])) == "01 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00 03 00 00 00 00 00 00 00 04 00 00 00 00 00 00 00", "STATUS payload")
        // Round trips through the decoder of the right direction.
        var toHost = InputDecoder(direction: .uiToHost)
        let up: [InputMessage] = [
            .hello, .focus(gained: true), .usage(InputUsage(page: 7, id: 0xE7, down: false)), .motion(m), .motion(motion(wheel: -5)),
            .focus(gained: false),
        ]
        let decodedUp = try? toHost.append(up.flatMap { $0.encoded() })
        check(decodedUp == up, "UI -> VMHost round trip")
        var toUI = InputDecoder(direction: .hostToUI)
        let down: [InputMessage] = [.accept(flags: 1), .accept(flags: 0), .status(InputStatus(rejectedDowns: .max, mergedMotions: 7, droppedUnfocused: 8, queueDepth: 256))]
        let decodedDown = try? toUI.append(down.flatMap { $0.encoded() })
        check(decodedDown == down, "VMHost -> UI round trip")
    }

    static func motion(dx: Int32 = 0, dy: Int32 = 0, wheel: Int32 = 0, pan: Int32 = 0) -> InputMotion {
        var m = InputMotion()
        m.dx = dx
        m.dy = dy
        m.wheel = wheel
        m.pan = pan
        return m
    }

    static func absoluteMotion(mode: UInt64 = 1, x: UInt32 = 3, y: UInt32 = 4, width: UInt32 = 64, height: UInt32 = 48) -> InputMotion {
        var m = InputMotion()
        m.absValid = true
        m.modeChangeCount = mode
        m.absX = x
        m.absY = y
        m.absWidth = width
        m.absHeight = height
        return m
    }

    static func expectError(_ bytes: [UInt8], direction: InputDirection, _ expected: (InputProtocolError) -> Bool, _ label: String) {
        var decoder = InputDecoder(direction: direction)
        do {
            _ = try decoder.append(bytes)
            check(false, "\(label): no error")
        } catch let error as InputProtocolError {
            check(expected(error), "\(label): wrong error \(error)")
        } catch {
            check(false, "\(label): foreign error \(error)")
        }
    }

    static func isMalformed(_ error: InputProtocolError) -> Bool { if case .malformed = error { return true } else { return false } }

    static func headerChecks() {
        let hello = InputMessage.hello.encoded()
        var bad = hello
        bad[0] = 0x58
        expectError(bad, direction: .uiToHost, { $0 == .badMagic }, "foreign magic")
        bad = hello
        bad[4] = 2
        expectError(bad, direction: .uiToHost, { $0 == .incompatibleVersion(2) }, "version 2")
        bad = hello
        bad[4] = 2
        bad[6] = 0xFF // other fields are not looked at after a foreign version
        expectError(bad, direction: .uiToHost, { $0 == .incompatibleVersion(2) }, "version 2 with garbage kind")
        bad = hello
        bad[6] = 7
        expectError(bad, direction: .uiToHost, isMalformed, "unknown kind 7")
        bad = hello
        bad[6] = 0x34
        bad[7] = 0x12
        expectError(bad, direction: .uiToHost, isMalformed, "unknown kind 0x1234")
        expectError(InputMessage.status(InputStatus()).encoded(), direction: .uiToHost, isMalformed, "STATUS sent to VMHost")
        expectError(InputMessage.accept(flags: 0).encoded(), direction: .uiToHost, isMalformed, "ACCEPT sent to VMHost")
        expectError(InputMessage.hello.encoded(), direction: .hostToUI, isMalformed, "HELLO sent to the UI")
        expectError(InputMessage.motion(motion(dx: 1)).encoded(), direction: .hostToUI, isMalformed, "MOTION sent to the UI")
        expectError(InputMessage.version.encoded(), direction: .hostToUI, isMalformed, "VERSION with version 1 is an error")
        bad = hello
        bad[8] = 1
        expectError(bad, direction: .uiToHost, isMalformed, "reserved != 0")
        bad = hello
        bad[12] = 8 // HELLO with a payload length
        expectError(bad, direction: .uiToHost, isMalformed, "HELLO payload_length 8")
        var usage = InputMessage.usage(InputUsage(page: 7, id: 4, down: true)).encoded()
        usage[12] = 0
        expectError(Array(usage[0..<16]), direction: .uiToHost, isMalformed, "USAGE payload_length 0")
        // Before HELLO the decision is made by the first 16 bytes: a FOCUS or USAGE header without HELLO is an error at once.
        var first = InputDecoder(direction: .uiToHost, requiredFirstKind: .hello)
        do {
            _ = try first.append(Array(InputMessage.focus(gained: true).encoded()[0..<16]))
            check(false, "FOCUS header before HELLO: no error")
        } catch { check(true, "FOCUS header before HELLO is rejected without its payload") }
        first = InputDecoder(direction: .uiToHost, requiredFirstKind: .hello)
        check((try? first.append(InputMessage.hello.encoded())) == [.hello], "HELLO first is accepted")
        check((try? first.append(InputMessage.focus(gained: true).encoded())) == [.focus(gained: true)], "FOCUS after HELLO is accepted")
        // The header is checked before the payload: a bad header is rejected from 16 bytes alone.
        var motionHeader = Array(InputMessage.motion(motion(dx: 1)).encoded()[0..<16])
        motionHeader[12] = 47
        expectError(motionHeader, direction: .uiToHost, isMalformed, "MOTION payload_length 47 (rejected before the payload)")
    }

    static func payloadChecks() {
        func usage(page: UInt16, id: UInt16, state: UInt32 = 1) -> [UInt8] {
            var bytes = Array(InputMessage.usage(InputUsage(page: 7, id: 4, down: true)).encoded()[0..<16])
            bytes.inAppendLE(page)
            bytes.inAppendLE(id)
            bytes.inAppendLE(state)
            return bytes
        }
        var valid = 0
        for id in UInt16(0)...0x100 {
            var decoder = InputDecoder(direction: .uiToHost)
            if (try? decoder.append(usage(page: 7, id: id))) != nil { valid += 1 }
        }
        let expectedKeyboard = (0xA4 - 0x04 + 1) + (0xDD - 0xB0 + 1) + 8
        check(valid == expectedKeyboard, "keyboard usages accepted: \(valid), expected \(expectedKeyboard)")
        for id in [0, 1, 2, 3, 0xA5, 0xAF, 0xDE, 0xDF, 0xE8, 0xFFFF] as [UInt16] {
            expectError(usage(page: 7, id: id), direction: .uiToHost, isMalformed, "keyboard usage \(id)")
        }
        for id in 1...5 as ClosedRange<UInt16> {
            var decoder = InputDecoder(direction: .uiToHost)
            check((try? decoder.append(usage(page: 9, id: id))) != nil, "button \(id) accepted")
        }
        for id in [0, 6, 0x100] as [UInt16] {
            expectError(usage(page: 9, id: id), direction: .uiToHost, isMalformed, "button \(id)")
        }
        for page in [0, 1, 8, 0x0C, 0xFF00] as [UInt16] {
            expectError(usage(page: page, id: 4), direction: .uiToHost, isMalformed, "page \(page)")
        }
        expectError(usage(page: 7, id: 4, state: 2), direction: .uiToHost, isMalformed, "USAGE state 2")
        // FOCUS and ACCEPT.
        var focus = InputMessage.focus(gained: true).encoded()
        focus[16] = 2
        expectError(focus, direction: .uiToHost, isMalformed, "FOCUS state 2")
        focus = InputMessage.focus(gained: true).encoded()
        focus[20] = 1
        expectError(focus, direction: .uiToHost, isMalformed, "FOCUS reserved")
        var accept = InputMessage.accept(flags: 0).encoded()
        accept[16] = 2
        expectError(accept, direction: .hostToUI, isMalformed, "ACCEPT undefined flag bit")
        // MOTION rules (4.2).
        expectError(InputMessage.motion(InputMotion()).encoded(), direction: .uiToHost, isMalformed, "empty MOTION")
        var decoder = InputDecoder(direction: .uiToHost)
        check((try? decoder.append(InputMessage.motion(absoluteMotion()).encoded())) != nil, "MOTION with zero deltas and ABS_VALID is allowed")
        expectError(InputMessage.motion(absoluteMotion(x: 64)).encoded(), direction: .uiToHost, isMalformed, "abs_x == abs_width")
        expectError(InputMessage.motion(absoluteMotion(y: 48)).encoded(), direction: .uiToHost, isMalformed, "abs_y == abs_height")
        expectError(InputMessage.motion(absoluteMotion(width: 4097, height: 48)).encoded(), direction: .uiToHost, isMalformed, "abs_width 4097")
        expectError(InputMessage.motion(absoluteMotion(width: 64, height: 2305)).encoded(), direction: .uiToHost, isMalformed, "abs_height 2305")
        expectError(InputMessage.motion(absoluteMotion(width: 0, height: 0, )).encoded(), direction: .uiToHost, isMalformed, "abs size 0")
        decoder = InputDecoder(direction: .uiToHost)
        check(
            (try? decoder.append(InputMessage.motion(absoluteMotion(x: 4095, y: 2303, width: 4096, height: 2304)).encoded())) != nil,
            "largest absolute position")
        var absWithoutFlag = motion(dx: 1)
        absWithoutFlag.absX = 1
        expectError(InputMessage.motion(absWithoutFlag).encoded(), direction: .uiToHost, isMalformed, "abs_x without ABS_VALID")
        absWithoutFlag = motion(dx: 1)
        absWithoutFlag.modeChangeCount = 3
        expectError(InputMessage.motion(absWithoutFlag).encoded(), direction: .uiToHost, isMalformed, "mode_change_count without ABS_VALID")
        var flagged = InputMessage.motion(absoluteMotion()).encoded()
        flagged[16 + 40] = 3
        expectError(flagged, direction: .uiToHost, isMalformed, "MOTION undefined flag bit")
        var reserved = InputMessage.motion(motion(dx: 1)).encoded()
        reserved[16 + 44] = 1
        expectError(reserved, direction: .uiToHost, isMalformed, "MOTION reserved")
        var statusReserved = InputMessage.status(InputStatus()).encoded()
        statusReserved[16 + 28] = 1
        expectError(statusReserved, direction: .hostToUI, isMalformed, "STATUS reserved")
    }

    static func decoderStreaming() {
        let messages: [InputMessage] = [.hello, .focus(gained: true), .motion(absoluteMotion()), .usage(InputUsage(page: 9, id: 1, down: true))]
        let stream = messages.flatMap { $0.encoded() }
        var decoder = InputDecoder(direction: .uiToHost)
        var got: [InputMessage] = []
        for byte in stream {
            if let part = try? decoder.append([byte]) { got += part } else { check(false, "byte-wise feed failed") }
            check(decoder.bufferedByteCount < 64, "decoder buffers less than one largest message")
        }
        check(got == messages, "byte-wise stream")
        check(decoder.bufferedByteCount == 0, "nothing left over")
    }

    // MARK: Queue (section 7.3)

    static func queueRules() {
        var q = InputQueue()
        check(q.focus(gained: true) == .queued && q.state == .focused, "GAINED queued")
        check(q.focus(gained: true) != .queued, "repeated GAINED is a violation")
        // MOTION merges with a MOTION tail, saturating; the absolute part comes from the newer one.
        check(q.motion(motion(dx: Int32.max - 1, wheel: 5)) == .queued, "first MOTION is a new record")
        check(q.motion(absoluteMotion(mode: 9, x: 1, y: 2)) == .merged, "second MOTION merges")
        var merged = motion(dx: 10, dy: -4)
        merged.absValid = false
        check(q.motion(merged) == .merged, "third MOTION merges")
        if case let .motion(m)? = q.peekTail {
            check(m.dx == Int32.max && m.dy == -4 && m.wheel == 5, "saturating sum: \(m)")
            check(!m.absValid && m.modeChangeCount == 0, "absolute part is the newest one")
        } else {
            check(false, "tail is a MOTION")
        }
        check(q.mergedMotions == 2 && q.count == 2, "counters after merges: merged \(q.mergedMotions) count \(q.count)")
        var negative = InputQueue()
        _ = negative.focus(gained: true)
        _ = negative.motion(motion(dx: Int32.min + 1))
        _ = negative.motion(motion(dx: -5))
        if case let .motion(m)? = negative.peekTail { check(m.dx == Int32.min, "negative saturation") }
        // USAGE is never merged or reordered, DOWN/UP pairs.
        var u = InputQueue()
        _ = u.focus(gained: true)
        check(u.usage(page: 7, id: 4, down: true) == .queued, "DOWN queued")
        check(u.usage(page: 7, id: 4, down: true) != .queued, "DOWN of a pressed usage is a violation")
        check(u.usage(page: 7, id: 5, down: false) != .queued, "UP of a usage that is not pressed is a violation")
        check(u.usage(page: 7, id: 4, down: false) == .queued, "UP queued")
        check(u.pressedCount == 0, "h back to 0")
        check(u.usage(page: 7, id: 0xA5, down: true) != .queued, "invalid usage")
        // Refusal of DOWN: fill the queue with presses until 2h + 5 > free.
        var f = InputQueue()
        _ = f.focus(gained: true)
        var accepted = 0
        var refused: [UInt16] = []
        for id in UInt16(4)...UInt16(0xA4) {
            let before = (f.free, f.pressedCount)
            let outcome = f.usage(page: 7, id: id, down: true)
            let expectAccept = before.0 >= 2 * before.1 + 5
            check((outcome == .queued) == expectAccept, "DOWN \(id): free \(before.0) h \(before.1) outcome \(outcome)")
            if outcome == .queued { accepted += 1 } else { refused.append(id) }
            check(f.invariantHolds, "invariant after DOWN \(id)")
        }
        // free = 256 - 1 (GAINED) - h must be >= 2h + 5, i.e. h <= 83: the 84th DOWN is the last one accepted.
        check(accepted == 84 && !refused.isEmpty, "DOWNs accepted before the first refusal: \(accepted)")
        check(f.rejectedDowns == UInt64(refused.count), "rejected counter")
        // UP of every pressed usage and of every refused one: pressed ones are queued, refused ones are silent.
        var queuedUps = 0
        for id in UInt16(4)...UInt16(0xA4) {
            let outcome = f.usage(page: 7, id: id, down: false)
            if refused.contains(id) { check(outcome == .releasedSuppressed, "UP of refused \(id) is silent: \(outcome)") }
            else { check(outcome == .queued, "UP of pressed \(id): \(outcome)"); queuedUps += 1 }
            check(f.invariantHolds, "invariant after UP \(id)")
        }
        check(queuedUps == accepted && f.pressedCount == 0 && f.suppressedCount == 0, "everything released")
        check(f.focus(gained: false) == .queued && f.state == .unfocused, "FOCUS LOST queued")
        // FOCUS LOST releases everything without separate UPs.
        var r = InputQueue()
        _ = r.focus(gained: true)
        _ = r.usage(page: 7, id: 4, down: true)
        _ = r.usage(page: 9, id: 1, down: true)
        check(r.pressedCount == 2, "two pressed")
        check(r.focus(gained: false) == .queued, "LOST queued")
        check(r.pressedCount == 0 && r.state == .unfocused, "LOST clears h")
        check(r.records.filter { if case let .usage(x) = $0 { return !x.down } else { return false } }.isEmpty, "no UP was queued by LOST")
        check(r.usage(page: 7, id: 4, down: false) != .queued, "UP after LOST is a violation")
        check(r.motion(motion(dx: 1)) != .queued, "MOTION while unfocused is a violation")
    }

    static func queuePendingRules() {
        // Fill the queue with non-mergeable records until FOCUS GAINED has fewer than 3 free records.
        var q = InputQueue()
        _ = q.focus(gained: true)
        // 126 presses leave the queue with few free entries: use focus cycles to fill instead.
        var filler = InputQueue()
        _ = filler.focus(gained: true)
        var cycles = 0
        while filler.free >= 3 {
            _ = filler.focus(gained: false)
            if filler.free < 3 { break }
            let outcome = filler.focus(gained: true)
            check(outcome == .queued || outcome == .focusPending, "cycle \(cycles)")
            cycles += 1
            if filler.state == .gainedPending { break }
        }
        check(filler.state == .gainedPending || filler.free < 3, "queue is nearly full: free \(filler.free) state \(filler.state)")
        if filler.state != .gainedPending {
            // LOST was queued last: the state is UNFOCUSED and GAINED does not fit.
            check(filler.focus(gained: true) == .focusPending, "GAINED with free < 3 becomes pending")
        }
        check(filler.state == .gainedPending, "state GAINED_PENDING")
        check(filler.motion(motion(dx: 1)) == .droppedPending && filler.droppedUnfocused == 1, "MOTION dropped and counted")
        check(filler.usage(page: 7, id: 4, down: true) == .droppedPending && filler.droppedUnfocused == 2, "DOWN dropped and counted")
        check(filler.usage(page: 7, id: 4, down: true) != .droppedPending, "second DOWN of a suppressed usage is a violation")
        check(filler.usage(page: 7, id: 4, down: false) == .releasedSuppressed, "UP removes it from suppressed")
        check(filler.usage(page: 7, id: 5, down: false) != .releasedSuppressed, "UP of an unknown usage is a violation")
        // After a removal from the head with free >= 3 the postponed GAINED is queued and the state is FOCUSED.
        var guardCount = 0
        while filler.state == .gainedPending, guardCount < 300 {
            _ = filler.pop()
            guardCount += 1
        }
        check(filler.state == .focused, "pending resolved after pops")
        check(filler.peekTail == .focus(gained: true), "the postponed GAINED is the tail")
        check(filler.invariantHolds, "invariant after the pending state ends")
        // LOST only cancels the wait.
        var c = InputQueue()
        _ = c.focus(gained: true)
        while c.free >= 3 {
            _ = c.focus(gained: false)
            _ = c.focus(gained: true)
            if c.state == .gainedPending { break }
        }
        if c.state != .gainedPending { _ = c.focus(gained: true) }
        let depth = c.count
        check(c.state == .gainedPending, "pending again")
        check(c.focus(gained: false) == .cancelledPending && c.state == .unfocused && c.count == depth, "LOST in pending queues nothing")
    }

    // MARK: Random sequences (section 12 item 3)

    static func queueInvariantRandom() {
        var g = Generator(state: 0x9E3779B97F4A7C15)
        var q = InputQueue()
        var modelRejected: UInt64 = 0
        var modelMerged: UInt64 = 0
        var modelDropped: UInt64 = 0
        var pressedModel = Set<Int>() // slots the producer believes are pressed (queued DOWN)
        var suppressedModel = Set<Int>()
        let usages: [(UInt16, UInt16)] = (4...0xA4).map { (7, UInt16($0)) } + (1...5).map { (9, UInt16($0)) }
        var maxCount = 0
        var upRefusals = 0
        var motionRefusals = 0
        var lostRefusals = 0
        var consumerEvery = 0
        var downBias = 30
        let steps = 1_000_000
        for step in 0..<steps {
            if step % 20_000 == 0 {
                // Phases: heavy pressing without a consumer, then light pressing, sometimes a consumer.
                downBias = [10, 30, 70, 90][g.below(4)]
                consumerEvery = [0, 0, 7, 50][g.below(4)]
            }
            if consumerEvery > 0, step % consumerEvery == 0 { _ = q.pop() }
            let roll = g.below(100)
            switch q.state {
            case .unfocused:
                let outcome = q.focus(gained: true)
                check(outcome == .queued || outcome == .focusPending, "GAINED outcome \(outcome) at step \(step)")
            case .gainedPending, .focused:
                let pending = q.state == .gainedPending
                if roll < 3 {
                    let before = q.state
                    let outcome = q.focus(gained: false)
                    if before == .focused { if outcome != .queued { lostRefusals += 1 } }
                    pressedModel.removeAll()
                    suppressedModel.removeAll()
                } else if roll < 3 + downBias / 2 {
                    // A DOWN of a usage that is neither pressed nor suppressed (a well-behaved producer).
                    let (page, id) = usages[g.below(usages.count)]
                    let slot = InputUsage.slot(page: page, id: id)!
                    if pressedModel.contains(slot) || suppressedModel.contains(slot) { continue }
                    let before = (q.free, q.pressedCount)
                    let outcome = q.usage(page: page, id: id, down: true)
                    if pending {
                        check(outcome == .droppedPending, "pending DOWN \(outcome)")
                        modelDropped += 1
                        suppressedModel.insert(slot)
                    } else if before.0 >= 2 * before.1 + 5 {
                        check(outcome == .queued, "DOWN with room: \(outcome) at step \(step)")
                        pressedModel.insert(slot)
                    } else {
                        check(outcome == .rejectedDown, "DOWN without room: \(outcome) at step \(step)")
                        modelRejected += 1
                        suppressedModel.insert(slot)
                    }
                } else if roll < 3 + downBias {
                    // UP of a usage the producer holds or that was suppressed.
                    let pool = pressedModel.union(suppressedModel).sorted() // sorted: Set order differs between runs
                    if pool.isEmpty { continue }
                    let slot = pool[g.below(pool.count)]
                    let (page, id) = slot < 256 ? (UInt16(7), UInt16(slot)) : (UInt16(9), UInt16(slot - 255))
                    let outcome = q.usage(page: page, id: id, down: false)
                    if pressedModel.contains(slot) {
                        if pending { check(false, "pressed usage in pending state") }
                        if outcome != .queued { upRefusals += 1 }
                        pressedModel.remove(slot)
                    } else {
                        check(outcome == .releasedSuppressed, "UP of suppressed: \(outcome)")
                        suppressedModel.remove(slot)
                    }
                } else {
                    var m = motion(dx: Int32(g.below(2001)) - 1000, dy: Int32(g.below(21)) - 10)
                    if g.below(3) == 0 { m = absoluteMotion(mode: UInt64(g.below(3)), x: UInt32(g.below(64)), y: UInt32(g.below(48))) }
                    let wasTailMotion: Bool
                    if case .motion? = q.peekTail { wasTailMotion = true } else { wasTailMotion = false }
                    let outcome = q.motion(m)
                    if pending {
                        check(outcome == .droppedPending, "pending MOTION \(outcome)")
                        modelDropped += 1
                    } else if wasTailMotion {
                        check(outcome == .merged, "MOTION onto a MOTION tail: \(outcome)")
                        modelMerged += 1
                    } else {
                        if outcome != .queued { motionRefusals += 1 }
                    }
                }
            }
            check(q.invariantHolds, "invariant free >= 2h + m + 1 broken at step \(step): free \(q.free) h \(q.pressedCount) m \(q.tailMotionTerm)")
            check(q.count <= InputQueue.capacity && q.storageRecordCount == InputQueue.capacity, "queue storage grew at step \(step)")
            maxCount = max(maxCount, q.count)
            if failures > 20 { return }
        }
        check(upRefusals == 0 && motionRefusals == 0 && lostRefusals == 0,
              "UP/MOTION/LOST refusals in focus: up \(upRefusals) motion \(motionRefusals) lost \(lostRefusals)")
        check(q.rejectedDowns == modelRejected, "rejected_downs \(q.rejectedDowns) vs model \(modelRejected)")
        check(q.mergedMotions == modelMerged, "merged_motions \(q.mergedMotions) vs model \(modelMerged)")
        check(q.droppedUnfocused == modelDropped, "dropped_unfocused \(q.droppedUnfocused) vs model \(modelDropped)")
        check(maxCount == InputQueue.capacity || maxCount > 200, "the random run reached a full queue (max \(maxCount))")
        print("random queue run: \(steps) steps, max depth \(maxCount), rejected \(q.rejectedDowns), merged \(q.mergedMotions), dropped \(q.droppedUnfocused)")
    }

    /// No consumer: depth never exceeds 256 and storage never grows (7.1), also with only focus toggles and keys.
    static func queueNoConsumerGrowth() {
        var g = Generator(state: 0xD1B54A32D192ED03)
        var q = InputQueue()
        var held = Set<Int>()
        for _ in 0..<200_000 {
            switch q.state {
            case .unfocused:
                _ = q.focus(gained: true)
            default:
                let roll = g.below(10)
                if roll == 0 {
                    _ = q.focus(gained: false)
                    held.removeAll()
                } else if roll < 6 {
                    let id = UInt16(4 + g.below(200))
                    let slot = Int(id)
                    if !held.contains(slot) {
                        _ = q.usage(page: 7, id: id, down: true)
                        held.insert(slot)
                    }
                } else if let slot = held.min() {
                    _ = q.usage(page: 7, id: UInt16(slot), down: false)
                    held.remove(slot)
                }
            }
            check(q.count <= 256 && q.storageRecordCount == 256, "depth \(q.count)")
        }
    }

    /// Section 8.2 item 4 at the queue exit.
    static func queueExitRecheck() {
        var q = InputQueue()
        _ = q.focus(gained: true)
        _ = q.motion(absoluteMotion(mode: 4)) // click position, zero deltas
        _ = q.usage(page: 9, id: 1, down: true)
        _ = q.motion(absoluteMotion(mode: 4, x: 9, y: 9))
        var withDelta = absoluteMotion(mode: 4, x: 5, y: 5)
        withDelta.dx = 7
        _ = q.usage(page: 9, id: 1, down: false)
        _ = q.motion(withDelta)
        let current = InputDisplayMode(modeChangeCount: 5, width: 64, height: 48) // the mode changed from 4 to 5 while they waited
        var out: [InputRecord] = []
        while let record = q.popForConsumer(currentMode: current) { out.append(record) }
        // focus, [MOTION dropped], DOWN, [MOTION dropped], UP, MOTION with the delta and no absolute part.
        check(out.count == 4, "records at the exit: \(out.map { InputHostChannelDescribe.text($0) })")
        if out.count == 4, case let .motion(m) = out[3] {
            check(m.dx == 7 && !m.absValid && m.absX == 0 && m.modeChangeCount == 0, "stale absolute part cleared, delta kept: \(m)")
        }
        check(q.staleMotionsDropped == 2, "two empty MOTIONs dropped: \(q.staleMotionsDropped)")
        // A current count is passed through; without a display the absolute part is always cleared.
        var q2 = InputQueue()
        _ = q2.focus(gained: true)
        _ = q2.motion(absoluteMotion(mode: 5))
        _ = q2.pop()
        check(q2.popForConsumer(currentMode: current) == .motion(absoluteMotion(mode: 5)), "current mode count is kept")
        _ = q2.motion(absoluteMotion(mode: 5))
        check(q2.popForConsumer(currentMode: nil) == nil, "no display: empty MOTION is dropped")
        // A fresh MOTION merged onto a stale tail keeps the fresh position (7.3): the stale one never wins.
        var q3 = InputQueue()
        _ = q3.focus(gained: true)
        _ = q3.motion(absoluteMotion(mode: 4, x: 1, y: 1))
        _ = q3.motion(absoluteMotion(mode: 5, x: 2, y: 2))
        _ = q3.pop()
        check(q3.popForConsumer(currentMode: current) == .motion(absoluteMotion(mode: 5, x: 2, y: 2)), "fresh position survives the merge")
    }

    static func motionReconcile() {
        let mode = InputDisplayMode(modeChangeCount: 3, width: 64, height: 48)
        check((try? motion(dx: 1).reconciled(with: mode)) == .keep(motion(dx: 1)), "relative motion is untouched")
        check((try? absoluteMotion(mode: 3).reconciled(with: mode)) == .keep(absoluteMotion(mode: 3)), "current absolute part kept")
        check((try? absoluteMotion(mode: 2).reconciled(with: mode)) == .dropEmpty, "stale and empty is dropped")
        var stale = absoluteMotion(mode: 2)
        stale.dx = 4
        var cleared = motion(dx: 4)
        cleared.absValid = false
        check((try? stale.reconciled(with: mode)) == .keep(cleared), "stale with a delta keeps only the delta")
        check((try? absoluteMotion(mode: 3).reconciled(with: nil)) == .dropEmpty, "no display resets the absolute part")
        do {
            _ = try absoluteMotion(mode: 3, width: 128, height: 48).reconciled(with: mode)
            check(false, "same count with another size must be an error")
        } catch {
            check(true, "same count with another size is a protocol error")
        }
    }

    // MARK: kVK table (section 6.2)

    static func sdkEventsHeader() -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
        process.arguments = ["--show-sdk-path"]
        let pipe = Pipe()
        process.standardOutput = pipe
        guard (try? process.run()) != nil else { return nil }
        process.waitUntilExit()
        let sdk = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        let path = sdk + "/System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/HIToolbox.framework/Versions/A/Headers/Events.h"
        return try? String(contentsOfFile: path, encoding: .utf8)
    }

    static func sdkKeyCodes() -> [(name: String, code: UInt16)] {
        guard let text = sdkEventsHeader() else { return [] }
        var result: [(String, UInt16)] = []
        for line in text.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("kVK_"), let equals = trimmed.firstIndex(of: "=") else { continue }
            let name = String(trimmed[trimmed.startIndex..<equals]).trimmingCharacters(in: .whitespaces)
            var value = trimmed[trimmed.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            if let comma = value.firstIndex(of: ",") { value = String(value[value.startIndex..<comma]) }
            if let comment = value.range(of: "/*") { value = String(value[value.startIndex..<comment.lowerBound]) }
            value = value.trimmingCharacters(in: .whitespaces)
            guard value.hasPrefix("0x"), let code = UInt16(value.dropFirst(2), radix: 16) else { continue }
            result.append((name, code))
        }
        return result
    }

    static func hidTableCoverage() {
        let codes = sdkKeyCodes()
        check(codes.count >= 100, "kVK_* constants read from the SDK Events.h: \(codes.count)")
        var seenUsages: [UInt16: UInt16] = [:]
        for (name, code) in codes {
            let mapped = HIDUsage.keyboard[code]
            let skipped = HIDUsage.untransmittable[code]
            check((mapped != nil) != (skipped != nil), "\(name) (0x\(String(code, radix: 16))) must have a row or a reason, not both")
            if let usage = mapped {
                check(InputUsage.isValid(page: InputUsagePage.keyboard, id: usage), "\(name) -> 0x\(String(usage, radix: 16)) is outside section 6.1")
                if let other = seenUsages[usage] { check(false, "usage 0x\(String(usage, radix: 16)) used by 0x\(String(other, radix: 16)) and \(name)") }
                seenUsages[usage] = code
            }
        }
        // Rows with no SDK constant would be dead entries.
        let known = Set(codes.map { $0.code })
        for code in HIDUsage.keyboard.keys { check(known.contains(code), "table row 0x\(String(code, radix: 16)) is not a kVK_* of the SDK") }
        for code in HIDUsage.untransmittable.keys { check(known.contains(code), "reason row 0x\(String(code, radix: 16)) is not a kVK_* of the SDK") }
        for (code, usage) in HIDUsage.keyboard {
            check(InputUsage.isValid(page: InputUsagePage.keyboard, id: usage), "row 0x\(String(code, radix: 16)) value 0x\(String(usage, radix: 16)) invalid")
        }
        print("kVK table: \(codes.count) SDK constants, \(HIDUsage.keyboard.count) transmitted, \(HIDUsage.untransmittable.count) listed as not transmitted")
    }

    static func hidTableSemantics() {
        // Examples of section 6.2.
        let examples: [(UInt16, UInt16)] = [
            (0x00, 0x04), (0x24, 0x28), (0x35, 0x29), (0x0A, 0x64), (0x5D, 0x89), (0x68, 0x90),
            (0x3B, 0xE0), (0x38, 0xE1), (0x3A, 0xE2), (0x37, 0xE3), (0x3E, 0xE4), (0x3C, 0xE5), (0x3D, 0xE6), (0x36, 0xE7),
        ]
        for (code, usage) in examples { check(HIDUsage.usage(forKeyCode: code) == usage, "section 6.2 example 0x\(String(code, radix: 16))") }
        check(HIDUsage.usage(forKeyCode: 0x3F) == nil, "Fn has no usage")
        // Names from the SDK against HUT positions: letters 0x04..0x1D, digits 0x1E..0x27, F1..F12 0x3A.., F13..F20 0x68.., keypad.
        for (name, code) in sdkKeyCodes() {
            guard let usage = HIDUsage.keyboard[code] else { continue }
            let tail = String(name.dropFirst(4))
            if tail.hasPrefix("ANSI_"), tail.count == 6, let letter = tail.last?.asciiValue, letter >= 65, letter <= 90 {
                check(usage == 0x04 + UInt16(letter - 65), "\(name) letter usage 0x\(String(usage, radix: 16))")
            } else if tail.hasPrefix("ANSI_Keypad"), tail.count == 12, let digit = tail.last?.wholeNumberValue {
                check(usage == (digit == 0 ? 0x62 : 0x58 + UInt16(digit)), "\(name) keypad digit")
            } else if tail.hasPrefix("ANSI_"), tail.count == 6, let digit = tail.last?.wholeNumberValue {
                check(usage == (digit == 0 ? 0x27 : 0x1D + UInt16(digit)), "\(name) digit")
            } else if tail.hasPrefix("F"), let number = Int(tail.dropFirst()) {
                check(usage == (number <= 12 ? 0x3A + UInt16(number - 1) : 0x68 + UInt16(number - 13)), "\(name) function key")
            }
        }
        check(HIDUsage.deviceModifierMask(forKeyCode: 0x3B) == 0x1 && HIDUsage.deviceModifierMask(forKeyCode: 0x3E) == 0x2000, "modifier masks")
        check(HIDUsage.deviceModifierMask(forKeyCode: 0x00) == nil, "no mask for a letter")
        check(HIDUsage.buttonUsage(forButtonNumber: 0) == 1 && HIDUsage.buttonUsage(forButtonNumber: 4) == 5, "buttons 0...4 -> usages 1...5")
        check(HIDUsage.buttonUsage(forButtonNumber: 5) == nil && HIDUsage.buttonUsage(forButtonNumber: -1) == nil, "buttons >= 5 are not transmitted")
    }

    // MARK: Coordinates (section 5.3, 12 items 1 and 2)

    static func geometry(_ bw: Double, _ bh: Double, scale: Double, _ mode: PresentMode, frame: (Int, Int)?, modeCount: UInt64 = 7) -> InputGeometry {
        InputGeometry(
            boundsWidth: bw, boundsHeight: bh, scale: scale, mode: mode,
            shown: frame.map { ShownFrame(width: $0.0, height: $0.1, modeChangeCount: modeCount) })
    }

    /// AppKit point (origin bottom-left) of the centre of a physical pixel (origin top-left).
    static func point(ofPhysical x: Int, _ y: Int, in g: InputGeometry) -> (Double, Double) {
        (
            (Double(x) + 0.5) * g.boundsWidth / Double(g.drawableWidth),
            g.boundsHeight - (Double(y) + 0.5) * g.boundsHeight / Double(g.drawableHeight)
        )
    }

    static func guestAt(_ g: InputGeometry, physical x: Int, _ y: Int) -> (Int, Int)? {
        let p = point(ofPhysical: x, y, in: g)
        return g.absolute(pointX: p.0, pointY: p.1).map { (Int($0.x), Int($0.y)) }
    }

    static func coordinateExamples() {
        // The four examples of section 5.3.
        var g = geometry(100, 100, scale: 2, .fit, frame: (64, 48))
        check(g.viewport == Viewport(x: 0, y: 25, width: 200, height: 150), "example 1 viewport \(String(describing: g.viewport))")
        check(guestAt(g, physical: 0, 25).map { $0 == (0, 0) } ?? false, "example 1 top-left")
        check(guestAt(g, physical: 199, 174).map { $0 == (63, 47) } ?? false, "example 1 bottom-right \(String(describing: guestAt(g, physical: 199, 174)))")
        g = geometry(640, 480, scale: 1, .fit, frame: (1920, 1080))
        check(g.viewport == Viewport(x: 0, y: 60, width: 640, height: 360), "example 2 viewport")
        check(guestAt(g, physical: 0, 60).map { $0 == (1, 1) } ?? false, "example 2 top-left \(String(describing: guestAt(g, physical: 0, 60)))")
        check(guestAt(g, physical: 639, 419).map { $0 == (1918, 1078) } ?? false, "example 2 bottom-right")
        g = geometry(640, 360, scale: 1, .fit, frame: (4096, 2304))
        check(g.viewport == Viewport(x: 0, y: 0, width: 640, height: 360), "example 3 viewport")
        check(guestAt(g, physical: 0, 0).map { $0 == (3, 3) } ?? false, "example 3 top-left \(String(describing: guestAt(g, physical: 0, 0)))")
        check(guestAt(g, physical: 639, 359).map { $0 == (4092, 2300) } ?? false, "example 3 bottom-right")
        g = geometry(10, 10, scale: 1, .pixelExact, frame: (64, 48))
        check(g.viewport == Viewport(x: -27, y: -19, width: 64, height: 48), "example 4 viewport")
        check(guestAt(g, physical: 0, 0).map { $0 == (27, 19) } ?? false, "example 4 (0,0)")
        check(guestAt(g, physical: 9, 9).map { $0 == (36, 28) } ?? false, "example 4 (9,9)")
        // ABS_VALID needs a shown frame; carries its mode count and size.
        let none = geometry(100, 100, scale: 2, .fit, frame: nil)
        check(none.absolute(pointX: 50, pointY: 50) == nil, "no shown frame: no absolute position")
        // A drag that left the window gives no absolute position, also in a cropping pixel-exact window (viewport -27,-19,64,48).
        let cropped = geometry(10, 10, scale: 1, .pixelExact, frame: (64, 48))
        check(cropped.absolute(pointX: 5, pointY: 5) != nil, "inside a cropped pixel-exact window")
        for outside in [(-0.5, 5.0), (10.5, 5.0), (5.0, -0.5), (5.0, 10.5), (-30.0, 5.0), (40.0, 5.0)] {
            check(cropped.absolute(pointX: outside.0, pointY: outside.1) == nil, "point \(outside) outside the bounds must not give ABS_VALID")
        }
        check(cropped.absolute(pointX: 10, pointY: 0) != nil, "the far edge of the bounds is still inside")
        let shown = geometry(100, 100, scale: 2, .fit, frame: (64, 48), modeCount: 42)
        if let a = shown.absolute(pointX: 50, pointY: 50) {
            check(a.modeChangeCount == 42 && a.width == 64 && a.height == 48 && a.x == 32 && a.y == 24, "absolute fields \(a)")
        } else { check(false, "absolute position in the middle") }
        check(shown.absolute(pointX: 50, pointY: 1) == nil, "a point in the letterbox margin has no guest pixel")
        check(shown.absolute(pointX: -1, pointY: 50) == nil && shown.absolute(pointX: 101, pointY: 50) == nil, "outside the bounds")
    }

    static let windowSet: [(bw: Double, bh: Double, scale: Double, mode: PresentMode)] = [
        (640, 480, 1, .fit), (640, 480, 2, .fit), (100, 100, 2, .fit), (100, 100, 2, .pixelExact), (333, 217, 2, .fit),
        (333.5, 217.25, 2, .fit), (1280, 720, 1.5, .fit), (1000, 1000, 2.5, .fit), (800, 600, 2, .pixelExact),
        (10, 10, 1, .pixelExact), (64, 48, 1, .pixelExact), (640, 360, 1, .fit), (2560, 1440, 2, .fit),
        (50, 200, 2, .fit), (200, 50, 1, .fit), (1, 1, 1, .fit), (4096, 2304, 1, .pixelExact), (3000, 2000, 1, .pixelExact),
    ]
    static let frames: [(Int, Int)] = [(64, 48), (1920, 1080), (4096, 2304)]

    /// Exact rational nearest-sample reference: floor((2(c - origin) + 1) * W / (2 * size)).
    static func reference(_ c: Int, origin: Int, size: Int, frame: Int) -> Int { ((2 * (c - origin) + 1) * frame) / (2 * size) }

    static func coordinateCornersAndPixels() {
        var offByOne = 0
        var cornerCases = 0
        for window in windowSet {
            for frame in frames {
                let g = geometry(window.bw, window.bh, scale: window.scale, window.mode, frame: frame)
                guard let viewport = g.viewport else { continue }
                let visible = viewport.clipped(targetWidth: g.drawableWidth, targetHeight: g.drawableHeight)
                if visible.isEmpty { continue }
                let label = "\(window.bw)x\(window.bh)@\(window.scale) \(window.mode) frame \(frame.0)x\(frame.1) viewport \(viewport)"
                let kx = Double(frame.0) / Double(viewport.width)
                let ky = Double(frame.1) / Double(viewport.height)
                let corners = [(visible.x, visible.y), (visible.x + visible.width - 1, visible.y + visible.height - 1)]
                for (index, corner) in corners.enumerated() {
                    guard let got = guestAt(g, physical: corner.0, corner.1) else {
                        check(false, "\(label): corner \(corner) has no guest pixel")
                        continue
                    }
                    let expected = (
                        reference(corner.0, origin: viewport.x, size: viewport.width, frame: frame.0),
                        reference(corner.1, origin: viewport.y, size: viewport.height, frame: frame.1))
                    check(abs(got.0 - expected.0) <= 1 && abs(got.1 - expected.1) <= 1, "\(label): corner \(corner) got \(got) reference \(expected)")
                    if abs(got.0 - expected.0) + abs(got.1 - expected.1) > 0 { offByOne += 1 }
                    cornerCases += 1
                    // The plan criterion "corner +-1 on any window size" (section 11 item 8) holds for k < 4 when the whole
                    // viewport is visible; for k >= 4 the drawn texel (the reference) is the expectation.
                    if visible == viewport {
                        let guestCorner = index == 0 ? (0, 0) : (frame.0 - 1, frame.1 - 1)
                        if kx < 4 && ky < 4 {
                            check(abs(got.0 - guestCorner.0) <= 1 && abs(got.1 - guestCorner.1) <= 1, "\(label): corner \(index) \(got) vs guest corner \(guestCorner)")
                        } else {
                            check(got == expected || abs(got.0 - expected.0) <= 1, "\(label): k >= 4 corner follows the drawn texel")
                        }
                    }
                }
                // Every physical pixel of the 64x48 frame at k <= 1 gives the pixel that nearest sampling draws (+-1 only at an
                // exact texel boundary, where the GPU may choose either, DISPLAY.md section 3).
                if frame.0 == 64, kx <= 1, ky <= 1 {
                    var exact = 0
                    var boundary = 0
                    for py in visible.y..<(visible.y + visible.height) {
                        for px in visible.x..<(visible.x + visible.width) {
                            guard let got = guestAt(g, physical: px, py) else {
                                check(false, "\(label): pixel \(px),\(py) has no guest pixel")
                                continue
                            }
                            let expected = (
                                reference(px, origin: viewport.x, size: viewport.width, frame: 64),
                                reference(py, origin: viewport.y, size: viewport.height, frame: 48))
                            if got == expected { exact += 1 } else {
                                boundary += 1
                                check(abs(got.0 - expected.0) <= 1 && abs(got.1 - expected.1) <= 1, "\(label): pixel \(px),\(py) got \(got) nearest \(expected)")
                            }
                        }
                    }
                    check(boundary * 100 <= (exact + boundary) , "\(label): too many boundary differences \(boundary) of \(exact + boundary)")
                }
            }
        }
        print("coordinate corners: \(cornerCases) corners checked, \(offByOne) differ from the exact rational reference by 1")
    }

    static func coordinateAppKitPoint() {
        // Origin at the bottom-left, Retina @2, a fractional bounds size gives a non-integer D/B ratio.
        let g = geometry(100.4, 50.2, scale: 2, .fit, frame: nil)
        check(g.drawableWidth == 201 && g.drawableHeight == 100, "drawable \(g.drawableWidth)x\(g.drawableHeight)")
        check(g.physicalPixel(pointX: 0, pointY: 50.2) == (0, 0), "top-left of the bounds is physical (0,0)")
        check(g.physicalPixel(pointX: 100.4, pointY: 0) == (200, 99), "bottom-right of the bounds is the last drawable pixel")
        check(g.physicalPixel(pointX: 100.4 - 1e-9, pointY: 1e-9) == (200, 99), "just inside the bottom-right edge")
        check(g.physicalPixel(pointX: 100.4 / 2, pointY: 50.2 / 2) == (100, 50), "centre of the bounds \(g.physicalPixel(pointX: 100.4 / 2, pointY: 50.2 / 2))")
        // Y is flipped: a point near the top of the view is a small physical y.
        check(g.physicalPixel(pointX: 10, pointY: 50.2 - 0.25).y == 0, "near the top is y = 0")
        check(g.physicalPixel(pointX: 10, pointY: 0.25).y == 99, "near the bottom is the last y")
        check(g.physicalPixel(pointX: -0.5, pointY: 10).x < 0 && g.physicalPixel(pointX: 101, pointY: 10).x >= g.drawableWidth, "outside the bounds stays outside")
        // Every drawable pixel centre maps back into its own pixel.
        let r = geometry(333.5, 217.25, scale: 2, .fit, frame: nil)
        var bad = 0
        for py in 0..<r.drawableHeight {
            for px in 0..<r.drawableWidth {
                let p = point(ofPhysical: px, py, in: r)
                if r.physicalPixel(pointX: p.0, pointY: p.1) != (px, py) { bad += 1 }
            }
        }
        check(bad == 0, "\(bad) pixel centres do not round-trip")
        // Retina @2, integer sizes: a click at logical (100, 50) is physical (200, ...).
        let e = geometry(640, 480, scale: 2, .fit, frame: nil)
        check(e.physicalPixel(pointX: 100, pointY: 480 - 50) == (200, 100), "Retina @2 point to pixel")
    }

    static func accumulators() {
        var a = FractionalAccumulator()
        check(a.add(0.4) == 0 && a.add(0.4) == 0 && a.add(0.4) == 1, "fractions add up")
        check(abs(a.remainder - 0.2) < 1e-9, "remainder kept")
        var b = FractionalAccumulator()
        check(b.add(-0.6) == 0 && b.add(-0.6) == -1, "truncation toward zero for negative values")
        check(b.add(.nan) == 0 && b.add(1e30) == Int32.max, "non-finite and huge values are bounded")
        var p = InputPointerState()
        let g1 = geometry(640, 480, scale: 2, .fit, frame: (64, 48)) // k = 64 / 1280... viewport fills 1280x960: k = 0.05
        let d1 = p.relative(dxPoints: 10, dyPoints: -10, geometry: g1)
        check(d1.dx == 1 && d1.dy == -1, "10 points = 20 physical pixels = 1 guest pixel at k 0.05: \(d1)")
        let g2 = geometry(640, 480, scale: 2, .fit, frame: nil) // k = 1
        _ = p.relative(dxPoints: 0.25, dyPoints: 0, geometry: g2) // remainder reset by the change of k
        let d2 = p.relative(dxPoints: 0.2, dyPoints: 0, geometry: g2)
        check(d2.dx == 0, "0.25*2 + 0.2*2 physical pixels is < 1 only after a reset: \(d2)")
        p.reset()
        // Wheel: notch wheel x120; precise scroll x units per point; natural scrolling turned back; pan sign.
        check(p.scroll(deltaX: 0, deltaY: 1, precise: false, invertedFromDevice: false) == (120, 0), "one notch")
        check(p.scroll(deltaX: 0, deltaY: 1, precise: false, invertedFromDevice: true) == (-120, 0), "inverted direction is turned back")
        let precise = p.scroll(deltaX: 0, deltaY: 16, precise: true, invertedFromDevice: false)
        check(precise.wheel == 120, "16 precise points = one notch at 7.5 units per point: \(precise)")
        check(p.scroll(deltaX: -1, deltaY: 0, precise: false, invertedFromDevice: false).pan == 120, "pan sign")
    }

    /// Section 5.4: wheel and pan remainders are reset on FOCUS GAINED as well as on LOST.
    static func focusResetsRemainders() {
        let (uiFD, hostFD) = socketPair()
        guard let channel = try? InputChannelUI(fd: uiFD) else { check(false, "channel"); return }
        channel.start(attachTimer: false)
        write(hostFD, .accept(flags: 0))
        channel.pump()
        channel.windowFocus(gained: true)
        // 0.4 of a wheel unit left over (precise scroll of 1 point = 7.5 units: 0.5 remains).
        check(channel.scrollUnits(deltaX: 0, deltaY: 1, precise: true, invertedFromDevice: false).wheel == 7, "7.5 units -> 7")
        channel.windowFocus(gained: false)
        channel.windowFocus(gained: true)
        check(channel.scrollUnits(deltaX: 0, deltaY: 1, precise: true, invertedFromDevice: false).wheel == 7, "remainder reset by the focus cycle")
        channel.windowFocus(gained: false)
        check(channel.scrollUnits(deltaX: 0, deltaY: 1, precise: true, invertedFromDevice: false).wheel == 7, "remainder reset by LOST")
        channel.shutdown()
        Darwin.close(hostFD)
    }

    // MARK: UI channel against a hand-written VMHost end (section 12 items 5 and 6)

    static func socketPair() -> (ui: Int32, host: Int32) {
        var fds: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        _ = InputSocket.makeNonBlocking(fds[1])
        return (fds[0], fds[1])
    }

    static func drain(_ fd: Int32) -> [UInt8] {
        var out: [UInt8] = []
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = chunk.withUnsafeMutableBytes { Darwin.recv(fd, $0.baseAddress, $0.count, 0) }
            if n <= 0 { break }
            out += chunk[0..<n]
        }
        return out
    }

    static func write(_ fd: Int32, _ message: InputMessage) { _ = InputSocket.send(fd: fd, message.encoded()) }

    /// Applies decoded UI messages to a VMHost-side queue by the same rules (the VMHost channel does this per message).
    static func apply(_ messages: [InputMessage], to queue: inout InputQueue) -> [String] {
        var errors: [String] = []
        for message in messages {
            var outcome = InputQueue.Outcome.queued
            switch message {
            case .hello: break
            case let .focus(gained): outcome = queue.focus(gained: gained)
            case let .usage(u): outcome = queue.usage(page: u.page, id: u.id, down: u.down)
            case let .motion(m): outcome = queue.motion(m)
            default: errors.append("unexpected \(message.kind)")
            }
            if case let .violation(reason) = outcome { errors.append(reason) }
        }
        return errors
    }

    static func channelHandshakeAndFocusRelease() {
        let (uiFD, hostFD) = socketPair()
        guard let channel = try? InputChannelUI(fd: uiFD) else { check(false, "channel"); return }
        channel.windowFocus(gained: true) // the window is key before ACCEPT arrives
        channel.start(attachTimer: false)
        check(channel.state == .handshake, "HANDSHAKE after HELLO")
        // Events before ACCEPT are dropped and counted, nothing but HELLO is on the wire.
        channel.usage(page: 7, id: 4, down: true)
        check(channel.counters.droppedNotReady == 1, "event in HANDSHAKE is counted")
        var decoder = InputDecoder(direction: .uiToHost)
        let first = (try? decoder.append(drain(hostFD))) ?? []
        check(first == [.hello], "HELLO only: \(first)")
        write(hostFD, .accept(flags: 0))
        channel.pump()
        check(channel.state == .ready && channel.guestDelivery == false, "READY with GUEST_DELIVERY = 0")
        check(channel.statusText.contains("not delivered to the guest"), "the UI says that input is not delivered: \(channel.statusText)")
        check(channel.isFocused, "FOCUS GAINED sent because the window was already key")
        // Press a key and a mouse button (with its position), the focus goes away, the release comes in another window.
        channel.usage(page: 7, id: 0x04, down: true)
        channel.usage(page: 7, id: 0x04, down: true) // duplicate: not sent
        channel.usage(page: 7, id: 0xE3, down: true)
        channel.button(number: 0, down: true, absolute: InputAbsolute(x: 3, y: 4, width: 64, height: 48, modeChangeCount: 1))
        channel.windowFocus(gained: false)
        channel.usage(page: 7, id: 0x04, down: false) // UP after LOST: unfocused, dropped
        channel.button(number: 0, down: false, absolute: nil)
        check(channel.counters.droppedUnfocused == 2 && channel.counters.ignoredDuplicates == 1, "counters \(channel.counters)")
        check(channel.queue.pressedCount == 0, "the UI cleared its pressed set")
        let wire = (try? decoder.append(drain(hostFD))) ?? []
        let ups = wire.filter { if case let .usage(u) = $0 { return !u.down } else { return false } }
        check(ups.isEmpty, "no UP on the wire after FOCUS LOST: \(ups)")
        check(wire.last == .focus(gained: false), "FOCUS LOST is the last message: \(String(describing: wire.last))")
        // The VMHost-side queue sees the same and holds nothing pressed.
        var hostQueue = InputQueue()
        let errors = apply([.hello] + (first.isEmpty ? [] : []) + wire, to: &hostQueue)
        check(errors.isEmpty, "VMHost queue errors: \(errors)")
        check(hostQueue.pressedCount == 0 && hostQueue.state == .unfocused, "VMHost: nothing pressed, UNFOCUSED")
        check(hostQueue.records.last == .focus(gained: false), "FOCUS LOST in the VMHost queue")
        let hostUps = hostQueue.records.filter { if case let .usage(u) = $0 { return !u.down } else { return false } }
        check(hostUps.isEmpty, "no extra UP in the VMHost queue")
        // The click position came first, with zero deltas, before the button DOWN.
        let kinds = wire.map { $0.kind }
        if let down = wire.firstIndex(where: { if case let .usage(u) = $0 { return u.page == 9 } else { return false } }), down > 0,
           case let .motion(m) = wire[down - 1] {
            check(m.absValid && !m.hasRelativePart && m.absX == 3 && m.absY == 4 && m.modeChangeCount == 1, "MOTION before the button: \(m)")
        } else {
            check(false, "a MOTION with the click position precedes the button DOWN: \(kinds)")
        }
        // STATUS is shown; a second ACCEPT is an error.
        write(hostFD, .status(InputStatus(rejectedDowns: 3, mergedMotions: 1, droppedUnfocused: 0, queueDepth: 2)))
        channel.pump()
        check(channel.hostStatus?.rejectedDowns == 3, "STATUS received")
        check(channel.statusText.contains("3 key presses rejected"), "rejections are shown: \(channel.statusText)")
        write(hostFD, .accept(flags: 0))
        channel.pump()
        if case .closed = channel.state { check(true, "second ACCEPT closes the channel") } else { check(false, "second ACCEPT: \(channel.state)") }
        check(drain(hostFD).isEmpty, "nothing is written after the close")
        var byte: UInt8 = 0
        check(Darwin.recv(hostFD, &byte, 1, 0) == 0, "the UI closed its end: VMHost reads EOF")
        Darwin.close(hostFD)
    }

    static func channelFailures() {
        // Foreign version -> INCOMPATIBLE.
        do {
            let (uiFD, hostFD) = socketPair()
            let channel = try! InputChannelUI(fd: uiFD)
            channel.start(attachTimer: false)
            var header = InputMessage.accept(flags: 0).encoded()
            header[4] = 2
            _ = InputSocket.send(fd: hostFD, header)
            channel.pump()
            check(channel.state == .incompatible(2) && !channel.isCollecting, "ODIT version 2 -> INCOMPATIBLE: \(channel.state)")
            check(channel.statusText.contains("incompatible"), "INCOMPATIBLE is shown")
            Darwin.close(hostFD)
        }
        // Garbage, EOF, a message of the wrong direction, a descriptor in a message.
        let garbage: [(String, [UInt8])] = [
            ("foreign magic", Array("ODFT".utf8) + [UInt8](repeating: 0, count: 12)),
            ("unknown kind", { var b = InputMessage.accept(flags: 0).encoded(); b[6] = 9; return b }()),
            ("HELLO from VMHost", InputMessage.hello.encoded()),
            ("STATUS before ACCEPT", InputMessage.status(InputStatus()).encoded()),
        ]
        for (label, bytes) in garbage {
            let (uiFD, hostFD) = socketPair()
            let channel = try! InputChannelUI(fd: uiFD)
            channel.start(attachTimer: false)
            _ = InputSocket.send(fd: hostFD, bytes)
            channel.pump()
            if case .closed = channel.state { check(true, label) } else { check(false, "\(label): state \(channel.state)") }
            Darwin.close(hostFD)
        }
        do {
            let (uiFD, hostFD) = socketPair()
            let channel = try! InputChannelUI(fd: uiFD)
            channel.start(attachTimer: false)
            Darwin.close(hostFD)
            channel.pump()
            if case let .closed(reason) = channel.state { check(reason.contains("EOF"), "EOF reason \(reason)") } else { check(false, "EOF state \(channel.state)") }
        }
        do {
            // A descriptor in a message (SCM_RIGHTS) is a protocol error and the descriptor is closed.
            var fds: [Int32] = [0, 0]
            precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
            let channel = try! InputChannelUI(fd: fds[0])
            channel.start(attachTimer: false)
            let passed = open("/dev/null", O_RDONLY)
            let message = InputMessage.accept(flags: 0).encoded()
            var control = [UInt8](repeating: 0, count: 16)
            control.inAppendLE(UInt32(0)) // placeholder to size the array below
            control = [UInt8](repeating: 0, count: 16)
            control.withUnsafeMutableBytes { raw in
                raw.storeBytes(of: UInt32(16), toByteOffset: 0, as: UInt32.self)
                raw.storeBytes(of: Int32(SOL_SOCKET), toByteOffset: 4, as: Int32.self)
                raw.storeBytes(of: Int32(SCM_RIGHTS), toByteOffset: 8, as: Int32.self)
                raw.storeBytes(of: passed, toByteOffset: 12, as: Int32.self)
            }
            var data = message
            let sent: Int = data.withUnsafeMutableBytes { dataRaw in
                control.withUnsafeMutableBytes { controlRaw in
                    var iov = iovec(iov_base: dataRaw.baseAddress, iov_len: dataRaw.count)
                    return withUnsafeMutablePointer(to: &iov) { iovPointer in
                        var header = msghdr()
                        header.msg_iov = iovPointer
                        header.msg_iovlen = 1
                        header.msg_control = controlRaw.baseAddress
                        header.msg_controllen = socklen_t(controlRaw.count)
                        return Darwin.sendmsg(fds[1], &header, 0)
                    }
                }
            }
            Darwin.close(passed)
            check(sent == message.count, "sendmsg with a descriptor")
            channel.pump()
            if case let .closed(reason) = channel.state { check(reason.contains("descriptor"), "descriptor reason: \(reason)") } else { check(false, "descriptor state \(channel.state)") }
            Darwin.close(fds[1])
        }
    }
}

/// Text of a record for failure messages (the VMHost trace uses its own copy of this format).
enum InputHostChannelDescribe {
    static func text(_ record: InputRecord) -> String { "\(record)" }
}

/// XCTest wrapper (S11-05a): each former step of `main()` is a test case; the checks above are unchanged and still count failures.
final class InputXCTests: XCTestCase {
    override func setUp() {
        failures = 0
    }

    override class func tearDown() {
        print("input tests: \(checks) checks in all test cases")
    }

    private func finish(_ name: String) {
        XCTAssertEqual(failures, 0, "input \(name): \(failures) check(s) failed (see FAIL lines above)")
    }

    func testMessageLayout() {
        InputTests.messageLayout()
        finish("messageLayout")
    }

    func testHeaderChecks() {
        InputTests.headerChecks()
        finish("headerChecks")
    }

    func testPayloadChecks() {
        InputTests.payloadChecks()
        finish("payloadChecks")
    }

    func testDecoderStreaming() {
        InputTests.decoderStreaming()
        finish("decoderStreaming")
    }

    func testQueueRules() {
        InputTests.queueRules()
        finish("queueRules")
    }

    func testQueuePendingRules() {
        InputTests.queuePendingRules()
        finish("queuePendingRules")
    }

    func testQueueInvariantRandom() {
        InputTests.queueInvariantRandom()
        finish("queueInvariantRandom")
    }

    func testQueueNoConsumerGrowth() {
        InputTests.queueNoConsumerGrowth()
        finish("queueNoConsumerGrowth")
    }

    func testQueueExitRecheck() {
        InputTests.queueExitRecheck()
        finish("queueExitRecheck")
    }

    func testMotionReconcile() {
        InputTests.motionReconcile()
        finish("motionReconcile")
    }

    func testHidTableCoverage() {
        InputTests.hidTableCoverage()
        finish("hidTableCoverage")
    }

    func testHidTableSemantics() {
        InputTests.hidTableSemantics()
        finish("hidTableSemantics")
    }

    func testCoordinateExamples() {
        InputTests.coordinateExamples()
        finish("coordinateExamples")
    }

    func testCoordinateCornersAndPixels() {
        InputTests.coordinateCornersAndPixels()
        finish("coordinateCornersAndPixels")
    }

    func testCoordinateAppKitPoint() {
        InputTests.coordinateAppKitPoint()
        finish("coordinateAppKitPoint")
    }

    func testAccumulators() {
        InputTests.accumulators()
        finish("accumulators")
    }

    func testFocusResetsRemainders() {
        InputTests.focusResetsRemainders()
        finish("focusResetsRemainders")
    }

    func testChannelHandshakeAndFocusRelease() {
        InputTests.channelHandshakeAndFocusRelease()
        finish("channelHandshakeAndFocusRelease")
    }

    func testChannelFailures() {
        InputTests.channelFailures()
        finish("channelFailures")
    }
}
