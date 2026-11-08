import Darwin
import Foundation

// Library index `Library.plist`: docs/specs/VM_BUNDLE.md section 8 (S11-06a).
//
// The index belongs to the application and is RECOVERABLE: losing or damaging it never loses a machine (disks and configuration
// live only in the bundle). It has no classes valid/incompatible/invalid; a damaged file means "rebuild", not a damaged machine.
//
// ASSUMPTION OF THIS SLICE (not a decision of the spec): section 8 stores the location of a bundle as `bookmark` data. The way a
// bookmark is created and resolved depends on decision (b) of ADR-0006, which the owner has not confirmed (VM_BUNDLE section 4).
// Until it is confirmed an index entry stores the location as an absolute `path` string (key `path`). An entry that has only a
// `bookmark` (written by a later version of the program) cannot be resolved here: it is kept in the file unchanged and is not
// shown. A `bookmark` next to a `path` is carried through untouched. The key `path` is an addition to the schema of section 8; a
// reader that follows the table to the letter skips such an element (no `bookmark`) and keeps it on rewrite. When decision (b) is
// confirmed, `CatalogEntry` gets its bookmark resolution and `path` becomes a fallback or goes away.
//
// Writes: the atomic replacement of 5.2 through `.Library.plist.tmp-<suffix>` in the directory of the index; there is no lock on
// `Library.plist` (8 item 2), so a write is a read-modify-write in one step and the last writer wins. Durability is not claimed
// (section 6, ISSUE-055). Nothing here deletes a bundle or any file of a machine: removing an entry removes it from the index only.

public enum CatalogLimits {
    /// 8 table: 0 to 1000 elements in `machines`; the same limit holds for the merged list of 8 item 3.
    public static let maxMachines = 1_000
    public static let maxIndexBytes = 4 << 20
    public static let fileName = "Library.plist"
    public static let temporaryPrefix = ".Library.plist.tmp-"
    public static let corruptCopyPrefix = "Library.plist.corrupt-"
    public static let bundleSuffix = ".odvm"
}

/// One shown machine: the location of a bundle and the hint of its identity.
public struct CatalogEntry: Equatable, Identifiable {
    /// Absolute URL of the bundle directory.
    public var bundleURL: URL
    /// UUID from `config.plist` at the time of adding. A hint, not a source of truth (8 table).
    public var identityHint: String?
    var bookmark: Data?
    var unknownKeys: UnknownKeys

    public var id: String { bundleURL.path }

    public init(bundleURL: URL, identityHint: String? = nil) {
        self.bundleURL = bundleURL
        self.identityHint = identityHint
        self.bookmark = nil
        self.unknownKeys = UnknownKeys()
    }
}

/// State of the index file as `load()` found it.
public enum CatalogIndexState: Equatable {
    case loaded
    /// No file: the list was rebuilt by scanning the default directory (8 item 3).
    case missing
    /// Not a plist, root not a dict, over 4 MiB, no or bad `schema_version`, `machines` not an array or over 1000 elements.
    case corrupt(String)
    /// `schema_version` above 1: the file is not rewritten and the list is not saved (8 item 1).
    case newerVersion(Int)
}

public enum CatalogWarning: Equatable {
    case indexCorrupt(String)
    /// The list is valid for this run only; changes are not written.
    case indexNewerVersion(Int)
    /// The scan of the default directory stopped at the limit of 1000 entries; the index is not considered damaged.
    case limitReached
}

/// Two or more entries with the same identity hint (a copied bundle): shown as a conflict, never merged or deleted (8 item 5).
public struct CatalogIdentityConflict: Equatable {
    public var identity: String
    public var entries: [CatalogEntry]
}

public enum CatalogBundleStatus: Equatable {
    case directoryMissing
    case notADirectory
    /// The result of reading `config.plist`: `valid`, `incompatible` or `invalid` (also "no config.plist").
    case config(BundleConfigState)
}

public enum CatalogError: Error, Equatable, CustomStringConvertible {
    case notADirectory(String)
    case limitReached
    /// The index would be larger than 4 MiB (not the limit of entries).
    case indexTooLarge
    case notFound(String)
    case io(Int32)

    public var description: String {
        switch self {
        case .notADirectory(let path): return "not a directory: \(path)"
        case .limitReached: return "the library holds 1000 entries"
        case .indexTooLarge: return "the library index would be larger than 4 MiB"
        case .notFound(let path): return "no such entry: \(path)"
        case .io(let code): return "I/O error \(code)"
        }
    }
}

/// Outcome of `add`.
public enum CatalogAddResult: Equatable {
    case added(CatalogEntry, persisted: Bool)
    /// The same bundle directory is already listed. (The same identity under another path is NOT this case: it is added and is a conflict.)
    case alreadyListed(CatalogEntry)
}

/// Not thread-safe: use from one queue (the main queue in the application).
public final class Catalog {
    enum Slot {
        case entry(CatalogEntry)
        /// An element the program cannot show (not a dict, no `path`): kept as it is on every rewrite (8 "Поведение при ошибке").
        case opaque(Any)
    }

    enum IndexRead {
        case loaded(slots: [Slot], topLevel: [String: Any])
        case missing
        case corrupt(String)
        case newerVersion(Int)
    }

    public let indexURL: URL
    public let defaultDirectory: URL
    public private(set) var indexState: CatalogIndexState = .missing
    public private(set) var warnings: [CatalogWarning] = []
    var slots: [Slot] = []
    var topLevelKeys: [String: Any] = [:]

    public init(indexURL: URL, defaultDirectory: URL) {
        self.indexURL = indexURL
        self.defaultDirectory = defaultDirectory
    }

    /// `~/Library/Application Support/OpenDesktop/Library.plist` and `~/Documents/Open Desktop/` (8, 8 item 3). Access to Documents after
    /// a restart is not guaranteed (section 4, ADR-0006, TCC not investigated).
    public static func standard() -> Catalog {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return Catalog(
            indexURL: home.appendingPathComponent("Library/Application Support/OpenDesktop").appendingPathComponent(CatalogLimits.fileName),
            defaultDirectory: home.appendingPathComponent("Documents/Open Desktop", isDirectory: true))
    }

    /// The machines in the order of the index (order of the array = order of display).
    public var entries: [CatalogEntry] {
        slots.compactMap { if case .entry(let entry) = $0 { return entry } else { return nil } }
    }

    /// Whether `add`, `remove` and `save` write the index. False for a newer `schema_version` (8 item 1).
    public var canPersist: Bool {
        if case .newerVersion = indexState { return false }
        return true
    }

    // MARK: load and rebuild

    /// Reads the index. A missing, damaged or newer index is not an error: the list is rebuilt in memory from the default directory
    /// only (8 items 1 and 3: bookmarks of a damaged or newer file are not trusted). Nothing is written.
    public func load() {
        warnings = []
        switch readIndex() {
        case .loaded(let loaded, let top):
            indexState = .loaded
            slots = loaded
            topLevelKeys = top
            fillMissingIdentityHints()
        case .missing:
            indexState = .missing
            rebuildFromDirectory()
        case .corrupt(let reason):
            indexState = .corrupt(reason)
            warnings.append(.indexCorrupt(reason))
            rebuildFromDirectory()
        case .newerVersion(let version):
            indexState = .newerVersion(version)
            warnings.append(.indexNewerVersion(version))
            rebuildFromDirectory()
        }
    }

    private func rebuildFromDirectory() {
        slots = []
        topLevelKeys = [:]
        appendScanResults(to: &slots)
    }

    /// 8 item 3 (b): the default directory without recursion, entries `*.odvm` that are directories (a link is not followed: `lstat`),
    /// not yet listed, sorted by name, appended; the merged list never exceeds 1000 entries.
    private func appendScanResults(to slots: inout [Slot]) {
        let known = Set(slots.compactMap { slot -> String? in
            if case .entry(let entry) = slot { return Self.dedupeKey(entry.bundleURL) } else { return nil }
        })
        let found = scanDefaultDirectory().filter { !known.contains(Self.dedupeKey($0)) }
        for url in found {
            if slots.count >= CatalogLimits.maxMachines {
                if !warnings.contains(.limitReached) { warnings.append(.limitReached) }
                break
            }
            slots.append(.entry(CatalogEntry(bundleURL: url, identityHint: Self.identityHint(of: url))))
        }
    }

    func scanDefaultDirectory() -> [URL] {
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: defaultDirectory.path) else { return [] }
        var urls: [URL] = []
        for name in names.sorted() where name.hasSuffix(CatalogLimits.bundleSuffix) && name.count > CatalogLimits.bundleSuffix.count {
            let url = defaultDirectory.appendingPathComponent(name, isDirectory: true)
            var info = stat()
            if lstat(url.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR { urls.append(url) }
        }
        return urls
    }

    /// Adds bundles of the default directory that are not listed (for example after the entry was lost). Written like any change.
    @discardableResult
    public func rescanDefaultDirectory() throws -> Bool {
        try mutate { slots in
            self.appendScanResults(to: &slots)
        }
    }

    private func fillMissingIdentityHints() {
        for index in slots.indices {
            if case .entry(var entry) = slots[index], entry.identityHint == nil {
                entry.identityHint = Self.identityHint(of: entry.bundleURL)
                slots[index] = .entry(entry)
            }
        }
    }

    // MARK: changes

    /// Adds a bundle directory to the end of the list and writes the index. A bundle whose `config.plist` is not valid is added too
    /// (it is shown as incompatible or damaged, section 3.1); the same bundle twice is `alreadyListed`.
    public func add(bundle: URL) throws -> CatalogAddResult {
        let url = bundle.standardizedFileURL
        var info = stat()
        guard stat(url.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else { throw CatalogError.notADirectory(url.path) }
        var result: CatalogAddResult?
        let persisted = try mutate { slots in
            if let existing = slots.compactMap({ slot -> CatalogEntry? in
                if case .entry(let entry) = slot { return entry } else { return nil }
            }).first(where: { Self.dedupeKey($0.bundleURL) == Self.dedupeKey(url) }) {
                result = .alreadyListed(existing)
                return
            }
            guard slots.count < CatalogLimits.maxMachines else { throw CatalogError.limitReached }
            let entry = CatalogEntry(bundleURL: url, identityHint: Self.identityHint(of: url))
            slots.append(.entry(entry))
            result = .added(entry, persisted: false)
        }
        guard let result else { throw CatalogError.io(EINVAL) }
        if case .added(let entry, _) = result { return .added(entry, persisted: persisted) }
        return result
    }

    /// Removes the entry from the index. No file of the bundle is touched (the deletion of files is a separate, confirmed action).
    @discardableResult
    public func remove(_ entry: CatalogEntry) throws -> Bool {
        try mutate { slots in
            guard let index = slots.firstIndex(where: {
                if case .entry(let candidate) = $0 { return candidate.id == entry.id } else { return false }
            }) else { throw CatalogError.notFound(entry.id) }
            slots.remove(at: index)
        }
    }

    /// Writes the list that `load()` rebuilt in memory (after a missing or damaged index). Returns false when nothing is written.
    @discardableResult
    public func save() throws -> Bool {
        try mutate { _ in }
    }

    /// One read-modify-write step (8 item 2): the file is read again, the change is applied to what is in it, the file is replaced.
    /// When the file is missing, damaged or newer, the base is the list in memory (the rebuilt one). Returns whether the index was written.
    private func mutate(_ change: (inout [Slot]) throws -> Void) throws -> Bool {
        let fresh = readIndex()
        var working: [Slot]
        var top: [String: Any]
        switch fresh {
        case .loaded(let loaded, let topLevel):
            working = loaded
            top = topLevel
            indexState = .loaded
        case .missing:
            working = slots
            top = topLevelKeys
            indexState = .missing
        case .corrupt(let reason):
            working = slots
            top = [:]
            indexState = .corrupt(reason)
        case .newerVersion(let version):
            working = slots
            top = [:]
            indexState = .newerVersion(version)
        }
        try change(&working)
        slots = working
        topLevelKeys = top
        // The fresh read has no hints that `load()` filled in memory; without this a conflict could vanish after any change.
        fillMissingIdentityHints()
        guard canPersist else { return false }
        if case .corrupt = fresh { try keepCopyOfCorruptIndex() }
        try writeIndex(slots: slots, topLevel: top)
        indexState = .loaded
        return true
    }

    /// A damaged file is replaced only after a copy of it lies next to it (8 item 1); the copy name is `Library.plist.corrupt-<suffix>`.
    private func keepCopyOfCorruptIndex() throws {
        let copy = indexURL.deletingLastPathComponent().appendingPathComponent(CatalogLimits.corruptCopyPrefix + AtomicReplace.randomSuffix())
        do {
            try FileManager.default.copyItem(at: indexURL, to: copy)
        } catch {
            throw CatalogError.io(EIO)
        }
    }

    // MARK: identity conflicts and status

    /// Entries that share an identity hint (8 item 5). Resolving a conflict is not done here.
    public var identityConflicts: [CatalogIdentityConflict] {
        var groups: [String: [CatalogEntry]] = [:]
        var order: [String] = []
        for entry in entries {
            guard let identity = entry.identityHint else { continue }
            if groups[identity] == nil { order.append(identity) }
            groups[identity, default: []].append(entry)
        }
        return order.compactMap { identity in
            guard let group = groups[identity], group.count > 1 else { return nil }
            return CatalogIdentityConflict(identity: identity, entries: group)
        }
    }

    /// Reads the bundle of an entry. Never throws and never writes; a missing directory or `config.plist` is a status, not a failure.
    public func status(of entry: CatalogEntry) -> CatalogBundleStatus {
        var info = stat()
        guard stat(entry.bundleURL.path, &info) == 0 else { return .directoryMissing }
        guard (info.st_mode & S_IFMT) == S_IFDIR else { return .notADirectory }
        return .config(BundleConfigCodec.read(bundle: entry.bundleURL))
    }

    // MARK: index file

    func readIndex() -> IndexRead {
        let fd = Darwin.open(indexURL.path, O_RDONLY | O_CLOEXEC)
        guard fd >= 0 else {
            if errno == ENOENT { return .missing }
            return .corrupt("cannot be opened (errno \(errno))")
        }
        defer { Darwin.close(fd) }
        var info = stat()
        guard fstat(fd, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG else { return .corrupt("is not a regular file") }
        guard info.st_size <= off_t(CatalogLimits.maxIndexBytes) else { return .corrupt("is larger than 4 MiB") }
        var bytes = [UInt8]()
        var chunk = [UInt8](repeating: 0, count: 65_536)
        while bytes.count <= CatalogLimits.maxIndexBytes {
            let count = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
            if count < 0 {
                if errno == EINTR { continue }
                return .corrupt("cannot be read (errno \(errno))")
            }
            if count == 0 { break }
            bytes.append(contentsOf: chunk[..<count])
        }
        guard bytes.count <= CatalogLimits.maxIndexBytes else { return .corrupt("is larger than 4 MiB") }
        let object: Any
        do {
            object = try PropertyListSerialization.propertyList(from: Data(bytes), options: [], format: nil)
        } catch {
            return .corrupt("is not a property list")
        }
        guard let root = PlistType.dictionary(object) else { return .corrupt("root is not a dict") }
        guard let versionValue = root["schema_version"], let version = PlistType.integer(versionValue) else {
            return .corrupt("schema_version is missing or not an integer")
        }
        if version < 1 { return .corrupt("schema_version is below 1") }
        if version > 1 { return .newerVersion(version) }
        var elements: [Any] = []
        if let machines = root["machines"] {
            guard let array = PlistType.array(machines) else { return .corrupt("machines is not an array") }
            guard array.count <= CatalogLimits.maxMachines else { return .corrupt("machines has more than 1000 elements") }
            elements = array
        }
        let slots = elements.map { Self.slot(from: $0) }
        return .loaded(slots: slots, topLevel: root.filter { $0.key != "schema_version" && $0.key != "machines" })
    }

    /// An element that is not a dict or has no usable location is skipped in the list but kept as it is (opaque). A hint that is not a
    /// canonical UUID is not used as the hint (8 "identity — подсказка, не ключ"), but its raw value stays in `unknownKeys` and is
    /// written back as it was, until a valid hint is read from the bundle (`fillMissingIdentityHints` then replaces it). The same for a
    /// `bookmark` that is not `Data`: nothing of an element is lost on a rewrite.
    private static func slot(from element: Any) -> Slot {
        guard let dictionary = PlistType.dictionary(element), let pathValue = dictionary["path"], let path = PlistType.string(pathValue),
              path.hasPrefix("/"), path.utf8.count < 1_024, !path.contains("\0") else { return .opaque(element) }
        var entry = CatalogEntry(bundleURL: URL(fileURLWithPath: path, isDirectory: true))
        var rest = dictionary
        rest["path"] = nil
        if let hint = dictionary["identity"], let text = PlistType.string(hint), BundleText.isCanonicalUUID(text) {
            entry.identityHint = text
            rest["identity"] = nil
        }
        if let bookmark = dictionary["bookmark"], let data = PlistType.data(bookmark) {
            entry.bookmark = data
            rest["bookmark"] = nil
        }
        entry.unknownKeys = UnknownKeys(rest)
        return .entry(entry)
    }

    private func writeIndex(slots: [Slot], topLevel: [String: Any]) throws {
        var root = topLevel
        root["schema_version"] = 1
        root["machines"] = slots.map { slot -> Any in
            switch slot {
            case .opaque(let value): return value
            case .entry(let entry):
                var element = entry.unknownKeys.values
                element["path"] = entry.bundleURL.path
                if let bookmark = entry.bookmark { element["bookmark"] = bookmark }
                if let identity = entry.identityHint { element["identity"] = identity }
                return element
            }
        }
        let data: Data
        do {
            data = try PropertyListSerialization.data(fromPropertyList: root, format: .xml, options: 0)
        } catch {
            throw CatalogError.io(EINVAL)
        }
        guard data.count <= CatalogLimits.maxIndexBytes else { throw CatalogError.indexTooLarge }
        let directory = indexURL.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try AtomicReplace.replace(
                directory: directory, targetName: indexURL.lastPathComponent,
                temporaryName: CatalogLimits.temporaryPrefix + AtomicReplace.randomSuffix(), data: data)
        } catch AtomicReplaceError.io(let code) {
            throw CatalogError.io(code)
        } catch {
            throw CatalogError.io(EIO)
        }
    }

    // MARK: helpers

    /// Symbolic links of the parent path are resolved for the comparison only: `/var/...` and `/private/var/...` are one bundle.
    static func dedupeKey(_ url: URL) -> String {
        url.resolvingSymlinksInPath().path
    }

    /// UUID of a valid `config.plist` at `url`, nil for a bundle that is not valid.
    static func identityHint(of bundle: URL) -> String? {
        BundleConfigCodec.read(bundle: bundle).config?.identity
    }
}
