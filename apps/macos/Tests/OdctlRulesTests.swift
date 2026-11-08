import Darwin
import Foundation
import XCTest

@testable import ODShared

// The pure rules of `odctl` (S11-13 review): the script grammar, the name of a bundle directory and the deletion of the files of a bundle.
// They live in `ODShared/OdctlRules.swift`; the `odctl` target is an executable and is not reachable from here.

final class OdctlScriptTests: XCTestCase {
    private func failure(_ text: String) -> ScriptError? {
        do {
            _ = try parseScript(text)
            return nil
        } catch {
            return error as? ScriptError
        }
    }

    func testLifecycleScript() throws {
        let lines = try parseScript("# test-only\nstart\nwait running\npause  # comment\nwait paused 5000\nresume\nsleep 0\nquery\nstop\n")
        XCTAssertEqual(lines.map(\.number), [2, 3, 4, 5, 6, 7, 8, 9])
        XCTAssertEqual(lines.map(\.command), [
            .start, .wait(state: .running, timeoutMs: ScriptLimits.defaultWaitMs), .pause, .wait(state: .paused, timeoutMs: 5000), .resume,
            .sleep(ms: 0), .query, .stop,
        ])
        XCTAssertEqual(lines[3].text, "wait paused 5000")
    }

    func testCarriageReturnsAndTabs() throws {
        let lines = try parseScript("start\r\n\twait\trunning  \r\nstop\r\n")
        XCTAssertEqual(lines.map(\.text), ["start", "wait running", "stop"])
    }

    func testMistakesNameTheLine() {
        XCTAssertEqual(failure("start\nwait flying")?.line, 2)
        XCTAssertEqual(failure("start\nfly")?.line, 2)
        XCTAssertEqual(failure("start now")?.line, 1)
        XCTAssertEqual(failure("wait")?.line, 1)
        XCTAssertEqual(failure("wait running 0")?.line, 1)
        XCTAssertEqual(failure("wait running 600001")?.line, 1)
        XCTAssertEqual(failure("wait running 1x")?.line, 1)
        XCTAssertEqual(failure("sleep -1")?.line, 1)
        XCTAssertEqual(failure("sleep 600001")?.line, 1)
        XCTAssertEqual(failure("START")?.line, 1, "the case is lower")
    }

    func testNothingAfterStop() {
        XCTAssertEqual(failure("stop\nstart")?.line, 2)
        XCTAssertEqual(failure("stop\n# only a comment\n\n")?.line, nil)
        XCTAssertNil(failure("start\nstop\n# only a comment\n\n"))
    }

    func testEmptyScript() {
        XCTAssertEqual(failure(""), ScriptError(line: nil, message: "нет ни одной команды"))
        XCTAssertNotNil(failure("# only a comment\n\n   \n"))
    }

    func testLineLimitCountsTheComment() {
        let exact = "start #" + String(repeating: "x", count: ScriptLimits.maxLineBytes - 7)
        XCTAssertEqual(exact.utf8.count, ScriptLimits.maxLineBytes)
        XCTAssertNil(failure(exact))
        XCTAssertEqual(failure(exact + "x")?.line, 1, "a comment after the command counts toward 256 bytes")
        // Bytes, not characters: 129 two-byte characters are 258 bytes.
        XCTAssertEqual(failure("start #" + String(repeating: "я", count: 125))?.line, 1)
        XCTAssertEqual(failure("# " + String(repeating: "x", count: 300) + "\nstart")?.line, 1)
    }

    func testCommandLimit() {
        XCTAssertNil(failure(Array(repeating: "query", count: ScriptLimits.maxCommands).joined(separator: "\n")))
        XCTAssertEqual(failure(Array(repeating: "query", count: ScriptLimits.maxCommands + 1).joined(separator: "\n"))?.line, ScriptLimits.maxCommands + 1)
    }

    func testStateNames() {
        for state in [ControlV2State.stopped, .running, .paused, .shuttingDown, .failed] {
            XCTAssertEqual(parseStateName(stateName(state)), state)
        }
        XCTAssertNil(parseStateName("exited"))
    }
}

final class OdctlBundleNamingTests: XCTestCase {
    func testDirectoryName() {
        XCTAssertEqual(BundleNaming.directoryName(for: "smoke"), "smoke.odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: "Мой Windows"), "Мой Windows.odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: "a/b:c"), "a_b_c.odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: ".hidden"), "_hidden.odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: ".."), "_..odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: "///"), "Machine.odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: ". ."), "_ ..odvm")
        XCTAssertEqual(BundleNaming.directoryName(for: "a\u{01}b\u{7F}"), "a_b_.odvm")
    }
}

final class OdctlBundleRemovalTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        let base = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).resolvingSymlinksInPath()
        root = base.appendingPathComponent("odctl-rules-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDown() {
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    /// `<name>.odvm` with a regular `config.plist` (its content does not matter here: the shape is what is checked) and `disks/`.
    @discardableResult
    private func makeBundle(_ name: String = "m.odvm", diskFiles: [String] = []) throws -> URL {
        let bundle = root.appendingPathComponent(name, isDirectory: true)
        let fm = FileManager.default
        try fm.createDirectory(at: bundle.appendingPathComponent("disks", isDirectory: true), withIntermediateDirectories: true)
        try Data("x".utf8).write(to: bundle.appendingPathComponent("config.plist"))
        for file in diskFiles { try Data("disk".utf8).write(to: bundle.appendingPathComponent("disks").appendingPathComponent(file)) }
        return bundle
    }

    private func refusal(_ url: URL, config: BundleConfig? = nil) -> BundleRemovalRefusal? {
        do {
            try BundleRemoval.removeFiles(at: url, config: config)
            return nil
        } catch {
            return error as? BundleRemovalRefusal
        }
    }

    private func configWithDisk() -> BundleConfig {
        var config = BundleConfig(name: "m", machineProfile: .displayTestPattern)
        config.disks = [BundleDisk(location: .inBundle(name: "disk0.img"))]
        return config
    }

    func testRemovesABundleWithoutDisks() throws {
        let bundle = try makeBundle()
        XCTAssertNil(refusal(bundle, config: BundleConfig(name: "m", machineProfile: .displayTestPattern)))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testRemovesABundleWithUnreadableConfigAndNoDiskFiles() throws {
        let bundle = try makeBundle()
        XCTAssertNil(refusal(bundle, config: nil))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testMissingDirectoryIsASuccess() {
        XCTAssertNil(refusal(root.appendingPathComponent("gone.odvm", isDirectory: true)))
    }

    func testRefusesALink() throws {
        let bundle = try makeBundle()
        let link = root.appendingPathComponent("link.odvm")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: bundle)
        XCTAssertEqual(refusal(link), .notADirectory)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("config.plist").path), "the target is not touched")
        XCTAssertNotNil(try? FileManager.default.destinationOfSymbolicLink(atPath: link.path), "the link stays")
    }

    func testRefusesAFileThatIsNotADirectory() throws {
        let file = root.appendingPathComponent("file.odvm")
        try Data("x".utf8).write(to: file)
        XCTAssertEqual(refusal(file), .notADirectory)
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.path))
    }

    func testRefusesADirectoryWithoutTheSuffix() throws {
        let plain = root.appendingPathComponent("Documents", isDirectory: true)
        try FileManager.default.createDirectory(at: plain, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: plain.appendingPathComponent("config.plist"))
        try Data("keep".utf8).write(to: plain.appendingPathComponent("note.txt"))
        XCTAssertEqual(refusal(plain), .notABundle)
        XCTAssertTrue(FileManager.default.fileExists(atPath: plain.appendingPathComponent("note.txt").path))
    }

    func testRefusesOnlyTheSuffixAndNoConfigAndALinkedConfig() throws {
        let onlySuffix = root.appendingPathComponent(".odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: onlySuffix, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: onlySuffix.appendingPathComponent("config.plist"))
        XCTAssertEqual(refusal(onlySuffix), .notABundle)

        let empty = root.appendingPathComponent("empty.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: empty, withIntermediateDirectories: true)
        XCTAssertEqual(refusal(empty), .notABundle)

        let linked = root.appendingPathComponent("linked.odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        let elsewhere = root.appendingPathComponent("elsewhere.plist")
        try Data("x".utf8).write(to: elsewhere)
        try FileManager.default.createSymbolicLink(at: linked.appendingPathComponent("config.plist"), withDestinationURL: elsewhere)
        XCTAssertEqual(refusal(linked), .notABundle)
        XCTAssertTrue(FileManager.default.fileExists(atPath: linked.path))
    }

    func testRefusesDotDotAndRoot() throws {
        let bundle = try makeBundle()
        // A path that still ends with `..` (not standardized) or is the root has no `.odvm` name: nothing is deleted.
        XCTAssertEqual(refusal(bundle.appendingPathComponent("..", isDirectory: true)), .notABundle)
        XCTAssertEqual(refusal(URL(fileURLWithPath: "/", isDirectory: true)), .notABundle)
        XCTAssertEqual(refusal(URL(fileURLWithPath: "..", isDirectory: true)), .notABundle)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("config.plist").path))
    }

    func testRefusesTheDisksOfAReadableConfiguration() throws {
        let bundle = try makeBundle(diskFiles: ["disk0.img"])
        XCTAssertEqual(refusal(bundle, config: configWithDisk()), .unsupportedDurability)
        // `disks` in the configuration refuses even when the image file is not there.
        let bare = try makeBundle("bare.odvm")
        XCTAssertEqual(refusal(bare, config: configWithDisk()), .unsupportedDurability)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("disks/disk0.img").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: bare.path))
    }

    func testRefusesDiskFilesWhenTheConfigurationCannotBeRead() throws {
        let bundle = try makeBundle(diskFiles: ["disk0.img"])
        XCTAssertEqual(refusal(bundle, config: nil), .unsupportedDurability)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("disks/disk0.img").path), "nothing is deleted")
        // A readable configuration without `disks` does not make files of `disks/` deletable either.
        XCTAssertEqual(refusal(bundle, config: BundleConfig(name: "m", machineProfile: .displayTestPattern)), .unsupportedDurability)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("disks/disk0.img").path))
    }

    func testHoldsDiskFiles() throws {
        let empty = try makeBundle("empty.odvm")
        XCTAssertFalse(try BundleRemoval.holdsDiskFiles(at: empty, config: nil))
        XCTAssertTrue(try BundleRemoval.holdsDiskFiles(at: empty, config: configWithDisk()))
        try FileManager.default.removeItem(at: empty.appendingPathComponent("disks"))
        XCTAssertFalse(try BundleRemoval.holdsDiskFiles(at: empty, config: nil), "no disks/ directory")
        let full = try makeBundle("full.odvm", diskFiles: [".hidden"])
        XCTAssertTrue(try BundleRemoval.holdsDiskFiles(at: full, config: nil), "a hidden file is a file")
    }

    func testRefusesABundleWhoseLockIsHeld() throws {
        let bundle = try makeBundle()
        let lock = try BundleWriterLock.acquire(bundle: bundle)
        XCTAssertEqual(refusal(bundle), .inUse)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("config.plist").path), "nothing is deleted")
        lock.release()
        XCTAssertNil(refusal(bundle))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testTheLockIsReleasedAfterARefusal() throws {
        let bundle = try makeBundle(diskFiles: ["disk0.img"])
        XCTAssertEqual(refusal(bundle), .unsupportedDurability)
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundle))
    }
}
