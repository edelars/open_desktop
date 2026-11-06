import AppKit
import Foundation
import ODDisplay
import ODShared

// The machine window inside the application (S11-10). On `running` the application opens a window for the machine: a reader of
// frame transport v3 on `--display-fd 5` (ODShared `FrameReader`, own thread), the input channel on `--input-fd 4` (`InputChannelUI`),
// and `DisplayWindow` with `MetalPresenter` from the target ODDisplay (the same code as the smoke viewer, which is not copied).
//
// The order of `running` (control, descriptor 3) and the first `ANNOUNCE` (frames, descriptor 5) is not defined by CONTROL_PROTOCOL_V2.md
// (section 13, item 11). This file does not depend on it: the window opens on `running`, the reader sends its `HELLO` then, and until
// the `ANNOUNCE` arrives (or the 10 s limit ends) the window says "Нет сигнала". VMHost reads the `HELLO` only after `START`.
//
// One screen. A machine with two screens (the test profile) shows screen 0; the second screen is the remainder of S10-15.
// The state of the machine comes from `MachineController` (the events of VMHost); the window never decides it.
// Everything except the reader thread runs on the main thread.

// MARK: reader thread

/// Owns the `FrameReader` of one run and its thread; uploads the frames into the presenter and tells the main thread what changed.
/// The reader belongs to the thread until the thread ends; it tears the reader down itself (munmap and close of the socket).
final class MachineFrameSession {
    private let reader: FrameReader
    private let presenter: MetalPresenter
    private let lock = NSLock()
    private var stopFlag = false
    /// Reader state of screen 0 and the shown frame, on the main thread, after every change.
    var onUpdate: ((FrameReaderState, ShownFrame?) -> Void)?

    static let announceTimeoutMs: UInt64 = 10_000

    /// Takes ownership of `fd`.
    init(fd: Int32, presenter: MetalPresenter) {
        reader = FrameReader(fd: fd)
        self.presenter = presenter
    }

    func start() {
        reader.start(announceTimeoutMs: MachineFrameSession.announceTimeoutMs)
        Thread.detachNewThread { [self] in loop() }
    }

    /// The thread ends within one pump (100 ms) and closes the socket: VMHost sees EOF on the frame link.
    func stop() {
        lock.lock()
        stopFlag = true
        lock.unlock()
    }

    private var isStopRequested: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopFlag
    }

    private func loop() {
        var published: FrameReaderState?
        var publishedShown: ShownFrame?
        // Same commit/discard/clear rules as the viewer's step, for screen 0.
        while !isStopRequested {
            let before = (reader.primary?.acceptedCount ?? 0, reader.primary?.cursorAcceptedCount ?? 0)
            var uploaded = false
            var cursorUploaded = false
            let presenter = presenter
            let changed = reader.pump(
                timeoutMs: 100,
                copy: { screen, pixels, frame in
                    guard screen == 0 else { return } // the other screens are the remainder of S10-15
                    uploaded = true
                    presenter.upload(pixels: pixels, frame: frame)
                },
                cursorCopy: { pixels, cursor in
                    cursorUploaded = true
                    return presenter.uploadCursor(pixels: pixels, cursor: cursor)
                })
            if let primary = reader.primary {
                if primary.acceptedCount != before.0 {
                    presenter.commit()
                } else if uploaded {
                    presenter.discard()
                }
                if primary.state != .showing { presenter.clear() }
                if primary.cursorAcceptedCount != before.1 {
                    presenter.commitCursor(primary.cursor)
                } else if cursorUploaded {
                    presenter.discardCursor()
                }
            }
            if reader.state.isTerminal {
                presenter.clear()
                presenter.commitCursor(nil)
            }
            let state = reader.state
            var shown: ShownFrame?
            if let primary = reader.primary, primary.state == .showing, let frame = primary.lastFrame {
                shown = ShownFrame(width: frame.width, height: frame.height, modeChangeCount: frame.modeChangeCount)
            }
            if changed || published != state || shown != publishedShown {
                published = state
                publishedShown = shown
                DispatchQueue.main.async { [weak self] in self?.onUpdate?(state, shown) }
            }
            if state.isTerminal { break }
        }
        reader.teardown()
    }
}

// MARK: window

final class MachineWindowController: NSObject, NSWindowDelegate {
    let machine: MachineController
    let display: DisplayWindow
    private weak var center: MachineWindowCenter?
    private let frames: MachineFrameSession
    private let input: InputChannelUI?
    private let profileText: String?
    private var closed = false

    private(set) var runState: MachineRunState
    private(set) var frameState: FrameReaderState = .connecting
    private(set) var capture = InputCaptureModel()
    /// The run of this window is over (`stopped`): the window stays and says so; a new run opens a new window.
    private(set) var isEnded = false

    var scaling: WindowScaling { WindowScaling(display.mode) }
    var isCaptured: Bool { capture.isCaptured }

    /// Takes ownership of both descriptors (also when it throws). `frame` is the frame of the window of an earlier run, if any.
    init(
        machine: MachineController, displayFD: Int32, inputFD: Int32?, center: MachineWindowCenter, frame: NSRect?
    ) throws {
        self.machine = machine
        self.center = center
        runState = machine.runState
        profileText = machine.config.map { MachinePresentation.profileText($0.machineProfile) }
        let presenter: MetalPresenter
        do {
            presenter = try MetalPresenter()
        } catch {
            Darwin.close(displayFD)
            if let inputFD { Darwin.close(inputFD) }
            throw error
        }
        // The channel closes the descriptor itself when it cannot be configured; the window then has no input.
        let channel = inputFD.flatMap { try? InputChannelUI(fd: $0) }
        input = channel
        display = DisplayWindow(
            presenter: presenter, title: machine.displayName, contentSize: NSSize(width: 800, height: 600), mode: .fit, input: channel)
        frames = MachineFrameSession(fd: displayFD, presenter: presenter)
        super.init()
        let window = display.window
        window.delegate = self
        window.collectionBehavior.insert(.fullScreenPrimary)
        if let frame { window.setFrame(frame, display: false) }
        let profile = profileText
        display.inputTextFormatter = { MachineWindowModel.subtitle(input: $0, profile: profile) }
        display.onInputChange = { [weak self] in self?.inputChanged() }
        display.keyChordHandler = { [weak self] keyCode, flags in
            guard let self else { return false }
            let chord = CaptureChord.toggle.matches(
                keyCode: keyCode, control: flags.contains(.control), option: flags.contains(.option),
                command: flags.contains(.command), shift: flags.contains(.shift))
            if chord { self.toggleCapture() }
            return chord
        }
        // The subtitle also without a channel.
        if channel == nil { window.subtitle = MachineWindowModel.subtitle(input: nil, profile: profile) }
        frames.onUpdate = { [weak self] state, shown in
            guard let self else { return }
            // The frame is drawn on every update; menus and the title are redone only when the reader state really changed.
            let stateChanged = self.frameState != state
            self.frameState = state
            self.display.update(state: state, detail: nil, shown: shown)
            if stateChanged { self.apply() }
        }
        apply()
        display.show()
        channel?.start()
        frames.start()
    }

    // MARK: state

    /// A state of the machine from the controller (events of VMHost).
    func machineStateChanged(_ state: MachineRunState) {
        guard !closed else { return }
        runState = state
        if state == .stopped || state == .failed { endRun() }
        apply()
    }

    /// Redraws the window for the current machine state, reader state and capture.
    private func apply() {
        capture.enforce(canCapture: canCapture)
        display.setOverlay(MachineWindowModel.overlay(run: runState, frame: frameState, endReason: machine.crashText))
        applyCapture()
    }

    private var canCapture: Bool {
        InputCaptureModel.canCapture(run: runState, windowIsKey: display.window.isKeyWindow, input: display.inputPhase)
    }

    private func applyCapture() {
        display.setInputCaptured(capture.isCaptured)
        display.hidesHostCursor = capture.isCaptured
        display.window.title = MachineWindowModel.title(name: machine.displayName, run: runState, captured: capture.isCaptured)
        center?.changed()
    }

    private func inputChanged() {
        guard !closed else { return }
        capture.enforce(canCapture: canCapture)
        applyCapture()
    }

    /// The process is gone: the window keeps its last text, the reader and the input channel are closed.
    private func endRun() {
        guard !isEnded else { return }
        isEnded = true
        capture.release()
        frames.stop()
        display.tearDownInput()
    }

    // MARK: menu actions

    func toggleCapture() {
        capture.toggle(canCapture: canCapture)
        applyCapture()
    }

    func setScaling(_ scaling: WindowScaling) {
        display.mode = scaling.presentMode
        center?.changed()
    }

    func toggleFullScreen() { display.window.toggleFullScreen(nil) }

    /// S11-12: the displays changed. A window on a display that is gone moves to the primary one (`WindowPlacement`); a window in full screen
    /// is the system's to place. Then the drawable and the viewport are rebuilt by the code of S10-11. The number of the guest's screens
    /// does not change by this.
    func screenParametersChanged() {
        guard !closed else { return }
        let window = display.window
        let screens = NSScreen.screens
        if !window.styleMask.contains(.fullScreen), let primary = screens.first?.visibleFrame {
            let target = WindowPlacement.adjusted(window.frame, visibleFrames: screens.map(\.visibleFrame), primary: primary)
            if target != window.frame { window.setFrame(target, display: true) }
        }
        display.screenParametersChanged()
    }

    /// Closes without asking (a new run replaces this window, or the application ends).
    func closeWithoutQuestion() {
        endRun()
        display.window.delegate = nil
        willClose()
        display.window.close()
    }

    // MARK: NSWindowDelegate

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        switch MachineWindowModel.closePolicy(run: runState, hasSession: machine.hasSession, canStop: machine.allows(.stop)) {
        case .closeNow:
            return true
        case .confirmStop:
            let alert = NSAlert()
            alert.messageText = "Закрыть окно и остановить машину «\(machine.displayName)»?"
            alert.informativeText = "Окно машины нельзя открыть заново до следующего запуска: кадры передаются один раз за запуск. "
                + "Машина получит команду остановки."
            alert.alertStyle = .warning
            alert.addButton(withTitle: "Остановить и закрыть")
            alert.addButton(withTitle: "Отмена")
            guard alert.runModal() == .alertFirstButtonReturn else { return false }
            // The state may have changed while the question was open: the stop is repeated only if it is still allowed.
            guard machine.allows(.stop) else { return false }
            machine.stop()
            return true
        case .refuse:
            let alert = NSAlert()
            alert.messageText = "Машина занята другой операцией"
            alert.informativeText = "Остановить её сейчас нельзя. Повторите закрытие через секунду."
            alert.alertStyle = .informational
            alert.addButton(withTitle: "OK")
            alert.runModal()
            return false
        }
    }

    func windowWillClose(_ notification: Notification) { willClose() }

    private func willClose() {
        guard !closed else { return }
        closed = true
        capture.release()
        display.setInputCaptured(false)
        display.hidesHostCursor = false
        frames.stop()
        display.tearDownInput()
        center?.windowClosed(self)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        center?.windowBecameKey(self)
        apply()
    }

    func windowDidResignKey(_ notification: Notification) {
        center?.windowResignedKey(self)
        // Not key any more: the capture ends (the cursor is visible again, the guest gets FOCUS LOST).
        capture.enforce(canCapture: false)
        applyCapture()
    }

    func windowDidMiniaturize(_ notification: Notification) {
        capture.enforce(canCapture: false)
        applyCapture()
    }
}

// MARK: center

/// All machine windows of the application, the target of the menus "Вид" and "Машина". The controller of a machine tells it every state.
final class MachineWindowCenter: NSObject, ObservableObject, MachineWindowHost {
    /// The machine window that is key, if the key window is one.
    @Published private(set) var active: MachineWindowController?
    private var windows: [String: MachineWindowController] = [:]

    /// A machine window is a window that exists and has not ended its run.
    func window(for machine: MachineController) -> MachineWindowController? { windows[machine.id] }

    func machine(_ machine: MachineController, didChange state: MachineRunState) {
        let existing = windows[machine.id]
        if let existing, !existing.isEnded {
            existing.machineStateChanged(state)
            return
        }
        guard state == .running else {
            existing?.machineStateChanged(state)
            return
        }
        open(for: machine, replacing: existing)
    }

    private func open(for machine: MachineController, replacing old: MachineWindowController?) {
        let channels = machine.takeDisplayChannels()
        guard let displayFD = channels.display else {
            if let input = channels.input { Darwin.close(input) }
            return
        }
        do {
            let created = try MachineWindowController(
                machine: machine, displayFD: displayFD, inputFD: channels.input, center: self, frame: old?.display.window.frame)
            windows[machine.id] = created
            old?.closeWithoutQuestion()
            created.machineStateChanged(machine.runState)
        } catch {
            FileHandle.standardError.write(Data("OpenDesktop: окно машины не открыто: \(error)\n".utf8))
        }
    }

    func windowClosed(_ controller: MachineWindowController) {
        if windows[controller.machine.id] === controller { windows[controller.machine.id] = nil }
        if active === controller { active = nil }
        objectWillChange.send()
    }

    func windowBecameKey(_ controller: MachineWindowController) {
        if active !== controller { active = controller }
    }

    func windowResignedKey(_ controller: MachineWindowController) {
        if active === controller { active = nil }
    }

    /// A window changed what the menus show (capture, scaling).
    func changed() { objectWillChange.send() }

    /// S11-12: the displays changed; every machine window rebuilds itself.
    func screenParametersChanged() { windows.values.forEach { $0.screenParametersChanged() } }

    // MARK: menu state and actions (the active window)

    var scaling: WindowScaling? { active?.scaling }
    var canToggleCapture: Bool {
        guard let active else { return false }
        return active.isCaptured || InputCaptureModel.canCapture(
            run: active.runState, windowIsKey: true, input: active.display.inputPhase)
    }
    var captureMenuTitle: String { MachineWindowModel.captureMenuTitle(captured: active?.isCaptured ?? false) }

    func setScaling(_ scaling: WindowScaling) { active?.setScaling(scaling) }
    func toggleFullScreen() { active?.toggleFullScreen() }
    func toggleCapture() { active?.toggleCapture() }
}
