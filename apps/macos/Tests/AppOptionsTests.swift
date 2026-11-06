import XCTest
@testable import OpenDesktop

final class AppOptionsXCTests: XCTestCase {
    func testNoArgumentsMeansBundledHelper() throws {
        XCTAssertNil(try AppOptions(arguments: []).vmhostOverride)
    }

    func testLaunchServicesArgumentIsIgnored() throws {
        let options = try AppOptions(arguments: ["-psn_0_12345", "--vmhost", "/x/od-vmhost"])
        XCTAssertEqual(options.vmhostOverride, "/x/od-vmhost")
    }

    func testVMHostOverrideBothForms() throws {
        XCTAssertEqual(try AppOptions(arguments: ["--vmhost", "/x/od-vmhost"]).vmhostOverride, "/x/od-vmhost")
        XCTAssertEqual(try AppOptions(arguments: ["--vmhost=/y/od-vmhost"]).vmhostOverride, "/y/od-vmhost")
    }

    func testUnknownSystemArgumentsAreIgnored() throws {
        let options = try AppOptions(arguments: ["-NSDocumentRevisionsDebugMode", "YES", "--vmhost", "/z"])
        XCTAssertEqual(options.vmhostOverride, "/z")
    }

    func testMissingAndEmptyValueAreErrors() {
        XCTAssertThrowsError(try AppOptions(arguments: ["--vmhost"])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .missingValue("--vmhost"))
        }
        XCTAssertThrowsError(try AppOptions(arguments: ["--vmhost="])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .emptyValue("--vmhost"))
        }
    }

    func testLocationInsideBundleAndOverride() throws {
        let bundle = URL(fileURLWithPath: "/Apps/OpenDesktop.app")
        let bundled = VMHostLocation.locate(options: try AppOptions(arguments: []), bundleURL: bundle) { _ in true }
        XCTAssertEqual(bundled.url.path, "/Apps/OpenDesktop.app/Contents/Helpers/od-vmhost")
        XCTAssertEqual(bundled.source, .bundle)
        XCTAssertTrue(bundled.isExecutable)

        let override = VMHostLocation.locate(options: try AppOptions(arguments: ["--vmhost", "/t/od-vmhost"]), bundleURL: bundle) { _ in false }
        XCTAssertEqual(override.url.path, "/t/od-vmhost")
        XCTAssertEqual(override.source, .argument)
        XCTAssertFalse(override.isExecutable)
    }

    func testOpenBundleBothFormsAndTogetherWithVMHost() throws {
        XCTAssertNil(try AppOptions(arguments: []).openBundle)
        XCTAssertEqual(try AppOptions(arguments: ["--open", "/b/A.odvm"]).openBundle, "/b/A.odvm")
        XCTAssertEqual(try AppOptions(arguments: ["--open=/b/B.odvm"]).openBundle, "/b/B.odvm")
        let both = try AppOptions(arguments: ["--vmhost", "/x/od-vmhost", "--open", "/b/C.odvm", "-psn_0_1"])
        XCTAssertEqual(both.vmhostOverride, "/x/od-vmhost")
        XCTAssertEqual(both.openBundle, "/b/C.odvm")
    }

    func testOpenMissingAndEmptyValueAreErrors() {
        XCTAssertThrowsError(try AppOptions(arguments: ["--open"])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .missingValue("--open"))
        }
        XCTAssertThrowsError(try AppOptions(arguments: ["--open="])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .emptyValue("--open"))
        }
    }

    func testAnOptionIsNotTheValueOfAnotherOption() {
        XCTAssertThrowsError(try AppOptions(arguments: ["--open", "--vmhost", "/x/od-vmhost"])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .missingValue("--open"))
        }
        XCTAssertThrowsError(try AppOptions(arguments: ["--vmhost", "--open", "/b/A.odvm"])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .missingValue("--vmhost"))
        }
        XCTAssertThrowsError(try AppOptions(arguments: ["--vmhost", "/x", "--open", "--other"])) {
            XCTAssertEqual($0 as? AppOptions.ParseError, .missingValue("--open"))
        }
        // The form with `=` states the value explicitly.
        XCTAssertEqual(try AppOptions(arguments: ["--open=--odd.odvm"]).openBundle, "--odd.odvm")
    }
}
