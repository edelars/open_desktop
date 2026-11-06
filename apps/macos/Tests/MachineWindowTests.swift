import Foundation
@testable import ODDisplay
@testable import ODShared
import XCTest
@testable import OpenDesktop

// Tests of the machine window of S11-10 (Sources/App/MachineWindowModel.swift, and the window host of MachineController.swift).
// No window, no Metal, no process: the model decides, the controller only applies. Every machine here is test-only, without a guest.
// Run: `swift test --filter MachineWindow` from apps/.

private func text(_ run: MachineRunState, _ frame: FrameReaderState, detail: String? = nil) -> String? {
    MachineWindowModel.overlay(run: run, frame: frame, detail: detail)?.text
}

final class MachineWindowOverlayXCTests: XCTestCase {
    func testRunningAndShowingDrawsTheFrameWithoutText() {
        XCTAssertNil(MachineWindowModel.overlay(run: .running, frame: .showing))
    }

    func testRunningWithoutAFrameSaysNoSignal() {
        XCTAssertEqual(text(.running, .connecting), "Нет сигнала")
        XCTAssertEqual(text(.running, .noSignal), "Нет сигнала")
    }

    func testPauseIsTextEvenWhileTheReaderStillShowsTheLastFrame() {
        // The reader says `showing` (VMHost stopped writing, nothing disconnected): the window must not show the frozen frame.
        XCTAssertEqual(text(.paused, .showing), "Пауза")
        XCTAssertEqual(text(.paused, .noSignal), "Пауза")
    }

    func testStoppedAndFailedAreTextWhateverTheReaderSays() {
        for frame: FrameReaderState in [.showing, .connecting, .disconnected("EOF")] {
            XCTAssertEqual(text(.stopped, frame), "Остановлено")
            XCTAssertEqual(text(.failed, frame), "Остановлено: сбой машины")
        }
    }

    func testIncompatibleVersionIsNamedWithItsReason() {
        XCTAssertEqual(text(.running, .incompatible("version 9")), "Несовместимая версия\nversion 9")
        XCTAssertEqual(text(.paused, .incompatible("version 9")), "Несовместимая версия\nversion 9", "pause does not hide the cause")
        XCTAssertEqual(text(.running, .incompatible("version 9"), detail: "другая версия"), "Несовместимая версия\nдругая версия")
    }

    func testStoppedKeepsTheReasonOfAnIncompatibleReader() {
        XCTAssertEqual(text(.stopped, .incompatible("version 9")), "Остановлено\nНесовместимая версия: version 9")
    }

    func testOtherReaderStatesHaveTheirOwnWords() {
        XCTAssertEqual(text(.running, .modeChanging), "Смена режима экрана")
        XCTAssertEqual(text(.running, .disabled), "Экран отключён гостем")
        XCTAssertEqual(text(.running, .disconnected("EOF")), "Нет сигнала: VMHost отключился\nEOF")
        XCTAssertEqual(text(.shuttingDown, .showing), "Останавливается…")
        XCTAssertEqual(text(.starting, .connecting), "Запуск…")
    }

    /// Every combination of machine state and reader state is either a frame or a text; a frame only for `running` + `showing`.
    func testOnlyRunningAndShowingDrawsTheFrame() {
        let runs: [MachineRunState] = [.stopped, .starting, .running, .paused, .shuttingDown, .failed]
        let frames: [FrameReaderState] = [
            .connecting, .noSignal, .modeChanging, .showing, .disabled, .disconnected("x"), .incompatible("y"),
        ]
        for run in runs {
            for frame in frames {
                let overlay = MachineWindowModel.overlay(run: run, frame: frame)
                if run == .running && frame == .showing {
                    XCTAssertNil(overlay)
                } else {
                    XCTAssertNotNil(overlay, "\(run) \(frame.name) must not draw a (possibly frozen) frame")
                    XCTAssertFalse(overlay?.text.isEmpty ?? true)
                }
            }
        }
    }
}

final class MachineWindowTextXCTests: XCTestCase {
    func testTitleNamesTheModeOfInputAndTheChord() {
        XCTAssertEqual(
            MachineWindowModel.title(name: "Тест", run: .running, captured: false),
            "Тест — ввод не захвачен (⌃⌥⌘G — захватить)")
        XCTAssertEqual(
            MachineWindowModel.title(name: "Тест", run: .running, captured: true),
            "Тест — ввод захвачен (⌃⌥⌘G — освободить)")
    }

    func testTitleOfAMachineThatDoesNotRunHasNoCaptureText() {
        XCTAssertEqual(MachineWindowModel.title(name: "Тест", run: .paused, captured: false), "Тест — приостановлена")
        XCTAssertEqual(MachineWindowModel.title(name: "Тест", run: .stopped, captured: false), "Тест — остановлена")
    }

    func testSubtitleSaysThatInputIsNotDeliveredToTheGuest() {
        let subtitle = MachineWindowModel.subtitle(input: .ready(guestDelivery: false), profile: nil)
        XCTAssertEqual(subtitle, "ввод в гостевую систему пока не доставляется")
        XCTAssertEqual(
            MachineWindowModel.subtitle(input: .ready(guestDelivery: false), profile: "Тестовый узор (test-only, без гостя)"),
            "ввод в гостевую систему пока не доставляется · Тестовый узор (test-only, без гостя)")
    }

    func testSubtitleOfOtherInputPhases() {
        XCTAssertEqual(MachineWindowModel.subtitle(input: .connecting, profile: nil), "ввод: соединение…")
        XCTAssertEqual(MachineWindowModel.subtitle(input: .closed("EOF"), profile: nil), "ввод отключён")
        XCTAssertEqual(MachineWindowModel.subtitle(input: .incompatible(9), profile: nil), "ввод отключён: несовместимая версия протокола")
        XCTAssertEqual(MachineWindowModel.subtitle(input: .ready(guestDelivery: true), profile: nil), "ввод доставляется в гостевую систему")
        XCTAssertEqual(MachineWindowModel.subtitle(input: nil, profile: nil), "")
    }

    func testCaptureMenuTitle() {
        XCTAssertEqual(MachineWindowModel.captureMenuTitle(captured: false), "Захватить ввод")
        XCTAssertEqual(MachineWindowModel.captureMenuTitle(captured: true), "Освободить ввод")
    }
}

final class InputCaptureXCTests: XCTestCase {
    private let ready = InputPhase.ready(guestDelivery: false)

    func testStartsReleased() {
        XCTAssertEqual(InputCaptureModel().mode, .released)
        XCTAssertFalse(InputCaptureModel().isCaptured)
    }

    func testCanCaptureOnlyWhileRunningKeyAndReady() {
        XCTAssertTrue(InputCaptureModel.canCapture(run: .running, windowIsKey: true, input: ready))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .paused, windowIsKey: true, input: ready))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .stopped, windowIsKey: true, input: ready))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .shuttingDown, windowIsKey: true, input: ready))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .running, windowIsKey: false, input: ready))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .running, windowIsKey: true, input: .connecting))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .running, windowIsKey: true, input: .closed("EOF")))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .running, windowIsKey: true, input: .incompatible(2)))
        XCTAssertFalse(InputCaptureModel.canCapture(run: .running, windowIsKey: true, input: nil))
    }

    func testToggleCapturesAndReleases() {
        var model = InputCaptureModel()
        XCTAssertEqual(model.toggle(canCapture: true), .captured)
        XCTAssertTrue(model.isCaptured)
        XCTAssertEqual(model.toggle(canCapture: true), .released)
    }

    func testRefusedCaptureChangesNothing() {
        var model = InputCaptureModel()
        XCTAssertEqual(model.toggle(canCapture: false), .released)
    }

    func testReleaseChordWorksEvenWhenCaptureWouldBeRefused() {
        // The way out never depends on the conditions.
        var model = InputCaptureModel()
        model.toggle(canCapture: true)
        XCTAssertEqual(model.toggle(canCapture: false), .released)
    }

    func testLostConditionsReleaseTheCapture() {
        var model = InputCaptureModel()
        model.toggle(canCapture: true)
        XCTAssertFalse(model.enforce(canCapture: true), "still possible: stays captured")
        XCTAssertTrue(model.isCaptured)
        XCTAssertTrue(model.enforce(canCapture: false), "pause, stop, resigned key window or a closed channel")
        XCTAssertEqual(model.mode, .released)
        XCTAssertFalse(model.enforce(canCapture: false), "nothing to release twice")
    }

    func testReleaseIsUnconditional() {
        var model = InputCaptureModel()
        model.toggle(canCapture: true)
        model.release()
        XCTAssertEqual(model.mode, .released)
    }

    func testChordIsControlOptionCommandOnThePhysicalKeyG() {
        let chord = CaptureChord.toggle
        XCTAssertEqual(chord.keyCode, 5, "kVK_ANSI_G")
        XCTAssertTrue(chord.matches(keyCode: 5, control: true, option: true, command: true, shift: false))
        XCTAssertFalse(chord.matches(keyCode: 5, control: true, option: true, command: false, shift: false))
        XCTAssertFalse(chord.matches(keyCode: 5, control: true, option: false, command: true, shift: false))
        XCTAssertFalse(chord.matches(keyCode: 5, control: false, option: true, command: true, shift: false))
        XCTAssertFalse(chord.matches(keyCode: 5, control: true, option: true, command: true, shift: true))
        XCTAssertFalse(chord.matches(keyCode: 4, control: true, option: true, command: true, shift: false))
        XCTAssertEqual(CaptureChord.symbols, "⌃⌥⌘G")
    }

    func testTheKeyCodeIsTheOneOfTheHIDTable() {
        // `G` of the table of HIDUsage.swift (usage 0x0A): the chord names the same physical key that the guest would get.
        XCTAssertEqual(HIDUsage.usage(forKeyCode: CaptureChord.toggle.keyCode), 0x0A)
    }
}

final class WindowScalingXCTests: XCTestCase {
    func testTheTwoModesMapToThePresenterModes() {
        XCTAssertEqual(WindowScaling.fitWindow.presentMode, .fit)
        XCTAssertEqual(WindowScaling.pixelExact.presentMode, .pixelExact)
        XCTAssertEqual(WindowScaling(.fit), .fitWindow)
        XCTAssertEqual(WindowScaling(.pixelExact), .pixelExact)
        XCTAssertEqual(WindowScaling.allCases.count, 2)
    }

    func testMenuWords() {
        XCTAssertEqual(WindowScaling.fitWindow.menuTitle, "По размеру окна")
        XCTAssertEqual(WindowScaling.pixelExact.menuTitle, "Пиксель в пиксель")
    }
}

final class MachineWindowCloseXCTests: XCTestCase {
    func testRunningAndPausedAskBeforeClosing() {
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .running, hasSession: true, canStop: true), .confirmStop)
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .paused, hasSession: true, canStop: true), .confirmStop)
    }

    func testBusyMachineIsNotClosedBehindItsBack() {
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .running, hasSession: true, canStop: false), .refuse)
    }

    func testNothingRunningClosesWithoutAQuestion() {
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .stopped, hasSession: true, canStop: false), .closeNow)
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .failed, hasSession: true, canStop: false), .closeNow)
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .shuttingDown, hasSession: true, canStop: true), .closeNow)
        XCTAssertEqual(MachineWindowModel.closePolicy(run: .stopped, hasSession: false, canStop: false), .closeNow)
    }
}

// MARK: the window host of the controller

private final class HostSession: MachineSession {
    var displayTaken = 0
    var inputTaken = 0
    func requestConfigure(_ configure: ControlV2Configure, completion: @escaping ControlClientV2.Completion) {}
    func requestStart(completion: @escaping ControlClientV2.Completion) {}
    func requestPause(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion) {}
    func requestResume(reason: ControlV2Reason, completion: @escaping ControlClientV2.Completion) {}
    func requestStop(completion: @escaping ControlClientV2.Completion) {}
    func requestQuery(completion: @escaping ControlClientV2.Completion) {}
    func terminateSession() {}
    func takeDisplaySocket() -> Int32? { displayTaken += 1; return displayTaken == 1 ? 77 : nil }
    func takeInputSocket() -> Int32? { inputTaken += 1; return inputTaken == 1 ? 78 : nil }
}

private final class RecordingHost: MachineWindowHost {
    var states: [MachineRunState] = []
    func machine(_ machine: MachineController, didChange state: MachineRunState) { states.append(state) }
}

final class MachineWindowHostXCTests: XCTestCase {
    private var launches: [ControlClientLaunch] = []
    private var handlers: [ControlClientV2.EventHandler] = []
    private let session = HostSession()

    private func makeController(host: MachineWindowHost?) -> MachineController {
        let location = VMHostLocation(url: URL(fileURLWithPath: "/x/od-vmhost"), source: .argument, isExecutable: true)
        let entry = CatalogEntry(bundleURL: URL(fileURLWithPath: "/x/Тест.odvm"))
        let machine = MachineController(
            entry: entry, status: .config(.valid(BundleConfig(name: "Тест"))), vmhost: location,
            launcher: { [self] launch, handler in
                launches.append(launch)
                handlers.append(handler)
                return session
            }, hostMemoryBytes: 64 << 30)
        machine.windowHost = host
        return machine
    }

    func testWithoutHostNoFrameAndNoInputSocketIsRequested() {
        let machine = makeController(host: nil)
        machine.start()
        XCTAssertFalse(launches[0].displayChannel)
        XCTAssertFalse(launches[0].inputChannel)
    }

    func testWithHostBothSocketsAreRequestedAndHandedOutOnce() {
        let host = RecordingHost()
        let machine = makeController(host: host)
        machine.start()
        XCTAssertTrue(launches[0].displayChannel, "--display-fd 5")
        XCTAssertTrue(launches[0].inputChannel, "--input-fd 4")
        let first = machine.takeDisplayChannels()
        XCTAssertEqual(first.display, 77)
        XCTAssertEqual(first.input, 78)
        let second = machine.takeDisplayChannels()
        XCTAssertNil(second.display, "the frame socket is handed out once per run")
        XCTAssertNil(second.input)
    }

    func testHostSeesTheStatesThatEventsMake() {
        let host = RecordingHost()
        let machine = makeController(host: host)
        machine.start()
        handlers[0](.stateChanged(from: .stopped, to: .running, reason: 0, requestID: 0))
        handlers[0](.stateChanged(from: .running, to: .paused, reason: 0, requestID: 0))
        handlers[0](.exited(ControlClientExit(termination: .exited(0), startRejection: nil)))
        XCTAssertEqual(host.states, [.running, .paused, .stopped])
    }

    func testNoSessionNoSockets() {
        let machine = makeController(host: RecordingHost())
        let channels = machine.takeDisplayChannels()
        XCTAssertNil(channels.display)
        XCTAssertNil(channels.input)
    }
}

// MARK: the real VMHost, frames and input without a window

/// The path of S11-10 without the window: the real `od-vmhost` started with `--display-fd 5` and `--input-fd 4`, the frame reader of
/// `MachineFrameSession` with a real `MetalPresenter`, and the input channel. It shows that frames arrive, that the input channel
/// reaches READY with `GUEST_DELIVERY = 0`, and that the end of the machine ends the reader. Skipped without `od-vmhost` or a Metal device.
final class MachineWindowRealHostXCTests: XCTestCase {
    private func vmhostPath() -> String? {
        let candidates = [
            Bundle(for: Self.self).bundleURL.deletingLastPathComponent().appendingPathComponent("od-vmhost").path,
            URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent(".build/debug/od-vmhost").path,
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    private func reply(_ send: (@escaping ControlClientV2.Completion) -> Void) -> Result<ControlV2Reply, ControlClientError>? {
        let semaphore = DispatchSemaphore(value: 0)
        var result: Result<ControlV2Reply, ControlClientError>?
        send { reply in
            result = reply
            semaphore.signal()
        }
        return semaphore.wait(timeout: .now() + 30) == .success ? result : nil
    }

    private func spin(until condition: () -> Bool, seconds: TimeInterval = 15) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        return condition()
    }

    func testFramesAndInputReachTheWindowSideAndTheEndOfTheMachineEndsTheReader() throws {
        guard let vmhost = vmhostPath() else { throw XCTSkip("od-vmhost is not built next to the tests (run `swift build` first)") }
        let presenter: MetalPresenter
        do { presenter = try MetalPresenter() } catch { throw XCTSkip("no Metal device: \(error)") }
        let bundle = FileManager.default.temporaryDirectory.appendingPathComponent("od-window-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: bundle) }

        let queue = DispatchQueue(label: "window-test-events")
        let ready = DispatchSemaphore(value: 0)
        let client = try ControlClientV2.launch(
            ControlClientLaunch(vmhostPath: vmhost, bundlePath: bundle.path, displayChannel: true, inputChannel: true),
            deliveryQueue: queue
        ) { event in if event == .ready { ready.signal() } }
        XCTAssertEqual(ready.wait(timeout: .now() + 30), .success)
        guard let displayFD = client.takeDisplaySocket(), let inputFD = client.takeInputSocket() else { return XCTFail("no sockets") }

        let channel = try InputChannelUI(fd: inputFD)
        let frames = MachineFrameSession(fd: displayFD, presenter: presenter)
        var states: [FrameReaderState] = []
        var shownFrames: [ShownFrame] = []
        frames.onUpdate = { state, shown in
            states.append(state)
            if let shown { shownFrames.append(shown) }
        }
        // The window opens on `running`: the reader and the channel start only after START, as in `MachineWindowCenter`.
        XCTAssertNotNil(reply { client.configure(BundleConfig(name: "m").controlConfigure(), completion: $0) })
        guard case .success(.ack(let started))? = reply({ client.start(completion: $0) }) else { return XCTFail("START failed") }
        XCTAssertEqual(started.state, ControlV2State.running.rawValue)
        channel.start()
        frames.start()

        XCTAssertTrue(spin { states.last == .showing }, "no frame: \(states)")
        XCTAssertNotNil(shownFrames.last, "a shown frame is published for the absolute position of the mouse")
        XCTAssertNotNil(presenter.shownSize, "the frame is in the presenter")
        XCTAssertTrue(spin { channel.state == .ready }, "the input channel did not reach READY: \(channel.state)")
        XCTAssertEqual(channel.guestDelivery, false, "GUEST_DELIVERY = 0")

        // The model decides the text from these real states.
        XCTAssertNil(MachineWindowModel.overlay(run: .running, frame: states.last ?? .connecting))
        XCTAssertEqual(MachineWindowModel.overlay(run: .paused, frame: states.last ?? .connecting)?.text, "Пауза")

        guard case .success? = reply({ client.stop(completion: $0) }) else { return XCTFail("STOP failed") }
        XCTAssertTrue(spin { states.last?.isTerminal == true }, "the reader did not end with the machine: \(states)")
        XCTAssertEqual(MachineWindowModel.overlay(run: .stopped, frame: states.last ?? .connecting)?.text.hasPrefix("Остановлено"), true)
        XCTAssertNil(presenter.shownSize, "no frame stays in the presenter after the end")
        channel.shutdown()
        frames.stop()
    }
}
