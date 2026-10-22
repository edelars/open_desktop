import Darwin
import Foundation

enum ControlKind: UInt16 {
    case hello = 1
    case ready = 2
    case run = 3
    case runAck = 4
    case stop = 5
    case stopAck = 6
    case result = 7
    case error = 255
}

struct ControlFrame {
    static let magic = Array("ODCP".utf8)
    static let version: UInt16 = 0
    static let headerLength = 16
    static let maxPayloadLength = 8192

    let kind: ControlKind
    let requestID: UInt32
    let payload: Data

    func write(to fd: Int32) throws {
        guard payload.count <= Self.maxPayloadLength else { throw ControlFrameError.tooLarge }
        var bytes = Data(Self.magic)
        bytes.appendLE(Self.version)
        bytes.appendLE(kind.rawValue)
        bytes.appendLE(requestID)
        bytes.appendLE(UInt32(payload.count))
        bytes.append(payload)
        try writeAll(fd: fd, data: bytes)
    }

    static func read(from fd: Int32) throws -> ControlFrame {
        let header = try readExact(fd: fd, length: headerLength)
        let bytes = [UInt8](header)
        guard Array(bytes[0..<4]) == magic else { throw ControlFrameError.badMagic }
        guard bytes.readLE(UInt16.self, at: 4) == version else { throw ControlFrameError.badVersion }
        guard let kind = ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) else {
            throw ControlFrameError.badKind
        }
        let requestID = bytes.readLE(UInt32.self, at: 8)
        let length = Int(bytes.readLE(UInt32.self, at: 12))
        guard length <= maxPayloadLength else { throw ControlFrameError.tooLarge }
        return ControlFrame(kind: kind, requestID: requestID, payload: try readExact(fd: fd, length: length))
    }

    fileprivate static func decodePrefix(_ data: Data) throws -> (frame: ControlFrame, length: Int)? {
        let bytes = [UInt8](data)
        guard bytes.count >= headerLength else { return nil }
        guard Array(bytes[0..<4]) == magic else { throw ControlFrameError.badMagic }
        guard bytes.readLE(UInt16.self, at: 4) == version else { throw ControlFrameError.badVersion }
        guard let kind = ControlKind(rawValue: bytes.readLE(UInt16.self, at: 6)) else {
            throw ControlFrameError.badKind
        }
        let requestID = bytes.readLE(UInt32.self, at: 8)
        let payloadLength = Int(bytes.readLE(UInt32.self, at: 12))
        guard payloadLength <= maxPayloadLength else { throw ControlFrameError.tooLarge }
        let frameLength = headerLength + payloadLength
        guard bytes.count >= frameLength else { return nil }
        return (
            ControlFrame(kind: kind, requestID: requestID, payload: Data(bytes[headerLength..<frameLength])),
            frameLength
        )
    }
}

struct ControlFrameDecoder {
    private var buffered = Data()

    mutating func append(_ bytes: Data) throws -> [ControlFrame] {
        buffered.append(bytes)
        var frames: [ControlFrame] = []
        while let decoded = try ControlFrame.decodePrefix(buffered) {
            frames.append(decoded.frame)
            buffered.removeSubrange(0..<decoded.length)
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
    mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }
}

extension Array where Element == UInt8 {
    func readLE<T: FixedWidthInteger>(_ type: T.Type, at offset: Int) -> T {
        var value: T = 0
        for index in 0..<MemoryLayout<T>.size {
            value |= T(self[offset + index]) << (index * 8)
        }
        return value
    }
}
