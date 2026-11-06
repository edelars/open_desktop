import Darwin
import Foundation
import XCTest
@testable import ODShared

// Unit tests of Shared/ControlV2.swift (S11-04): widths, offsets, limits and the fatal / value split of
// protocols/control/CONTROL_PROTOCOL_V2.md. No VMHost process; run with `swift test --filter ControlV2XCTests` from apps/.

final class ControlV2XCTests: XCTestCase {
    private func hex(_ bytes: [UInt8]) -> String { bytes.map { String(format: "%02x", $0) }.joined(separator: " ") }

    private func decodeAll(_ bytes: [UInt8], direction: ControlV2Direction = .towardHost) throws -> [ControlV2Frame] {
        var decoder = ControlV2Decoder(direction: direction)
        decoder.append(bytes)
        var frames: [ControlV2Frame] = []
        while let received = try decoder.nextFrame() { frames.append(received.frame) }
        return frames
    }

    /// Section 10: the example header bytes.
    func testHeaderBytesOfTheSpecificationExample() {
        XCTAssertEqual(hex(ControlV2Frame(kind: .hello, requestID: 0).encoded()),
                       "4f 44 43 50 02 00 01 00 00 00 00 00 00 00 00 00")
        XCTAssertEqual(hex(ControlV2Frame(response: .ready(profile: 2), requestID: 0).encoded()),
                       "4f 44 43 50 02 00 02 00 00 00 00 00 08 00 00 00 02 00 00 00 00 00 00 00")
        XCTAssertEqual(hex(ControlV2Frame(kind: .query, requestID: 5).encoded()),
                       "4f 44 43 50 02 00 0b 00 05 00 00 00 00 00 00 00")
    }

    func testConfigureRoundTripAndLength() throws {
        let configure = ControlV2Configure(
            machineProfile: 4, memoryMiB: 8192, vcpuCount: 1, displayScreens: 2, network: 1, audio: 0, clipboardSync: 1,
            bootMedia: 1,
            disks: [.init(source: 1, format: 1, writable: 1, name: Array("disk0.img".utf8)),
                    .init(source: 2, format: 1, writable: 0, name: [])],
            sharedFolders: [.init(readOnly: 1, guestName: Array("share".utf8))])
        let bytes = configure.encoded()
        XCTAssertEqual(bytes.count, 1588)
        XCTAssertEqual(try ControlV2Configure.parse(bytes), configure)
        XCTAssertNil(configure.validate())
        XCTAssertEqual(bytes[16], 2) // disk_count
        XCTAssertEqual(bytes[17], 1) // shared_folder_count
        XCTAssertEqual(bytes[20 + 3], 9) // name_len of slot 0
        XCTAssertEqual(bytes[1044 + 1], 5) // guest_name_len of folder slot 0
    }

    func testConfigureStructureViolationsAreFatal() throws {
        let good = ControlV2Configure(disks: [.init(name: Array("d".utf8))]).encoded()
        func mutated(_ change: (inout [UInt8]) -> Void) -> [UInt8] {
            var copy = good
            change(&copy)
            return copy
        }
        let broken: [[UInt8]] = [
            Array(good.dropLast()),
            mutated { $0[18] = 1 }, // reserved
            mutated { $0[16] = 5 }, // disk_count above 4
            mutated { $0[17] = 9 }, // shared_folder_count above 8
            mutated { $0[20 + 256 * 2] = 1 }, // unused disk slot is not zero
            mutated { $0[20 + 3] = 250 }, // name_len above 249
            mutated { $0[20 + 4 + 1] = 0x41 }, // the name tail is not zero
            mutated { $0[1044] = 1 }, // unused shared folder slot is not zero
        ]
        for payload in broken {
            XCTAssertThrowsError(try ControlV2Configure.parse(payload))
        }
    }

    func testConfigureValueViolationsAreErrorsWithOffsets() throws {
        func code(_ configure: ControlV2Configure) -> (ControlV2ErrorCode, UInt64)? {
            configure.validate().map { ($0.code, $0.aux0) }
        }
        XCTAssertNil(ControlV2Configure().validate())
        XCTAssertNil(ControlV2Configure(machineProfile: 4, displayScreens: 2).validate())
        XCTAssertTrue(code(ControlV2Configure(machineProfile: 2))! == (.invalidConfig, 0))
        XCTAssertTrue(code(ControlV2Configure(memoryMiB: 255))! == (.invalidConfig, 4))
        XCTAssertTrue(code(ControlV2Configure(memoryMiB: 65_537))! == (.invalidConfig, 4))
        XCTAssertNil(code(ControlV2Configure(memoryMiB: 65_536)))
        XCTAssertTrue(code(ControlV2Configure(vcpuCount: 2))! == (.invalidConfig, 8))
        XCTAssertTrue(code(ControlV2Configure(machineProfile: 1, displayScreens: 2))! == (.invalidConfig, 10))
        XCTAssertTrue(code(ControlV2Configure(machineProfile: 4, displayScreens: 1))! == (.invalidConfig, 10))
        XCTAssertTrue(code(ControlV2Configure(network: 2))! == (.invalidConfig, 12))
        XCTAssertTrue(code(ControlV2Configure(bootMedia: 2))! == (.invalidConfig, 15))
        // Disk slots: names are `invalid_name` at the offset of the name field, the rest `invalid_config`.
        for name in ["a/b", ".", "..", "", "a\u{1}b", "a\u{7f}"] {
            XCTAssertTrue(code(ControlV2Configure(disks: [.init(name: Array(name.utf8))]))! == (.invalidName, 24), name)
        }
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(name: [0xff, 0xfe])]))! == (.invalidName, 24))
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(source: 3, name: Array("d".utf8))]))! == (.invalidConfig, 20))
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(format: 2, name: Array("d".utf8))]))! == (.invalidConfig, 21))
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(writable: 2, name: Array("d".utf8))]))! == (.invalidConfig, 22))
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(source: 2, name: Array("d".utf8))]))! == (.invalidConfig, 23))
        XCTAssertTrue(code(ControlV2Configure(disks: [.init(name: Array("d".utf8)), .init(name: Array("d".utf8))]))!
            == (.invalidConfig, 20 + 256 + 4))
        // Shared folders: guest_name rules.
        for name in ["", "a/b", "a\\b", "a:b", "a\u{0}b"] {
            XCTAssertTrue(code(ControlV2Configure(sharedFolders: [.init(guestName: Array(name.utf8))]))! == (.invalidConfig, 1048), name)
        }
        XCTAssertTrue(code(ControlV2Configure(sharedFolders: [.init(guestName: [0xc0, 0x80])]))! == (.invalidConfig, 1048))
        // The error of the field with the smaller offset wins.
        XCTAssertTrue(code(ControlV2Configure(memoryMiB: 1, disks: [.init(name: Array("a/b".utf8))]))! == (.invalidConfig, 4))
    }

    func testDecoderHandlesPartialReadsByteByByte() throws {
        let frames = [ControlV2Frame(kind: .hello, requestID: 0), ControlV2Frame(request: .pause(reason: 1), requestID: 1),
                      ControlV2Frame(request: .setClipboard(Array(repeating: 7, count: 65_520)), requestID: 2)]
        var decoder = ControlV2Decoder()
        var seen: [ControlV2Frame] = []
        for byte in frames.flatMap({ $0.encoded() }) {
            decoder.append([byte])
            while let received = try decoder.nextFrame() { seen.append(received.frame) }
        }
        XCTAssertEqual(seen, frames)
        XCTAssertEqual(decoder.bufferedPartialBytes, 0)
        XCTAssertNoThrow(try decoder.endOfStream())
    }

    func testDecoderRejectsBrokenHeaders() {
        func header(magic: [UInt8] = ControlV2.magic, version: UInt16 = 2, kind: UInt16, length: UInt32) -> [UInt8] {
            var bytes = magic
            bytes += [UInt8(version & 0xff), UInt8(version >> 8), UInt8(kind & 0xff), UInt8(kind >> 8), 1, 0, 0, 0]
            bytes += [UInt8(length & 0xff), UInt8((length >> 8) & 0xff), UInt8((length >> 16) & 0xff), 0]
            return bytes
        }
        let cases: [([UInt8], ControlV2FatalError)] = [
            (header(magic: Array("ODFT".utf8), kind: 11, length: 0), .badMagic),
            (header(version: 1, kind: 11, length: 0), .badVersion(1)),
            (header(version: 3, kind: 11, length: 0), .badVersion(3)),
            (header(kind: 13, length: 0), .unknownKind(13)),
            (header(kind: 64, length: 12), .wrongDirection(64)),
            (header(kind: 11, length: 1), .badPayloadLength(kind: 11, length: 1)),
            (header(kind: 3, length: 1587), .badPayloadLength(kind: 3, length: 1587)),
            (header(kind: 12, length: 65_521), .badPayloadLength(kind: 12, length: 65_521)),
            (header(kind: 5, length: 15), .badPayloadLength(kind: 5, length: 15)),
            (header(kind: 5, length: 16 + 250), .badPayloadLength(kind: 5, length: 266)),
        ]
        for (bytes, expected) in cases {
            var decoder = ControlV2Decoder()
            decoder.append(bytes)
            XCTAssertThrowsError(try decoder.nextFrame()) { XCTAssertEqual($0 as? ControlV2FatalError, expected) }
        }
        // A frame that is complete before the broken one is still delivered.
        var decoder = ControlV2Decoder()
        decoder.append(ControlV2Frame(kind: .query, requestID: 1).encoded() + header(kind: 99, length: 0))
        XCTAssertEqual(try decoder.nextFrame()?.frame.kind, .query)
        XCTAssertThrowsError(try decoder.nextFrame())
        // EOF in the middle of a frame.
        var truncated = ControlV2Decoder()
        truncated.append(Array(ControlV2Frame(kind: .query, requestID: 1).encoded().prefix(10)))
        XCTAssertNil(try truncated.nextFrame())
        XCTAssertThrowsError(try truncated.endOfStream()) { XCTAssertEqual($0 as? ControlV2FatalError, .truncated) }
    }

    private func openDevNull() -> Int32 { Darwin.open("/dev/null", O_RDONLY) }

    private func isOpen(_ descriptor: Int32) -> Bool { fcntl(descriptor, F_GETFD) >= 0 }

    func testDescriptorOnlyWithTheFirstByteOfAttachResource() throws {
        let attach = ControlV2Frame(request: .attachResource(ControlV2Attach(role: .dropFile)), requestID: 1).encoded()
        // The descriptor arrives with the first bytes of ATTACH_RESOURCE, the rest later.
        var decoder = ControlV2Decoder()
        let descriptor = openDevNull()
        decoder.append(Array(attach.prefix(10)), descriptor: descriptor)
        XCTAssertNil(try decoder.nextFrame())
        decoder.append(Array(attach.dropFirst(10)))
        let received = try decoder.nextFrame()
        XCTAssertEqual(received?.descriptor, descriptor)
        Darwin.close(descriptor)

        // A descriptor with another kind is closed and fatal.
        var other = ControlV2Decoder()
        let second = openDevNull()
        other.append(ControlV2Frame(kind: .query, requestID: 1).encoded(), descriptor: second)
        XCTAssertThrowsError(try other.nextFrame())
        XCTAssertFalse(isOpen(second))

        // ATTACH_RESOURCE without a descriptor is fatal.
        var bare = ControlV2Decoder()
        bare.append(attach)
        XCTAssertThrowsError(try bare.nextFrame())

        // A descriptor in a read that does not start a frame is closed and fatal.
        var middle = ControlV2Decoder()
        middle.append(Array(attach.prefix(5)))
        let third = openDevNull()
        middle.append(Array(attach.dropFirst(5)), descriptor: third)
        XCTAssertThrowsError(try middle.nextFrame())
        XCTAssertFalse(isOpen(third))
    }

    func testRequestsRoundTrip() throws {
        let requests: [ControlV2Request] = [
            .hello, .configure(ControlV2Configure()), .attachResource(ControlV2Attach(role: .disk, index: 3)),
            .prepareDisk(ControlV2PrepareDisk(capacityBytes: 1 << 30, name: Array("d.img".utf8))),
            .start(operationID: 0), .pause(reason: 2), .resume(reason: 3), .stop(operationID: 9), .shutdown(operationID: 0),
            .query, .setClipboard([1, 2, 3]), .reservedStage9(.checkpoint),
        ]
        for (index, request) in requests.enumerated() {
            let frame = ControlV2Frame(request: request, requestID: UInt32(index))
            if case .attachResource = request {
                XCTAssertEqual(try ControlV2Request.parse(frame), request) // the descriptor rules are covered above
                continue
            }
            let decoded = try decodeAll(frame.encoded())
            XCTAssertEqual(decoded, [frame])
            XCTAssertEqual(try ControlV2Request.parse(decoded[0]), request)
        }
        XCTAssertThrowsError(try ControlV2Request.parse(ControlV2Frame(kind: .pause, requestID: 1, payload: [1, 0, 0, 0, 1, 0, 0, 0])))
        XCTAssertThrowsError(try ControlV2Request.parse(ControlV2Frame(kind: .prepareDisk, requestID: 1,
                                                                       payload: [UInt8](repeating: 0, count: 16) + [0x41])))
    }

    func testPrepareDiskValidation() {
        XCTAssertNil(ControlV2PrepareDisk(capacityBytes: 4096, name: Array("d".utf8)).validate())
        XCTAssertTrue(ControlV2PrepareDisk(format: 2, capacityBytes: 4096, name: Array("d".utf8)).validate()
            == ControlV2ValueError(.invalidArgument, aux0: 0, detail: "format"))
        XCTAssertEqual(ControlV2PrepareDisk(capacityBytes: 0, name: Array("d".utf8)).validate()?.aux0, 8)
        XCTAssertEqual(ControlV2PrepareDisk(capacityBytes: 1, name: Array("a/b".utf8)).validate()?.code, .invalidName)
    }

    func testResponsesRoundTripAndErrorDetail() throws {
        let responses: [ControlV2Response] = [
            .ready(profile: 2), .ack(ControlV2Ack(ackedKind: 6, flags: 1, state: 2)),
            .state(ControlV2StateReport(state: 3, flags: 3, lastReason: 1)),
            .stateChanged(from: 1, to: 2, reason: 1), .progress(requestKind: 5, completed: 10, total: 20),
            .stats(ControlV2Stats(state: 2, pollCount: 52, framesPublished: 1, rssBytes: 4096)),
            .error(ControlV2ErrorReport(code: .busy, failedKind: 0, flags: 2, state: 1, aux0: 1, aux1: 2, detail: "busy")),
        ]
        for response in responses {
            let frame = ControlV2Frame(response: response, requestID: 7)
            let decoded = try decodeAll(frame.encoded(), direction: .towardController)
            XCTAssertEqual(try ControlV2Response.parse(decoded[0]), response)
        }
        XCTAssertEqual(ControlV2Frame(response: .stats(ControlV2Stats(state: 2, pollCount: 0, framesPublished: 0, rssBytes: 0)),
                                      requestID: 0).encoded().count, 16 + 32)
        // The detail is cut on a character boundary (section 2): 255 ASCII bytes and one 2-byte character.
        let long = String(repeating: "a", count: 255) + "é"
        let detail = ControlV2ErrorReport.truncatedDetail(long)
        XCTAssertEqual(detail.count, 255)
        XCTAssertNotNil(String(bytes: detail, encoding: .utf8))
        // A code that is not in the table is read, not rejected (section 6.7).
        let unknown = ControlV2ErrorReport(code: 999, failedKind: 6, state: 1)
        XCTAssertEqual(try ControlV2Response.parse(ControlV2Frame(response: .error(unknown), requestID: 1)), .error(unknown))
        // READY with another profile and a non-zero reserved are violations.
        XCTAssertThrowsError(try ControlV2Response.parse(ControlV2Frame(kind: .ready, requestID: 0, payload: [3, 0, 0, 0, 0, 0, 0, 0])))
        XCTAssertThrowsError(try ControlV2Response.parse(ControlV2Frame(kind: .ready, requestID: 0, payload: [2, 0, 0, 0, 1, 0, 0, 0])))
    }

    /// A frame made by hand with a payload of the wrong length is an error of `parse`, not a trap; an unknown ACK code is an error.
    func testParseChecksLengthAndAckCode() {
        for kind in [ControlV2Kind.hello, .query, .start, .pause, .configure, .attachResource, .prepareDisk, .stop] {
            XCTAssertThrowsError(try ControlV2Request.parse(ControlV2Frame(kind: kind, requestID: 1, payload: [1, 2, 3])), "\(kind)")
        }
        XCTAssertThrowsError(try ControlV2Request.parse(ControlV2Frame(kind: .start, requestID: 1, payload: [UInt8](repeating: 0, count: 9))))
        for kind in [ControlV2Kind.ready, .ack, .state, .stateChanged, .progress, .stats, .error] {
            XCTAssertThrowsError(try ControlV2Response.parse(ControlV2Frame(kind: kind, requestID: 0, payload: [])), "\(kind)")
            XCTAssertThrowsError(try ControlV2Response.parse(ControlV2Frame(kind: kind, requestID: 0, payload: [UInt8](repeating: 0, count: 5))))
        }
        // ACK: code 0 and 1 are known, 2 is not.
        func ack(code: UInt8) -> ControlV2Frame {
            ControlV2Frame(kind: .ack, requestID: 1, payload: [code, 0, 0, 0, 6, 0, 0, 0, 2, 0, 0, 0])
        }
        XCTAssertNoThrow(try ControlV2Response.parse(ack(code: 0)))
        XCTAssertNoThrow(try ControlV2Response.parse(ack(code: 1)))
        XCTAssertThrowsError(try ControlV2Response.parse(ack(code: 2)))
    }
}
