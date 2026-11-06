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

    /// The words for the user: no raw codes (they are in `details`).
    var description: String {
        switch self {
        case .notStopped: return "машина не остановлена"
        case .unsupportedDurability: return "unsupported_durability: файлы машины с дисками не удаляются до закрытия S9-06"
        case .notADirectory: return "это не каталог (или ссылка на каталог)"
        case .notABundle: return "каталог не похож на машину (нужны суффикс .odvm и обычный файл config.plist)"
        case .inUse: return MachineNotice.busyText
        case .io(let code):
            switch code {
            case EACCES, EPERM: return "нет прав на изменение файлов машины"
            case EBUSY, ETXTBSY: return "файл занят другой программой"
            case EROFS: return "диск доступен только для чтения"
            case ENOSPC: return "на диске нет места"
            default: return "ошибка файловой системы"
            }
        case .other: return "ошибка файловой системы"
        }
    }

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

/// A failure for the screen: a sentence in Russian and the technical part kept apart (the banner and the sheets show the details on request).
struct LibraryFailure: Equatable {
    var text: String
    var details: String?

    init(_ error: Error) {
        switch error {
        case let error as BundleRemovalError:
            text = error.description
            details = error.details
        case let error as CatalogError:
            switch error {
            case .notADirectory(let path): (text, details) = ("это не каталог", "Путь: \(path)")
            case .limitReached: (text, details) = ("в списке уже 1000 машин", nil)
            case .indexTooLarge: (text, details) = ("список машин занял бы больше 4 МиБ", nil)
            case .notFound(let path): (text, details) = ("записи нет в списке", "Путь: \(path)")
            case .io(let code):
                let removal = BundleRemovalError.io(code)
                (text, details) = ("не удалось записать список машин: " + removal.description, removal.details)
            }
        default:
            let removal = BundleRemovalError.from(error)
            (text, details) = (removal.description, removal.details ?? "\(error)")
        }
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
        setBanner(Self.banner(for: catalog.warnings))
        rebuild()
    }

    /// "Обновить": re-reads the bundles and asks the VMHost of a running machine once (`QUERY`). Never on a timer.
    func refresh() {
        catalog.load()
        setBanner(Self.banner(for: catalog.warnings))
        rebuild()
        machines.forEach { $0.refresh() }
    }

    private static func banner(for warnings: [CatalogWarning]) -> String? {
        for warning in warnings {
            switch warning {
            case .indexCorrupt: return "Список машин был повреждён: он собран заново по каталогу машин. Копия повреждённого файла сохранена рядом."
            case .indexNewerVersion: return "Список машин записан более новой версией приложения: изменения в нём не сохраняются."
            case .limitReached: return "В каталоге больше 1000 машин: показаны первые 1000."
            }
        }
        return nil
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
                if !persisted { setBanner(Self.notSavedText) }
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
                setBanner(Self.notSavedText, details: [failure.text, failure.details].compactMap { $0 }.joined(separator: "; "))
                rebuild()
                selection = entry.id
                return true
            }
            reportOpenFailure(url, error)
            return false
        }
    }

    private static let notSavedText = "Список машин не сохранён: машина показана только до закрытия приложения."

    private func reportOpenFailure(_ url: URL, _ error: Error) {
        let failure = LibraryFailure(error)
        setBanner(
            "Не удалось открыть «\(url.lastPathComponent)»: \(failure.text).",
            details: ["Путь: \(url.path)", failure.details].compactMap { $0 }.joined(separator: "; "))
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
                "\(done), но список машин не удалось сохранить: \(failure.text). При следующем запуске она может появиться снова.",
                details: failure.details)
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
