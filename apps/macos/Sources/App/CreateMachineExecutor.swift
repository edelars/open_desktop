import Combine
import Darwin
import Foundation
import ODShared

// Execution of the plan of the "Create machine" wizard (S11-08b, docs/specs/VM_BUNDLE.md 5.1, docs/plans/STAGE_11.md section 7).
// No UI framework: the wizard window only shows `phase`; the order of the steps and the rollback are tested without windows. Main queue
// only (the client delivers its events and answers on `.main`).
//
// Order of VM_BUNDLE 5.1 (`config.plist` is the LAST file, so a bundle without it is not a machine):
//  1. `mkdir <name>.odvm` (0700); an existing name is an error, an existing directory is never touched;
//  2. the directories `disks/ logs/ firmware/ tpm/ snapshots/`, then the file `lock`, taken by `BundleWriterLock` (2.1);
//  3. a machine with a disk: the descriptor of `lock` is CLOSED, `od-vmhost --control-v2 --bundle <path>` starts (it takes the lock
//     itself), `CONFIGURE` WITHOUT `disks`, `PREPARE_DISK disk0.img`, `STOP`, the process exits. A machine without a disk starts no VMHost;
//  4. `config.plist` with `disks`: the lock is taken again (inside `BundleConfigStore.write`, `.create`), the file is written atomically.
//
// Rollback (VM_BUNDLE 5.1 "Откат"): only what THIS run created before the lock (the directories of steps 1-2), by `rmdir` (an object that
// is not empty stays), and only when the lock is taken by this process again; if it cannot be taken (`busy`, `lock_changed`, `invalid`)
// nothing is deleted at all. The rollback NEVER deletes `lock` and the disk image, so the bundle directory itself stays while they are in
// it: the user removes it in Finder. A `busy` at step 4 is "повторите" (`retryWriteConfig`): every created file stays. Nothing is
// deleted without the confirmation of the user: `rollback()` is called by the window after its dialog. The default directory (the parent
// of the bundle) is created when it is missing and is not part of the rollback: it is the library root, not a part of the machine.

/// The part of `ControlClientV2` that the creation session uses. The tests put a fake in its place.
protocol CreationSession: AnyObject {
    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion)
    func requestPrepareDisk(_ prepare: ControlV2PrepareDisk, completion: @escaping ControlClientV2.Completion)
    func requestStop(completion: @escaping ControlClientV2.Completion)
    func terminateSession()
}

extension ControlClientV2: CreationSession {
    func requestPrepareDisk(_ prepare: ControlV2PrepareDisk, completion: @escaping Completion) {
        send(.prepareDisk(prepare), completion: completion)
    }
}

typealias CreationLauncher = (ControlClientLaunch, @escaping ControlClientV2.EventHandler) throws -> CreationSession

let defaultCreationLauncher: CreationLauncher = { launch, handler in
    try ControlClientV2.launch(launch, deliveryQueue: .main, eventHandler: handler)
}

/// A failure for the screen: the words come from `ErrorCatalog` through `ErrorPresentation` (S11-09).
struct CreateMachineFailure: Equatable {
    enum Step: Equatable { case preparation, directories, lock, hostStart, configure, prepareDisk, stop, writeConfig }

    var step: Step
    /// "Не удалось создать машину: <title of the entry>."
    var headline: String
    /// The explanation and the action of the catalog entry.
    var hint: String
    var details: String?
    /// `busy` at step 4: the files stay and the user repeats the write (`retryWriteConfig`).
    var canRetry: Bool
    /// Whether anything of this run may be offered for deletion: directories this run created, a lock that is not lost.
    var canRollback: Bool

    init(step: Step, presentation: ErrorPresentation, canRetry: Bool = false, canRollback: Bool = false, extraDetails: [String] = []) {
        self.step = step
        headline = "Не удалось создать машину: \(presentation.fragment)."
        hint = presentation.message
        details = ([presentation.details] + extraDetails.map { Optional($0) }).compactMap { $0 }.joined(separator: "\n")
        if details?.isEmpty == true { details = nil }
        self.canRetry = canRetry
        self.canRollback = canRollback
    }
}

/// What the rollback did, for the window and the tests. Paths are relative to the default directory's bundle name.
enum CreateMachineRollbackOutcome: Equatable {
    /// Nothing to offer: no failure, nothing created, a retry is the way, or the rollback was done already.
    case notAllowed
    /// The lock could not be taken again by this process: nothing was deleted (VM_BUNDLE 5.1).
    case refused(lock: BundleWriterLockFailure)
    /// `removed`: the empty directories deleted; `remaining`: what is left in the bundle directory (never deleted by the application).
    case done(removed: [String], remaining: [String])
}

final class CreateMachineExecutor: ObservableObject {
    enum Stage: Equatable { case directories, startingHost, configuring, preparingDisk, stopping, writingConfig }

    enum Phase: Equatable {
        case idle
        case running(Stage)
        case succeeded
        case failed(CreateMachineFailure)
    }

    /// `ControlV2Kind.prepareDisk` as the `request_kind` of `PROGRESS`.
    private static let prepareDiskKind = UInt32(ControlV2Kind.prepareDisk.rawValue)
    /// Names of the directories of VM_BUNDLE 5.1 step 2, in the order of creation.
    static let subdirectories = ["disks", "logs", "firmware", "tpm", "snapshots"]

    let plan: CreateMachinePlan
    let vmhost: VMHostLocation

    @Published private(set) var phase: Phase = .idle
    /// `PROGRESS` of `PREPARE_DISK`: completed and total bytes; nil before the first event.
    @Published private(set) var progress: (completed: UInt64, total: UInt64)?
    @Published private(set) var rollbackOutcome: CreateMachineRollbackOutcome?

    /// The directories this run created, in order of creation (before the lock). The only objects the rollback may remove.
    private(set) var createdDirectories: [URL] = []
    /// The file `lock` was created by this run (it is never deleted).
    private(set) var lockCreated = false

    private let launcher: CreationLauncher
    private var lock: BundleWriterLock?
    private var session: CreationSession?
    private var pendingFailure: CreateMachineFailure?
    private var stopAcknowledged = false
    private var started = false

    init(plan: CreateMachinePlan, vmhost: VMHostLocation, launcher: @escaping CreationLauncher = defaultCreationLauncher) {
        self.plan = plan
        self.vmhost = vmhost
        self.launcher = launcher
    }

    var isRunning: Bool {
        if case .running = phase { return true }
        return false
    }

    var bundleURL: URL { plan.bundleURL }

    // MARK: run

    /// Starts the creation. Once: a second call does nothing.
    func start() {
        guard !started else { return }
        started = true
        if plan.disk != nil, !vmhost.isExecutable {
            // Before anything is created: no directory exists, nothing is left to remove.
            fail(.preparation, ErrorPresentation.client(.vmhostMissing, technical: "Путь: \(vmhost.url.path)", context: errorContext()))
            return
        }
        phase = .running(.directories)
        guard createDirectories() else { return }
        // VM_BUNDLE 5.1 step 3 (a machine with a disk) closes the descriptor before VMHost; step 4 takes the lock inside the writer.
        lock?.release()
        lock = nil
        if plan.disk != nil {
            startHost()
        } else {
            writeConfig()
        }
    }

    private func errorContext(requestID: UInt32? = nil) -> ErrorContext {
        ErrorContext(requestID: requestID, bundleURL: plan.bundleURL)
    }

    // MARK: steps 1 and 2

    private func createDirectories() -> Bool {
        let parent = plan.bundleURL.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        } catch {
            fail(.directories, filePresentation(BundleRemovalError.from(error)))
            return false
        }
        if !makeDirectory(plan.bundleURL, existingIsConflict: true) { return false }
        for name in Self.subdirectories where !makeDirectory(plan.bundleURL.appendingPathComponent(name, isDirectory: true), existingIsConflict: false) {
            return false
        }
        do {
            lock = try BundleWriterLock.acquire(bundle: plan.bundleURL)
            lockCreated = true
        } catch let failure as BundleWriterLockFailure {
            // The file `lock` may exist now (the open creates it before the flock): it is not ours to delete, the rollback is not offered.
            lockCreated = true
            fail(.lock, ErrorPresentation.library(.creationLockFailed, technical: failure.description, context: errorContext()))
        } catch {
            fail(.lock, ErrorPresentation.library(.creationLockFailed, technical: "\(error)", context: errorContext()))
        }
        return lock != nil
    }

    /// `mkdir` with 0700: it never succeeds on an existing object, so a directory that was there before is never claimed or removed.
    private func makeDirectory(_ url: URL, existingIsConflict: Bool) -> Bool {
        if Darwin.mkdir(url.path, 0o700) == 0 {
            createdDirectories.append(url)
            return true
        }
        let code = errno
        if code == EEXIST, existingIsConflict {
            fail(.directories, ErrorPresentation.library(.creationDirectoryExists, technical: "Путь: \(url.path)", context: errorContext()))
        } else {
            fail(.directories, filePresentation(.io(code), path: url.path))
        }
        return false
    }

    private func filePresentation(_ error: BundleRemovalError, path: String? = nil) -> ErrorPresentation {
        var technical = [error.details, path.map { "Путь: \($0)" }].compactMap { $0 }
        if technical.isEmpty { technical = ["\(error)"] }
        return ErrorPresentation.library(error.issue, technical: technical.joined(separator: "\n"), context: errorContext())
    }

    // MARK: step 3 (the VMHost session)

    private func startHost() {
        phase = .running(.startingHost)
        let launch = ControlClientLaunch(vmhostPath: vmhost.url.path, bundlePath: plan.bundleURL.path, displayChannel: false)
        do {
            session = try launcher(launch) { [weak self] event in self?.handle(event) }
        } catch {
            fail(.hostStart, ErrorPresentation.client(.spawnFailed, technical: "\(error)", context: errorContext()))
        }
    }

    private func handle(_ event: ControlClientEvent) {
        switch event {
        case .ready:
            sendConfigure()
        case .progress(let kind, let completed, let total):
            if kind == Self.prepareDiskKind { progress = (completed, total) }
        case .startRejected(let rejection):
            // The connection ends and `exited` follows: the failure is told there, together with the end of the process.
            pendingFailure = pendingFailure ?? failure(.hostStart, ErrorPresentation.rejection(rejection, context: errorContext()))
        case .error(let report, let requestID):
            // An `ERROR` that answers a request is handled by the completion of that request; one that is not caused by a request is the cause.
            if requestID == 0 {
                pendingFailure = pendingFailure ?? failure(currentStep, ErrorPresentation.control(report, context: errorContext(requestID: requestID)))
            }
        case .protocolViolation(let text):
            pendingFailure = pendingFailure
                ?? failure(currentStep, ErrorPresentation.client(.protocolViolation, technical: text, context: errorContext()))
        case .stateChanged, .stats:
            break
        case .exited(let exit):
            sessionEnded(exit)
        }
    }

    private var currentStep: CreateMachineFailure.Step {
        switch phase {
        case .running(.configuring): return .configure
        case .running(.preparingDisk): return .prepareDisk
        case .running(.stopping): return .stop
        case .running(.writingConfig): return .writeConfig
        default: return .hostStart
        }
    }

    private func sendConfigure() {
        guard let session else { return }
        phase = .running(.configuring)
        // `CONFIGURE` of the creation session has no `disks`: the image does not exist yet (VM_BUNDLE 5.1 step 3).
        session.requestConfigure(plan.creationConfigure) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success: self.sendPrepareDisk()
            case .failure(let error): self.requestFailed(error, step: .configure)
            }
        }
    }

    private func sendPrepareDisk() {
        guard let session, let disk = plan.disk else { return }
        phase = .running(.preparingDisk)
        let request = ControlV2PrepareDisk(capacityBytes: disk.capacityBytes, name: Array(disk.name.utf8))
        session.requestPrepareDisk(request) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success: self.sendStop()
            case .failure(let error): self.requestFailed(error, step: .prepareDisk)
            }
        }
    }

    private func sendStop() {
        guard let session else { return }
        phase = .running(.stopping)
        session.requestStop { [weak self] result in
            guard let self else { return }
            switch result {
            case .success: self.stopAcknowledged = true // the process exits by itself; the exit is what ends the step
            case .failure(let error): self.requestFailed(error, step: .stop)
            }
        }
    }

    /// A refused or failed request of the session. The first cause is kept. The session is closed with `STOP` (valid in `stopped`) when
    /// the connection is alive; the failure is told when the process has exited, so the state of the files is settled by then.
    private func requestFailed(_ error: ControlClientError, step: CreateMachineFailure.Step) {
        guard let session else { return }
        switch error {
        case .connectionClosed:
            return // `exited` follows and says why
        case .refused(let report):
            pendingFailure = pendingFailure ?? failure(step, ErrorPresentation.control(report, context: errorContext()))
            if report.fatal { return } // the VMHost closes the connection itself
            if step == .stop {
                session.terminateSession()
            } else {
                session.requestStop { result in
                    if case .failure = result { session.terminateSession() }
                }
            }
        default:
            pendingFailure = pendingFailure
                ?? failure(step, ErrorPresentation.client(ClientIssue(error), technical: "\(error)", context: errorContext()))
            session.terminateSession()
        }
    }

    private func sessionEnded(_ exit: ControlClientExit) {
        session = nil
        if let pendingFailure {
            fail(pendingFailure)
            return
        }
        if exit.startRejection == nil, stopAcknowledged, exit.termination == .exited(0) {
            writeConfig()
            return
        }
        if let presentation = ErrorPresentation.exit(exit, context: errorContext()) {
            fail(.hostStart, presentation)
        } else {
            // A normal end that nobody asked for (no `STOP` answered): the disk is not known to exist.
            fail(.hostStart, ErrorPresentation.client(.connectionClosed, technical: "Процесс машины завершился без ответа на остановку", context: errorContext()))
        }
    }

    // MARK: step 4

    private func writeConfig() {
        phase = .running(.writingConfig)
        do {
            try BundleConfigStore.write(plan.config, bundle: plan.bundleURL, mode: .create)
            phase = .succeeded
        } catch BundleConfigWriteError.lock(.busy) {
            // VM_BUNDLE 5.1 step 4: "повторите"; nothing is deleted (a false `busy` does not prove that the disk is not needed).
            fail(.writeConfig, ErrorPresentation.library(.creationRetryNeeded, context: errorContext()), canRetry: true, canRollback: false)
        } catch BundleConfigWriteError.lock(let lockFailure) {
            fail(.writeConfig, ErrorPresentation.library(.creationLockFailed, technical: lockFailure.description, context: errorContext()), canRollback: false)
        } catch BundleConfigWriteError.alreadyExists, BundleConfigWriteError.currentFileNotValid, BundleConfigWriteError.invalidConfig {
            // The directory now holds a configuration that is not ours to replace: nothing is rolled back (VM_BUNDLE 5.1, 5.2).
            fail(.writeConfig, ErrorPresentation.library(.creationConfigConflict, context: errorContext()), canRollback: false)
        } catch BundleConfigWriteError.io(let code) {
            fail(.writeConfig, filePresentation(.io(code)))
        } catch {
            fail(.writeConfig, ErrorPresentation.library(.fileSystemError, technical: "\(error)", context: errorContext()))
        }
    }

    /// "Повторить" after `busy` at step 4: only the write of `config.plist` is repeated; the directories and the image are as they were.
    func retryWriteConfig() {
        guard case .failed(let current) = phase, current.canRetry else { return }
        writeConfig()
    }

    // MARK: failures

    private func failure(
        _ step: CreateMachineFailure.Step, _ presentation: ErrorPresentation, canRetry: Bool = false, canRollback: Bool? = nil
    ) -> CreateMachineFailure {
        // What remains on the disk after the failure goes to the details; the rollback is offered when something of this run can go.
        CreateMachineFailure(
            step: step, presentation: presentation, canRetry: canRetry,
            canRollback: canRollback ?? (!createdDirectories.isEmpty && step != .lock),
            extraDetails: leftovers())
    }

    private func fail(_ step: CreateMachineFailure.Step, _ presentation: ErrorPresentation, canRetry: Bool = false, canRollback: Bool? = nil) {
        fail(failure(step, presentation, canRetry: canRetry, canRollback: canRollback))
    }

    private func fail(_ failure: CreateMachineFailure) {
        lock?.release()
        lock = nil
        session = nil
        phase = .failed(failure)
    }

    /// The names that exist in the bundle directory now (a line for the details): the user sees what stays.
    private func leftovers() -> [String] {
        guard !createdDirectories.isEmpty else { return [] }
        var lines = ["Каталог машины: \(plan.bundleURL.path)"]
        let names = remainingEntries()
        if !names.isEmpty { lines.append("В каталоге машины остались: " + names.joined(separator: ", ")) }
        return lines
    }

    /// Names in the bundle directory and the files inside `disks/` (the image is the file that matters).
    private func remainingEntries() -> [String] {
        let fm = FileManager.default
        var result: [String] = []
        for name in (try? fm.contentsOfDirectory(atPath: plan.bundleURL.path))?.sorted() ?? [] {
            let inner = (try? fm.contentsOfDirectory(atPath: plan.bundleURL.appendingPathComponent(name).path)) ?? []
            if name == "disks", !inner.isEmpty {
                result.append(contentsOf: inner.sorted().map { "disks/\($0)" })
            } else {
                result.append(name)
            }
        }
        return result
    }

    // MARK: rollback (after the confirmation of the user)

    /// Deletes what this run created before the lock and only that: the empty directories, in reverse order. Needs the lock taken by this
    /// process again (when the file `lock` is in the bundle); without it nothing is deleted. Never deletes `lock`, the image or anything
    /// that was not created by this run. The window calls it after "Удалить частично созданную машину?" is answered "Удалить".
    @discardableResult
    func rollback() -> CreateMachineRollbackOutcome {
        guard case .failed(let current) = phase, current.canRollback, rollbackOutcome == nil, !createdDirectories.isEmpty else {
            return .notAllowed
        }
        var held: BundleWriterLock?
        if lockCreated {
            do {
                held = try BundleWriterLock.acquire(bundle: plan.bundleURL)
            } catch let failure as BundleWriterLockFailure {
                let outcome = CreateMachineRollbackOutcome.refused(lock: failure)
                rollbackOutcome = outcome
                return outcome
            } catch {
                let outcome = CreateMachineRollbackOutcome.refused(lock: .io(0))
                rollbackOutcome = outcome
                return outcome
            }
        }
        defer { held?.release() }
        var removed: [String] = []
        for directory in createdDirectories.reversed() {
            // `rmdir` removes an empty directory only: the one that holds `lock` or an image stays.
            if Darwin.rmdir(directory.path) == 0 { removed.append(relativeName(directory)) }
        }
        let outcome = CreateMachineRollbackOutcome.done(removed: removed.reversed(), remaining: remainingEntries())
        rollbackOutcome = outcome
        return outcome
    }

    private func relativeName(_ url: URL) -> String {
        url == plan.bundleURL ? url.lastPathComponent : url.lastPathComponent + "/"
    }

    /// The words of the confirmation dialog: exactly what will be removed and what will stay.
    var rollbackDescription: String {
        let removable = createdDirectories.reversed().map { relativeName($0) }
        var lines = ["Будут удалены только пустые каталоги, созданные при этой попытке: " + removable.joined(separator: ", ") + "."]
        lines.append("Файл блокировки и образ диска приложение не удаляет: если они есть, каталог «\(plan.bundleURL.lastPathComponent)» останется, и его можно убрать вручную в Finder.")
        lines.append("Существующие файлы и каталоги не затрагиваются.")
        return lines.joined(separator: " ")
    }
}
