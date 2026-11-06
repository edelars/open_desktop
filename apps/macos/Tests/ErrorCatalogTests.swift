import Foundation
@testable import ODShared
import XCTest
@testable import OpenDesktop

// The catalog of user-facing errors (S11-09, docs/specs/USER_FACING_ERRORS.md, Sources/Shared/ErrorCatalog.swift) and its presentation
// (Sources/App/ErrorPresentation.swift). The test of completeness is the point: it iterates the enumerations of the protocol
// (`CaseIterable`), so a new code without a text fails here. No window and no process. Run: `swift test --filter ErrorCatalog` from apps/.

final class ErrorCatalogXCTests: XCTestCase {
    /// Every entry of the catalog, whatever its source.
    private var allEntries: [ErrorEntry] {
        var entries: [ErrorEntry] = [ErrorCatalog.unknown]
        entries += ControlV2ErrorCode.allCases.map { ErrorCatalog.entry(for: $0) }
        entries += ControlV2AckCode.allCases.map { ErrorCatalog.entry(for: $0) }
        entries += ControlClientExit.Disposition.allCases.map { ErrorCatalog.entry(for: $0) }
        entries += ClientIssue.allCases.map { ErrorCatalog.entry(for: $0) }
        entries += LibraryIssue.allCases.map { ErrorCatalog.entry(for: $0) }
        entries += HostConditionIssue.allCases.map { ErrorCatalog.entry(for: $0) }
        return entries
    }

    // MARK: completeness

    func testEveryControlErrorCodeHasAnEntry() {
        XCTAssertEqual(ControlV2ErrorCode.allCases.count, 25, "CONTROL_PROTOCOL_V2.md 12.1 lists 25 codes")
        XCTAssertEqual(ControlV2ErrorCode.allCases.map(\.rawValue), Array(1...25), "the codes are 1...25 without a gap")
        for code in ControlV2ErrorCode.allCases {
            let entry = ErrorCatalog.entry(for: code)
            XCTAssertEqual(entry.key, "error.control." + code.wireName, "\(code)")
            XCTAssertNotEqual(entry, ErrorCatalog.unknown, "\(code) has its own words")
            XCTAssertEqual(ErrorCatalog.entry(forErrorCode: code.rawValue), entry, "\(code): the raw code finds the same entry")
        }
    }

    func testEveryAckCodeHasAnEntry() {
        XCTAssertEqual(ControlV2AckCode.allCases.map(\.rawValue), [ControlV2Ack.codeOK, ControlV2Ack.codeInProgress])
        for code in ControlV2AckCode.allCases {
            let entry = ErrorCatalog.entry(for: code)
            XCTAssertEqual(ErrorCatalog.entry(forAckCode: code.rawValue), entry)
            XCTAssertEqual(entry.severity, .info, "an ACK is not a failure")
            XCTAssertFalse(entry.showsDetails)
        }
    }

    func testEveryExitDispositionHasAnEntry() {
        XCTAssertEqual(ControlClientExit.Disposition.allCases.count, 9)
        for disposition in ControlClientExit.Disposition.allCases {
            let entry = ErrorCatalog.entry(for: disposition)
            XCTAssertTrue(entry.key.hasPrefix("exit."), "\(disposition)")
        }
        // Every status of 12.3 reaches its disposition, and a signal is never folded into a status.
        for (status, expected): (Int32, ControlClientExit.Disposition) in
            [(0, .stopped), (2, .usageError), (3, .startRejected), (4, .protocolError), (5, .transportLost), (6, .hostFailed), (7, .unexpected), (255, .unexpected)] {
            XCTAssertEqual(ControlClientExit(termination: .exited(status), startRejection: nil).disposition, expected, "status \(status)")
        }
        XCTAssertEqual(ControlClientExit(termination: .signaled(9), startRejection: nil).disposition, .killedBySignal)
    }

    func testEveryClientAndLibraryIssueHasAnEntry() {
        XCTAssertEqual(ClientIssue.allCases.count, 8)
        for issue in ClientIssue.allCases { XCTAssertTrue(ErrorCatalog.entry(for: issue).key.hasPrefix("client."), "\(issue)") }
        for issue in LibraryIssue.allCases { XCTAssertTrue(ErrorCatalog.entry(for: issue).key.hasPrefix("library."), "\(issue)") }
        // S11-12: the conditions of the host have their words too (the test fails when a case is added without a text).
        XCTAssertEqual(HostConditionIssue.allCases.count, 7)
        for issue in HostConditionIssue.allCases { XCTAssertTrue(ErrorCatalog.entry(for: issue).key.hasPrefix("host."), "\(issue)") }
        // The sources map to a case, none to a hole.
        XCTAssertEqual(ClientIssue(.connectionClosed), .connectionClosed)
        XCTAssertEqual(ClientIssue(.io(5)), .internalFailure)
        XCTAssertEqual(ClientIssue(.protocolViolation("x")), .protocolViolation)
        XCTAssertEqual(LibraryIssue(CatalogError.limitReached), .entryLimitReached)
        XCTAssertEqual(LibraryIssue(CatalogError.io(1)), .indexWriteFailed)
        XCTAssertEqual(LibraryIssue(CatalogWarning.indexCorrupt("x")), .indexCorrupt)
        XCTAssertEqual(LibraryIssue(CatalogWarning.limitReached), .scanLimitReached)
        XCTAssertEqual(LibraryIssue(errno: ENOSPC), .noSpace)
        XCTAssertEqual(LibraryIssue(errno: EACCES), .permissionDenied)
        XCTAssertEqual(LibraryIssue(errno: 9999), .fileSystemError)
        XCTAssertNil(LibraryIssue(CatalogBundleStatus.config(.valid(BundleConfig(name: "Тест")))))
        XCTAssertEqual(LibraryIssue(CatalogBundleStatus.directoryMissing), .bundleDirectoryMissing)
    }

    /// The enumeration of the code is the list of the specification, not only a list that happens to be complete: the table of 12.1 is parsed.
    func testCodesMatchTheSpecificationTable() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let spec = root.appendingPathComponent("protocols/control/CONTROL_PROTOCOL_V2.md")
        guard let text = try? String(contentsOf: spec, encoding: .utf8) else { throw XCTSkip("the specification is not next to the sources") }
        guard let start = text.range(of: "### 12.1."), let end = text.range(of: "### 12.2.") else { return XCTFail("sections 12.1 and 12.2 not found") }
        var fromSpec: [UInt32: String] = [:]
        for line in text[start.upperBound..<end.lowerBound].split(separator: "\n") {
            let cells = line.split(separator: "|", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
            guard cells.count > 3, let number = UInt32(cells[1]) else { continue }
            fromSpec[number] = cells[2].trimmingCharacters(in: CharacterSet(charactersIn: "`"))
        }
        XCTAssertEqual(fromSpec.count, 25)
        XCTAssertEqual(fromSpec.count, ControlV2ErrorCode.allCases.count, "the codes of 12.1 and the enumeration are the same set")
        for code in ControlV2ErrorCode.allCases {
            XCTAssertEqual(fromSpec[code.rawValue], code.wireName, "code \(code.rawValue)")
        }
    }

    // MARK: unknown

    func testUnknownCodeIsTheUnknownErrorWithDetails() {
        for raw: UInt32 in [0, 26, 99, 0xFFFF_FFFF] {
            let entry = ErrorCatalog.entry(forErrorCode: raw)
            XCTAssertEqual(entry.title, "Неизвестная ошибка", "\(raw)")
            XCTAssertTrue(entry.showsDetails)
            XCTAssertFalse(entry.retrySafe)
        }
        XCTAssertEqual(ErrorCatalog.entry(forAckCode: 2).title, "Неизвестная ошибка")
        let report = ControlV2ErrorReport(code: 99, failedKind: 6, state: ControlV2State.running.rawValue)
        let presentation = ErrorPresentation.control(report, context: ErrorContext(requestID: 4, bundleURL: nil))
        XCTAssertEqual(presentation.title, "Неизвестная ошибка")
        XCTAssertEqual(presentation.details?.contains("Код: неизвестный (99)"), true)
        XCTAssertEqual(presentation.details?.contains("Запрос: 4"), true)
    }

    // MARK: rules of the text

    func testNoEntryIsEmptyAndKeysAreUnique() {
        var keys = Set<String>()
        for entry in allEntries {
            XCTAssertFalse(entry.title.isEmpty, entry.key)
            XCTAssertFalse(entry.explanation.isEmpty, entry.key)
            XCTAssertFalse(entry.action.isEmpty, entry.key)
            XCTAssertFalse(entry.title.trimmingCharacters(in: .whitespaces).isEmpty, entry.key)
            XCTAssertTrue(keys.insert(entry.key).inserted, "duplicate key \(entry.key)")
            XCTAssertEqual(entry.titleKey, entry.key + ".title")
            XCTAssertNotNil(entry.key.range(of: "^[a-z]+(\\.[a-z_]+)+$", options: .regularExpression), "key \(entry.key)")
        }
        XCTAssertEqual(allEntries.count, 1 + 25 + 2 + 9 + 8 + LibraryIssue.allCases.count + HostConditionIssue.allCases.count)
    }

    func testTitlesHaveNoCodesDigitsOrPaths() {
        for entry in allEntries {
            XCTAssertNil(entry.title.range(of: "[0-9]", options: .regularExpression), "digit in the title of \(entry.key)")
            XCTAssertFalse(entry.title.contains("/"), entry.key)
            XCTAssertFalse(entry.title.contains("_"), entry.key)
            XCTAssertNil(entry.title.range(of: "[A-Za-z]{3,}", options: .regularExpression), "the title is in Russian: \(entry.title)")
            XCTAssertFalse(entry.title.hasSuffix("."), entry.key)
        }
    }

    func testTextsAreInRussianWithoutCodeNamesOrPaths() {
        for entry in allEntries {
            for text in [entry.explanation, entry.action] {
                XCTAssertNil(text.range(of: "[a-z]+_[a-z_]+", options: .regularExpression), "a code name in the text of \(entry.key)")
                XCTAssertFalse(text.contains(".swift") || text.contains("Fatal error") || text.contains("stack"), "no stack trace: \(entry.key)")
                XCTAssertFalse(text.contains("/Users/") || text.contains("/private/"), entry.key)
            }
        }
    }

    func testActionNeverSaysRepeatWhereRepeatIsNotSafe() {
        let repeatWords = ["повтор", "снова", "ещё раз", "еще раз", "попробуйте", "позже"]
        for entry in allEntries where !entry.retrySafe {
            let action = entry.action.lowercased()
            for word in repeatWords { XCTAssertFalse(action.contains(word), "«\(word)» in the action of \(entry.key)") }
        }
        // The cases where the state of the resources is unknown after the failure are never retry-safe.
        for code in [ControlV2ErrorCode.hostFailure, .ioError, .unsupportedDurability, .notConfigured] {
            XCTAssertFalse(ErrorCatalog.entry(for: code).retrySafe, "\(code)")
        }
        for disposition in [ControlClientExit.Disposition.hostFailed, .killedBySignal, .transportLost, .protocolError, .unexpected] {
            XCTAssertFalse(ErrorCatalog.entry(for: disposition).retrySafe, "\(disposition)")
        }
    }

    func testRequiredWordsOfThePlan() {
        let signal = ErrorCatalog.entry(for: .killedBySignal)
        XCTAssertEqual(signal.title, "Процесс машины аварийно завершился")
        XCTAssertEqual(signal.severity, .critical)
        XCTAssertTrue(signal.explanation.contains("не ошибка гостевой системы"), "a signal is not an error of the guest")
        XCTAssertTrue(ErrorCatalog.entry(for: ControlV2ErrorCode.hostFailure).explanation.contains("не ошибка гостевой системы"))
        let durability = ErrorCatalog.entry(for: ControlV2ErrorCode.unsupportedDurability)
        XCTAssertTrue(durability.explanation.contains("не временная ошибка"), "unsupported_durability is not hidden behind a temporary error")
        XCTAssertFalse(durability.retrySafe)
        XCTAssertEqual(ErrorCatalog.entry(for: ControlV2ErrorCode.busy).title, "Машина уже запущена другим процессом")
        // Information is not a failure; a failure is not information.
        XCTAssertEqual(ErrorCatalog.entry(for: .stopped).severity, .info)
        for code in ControlV2ErrorCode.allCases { XCTAssertNotEqual(ErrorCatalog.entry(for: code).severity, .info, "\(code)") }
    }

    func testRefusalsOfTheApplicationUseTheWordsOfTheirCode() {
        XCTAssertEqual(ErrorCatalog.entry(for: LaunchRefusal.unsupportedDurability), ErrorCatalog.entry(for: ControlV2ErrorCode.unsupportedDurability))
        XCTAssertEqual(ErrorCatalog.entry(for: LaunchRefusal.unsupported(field: "audio")), ErrorCatalog.entry(for: ControlV2ErrorCode.unsupported))
        XCTAssertEqual(
            ErrorCatalog.entry(for: LaunchRefusal.hostMemoryExceeded(requestedMiB: 1, limitMiB: 1)), ErrorCatalog.entry(for: ControlV2ErrorCode.hostMemoryExceeded))
        XCTAssertEqual(ErrorCatalog.entry(for: StartRejection.busy), ErrorCatalog.entry(for: ControlV2ErrorCode.busy))
        XCTAssertEqual(ErrorCatalog.entry(for: StartRejection.other(code: 77)), ErrorCatalog.unknown)
    }

    // MARK: presentation

    private func context(id: UInt32? = nil, bundle: URL? = URL(fileURLWithPath: "/tmp/Test.odvm"), logExists: Bool = false) -> ErrorContext {
        ErrorContext(requestID: id, bundleURL: bundle, date: Date(timeIntervalSince1970: 0), logExists: { _ in logExists })
    }

    func testDetailsHoldTheCodeTheRequestTheLogAndTheTime() throws {
        let report = ControlV2ErrorReport(code: .pauseTimeout, failedKind: ControlV2Kind.pause.rawValue, state: ControlV2State.running.rawValue)
        let presentation = ErrorPresentation.control(report, context: context(id: 3))
        XCTAssertEqual(presentation.title, "Машина не успела приостановиться")
        let details = try XCTUnwrap(presentation.details)
        XCTAssertTrue(details.contains("Код: pause_timeout (10)"), details)
        XCTAssertTrue(details.contains("Запрос: 3"), details)
        XCTAssertTrue(details.contains("Журнал: vmhost.log пока не ведётся"), "the log does not exist yet: the text says so, no invented path\n\(details)")
        XCTAssertNotNil(details.range(of: "Время: \\d{4}-\\d\\d-\\d\\d \\d\\d:\\d\\d:\\d\\d [+-]\\d{4}", options: .regularExpression), details)
        let logged = ErrorPresentation.control(report, context: context(id: 3, logExists: true))
        XCTAssertTrue(logged.details?.contains("Журнал: /tmp/Test.odvm/logs/vmhost.log") == true, "the path of the log, when there is one")
        let unknownRequest = ErrorPresentation.control(report, context: context())
        XCTAssertTrue(unknownRequest.details?.contains("Запрос: не определён") == true)
        XCTAssertFalse(presentation.title.contains("pause_timeout") || presentation.title.contains("10"), "the headline has no code")
    }

    func testEntryWithoutDetailsOffersNone() {
        let report = ControlV2ErrorReport(code: .operationInProgress, failedKind: ControlV2Kind.pause.rawValue, state: ControlV2State.running.rawValue)
        XCTAssertNil(ErrorPresentation.control(report, context: context(id: 2)).details)
    }

    func testDetailTextOfTheVMHostGoesOnlyToTheDetails() {
        let report = ControlV2ErrorReport(code: .invalidName, failedKind: 5, state: 1, aux0: 4, detail: "disk root.raw")
        let presentation = ErrorPresentation.control(report, context: context(id: 1))
        XCTAssertFalse(presentation.title.contains("root.raw") || presentation.message.contains("root.raw"))
        XCTAssertEqual(presentation.details?.contains("Сообщение процесса: disk root.raw"), true)
    }

    func testSignalIsAProcessCrashNotAGuestError() throws {
        let exit = ControlClientExit(termination: .signaled(9), startRejection: nil)
        let presentation = try XCTUnwrap(ErrorPresentation.exit(exit, context: context()))
        XCTAssertEqual(presentation.title, "Процесс машины аварийно завершился")
        XCTAssertEqual(presentation.details?.contains("Сигнал: 9"), true)
        XCTAssertEqual(presentation.details?.contains("Код: сигнал 9"), true)
        XCTAssertFalse(presentation.fullText.lowercased().contains("повтор"))
    }

    func testNormalEndAndUnavailableStatusSayNothing() {
        XCTAssertNil(ErrorPresentation.exit(ControlClientExit(termination: .exited(0), startRejection: nil)))
        XCTAssertNil(ErrorPresentation.exit(ControlClientExit(termination: .unavailable, startRejection: nil)))
    }

    func testRefusedStartIsToldByItsCodeAndKeepsTheStatus() throws {
        let exit = ControlClientExit(termination: .exited(3), startRejection: .ioError(errno: UInt64(EACCES)))
        let presentation = try XCTUnwrap(ErrorPresentation.exit(exit, context: context()))
        XCTAssertEqual(presentation.entry, ErrorCatalog.entry(for: ControlV2ErrorCode.ioError))
        let details = try XCTUnwrap(presentation.details)
        XCTAssertTrue(details.contains("Код: io_error (24)"), details)
        XCTAssertTrue(details.contains("errno: 13"), details)
        XCTAssertTrue(details.contains("Статус процесса: 3"), details)
    }

    func testLocalRefusalDetailsNameTheFieldAndSayThatNothingWasSpawned() throws {
        let presentation = ErrorPresentation.refusal(.unsupported(field: "audio"), context: context())
        let details = try XCTUnwrap(presentation.details)
        XCTAssertTrue(details.contains("Код: unsupported (4)"), details)
        XCTAssertTrue(details.contains("audio"), details)
        XCTAssertTrue(details.contains("Процесс машины не запускался"), details)
    }

    func testLibraryErrorsHaveNoLogLineAndNoPathInTheHeadline() throws {
        let presentation = ErrorPresentation.library(.notADirectory, technical: "Путь: /very/long/path", context: context())
        XCTAssertFalse(presentation.title.contains("/very"))
        let details = try XCTUnwrap(presentation.details)
        XCTAssertTrue(details.contains("/very/long/path"))
        XCTAssertFalse(details.contains("vmhost.log"), "a library error is not about a running machine")
        XCTAssertFalse(details.contains("Запрос:"))
    }

    func testRemovalErrorsMapToCatalogEntries() {
        XCTAssertEqual(BundleRemovalError.unsupportedDurability.issue, .removalUnsupportedDurability)
        XCTAssertEqual(BundleRemovalError.inUse.description, "машина запущена другим процессом")
        XCTAssertEqual(BundleRemovalError.io(EROFS).issue, .readOnlyVolume)
        XCTAssertEqual(BundleRemovalError.other("x").issue, .fileSystemError)
        XCTAssertFalse(BundleRemovalError.unsupportedDurability.description.contains("unsupported_durability"), "no raw code in the sentence")
    }
}
