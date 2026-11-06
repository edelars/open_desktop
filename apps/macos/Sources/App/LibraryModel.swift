import Combine
import Darwin
import Foundation
import ODShared

// The library of machines as the window sees it (S11-06b): the `Catalog` (index `Library.plist`, VM_BUNDLE.md section 8) and one
// `MachineController` per listed bundle. A local observable of the library, not a global store (ARCHITECTURE section 3).
// No UI framework here, so the model is tested without windows. Main queue only (`Catalog` is not thread-safe).

enum BundleRemovalError: Error, Equatable, CustomStringConvertible {
    /// A machine with a session, or not `stopped`.
    case notStopped
    /// Files of a machine with `disks` are not deleted before S9-06 (ISSUE-055): the library entry alone may go.
    case unsupportedDurability
    case notADirectory
    /// The directory is not a bundle: VM_BUNDLE section 2 wants the suffix `.odvm` AND a regular file `config.plist` (not a link). The
    /// application deletes and lists nothing else.
    case notABundle
    /// The bundle lock is held (a VMHost of another process runs the machine).
    case inUse
    /// A POSIX error (`errno`) of the file system.
    case io(Int32)
    /// An error that has no POSIX code; the argument is its technical description (for the details only).
    case other(String)

    /// The entry of the catalog that tells this error (S11-09).
    var issue: LibraryIssue {
        switch self {
        case .notStopped: return .removalNotStopped
        case .unsupportedDurability: return .removalUnsupportedDurability
        case .notADirectory: return .notADirectory
        case .notABundle: return .notABundle
        case .inUse: return .removalInUse
        case .io(let code): return LibraryIssue(errno: code)
        case .other: return .fileSystemError
        }
    }

    /// The words for the user as a fragment of a sentence: the title of the catalog entry, no raw codes (they are in `details`).
    var description: String { ErrorPresentation.lowercasedFirst(ErrorCatalog.entry(for: issue).title) }

    /// The technical part: the code that the user does not need in the sentence.
    var details: String? {
        switch self {
        case .io(let code): return "errno \(code): \(String(cString: strerror(code)))"
        case .other(let text): return text
        default: return nil
        }
    }

    /// A file system error as one of the cases: the POSIX code is taken from the error itself or from the underlying one (Cocoa wraps it:
    /// `NSCocoaErrorDomain` 513 holds `EACCES` or `EPERM`).
    static func from(_ error: Error) -> BundleRemovalError {
        if let code = posixCode(of: error as NSError) { return .io(code) }
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain {
            switch ns.code {
            case NSFileWriteNoPermissionError, NSFileReadNoPermissionError: return .io(EACCES)
            case NSFileWriteVolumeReadOnlyError: return .io(EROFS)
            default: break
            }
        }
        return .other("\(ns.domain) \(ns.code)")
    }

    private static func posixCode(of error: NSError) -> Int32? {
        if error.domain == NSPOSIXErrorDomain { return Int32(truncatingIfNeeded: error.code) }
        if let underlying = error.userInfo[NSUnderlyingErrorKey] as? NSError { return posixCode(of: underlying) }
        return nil
    }
}

/// A failure for the screen: a fragment of a sentence in Russian (the title of the catalog entry), the explanation with the action, and the
/// technical part kept apart (the banner and the sheets show the details on request).
struct LibraryFailure: Equatable {
    var text: String
    /// The explanation and the action of the catalog entry.
    var hint: String
    var details: String?

    init(_ error: Error) {
        let presentation: ErrorPresentation
        switch error {
        case let error as BundleRemovalError:
            presentation = .library(error.issue, technical: error.details)
        case let error as CatalogError:
            switch error {
            case .notADirectory(let path), .notFound(let path):
                presentation = .library(LibraryIssue(error), technical: "Путь: \(path)")
            case .limitReached, .indexTooLarge:
                presentation = .library(LibraryIssue(error))
            case .io(let code):
                presentation = .library(LibraryIssue(error), technical: BundleRemovalError.io(code).details)
            }
        default:
            let removal = BundleRemovalError.from(error)
            presentation = .library(removal.issue, technical: removal.details ?? "\(error)")
        }
        text = presentation.fragment
        hint = presentation.message
        details = presentation.details
    }
}

final class LibraryModel: ObservableObject {
    let catalog: Catalog
    let vmhost: VMHostLocation
    private let launcher: MachineLauncher
    private let hostMemoryBytes: UInt64

    @Published private(set) var machines: [MachineController] = []
    @Published var selection: String?
    /// The machine whose deletion sheet is open ("Удалить машину…").
    @Published var pendingDelete: MachineController?
    /// A line about the library itself (a damaged or newer index, a failed add or delete), not about one machine.
    @Published private(set) var banner: String?
    /// The technical part of the banner (a path, a code): shown on request, never in the headline.
    @Published private(set) var bannerDetails: String?
    /// Entries that share an identity (a copied bundle): shown as a warning, never merged (VM_BUNDLE 8 item 5).
    @Published private(set) var conflictingIDs: Set<String> = []

    private var observers: [String: AnyCancellable] = [:]
    /// Bundles opened in this run (`--open`, "Открыть…"). They stay in the list until the application closes, whatever the index says:
    /// a new or unwritable index must not make `refresh()` lose a machine that the user just opened.
    private var openedThisRun: [CatalogEntry] = []

    init(
        catalog: Catalog, vmhost: VMHostLocation, launcher: @escaping MachineLauncher = defaultMachineLauncher,
        hostMemoryBytes: UInt64 = ProcessInfo.processInfo.physicalMemory
    ) {
        self.catalog = catalog
        self.vmhost = vmhost
        self.launcher = launcher
        self.hostMemoryBytes = hostMemoryBytes
    }

    var selected: MachineController? {
        guard let selection else { return nil }
        return machines.first { $0.id == selection }
    }

    private func setBanner(_ text: String?, details: String? = nil) {
        banner = text
        bannerDetails = text == nil ? nil : details
    }

    // MARK: loading

    func load() {
        catalog.load()
        setBanner(presentation: Self.banner(for: catalog.warnings))
        rebuild()
    }

    /// "Обновить": re-reads the bundles and asks the VMHost of a running machine once (`QUERY`). Never on a timer.
    func refresh() {
        catalog.load()
        setBanner(presentation: Self.banner(for: catalog.warnings))
        rebuild()
        machines.forEach { $0.refresh() }
    }

    /// The first warning of the catalog, as the words of the catalog of errors (S11-09).
    private static func banner(for warnings: [CatalogWarning]) -> ErrorPresentation? {
        warnings.first.map { ErrorPresentation.library(LibraryIssue($0)) }
    }

    private func setBanner(presentation: ErrorPresentation?) {
        setBanner(presentation?.fullText, details: presentation?.details)
    }

    private static func key(_ url: URL) -> String { url.resolvingSymlinksInPath().path }

    /// The entries of the catalog, then the bundles opened in this run that the catalog (re-read) does not list.
    private var listedEntries: [CatalogEntry] {
        var entries = catalog.entries
        var known = Set(entries.map { Self.key($0.bundleURL) })
        for entry in openedThisRun where !known.contains(Self.key(entry.bundleURL)) && entries.count < CatalogLimits.maxMachines {
            entries.append(entry)
            known.insert(Self.key(entry.bundleURL))
        }
        return entries
    }

    /// The controllers follow the entries of the catalog; a controller that has a session stays (the process exists, whatever the index says).
    private func rebuild() {
        var byID: [String: MachineController] = [:]
        machines.forEach { byID[$0.id] = $0 }
        var next: [MachineController] = []
        let entries = listedEntries
        for entry in entries {
            let status = catalog.status(of: entry)
            if let existing = byID.removeValue(forKey: entry.id) {
                existing.updateStatus(status)
                next.append(existing)
            } else {
                next.append(MachineController(
                    entry: entry, status: status, vmhost: vmhost, launcher: launcher, hostMemoryBytes: hostMemoryBytes))
            }
        }
        for (_, orphan) in byID where orphan.hasSession { next.append(orphan) }
        machines = next
        var groups: [String: [String]] = [:]
        for entry in entries { if let hint = entry.identityHint { groups[hint, default: []].append(entry.id) } }
        conflictingIDs = Set(groups.values.filter { $0.count > 1 }.joined())
        // A change of one row redraws the views that read the selected machine (the "Машина" menu, the toolbar).
        observers = [:]
        for machine in next {
            observers[machine.id] = machine.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        }
        if let selection, !next.contains(where: { $0.id == selection }) { self.selection = nil }
    }

    // MARK: adding

    /// Whether a directory has the shape of a bundle (VM_BUNDLE section 2): the name ends with `.odvm` (and is not only the suffix) AND
    /// it holds `config.plist` as a regular file, not a link. Only the shape: whether the file is valid is the status of the row.
    static func hasBundleShape(_ url: URL) -> Bool {
        let name = url.lastPathComponent
        guard name.hasSuffix(CatalogLimits.bundleSuffix), name.count > CatalogLimits.bundleSuffix.count else { return false }
        var info = stat()
        guard lstat(url.appendingPathComponent(BundleConfigLimits.fileName).path, &info) == 0 else { return false }
        return (info.st_mode & S_IFMT) == S_IFREG
    }

    /// `--open <bundle>` and "Открыть…": lists the bundle (when it is not listed) and selects it. A directory that has no `.odvm` suffix or
    /// no `config.plist` is refused (nothing is written to the index). A bundle whose `config.plist` is not valid is listed: it is shown as
    /// incompatible or damaged and cannot be started.
    @discardableResult
    func open(bundle: URL) -> Bool {
        let url = bundle.standardizedFileURL
        var info = stat()
        guard stat(url.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else {
            reportOpenFailure(url, BundleRemovalError.notADirectory)
            return false
        }
        guard Self.hasBundleShape(url) else {
            reportOpenFailure(url, BundleRemovalError.notABundle)
            return false
        }
        do {
            switch try catalog.add(bundle: url) {
            case .added(let entry, let persisted):
                openedThisRun.append(entry)
                if !persisted { setBanner(presentation: Self.notSaved()) }
                rebuild()
                selection = entry.id
            case .alreadyListed(let entry):
                rebuild()
                selection = entry.id
            }
            return true
        } catch {
            // A write that failed after the entry was put into the list in memory: the machine is shown until the application closes.
            if let entry = catalog.entries.first(where: { Self.key($0.bundleURL) == Self.key(url) }) {
                openedThisRun.append(entry)
                let failure = LibraryFailure(error)
                setBanner(presentation: Self.notSaved(technical: [failure.text, failure.details].compactMap { $0 }.joined(separator: "\n")))
                rebuild()
                selection = entry.id
                return true
            }
            reportOpenFailure(url, error)
            return false
        }
    }

    private static func notSaved(technical: String? = nil) -> ErrorPresentation {
        .library(.indexNotSaved, technical: technical)
    }

    private func reportOpenFailure(_ url: URL, _ error: Error) {
        let failure = LibraryFailure(error)
        setBanner(
            "Не удалось открыть «\(url.lastPathComponent)»: \(failure.text).",
            details: ["Путь: \(url.path)", "Что делать: \(failure.hint)", failure.details].compactMap { $0 }.joined(separator: "\n"))
    }

    // MARK: deleting

    /// Whether the files of the machine may be deleted from the disk at all: not for a configuration with `disks` (before S9-06).
    static func canDeleteFiles(of machine: MachineController) -> Bool {
        machine.config?.disks.isEmpty ?? true
    }

    /// Removes the machine from the library; with `deleteFiles` also the bundle directory. The files go first: when they cannot be
    /// deleted nothing has changed (an error is thrown). Once the files are gone the deletion has happened: a failure to write the index
    /// afterwards is a line of the banner, not a failed deletion. Only in `stopped` without a session (the table of 7.2).
    func delete(_ machine: MachineController, deleteFiles: Bool) throws {
        guard machine.allows(.delete) else { throw BundleRemovalError.notStopped }
        if deleteFiles {
            guard Self.canDeleteFiles(of: machine) else { throw BundleRemovalError.unsupportedDurability }
            try Self.removeBundleFiles(at: machine.entry.bundleURL)
        }
        openedThisRun.removeAll { Self.key($0.bundleURL) == Self.key(machine.entry.bundleURL) }
        var indexFailure: Error?
        do {
            try catalog.remove(machine.entry)
        } catch CatalogError.notFound {
            // Already gone from the index: the goal is reached.
        } catch {
            indexFailure = error // the entry is already out of the list in memory (the write is the last step of the change)
        }
        if pendingDelete?.id == machine.id { pendingDelete = nil }
        rebuild()
        if let indexFailure {
            let failure = LibraryFailure(indexFailure)
            let done = deleteFiles ? "Файлы машины удалены" : "Машина убрана из списка"
            setBanner(
                "\(done), но \(failure.text). При следующем запуске она может появиться снова.",
                details: ["Что делать: \(failure.hint)", failure.details].compactMap { $0 }.joined(separator: "\n"))
        }
    }

    /// Deletes the directory of a bundle. Refuses a link, a non-directory, a directory that is not a bundle (VM_BUNDLE section 2: suffix
    /// `.odvm` and a regular `config.plist`) and a bundle whose lock is held. A directory that is gone is a success of this step.
    static func removeBundleFiles(at url: URL) throws {
        var info = stat()
        guard lstat(url.path, &info) == 0 else {
            if errno == ENOENT { return } // nothing to delete
            throw BundleRemovalError.io(errno)
        }
        guard (info.st_mode & S_IFMT) == S_IFDIR else { throw BundleRemovalError.notADirectory }
        guard hasBundleShape(url) else { throw BundleRemovalError.notABundle }
        if BundleWriterLock.probeFailure(bundle: url) == .busy { throw BundleRemovalError.inUse }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            var after = stat()
            if lstat(url.path, &after) != 0, errno == ENOENT { return } // it is gone anyway
            throw BundleRemovalError.from(error)
        }
    }

    func reportFailure(_ text: String, details: String? = nil) { setBanner(text, details: details) }

    func dismissBanner() { setBanner(nil) }
}
