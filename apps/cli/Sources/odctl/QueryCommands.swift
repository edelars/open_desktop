import Darwin
import Foundation
import ODShared

// `list`, `inspect` and `delete` (S11-13). They work on the files of a bundle and on the index; none of them starts a VMHost.

func refusalName(_ refusal: LaunchRefusal) -> String {
    switch refusal {
    case .unsupportedDurability: return ControlV2ErrorCode.unsupportedDurability.wireName
    case .unsupported: return ControlV2ErrorCode.unsupported.wireName
    case .hostMemoryExceeded: return ControlV2ErrorCode.hostMemoryExceeded.wireName
    }
}

func lockName(_ failure: BundleWriterLockFailure?) -> String {
    switch failure {
    case nil: return "free"
    case .busy?: return "busy"
    case .lockChanged?, .invalid?: return "invalid"
    case .io?: return "error"
    }
}

// MARK: list

func runList(_ parsed: ParsedArguments) throws -> Int32 {
    let json = parsed.flag("--json")
    let catalog = makeCatalog(library: parsed.value("--library"))
    catalog.load()
    let warnings = catalog.warnings.map(warningText)
    var machines: [[String: Any]] = []
    var lines: [String] = []
    for entry in catalog.entries {
        let status = catalog.status(of: entry)
        var machine: [String: Any] = ["path": entry.bundleURL.path, "status": statusName(status)]
        var name = directoryStem(entry.bundleURL)
        if case .config(let state) = status, let config = state.config {
            name = config.name
            machine["name"] = config.name
            machine["profile"] = config.machineProfile.rawValue
            machine["memory_mib"] = config.memoryMiB
            machine["disks"] = config.disks.count
        } else {
            machine["name"] = NSNull()
            machine["problem"] = statusProblem(status) ?? NSNull()
        }
        machines.append(machine)
        let profile = machine["profile"] as? String ?? "-"
        let memory = (machine["memory_mib"] as? Int).map { "\($0) МиБ" } ?? "-"
        lines.append("\(name)\t\(statusName(status))\t\(profile)\t\(memory)\t\(entry.bundleURL.path)")
    }
    if json {
        Output.json([
            "command": "list", "ok": true, "library": catalog.defaultDirectory.path, "machines": machines,
            "warnings": warnings.map { ["code": $0.code, "message": $0.text] },
        ])
    } else {
        for warning in warnings { Output.stderr("Предупреждение: \(warning.text)") }
        if lines.isEmpty {
            Output.stdout("Машин нет: \(catalog.defaultDirectory.path)")
        } else {
            Output.stdout("ИМЯ\tСОСТОЯНИЕ КАТАЛОГА\tПРОФИЛЬ\tПАМЯТЬ\tПУТЬ")
            lines.forEach(Output.stdout)
        }
    }
    return ExitCode.success.rawValue
}

// MARK: inspect

func runInspect(_ parsed: ParsedArguments) throws -> Int32 {
    let json = parsed.flag("--json")
    let resolved = try resolveBundle(parsed.positional[0], library: parsed.value("--library"))
    let state = resolved.state
    let status = CatalogBundleStatus.config(state)
    let lock = BundleWriterLock.probeFailure(bundle: resolved.url)
    var machine: [String: Any] = [
        "path": resolved.url.path, "status": statusName(status), "listed": resolved.entry != nil, "lock": lockName(lock),
    ]
    var lines: [String] = ["Каталог: \(resolved.url.path)", "Состояние каталога: \(statusName(status))", "Блокировка: \(lockName(lock))"]
    if let config = state.config {
        machine["name"] = config.name
        machine["identity"] = config.identity
        machine["profile"] = config.machineProfile.rawValue
        machine["memory_mib"] = config.memoryMiB
        machine["vcpu_count"] = config.vcpuCount
        machine["display_screens"] = config.displayScreens
        machine["network"] = config.network.rawValue
        machine["audio"] = config.audio
        machine["clipboard_sync"] = config.clipboardSync
        machine["boot_media"] = config.bootMedia != nil
        let refusal = config.launchRefusal()
        machine["launch_refusal"] = refusal.map(refusalName) ?? NSNull()
        var disks: [[String: Any]] = []
        for disk in config.disks {
            var item: [String: Any] = ["format": disk.format.rawValue, "writable": disk.writable]
            switch disk.location {
            case .inBundle(let name):
                item["location"] = "in_bundle"
                item["name"] = name
                var info = stat()
                let path = resolved.url.appendingPathComponent("disks").appendingPathComponent(name).path
                item["size_bytes"] = lstat(path, &info) == 0 ? Int64(info.st_size) : NSNull()
            case .external:
                item["location"] = "external"
            }
            disks.append(item)
        }
        machine["disks"] = disks
        lines.insert(contentsOf: [
            "Имя: \(config.name)", "Профиль: \(config.machineProfile.rawValue)", "Память: \(config.memoryMiB) МиБ",
            "Процессоров: \(config.vcpuCount)", "Экранов: \(config.displayScreens)", "Дисков: \(config.disks.count)",
        ], at: 0)
        if let refusal {
            let entry = refusalEntry(refusal)
            lines.append("Запуск: отказ «\(entry.title)». \(entry.explanation) \(entry.action)")
        }
    } else {
        machine["name"] = NSNull()
        let problem = statusProblem(status)
        machine["problem"] = problem ?? NSNull()
        lines.append("Проблема: \(problem ?? "неизвестна")")
    }
    if json {
        Output.json(["command": "inspect", "ok": true, "machine": machine])
    } else {
        lines.forEach(Output.stdout)
    }
    return ExitCode.success.rawValue
}

private func refusalEntry(_ refusal: LaunchRefusal) -> ErrorEntry { ErrorCatalog.entry(for: refusal) }

// MARK: delete

func runDelete(_ parsed: ParsedArguments) throws -> Int32 {
    let json = parsed.flag("--json")
    let deleteFiles = parsed.flag("--files")
    let confirm = try require(parsed, "--confirm")
    var resolved = try resolveBundle(parsed.positional[0], library: parsed.value("--library"))
    if resolved.entry == nil {
        // A path: the machine may still be listed in the index, and a deleted directory must not stay in it.
        let catalog = makeCatalog(library: parsed.value("--library"))
        catalog.load()
        if let listed = catalog.entries.first(where: { $0.bundleURL.standardizedFileURL.path == resolved.url.path }) {
            resolved.entry = listed
            resolved.catalog = catalog
        }
    }
    let expected = confirmationName(for: resolved)
    guard confirm == expected else {
        throw CLIFailure.usage("Подтверждение не совпадает с именем машины", details: ["Повторите имя машины в --confirm: «\(expected)»"])
    }
    if deleteFiles {
        // The check of the lock and the deletion are under one lock inside `removeBundleFiles`: busy is exit 3, disk data is exit 4, and in
        // both cases nothing is deleted. The disks are looked at whether or not `config.plist` can be read (files of disks are not deleted
        // before S9-06, ISSUE-055; the library entry alone may go).
        try removeBundleFiles(at: resolved.url, config: resolved.state.config)
    } else if BundleWriterLock.probeFailure(bundle: resolved.url) == .busy {
        // Only a stopped machine is taken off the list: a held lock means that another process runs it (the application or another `odctl`).
        throw lockBusyFailure()
    }
    var warnings: [String] = []
    if let entry = resolved.entry, let catalog = resolved.catalog {
        do {
            try catalog.remove(entry)
        } catch CatalogError.notFound {
            // Already out of the index: the goal is reached.
        } catch {
            let issue = (error as? CatalogError).map(LibraryIssue.init) ?? .indexWriteFailed
            let entry = ErrorCatalog.entry(for: issue)
            warnings.append(entry.title + ". " + entry.explanation + " " + entry.action)
        }
    }
    // A path of a machine that is not in the index with no `--files` changes nothing.
    let removedFromIndex = resolved.entry != nil
    let done = deleteFiles ? "Файлы машины удалены" : (removedFromIndex ? "Машина убрана из списка" : "Машины нет в списке, ничего не изменено")
    if json {
        Output.json([
            "command": "delete", "ok": true, "name": expected, "path": resolved.url.path, "files_deleted": deleteFiles,
            "index_removed": removedFromIndex, "warnings": warnings,
        ])
    } else {
        Output.stdout("\(done): \(expected)")
        for warning in warnings { Output.stderr("Предупреждение: \(warning)") }
    }
    return ExitCode.success.rawValue
}
