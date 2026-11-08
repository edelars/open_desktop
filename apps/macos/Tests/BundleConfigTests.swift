import Darwin
import Foundation
import XCTest
@testable import ODShared

// Tests of Shared/BundleConfig.swift (S11-06a): config.plist v1 of docs/specs/VM_BUNDLE.md section 3 and 5.2, the bundle lock 2.1.
// No VMHost process. Run: `swift test --filter BundleConfigXCTests` from apps/.

final class BundleConfigXCTests: XCTestCase {
    private var temporaryDirectories: [URL] = []

    override func tearDown() {
        for url in temporaryDirectories { try? FileManager.default.removeItem(at: url) }
        temporaryDirectories = []
        AtomicReplace.fault = nil
        super.tearDown()
    }

    private func makeBundle(_ plist: String? = nil) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("od-bundle-\(UUID().uuidString).odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        temporaryDirectories.append(directory)
        if let plist {
            try Data(plist.utf8).write(to: directory.appendingPathComponent("config.plist"))
        }
        return directory
    }

    private let identity = "3f2a9c1e-7b4d-4e0a-9d52-6c18a0f4b7e3"

    /// A complete plist document; `body` replaces the keys after `identity`, `name` and `machine_profile` when `core` is true.
    private func document(_ keys: String) -> String {
        """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0"><dict>\(keys)</dict></plist>
        """
    }

    private func core(identity: String? = nil, name: String = "Test machine", profile: String = "display-test-pattern") -> String {
        "<key>schema_version</key><integer>1</integer><key>identity</key><string>\(identity ?? self.identity)</string>"
            + "<key>name</key><string>\(name)</string><key>machine_profile</key><string>\(profile)</string>"
    }

    private func state(_ keys: String) -> BundleConfigState {
        BundleConfigCodec.parse(data: Data(document(keys).utf8))
    }

    private func assertInvalid(_ keys: String, field: String, file: StaticString = #filePath, line: UInt = #line) {
        let result = state(keys)
        guard case .invalid(let issue) = result else { return XCTFail("expected invalid, got \(result)", file: file, line: line) }
        XCTAssertEqual(issue.field, field, "\(issue)", file: file, line: line)
    }

    private func assertIncompatible(_ keys: String, field: String, file: StaticString = #filePath, line: UInt = #line) {
        let result = state(keys)
        guard case .incompatible(let issue) = result else { return XCTFail("expected incompatible, got \(result)", file: file, line: line) }
        XCTAssertEqual(issue.field, field, "\(issue)", file: file, line: line)
    }

    // MARK: valid

    /// VM_BUNDLE 10.1.
    func testSpecificationExampleIsValidAndKeepsTheUnknownKey() throws {
        let keys = core(name: "Тестовая машина") + """
        <key>memory_mib</key><integer>4096</integer><key>vcpu_count</key><integer>1</integer>
        <key>disks</key><array><dict><key>path</key><string>disks/disk0.img</string><key>format</key><string>raw</string>
        <key>writable</key><true/></dict></array>
        <key>display</key><dict><key>screens</key><integer>1</integer></dict>
        <key>network</key><string>none</string><key>audio</key><false/><key>shared_folders</key><array/>
        <key>clipboard_sync</key><false/><key>x_future_note</key><string>kept</string>
        """
        guard case .valid(let config) = state(keys) else { return XCTFail("expected valid: \(state(keys))") }
        XCTAssertEqual(config.identity, identity)
        XCTAssertEqual(config.name, "Тестовая машина")
        XCTAssertEqual(config.machineProfile, .displayTestPattern)
        XCTAssertEqual(config.memoryMiB, 4096)
        XCTAssertEqual(config.disks, [BundleDisk(location: .inBundle(name: "disk0.img"), format: .raw, writable: true)])
        XCTAssertEqual(config.unknownKeys.values["x_future_note"] as? String, "kept")
        XCTAssertEqual(config.launchRefusal(hostMemoryBytes: 64 << 30), .unsupportedDurability) // non-empty disks, 3.4 item 4
    }

    func testDefaultsOfAMinimalFile() throws {
        guard case .valid(let config) = state(core()) else { return XCTFail("expected valid") }
        XCTAssertEqual(config.memoryMiB, 4096)
        XCTAssertEqual(config.vcpuCount, 1)
        XCTAssertEqual(config.disks, [])
        XCTAssertNil(config.bootMedia)
        XCTAssertEqual(config.displayScreens, 1)
        XCTAssertEqual(config.network, .none)
        XCTAssertFalse(config.audio)
        XCTAssertEqual(config.sharedFolders, [])
        XCTAssertFalse(config.clipboardSync)
        XCTAssertNil(config.launchRefusal(hostMemoryBytes: 64 << 30))
        // The second profile takes its screens from the profile.
        guard case .valid(let two) = state(core(profile: "display-two-screens-test")) else { return XCTFail("expected valid") }
        XCTAssertEqual(two.displayScreens, 2)
        XCTAssertEqual(two.machineProfile.controlValue, 4)
    }

    func testBookmarkFieldsAndSharedFolders() throws {
        let keys = core() + """
        <key>boot_media</key><data>AAECAwQFBgcICQoLDA0ODw==</data>
        <key>disks</key><array><dict><key>bookmark</key><data>AAECAwQ=</data><key>format</key><string>raw</string><key>writable</key><false/></dict></array>
        <key>shared_folders</key><array><dict><key>bookmark</key><data>AAEC</data><key>guest_name</key><string>Projects</string></dict></array>
        """
        guard case .valid(let config) = state(keys) else { return XCTFail("expected valid: \(state(keys))") }
        XCTAssertNotNil(config.bootMedia)
        XCTAssertEqual(config.disks.first?.location, .external(bookmark: Data([0, 1, 2, 3, 4])))
        XCTAssertEqual(config.disks.first?.writable, false)
        XCTAssertEqual(config.sharedFolders.first?.readOnly, true) // default
        XCTAssertEqual(config.sharedFolders.first?.guestName, "Projects")
        // 3.5 item 3: these need devices that the test-only profile does not have.
        XCTAssertEqual(config.launchRefusal(hostMemoryBytes: 64 << 30), .unsupportedDurability)
        var withoutDisks = config
        withoutDisks.disks = []
        XCTAssertEqual(withoutDisks.launchRefusal(hostMemoryBytes: 64 << 30), .unsupported(field: "boot_media"))
    }

    // MARK: invalid (examples 10.2 - 10.4 and the table)

    func testSpecificationInvalidExamples() {
        // 10.2: an image outside the bundle as a path string.
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>../../Users/me/disk.img</string><key>format</key><string>raw</string></dict></array>",
                      field: "disks[0].path")
        // 10.3: the boot medium as a path.
        assertInvalid(core() + "<key>boot_media</key><string>/Users/me/Downloads/ubuntu.iso</string>", field: "boot_media")
        // 10.4: the profile and the number of screens do not agree.
        assertInvalid(core(profile: "display-two-screens-test") + "<key>display</key><dict><key>screens</key><integer>1</integer></dict>",
                      field: "display.screens")
    }

    func testInvalidTable() {
        assertInvalid("<key>identity</key><string>\(identity)</string>", field: "schema_version")
        assertInvalid(core(identity: identity.uppercased()), field: "identity")
        assertInvalid(core(identity: "not-a-uuid"), field: "identity")
        assertInvalid(core(name: "   "), field: "name")
        assertInvalid(core(name: String(repeating: "я", count: 65)), field: "name") // 130 bytes
        assertInvalid(core() + "<key>memory_mib</key><integer>255</integer>", field: "memory_mib")
        assertInvalid(core() + "<key>memory_mib</key><integer>65537</integer>", field: "memory_mib")
        assertInvalid(core() + "<key>memory_mib</key><real>4096</real>", field: "memory_mib") // a real is not an integer
        assertInvalid(core() + "<key>memory_mib</key><string>4096</string>", field: "memory_mib")
        assertInvalid(core() + "<key>vcpu_count</key><integer>2</integer>", field: "vcpu_count")
        assertInvalid(core() + "<key>vcpu_count</key><integer>0</integer>", field: "vcpu_count")
        assertInvalid(core() + "<key>audio</key><integer>1</integer>", field: "audio") // 0/1 is not a boolean
        assertInvalid(core() + "<key>clipboard_sync</key><string>true</string>", field: "clipboard_sync")
        assertInvalid(core() + "<key>network</key><integer>0</integer>", field: "network")
        assertInvalid(core() + "<key>display</key><array/>", field: "display")
        assertInvalid(core() + "<key>display</key><dict><key>screens</key><integer>3</integer></dict>", field: "display.screens")
        assertInvalid(core() + "<key>boot_media</key><data></data>", field: "boot_media")
        assertInvalid(core() + "<key>disks</key><dict/>", field: "disks")
        let disk = "<dict><key>path</key><string>disks/a</string><key>format</key><string>raw</string></dict>"
        assertInvalid(core() + "<key>disks</key><array>\(String(repeating: disk, count: 5))</array>", field: "disks")
        assertInvalid(core() + "<key>disks</key><array><string>x</string></array>", field: "disks[0]")
        assertInvalid(core() + "<key>disks</key><array><dict><key>format</key><string>raw</string></dict></array>", field: "disks[0]")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>disks/a</string><key>bookmark</key><data>AAEC</data><key>format</key><string>raw</string></dict></array>",
                      field: "disks[0]")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>disks/a/b.img</string><key>format</key><string>raw</string></dict></array>",
                      field: "disks[0].path")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>/disks/a</string><key>format</key><string>raw</string></dict></array>",
                      field: "disks[0].path")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>disks/..</string><key>format</key><string>raw</string></dict></array>",
                      field: "disks[0].path")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>disks/a</string></dict></array>", field: "disks[0].format")
        assertInvalid(core() + "<key>disks</key><array><dict><key>path</key><string>disks/a</string><key>format</key><integer>1</integer></dict></array>",
                      field: "disks[0].format")
        assertInvalid(core() + "<key>disks</key><array>\(disk)\(disk)</array>", field: "disks[1].path")
        assertInvalid(core() + "<key>shared_folders</key><array><dict><key>guest_name</key><string>x</string></dict></array>",
                      field: "shared_folders[0].bookmark")
        assertInvalid(core() + "<key>shared_folders</key><array><dict><key>bookmark</key><data>AAEC</data><key>guest_name</key><string>a:b</string></dict></array>",
                      field: "shared_folders[0].guest_name")
        let folder = "<dict><key>bookmark</key><data>AAEC</data><key>guest_name</key><string>same</string></dict>"
        assertInvalid(core() + "<key>shared_folders</key><array>\(folder)\(folder)</array>", field: "shared_folders[1].guest_name") // 3.3: bytewise unique
    }

    func testInvalidFiles() throws {
        XCTAssertEqual(BundleConfigCodec.parse(data: Data("not a plist".utf8)).kind, .invalid)
        XCTAssertEqual(BundleConfigCodec.parse(data: Data("<?xml version=\"1.0\"?><plist version=\"1.0\"><array/></plist>".utf8)).kind, .invalid)
        XCTAssertEqual(BundleConfigCodec.parse(data: Data(repeating: 0x20, count: (1 << 20) + 1)).kind, .invalid)
        // schema_version: the wrong type or below 1 is damage, not "newer".
        XCTAssertEqual(state("<key>schema_version</key><string>1</string>").kind, .invalid)
        XCTAssertEqual(state("<key>schema_version</key><real>1</real>").kind, .invalid)
        XCTAssertEqual(state("<key>schema_version</key><integer>0</integer>").kind, .invalid)
        // No file: an incomplete bundle, shown as damaged.
        let bundle = try makeBundle()
        XCTAssertEqual(BundleConfigCodec.read(bundle: bundle), .invalid(.fileMissing))
        // A file over 1 MiB on disk.
        try Data(repeating: 0x20, count: (1 << 20) + 1).write(to: bundle.appendingPathComponent("config.plist"))
        XCTAssertEqual(BundleConfigCodec.read(bundle: bundle).kind, .invalid)
    }

    // MARK: incompatible (3.1 step 4)

    func testIncompatibleClasses() {
        XCTAssertEqual(state("<key>schema_version</key><integer>2</integer>").kind, .incompatible) // nothing else is read
        assertIncompatible(core(profile: "display-probe-stage10"), field: "machine_profile")
        assertIncompatible(core() + "<key>network</key><string>bridged</string>", field: "network")
        assertIncompatible(core() + "<key>disks</key><array><dict><key>path</key><string>disks/a</string><key>format</key><string>qcow2</string></dict></array>",
                           field: "disks[0].format")
        // An unknown enumeration value wins over another violation: the file may come from a newer version.
        assertIncompatible(core(profile: "future-profile") + "<key>memory_mib</key><string>lots</string>", field: "machine_profile")
        assertIncompatible(core() + "<key>network</key><string>wifi</string><key>memory_mib</key><integer>1</integer>", field: "network")
        // A wrong type of an enumeration field is damage, not "newer".
        XCTAssertEqual(state(core() + "<key>network</key><integer>5</integer>").kind, .invalid)
    }

    // MARK: writing

    private func validConfigOnDisk(in bundle: URL, extra: String = "") throws -> BundleConfig {
        try Data(document(core() + extra).utf8).write(to: bundle.appendingPathComponent("config.plist"))
        guard case .valid(let config) = BundleConfigCodec.read(bundle: bundle) else { throw XCTSkip("fixture is not valid") }
        return config
    }

    func testRoundTripKeepsUnknownKeysAndWritesDefaultsExplicitly() throws {
        let bundle = try makeBundle()
        let extra = """
        <key>x_top</key><dict><key>nested</key><array><integer>7</integer></array></dict>
        <key>display</key><dict><key>screens</key><integer>1</integer><key>x_display</key><string>d</string></dict>
        <key>disks</key><array><dict><key>path</key><string>disks/d0</string><key>format</key><string>raw</string><key>x_disk</key><true/></dict></array>
        <key>shared_folders</key><array><dict><key>bookmark</key><data>AAEC</data><key>guest_name</key><string>s</string><key>x_folder</key><integer>3</integer></dict></array>
        """
        var config = try validConfigOnDisk(in: bundle, extra: extra)
        config.name = "Renamed"
        config.memoryMiB = 2048
        try BundleConfigStore.write(config, bundle: bundle)

        let reread = BundleConfigCodec.read(bundle: bundle)
        guard case .valid(let after) = reread else { return XCTFail("expected valid: \(reread)") }
        XCTAssertEqual(after, config)
        XCTAssertEqual(after.name, "Renamed")
        XCTAssertEqual(after.identity, identity, "the identity never changes")
        XCTAssertEqual(NSDictionary(dictionary: after.unknownKeys.values), ["x_top": ["nested": [7]]] as NSDictionary)
        XCTAssertEqual(after.displayUnknownKeys.values["x_display"] as? String, "d")
        // By plist type: `as? Bool` also takes the integers 0 and 1, and `as? Int` takes a boolean.
        let diskFlag = try XCTUnwrap(after.disks.first?.unknownKeys.values["x_disk"])
        XCTAssertEqual(CFGetTypeID(diskFlag as CFTypeRef), CFBooleanGetTypeID())
        XCTAssertEqual(PlistType.bool(diskFlag), true)
        let folderNumber = try XCTUnwrap(after.sharedFolders.first?.unknownKeys.values["x_folder"])
        XCTAssertEqual(CFGetTypeID(folderNumber as CFTypeRef), CFNumberGetTypeID())
        XCTAssertNil(PlistType.bool(folderNumber))
        XCTAssertEqual(PlistType.integer(folderNumber), 3)
        // Defaults are explicit in the new file (5.2), and the file is the XML format.
        let text = try String(contentsOf: bundle.appendingPathComponent("config.plist"), encoding: .utf8)
        for key in ["memory_mib", "vcpu_count", "network", "audio", "clipboard_sync", "writable", "read_only"] {
            XCTAssertTrue(text.contains("<key>\(key)</key>"), key)
        }
        XCTAssertTrue(text.hasPrefix("<?xml"))
    }

    func testAtomicWriteLeavesNoTemporaryFileAndRemovesOnlyOwnPatternWhenStale() throws {
        let bundle = try makeBundle()
        let config = try validConfigOnDisk(in: bundle)
        let stale = bundle.appendingPathComponent(".config.plist.tmp-deadbeef")
        let foreign = bundle.appendingPathComponent(".config.plist.other")
        let unknown = bundle.appendingPathComponent("notes.txt")
        for url in [stale, foreign, unknown] { try Data("x".utf8).write(to: url) }
        try BundleConfigStore.write(config, bundle: bundle, suffix: "cafe0001")
        let names = try FileManager.default.contentsOfDirectory(atPath: bundle.path).sorted()
        XCTAssertEqual(names, [".config.plist.other", "config.plist", "lock", "notes.txt"], "an unknown entry is never deleted (VM_BUNDLE 2 item 2)")
        var info = stat()
        XCTAssertEqual(stat(bundle.appendingPathComponent("config.plist").path, &info), 0)
        XCTAssertEqual(info.st_mode & 0o777, 0o600)
    }

    func testFailedWriteKeepsTheOldFileAndLeavesNoTemporaryFile() throws {
        let bundle = try makeBundle()
        var config = try validConfigOnDisk(in: bundle)
        let before = try Data(contentsOf: bundle.appendingPathComponent("config.plist"))
        config.name = "Never written"
        AtomicReplace.fault = .beforeRename // the temporary file is written completely, then the process "stops"
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: bundle)) { error in
            XCTAssertEqual(error as? BundleConfigWriteError, .io(EIO))
        }
        AtomicReplace.fault = nil
        XCTAssertEqual(try Data(contentsOf: bundle.appendingPathComponent("config.plist")), before)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: bundle.path).filter { $0.hasPrefix(".config.plist.tmp-") }, [])
    }

    func testIncompatibleAndInvalidFilesAreNotRewritten() throws {
        let config = BundleConfig(name: "x")
        for text in [document("<key>schema_version</key><integer>2</integer><key>x</key><string>future</string>"),
                     document(core() + "<key>memory_mib</key><integer>1</integer>")] {
            let bundle = try makeBundle(text)
            let before = try Data(contentsOf: bundle.appendingPathComponent("config.plist"))
            XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: bundle)) { error in
                guard case .currentFileNotValid = error as? BundleConfigWriteError else { return XCTFail("\(error)") }
            }
            XCTAssertEqual(try Data(contentsOf: bundle.appendingPathComponent("config.plist")), before)
            XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: bundle.path).filter { $0.hasPrefix(".config.plist.tmp-") }, [])
        }
        // An update of a bundle without config.plist is refused too; only `create` makes the first file.
        let empty = try makeBundle()
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: empty))
        try BundleConfigStore.write(config, bundle: empty, mode: .create)
        XCTAssertEqual(BundleConfigCodec.read(bundle: empty).config, config)
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: empty, mode: .create)) { error in
            XCTAssertEqual(error as? BundleConfigWriteError, .alreadyExists)
        }
    }

    func testAnUpdateWithAnotherIdentityIsRefusedAndTheFileIsNotTouched() throws {
        let bundle = try makeBundle()
        var config = try validConfigOnDisk(in: bundle)
        let before = try Data(contentsOf: bundle.appendingPathComponent("config.plist"))
        config.identity = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        config.name = "Another machine"
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: bundle)) { error in
            guard case .invalidConfig(let issue) = error as? BundleConfigWriteError else { return XCTFail("\(error)") }
            XCTAssertEqual(issue.field, "identity")
        }
        XCTAssertEqual(try Data(contentsOf: bundle.appendingPathComponent("config.plist")), before)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: bundle.path).filter { $0.hasPrefix(".config.plist.tmp-") }, [])
    }

    func testUnknownKeysCompareByPlistTypeAndDiskNamesByBytes() throws {
        XCTAssertNotEqual(UnknownKeys(["k": true]), UnknownKeys(["k": 1]), "@YES is not @1")
        XCTAssertNotEqual(UnknownKeys(["k": [true]]), UnknownKeys(["k": [1]]))
        XCTAssertNotEqual(UnknownKeys(["k": 1]), UnknownKeys(["k": 1.0]))
        XCTAssertEqual(UnknownKeys(["k": ["a": [1, "x", Data([1])]]]), UnknownKeys(["k": ["a": [1, "x", Data([1])]]]))
        // Canonically equivalent names are two names (bytes differ): not a duplicate.
        let composed = "\u{00E9}", decomposed = "e\u{0301}"
        let disks = [composed, decomposed].map {
            "<dict><key>path</key><string>disks/\($0)</string><key>format</key><string>raw</string></dict>"
        }.joined()
        guard case .valid(let config) = state(core() + "<key>disks</key><array>\(disks)</array>") else { return XCTFail("expected valid") }
        XCTAssertEqual(config.disks.count, 2)
        let first = "<dict><key>path</key><string>disks/\(composed)</string><key>format</key><string>raw</string></dict>"
        assertInvalid(core() + "<key>disks</key><array>\(disks)\(first)</array>", field: "disks[2].path")
    }

    func testAConfigThatWouldNotBeValidIsNotEncoded() throws {
        let bundle = try makeBundle()
        var config = BundleConfig(name: "x")
        XCTAssertEqual(config.identity, config.identity.lowercased(), "a new identity is lower case")
        config.identity = UUID().uuidString // upper case
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: bundle, mode: .create)) { error in
            guard case .invalidConfig(let issue) = error as? BundleConfigWriteError else { return XCTFail("\(error)") }
            XCTAssertEqual(issue.field, "identity")
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("config.plist").path))
        config = BundleConfig(name: "x")
        config.memoryMiB = 100
        XCTAssertThrowsError(try BundleConfigCodec.encode(config))
    }

    // MARK: lock (2.1)

    func testWriteNeedsTheBundleLock() throws {
        let bundle = try makeBundle()
        let config = try validConfigOnDisk(in: bundle)
        let held = try BundleWriterLock.acquire(bundle: bundle) // the VMHost's place
        XCTAssertThrowsError(try BundleConfigStore.write(config, bundle: bundle)) { error in
            XCTAssertEqual(error as? BundleConfigWriteError, .lock(.busy))
        }
        XCTAssertEqual(BundleWriterLock.probeFailure(bundle: bundle), .busy)
        held.release()
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundle))
        XCTAssertNoThrow(try BundleConfigStore.write(config, bundle: bundle))
    }

    func testLockProbeCreatesNothingAndRejectsAnIrregularLock() throws {
        let bundle = try makeBundle()
        XCTAssertNil(BundleWriterLock.probeFailure(bundle: bundle), "no lock file: not busy")
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.appendingPathComponent("lock").path), "a probe creates nothing")
        // A symbolic link instead of the lock file is `invalid`, not `busy`.
        try FileManager.default.createSymbolicLink(at: bundle.appendingPathComponent("lock"), withDestinationURL: bundle.appendingPathComponent("elsewhere"))
        XCTAssertEqual(BundleWriterLock.probeFailure(bundle: bundle), .invalid)
        XCTAssertThrowsError(try BundleWriterLock.acquire(bundle: bundle)) { error in
            XCTAssertEqual(error as? BundleWriterLockFailure, .invalid)
        }
    }

    // MARK: control

    func testControlConfigureCarriesOnlyCheckedFieldsAndImageNames() throws {
        var config = BundleConfig(name: "x", machineProfile: .displayTwoScreensTest)
        config.memoryMiB = 8192
        config.network = .nat
        config.disks = [BundleDisk(location: .inBundle(name: "disk0.img"), writable: false),
                        BundleDisk(location: .external(bookmark: Data([1])))]
        config.bootMedia = Data([1, 2])
        config.sharedFolders = [BundleSharedFolder(bookmark: Data([1]), readOnly: false, guestName: "share")]
        let configure = config.controlConfigure()
        XCTAssertNil(configure.validate())
        XCTAssertEqual(configure.machineProfile, 4)
        XCTAssertEqual(configure.displayScreens, 2)
        XCTAssertEqual(configure.network, 1)
        XCTAssertEqual(configure.bootMedia, 1)
        XCTAssertEqual(configure.disks[0], .init(source: 1, format: 1, writable: 0, name: Array("disk0.img".utf8)), "name only, no disks/ prefix")
        XCTAssertEqual(configure.disks[1].source, 2)
        XCTAssertEqual(configure.disks[1].name, [])
        XCTAssertEqual(configure.sharedFolders, [.init(readOnly: 0, guestName: Array("share".utf8))])
        XCTAssertEqual(try ControlV2Configure.parse(configure.encoded()), configure)
    }

    func testLaunchRefusalOrderAndMemory() {
        var config = BundleConfig(name: "x")
        config.memoryMiB = 4096
        XCTAssertNil(config.launchRefusal(hostMemoryBytes: 8 << 30)) // limit 4096 MiB: equal is allowed
        XCTAssertEqual(config.launchRefusal(hostMemoryBytes: 8 << 30 - 1), .hostMemoryExceeded(requestedMiB: 4096, limitMiB: 4095))
        config.network = .nat
        config.audio = true
        XCTAssertEqual(config.launchRefusal(hostMemoryBytes: 1 << 30), .unsupported(field: "network"), "unsupported comes before memory")
        config.disks = [BundleDisk(location: .inBundle(name: "d"))]
        XCTAssertEqual(config.launchRefusal(hostMemoryBytes: 1 << 30), .unsupportedDurability, "durability comes first")
    }
}
