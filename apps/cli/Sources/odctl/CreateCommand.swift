import Darwin
import Foundation
import ODShared

// `odctl create` (S11-13): the order of VM_BUNDLE.md 5.1, the same as `CreateMachineExecutor` of the application (S11-08b), written for a
// process without a window. The executor lives in the application target (it uses its error presentation and `Combine`); the rules are
// repeated here on the shared types and the differences are listed in docs/specs/CLI.md section 7.
//
//  1. `mkdir <name>.odvm` (0700); an existing name is an error (no numeric suffix, unlike VM_BUNDLE 2 item 4) and the existing directory is
//     never touched;
//  2. `disks/ logs/ firmware/ tpm/ snapshots/`, then `lock` taken by `BundleWriterLock`;
//  3. with `--disk-gib`: the lock is released, `od-vmhost` starts (it takes the lock itself), `CONFIGURE` WITHOUT `disks`, `PREPARE_DISK`, `STOP`,
//     the process exits. Without it no VMHost is started;
//  4. `config.plist` with `disks`, the last file: a bundle without it is not a machine.
// A failure leaves what exists on disk and says where it is: nothing is deleted without the user (the application asks in a dialog; the CLI
// has no dialog). The CLI creates one disk, `disks/disk0.img`, RAW.

let diskImageName = "disk0.img"
let creationSubdirectories = ["disks", "logs", "firmware", "tpm", "snapshots"]

/// VM_BUNDLE 3.2: 1...128 bytes of UTF-8, no control characters, not only spaces.
func validateMachineName(_ name: String) throws {
    let bytes = name.utf8.count
    if bytes == 0 { throw CLIFailure.usage("Имя машины пустое") }
    if bytes > BundleConfigLimits.maxNameBytes {
        throw CLIFailure.usage("Имя слишком длинное: \(bytes) байт в UTF-8, допустимо не больше \(BundleConfigLimits.maxNameBytes)")
    }
    if name.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F }) {
        throw CLIFailure.usage("В имени есть управляющий символ")
    }
    if name.trimmingCharacters(in: .whitespaces).isEmpty { throw CLIFailure.usage("Имя состоит только из пробелов") }
}

private func filesFailure(_ code: Int32, path: String) -> CLIFailure {
    CLIFailure.library(LibraryIssue(errno: code), details: ["errno: \(code) (\(String(cString: strerror(code))))", "Путь: \(path)"])
}

/// What remains after a failed creation, for the details: the directory and the names inside.
private func leftoverDetails(_ bundle: URL, created: Bool) -> [String] {
    guard created else { return [] }
    var lines = ["Каталог машины: \(bundle.path)"]
    let fm = FileManager.default
    var names: [String] = []
    for name in (try? fm.contentsOfDirectory(atPath: bundle.path))?.sorted() ?? [] {
        let inner = (try? fm.contentsOfDirectory(atPath: bundle.appendingPathComponent(name).path)) ?? []
        if name == "disks", !inner.isEmpty {
            names.append(contentsOf: inner.sorted().map { "disks/\($0)" })
        } else {
            names.append(name)
        }
    }
    if !names.isEmpty { lines.append("В каталоге машины остались: " + names.joined(separator: ", ")) }
    return lines
}

func runCreate(_ parsed: ParsedArguments) throws -> Int32 {
    let json = parsed.flag("--json")
    let name = try require(parsed, "--name")
    try validateMachineName(name)
    let profileText = try require(parsed, "--profile")
    guard let profile = MachineProfile(rawValue: profileText) else {
        throw CLIFailure.usage("Неизвестный профиль «\(profileText)»", details: ["Профили: " + MachineProfile.allCases.map(\.rawValue).joined(separator: ", ")])
    }
    let memoryMiB = try integerOption(try require(parsed, "--memory-mib"), "--memory-mib",
                                      range: BundleConfigLimits.minMemoryMiB...BundleConfigLimits.maxMemoryMiB)
    // `--disk-gib` is a whole number of GiB (a multiple of 512 bytes, not zero, within Int64: STORAGE_HOST_ABI_V1.md).
    let diskGiB = try parsed.value("--disk-gib").map { try integerOption($0, "--disk-gib", range: 1...(1 << 20)) }

    var config = BundleConfig(name: name, machineProfile: profile)
    config.memoryMiB = memoryMiB
    let creationConfigure = config.controlConfigure() // before the disk is added: PREPARE_DISK runs in a session without `disks`
    // The checks of VM_BUNDLE 3.5 for the machine itself (the disk is not part of it: it does not start before S9-06).
    if let refusal = config.launchRefusal(), case .hostMemoryExceeded = refusal { throw CLIFailure.refusal(refusal) }
    var vmhost: String?
    if diskGiB != nil { vmhost = try locateVMHost(option: parsed.value("--vmhost")) } // before anything is created
    var capacityBytes: UInt64?
    if let diskGiB {
        capacityBytes = UInt64(diskGiB) << 30
        config.disks = [BundleDisk(location: .inBundle(name: diskImageName))]
    }

    let catalog = makeCatalog(library: parsed.value("--library"))
    let bundle = catalog.defaultDirectory.appendingPathComponent(BundleNaming.directoryName(for: name), isDirectory: true)
    var created = false
    do {
        try createDirectories(bundle: bundle, created: &created)
        if let vmhost, let capacityBytes {
            try prepareDisk(vmhost: vmhost, bundle: bundle, configure: creationConfigure, capacity: capacityBytes)
        }
        try writeConfig(config, bundle: bundle)
    } catch var failure as CLIFailure {
        failure.details += leftoverDetails(bundle, created: created)
        throw failure
    }
    // The machine is created in the default directory of the library, where the catalog finds it by scanning. It is added to the index too
    // when there is one, so that a machine of a library with an index is listed. A failure to write the index is a warning: the machine exists.
    var warnings: [String] = []
    catalog.load()
    do {
        _ = try catalog.add(bundle: bundle)
    } catch {
        let issue = (error as? CatalogError).map(LibraryIssue.init) ?? .indexWriteFailed
        let entry = ErrorCatalog.entry(for: issue)
        warnings.append(entry.title + ". " + entry.explanation + " " + entry.action)
    }
    if json {
        Output.json([
            "command": "create", "ok": true, "name": name, "path": bundle.path, "profile": profile.rawValue, "memory_mib": memoryMiB,
            "disk_bytes": capacityBytes.map { Int64($0) as Any } ?? NSNull(), "vmhost_started": diskGiB != nil, "warnings": warnings,
        ])
    } else {
        Output.stdout("Машина создана: \(name) (test-only, без гостя)")
        Output.stdout("Каталог: \(bundle.path)")
        if let diskGiB { Output.stdout("Диск: disks/\(diskImageName), \(diskGiB) ГиБ (машина с диском не запускается до этапа 9)") }
        for warning in warnings { Output.stderr("Предупреждение: \(warning)") }
    }
    return ExitCode.success.rawValue
}

// MARK: steps 1 and 2

/// Takes `lock` and releases it at once: step 3 closes the descriptor before a VMHost starts, step 4 takes the lock again inside the writer.
private func createDirectories(bundle: URL, created: inout Bool) throws {
    do {
        try FileManager.default.createDirectory(at: bundle.deletingLastPathComponent(), withIntermediateDirectories: true)
    } catch {
        throw CLIFailure.library(.fileSystemError, details: ["\(error)", "Путь: \(bundle.deletingLastPathComponent().path)"])
    }
    if Darwin.mkdir(bundle.path, 0o700) != 0 {
        let code = errno
        if code == EEXIST { throw CLIFailure.library(.creationDirectoryExists, details: ["Путь: \(bundle.path)"]) }
        throw filesFailure(code, path: bundle.path)
    }
    created = true
    for name in creationSubdirectories {
        let path = bundle.appendingPathComponent(name, isDirectory: true).path
        if Darwin.mkdir(path, 0o700) != 0 { throw filesFailure(errno, path: path) }
    }
    let lock: BundleWriterLock
    do {
        lock = try BundleWriterLock.acquire(bundle: bundle)
    } catch let failure as BundleWriterLockFailure {
        throw CLIFailure.library(.creationLockFailed, details: [failure.description])
    }
    lock.release()
}

// MARK: step 3

private func prepareDisk(vmhost: String, bundle: URL, configure: ControlV2Configure, capacity: UInt64) throws {
    let session: HostSession
    do {
        session = try HostSession(vmhost: vmhost, bundle: bundle)
    } catch {
        throw CLIFailure.client(.spawnFailed, details: ["\(error)"])
    }
    func fail(_ failure: CLIFailure) -> CLIFailure {
        session.finish()
        return failure
    }
    switch session.waitReady(timeout: 30) {
    case .ready?: break
    case .ended(let end)?: throw failure(ofEnd: end)
    case nil: throw fail(CLIFailure.client(.notReady))
    }
    if case .failure(let error) = session.send(.configure(configure)) { throw fail(failure(of: error, session: session)) }
    // The disk may be large: the image is created by the VMHost (sparse), the wait is long enough for a slow volume.
    let prepare = ControlV2PrepareDisk(capacityBytes: capacity, name: Array(diskImageName.utf8))
    if case .failure(let error) = session.send(.prepareDisk(prepare), timeout: 300) { throw fail(failure(of: error, session: session)) }
    if case .failure(let error) = session.send(.stop(operationID: 0)) { throw fail(failure(of: error, session: session)) }
    guard let end = session.finish(sendStop: false) else { throw CLIFailure.client(.connectionClosed) }
    if let failure = CLIFailure.processEnd(end) { throw failure }
    guard end.termination == .exited(0) else {
        // A normal end that nobody asked for cannot be told from here: the disk is not known to exist.
        throw CLIFailure.client(.connectionClosed, details: ["Процесс машины завершился без ответа на остановку"])
    }
}

// MARK: step 4

private func writeConfig(_ config: BundleConfig, bundle: URL) throws {
    do {
        try BundleConfigStore.write(config, bundle: bundle, mode: .create)
    } catch BundleConfigWriteError.lock(.busy) {
        // VM_BUNDLE 5.1 step 4: "повторите"; nothing is deleted (a false `busy` does not prove that the disk is not needed).
        throw CLIFailure.library(.creationRetryNeeded, exit: .busy)
    } catch BundleConfigWriteError.lock(let lockFailure) {
        throw CLIFailure.library(.creationLockFailed, details: [lockFailure.description])
    } catch BundleConfigWriteError.alreadyExists, BundleConfigWriteError.currentFileNotValid, BundleConfigWriteError.invalidConfig {
        throw CLIFailure.library(.creationConfigConflict)
    } catch BundleConfigWriteError.io(let code) {
        throw filesFailure(code, path: bundle.appendingPathComponent(BundleConfigLimits.fileName).path)
    } catch {
        throw CLIFailure.library(.fileSystemError, details: ["\(error)"])
    }
}
