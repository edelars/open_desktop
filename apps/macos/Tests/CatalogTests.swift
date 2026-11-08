import Darwin
import Foundation
import XCTest
@testable import ODShared

// Tests of Shared/Catalog.swift (S11-06a): the index `Library.plist` of docs/specs/VM_BUNDLE.md section 8. A temporary directory stands
// in for `~/Library/Application Support/OpenDesktop` and `~/Documents/Open Desktop`; the user's real library is never touched.
// Locations are paths, not bookmarks (assumption of this slice, see Catalog.swift). Run: `swift test --filter CatalogXCTests` from apps/.

final class CatalogXCTests: XCTestCase {
    private var root: URL!
    private var indexURL: URL { root.appendingPathComponent("support/OpenDesktop/Library.plist") }
    private var defaultDirectory: URL { root.appendingPathComponent("Documents/Open Desktop", isDirectory: true) }

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("od-catalog-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: defaultDirectory, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: root)
        AtomicReplace.fault = nil
        super.tearDown()
    }

    private func makeCatalog() -> Catalog { Catalog(indexURL: indexURL, defaultDirectory: defaultDirectory) }

    /// A bundle directory with a valid `config.plist` (written through the product code) and the `lock` file.
    @discardableResult
    private func makeBundle(_ name: String, in directory: URL? = nil, identity: String = UUID().uuidString.lowercased()) throws -> URL {
        let bundle = (directory ?? defaultDirectory).appendingPathComponent("\(name).odvm", isDirectory: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        try BundleConfigStore.write(BundleConfig(name: name, identity: identity), bundle: bundle, mode: .create)
        return bundle
    }

    private func names(_ catalog: Catalog) -> [String] { catalog.entries.map { $0.bundleURL.lastPathComponent } }

    private func indexDictionary() throws -> [String: Any] {
        let object = try PropertyListSerialization.propertyList(from: Data(contentsOf: indexURL), options: [], format: nil)
        return object as! [String: Any]
    }

    private func writeRawIndex(_ root: [String: Any]) throws {
        try FileManager.default.createDirectory(at: indexURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try PropertyListSerialization.data(fromPropertyList: root, format: .xml, options: 0).write(to: indexURL)
    }

    private func temporaryFiles() throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: indexURL.deletingLastPathComponent().path).filter { $0.hasPrefix(".Library.plist.tmp-") }
    }

    // MARK: rebuild (8 item 3)

    func testRebuildFromTheDefaultDirectoryWhenTheIndexIsMissing() throws {
        let c = try makeBundle("C"), a = try makeBundle("A"), b = try makeBundle("B")
        // Not machines: a plain directory, a regular file and a link named `*.odvm` (links are not followed), a nested bundle.
        try FileManager.default.createDirectory(at: defaultDirectory.appendingPathComponent("plain"), withIntermediateDirectories: true)
        try Data().write(to: defaultDirectory.appendingPathComponent("file.odvm"))
        let elsewhere = root.appendingPathComponent("elsewhere.odvm")
        try FileManager.default.createDirectory(at: elsewhere, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: defaultDirectory.appendingPathComponent("link.odvm"), withDestinationURL: elsewhere)
        try makeBundle("Nested", in: defaultDirectory.appendingPathComponent("plain"))

        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.indexState, .missing)
        XCTAssertEqual(names(catalog), ["A.odvm", "B.odvm", "C.odvm"], "sorted by name, no recursion, no links")
        XCTAssertEqual(catalog.warnings, [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: indexURL.path), "load() writes nothing")

        XCTAssertTrue(try catalog.save())
        let again = makeCatalog()
        again.load()
        XCTAssertEqual(again.indexState, .loaded)
        XCTAssertEqual(names(again), ["A.odvm", "B.odvm", "C.odvm"])

        // The loss of the index loses no machine: the bundles are where they were.
        try FileManager.default.removeItem(at: indexURL)
        for bundle in [a, b, c] { XCTAssertEqual(BundleConfigCodec.read(bundle: bundle).kind, .valid) }
        let rebuilt = makeCatalog()
        rebuilt.load()
        XCTAssertEqual(names(rebuilt), ["A.odvm", "B.odvm", "C.odvm"])
    }

    func testAMissingDefaultDirectoryIsNotAnError() throws {
        try FileManager.default.removeItem(at: defaultDirectory)
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.entries, [])
        XCTAssertEqual(catalog.warnings, [])
    }

    func testTheOrderOfTheIndexIsKeptAndRescanAppendsNewBundlesAtTheEnd() throws {
        let z = try makeBundle("Z"), m = try makeBundle("M")
        let catalog = makeCatalog()
        catalog.load()
        _ = try catalog.remove(catalog.entries[0]) // M gone from the list
        _ = try catalog.add(bundle: m) // and added again at the end
        XCTAssertEqual(names(catalog), ["Z.odvm", "M.odvm"])
        try makeBundle("A")
        XCTAssertTrue(try catalog.rescanDefaultDirectory())
        XCTAssertEqual(names(catalog), ["Z.odvm", "M.odvm", "A.odvm"], "found bundles go to the end, the order of the index stays")
        let reloaded = makeCatalog()
        reloaded.load()
        XCTAssertEqual(names(reloaded), ["Z.odvm", "M.odvm", "A.odvm"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: z.path))
    }

    // MARK: invalid bundles do not crash

    func testInvalidBundlesAreListedAndReportedNotDropped() throws {
        let noConfig = defaultDirectory.appendingPathComponent("NoConfig.odvm")
        try FileManager.default.createDirectory(at: noConfig, withIntermediateDirectories: true)
        let broken = defaultDirectory.appendingPathComponent("Broken.odvm")
        try FileManager.default.createDirectory(at: broken, withIntermediateDirectories: true)
        try Data("<plist><dict>".utf8).write(to: broken.appendingPathComponent("config.plist"))
        let newer = defaultDirectory.appendingPathComponent("Newer.odvm")
        try FileManager.default.createDirectory(at: newer, withIntermediateDirectories: true)
        try Data("""
        <?xml version="1.0"?><plist version="1.0"><dict><key>schema_version</key><integer>2</integer></dict></plist>
        """.utf8).write(to: newer.appendingPathComponent("config.plist"))
        try makeBundle("Good")

        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(names(catalog), ["Broken.odvm", "Good.odvm", "NoConfig.odvm", "Newer.odvm"].sorted())
        func kind(_ name: String) -> BundleConfigClass? {
            guard let entry = catalog.entries.first(where: { $0.bundleURL.lastPathComponent == name }),
                  case .config(let state) = catalog.status(of: entry) else { return nil }
            return state.kind
        }
        XCTAssertEqual(kind("Broken.odvm"), .invalid)
        XCTAssertEqual(kind("NoConfig.odvm"), .invalid)
        XCTAssertEqual(kind("Newer.odvm"), .incompatible)
        XCTAssertEqual(kind("Good.odvm"), .valid)
        // Files of the damaged bundles are untouched by the catalog.
        XCTAssertEqual(try Data(contentsOf: broken.appendingPathComponent("config.plist")), Data("<plist><dict>".utf8))
        // A listed bundle that has been moved away is a status, not a crash; `add` of a non-directory is an error and changes nothing.
        let moved = catalog.entries[0]
        try FileManager.default.removeItem(at: moved.bundleURL)
        XCTAssertEqual(catalog.status(of: moved), .directoryMissing)
        XCTAssertThrowsError(try catalog.add(bundle: defaultDirectory.appendingPathComponent("nothing.odvm"))) { error in
            guard case .notADirectory = error as? CatalogError else { return XCTFail("\(error)") }
        }
    }

    // MARK: add and remove

    func testAddAndRemoveChangeTheIndexOnlyAndNeverDeleteFiles() throws {
        let outside = root.appendingPathComponent("Elsewhere", isDirectory: true)
        let first = try makeBundle("First", in: outside), second = try makeBundle("Second", in: outside)
        let catalog = makeCatalog()
        catalog.load()
        guard case .added(let entry, let persisted) = try catalog.add(bundle: first) else { return XCTFail("expected added") }
        XCTAssertTrue(persisted)
        XCTAssertEqual(entry.identityHint, BundleConfigCodec.read(bundle: first).config?.identity)
        _ = try catalog.add(bundle: second)
        XCTAssertEqual(names(catalog), ["First.odvm", "Second.odvm"])
        // The same directory again, also through another spelling of the path, is one entry.
        guard case .alreadyListed = try catalog.add(bundle: first.appendingPathComponent("..").appendingPathComponent("First.odvm")) else {
            return XCTFail("expected alreadyListed")
        }
        XCTAssertEqual(catalog.entries.count, 2)

        XCTAssertTrue(try catalog.remove(catalog.entries[0]))
        XCTAssertEqual(names(catalog), ["Second.odvm"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: first.appendingPathComponent("config.plist").path), "the bundle stays on disk")
        XCTAssertThrowsError(try catalog.remove(entry)) { error in
            guard case .notFound = error as? CatalogError else { return XCTFail("\(error)") }
        }
        let reloaded = makeCatalog()
        reloaded.load()
        XCTAssertEqual(reloaded.indexState, .loaded)
        XCTAssertEqual(names(reloaded), ["Second.odvm"], "a bundle outside the default directory comes back only through the index")
        // The index file: schema 1, a `machines` array, mode 0600 is not required but the directory is created.
        let file = try indexDictionary()
        XCTAssertEqual(file["schema_version"] as? Int, 1)
        XCTAssertEqual((file["machines"] as? [Any])?.count, 1)
    }

    func testAWriteReadsTheFileAgainSoAnotherWritersEntryIsNotLost() throws {
        let a = try makeBundle("A", in: root.appendingPathComponent("o")), b = try makeBundle("B", in: root.appendingPathComponent("o"))
        let one = makeCatalog(), two = makeCatalog()
        one.load()
        two.load()
        _ = try one.add(bundle: a)
        _ = try two.add(bundle: b) // `two` never saw A in memory; the step reads the file first
        XCTAssertEqual(names(two), ["A.odvm", "B.odvm"])
    }

    // MARK: atomic write (8 item 2, 5.2)

    func testIndexWriteIsAtomicAndLeavesNoTemporaryFile() throws {
        let a = try makeBundle("A"), b = try makeBundle("B")
        let catalog = makeCatalog()
        catalog.load()
        _ = try catalog.add(bundle: a)
        XCTAssertEqual(try temporaryFiles(), [])
        let before = try Data(contentsOf: indexURL)

        // A stop between the end of the temporary file and `rename`: the old index stays, our temporary file is removed.
        AtomicReplace.fault = .beforeRename
        XCTAssertThrowsError(try catalog.add(bundle: b)) { error in
            XCTAssertEqual(error as? CatalogError, .io(EIO))
        }
        AtomicReplace.fault = nil
        XCTAssertEqual(try Data(contentsOf: indexURL), before)
        XCTAssertEqual(try temporaryFiles(), [])

        // A temporary file of someone else is not deleted automatically: there is no lock on Library.plist (8 item 2).
        let foreign = indexURL.deletingLastPathComponent().appendingPathComponent(".Library.plist.tmp-foreign01")
        try Data("other writer".utf8).write(to: foreign)
        _ = try catalog.add(bundle: b)
        XCTAssertEqual(try temporaryFiles(), [".Library.plist.tmp-foreign01"])
        XCTAssertEqual(try String(contentsOf: foreign, encoding: .utf8), "other writer")
        var info = stat()
        XCTAssertEqual(stat(indexURL.path, &info), 0)
        XCTAssertEqual(info.st_mode & 0o777, 0o600)
    }

    // MARK: identity conflicts (8 item 5)

    func testDuplicateIdentityIsAConflictAndNothingIsRemoved() throws {
        let original = try makeBundle("Original", identity: "11111111-2222-4333-8444-555555555555")
        let copy = defaultDirectory.appendingPathComponent("Original copy.odvm")
        try FileManager.default.copyItem(at: original, to: copy)
        try makeBundle("Other")
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(names(catalog), ["Original copy.odvm", "Original.odvm", "Other.odvm"])
        let conflicts = catalog.identityConflicts
        XCTAssertEqual(conflicts.count, 1)
        XCTAssertEqual(conflicts[0].identity, "11111111-2222-4333-8444-555555555555")
        XCTAssertEqual(conflicts[0].entries.map { $0.bundleURL.lastPathComponent }, ["Original copy.odvm", "Original.odvm"])
        // Adding a third path to the same identity adds it (a conflict is shown, not refused or merged).
        let third = try makeBundle("Third", in: root.appendingPathComponent("o"), identity: "11111111-2222-4333-8444-555555555555")
        guard case .added = try catalog.add(bundle: third) else { return XCTFail("expected added") }
        XCTAssertEqual(catalog.identityConflicts[0].entries.count, 3)
        XCTAssertTrue(FileManager.default.fileExists(atPath: copy.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: original.path))
        // The hint does not decide whether an entry is shown: removing one entry leaves the others.
        _ = try catalog.remove(catalog.entries[0])
        XCTAssertEqual(catalog.identityConflicts[0].entries.count, 2)
    }

    // MARK: damaged and newer index (8 item 1)

    func testANewerIndexIsNeverRewritten() throws {
        let bundle = try makeBundle("A")
        let outside = try makeBundle("Outside", in: root.appendingPathComponent("o"))
        try writeRawIndex(["schema_version": 2, "machines": [["path": outside.path]], "x_future": "kept"])
        let before = try Data(contentsOf: indexURL)
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.indexState, .newerVersion(2))
        XCTAssertEqual(catalog.warnings, [.indexNewerVersion(2)])
        XCTAssertFalse(catalog.canPersist)
        // Only the scan (b): the entry of the newer file is not trusted, the bundle outside the default directory is not shown.
        XCTAssertEqual(names(catalog), ["A.odvm"])
        guard case .added(_, let persisted) = try catalog.add(bundle: outside) else { return XCTFail("expected added") }
        XCTAssertFalse(persisted, "the change lives for this run only")
        XCTAssertEqual(names(catalog), ["A.odvm", "Outside.odvm"])
        XCTAssertEqual(try Data(contentsOf: indexURL), before, "the file is neither replaced nor copied")
        XCTAssertFalse(try catalog.save())
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: indexURL.deletingLastPathComponent().path), ["Library.plist"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testADamagedIndexIsReplacedOnlyAfterACopyAndFilesStay() throws {
        let bundle = try makeBundle("A")
        try FileManager.default.createDirectory(at: indexURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        let garbage = Data("this is not a plist".utf8)
        try garbage.write(to: indexURL)
        let catalog = makeCatalog()
        catalog.load()
        guard case .corrupt = catalog.indexState else { return XCTFail("expected corrupt, got \(catalog.indexState)") }
        XCTAssertEqual(catalog.warnings.count, 1)
        XCTAssertEqual(names(catalog), ["A.odvm"], "rebuilt by scanning")
        XCTAssertEqual(try Data(contentsOf: indexURL), garbage, "load() does not touch the file")
        XCTAssertTrue(try catalog.save())
        let copies = try FileManager.default.contentsOfDirectory(atPath: indexURL.deletingLastPathComponent().path).filter { $0.hasPrefix("Library.plist.corrupt-") }
        XCTAssertEqual(copies.count, 1)
        XCTAssertEqual(try Data(contentsOf: indexURL.deletingLastPathComponent().appendingPathComponent(copies[0])), garbage)
        let reloaded = makeCatalog()
        reloaded.load()
        XCTAssertEqual(reloaded.indexState, .loaded)
        XCTAssertEqual(names(reloaded), ["A.odvm"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
    }

    func testEveryFileLevelViolationMeansCorrupt() throws {
        let cases: [(String, Data)] = [
            ("root not a dict", try PropertyListSerialization.data(fromPropertyList: [1, 2], format: .xml, options: 0)),
            ("no schema_version", try PropertyListSerialization.data(fromPropertyList: ["machines": []], format: .xml, options: 0)),
            ("schema_version string", try PropertyListSerialization.data(fromPropertyList: ["schema_version": "1"], format: .xml, options: 0)),
            ("schema_version 0", try PropertyListSerialization.data(fromPropertyList: ["schema_version": 0], format: .xml, options: 0)),
            ("machines not an array", try PropertyListSerialization.data(fromPropertyList: ["schema_version": 1, "machines": "x"], format: .xml, options: 0)),
            ("1001 elements", try PropertyListSerialization.data(
                fromPropertyList: ["schema_version": 1, "machines": (0..<1001).map { ["path": "/nowhere/\($0).odvm"] }], format: .xml, options: 0)),
            ("over 4 MiB", Data(repeating: 0x20, count: (4 << 20) + 1)),
        ]
        for (label, data) in cases {
            try FileManager.default.createDirectory(at: indexURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: indexURL)
            let catalog = makeCatalog()
            catalog.load()
            guard case .corrupt = catalog.indexState else { XCTFail("\(label): expected corrupt, got \(catalog.indexState)"); continue }
        }
    }

    func testElementsThatCannotBeShownAreKeptOnRewrite() throws {
        let shown = try makeBundle("Shown", in: root.appendingPathComponent("o"))
        let added = try makeBundle("Added", in: root.appendingPathComponent("o"))
        let bookmarkOnly: [String: Any] = ["bookmark": Data([1, 2, 3]), "x_bm": "future"]
        try writeRawIndex([
            "schema_version": 1, "x_top": ["a": 1],
            "machines": [
                ["path": shown.path, "identity": "not-a-uuid", "x_entry": true] as [String: Any],
                bookmarkOnly,
                "not a dict",
                ["path": 5] as [String: Any],
            ],
        ])
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.indexState, .loaded)
        XCTAssertEqual(names(catalog), ["Shown.odvm"], "the others are skipped in the list")
        XCTAssertEqual(catalog.entries[0].identityHint, BundleConfigCodec.read(bundle: shown).config?.identity, "a bad hint is dropped and filled in")
        _ = try catalog.add(bundle: added)
        let file = try indexDictionary()
        XCTAssertEqual(NSDictionary(dictionary: file["x_top"] as! [String: Any]), ["a": 1] as NSDictionary)
        let machines = file["machines"] as! [Any]
        XCTAssertEqual(machines.count, 5, "skipping is not deleting")
        XCTAssertEqual((machines[0] as! [String: Any])["x_entry"] as? Bool, true)
        XCTAssertEqual(NSDictionary(dictionary: machines[1] as! [String: Any]), bookmarkOnly as NSDictionary)
        XCTAssertEqual(machines[2] as? String, "not a dict")
        XCTAssertEqual((machines[3] as! [String: Any])["path"] as? Int, 5)
        XCTAssertEqual((machines[4] as! [String: Any])["path"] as? String, added.standardizedFileURL.path)
    }

    func testAnEntryWithPathAndBookmarkPassesThroughAndABookmarkOfTheWrongTypeIsKept() throws {
        let one = try makeBundle("One", in: root.appendingPathComponent("o"))
        let two = try makeBundle("Two", in: root.appendingPathComponent("o"))
        let added = try makeBundle("Added", in: root.appendingPathComponent("o"))
        let data = Data([9, 8, 7])
        try writeRawIndex([
            "schema_version": 1,
            "machines": [
                ["path": one.path, "bookmark": data, "x_one": 1] as [String: Any],
                ["path": two.path, "bookmark": "not data", "identity": 42] as [String: Any],
            ],
        ])
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(names(catalog), ["One.odvm", "Two.odvm"])
        XCTAssertEqual(catalog.entries[0].bookmark, data)
        XCTAssertNil(catalog.entries[1].bookmark)
        _ = try catalog.add(bundle: added)
        let machines = try indexDictionary()["machines"] as! [Any]
        let first = machines[0] as! [String: Any]
        XCTAssertEqual(first["bookmark"] as? Data, data)
        XCTAssertEqual(first["x_one"] as? Int, 1)
        let second = machines[1] as! [String: Any]
        XCTAssertEqual(second["bookmark"] as? String, "not data", "a bookmark that is not data is not lost on rewrite")
        XCTAssertEqual(second["path"] as? String, two.path)
        // The hint is filled from the bundle (it replaces the raw value that was no UUID).
        XCTAssertEqual(second["identity"] as? String, BundleConfigCodec.read(bundle: two).config?.identity)
    }

    func testIdentityHintsSurviveAChangeSoAConflictDoesNotDisappear() throws {
        let identity = "11111111-2222-4333-8444-555555555555"
        let original = try makeBundle("Original", in: root.appendingPathComponent("o"), identity: identity)
        let copy = root.appendingPathComponent("o/Original copy.odvm")
        try FileManager.default.copyItem(at: original, to: copy)
        let other = try makeBundle("Other", in: root.appendingPathComponent("o"))
        // An index of an older run: no hints in the file.
        try writeRawIndex(["schema_version": 1, "machines": [["path": original.path] as [String: Any], ["path": copy.path] as [String: Any]]])
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.identityConflicts.count, 1)
        _ = try catalog.add(bundle: other)
        XCTAssertEqual(catalog.identityConflicts.count, 1, "the fresh read of the file has no hints; they are filled again")
        XCTAssertEqual(catalog.identityConflicts[0].entries.count, 2)
        _ = try catalog.remove(catalog.entries[2])
        XCTAssertEqual(catalog.identityConflicts.count, 1)
        let machines = try indexDictionary()["machines"] as! [Any]
        XCTAssertEqual((machines[0] as! [String: Any])["identity"] as? String, identity)
    }

    // MARK: limit of 1000 entries (8 table and item 3)

    func testTheMergedListNeverExceedsTheLimitAndTheScanWarns() throws {
        for index in 0..<1_005 {
            try FileManager.default.createDirectory(
                at: defaultDirectory.appendingPathComponent(String(format: "M%04d.odvm", index)), withIntermediateDirectories: true)
        }
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.indexState, .missing, "the limit does not make the index damaged")
        XCTAssertEqual(catalog.entries.count, 1_000)
        XCTAssertEqual(catalog.warnings, [.limitReached])
        XCTAssertEqual(catalog.entries.last?.bundleURL.lastPathComponent, "M0999.odvm")
        XCTAssertTrue(try catalog.save())
        let reloaded = makeCatalog()
        reloaded.load()
        XCTAssertEqual(reloaded.indexState, .loaded)
        XCTAssertEqual(reloaded.entries.count, 1_000)
        // Adding to a full library is refused, nothing is written.
        let before = try Data(contentsOf: indexURL)
        let extra = try makeBundle("Extra", in: root.appendingPathComponent("o"))
        XCTAssertThrowsError(try reloaded.add(bundle: extra)) { error in
            XCTAssertEqual(error as? CatalogError, .limitReached)
        }
        XCTAssertEqual(try Data(contentsOf: indexURL), before)
        XCTAssertEqual(reloaded.entries.count, 1_000)
    }

    func testAnIndexWithExactlyThe1000ElementsIsValid() throws {
        try writeRawIndex(["schema_version": 1, "machines": (0..<1_000).map { ["path": "/nowhere/\($0).odvm"] }])
        let catalog = makeCatalog()
        catalog.load()
        XCTAssertEqual(catalog.indexState, .loaded)
        XCTAssertEqual(catalog.entries.count, 1_000)
        XCTAssertEqual(catalog.status(of: catalog.entries[0]), .directoryMissing)
    }
}
