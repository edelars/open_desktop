import Combine
import Darwin
import Foundation
@testable import ODShared
import XCTest
@testable import OpenDesktop

// Execution of the plan of the "Create machine" wizard (S11-08b, Sources/App/CreateMachineExecutor.swift): the order of VM_BUNDLE 5.1, the
// rollback, `busy` at the write of `config.plist`, cancellation. No window. Scenarios with a fake session cover the refusals and `busy`;
// the scenarios with the real `od-vmhost` (test-only profile, no guest) are skipped when the binary is not built (`swift build` first).
// The default directory is a temporary one. Run: `swift test --filter CreateMachineWizard` from apps/.

private final class FakeCreationSession: CreationSession {
    var onConfigure: (ControlV2Configure) -> Result<ControlV2Reply, ControlClientError> = { _ in .success(.ack(ControlV2Ack(ackedKind: 4, state: 0))) }
    var onPrepare: (ControlV2PrepareDisk) -> Result<ControlV2Reply, ControlClientError> = { _ in .success(.ack(ControlV2Ack(ackedKind: 5, state: 0))) }
    var onStopped: () -> Void = {}
    var handler: ControlClientV2.EventHandler = { _ in }
    private(set) var calls: [String] = []

    private func deliver(_ result: Result<ControlV2Reply, ControlClientError>, to completion: @escaping ControlClientV2.Completion) {
        DispatchQueue.main.async { completion(result) }
    }

    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion) {
        calls.append("configure")
        deliver(onConfigure(configure), to: completion)
    }

    func requestPrepareDisk(_ prepare: ControlV2PrepareDisk, completion: @escaping ControlClientV2.Completion) {
        calls.append("prepare")
        deliver(onPrepare(prepare), to: completion)
    }

    func requestStop(completion: @escaping ControlClientV2.Completion) {
        calls.append("stop")
        onStopped()
        deliver(.success(.ack(ControlV2Ack(ackedKind: 8, state: 0))), to: completion)
        DispatchQueue.main.async { [handler] in
            handler(.exited(ControlClientExit(termination: .exited(0), startRejection: nil)))
        }
    }

    func terminateSession() {
        calls.append("terminate")
        DispatchQueue.main.async { [handler] in
            handler(.exited(ControlClientExit(termination: .signaled(SIGTERM), startRejection: nil)))
        }
    }
}

final class CreateMachineWizardXCTests: XCTestCase {
    private var root: URL!
    private var library: URL!

    override func setUpWithError() throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("create-wizard-\(getpid())-\(UUID().uuidString)", isDirectory: true)
        library = root.appendingPathComponent("Open Desktop", isDirectory: true)
        try FileManager.default.createDirectory(at: library, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    // MARK: helpers

    private func makeModel(name: String) -> CreateMachineModel {
        let environment = CreateMachineEnvironment(hostMemoryBytes: 64 << 30, defaultDirectory: library, availableSpace: { _ in 1 << 40 })
        return CreateMachineModel(environment: environment, name: name)
    }

    private func makePlan(name: String, disk: Bool, capacity: UInt64 = 1 << 20) throws -> CreateMachinePlan {
        let model = makeModel(name: name)
        model.diskEnabled = disk
        model.diskCapacityBytes = capacity
        for _ in 0..<4 { XCTAssertTrue(model.advance()) }
        return try XCTUnwrap(model.confirm())
    }

    private func realVMHost() throws -> VMHostLocation {
        let candidates = [
            Bundle(for: Self.self).bundleURL.deletingLastPathComponent().appendingPathComponent("od-vmhost").path,
            URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent(".build/debug/od-vmhost").path,
        ]
        guard let path = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            throw XCTSkip("od-vmhost is not built next to the tests (run `swift build` first)")
        }
        return VMHostLocation(url: URL(fileURLWithPath: path), source: .argument, isExecutable: true)
    }

    private let missingVMHost = VMHostLocation(url: URL(fileURLWithPath: "/nonexistent/od-vmhost"), source: .argument, isExecutable: false)

    private func fakeExecutor(plan: CreateMachinePlan, session: FakeCreationSession) -> CreateMachineExecutor {
        // The location is "executable" so that the fake launcher is reached; the path is never started.
        let location = VMHostLocation(url: URL(fileURLWithPath: "/fake/od-vmhost"), source: .argument, isExecutable: true)
        return CreateMachineExecutor(plan: plan, vmhost: location) { _, handler in
            session.handler = handler
            DispatchQueue.main.async { handler(.ready) }
            return session
        }
    }

    private func waitUntil(_ what: String, timeout: TimeInterval = 20, _ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        XCTAssertTrue(condition(), "timeout waiting for: \(what)")
    }

    private func isFinished(_ executor: CreateMachineExecutor) -> Bool {
        switch executor.phase {
        case .succeeded, .failed: return true
        default: return false
        }
    }

    private func names(_ directory: URL) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []).sorted()
    }

    private func exists(_ url: URL) -> Bool {
        var info = stat()
        return lstat(url.path, &info) == 0
    }

    /// A pre-existing machine of the user in the default directory: the creation must not touch it.
    private func makeExistingMachine() throws -> (url: URL, config: Data) {
        let url = library.appendingPathComponent("Existing.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try BundleConfigStore.write(BundleConfig(name: "Existing"), bundle: url, mode: .create)
        return (url, try Data(contentsOf: url.appendingPathComponent("config.plist")))
    }

    private func assertUntouched(_ existing: (url: URL, config: Data), file: StaticString = #filePath, line: UInt = #line) throws {
        XCTAssertEqual(try Data(contentsOf: existing.url.appendingPathComponent("config.plist")), existing.config, "the existing machine is not changed", file: file, line: line)
        XCTAssertEqual(names(existing.url), ["config.plist", "lock"], file: file, line: line)
    }

    // MARK: 1. the full scenario with the real VMHost

    func testCreatesAMachineWithADiskThroughTheRealVMHost() throws {
        let vmhost = try realVMHost()
        let existing = try makeExistingMachine()
        let plan = try makePlan(name: "Real Disk", disk: true, capacity: 1 << 20)
        let executor = CreateMachineExecutor(plan: plan, vmhost: vmhost)
        var phases: [CreateMachineExecutor.Phase] = []
        var configSeenEarly = false
        let observer = executor.$phase.sink { phase in
            if phases.last != phase { phases.append(phase) }
            if phase != .running(.writingConfig), phase != .succeeded, FileManager.default.fileExists(atPath: plan.bundleURL.appendingPathComponent("config.plist").path) {
                configSeenEarly = true // VM_BUNDLE 5.1: `config.plist` is the last file
            }
        }
        defer { observer.cancel() }
        var lastProgress: (completed: UInt64, total: UInt64)?
        let progressObserver = executor.$progress.sink { if let value = $0 { lastProgress = value } }
        defer { progressObserver.cancel() }

        executor.start()
        waitUntil("the end of the creation") { isFinished(executor) }
        XCTAssertEqual(executor.phase, .succeeded, "\(executor.phase)")
        XCTAssertFalse(configSeenEarly, "config.plist did not exist before the last step")
        XCTAssertEqual(phases, [.idle, .running(.directories), .running(.startingHost), .running(.configuring), .running(.preparingDisk),
                                .running(.stopping), .running(.writingConfig), .succeeded])
        XCTAssertEqual(lastProgress?.completed, 1 << 20)
        XCTAssertEqual(lastProgress?.total, 1 << 20, "the final PROGRESS: completed == total")

        // The files: the directories, `lock`, the image of the requested size, `config.plist` with `disks`.
        let bundle = plan.bundleURL
        XCTAssertEqual(bundle.lastPathComponent, "Real Disk.odvm")
        XCTAssertTrue(names(bundle).contains("lock"))
        for directory in CreateMachineExecutor.subdirectories { XCTAssertTrue(exists(bundle.appendingPathComponent(directory)), directory) }
        let image = bundle.appendingPathComponent("disks/disk0.img")
        let attributes = try FileManager.default.attributesOfItem(atPath: image.path)
        XCTAssertEqual((attributes[.size] as? NSNumber)?.uint64Value, 1 << 20)
        guard case .valid(let config) = BundleConfigCodec.read(bundle: bundle) else { return XCTFail("config.plist is not valid") }
        XCTAssertEqual(config.name, "Real Disk")
        XCTAssertEqual(config.disks.map(\.location), [.inBundle(name: "disk0.img")])
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundle), "the lock is free: no VMHost, no writer")
        try assertUntouched(existing)

        // The bundle goes to the library.
        let catalog = Catalog(indexURL: root.appendingPathComponent("Library.plist"), defaultDirectory: library)
        let libraryModel = LibraryModel(catalog: catalog, vmhost: vmhost)
        libraryModel.load()
        XCTAssertTrue(libraryModel.open(bundle: bundle))
        XCTAssertTrue(libraryModel.machines.contains { $0.displayName == "Real Disk" })
        XCTAssertEqual(libraryModel.selected?.displayName, "Real Disk")
    }

    func testCreatesAMachineWithoutADiskAndStartsNoVMHost() throws {
        let plan = try makePlan(name: "No Disk", disk: false)
        XCTAssertNil(plan.disk)
        var launched = false
        let executor = CreateMachineExecutor(plan: plan, vmhost: missingVMHost) { _, _ in
            launched = true
            throw NSError(domain: "test", code: 1)
        }
        executor.start()
        XCTAssertEqual(executor.phase, .succeeded, "a machine without a disk needs no VMHost: no run loop is needed")
        XCTAssertFalse(launched, "VM_BUNDLE 5.1 step 3 is skipped")
        guard case .valid(let config) = BundleConfigCodec.read(bundle: plan.bundleURL) else { return XCTFail("config.plist is not valid") }
        XCTAssertTrue(config.disks.isEmpty)
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: plan.bundleURL))
    }

    // MARK: 2. a refusal of PREPARE_DISK: the rollback of this run only

    func testRealVMHostRefusesTheDiskAndTheRollbackRemovesOnlyWhatThisRunCreated() throws {
        let vmhost = try realVMHost()
        let existing = try makeExistingMachine()
        var plan = try makePlan(name: "Refused", disk: true)
        plan.disk?.capacityBytes = 100 // not a multiple of 512: the VMHost answers `invalid_argument`; the model would not let it pass
        let executor = CreateMachineExecutor(plan: plan, vmhost: vmhost)
        executor.start()
        waitUntil("the end of the creation") { isFinished(executor) }
        guard case .failed(let failure) = executor.phase else { return XCTFail("\(executor.phase)") }
        XCTAssertEqual(failure.step, .prepareDisk)
        XCTAssertTrue(failure.headline.hasPrefix("Не удалось создать машину: "), failure.headline)
        XCTAssertNil(failure.headline.range(of: "[a-z]+_[a-z_]+", options: .regularExpression), "no code name in the headline")
        XCTAssertEqual(failure.details?.contains("invalid_argument"), true, "the code is in the details")
        XCTAssertFalse(failure.canRetry)
        XCTAssertTrue(failure.canRollback)
        XCTAssertFalse(exists(plan.bundleURL.appendingPathComponent("config.plist")), "the machine does not exist without config.plist")
        XCTAssertFalse(exists(plan.bundleURL.appendingPathComponent("disks/disk0.img")))
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: plan.bundleURL), "the VMHost has exited")

        // Nothing is deleted before the confirmation: the window calls `rollback()` after its dialog.
        XCTAssertTrue(exists(plan.bundleURL.appendingPathComponent("disks")))
        guard case .done(let removed, let remaining) = executor.rollback() else { return XCTFail("rollback") }
        XCTAssertTrue(removed.contains("disks/") && removed.contains("firmware/") && removed.contains("tpm/") && removed.contains("snapshots/"), "\(removed)")
        XCTAssertFalse(removed.contains(plan.bundleURL.lastPathComponent), "the directory that holds `lock` stays")
        XCTAssertTrue(remaining.contains("lock"), "\(remaining)")
        XCTAssertTrue(exists(plan.bundleURL.appendingPathComponent("lock")), "the rollback never deletes `lock`")
        XCTAssertFalse(exists(plan.bundleURL.appendingPathComponent("disks")))
        try assertUntouched(existing)
        XCTAssertEqual(executor.rollback(), .notAllowed, "once")
        XCTAssertEqual(names(library), ["Existing.odvm", "Refused.odvm"])
    }

    func testDuplicateDiskNameRefusedByAFakeSessionLeavesTheExistingImageAndTheMachineAlone() throws {
        let existing = try makeExistingMachine()
        let plan = try makePlan(name: "Dup", disk: true)
        let session = FakeCreationSession()
        let report = ControlV2ErrorReport(code: .diskExists, failedKind: UInt16(ControlV2Kind.prepareDisk.rawValue), state: ControlV2State.stopped.rawValue)
        session.onPrepare = { _ in .failure(.refused(report)) }
        let executor = fakeExecutor(plan: plan, session: session)
        executor.start()
        waitUntil("the end of the creation") { isFinished(executor) }
        guard case .failed(let failure) = executor.phase else { return XCTFail("\(executor.phase)") }
        XCTAssertEqual(failure.step, .prepareDisk)
        XCTAssertEqual(failure.headline, "Не удалось создать машину: диск с таким именем уже есть.")
        XCTAssertEqual(session.calls, ["configure", "prepare", "stop"], "the session is closed with STOP after a refusal")
        XCTAssertTrue(failure.canRollback)
        XCTAssertEqual(executor.createdDirectories.count, 1 + CreateMachineExecutor.subdirectories.count)
        guard case .done(_, let remaining) = executor.rollback() else { return XCTFail("rollback") }
        XCTAssertEqual(remaining, ["lock"])
        XCTAssertEqual(names(plan.bundleURL), ["lock"])
        try assertUntouched(existing)
    }

    func testRollbackIsRefusedWhenTheLockCannotBeTakenAndDeletesNothing() throws {
        let plan = try makePlan(name: "Held", disk: true)
        let session = FakeCreationSession()
        session.onPrepare = { _ in
            .failure(.refused(ControlV2ErrorReport(code: .ioError, failedKind: 5, state: ControlV2State.stopped.rawValue, aux0: UInt64(EIO))))
        }
        let executor = fakeExecutor(plan: plan, session: session)
        executor.start()
        waitUntil("the end of the creation") { isFinished(executor) }
        guard case .failed = executor.phase else { return XCTFail("\(executor.phase)") }
        let before = names(plan.bundleURL)
        let held = try BundleWriterLock.acquire(bundle: plan.bundleURL) // another process holds the lock
        defer { held.release() }
        XCTAssertEqual(executor.rollback(), .refused(lock: .busy))
        XCTAssertEqual(names(plan.bundleURL), before, "nothing is deleted without the lock")
    }

    // MARK: 3. `busy` at step 4

    func testBusyAtTheWriteOfConfigDeletesNothingAndTheRetrySucceeds() throws {
        let plan = try makePlan(name: "Busy", disk: true)
        let session = FakeCreationSession()
        let image = plan.bundleURL.appendingPathComponent("disks/disk0.img")
        let imageBytes = Data(repeating: 0xAB, count: 4096)
        var foreignLock: BundleWriterLock?
        session.onPrepare = { _ in
            try? imageBytes.write(to: image) // what the VMHost would have created
            return .success(.ack(ControlV2Ack(ackedKind: 5, state: 0)))
        }
        session.onStopped = {
            // The VMHost has released the lock; another process (or a probe) takes it before the writer does.
            foreignLock = try? BundleWriterLock.acquire(bundle: plan.bundleURL)
        }
        let executor = fakeExecutor(plan: plan, session: session)
        executor.start()
        waitUntil("the end of the creation") { isFinished(executor) }
        XCTAssertNotNil(foreignLock, "the foreign lock was taken")
        guard case .failed(let failure) = executor.phase else { return XCTFail("\(executor.phase)") }
        XCTAssertEqual(failure.step, .writeConfig)
        XCTAssertTrue(failure.canRetry)
        XCTAssertFalse(failure.canRollback, "busy: nothing may be deleted (VM_BUNDLE 5.1)")
        XCTAssertTrue(failure.hint.contains("Повторите"), failure.hint)
        XCTAssertEqual(executor.rollback(), .notAllowed)
        let after = names(plan.bundleURL)
        XCTAssertEqual(after, ["disks", "firmware", "lock", "logs", "snapshots", "tpm"])
        XCTAssertEqual(try Data(contentsOf: image), imageBytes, "the image stays")
        XCTAssertFalse(exists(plan.bundleURL.appendingPathComponent("config.plist")))

        foreignLock?.release()
        executor.retryWriteConfig()
        XCTAssertEqual(executor.phase, .succeeded)
        guard case .valid(let config) = BundleConfigCodec.read(bundle: plan.bundleURL) else { return XCTFail("config.plist is not valid") }
        XCTAssertEqual(config.disks.count, 1)
        XCTAssertEqual(try Data(contentsOf: image), imageBytes)
    }

    // MARK: 4. refusals before or at the first step; cancellation

    func testExistingDirectoryIsAnErrorAndItIsNeverTouched() throws {
        let existing = try makeExistingMachine()
        // The plan is built by hand: the model refuses the name `Existing` already at the first step.
        var plan = try makePlan(name: "Other", disk: false)
        plan.bundleURL = existing.url
        let executor = CreateMachineExecutor(plan: plan, vmhost: missingVMHost) { _, _ in XCTFail("no VMHost"); throw NSError(domain: "t", code: 0) }
        executor.start()
        guard case .failed(let failure) = executor.phase else { return XCTFail("\(executor.phase)") }
        XCTAssertEqual(failure.step, .directories)
        XCTAssertEqual(failure.headline, "Не удалось создать машину: каталог машины уже существует.")
        XCTAssertFalse(failure.canRollback)
        XCTAssertTrue(executor.createdDirectories.isEmpty, "nothing is claimed that this run did not create")
        XCTAssertEqual(executor.rollback(), .notAllowed)
        try assertUntouched(existing)
    }

    func testMissingVMHostFailsBeforeAnythingIsCreated() throws {
        let plan = try makePlan(name: "NoHost", disk: true)
        let executor = CreateMachineExecutor(plan: plan, vmhost: missingVMHost)
        executor.start()
        guard case .failed(let failure) = executor.phase else { return XCTFail("\(executor.phase)") }
        XCTAssertEqual(failure.step, .preparation)
        XCTAssertFalse(failure.canRollback)
        XCTAssertFalse(exists(plan.bundleURL), "no directory was created")
        XCTAssertEqual(names(library), [])
    }

    func testCancelAtAnyStepCreatesNothing() throws {
        let missingLibrary = root.appendingPathComponent("Never Created", isDirectory: true)
        for stop in 0..<CreateMachineStep.allCases.count {
            let environment = CreateMachineEnvironment(hostMemoryBytes: 64 << 30, defaultDirectory: missingLibrary, availableSpace: { _ in 1 << 40 })
            let model = CreateMachineModel(environment: environment, name: "Cancelled")
            model.diskEnabled = true
            for _ in 0..<stop { XCTAssertTrue(model.advance()) }
            model.cancel()
            XCTAssertEqual(model.phase, .cancelled)
            XCTAssertNil(model.confirm(), "a cancelled wizard returns no plan")
            XCTAssertFalse(exists(missingLibrary), "step \(stop): the wizard created no directory")
        }
        XCTAssertEqual(names(library), [])
    }

    func testFailureTextsComeFromTheCatalogOfErrors() {
        let busy = ErrorCatalog.entry(for: LibraryIssue.creationRetryNeeded)
        XCTAssertTrue(busy.retrySafe)
        XCTAssertTrue(busy.action.contains("Повторите"))
        XCTAssertFalse(ErrorCatalog.entry(for: LibraryIssue.creationLockFailed).retrySafe)
        XCTAssertEqual(CreateMachineProgressView.rollbackText(.done(removed: ["disks/"], remaining: ["lock"])).contains("lock"), true)
    }
}
