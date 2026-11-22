import Darwin
import Foundation
import ODHostABI
import XCTest
@testable import od_vmhost

// S10-09: the probe mode of the test-only display smoke (`--display-smoke probe`): argument parsing and `ProbeGuestRun`, the quanta
// loop over od_display_session_step, against a real display session of OD_DISPLAY_PROFILE_STAGE10_PROBE (the own guest code of
// S10-08). The whole chain to a Metal frame is tests/system/display_end_to_end_smoke.py. From apps/ (the Rust staticlib first,
// `cargo build -p od-host --offline` from the repository root):
//
//   swift test --filter DisplayProbeSmokeXCTests

private let repositoryRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

private func makeProbeSession() -> OpaquePointer {
    var config = od_display_session_config()
    config.abi_version = vmhost_display_abi_version_v1()
    config.struct_size = UInt32(OD_DISPLAY_SESSION_CONFIG_SIZE_V1)
    config.profile = OD_DISPLAY_PROFILE_STAGE10_PROBE.rawValue
    var created: OpaquePointer?
    precondition(od_display_session_create(&config, &created) == OD_DISPLAY_STATUS_OK.rawValue)
    return created!
}

final class DisplayProbeSmokeXCTests: XCTestCase {
    func testParseProbeProfileAndBudget() throws {
        let plain = try XCTUnwrap(DisplaySmokeOptions.parse(["--socket-fd", "3", "--display-smoke", "probe"]))
        XCTAssertEqual(plain.profile, .probe)
        XCTAssertEqual(plain.probeBudget, DisplaySmokeOptions.defaultProbeBudget)
        XCTAssertEqual(plain.screens, 1)
        let budget = try XCTUnwrap(DisplaySmokeOptions.parse(
            ["--socket-fd", "3", "--display-smoke", "probe", "--display-probe-budget", "5000"]))
        XCTAssertEqual(budget.probeBudget, 5000)
        let pattern = try XCTUnwrap(DisplaySmokeOptions.parse(["--socket-fd", "3", "--display-smoke", "pattern"]))
        XCTAssertEqual(pattern.profile, .pattern)
    }

    func testParseRejectsWrongCombinations() {
        let bad: [[String]] = [
            ["--socket-fd", "3", "--display-smoke", "probe", "--display-screens", "2"], // the probe has one screen
            ["--socket-fd", "3", "--display-smoke", "pattern", "--display-probe-budget", "10"], // a budget without a guest
            ["--socket-fd", "3", "--display-smoke", "probe", "--display-probe-budget", "0"],
            ["--socket-fd", "3", "--display-smoke", "probe", "--display-probe-budget", "x"],
            ["--socket-fd", "3", "--display-smoke", "other"],
        ]
        for arguments in bad {
            XCTAssertThrowsError(try DisplaySmokeOptions.parse(arguments), "\(arguments)")
        }
    }

    func testProbeHaltsAndPublishesTheReferenceFrame() throws {
        let session = makeProbeSession()
        defer { _ = od_display_session_destroy(session) }
        // Before the guest runs there is no mode and no frame.
        XCTAssertEqual(try DisplaySmoke.screenMode(session, screen: 0).flags & 1, 0)
        let run = ProbeGuestRun(budget: DisplaySmokeOptions.defaultProbeBudget)
        var quanta = 0
        while run.isRunning {
            try run.advance(session: session)
            quanta += 1
            XCTAssertLessThan(quanta, 100, "the probe must reach HLT")
        }
        // 25 983 instructions in quanta of 8 192: four of them (tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.json).
        XCTAssertEqual(quanta, 4)
        try run.advance(session: session) // after HLT: nothing is issued, no error
        let mode = try DisplaySmoke.screenMode(session, screen: 0)
        XCTAssertEqual(mode.flags & 3, 3) // ACTIVE and HAS_FRAME
        XCTAssertEqual(mode.generation, 1)
        XCTAssertEqual([mode.width, mode.height], [64, 48])
        var frame = [UInt8](repeating: 0, count: 64 * 48 * 4)
        var info = od_display_frame_info()
        info.abi_version = vmhost_display_abi_version_v1()
        info.struct_size = UInt32(OD_DISPLAY_FRAME_INFO_SIZE_V1)
        XCTAssertEqual(od_display_session_read_screen_frame(session, 0, &frame, frame.count, &info), OD_DISPLAY_STATUS_OK.rawValue)
        let reference = Array(try Data(contentsOf: repositoryRoot.appendingPathComponent(
            "tests/fixtures/guest/stage10/bars_64x48_seed0.raw")))
        XCTAssertEqual(frame, reference)
    }

    func testBudgetSpentBeforeHltIsAFailure() throws {
        let session = makeProbeSession()
        defer { _ = od_display_session_destroy(session) }
        let run = ProbeGuestRun(budget: 10_000)
        XCTAssertThrowsError(try {
            for _ in 0..<100 { try run.advance(session: session) }
        }()) { error in
            XCTAssertTrue("\(error)".contains("did not reach HLT within 10000 instructions"), "\(error)")
        }
        XCTAssertFalse(run.halted)
    }
}
