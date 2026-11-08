import Darwin
import Foundation

public enum ControlKind: UInt16 {
    case hello = 1
    case ready = 2
    case run = 3
    case runAck = 4
    case stop = 5
    case stopAck = 6
    case result = 7
    case error = 255
}

public struct ControlFrame {
    public static let magic = Array("ODCP".utf8)
    public static let versionV0: UInt16 = 0
    public static let versionV1: UInt16 = 1
    public static let headerLength = 16
    static let maxPayloadLength = 8192

    public let kind: ControlKind
    public let requestID: UInt32
    public let payload: Data
    public let protocolVersion: UInt16

    public init(kind: ControlKind, requestID: UInt32, payload: Data, protocolVersion: UInt16 = versionV0) {
        self.kind = kind
        self.requestID = requestID
        self.payload = payload
        self.protocolVersion = protocolVersion
    }

    public func write(to fd: Int32) throws {
        try writeAll(fd: fd, data: encoded())
    }

    public func encoded() throws -> Data {
        guard payload.count <= Self.maxPayloadLength else { throw ControlFrameError.tooLarge }
        var bytes = Data(Self.magic)
        bytes.appendLE(protocolVersion)
        bytes.appendLE(kind.rawValue)
        bytes.appendLE(requestID)
        bytes.appendLE(UInt32(payload.count))
        bytes.append(payload)
        return bytes
    }

    public static func read(from fd: Int32) throws -> ControlFrame {
        try read(from: fd, expectedVersion: versionV0)
    }

    public static func readInitial(from fd: Int32) throws -> ControlFrame {
        let header = try readExact(fd: fd, length: headerLength)
        let bytes = [UInt8](header)
        guard Array(bytes[0..<4]) == magic else { throw ControlFrameError.badMagic }
        let frameVersion = bytes.readLE(UInt16.self, at: 4)
        guard frameVersion == versionV0 || frameVersion == versionV1 else {
            throw ControlFrameError.badVersion
        }
        guard let kind = ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) else {
            throw ControlFrameError.badKind
        }
        let requestID = bytes.readLE(UInt32.self, at: 8)
        let length = Int(bytes.readLE(UInt32.self, at: 12))
        guard length <= maxPayloadLength else { throw ControlFrameError.tooLarge }
        return ControlFrame(
            kind: kind,
            requestID: requestID,
            payload: try readExact(fd: fd, length: length),
            protocolVersion: frameVersion
        )
    }

    private static func read(from fd: Int32, expectedVersion: UInt16) throws -> ControlFrame {
        let header = try readExact(fd: fd, length: headerLength)
        let bytes = [UInt8](header)
        guard Array(bytes[0..<4]) == magic else { throw ControlFrameError.badMagic }
        guard bytes.readLE(UInt16.self, at: 4) == expectedVersion else { throw ControlFrameError.badVersion }
        guard let kind = ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) else {
            throw ControlFrameError.badKind
        }
        let requestID = bytes.readLE(UInt32.self, at: 8)
        let length = Int(bytes.readLE(UInt32.self, at: 12))
        guard length <= maxPayloadLength else { throw ControlFrameError.tooLarge }
        return ControlFrame(kind: kind, requestID: requestID, payload: try readExact(fd: fd, length: length))
    }

    fileprivate static func decodePrefix(_ data: Data, expectedVersion: UInt16) throws -> (frame: ControlFrame, length: Int)? {
        let bytes = [UInt8](data)
        guard bytes.count >= headerLength else { return nil }
        let frameLength = try frameLength(fromHeader: data, expectedVersion: expectedVersion)
        guard let kind = ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) else {
            throw ControlFrameError.badKind
        }
        let requestID = bytes.readLE(UInt32.self, at: 8)
        guard bytes.count >= frameLength else { return nil }
        return (
            ControlFrame(
                kind: kind,
                requestID: requestID,
                payload: Data(bytes[headerLength..<frameLength]),
                protocolVersion: expectedVersion
            ),
            frameLength
        )
    }

    fileprivate static func frameLength(fromHeader data: Data, expectedVersion: UInt16) throws -> Int {
        guard data.count >= headerLength else { throw ControlFrameError.truncated }
        let bytes = [UInt8](data.prefix(headerLength))
        guard Array(bytes[0..<4]) == magic else { throw ControlFrameError.badMagic }
        guard bytes.readLE(UInt16.self, at: 4) == expectedVersion else { throw ControlFrameError.badVersion }
        guard ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) != nil else {
            throw ControlFrameError.badKind
        }
        let payloadLength = Int(bytes.readLE(UInt32.self, at: 12))
        guard payloadLength <= maxPayloadLength else { throw ControlFrameError.tooLarge }
        return headerLength + payloadLength
    }
}

public struct ControlFrameDecoder {
    private let expectedVersion: UInt16
    private var buffered = Data()

    public init(expectedVersion: UInt16 = ControlFrame.versionV0) {
        self.expectedVersion = expectedVersion
    }

    public var isEmpty: Bool { buffered.isEmpty }

    public mutating func append(_ bytes: Data) throws -> [ControlFrame] {
        var frames: [ControlFrame] = []
        var inputOffset = 0
        while inputOffset < bytes.count {
            if buffered.count < ControlFrame.headerLength {
                let count = min(ControlFrame.headerLength - buffered.count, bytes.count - inputOffset)
                buffered.append(bytes[inputOffset..<(inputOffset + count)])
                inputOffset += count
                if buffered.count < ControlFrame.headerLength { break }
            }

            let frameLength = try ControlFrame.frameLength(fromHeader: buffered, expectedVersion: expectedVersion)
            let count = min(frameLength - buffered.count, bytes.count - inputOffset)
            if count > 0 {
                buffered.append(bytes[inputOffset..<(inputOffset + count)])
                inputOffset += count
            }
            if buffered.count == frameLength {
                guard let decoded = try ControlFrame.decodePrefix(buffered, expectedVersion: expectedVersion) else {
                    throw ControlFrameError.truncated
                }
                frames.append(decoded.frame)
                buffered.removeAll(keepingCapacity: true)
            }
        }
        return frames
    }
}

enum ControlFrameError: Error, CustomStringConvertible {
    case badMagic
    case badVersion
    case badKind
    case truncated
    case tooLarge
    case writeFailed

    var description: String {
        switch self {
        case .badMagic: return "invalid control frame magic"
        case .badVersion: return "unsupported control protocol version"
        case .badKind: return "unknown control message kind"
        case .truncated: return "control channel closed during a frame"
        case .tooLarge: return "control frame exceeds the maximum size"
        case .writeFailed: return "control frame write failed"
        }
    }
}

func readExact(fd: Int32, length: Int) throws -> Data {
    guard length >= 0 else { throw ControlFrameError.truncated }
    var storage = [UInt8](repeating: 0, count: length)
    var offset = 0
    while offset < length {
        let result = storage.withUnsafeMutableBytes { raw in
            Darwin.read(fd, raw.baseAddress!.advanced(by: offset), length - offset)
        }
        if result == 0 { throw ControlFrameError.truncated }
        if result < 0 {
            if errno == EINTR { continue }
            throw ControlFrameError.truncated
        }
        offset += result
    }
    return Data(storage)
}

func writeAll(fd: Int32, data: Data) throws {
    var offset = 0
    while offset < data.count {
        let result = data.withUnsafeBytes { raw in
            Darwin.write(fd, raw.baseAddress!.advanced(by: offset), data.count - offset)
        }
        if result < 0 {
            if errno == EINTR { continue }
            throw ControlFrameError.writeFailed
        }
        if result == 0 { throw ControlFrameError.writeFailed }
        offset += result
    }
}

extension Data {
    public mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }
}

extension Array where Element == UInt8 {
    public func readLE<T: FixedWidthInteger>(_ type: T.Type, at offset: Int) -> T {
        var value: T = 0
        for index in 0..<MemoryLayout<T>.size {
            value |= T(self[offset + index]) << (index * 8)
        }
        return value
    }
}
