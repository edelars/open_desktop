import Darwin
import Foundation

// Encoding and decoding of control protocol v2 (protocols/control/CONTROL_PROTOCOL_V2.md, S11-04).
//
// Every integer is unsigned little-endian; there is no padding. The specification is the source of widths, offsets and limits;
// section numbers in the comments refer to it. This file knows nothing about VM state: the state machine lives in the VMHost
// (apps/vmhost/Sources/Lifecycle.swift). A structure violation is thrown as `ControlV2FatalError` (the connection is closed
// without a reply, section 11); a value violation of a well-formed frame is returned as `ControlV2ValueError` and answered by
// an ERROR frame on a live connection (sections 2, 6.1, 6.8).

public enum ControlV2 {
    public static let magic = Array("ODCP".utf8)
    public static let version: UInt16 = 2
    public static let headerLength = 16
    public static let maxFrameLength = 65_536
    public static let maxPayloadLength = 65_520
    /// Output queue of the VMHost (section 2): two maximum frames.
    public static let outputQueueLimit = 131_072
    /// `READY.profile` (section 4 item 3).
    public static let readyProfile: UInt32 = 2
    public static let configurePayloadLength = 1_588
    public static let maxDiskNameLength = 249
    public static let maxGuestNameLength = 64
    public static let maxErrorDetailLength = 256
    public static let diskSlotCount = 4
    public static let sharedFolderSlotCount = 8
    public static let requestRingSize = 64
    public static let statsIntervalMs = 1_000
}

/// Kinds of section 3. Values not listed are not defined.
public enum ControlV2Kind: UInt16 {
    case hello = 1
    case ready = 2
    case configure = 3
    case attachResource = 4
    case prepareDisk = 5
    case start = 6
    case pause = 7
    case resume = 8
    case stop = 9
    case shutdown = 10
    case query = 11
    case setClipboard = 12
    case suspend = 40
    case save = 41
    case checkpoint = 42
    case restore = 43
    case ack = 64
    case state = 65
    case stateChanged = 66
    case progress = 67
    case stats = 68
    case clipboardChanged = 69
    case error = 255

    /// True for kinds sent by the controlling process (to the VMHost).
    public var isRequest: Bool {
        switch self {
        case .ready, .ack, .state, .stateChanged, .progress, .stats, .clipboardChanged, .error: return false
        default: return true
        }
    }

    /// True when the payload length of a frame of this kind may be `length` (section 3). `PREPARE_DISK` is checked against
    /// `name_len` when the payload is parsed.
    func acceptsPayloadLength(_ length: Int) -> Bool {
        switch self {
        case .hello, .query, .suspend, .save, .checkpoint, .restore: return length == 0
        case .ready, .attachResource, .start, .pause, .resume, .stop, .shutdown: return length == 8
        case .configure: return length == ControlV2.configurePayloadLength
        case .prepareDisk: return length >= 16 && length <= 16 + ControlV2.maxDiskNameLength
        case .setClipboard, .clipboardChanged: return length <= ControlV2.maxPayloadLength
        case .ack, .state, .stateChanged: return length == 12
        case .progress: return length == 24
        case .stats: return length == 32
        case .error: return length >= 32 && length <= 32 + ControlV2.maxErrorDetailLength
        }
    }
}

/// States of section 7.1.
public enum ControlV2State: UInt32 {
    case stopped = 1
    case running = 2
    case paused = 3
    case shuttingDown = 4
    case failed = 5
}

/// Reasons of `STATE_CHANGED` (section 6.4); `PAUSE` and `RESUME` use the subsets of section 6.8.
public enum ControlV2Reason: UInt32 {
    case user = 1
    case systemSleep = 2
    case systemWake = 3
    case guestShutdown = 4
    case hostFailure = 5
    case diskSpace = 6
}

/// Codes of `ERROR` (section 12.1). The set is closed for the sender.
public enum ControlV2ErrorCode: UInt32 {
    case invalidState = 1
    case invalidConfig = 2
    case notConfigured = 3
    case unsupported = 4
    case unsupportedDurability = 5
    case unsupportedNoAgent = 6
    case busy = 7
    case lockChanged = 8
    case lockInvalid = 9
    case pauseTimeout = 10
    case shutdownTimeout = 11
    case hostMemoryExceeded = 12
    case diskImageUnavailable = 13
    case invalidName = 14
    case diskExists = 15
    case insufficientSpace = 16
    case invalidResource = 17
    case resourceConflict = 18
    case resourceLimit = 19
    case invalidArgument = 20
    case operationInProgress = 21
    case requestIDConflict = 22
    case requestIDStale = 23
    case ioError = 24
    case hostFailure = 25
}

/// Roles of `ATTACH_RESOURCE` (section 8).
public enum ControlV2Role: UInt16 {
    case bootMedia = 1
    case disk = 2
    case sharedFolderRoot = 3
    case dropFile = 4
}

/// A violation of the frame structure or of the order of the protocol: the connection is closed without a reply (section 11).
public enum ControlV2FatalError: Error, Equatable, CustomStringConvertible {
    case badMagic
    case badVersion(UInt16)
    case unknownKind(UInt16)
    case wrongDirection(UInt16)
    case badPayloadLength(kind: UInt16, length: UInt32)
    case nonZeroReserved(String)
    case structure(String)
    case requestIDZero
    case requestIDSkipped
    case helloExpected
    case helloRepeated
    case descriptor(String)
    case truncated

    public var description: String {
        switch self {
        case .badMagic: return "control v2: bad magic"
        case .badVersion(let value): return "control v2: protocol version \(value) is not 2"
        case .unknownKind(let value): return "control v2: unknown kind \(value)"
        case .wrongDirection(let value): return "control v2: kind \(value) is not valid in this direction"
        case .badPayloadLength(let kind, let length): return "control v2: payload length \(length) is invalid for kind \(kind)"
        case .nonZeroReserved(let what): return "control v2: reserved bytes are not zero (\(what))"
        case .structure(let what): return "control v2: invalid structure (\(what))"
        case .requestIDZero: return "control v2: request id 0 is not allowed here"
        case .requestIDSkipped: return "control v2: request id skips a value"
        case .helloExpected: return "control v2: the first frame is not HELLO"
        case .helloRepeated: return "control v2: repeated HELLO"
        case .descriptor(let what): return "control v2: descriptor violation (\(what))"
        case .truncated: return "control v2: connection closed inside a frame"
        }
    }
}

/// A value violation of a well-formed frame: an ERROR frame with this code answers it (state does not change).
public struct ControlV2ValueError: Error, Equatable {
    public var code: ControlV2ErrorCode
    public var aux0: UInt64
    public var aux1: UInt64
    public var detail: String

    public init(_ code: ControlV2ErrorCode, aux0: UInt64 = 0, aux1: UInt64 = 0, detail: String = "") {
        self.code = code
        self.aux0 = aux0
        self.aux1 = aux1
        self.detail = detail
    }
}

// MARK: little-endian helpers

fileprivate extension Array where Element == UInt8 {
    mutating func putLE<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }

    mutating func putZeros(_ count: Int) { append(contentsOf: [UInt8](repeating: 0, count: count)) }

    func isZero(_ range: Range<Int>) -> Bool { self[range].allSatisfy { $0 == 0 } }
}

// MARK: frame

public struct ControlV2Frame: Equatable {
    public var kind: ControlV2Kind
    public var requestID: UInt32
    public var payload: [UInt8]

    public init(kind: ControlV2Kind, requestID: UInt32, payload: [UInt8] = []) {
        self.kind = kind
        self.requestID = requestID
        self.payload = payload
    }

    public init(request: ControlV2Request, requestID: UInt32) {
        self.init(kind: request.kind, requestID: requestID, payload: request.payload())
    }

    public init(response: ControlV2Response, requestID: UInt32) {
        self.init(kind: response.kind, requestID: requestID, payload: response.payload())
    }

    /// Header (16 bytes) and payload. A payload above 65 520 bytes is a programming error of the caller.
    public func encoded() -> [UInt8] {
        precondition(payload.count <= ControlV2.maxPayloadLength, "control v2 payload exceeds 65520 bytes")
        var bytes = ControlV2.magic
        bytes.putLE(ControlV2.version)
        bytes.putLE(kind.rawValue)
        bytes.putLE(requestID)
        bytes.putLE(UInt32(payload.count))
        bytes.append(contentsOf: payload)
        return bytes
    }
}

/// Which side of the connection reads: it fixes the kinds that may arrive (section 3).
public enum ControlV2Direction {
    /// The VMHost reads requests.
    case towardHost
    /// The controlling process reads responses and events.
    case towardController
}

/// One decoded frame and the descriptor that arrived with its first byte (only `ATTACH_RESOURCE`, section 8).
public struct ControlV2Received {
    public var frame: ControlV2Frame
    public var descriptor: Int32?

    public init(frame: ControlV2Frame, descriptor: Int32? = nil) {
        self.frame = frame
        self.descriptor = descriptor
    }
}

/// Incremental reader. The header is validated completely before the payload is stored; partial reads are expected.
/// Complete frames that arrived before a fatal error are still delivered, then `nextFrame()` throws.
public struct ControlV2Decoder {
    private let direction: ControlV2Direction
    private var buffer: [UInt8] = []
    private var pending: [ControlV2Received] = []
    private var heldDescriptor: Int32?
    private var fatal: ControlV2FatalError?
    private var cursor = 0

    public init(direction: ControlV2Direction = .towardHost) {
        self.direction = direction
    }

    /// Bytes of an incomplete frame; EOF while this is non-zero is a truncated frame (section 11).
    public var bufferedPartialBytes: Int { buffer.count - cursor }

    public var hasPendingFrames: Bool { !pending.isEmpty || fatal != nil }

    /// `descriptor` (already `FD_CLOEXEC`) is owned by the decoder from this call on: it is delivered with the frame that starts
    /// at the first byte of `bytes`, or closed when that is a violation.
    public mutating func append(_ bytes: [UInt8], descriptor: Int32? = nil) {
        if let descriptor {
            if fatal != nil {
                Darwin.close(descriptor)
            } else if bufferedPartialBytes != 0 || heldDescriptor != nil {
                Darwin.close(descriptor)
                fail(.descriptor("a descriptor arrived where no frame starts"))
            } else {
                heldDescriptor = descriptor
            }
        }
        guard fatal == nil else { return }
        buffer.append(contentsOf: bytes)
        parse()
    }

    /// The next complete frame, nil when more bytes are needed, a throw once the stream is broken and the frames before are consumed.
    public mutating func nextFrame() throws -> ControlV2Received? {
        if !pending.isEmpty { return pending.removeFirst() }
        if let fatal { throw fatal }
        return nil
    }

    /// Call at EOF between reads: a frame in progress is a truncated frame.
    public mutating func endOfStream() throws {
        if let fatal { throw fatal }
        if bufferedPartialBytes != 0 { throw ControlV2FatalError.truncated }
        if let held = heldDescriptor {
            Darwin.close(held)
            heldDescriptor = nil
        }
    }

    /// Closes a descriptor that no frame has claimed. Used at teardown.
    public mutating func discard() {
        if let held = heldDescriptor {
            Darwin.close(held)
            heldDescriptor = nil
        }
        for item in pending { if let descriptor = item.descriptor { Darwin.close(descriptor) } }
        pending.removeAll()
    }

    private mutating func fail(_ error: ControlV2FatalError) {
        fatal = error
        if let held = heldDescriptor {
            Darwin.close(held)
            heldDescriptor = nil
        }
    }

    private mutating func parse() {
        while fatal == nil {
            let available = buffer.count - cursor
            guard available >= ControlV2.headerLength else { break }
            let header = Array(buffer[cursor..<(cursor + ControlV2.headerLength)])
            guard Array(header[0..<4]) == ControlV2.magic else { return fail(.badMagic) }
            let version = header.readLE(UInt16.self, at: 4)
            guard version == ControlV2.version else { return fail(.badVersion(version)) }
            let rawKind = header.readLE(UInt16.self, at: 6)
            guard let kind = ControlV2Kind(rawValue: rawKind) else { return fail(.unknownKind(rawKind)) }
            guard kind.isRequest == (direction == .towardHost) else { return fail(.wrongDirection(rawKind)) }
            let requestID = header.readLE(UInt32.self, at: 8)
            let length = header.readLE(UInt32.self, at: 12)
            guard length <= UInt32(ControlV2.maxPayloadLength), kind.acceptsPayloadLength(Int(length)) else {
                return fail(.badPayloadLength(kind: rawKind, length: length))
            }
            guard available >= ControlV2.headerLength + Int(length) else { break }
            let start = cursor + ControlV2.headerLength
            let payload = Array(buffer[start..<(start + Int(length))])
            cursor = start + Int(length)
            var descriptor: Int32?
            if let held = heldDescriptor {
                heldDescriptor = nil
                if kind == .attachResource {
                    descriptor = held
                } else {
                    Darwin.close(held)
                    return fail(.descriptor("a descriptor arrived with a frame that is not ATTACH_RESOURCE"))
                }
            } else if kind == .attachResource {
                return fail(.descriptor("ATTACH_RESOURCE without a descriptor"))
            }
            pending.append(ControlV2Received(frame: ControlV2Frame(kind: kind, requestID: requestID, payload: payload),
                                             descriptor: descriptor))
        }
        if cursor > 0 {
            buffer.removeSubrange(0..<cursor)
            cursor = 0
        }
    }
}

// MARK: names (sections 2, 6.1, 9)

public enum ControlV2Names {
    /// Rules of an image name inside `disks/` (VM_BUNDLE 3.3, section 6.1): valid UTF-8, 1...249 bytes, not `.` or `..`, no `/`,
    /// NUL or control characters U+0000-U+001F, U+007F.
    public static func isValidDiskName(_ bytes: [UInt8]) -> Bool {
        guard (1...ControlV2.maxDiskNameLength).contains(bytes.count) else { return false }
        guard bytes != [0x2E], bytes != [0x2E, 0x2E] else { return false }
        guard bytes.allSatisfy({ $0 >= 0x20 && $0 != 0x7F && $0 != 0x2F }) else { return false }
        return String(bytes: bytes, encoding: .utf8) != nil
    }

    /// Rules of `guest_name` (VM_BUNDLE 3.3): valid UTF-8, 1...64 bytes, no `/`, `\`, `:`, NUL, control characters.
    public static func isValidGuestName(_ bytes: [UInt8]) -> Bool {
        guard (1...ControlV2.maxGuestNameLength).contains(bytes.count) else { return false }
        guard bytes.allSatisfy({ $0 >= 0x20 && $0 != 0x7F && $0 != 0x2F && $0 != 0x5C && $0 != 0x3A }) else { return false }
        return String(bytes: bytes, encoding: .utf8) != nil
    }
}

// MARK: CONFIGURE (section 6.1)

public struct ControlV2Configure: Equatable {
    public struct Disk: Equatable {
        /// `1` image inside the bundle (by name), `2` image outside the bundle (descriptor, role `disk`).
        public var source: UInt8
        /// `1` raw.
        public var format: UInt8
        public var writable: UInt8
        /// Bytes of `name`; kept as sent so a wrong length is visible to `validate()`.
        public var nameLength: UInt8
        public var name: [UInt8]

        public init(source: UInt8 = 1, format: UInt8 = 1, writable: UInt8 = 1, name: [UInt8]) {
            self.source = source
            self.format = format
            self.writable = writable
            self.nameLength = UInt8(truncatingIfNeeded: name.count)
            self.name = name
        }

        init(source: UInt8, format: UInt8, writable: UInt8, nameLength: UInt8, name: [UInt8]) {
            self.source = source
            self.format = format
            self.writable = writable
            self.nameLength = nameLength
            self.name = name
        }
    }

    public struct SharedFolder: Equatable {
        public var readOnly: UInt8
        public var guestName: [UInt8]

        public init(readOnly: UInt8 = 1, guestName: [UInt8]) {
            self.readOnly = readOnly
            self.guestName = guestName
        }
    }

    public var machineProfile: UInt32
    public var memoryMiB: UInt32
    public var vcpuCount: UInt16
    public var displayScreens: UInt16
    public var network: UInt8
    public var audio: UInt8
    public var clipboardSync: UInt8
    public var bootMedia: UInt8
    public var disks: [Disk]
    public var sharedFolders: [SharedFolder]

    public init(
        machineProfile: UInt32 = 1, memoryMiB: UInt32 = 4096, vcpuCount: UInt16 = 1, displayScreens: UInt16 = 1,
        network: UInt8 = 0, audio: UInt8 = 0, clipboardSync: UInt8 = 0, bootMedia: UInt8 = 0,
        disks: [Disk] = [], sharedFolders: [SharedFolder] = []
    ) {
        self.machineProfile = machineProfile
        self.memoryMiB = memoryMiB
        self.vcpuCount = vcpuCount
        self.displayScreens = displayScreens
        self.network = network
        self.audio = audio
        self.clipboardSync = clipboardSync
        self.bootMedia = bootMedia
        self.disks = disks
        self.sharedFolders = sharedFolders
    }

    static let disksOffset = 20
    static let diskSlotLength = 256
    static let sharedFoldersOffset = 1_044
    static let sharedFolderSlotLength = 68

    /// `disk_count > 0`: a machine with a disk (bit 1 of `STATE.flags`; `START`/`STOP` answer `unsupported_durability`).
    public var hasDisks: Bool { !disks.isEmpty }

    public func encoded() -> [UInt8] {
        var bytes: [UInt8] = []
        bytes.putLE(machineProfile)
        bytes.putLE(memoryMiB)
        bytes.putLE(vcpuCount)
        bytes.putLE(displayScreens)
        bytes.append(network)
        bytes.append(audio)
        bytes.append(clipboardSync)
        bytes.append(bootMedia)
        bytes.append(UInt8(truncatingIfNeeded: disks.count))
        bytes.append(UInt8(truncatingIfNeeded: sharedFolders.count))
        bytes.putZeros(2)
        for index in 0..<ControlV2.diskSlotCount {
            var slot = [UInt8](repeating: 0, count: Self.diskSlotLength)
            if index < disks.count {
                let disk = disks[index]
                slot[0] = disk.source
                slot[1] = disk.format
                slot[2] = disk.writable
                slot[3] = disk.nameLength
                for (offset, byte) in disk.name.prefix(252).enumerated() { slot[4 + offset] = byte }
            }
            bytes.append(contentsOf: slot)
        }
        for index in 0..<ControlV2.sharedFolderSlotCount {
            var slot = [UInt8](repeating: 0, count: Self.sharedFolderSlotLength)
            if index < sharedFolders.count {
                let folder = sharedFolders[index]
                slot[0] = folder.readOnly
                slot[1] = UInt8(truncatingIfNeeded: folder.guestName.count)
                for (offset, byte) in folder.guestName.prefix(64).enumerated() { slot[4 + offset] = byte }
            }
            bytes.append(contentsOf: slot)
        }
        return bytes
    }

    /// Structure only (section 11 list): length, reserved, counts, unused slots, tails of names. Values are not judged here.
    public static func parse(_ payload: [UInt8]) throws -> ControlV2Configure {
        guard payload.count == ControlV2.configurePayloadLength else {
            throw ControlV2FatalError.badPayloadLength(kind: ControlV2Kind.configure.rawValue, length: UInt32(payload.count))
        }
        guard payload.isZero(18..<20) else { throw ControlV2FatalError.nonZeroReserved("CONFIGURE offset 18") }
        let diskCount = Int(payload[16])
        let folderCount = Int(payload[17])
        guard diskCount <= ControlV2.diskSlotCount else { throw ControlV2FatalError.structure("disk_count \(diskCount)") }
        guard folderCount <= ControlV2.sharedFolderSlotCount else {
            throw ControlV2FatalError.structure("shared_folder_count \(folderCount)")
        }
        var disks: [Disk] = []
        for index in 0..<ControlV2.diskSlotCount {
            let base = disksOffset + index * diskSlotLength
            let slot = base..<(base + diskSlotLength)
            if index >= diskCount {
                guard payload.isZero(slot) else { throw ControlV2FatalError.structure("unused disk slot \(index) is not zero") }
                continue
            }
            let nameLength = Int(payload[base + 3])
            guard nameLength <= ControlV2.maxDiskNameLength else {
                throw ControlV2FatalError.structure("disk slot \(index) name_len \(nameLength)")
            }
            guard payload.isZero((base + 4 + nameLength)..<(base + diskSlotLength)) else {
                throw ControlV2FatalError.structure("disk slot \(index) name tail is not zero")
            }
            disks.append(Disk(source: payload[base], format: payload[base + 1], writable: payload[base + 2],
                              nameLength: payload[base + 3], name: Array(payload[(base + 4)..<(base + 4 + nameLength)])))
        }
        var folders: [SharedFolder] = []
        for index in 0..<ControlV2.sharedFolderSlotCount {
            let base = sharedFoldersOffset + index * sharedFolderSlotLength
            let slot = base..<(base + sharedFolderSlotLength)
            if index >= folderCount {
                guard payload.isZero(slot) else { throw ControlV2FatalError.structure("unused shared folder slot \(index) is not zero") }
                continue
            }
            let nameLength = Int(payload[base + 1])
            guard nameLength <= ControlV2.maxGuestNameLength else {
                throw ControlV2FatalError.structure("shared folder slot \(index) guest_name_len \(nameLength)")
            }
            guard payload.isZero((base + 2)..<(base + 4)) else {
                throw ControlV2FatalError.nonZeroReserved("shared folder slot \(index)")
            }
            guard payload.isZero((base + 4 + nameLength)..<(base + sharedFolderSlotLength)) else {
                throw ControlV2FatalError.structure("shared folder slot \(index) name tail is not zero")
            }
            folders.append(SharedFolder(readOnly: payload[base], guestName: Array(payload[(base + 4)..<(base + 4 + nameLength)])))
        }
        return ControlV2Configure(
            machineProfile: payload.readLE(UInt32.self, at: 0), memoryMiB: payload.readLE(UInt32.self, at: 4),
            vcpuCount: payload.readLE(UInt16.self, at: 8), displayScreens: payload.readLE(UInt16.self, at: 10),
            network: payload[12], audio: payload[13], clipboardSync: payload[14], bootMedia: payload[15],
            disks: disks, sharedFolders: folders)
    }

    /// Values of section 6.1 in order of ascending payload offset; the first violation answers. nil = valid.
    /// `invalid_name` for a disk name, `invalid_config` for everything else; `aux0` is the payload offset of the field.
    public func validate() -> ControlV2ValueError? {
        func config(_ offset: Int, _ what: String) -> ControlV2ValueError {
            ControlV2ValueError(.invalidConfig, aux0: UInt64(offset), detail: what)
        }
        guard machineProfile == 1 || machineProfile == 4 else { return config(0, "machine_profile") }
        guard (256...65_536).contains(memoryMiB) else { return config(4, "memory_mib") }
        guard (1...32).contains(vcpuCount), vcpuCount == 1 else { return config(8, "vcpu_count") }
        let expectedScreens: UInt16 = machineProfile == 1 ? 1 : 2
        guard displayScreens == expectedScreens else { return config(10, "display_screens") }
        guard network <= 1 else { return config(12, "network") }
        guard audio <= 1 else { return config(13, "audio") }
        guard clipboardSync <= 1 else { return config(14, "clipboard_sync") }
        guard bootMedia <= 1 else { return config(15, "boot_media") }
        for (index, disk) in disks.enumerated() {
            let base = Self.disksOffset + index * Self.diskSlotLength
            guard disk.source == 1 || disk.source == 2 else { return config(base, "disk source") }
            guard disk.format == 1 else { return config(base + 1, "disk format") }
            guard disk.writable <= 1 else { return config(base + 2, "disk writable") }
            if disk.source == 2 {
                guard disk.nameLength == 0 else { return config(base + 3, "disk outside the bundle has a name") }
                continue
            }
            guard ControlV2Names.isValidDiskName(disk.name) else {
                return ControlV2ValueError(.invalidName, aux0: UInt64(base + 4), detail: "disk name")
            }
            for earlier in disks[..<index] where earlier.source == 1 && earlier.name == disk.name {
                return config(base + 4, "duplicate disk name")
            }
        }
        for (index, folder) in sharedFolders.enumerated() {
            let base = Self.sharedFoldersOffset + index * Self.sharedFolderSlotLength
            guard folder.readOnly <= 1 else { return config(base, "shared folder read_only") }
            guard ControlV2Names.isValidGuestName(folder.guestName) else { return config(base + 4, "guest_name") }
            for earlier in sharedFolders[..<index] where earlier.guestName == folder.guestName {
                return config(base + 4, "duplicate guest_name")
            }
        }
        return nil
    }
}

// MARK: ATTACH_RESOURCE (section 8), PREPARE_DISK (section 9)

public struct ControlV2Attach: Equatable {
    /// Raw role: values outside `ControlV2Role` are answered `invalid_resource`.
    public var role: UInt16
    public var index: UInt16

    public init(role: UInt16, index: UInt16) {
        self.role = role
        self.index = index
    }

    public init(role: ControlV2Role, index: UInt16 = 0) {
        self.init(role: role.rawValue, index: index)
    }

    public func encoded() -> [UInt8] {
        var bytes: [UInt8] = []
        bytes.putLE(role)
        bytes.putLE(index)
        bytes.putZeros(4)
        return bytes
    }

    public static func parse(_ payload: [UInt8]) throws -> ControlV2Attach {
        guard payload.count == 8 else {
            throw ControlV2FatalError.badPayloadLength(kind: ControlV2Kind.attachResource.rawValue, length: UInt32(payload.count))
        }
        guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("ATTACH_RESOURCE") }
        return ControlV2Attach(role: payload.readLE(UInt16.self, at: 0), index: payload.readLE(UInt16.self, at: 2))
    }
}

public struct ControlV2PrepareDisk: Equatable {
    public var format: UInt16
    public var capacityBytes: UInt64
    public var name: [UInt8]

    public init(format: UInt16 = 1, capacityBytes: UInt64, name: [UInt8]) {
        self.format = format
        self.capacityBytes = capacityBytes
        self.name = name
    }

    public func encoded() -> [UInt8] {
        var bytes: [UInt8] = []
        bytes.putLE(format)
        bytes.putLE(UInt16(truncatingIfNeeded: name.count))
        bytes.putZeros(4)
        bytes.putLE(capacityBytes)
        bytes.append(contentsOf: name)
        return bytes
    }

    public static func parse(_ payload: [UInt8]) throws -> ControlV2PrepareDisk {
        guard payload.count >= 16 else {
            throw ControlV2FatalError.badPayloadLength(kind: ControlV2Kind.prepareDisk.rawValue, length: UInt32(payload.count))
        }
        let nameLength = Int(payload.readLE(UInt16.self, at: 2))
        guard nameLength <= ControlV2.maxDiskNameLength, payload.count == 16 + nameLength else {
            throw ControlV2FatalError.badPayloadLength(kind: ControlV2Kind.prepareDisk.rawValue, length: UInt32(payload.count))
        }
        guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("PREPARE_DISK") }
        return ControlV2PrepareDisk(format: payload.readLE(UInt16.self, at: 0), capacityBytes: payload.readLE(UInt64.self, at: 8),
                                    name: Array(payload[16...]))
    }

    /// Section 9: `format` other than `raw` and a zero capacity are `invalid_argument`; a name that breaks the rules is `invalid_name`.
    public func validate() -> ControlV2ValueError? {
        if format != 1 { return ControlV2ValueError(.invalidArgument, aux0: 0, detail: "format") }
        if capacityBytes == 0 { return ControlV2ValueError(.invalidArgument, aux0: 8, detail: "capacity_bytes") }
        if !ControlV2Names.isValidDiskName(name) { return ControlV2ValueError(.invalidName, aux0: 0, detail: "disk name") }
        return nil
    }
}

// MARK: requests

public enum ControlV2Request: Equatable {
    case hello
    case configure(ControlV2Configure)
    case attachResource(ControlV2Attach)
    case prepareDisk(ControlV2PrepareDisk)
    case start(operationID: UInt64)
    /// Raw `reason`: a value outside section 6.8 is answered `invalid_argument` by the state machine.
    case pause(reason: UInt32)
    case resume(reason: UInt32)
    case stop(operationID: UInt64)
    case shutdown(operationID: UInt64)
    case query
    case setClipboard([UInt8])
    /// `SUSPEND`, `SAVE`, `CHECKPOINT`, `RESTORE`: only the kind (40...43).
    case reservedStage9(ControlV2Kind)

    public var kind: ControlV2Kind {
        switch self {
        case .hello: return .hello
        case .configure: return .configure
        case .attachResource: return .attachResource
        case .prepareDisk: return .prepareDisk
        case .start: return .start
        case .pause: return .pause
        case .resume: return .resume
        case .stop: return .stop
        case .shutdown: return .shutdown
        case .query: return .query
        case .setClipboard: return .setClipboard
        case .reservedStage9(let kind): return kind
        }
    }

    public func payload() -> [UInt8] {
        var bytes: [UInt8] = []
        switch self {
        case .hello, .query, .reservedStage9: break
        case .configure(let value): bytes = value.encoded()
        case .attachResource(let value): bytes = value.encoded()
        case .prepareDisk(let value): bytes = value.encoded()
        case .start(let id), .stop(let id), .shutdown(let id): bytes.putLE(id)
        case .pause(let reason), .resume(let reason):
            bytes.putLE(reason)
            bytes.putZeros(4)
        case .setClipboard(let value): bytes = value
        }
        return bytes
    }

    /// Structure of one request frame (section 11). A frame made by hand may have a payload of any length, so the length is
    /// checked against the kind first: the readers below never see a short payload.
    public static func parse(_ frame: ControlV2Frame) throws -> ControlV2Request {
        let payload = frame.payload
        guard frame.kind.acceptsPayloadLength(payload.count) else {
            throw ControlV2FatalError.badPayloadLength(kind: frame.kind.rawValue, length: UInt32(payload.count))
        }
        switch frame.kind {
        case .hello: return .hello
        case .configure: return .configure(try ControlV2Configure.parse(payload))
        case .attachResource: return .attachResource(try ControlV2Attach.parse(payload))
        case .prepareDisk: return .prepareDisk(try ControlV2PrepareDisk.parse(payload))
        case .start: return .start(operationID: payload.readLE(UInt64.self, at: 0))
        case .stop: return .stop(operationID: payload.readLE(UInt64.self, at: 0))
        case .shutdown: return .shutdown(operationID: payload.readLE(UInt64.self, at: 0))
        case .pause, .resume:
            guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("PAUSE/RESUME") }
            let reason = payload.readLE(UInt32.self, at: 0)
            return frame.kind == .pause ? .pause(reason: reason) : .resume(reason: reason)
        case .query: return .query
        case .setClipboard: return .setClipboard(payload)
        case .suspend, .save, .checkpoint, .restore: return .reservedStage9(frame.kind)
        default: throw ControlV2FatalError.wrongDirection(frame.kind.rawValue)
        }
    }
}

// MARK: responses and events

/// `ACK` (section 6.2).
public struct ControlV2Ack: Equatable {
    public static let codeOK: UInt32 = 0
    public static let codeInProgress: UInt32 = 1
    public static let flagReplayed: UInt16 = 1

    public var code: UInt32
    public var ackedKind: UInt16
    public var flags: UInt16
    public var state: UInt32

    public init(code: UInt32 = ControlV2Ack.codeOK, ackedKind: UInt16, flags: UInt16 = 0, state: UInt32) {
        self.code = code
        self.ackedKind = ackedKind
        self.flags = flags
        self.state = state
    }

    public var replayed: Bool { flags & Self.flagReplayed != 0 }
}

/// `STATE` (section 6.3).
public struct ControlV2StateReport: Equatable {
    public static let flagConfigured: UInt32 = 1
    public static let flagDisksNonEmpty: UInt32 = 2

    public var state: UInt32
    public var flags: UInt32
    public var lastReason: UInt32

    public init(state: UInt32, flags: UInt32, lastReason: UInt32) {
        self.state = state
        self.flags = flags
        self.lastReason = lastReason
    }
}

/// `STATS` (section 6.6).
public struct ControlV2Stats: Equatable {
    public var state: UInt32
    public var pollCount: UInt64
    public var framesPublished: UInt64
    public var rssBytes: UInt64

    public init(state: UInt32, pollCount: UInt64, framesPublished: UInt64, rssBytes: UInt64) {
        self.state = state
        self.pollCount = pollCount
        self.framesPublished = framesPublished
        self.rssBytes = rssBytes
    }
}

/// `ERROR` (section 6.7).
public struct ControlV2ErrorReport: Equatable {
    public static let flagReplayed: UInt16 = 1
    public static let flagFatal: UInt16 = 2

    /// Raw code: a receiver shows a code outside `ControlV2ErrorCode` as an unknown error (section 6.7).
    public var code: UInt32
    public var failedKind: UInt16
    public var flags: UInt16
    public var state: UInt32
    public var aux0: UInt64
    public var aux1: UInt64
    /// UTF-8 text for the log, at most 256 bytes; never a path outside the bundle.
    public var detail: [UInt8]

    public init(code: UInt32, failedKind: UInt16, flags: UInt16 = 0, state: UInt32, aux0: UInt64 = 0, aux1: UInt64 = 0,
                detail: [UInt8] = []) {
        self.code = code
        self.failedKind = failedKind
        self.flags = flags
        self.state = state
        self.aux0 = aux0
        self.aux1 = aux1
        self.detail = detail
    }

    public init(code: ControlV2ErrorCode, failedKind: UInt16, flags: UInt16 = 0, state: UInt32, aux0: UInt64 = 0,
                aux1: UInt64 = 0, detail: String = "") {
        self.init(code: code.rawValue, failedKind: failedKind, flags: flags, state: state, aux0: aux0, aux1: aux1,
                  detail: Self.truncatedDetail(detail))
    }

    public var replayed: Bool { flags & Self.flagReplayed != 0 }
    public var fatal: Bool { flags & Self.flagFatal != 0 }

    /// At most 256 bytes, cut on a character boundary (section 2).
    public static func truncatedDetail(_ text: String) -> [UInt8] {
        var bytes = Array(text.utf8)
        if bytes.count > ControlV2.maxErrorDetailLength {
            var end = ControlV2.maxErrorDetailLength
            while end > 0, bytes[end] & 0xC0 == 0x80 { end -= 1 }
            bytes = Array(bytes[..<end])
        }
        return bytes
    }
}

public enum ControlV2Response: Equatable {
    case ready(profile: UInt32)
    case ack(ControlV2Ack)
    case state(ControlV2StateReport)
    case stateChanged(from: UInt32, to: UInt32, reason: UInt32)
    case progress(requestKind: UInt32, completed: UInt64, total: UInt64)
    case stats(ControlV2Stats)
    /// Reserved until S11-24: the VMHost does not send it.
    case clipboardChanged([UInt8])
    case error(ControlV2ErrorReport)

    public var kind: ControlV2Kind {
        switch self {
        case .ready: return .ready
        case .ack: return .ack
        case .state: return .state
        case .stateChanged: return .stateChanged
        case .progress: return .progress
        case .stats: return .stats
        case .clipboardChanged: return .clipboardChanged
        case .error: return .error
        }
    }

    public func payload() -> [UInt8] {
        var bytes: [UInt8] = []
        switch self {
        case .ready(let profile):
            bytes.putLE(profile)
            bytes.putZeros(4)
        case .ack(let value):
            bytes.putLE(value.code)
            bytes.putLE(value.ackedKind)
            bytes.putLE(value.flags)
            bytes.putLE(value.state)
        case .state(let value):
            bytes.putLE(value.state)
            bytes.putLE(value.flags)
            bytes.putLE(value.lastReason)
        case .stateChanged(let from, let to, let reason):
            bytes.putLE(from)
            bytes.putLE(to)
            bytes.putLE(reason)
        case .progress(let requestKind, let completed, let total):
            bytes.putLE(requestKind)
            bytes.putZeros(4)
            bytes.putLE(completed)
            bytes.putLE(total)
        case .stats(let value):
            bytes.putLE(value.state)
            bytes.putZeros(4)
            bytes.putLE(value.pollCount)
            bytes.putLE(value.framesPublished)
            bytes.putLE(value.rssBytes)
        case .clipboardChanged(let value):
            bytes = value
        case .error(let value):
            let detail = Array(value.detail.prefix(ControlV2.maxErrorDetailLength))
            bytes.putLE(value.code)
            bytes.putLE(value.failedKind)
            bytes.putLE(value.flags)
            bytes.putLE(value.state)
            bytes.putLE(UInt16(detail.count))
            bytes.putZeros(2)
            bytes.putLE(value.aux0)
            bytes.putLE(value.aux1)
            bytes.append(contentsOf: detail)
        }
        return bytes
    }

    /// Structure of a frame sent by the VMHost, as the controlling process reads it (reserved bytes, unknown flag bits, lengths).
    public static func parse(_ frame: ControlV2Frame) throws -> ControlV2Response {
        let payload = frame.payload
        guard frame.kind.acceptsPayloadLength(payload.count) else {
            throw ControlV2FatalError.badPayloadLength(kind: frame.kind.rawValue, length: UInt32(payload.count))
        }
        switch frame.kind {
        case .ready:
            guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("READY") }
            let profile = payload.readLE(UInt32.self, at: 0)
            guard profile == ControlV2.readyProfile else { throw ControlV2FatalError.structure("READY profile \(profile)") }
            return .ready(profile: profile)
        case .ack:
            let flags = payload.readLE(UInt16.self, at: 6)
            guard flags & ~ControlV2Ack.flagReplayed == 0 else { throw ControlV2FatalError.structure("ACK flags") }
            let code = payload.readLE(UInt32.self, at: 0)
            guard code == ControlV2Ack.codeOK || code == ControlV2Ack.codeInProgress else {
                throw ControlV2FatalError.structure("ACK code \(code)")
            }
            return .ack(ControlV2Ack(code: code, ackedKind: payload.readLE(UInt16.self, at: 4),
                                     flags: flags, state: payload.readLE(UInt32.self, at: 8)))
        case .state:
            let flags = payload.readLE(UInt32.self, at: 4)
            guard flags & ~(ControlV2StateReport.flagConfigured | ControlV2StateReport.flagDisksNonEmpty) == 0 else {
                throw ControlV2FatalError.structure("STATE flags")
            }
            return .state(ControlV2StateReport(state: payload.readLE(UInt32.self, at: 0), flags: flags,
                                               lastReason: payload.readLE(UInt32.self, at: 8)))
        case .stateChanged:
            return .stateChanged(from: payload.readLE(UInt32.self, at: 0), to: payload.readLE(UInt32.self, at: 4),
                                 reason: payload.readLE(UInt32.self, at: 8))
        case .progress:
            guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("PROGRESS") }
            return .progress(requestKind: payload.readLE(UInt32.self, at: 0), completed: payload.readLE(UInt64.self, at: 8),
                             total: payload.readLE(UInt64.self, at: 16))
        case .stats:
            guard payload.isZero(4..<8) else { throw ControlV2FatalError.nonZeroReserved("STATS") }
            return .stats(ControlV2Stats(state: payload.readLE(UInt32.self, at: 0), pollCount: payload.readLE(UInt64.self, at: 8),
                                         framesPublished: payload.readLE(UInt64.self, at: 16),
                                         rssBytes: payload.readLE(UInt64.self, at: 24)))
        case .clipboardChanged:
            throw ControlV2FatalError.structure("CLIPBOARD_CHANGED before S11-24")
        case .error:
            let flags = payload.readLE(UInt16.self, at: 6)
            guard flags & ~(ControlV2ErrorReport.flagReplayed | ControlV2ErrorReport.flagFatal) == 0 else {
                throw ControlV2FatalError.structure("ERROR flags")
            }
            let detailLength = Int(payload.readLE(UInt16.self, at: 12))
            guard detailLength <= ControlV2.maxErrorDetailLength, payload.count == 32 + detailLength else {
                throw ControlV2FatalError.badPayloadLength(kind: frame.kind.rawValue, length: UInt32(payload.count))
            }
            guard payload.isZero(14..<16) else { throw ControlV2FatalError.nonZeroReserved("ERROR") }
            return .error(ControlV2ErrorReport(
                code: payload.readLE(UInt32.self, at: 0), failedKind: payload.readLE(UInt16.self, at: 4), flags: flags,
                state: payload.readLE(UInt32.self, at: 8), aux0: payload.readLE(UInt64.self, at: 16),
                aux1: payload.readLE(UInt64.self, at: 24), detail: Array(payload[32...])))
        default:
            throw ControlV2FatalError.wrongDirection(frame.kind.rawValue)
        }
    }
}
