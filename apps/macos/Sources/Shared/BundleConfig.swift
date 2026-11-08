import Darwin
import Foundation

// `config.plist` schema v1 of a VM bundle: docs/specs/VM_BUNDLE.md sections 2, 3, 5.2 (S11-06a).
//
// Only Swift (Foundation) reads this file; the VMHost gets already checked values through `CONFIGURE`
// (`BundleConfig.controlConfigure()`). Section numbers in the comments refer to VM_BUNDLE.md.
//
// Reading follows 3.1 in order and stops at the first violation: size, plist, `schema_version`, closed enumerations
// (an unknown value is `incompatible` before any other field is judged), then the field table and the cross-field
// rules. Unknown keys are kept in memory and written back unchanged. A file of class `incompatible` or `invalid` is never
// rewritten. Writing is an atomic replacement through `.config.plist.tmp-<suffix>` in the bundle directory under the
// bundle lock (5.2, 2.1). Durability of the replacement is NOT claimed (section 6, ISSUE-055): there is no fsync here.

// MARK: limits

public enum BundleConfigLimits {
    public static let maxFileBytes = 1 << 20
    public static let minMemoryMiB = 256
    public static let maxMemoryMiB = 65_536
    public static let defaultMemoryMiB = 4_096
    public static let maxVcpuCount = 32
    public static let maxDisks = 4
    public static let maxSharedFolders = 8
    public static let maxBookmarkBytes = 65_536
    public static let maxNameBytes = 128
    public static let maxGuestNameBytes = 64
    public static let maxDiskPathBytes = 255
    public static let diskPathPrefix = "disks/"
    public static let fileName = "config.plist"
    public static let temporaryPrefix = ".config.plist.tmp-"
}

// MARK: closed enumerations of v1

public enum MachineProfile: String, CaseIterable {
    case displayTestPattern = "display-test-pattern"
    case displayTwoScreensTest = "display-two-screens-test"

    /// Profile number of the display ABI and of `CONFIGURE.machine_profile` (CONTROL_PROTOCOL_V2.md 6.1).
    public var controlValue: UInt32 {
        switch self {
        case .displayTestPattern: return 1
        case .displayTwoScreensTest: return 4
        }
    }

    /// `display.screens` that goes with the profile (3.2).
    public var screens: Int {
        switch self {
        case .displayTestPattern: return 1
        case .displayTwoScreensTest: return 2
        }
    }
}

public enum NetworkMode: String, CaseIterable {
    case none
    case nat
}

public enum DiskFormat: String, CaseIterable {
    case raw
}

// MARK: unknown keys

/// Keys that this reader does not know. They are not an error; they are written back with the value they had (3.1).
public struct UnknownKeys: Equatable {
    public var values: [String: Any]

    public init(_ values: [String: Any] = [:]) { self.values = values }

    public var isEmpty: Bool { values.isEmpty }

    /// Type-aware: `NSNumber` booleans and integers are different plist types (`NSDictionary.isEqual` takes `@YES == @1`).
    public static func == (lhs: UnknownKeys, rhs: UnknownKeys) -> Bool {
        plistEqual(lhs.values, rhs.values)
    }

    static func plistEqual(_ left: Any, _ right: Any) -> Bool {
        let a = left as CFTypeRef
        let b = right as CFTypeRef
        let type = CFGetTypeID(a)
        guard type == CFGetTypeID(b) else { return false }
        if type == CFDictionaryGetTypeID() {
            guard let x = left as? [String: Any], let y = right as? [String: Any], x.count == y.count else { return false }
            for (key, value) in x {
                guard let other = y[key], plistEqual(value, other) else { return false }
            }
            return true
        }
        if type == CFArrayGetTypeID() {
            guard let x = left as? [Any], let y = right as? [Any], x.count == y.count else { return false }
            return zip(x, y).allSatisfy { plistEqual($0, $1) }
        }
        if type == CFNumberGetTypeID() {
            let x = a as! CFNumber
            let y = b as! CFNumber
            guard CFNumberIsFloatType(x) == CFNumberIsFloatType(y) else { return false }
        }
        return (left as AnyObject).isEqual(right)
    }
}

// MARK: model

public struct BundleDisk: Equatable {
    public enum Location: Equatable {
        /// Image inside the bundle: `path` is `disks/<name>`; only `<name>` is kept (it is what control v2 carries).
        case inBundle(name: String)
        /// Image outside the bundle: bookmark data (section 4).
        case external(bookmark: Data)
    }

    public var location: Location
    public var format: DiskFormat
    public var writable: Bool
    public var unknownKeys: UnknownKeys

    public init(location: Location, format: DiskFormat = .raw, writable: Bool = true, unknownKeys: UnknownKeys = UnknownKeys()) {
        self.location = location
        self.format = format
        self.writable = writable
        self.unknownKeys = unknownKeys
    }
}

public struct BundleSharedFolder: Equatable {
    public var bookmark: Data
    public var readOnly: Bool
    public var guestName: String
    public var unknownKeys: UnknownKeys

    public init(bookmark: Data, readOnly: Bool = true, guestName: String, unknownKeys: UnknownKeys = UnknownKeys()) {
        self.bookmark = bookmark
        self.readOnly = readOnly
        self.guestName = guestName
        self.unknownKeys = unknownKeys
    }
}

public struct BundleConfig: Equatable {
    /// Canonical lower-case UUID (3.2). Created once and never changed.
    public var identity: String
    public var name: String
    public var machineProfile: MachineProfile
    public var memoryMiB: Int
    public var vcpuCount: Int
    public var disks: [BundleDisk]
    /// Bookmark data of the ISO (always `data`, never a path string).
    public var bootMedia: Data?
    public var displayScreens: Int
    public var displayUnknownKeys: UnknownKeys
    public var network: NetworkMode
    public var audio: Bool
    public var sharedFolders: [BundleSharedFolder]
    public var clipboardSync: Bool
    public var unknownKeys: UnknownKeys

    /// A new machine: defaults of 3.2, a fresh identity (lower case: `UUID().uuidString` is upper case and would be `invalid`).
    public init(name: String, machineProfile: MachineProfile = .displayTestPattern, identity: String = UUID().uuidString.lowercased()) {
        self.identity = identity
        self.name = name
        self.machineProfile = machineProfile
        self.memoryMiB = BundleConfigLimits.defaultMemoryMiB
        self.vcpuCount = 1
        self.disks = []
        self.bootMedia = nil
        self.displayScreens = machineProfile.screens
        self.displayUnknownKeys = UnknownKeys()
        self.network = .none
        self.audio = false
        self.sharedFolders = []
        self.clipboardSync = false
        self.unknownKeys = UnknownKeys()
    }

    /// The fixed fields of `CONFIGURE` (CONTROL_PROTOCOL_V2.md 6.1; VM_BUNDLE 7 item 2). Images inside the bundle go by name only.
    public func controlConfigure() -> ControlV2Configure {
        let controlDisks: [ControlV2Configure.Disk] = disks.map { disk in
            switch disk.location {
            case .inBundle(let name):
                return .init(source: 1, format: 1, writable: disk.writable ? 1 : 0, name: Array(name.utf8))
            case .external:
                return .init(source: 2, format: 1, writable: disk.writable ? 1 : 0, name: [])
            }
        }
        return ControlV2Configure(
            machineProfile: machineProfile.controlValue, memoryMiB: UInt32(memoryMiB), vcpuCount: UInt16(vcpuCount),
            displayScreens: UInt16(displayScreens), network: network == .nat ? 1 : 0, audio: audio ? 1 : 0,
            clipboardSync: clipboardSync ? 1 : 0, bootMedia: bootMedia == nil ? 0 : 1, disks: controlDisks,
            sharedFolders: sharedFolders.map { .init(readOnly: $0.readOnly ? 1 : 0, guestName: Array($0.guestName.utf8)) })
    }
}

// MARK: launch checks (3.4 item 4, 3.5 items 2-4; they are not part of the validity of the file)

public enum LaunchRefusal: Equatable {
    /// A non-empty `disks` before S9-06 (ISSUE-055).
    case unsupportedDurability
    /// The configuration needs a device the profile does not have; `field` is the key of `config.plist`.
    case unsupported(field: String)
    /// `memory_mib` above half of the physical memory of the host (a hypothesis until S11-14).
    case hostMemoryExceeded(requestedMiB: Int, limitMiB: Int)
}

extension BundleConfig {
    /// Checks of 3.5 in their order: `unsupported_durability`, `unsupported`, memory. The lock, bookmark and image checks need the
    /// file system and belong to the caller. `nil` means that none of these three refuses.
    public func launchRefusal(hostMemoryBytes: UInt64 = ProcessInfo.processInfo.physicalMemory) -> LaunchRefusal? {
        if !disks.isEmpty { return .unsupportedDurability }
        if network == .nat { return .unsupported(field: "network") }
        if audio { return .unsupported(field: "audio") }
        if bootMedia != nil { return .unsupported(field: "boot_media") }
        if !sharedFolders.isEmpty { return .unsupported(field: "shared_folders") }
        if clipboardSync { return .unsupported(field: "clipboard_sync") }
        let limitMiB = Int(hostMemoryBytes / UInt64(1 << 20) / 2)
        if memoryMiB > limitMiB { return .hostMemoryExceeded(requestedMiB: memoryMiB, limitMiB: limitMiB) }
        return nil
    }
}

// MARK: classes (3.1)

public struct BundleConfigIssue: Error, Equatable, CustomStringConvertible {
    /// Key path in the notation `disks[1].path`.
    public var field: String
    public var rule: String

    public init(field: String, rule: String) {
        self.field = field
        self.rule = rule
    }

    public var description: String { "\(field): \(rule)" }

    /// No `config.plist`: an incomplete bundle, shown as damaged, never started and never deleted automatically (section 2).
    public static let fileMissing = BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "file is missing")
}

public enum BundleConfigClass: Equatable {
    case valid
    case incompatible
    case invalid
}

public enum BundleConfigState: Equatable {
    case valid(BundleConfig)
    /// `schema_version` is newer, or a closed enumeration holds an unknown value. The file is not rewritten.
    case incompatible(BundleConfigIssue)
    /// The file is damaged. The file is not rewritten.
    case invalid(BundleConfigIssue)

    public var kind: BundleConfigClass {
        switch self {
        case .valid: return .valid
        case .incompatible: return .incompatible
        case .invalid: return .invalid
        }
    }

    public var config: BundleConfig? {
        if case .valid(let config) = self { return config }
        return nil
    }

    public var issue: BundleConfigIssue? {
        switch self {
        case .valid: return nil
        case .incompatible(let issue), .invalid(let issue): return issue
        }
    }
}

// MARK: plist types

/// Plist types are exact (3.1): an integer is only `integer` (not a real, not a boolean), a boolean only `true`/`false`.
enum PlistType {
    static func integer(_ value: Any) -> Int? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFNumberGetTypeID() else { return nil }
        let number = object as! CFNumber
        guard !CFNumberIsFloatType(number) else { return nil }
        var result: Int64 = 0
        guard CFNumberGetValue(number, .sInt64Type, &result) else { return nil }
        return Int(exactly: result)
    }

    static func bool(_ value: Any) -> Bool? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFBooleanGetTypeID() else { return nil }
        return CFBooleanGetValue((object as! CFBoolean))
    }

    static func string(_ value: Any) -> String? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFStringGetTypeID() else { return nil }
        return value as? String
    }

    static func data(_ value: Any) -> Data? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFDataGetTypeID() else { return nil }
        return value as? Data
    }

    static func array(_ value: Any) -> [Any]? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFArrayGetTypeID() else { return nil }
        return value as? [Any]
    }

    static func dictionary(_ value: Any) -> [String: Any]? {
        let object = value as CFTypeRef
        guard CFGetTypeID(object) == CFDictionaryGetTypeID() else { return nil }
        return value as? [String: Any]
    }
}

enum BundleText {
    /// U+0000-U+001F and U+007F.
    static func hasControlCharacter(_ text: String) -> Bool {
        text.unicodeScalars.contains { $0.value < 0x20 || $0.value == 0x7F }
    }

    /// 36 characters, lower-case hex digits and hyphens as 8-4-4-4-12 (3.2).
    static func isCanonicalUUID(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard bytes.count == 36 else { return false }
        for (index, byte) in bytes.enumerated() {
            if [8, 13, 18, 23].contains(index) {
                if byte != 0x2D { return false }
            } else if !((0x30...0x39).contains(byte) || (0x61...0x66).contains(byte)) {
                return false
            }
        }
        return true
    }
}

// MARK: reading

public enum BundleConfigCodec {
    static let knownTopLevelKeys: Set<String> = [
        "schema_version", "identity", "name", "machine_profile", "memory_mib", "vcpu_count", "disks", "boot_media", "display",
        "network", "audio", "shared_folders", "clipboard_sync",
    ]

    /// Reads `<bundle>/config.plist`. Never throws and never writes: every failure is a class (3.1).
    public static func read(bundle: URL) -> BundleConfigState {
        let path = bundle.appendingPathComponent(BundleConfigLimits.fileName).path
        let fd = Darwin.open(path, O_RDONLY | O_CLOEXEC)
        guard fd >= 0 else {
            if errno == ENOENT { return .invalid(.fileMissing) }
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "cannot be opened (errno \(errno))"))
        }
        defer { Darwin.close(fd) }
        var info = stat()
        guard fstat(fd, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG else {
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "is not a regular file"))
        }
        guard info.st_size <= off_t(BundleConfigLimits.maxFileBytes) else {
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "is larger than 1 MiB"))
        }
        var bytes = [UInt8]()
        var chunk = [UInt8](repeating: 0, count: 65_536)
        while bytes.count <= BundleConfigLimits.maxFileBytes {
            let count = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
            if count < 0 {
                if errno == EINTR { continue }
                return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "cannot be read (errno \(errno))"))
            }
            if count == 0 { break }
            bytes.append(contentsOf: chunk[..<count])
        }
        return parse(data: Data(bytes))
    }

    /// Steps 1-5 of 3.1 on the bytes of a file.
    public static func parse(data: Data) -> BundleConfigState {
        guard data.count <= BundleConfigLimits.maxFileBytes else {
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "is larger than 1 MiB"))
        }
        let object: Any
        do {
            object = try PropertyListSerialization.propertyList(from: data, options: [], format: nil)
        } catch {
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "is not a property list"))
        }
        guard let root = PlistType.dictionary(object) else {
            return .invalid(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "root is not a dict"))
        }
        return parse(root: root)
    }

    static func parse(root: [String: Any]) -> BundleConfigState {
        func invalid(_ field: String, _ rule: String) -> BundleConfigState { .invalid(BundleConfigIssue(field: field, rule: rule)) }
        func incompatible(_ field: String, _ rule: String) -> BundleConfigState {
            .incompatible(BundleConfigIssue(field: field, rule: rule))
        }

        // 3: schema_version.
        guard let versionValue = root["schema_version"] else { return invalid("schema_version", "is missing") }
        guard let version = PlistType.integer(versionValue) else { return invalid("schema_version", "is not an integer") }
        if version < 1 { return invalid("schema_version", "is below 1") }
        if version > 1 { return incompatible("schema_version", "is newer than 1") }

        // 4: closed enumerations. An unknown string value is `incompatible` and stops the read.
        if let value = root["machine_profile"], let text = PlistType.string(value), MachineProfile(rawValue: text) == nil {
            return incompatible("machine_profile", "unknown value")
        }
        if let value = root["network"], let text = PlistType.string(value), NetworkMode(rawValue: text) == nil {
            return incompatible("network", "unknown value")
        }
        if let value = root["disks"], let elements = PlistType.array(value) {
            for (index, element) in elements.enumerated() {
                if let dictionary = PlistType.dictionary(element), let format = dictionary["format"],
                   let text = PlistType.string(format), DiskFormat(rawValue: text) == nil {
                    return incompatible("disks[\(index)].format", "unknown value")
                }
            }
        }

        // 5: the field table in its order.
        guard let identityValue = root["identity"] else { return invalid("identity", "is missing") }
        guard let identity = PlistType.string(identityValue), BundleText.isCanonicalUUID(identity) else {
            return invalid("identity", "is not a lower-case canonical UUID")
        }

        guard let nameValue = root["name"] else { return invalid("name", "is missing") }
        guard let name = PlistType.string(nameValue) else { return invalid("name", "is not a string") }
        guard (1...BundleConfigLimits.maxNameBytes).contains(name.utf8.count) else { return invalid("name", "is not 1 to 128 bytes") }
        guard !BundleText.hasControlCharacter(name) else { return invalid("name", "has a control character") }
        guard !name.trimmingCharacters(in: .whitespaces).isEmpty else { return invalid("name", "consists of spaces only") }

        guard let profileValue = root["machine_profile"] else { return invalid("machine_profile", "is missing") }
        guard let profileText = PlistType.string(profileValue), let profile = MachineProfile(rawValue: profileText) else {
            return invalid("machine_profile", "is not a string")
        }

        var memoryMiB = BundleConfigLimits.defaultMemoryMiB
        if let value = root["memory_mib"] {
            guard let number = PlistType.integer(value), (BundleConfigLimits.minMemoryMiB...BundleConfigLimits.maxMemoryMiB).contains(number) else {
                return invalid("memory_mib", "is not an integer of 256 to 65536")
            }
            memoryMiB = number
        }

        var vcpuCount = 1
        if let value = root["vcpu_count"] {
            guard let number = PlistType.integer(value), (1...BundleConfigLimits.maxVcpuCount).contains(number) else {
                return invalid("vcpu_count", "is not an integer of 1 to 32")
            }
            vcpuCount = number
        }

        var disks: [BundleDisk] = []
        if let value = root["disks"] {
            guard let elements = PlistType.array(value) else { return invalid("disks", "is not an array") }
            guard elements.count <= BundleConfigLimits.maxDisks else { return invalid("disks", "has more than 4 elements") }
            for (index, element) in elements.enumerated() {
                switch parseDisk(element, field: "disks[\(index)]") {
                case .success(let disk): disks.append(disk)
                case .failure(let issue): return .invalid(issue)
                }
            }
        }

        var bootMedia: Data?
        if let value = root["boot_media"] {
            guard let data = PlistType.data(value), (1...BundleConfigLimits.maxBookmarkBytes).contains(data.count) else {
                return invalid("boot_media", "is not data of 1 to 65536 bytes")
            }
            bootMedia = data
        }

        var screens: Int?
        var displayUnknown: [String: Any] = [:]
        if let value = root["display"] {
            guard let dictionary = PlistType.dictionary(value) else { return invalid("display", "is not a dict") }
            if let screensValue = dictionary["screens"] {
                guard let number = PlistType.integer(screensValue), (1...2).contains(number) else {
                    return invalid("display.screens", "is not an integer of 1 to 2")
                }
                screens = number
            }
            displayUnknown = dictionary.filter { $0.key != "screens" }
        }

        var network = NetworkMode.none
        if let value = root["network"] {
            guard let text = PlistType.string(value), let mode = NetworkMode(rawValue: text) else { return invalid("network", "is not a string") }
            network = mode
        }

        var audio = false
        if let value = root["audio"] {
            guard let flag = PlistType.bool(value) else { return invalid("audio", "is not a boolean") }
            audio = flag
        }

        var sharedFolders: [BundleSharedFolder] = []
        if let value = root["shared_folders"] {
            guard let elements = PlistType.array(value) else { return invalid("shared_folders", "is not an array") }
            guard elements.count <= BundleConfigLimits.maxSharedFolders else { return invalid("shared_folders", "has more than 8 elements") }
            var seenGuestNames = Set<[UInt8]>()
            for (index, element) in elements.enumerated() {
                switch parseSharedFolder(element, field: "shared_folders[\(index)]") {
                case .success(let folder):
                    // 3.3: `guest_name` is unique among the elements, compared byte by byte.
                    guard seenGuestNames.insert(Array(folder.guestName.utf8)).inserted else {
                        return invalid("shared_folders[\(index)].guest_name", "duplicates an earlier shared folder")
                    }
                    sharedFolders.append(folder)
                case .failure(let issue): return .invalid(issue)
                }
            }
        }

        var clipboardSync = false
        if let value = root["clipboard_sync"] {
            guard let flag = PlistType.bool(value) else { return invalid("clipboard_sync", "is not a boolean") }
            clipboardSync = flag
        }

        // 3.4: cross-field rules.
        guard vcpuCount == 1 else { return invalid("vcpu_count", "must be 1 for a display test profile") }
        if let screens, screens != profile.screens { return invalid("display.screens", "does not match machine_profile") }
        // The in-bundle names are compared byte by byte (canonically equivalent spellings are different names on disk).
        var seenNames = Set<[UInt8]>()
        var seenBookmarks = Set<Data>()
        for (index, disk) in disks.enumerated() {
            switch disk.location {
            case .inBundle(let diskName):
                guard seenNames.insert(Array(diskName.utf8)).inserted else { return invalid("disks[\(index)].path", "duplicates an earlier disk") }
            case .external(let bookmark):
                guard seenBookmarks.insert(bookmark).inserted else { return invalid("disks[\(index)].bookmark", "duplicates an earlier disk") }
            }
        }

        var config = BundleConfig(name: name, machineProfile: profile, identity: identity)
        config.memoryMiB = memoryMiB
        config.vcpuCount = vcpuCount
        config.disks = disks
        config.bootMedia = bootMedia
        config.displayScreens = screens ?? profile.screens
        config.displayUnknownKeys = UnknownKeys(displayUnknown)
        config.network = network
        config.audio = audio
        config.sharedFolders = sharedFolders
        config.clipboardSync = clipboardSync
        config.unknownKeys = UnknownKeys(root.filter { !knownTopLevelKeys.contains($0.key) })
        return .valid(config)
    }

    private static func parseDisk(_ element: Any, field: String) -> Result<BundleDisk, BundleConfigIssue> {
        func fail(_ key: String, _ rule: String) -> Result<BundleDisk, BundleConfigIssue> {
            .failure(BundleConfigIssue(field: key.isEmpty ? field : "\(field).\(key)", rule: rule))
        }
        guard let dictionary = PlistType.dictionary(element) else { return fail("", "is not a dict") }
        let location: BundleDisk.Location
        switch (dictionary["path"], dictionary["bookmark"]) {
        case (.some(let path), .none):
            guard let text = PlistType.string(path) else { return fail("path", "is not a string") }
            guard let name = diskName(inPath: text) else { return fail("path", "is not disks/<name> (one component, 1 to 249 bytes)") }
            location = .inBundle(name: name)
        case (.none, .some(let bookmark)):
            guard let data = PlistType.data(bookmark), (1...BundleConfigLimits.maxBookmarkBytes).contains(data.count) else {
                return fail("bookmark", "is not data of 1 to 65536 bytes")
            }
            location = .external(bookmark: data)
        default:
            return fail("", "must have exactly one of path and bookmark")
        }
        guard let formatValue = dictionary["format"] else { return fail("format", "is missing") }
        guard let formatText = PlistType.string(formatValue), let format = DiskFormat(rawValue: formatText) else {
            return fail("format", "is not a string")
        }
        var writable = true
        if let value = dictionary["writable"] {
            guard let flag = PlistType.bool(value) else { return fail("writable", "is not a boolean") }
            writable = flag
        }
        let unknown = dictionary.filter { !["path", "bookmark", "format", "writable"].contains($0.key) }
        return .success(BundleDisk(location: location, format: format, writable: writable, unknownKeys: UnknownKeys(unknown)))
    }

    /// `disks/<name>` with exactly one component after the prefix (3.3); the whole value is at most 255 bytes.
    static func diskName(inPath path: String) -> String? {
        let prefix = BundleConfigLimits.diskPathPrefix
        guard path.utf8.count <= BundleConfigLimits.maxDiskPathBytes, path.hasPrefix(prefix) else { return nil }
        let name = String(path.dropFirst(prefix.count))
        guard ControlV2Names.isValidDiskName(Array(name.utf8)) else { return nil }
        return name
    }

    private static func parseSharedFolder(_ element: Any, field: String) -> Result<BundleSharedFolder, BundleConfigIssue> {
        func fail(_ key: String, _ rule: String) -> Result<BundleSharedFolder, BundleConfigIssue> {
            .failure(BundleConfigIssue(field: key.isEmpty ? field : "\(field).\(key)", rule: rule))
        }
        guard let dictionary = PlistType.dictionary(element) else { return fail("", "is not a dict") }
        guard let bookmarkValue = dictionary["bookmark"] else { return fail("bookmark", "is missing") }
        guard let bookmark = PlistType.data(bookmarkValue), (1...BundleConfigLimits.maxBookmarkBytes).contains(bookmark.count) else {
            return fail("bookmark", "is not data of 1 to 65536 bytes")
        }
        var readOnly = true
        if let value = dictionary["read_only"] {
            guard let flag = PlistType.bool(value) else { return fail("read_only", "is not a boolean") }
            readOnly = flag
        }
        guard let guestValue = dictionary["guest_name"] else { return fail("guest_name", "is missing") }
        guard let guestName = PlistType.string(guestValue), ControlV2Names.isValidGuestName(Array(guestName.utf8)) else {
            return fail("guest_name", "is not 1 to 64 bytes without / \\ : and control characters")
        }
        let unknown = dictionary.filter { !["bookmark", "read_only", "guest_name"].contains($0.key) }
        return .success(BundleSharedFolder(bookmark: bookmark, readOnly: readOnly, guestName: guestName, unknownKeys: UnknownKeys(unknown)))
    }

    // MARK: encoding

    /// The plist dictionary of a config: unknown keys unchanged, defaults written explicitly (5.2), `schema_version` 1.
    public static func plistDictionary(for config: BundleConfig) -> [String: Any] {
        var root = config.unknownKeys.values
        root["schema_version"] = 1
        root["identity"] = config.identity
        root["name"] = config.name
        root["machine_profile"] = config.machineProfile.rawValue
        root["memory_mib"] = config.memoryMiB
        root["vcpu_count"] = config.vcpuCount
        root["disks"] = config.disks.map { disk -> [String: Any] in
            var element = disk.unknownKeys.values
            switch disk.location {
            case .inBundle(let name): element["path"] = BundleConfigLimits.diskPathPrefix + name
            case .external(let bookmark): element["bookmark"] = bookmark
            }
            element["format"] = disk.format.rawValue
            element["writable"] = disk.writable
            return element
        }
        if let bootMedia = config.bootMedia { root["boot_media"] = bootMedia }
        var display = config.displayUnknownKeys.values
        display["screens"] = config.displayScreens
        root["display"] = display
        root["network"] = config.network.rawValue
        root["audio"] = config.audio
        root["shared_folders"] = config.sharedFolders.map { folder -> [String: Any] in
            var element = folder.unknownKeys.values
            element["bookmark"] = folder.bookmark
            element["read_only"] = folder.readOnly
            element["guest_name"] = folder.guestName
            return element
        }
        root["clipboard_sync"] = config.clipboardSync
        return root
    }

    /// XML plist of a config. The result is read back through the same reader: a config that the reader would not accept as `valid`
    /// is not encoded (`BundleConfigWriteError.invalidConfig`).
    public static func encode(_ config: BundleConfig) throws -> Data {
        let root = plistDictionary(for: config)
        let data: Data
        do {
            data = try PropertyListSerialization.data(fromPropertyList: root, format: .xml, options: 0)
        } catch {
            throw BundleConfigWriteError.invalidConfig(BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "cannot be serialized"))
        }
        let state = parse(data: data)
        guard case .valid = state else {
            throw BundleConfigWriteError.invalidConfig(state.issue ?? BundleConfigIssue(field: BundleConfigLimits.fileName, rule: "is not valid"))
        }
        return data
    }
}

// MARK: bundle lock (2.1), the part that the controlling process needs

public enum BundleWriterLockFailure: Error, Equatable, CustomStringConvertible {
    /// `flock` returned EWOULDBLOCK: the VMHost, another writer or a probe holds the lock. A false `busy` is possible (2.1).
    case busy
    /// `lstat` after `flock` shows another file or none.
    case lockChanged
    /// The object `lock` is not a regular file.
    case invalid
    case io(Int32)

    public var description: String {
        switch self {
        case .busy: return "bundle lock is held by another process"
        case .lockChanged: return "bundle lock file was replaced or removed"
        case .invalid: return "bundle lock is not a regular file"
        case .io(let code): return "bundle lock I/O error \(code)"
        }
    }
}

/// `<bundle>/lock` taken by the process that writes `config.plist` (VM_BUNDLE 2.1: `open` with `O_CREAT | O_EXCL`, on `EEXIST` without
/// them, `fstat` regular file, `flock(LOCK_EX | LOCK_NB)`, `lstat` same identity; `ENOENT` at step 2 repeats the order once). The
/// descriptor is `O_CLOEXEC`. The creator of a bundle must release it before it starts a VMHost (2.1).
public final class BundleWriterLock {
    private var fd: Int32

    private init(fd: Int32) { self.fd = fd }

    deinit { release() }

    public func release() {
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
    }

    public static func acquire(bundle: URL) throws -> BundleWriterLock {
        let path = bundle.appendingPathComponent("lock").path
        var retried = false
        while true {
            var fd = Darwin.open(path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0o600)
            if fd < 0 {
                guard errno == EEXIST else { throw BundleWriterLockFailure.io(errno) }
                fd = Darwin.open(path, O_RDWR | O_CLOEXEC | O_NOFOLLOW)
                if fd < 0 {
                    let code = errno
                    if code == ELOOP || code == EISDIR { throw BundleWriterLockFailure.invalid }
                    if code == ENOENT {
                        if retried { throw BundleWriterLockFailure.lockChanged }
                        retried = true
                        continue
                    }
                    throw BundleWriterLockFailure.io(code)
                }
            }
            do {
                try verifyAndLock(fd: fd, path: path, operation: LOCK_EX | LOCK_NB)
            } catch {
                Darwin.close(fd)
                throw error
            }
            return BundleWriterLock(fd: fd)
        }
    }

    /// The probe of 2.1 before a start: nothing is created or written. `nil` means free (also when there is no `lock` file).
    /// The result is a snapshot, not a lock; the VMHost's own `flock` arbitrates.
    public static func probeFailure(bundle: URL) -> BundleWriterLockFailure? {
        let path = bundle.appendingPathComponent("lock").path
        let fd = Darwin.open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW)
        if fd < 0 {
            let code = errno
            if code == ENOENT { return nil }
            if code == ELOOP || code == EISDIR { return .invalid }
            return .io(code)
        }
        defer { Darwin.close(fd) }
        do {
            try verifyAndLock(fd: fd, path: path, operation: LOCK_SH | LOCK_NB)
        } catch {
            return error as? BundleWriterLockFailure ?? .io(0)
        }
        _ = flock(fd, LOCK_UN) as Int32
        return nil
    }

    private static func verifyAndLock(fd: Int32, path: String, operation: Int32) throws {
        var opened = stat()
        guard fstat(fd, &opened) == 0 else { throw BundleWriterLockFailure.io(errno) }
        guard (opened.st_mode & S_IFMT) == S_IFREG else { throw BundleWriterLockFailure.invalid }
        while true {
            // `flock` is also a struct name in Darwin: the Int32 type of the result selects the function.
            let result: Int32 = flock(fd, operation)
            if result == 0 { break }
            if errno == EINTR { continue }
            if errno == EWOULDBLOCK { throw BundleWriterLockFailure.busy }
            throw BundleWriterLockFailure.io(errno)
        }
        var named = stat()
        guard lstat(path, &named) == 0 else {
            if errno == ENOENT { throw BundleWriterLockFailure.lockChanged }
            throw BundleWriterLockFailure.io(errno)
        }
        guard named.st_dev == opened.st_dev, named.st_ino == opened.st_ino else { throw BundleWriterLockFailure.lockChanged }
    }
}

// MARK: atomic replacement (5.2, 8 item 2)

/// Writes a file completely under a temporary name in the SAME directory (created `O_CREAT | O_EXCL | O_NOFOLLOW`, mode 0600),
/// then `rename(2)` over the target. Foundation's atomic write is not used: its temporary file has its own name and may lie in
/// another directory (5.2). There is no `fsync`: durability is not claimed (section 6, ISSUE-055).
enum AtomicReplace {
    enum Fault: Equatable {
        /// Simulated stop after the temporary file is written and before `rename(2)`: the target stays as it was.
        case beforeRename
    }

    /// Test hook. nil in the product.
    static var fault: Fault?

    static func replace(directory: URL, targetName: String, temporaryName: String, data: Data) throws {
        let temporaryPath = directory.appendingPathComponent(temporaryName).path
        let targetPath = directory.appendingPathComponent(targetName).path
        let fd = Darwin.open(temporaryPath, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw AtomicReplaceError.io(errno) }
        var created = true
        defer { if created { _ = Darwin.unlink(temporaryPath) } }
        var offset = 0
        while offset < data.count {
            let written = data.withUnsafeBytes { Darwin.write(fd, $0.baseAddress! + offset, data.count - offset) }
            if written < 0 {
                if errno == EINTR { continue }
                let code = errno
                Darwin.close(fd)
                throw AtomicReplaceError.io(code)
            }
            offset += written
        }
        guard Darwin.close(fd) == 0 else { throw AtomicReplaceError.io(errno) }
        if fault == .beforeRename { throw AtomicReplaceError.io(EIO) }
        guard Darwin.rename(temporaryPath, targetPath) == 0 else { throw AtomicReplaceError.io(errno) }
        created = false
    }

    /// 8 hex digits: enough to keep concurrent temporary names apart; not a secret.
    static func randomSuffix() -> String {
        String(format: "%08x", UInt32.random(in: 0...UInt32.max))
    }
}

enum AtomicReplaceError: Error, Equatable {
    case io(Int32)
}

// MARK: writing config.plist

public enum BundleConfigWriteMode: Equatable {
    /// An existing machine: the current file must be `valid`.
    case update
    /// A new bundle (5.1 step 4): `config.plist` does not exist yet.
    case create
}

public enum BundleConfigWriteError: Error, Equatable, CustomStringConvertible {
    /// The config would not be `valid` when read back; nothing is written.
    case invalidConfig(BundleConfigIssue)
    /// The current file is `incompatible` or `invalid`: it is not rewritten (3.1, 5.2).
    case currentFileNotValid(BundleConfigClass, BundleConfigIssue)
    /// `.create` and a `config.plist` exists.
    case alreadyExists
    case lock(BundleWriterLockFailure)
    case io(Int32)

    public var description: String {
        switch self {
        case .invalidConfig(let issue): return "config is not valid: \(issue)"
        case .currentFileNotValid(let kind, let issue): return "current config.plist is \(kind): \(issue); it is not rewritten"
        case .alreadyExists: return "config.plist already exists"
        case .lock(let failure): return failure.description
        case .io(let code): return "I/O error \(code)"
        }
    }
}

public enum BundleConfigStore {
    /// Writes `config` as `<bundle>/config.plist` under the bundle lock: reads the current file (class check), removes stale
    /// `.config.plist.tmp-*` files of this pattern, writes the temporary file, `rename(2)`. The caller reads, changes and writes without
    /// a long cache, with the config it just read, so that the unknown keys it carries are the ones in the file (3.1, 5.2).
    /// Success is not a durability statement (section 6).
    ///
    /// The lock is taken here, also for `.create`. The creator of a bundle (5.1 steps 2-4) holds the lock while it builds the
    /// directory, so it must release it before this call, or use a variant that writes under the lock it already holds (not here
    /// yet: `flock` on a second descriptor of the same file would be `busy`). The creation of a bundle is S11-06 item 5 / S11-08.
    /// `.update` refuses a config whose `identity` differs from the one in the file.
    public static func write(_ config: BundleConfig, bundle: URL, mode: BundleConfigWriteMode = .update, suffix: String? = nil) throws {
        let data = try BundleConfigCodec.encode(config)
        let lock: BundleWriterLock
        do {
            lock = try BundleWriterLock.acquire(bundle: bundle)
        } catch let failure as BundleWriterLockFailure {
            throw BundleConfigWriteError.lock(failure)
        }
        defer { lock.release() }

        let current = BundleConfigCodec.read(bundle: bundle)
        switch mode {
        case .update:
            guard case .valid(let currentConfig) = current else {
                throw BundleConfigWriteError.currentFileNotValid(current.kind, current.issue ?? .fileMissing)
            }
            // The identity does not change in an update: a config of another machine is not written over this one.
            guard config.identity == currentConfig.identity else {
                throw BundleConfigWriteError.invalidConfig(BundleConfigIssue(field: "identity", rule: "differs from the identity in config.plist"))
            }
        case .create:
            if current.issue != .fileMissing { throw BundleConfigWriteError.alreadyExists }
        }

        removeStaleTemporaryFiles(in: bundle)
        let temporaryName = BundleConfigLimits.temporaryPrefix + (suffix ?? AtomicReplace.randomSuffix())
        do {
            try AtomicReplace.replace(directory: bundle, targetName: BundleConfigLimits.fileName, temporaryName: temporaryName, data: data)
        } catch AtomicReplaceError.io(let code) {
            throw BundleConfigWriteError.io(code)
        }
    }

    /// Only files of the pattern `.config.plist.tmp-*` (regular files, not followed through links) and only under the bundle lock (2).
    private static func removeStaleTemporaryFiles(in bundle: URL) {
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: bundle.path) else { return }
        for name in names where name.hasPrefix(BundleConfigLimits.temporaryPrefix) {
            let path = bundle.appendingPathComponent(name).path
            var info = stat()
            if lstat(path, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG { _ = Darwin.unlink(path) }
        }
    }
}
