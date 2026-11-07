import Darwin
import Foundation
import ODShared

// The library as `odctl` sees it (S11-13, docs/specs/CLI.md section 5, VM_BUNDLE.md section 8).
// Without `--library` the catalog is the one of the application: the index `~/Library/Application Support/OpenDesktop/Library.plist` and
// the default directory `~/Documents/Open Desktop/`. With `--library DIR` the default directory is `DIR` and the index is `DIR/Library.plist`
// (nothing outside `DIR` is read or written). The index has one format (`Catalog` of ODShared); the CLI does not invent a second one.

func makeCatalog(library: String?) -> Catalog {
    guard let library else { return Catalog.standard() }
    let directory = URL(fileURLWithPath: library, isDirectory: true).standardizedFileURL
    return Catalog(indexURL: directory.appendingPathComponent(CatalogLimits.fileName), defaultDirectory: directory)
}

/// The bundle a command works on.
struct ResolvedBundle {
    var url: URL
    /// The catalog entry when the argument was a name or a listed path; `nil` for a path that is not listed.
    var entry: CatalogEntry?
    var catalog: Catalog?
    var state: BundleConfigState { BundleConfigCodec.read(bundle: url) }
}

/// The name of a bundle directory without the suffix (`smoke.odvm` is `smoke`).
func directoryStem(_ url: URL) -> String {
    let name = url.lastPathComponent
    return name.hasSuffix(CatalogLimits.bundleSuffix) ? String(name.dropLast(CatalogLimits.bundleSuffix.count)) : name
}

/// `<name|path>`: an argument with `/` or the suffix `.odvm` is a path (no index is read); anything else is a machine name that is looked
/// up in the library by the `name` of `config.plist` and by the name of the directory.
func resolveBundle(_ argument: String, library: String?) throws -> ResolvedBundle {
    if argument.contains("/") || argument.hasSuffix(CatalogLimits.bundleSuffix) {
        let url = URL(fileURLWithPath: argument, isDirectory: true).standardizedFileURL
        var info = stat()
        guard lstat(url.path, &info) == 0 else {
            // A path from the command line, not an entry of the list: the words of the catalog entry speak about the list.
            throw CLIFailure(exit: .failure, code: "library.bundle_directory_missing", title: "Каталог машины не найден",
                             message: "По указанному пути каталога нет. Проверьте путь.", details: ["Путь: \(url.path)"])
        }
        guard (info.st_mode & S_IFMT) == S_IFDIR else {
            throw CLIFailure.library(.notADirectory, details: ["Путь: \(url.path)"])
        }
        return ResolvedBundle(url: url, entry: nil, catalog: nil)
    }
    let catalog = makeCatalog(library: library)
    catalog.load()
    var matches: [CatalogEntry] = []
    for entry in catalog.entries {
        if directoryStem(entry.bundleURL) == argument {
            matches.append(entry)
        } else if case .config(let state) = catalog.status(of: entry), let config = state.config, config.name == argument {
            matches.append(entry)
        }
    }
    guard let first = matches.first else {
        throw CLIFailure.library(.entryNotFound, details: ["Имя: \(argument)", "Каталог: \(catalog.defaultDirectory.path)"])
    }
    if matches.count > 1 {
        throw CLIFailure(exit: .usage, code: "ambiguous_name", title: "Имя «\(argument)» подходит нескольким машинам",
                         message: "Укажите путь к каталогу машины вместо имени.",
                         details: matches.map { "Путь: \($0.bundleURL.path)" })
    }
    return ResolvedBundle(url: first.bundleURL, entry: first, catalog: catalog)
}

/// The name that `--confirm` must repeat: the `name` of `config.plist`, or the name of the directory when the file cannot be read.
func confirmationName(for resolved: ResolvedBundle) -> String {
    resolved.state.config?.name ?? directoryStem(resolved.url)
}

// MARK: removal (`BundleRemoval` of ODShared; the application has its own copy of the rule, S11-06b)

/// Deletes the directory of a bundle with the rule of `BundleRemoval`, as a failure of the command: a link or a non-directory, a directory
/// that is not a bundle, disk data (exit 4, nothing is deleted) and a bundle whose lock is held (exit 3). `config` is what could be read
/// from `config.plist` (`nil` when it cannot be read: `disks/` is looked at in any case).
func removeBundleFiles(at url: URL, config: BundleConfig?) throws {
    do {
        try BundleRemoval.removeFiles(at: url, config: config)
    } catch let refusal as BundleRemovalRefusal {
        switch refusal {
        case .notADirectory: throw CLIFailure.library(.notADirectory, details: ["Путь: \(url.path)"])
        case .notABundle: throw CLIFailure.library(.notABundle, details: ["Путь: \(url.path)"])
        case .inUse: throw lockBusyFailure()
        case .unsupportedDurability:
            var details = ["Путь: \(url.path)"]
            if let config { details.append("Дисков в конфигурации: \(config.disks.count)") } else { details.append("Конфигурация не читается, в disks/ есть файлы") }
            throw CLIFailure.library(.removalUnsupportedDurability, exit: .unsupportedDurability, details: details)
        case .io(let code):
            throw CLIFailure.library(LibraryIssue(errno: code), details: ["errno: \(code)", "Путь: \(url.path)"])
        }
    }
}

/// The lock of the machine is held: another process (the application, another `odctl`) runs it. Exit 3.
func lockBusyFailure() -> CLIFailure {
    CLIFailure.library(.removalInUse, exit: .busy, details: ["Блокировка каталога машины занята другим процессом"])
}

// MARK: description of a bundle (list, inspect)

/// `valid`, `incompatible`, `invalid`, `missing` (no directory) or `not_a_directory`.
func statusName(_ status: CatalogBundleStatus) -> String {
    switch status {
    case .directoryMissing: return "missing"
    case .notADirectory: return "not_a_directory"
    case .config(let state):
        switch state.kind {
        case .valid: return "valid"
        case .incompatible: return "incompatible"
        case .invalid: return "invalid"
        }
    }
}

/// The title of the catalog entry for a bundle that is not `valid`; `nil` for a valid one.
func statusProblem(_ status: CatalogBundleStatus) -> String? {
    LibraryIssue(status).map { ErrorCatalog.entry(for: $0).title }
}
