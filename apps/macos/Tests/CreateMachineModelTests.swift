import Darwin
import Foundation
@testable import ODShared
import XCTest
@testable import OpenDesktop

// Tests of the model of the "Create machine" wizard (S11-08a, Sources/App/CreateMachineModel.swift): validation of every step, the limits,
// the confirmation and the cancellation. No window, no VMHost, no process. The default directory is a temporary one; the tests check that
// nothing is created in it by the model. Run: `swift test --filter CreateMachineModel` from apps/.

final class CreateMachineModelTests: XCTestCase {
    private let gib: UInt64 = 1 << 30
    private var root: URL!
    /// `availableSpace` answers this value (nil: unknown).
    private var free: UInt64? = 1 << 40

    override func setUpWithError() throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("create-machine-\(getpid())-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        free = 1 << 40
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private var library: URL { root.appendingPathComponent("Open Desktop", isDirectory: true) }

    private func makeModel(hostMemory: UInt64 = 64 << 30, name: String = "") -> CreateMachineModel {
        let environment = CreateMachineEnvironment(hostMemoryBytes: hostMemory, defaultDirectory: library, availableSpace: { [unowned self] _ in self.free })
        return CreateMachineModel(environment: environment, name: name)
    }

    private func makeLibraryDirectory() throws {
        try FileManager.default.createDirectory(at: library, withIntermediateDirectories: true)
    }

    private func messages(_ model: CreateMachineModel, _ step: CreateMachineStep) -> [String] { model.errors(for: step).map(\.message) }

    /// A model that stands on the summary with valid values.
    private func modelOnSummary(disk: Bool) throws -> CreateMachineModel {
        let model = makeModel(name: "Test")
        model.diskEnabled = disk
        for _ in 0..<4 { XCTAssertTrue(model.advance()) }
        XCTAssertEqual(model.currentStep, .summary)
        return model
    }

    private func entries(of directory: URL) -> [String]? {
        try? FileManager.default.contentsOfDirectory(atPath: directory.path).sorted()
    }

    // MARK: step 1, name

    func testNameRules() throws {
        let model = makeModel()
        XCTAssertEqual(model.errors(for: .name).count, 1, "empty")
        XCTAssertFalse(model.canAdvance)
        model.name = "   "
        XCTAssertEqual(model.errors(for: .name).count, 1, "spaces only")
        model.name = "a\tb"
        XCTAssertEqual(model.errors(for: .name).count, 1, "control character")
        model.name = "a\u{7F}b"
        XCTAssertEqual(model.errors(for: .name).count, 1, "DEL")
        model.name = String(repeating: "a", count: 129)
        XCTAssertEqual(model.errors(for: .name).count, 1, "129 bytes")
        model.name = String(repeating: "я", count: 65)  // 130 bytes: the limit counts bytes of UTF-8, not characters
        XCTAssertEqual(model.errors(for: .name).count, 1, "130 bytes in 65 characters")
        model.name = String(repeating: "a", count: 128)
        XCTAssertTrue(model.errors(for: .name).isEmpty, "128 bytes")
        model.name = String(repeating: "я", count: 64)
        XCTAssertTrue(model.errors(for: .name).isEmpty, "128 bytes in 64 characters")
        model.name = "Рабочая машина 1"
        XCTAssertTrue(model.errors(for: .name).isEmpty)
        XCTAssertTrue(model.canAdvance)
        // The name is never edited by the model.
        model.name = " a "
        XCTAssertEqual(model.name, " a ")
        XCTAssertTrue(model.errors(for: .name).isEmpty)
    }

    func testBundleDirectoryNameFollowsVMBundleRules() {
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: "Мой Windows"), "Мой Windows.odvm")
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: "a/b:c"), "a_b_c.odvm")
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: ".hidden"), "_hidden.odvm")
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: ".."), "_..odvm")
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: "///"), "Machine.odvm")
        XCTAssertEqual(CreateMachineModel.bundleDirectoryName(for: ". ."), "_ ..odvm")
        // `/` and `:` are legal in the name (the directory name replaces them), the control characters are not (they are an error).
        let model = makeModel(name: "a/b:c")
        XCTAssertTrue(model.errors(for: .name).isEmpty)
        XCTAssertEqual(model.bundleURL.lastPathComponent, "a_b_c.odvm")
        XCTAssertEqual(model.bundleURL.deletingLastPathComponent().path, library.path)
    }

    func testDuplicateBundleIsAnErrorOfTheStep() throws {
        try makeLibraryDirectory()
        let model = makeModel(name: "Test")
        XCTAssertTrue(model.errors(for: .name).isEmpty, "no such directory yet")
        try FileManager.default.createDirectory(at: library.appendingPathComponent("Test.odvm"), withIntermediateDirectories: true)
        XCTAssertEqual(model.errors(for: .name).count, 1)
        XCTAssertTrue(messages(model, .name)[0].contains("Test.odvm"))
        XCTAssertFalse(model.advance())
        XCTAssertEqual(model.currentStep, .name)
        // A different name that gives the same directory name is a duplicate as well (the check is on the directory name).
        model.name = "Test"
        let other = makeModel(name: "Te/st")
        XCTAssertTrue(other.errors(for: .name).isEmpty)
        try FileManager.default.createDirectory(at: library.appendingPathComponent("Te_st.odvm"), withIntermediateDirectories: true)
        XCTAssertEqual(other.errors(for: .name).count, 1)
        // A dangling symlink with that name also counts as taken (lstat, nothing is followed).
        let linked = makeModel(name: "Link")
        try FileManager.default.createSymbolicLink(at: library.appendingPathComponent("Link.odvm"), withDestinationURL: root.appendingPathComponent("nowhere"))
        XCTAssertEqual(linked.errors(for: .name).count, 1)
        // Another name is free.
        model.name = "Test 2"
        XCTAssertTrue(model.errors(for: .name).isEmpty)
        XCTAssertTrue(model.advance())
    }

    // MARK: step 2, profile

    func testProfilesAreTheClosedListAndLabeledTestOnly() {
        let model = makeModel(name: "x")
        XCTAssertEqual(model.profileChoices.map(\.profile.rawValue), ["display-test-pattern", "display-two-screens-test"])
        XCTAssertEqual(model.profileChoices.map(\.profile), MachineProfile.allCases)
        for choice in model.profileChoices { XCTAssertTrue(choice.detail.contains("тестовый, без гостя"), choice.title) }
        XCTAssertEqual(model.profile, .displayTestPattern)
        XCTAssertTrue(model.advance())
        XCTAssertEqual(model.currentStep, .profile)
        XCTAssertTrue(model.errors(for: .profile).isEmpty)
        model.profile = .displayTwoScreensTest
        XCTAssertTrue(model.advance())
        XCTAssertEqual(model.currentStep, .memory)
    }

    // MARK: step 3, memory

    func testMemoryDefaultAndRange() {
        let model = makeModel(hostMemory: 256 * gib, name: "x")  // the half-of-host rule does not interfere with the fixed range
        XCTAssertEqual(model.memoryMiB, 4096)
        XCTAssertTrue(model.errors(for: .memory).isEmpty)
        for value in [255, 0, -1, 65_537, 1_000_000] {
            model.memoryMiB = value
            XCTAssertEqual(model.errors(for: .memory).count, 1, "\(value)")
            XCTAssertTrue(messages(model, .memory)[0].contains("\(value)"), "the message names the value")
            XCTAssertTrue(messages(model, .memory)[0].contains("256") && messages(model, .memory)[0].contains("65536"))
        }
        for value in [256, 257, 65_535, 65_536] {
            model.memoryMiB = value
            XCTAssertTrue(model.errors(for: .memory).isEmpty, "\(value)")
        }
    }

    func testMemoryAboveHalfOfHostMemoryAndNoSilentCorrection() {
        let model = makeModel(hostMemory: 8 * gib, name: "x")  // limit 4096 MiB
        XCTAssertEqual(model.hostMemoryLimitMiB, 4096)
        XCTAssertTrue(model.errors(for: .memory).isEmpty, "the default fits a host with 8 GiB")
        model.memoryMiB = 4097
        XCTAssertEqual(model.errors(for: .memory).count, 1)
        XCTAssertTrue(messages(model, .memory)[0].contains("4096"))
        // Odd host memory: the limit is rounded down like `BundleConfig.launchRefusal`.
        XCTAssertEqual(makeModel(hostMemory: 8 * gib + (1 << 20) - 1).hostMemoryLimitMiB, 4096)
        XCTAssertEqual(makeModel(hostMemory: 8 * gib + (2 << 20)).hostMemoryLimitMiB, 4097)
        // The default of 4096 MiB does not fit a host with 4 GiB: an error of the step, the value is kept as it is.
        let small = makeModel(hostMemory: 4 * gib, name: "x")
        XCTAssertEqual(small.errors(for: .memory).count, 1)
        XCTAssertEqual(small.memoryMiB, 4096)
        XCTAssertTrue(small.advance() && small.advance())
        XCTAssertEqual(small.currentStep, .memory)
        XCTAssertFalse(small.advance(), "the error blocks the step")
        XCTAssertEqual(small.currentStep, .memory)
        XCTAssertEqual(small.memoryMiB, 4096, "no silent correction")
        small.memoryMiB = 2048
        XCTAssertTrue(small.advance())
        XCTAssertEqual(small.currentStep, .disk)
    }

    func testMemoryRuleAgreesWithLaunchRefusal() {
        for (host, memory) in [(8 * gib, 4096), (8 * gib, 4097), (64 * gib, 32_768), (64 * gib, 32_769), (4 * gib, 4096)] {
            let model = makeModel(hostMemory: host, name: "x")
            model.memoryMiB = memory
            var config = BundleConfig(name: "x")
            config.memoryMiB = memory
            let refused: Bool
            if case .hostMemoryExceeded? = config.launchRefusal(hostMemoryBytes: host) { refused = true } else { refused = false }
            XCTAssertEqual(!model.errors(for: .memory).isEmpty, refused, "host \(host / gib) GiB, \(memory) MiB")
        }
    }

    // MARK: step 4, disk

    func testDiskStepIsSkippedWithoutDisk() {
        let model = makeModel(name: "x")
        XCTAssertFalse(model.diskEnabled)
        model.diskCapacityBytes = 7  // not looked at while the disk is off
        XCTAssertTrue(model.issues(for: .disk).isEmpty)
    }

    func testDiskCapacityRules() {
        let model = makeModel(name: "x")
        model.diskEnabled = true
        XCTAssertEqual(model.diskCapacityBytes, 32 * gib)
        XCTAssertTrue(model.errors(for: .disk).isEmpty)
        for value: UInt64 in [0, 1, 511, 513, 1000, (1 << 20) + 1] {
            model.diskCapacityBytes = value
            XCTAssertEqual(model.errors(for: .disk).count, 1, "\(value)")
        }
        for value: UInt64 in [512, 1024, 1 << 20, 64 * gib] {
            model.diskCapacityBytes = value
            XCTAssertTrue(model.errors(for: .disk).isEmpty, "\(value)")
        }
        // Above Int64.max (a multiple of 512, so only the upper limit refuses it).
        model.diskCapacityBytes = UInt64(Int64.max) + 1
        XCTAssertEqual(model.errors(for: .disk).count, 1)
        model.diskCapacityBytes = UInt64.max - 511
        XCTAssertEqual(model.errors(for: .disk).count, 1)
        model.diskCapacityBytes = UInt64(Int64.max) - 511  // the largest multiple of 512 not above Int64.max
        free = nil
        XCTAssertTrue(model.errors(for: .disk).isEmpty)
    }

    func testDiskLargerThanFreeSpaceIsAWarningNotAnError() throws {
        free = 10 * gib
        let model = makeModel(name: "x")
        model.diskEnabled = true
        model.diskCapacityBytes = 10 * gib  // equal: no warning
        XCTAssertTrue(model.issues(for: .disk).isEmpty)
        model.diskCapacityBytes = 10 * gib + 512
        XCTAssertTrue(model.errors(for: .disk).isEmpty)
        let warnings = model.warnings(for: .disk)
        XCTAssertEqual(warnings.count, 1)
        XCTAssertTrue(warnings[0].message.contains("разрежен"), "the sparse-file explanation")
        XCTAssertTrue(warnings[0].message.contains("гипотез"), "it is a hypothesis")
        XCTAssertTrue(warnings[0].message.contains("отклонит"), "and VMHost refuses a capacity above the free space")
        XCTAssertEqual(model.availableSpaceBytes, 10 * gib)
        XCTAssertTrue(model.advance() && model.advance() && model.advance())
        XCTAssertTrue(model.advance(), "a warning does not block the step")
        XCTAssertEqual(model.currentStep, .summary)
        // Unknown free space: no warning.
        free = nil
        XCTAssertTrue(model.issues(for: .disk).isEmpty)
        XCTAssertNil(model.availableSpaceBytes)
    }

    func testVolumeAvailableBytesOfExistingAndMissingDirectory() throws {
        let existing = CreateMachineModel.volumeAvailableBytes(at: root)
        XCTAssertNotNil(existing)
        XCTAssertGreaterThan(existing ?? 0, 0)
        // A directory that does not exist yet is measured at its nearest existing ancestor and is not created.
        let missing = root.appendingPathComponent("a/b/c", isDirectory: true)
        XCTAssertNotNil(CreateMachineModel.volumeAvailableBytes(at: missing))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("a").path))
    }

    // MARK: step 5, summary and confirmation

    func testSummaryWarnsAboutDurabilityOnlyForAMachineWithADisk() throws {
        let withoutDisk = try modelOnSummary(disk: false)
        XCTAssertTrue(withoutDisk.issues(for: .summary).isEmpty)
        let withDisk = try modelOnSummary(disk: true)
        let warnings = withDisk.warnings(for: .summary).map(\.message)
        XCTAssertEqual(warnings.count, 1)
        XCTAssertTrue(warnings[0].contains("unsupported_durability"))
        XCTAssertTrue(warnings[0].contains("этап 9"))
        XCTAssertTrue(withDisk.errors(for: .summary).isEmpty)
        XCTAssertTrue(withDisk.summaryLines().contains { $0.label == "Файл диска" && $0.value == "disks/disk0.img" })
        XCTAssertFalse(withoutDisk.summaryLines().contains { $0.label == "Файл диска" })
        XCTAssertTrue(withDisk.summaryLines().contains { $0.label == "Память" && $0.value == "4096 МиБ" })
    }

    func testConfirmReturnsThePlanWithoutCreatingAnything() throws {
        let model = try modelOnSummary(disk: true)
        model.diskCapacityBytes = 1 << 20
        let plan = try XCTUnwrap(model.confirm())
        XCTAssertEqual(model.phase, .confirmed)
        XCTAssertEqual(plan.bundleURL.lastPathComponent, "Test.odvm")
        XCTAssertEqual(plan.config.name, "Test")
        XCTAssertEqual(plan.config.machineProfile, .displayTestPattern)
        XCTAssertEqual(plan.config.memoryMiB, 4096)
        XCTAssertEqual(plan.config.disks, [BundleDisk(location: .inBundle(name: "disk0.img"))])
        XCTAssertEqual(plan.disk, CreateMachineDiskPlan(name: "disk0.img", capacityBytes: 1 << 20))
        // The session of the creation configures a machine WITHOUT disks (VM_BUNDLE 5.1 step 3); the final file has the disk.
        XCTAssertTrue(plan.creationConfigure.disks.isEmpty)
        XCTAssertNil(plan.creationConfigure.validate())
        XCTAssertEqual(plan.config.controlConfigure().disks.count, 1)
        // The final config is something the reader accepts as valid (the encoder re-reads what it writes).
        XCTAssertNoThrow(try BundleConfigCodec.encode(plan.config))
        // Nothing was created: not the bundle, not even the default directory.
        XCTAssertFalse(FileManager.default.fileExists(atPath: library.path))
        XCTAssertEqual(entries(of: root), [])
        // A second confirmation is refused.
        XCTAssertNil(model.confirm())
    }

    func testPlanWithoutDiskAndTwoScreens() throws {
        let model = makeModel(name: "Две/панели")
        model.profile = .displayTwoScreensTest
        model.memoryMiB = 1024
        for _ in 0..<4 { XCTAssertTrue(model.advance()) }
        let plan = try XCTUnwrap(model.confirm())
        XCTAssertNil(plan.disk)
        XCTAssertTrue(plan.config.disks.isEmpty)
        XCTAssertEqual(plan.config.displayScreens, 2)
        XCTAssertEqual(plan.creationConfigure.machineProfile, 4)
        XCTAssertEqual(plan.creationConfigure.memoryMiB, 1024)
        XCTAssertEqual(plan.bundleURL.lastPathComponent, "Две_панели.odvm")
        XCTAssertEqual(plan.config.name, "Две/панели", "the name in config.plist is not changed by the directory name rule")
        XCTAssertNoThrow(try BundleConfigCodec.encode(plan.config))
    }

    func testConfirmChecksEverythingAgainAtTheEnd() throws {
        let model = try modelOnSummary(disk: false)
        XCTAssertTrue(model.canConfirm)
        // The directory appears while the wizard stands on the summary: no plan, an error on the name step and on the summary.
        try makeLibraryDirectory()
        try FileManager.default.createDirectory(at: library.appendingPathComponent("Test.odvm"), withIntermediateDirectories: true)
        XCTAssertFalse(model.canConfirm)
        XCTAssertNil(model.confirm())
        XCTAssertEqual(model.errors(for: .summary).count, 1)
        XCTAssertEqual(model.phase, .editing)
        // Values changed behind a step are caught too: back to the name, a new name, forward again.
        model.back(); model.back(); model.back(); model.back()
        XCTAssertEqual(model.currentStep, .name)
        model.name = "Test 2"
        for _ in 0..<4 { XCTAssertTrue(model.advance()) }
        XCTAssertNotNil(model.confirm())
    }

    func testConfirmOnlyOnTheSummary() {
        let model = makeModel(name: "x")
        XCTAssertNil(model.confirm())
        XCTAssertTrue(model.advance())
        XCTAssertNil(model.confirm())
    }

    // MARK: navigation

    func testStepsGoInOrderAndBackKeepsTheValues() {
        let model = makeModel(name: "x")
        XCTAssertEqual(CreateMachineStep.allCases, [.name, .profile, .memory, .disk, .summary])
        XCTAssertEqual(model.currentStep, .name)
        model.back()
        XCTAssertEqual(model.currentStep, .name)
        model.profile = .displayTwoScreensTest
        model.memoryMiB = 512
        model.diskEnabled = true
        model.diskCapacityBytes = 4096
        var seen: [CreateMachineStep] = [model.currentStep]
        while model.advance() { seen.append(model.currentStep) }
        XCTAssertEqual(seen, CreateMachineStep.allCases)
        XCTAssertFalse(model.advance(), "no step after the summary")
        for _ in 0..<4 { model.back() }
        XCTAssertEqual(model.currentStep, .name)
        XCTAssertEqual(model.name, "x")
        XCTAssertEqual(model.profile, .displayTwoScreensTest)
        XCTAssertEqual(model.memoryMiB, 512)
        XCTAssertTrue(model.diskEnabled)
        XCTAssertEqual(model.diskCapacityBytes, 4096)
    }

    func testAdvanceIsRefusedWhileTheStepHasAnError() {
        let model = makeModel()
        XCTAssertFalse(model.advance())
        XCTAssertEqual(model.currentStep, .name)
        XCTAssertFalse(model.errors(for: .name).isEmpty)
        model.name = "ok"
        XCTAssertTrue(model.advance())
        model.memoryMiB = 100
        XCTAssertTrue(model.advance(), "an error of a later step does not block the earlier one")
        XCTAssertEqual(model.currentStep, .memory)
        XCTAssertFalse(model.advance())
        XCTAssertEqual(model.currentStep, .memory)
        model.diskEnabled = true
        model.diskCapacityBytes = 5
        model.memoryMiB = 256
        XCTAssertTrue(model.advance())
        XCTAssertEqual(model.currentStep, .disk)
        XCTAssertFalse(model.advance())
        XCTAssertEqual(model.currentStep, .disk)
    }

    // MARK: cancellation

    func testCancelAtAnyStepCreatesNothing() throws {
        for stopAt in CreateMachineStep.allCases {
            let model = makeModel(name: "Cancel \(stopAt.rawValue)")
            model.diskEnabled = true
            model.diskCapacityBytes = 1 << 20
            while model.currentStep != stopAt { XCTAssertTrue(model.advance()) }
            let before = entries(of: root)
            XCTAssertEqual(before, [])
            model.cancel()
            XCTAssertEqual(model.phase, .cancelled, "\(stopAt)")
            // The library directory was not created and nothing is in the temporary root.
            XCTAssertFalse(FileManager.default.fileExists(atPath: library.path), "\(stopAt)")
            XCTAssertEqual(entries(of: root), before, "\(stopAt)")
            // A cancelled wizard goes nowhere and confirms nothing.
            XCTAssertFalse(model.advance())
            XCTAssertNil(model.confirm())
            XCTAssertFalse(model.canAdvance)
            XCTAssertFalse(model.canConfirm)
            model.back()
            XCTAssertEqual(model.currentStep, stopAt)
            model.cancel()
            XCTAssertEqual(model.phase, .cancelled)
        }
    }

    func testCancelAfterAllStepsLeavesAnExistingLibraryUntouched() throws {
        try makeLibraryDirectory()
        let keep = library.appendingPathComponent("Other.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: keep, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: keep.appendingPathComponent("config.plist"))
        let model = try modelOnSummary(disk: true)
        model.cancel()
        XCTAssertEqual(entries(of: library), ["Other.odvm"])
        XCTAssertEqual(entries(of: keep), ["config.plist"])
        XCTAssertEqual(try Data(contentsOf: keep.appendingPathComponent("config.plist")), Data("x".utf8))
        XCTAssertNil(model.confirm())
    }

    func testConfirmedWizardCannotBeCancelled() throws {
        let model = try modelOnSummary(disk: false)
        XCTAssertNotNil(model.confirm())
        model.cancel()
        XCTAssertEqual(model.phase, .confirmed)
    }
}
