import Foundation
import ODDisplay
import ODShared

// What the machine window says and does, as pure functions and small value types (S11-10). No AppKit and no window here: the tests
// (MachineWindowTests.swift) run without a screen. `MachineWindowController.swift` applies these decisions to a real window.
//
// The rules (STAGE_11.md S11-10, INPUT_TRANSPORT_V1.md section 11, open decisions 2 and 3):
// - "no signal", "pause", "stopped", "incompatible version" are text over a plain colour; the window never shows a frozen last frame
//   under them (the presenter keeps its texture, the window does not draw it).
// - Input is collected only in the explicit mode "captured". Outside it every key (Cmd-Q, Cmd-W, ...) belongs to the application,
//   and the host cursor is visible. In "captured" every key belongs to the guest, except the release chord.
// - Pause, stop, loss of the key window, loss of the input channel and closing the window release the capture.

// MARK: scaling (menu "Вид")

enum WindowScaling: CaseIterable, Equatable {
    /// "По размеру окна": the frame is scaled to the window with its aspect ratio kept.
    case fitWindow
    /// "Пиксель в пиксель": one guest pixel is one physical pixel.
    case pixelExact

    var presentMode: PresentMode {
        switch self {
        case .fitWindow: return .fit
        case .pixelExact: return .pixelExact
        }
    }

    init(_ mode: PresentMode) {
        self = mode == .pixelExact ? .pixelExact : .fitWindow
    }

    var menuTitle: String {
        switch self {
        case .fitWindow: return "По размеру окна"
        case .pixelExact: return "Пиксель в пиксель"
        }
    }
}

// MARK: capture of input

/// The release chord of the captured mode and the capture itself: Control + Option + Command + G. The key is the physical key `G`
/// (`kVK_ANSI_G` = 5), so the layout (for example the Russian one) does not matter. Shift must not be down.
struct CaptureChord: Equatable {
    var keyCode: UInt16

    static let toggle = CaptureChord(keyCode: 5)
    /// The text for titles and menus.
    static let symbols = "⌃⌥⌘G"

    func matches(keyCode: UInt16, control: Bool, option: Bool, command: Bool, shift: Bool) -> Bool {
        keyCode == self.keyCode && control && option && command && !shift
    }
}

/// "Captured" or not. A request to capture is refused unless the machine runs, the window is the key window and the input channel is
/// ready (`ACCEPT` received); the guest gets nothing in any other case. The refusal changes nothing.
struct InputCaptureModel: Equatable {
    enum Mode: Equatable {
        case released
        case captured
    }

    private(set) var mode = Mode.released

    var isCaptured: Bool { mode == .captured }

    static func canCapture(run: MachineRunState, windowIsKey: Bool, input: InputPhase?) -> Bool {
        guard run == .running, windowIsKey, case .ready = input else { return false }
        return true
    }

    /// The chord or the menu item: captured -> released always; released -> captured only when `canCapture`. Returns the mode.
    @discardableResult
    mutating func toggle(canCapture: Bool) -> Mode {
        switch mode {
        case .captured: mode = .released
        case .released: if canCapture { mode = .captured }
        }
        return mode
    }

    /// A change of the conditions (pause, stop, resigned key window, closed input channel): captured stays only while it can.
    /// Returns true when the mode changed.
    @discardableResult
    mutating func enforce(canCapture: Bool) -> Bool {
        guard mode == .captured, !canCapture else { return false }
        mode = .released
        return true
    }

    /// Unconditional release (the window is closing).
    mutating func release() { mode = .released }
}

// MARK: texts and overlays

enum MachineWindowModel {
    private static let grey = (red: 0.12, green: 0.12, blue: 0.14)
    private static let blue = (red: 0.08, green: 0.16, blue: 0.40)
    private static let red = (red: 0.40, green: 0.08, blue: 0.08)
    private static let amber = (red: 0.45, green: 0.28, blue: 0.04)
    private static let dark = (red: 0.10, green: 0.10, blue: 0.10)

    private static func overlay(_ text: String, _ color: (red: Double, green: Double, blue: Double)) -> DisplayOverlay {
        DisplayOverlay(text: text, red: color.red, green: color.green, blue: color.blue)
    }

    /// The text over the window, or nil when the frame itself is shown (running, reader state `showing`). Order of the rules:
    /// a machine that is not there ("Остановлено") first, then an incompatible version, then pause and shutdown, then the reader state.
    /// `endReason` (S11-11) is "Процесс машины завершился: <причина>" when the process crashed (a signal, or a status that is not 0): it is
    /// the second line of "Остановлено", in the colour of a failure.
    static func overlay(run: MachineRunState, frame: FrameReaderState, detail: String? = nil, endReason: String? = nil) -> DisplayOverlay? {
        switch run {
        case .stopped, .failed:
            var text = run == .failed ? "Остановлено: сбой машины" : "Остановлено"
            if let endReason { text += "\n\(endReason)" }
            if case let .incompatible(reason) = frame { text += "\nНесовместимая версия: \(detail ?? reason)" }
            return overlay(text, run == .failed || endReason != nil ? red : dark)
        case .starting:
            return overlay("Запуск…", grey)
        case .shuttingDown, .paused, .running:
            break
        }
        if case let .incompatible(reason) = frame { return overlay("Несовместимая версия\n\(detail ?? reason)", amber) }
        if run == .paused { return overlay("Пауза", blue) }
        if run == .shuttingDown { return overlay("Останавливается…", dark) }
        switch frame {
        case .showing: return nil
        case .connecting, .noSignal: return overlay("Нет сигнала", grey)
        case .modeChanging: return overlay("Смена режима экрана", blue)
        case .disabled: return overlay("Экран отключён гостем", dark)
        case let .disconnected(reason): return overlay("Нет сигнала: VMHost отключился\n\(detail ?? reason)", red)
        case .incompatible: return nil // handled above
        }
    }

    /// Title of the window: the name and the mode of input; the chord is named in both modes.
    static func title(name: String, run: MachineRunState, captured: Bool) -> String {
        guard run == .running else { return "\(name) — \(MachinePresentation.stateText(run).lowercased())" }
        return captured
            ? "\(name) — ввод захвачен (\(CaptureChord.symbols) — освободить)"
            : "\(name) — ввод не захвачен (\(CaptureChord.symbols) — захватить)"
    }

    /// Subtitle of the window: what the input channel does. While `GUEST_DELIVERY = 0` (the board has no input device yet) it says so.
    /// `profile` is the profile line of the machine ("... (test-only, без гостя)") and is appended.
    static func subtitle(input: InputPhase?, profile: String?) -> String {
        var parts: [String] = []
        switch input {
        case nil: break
        case .connecting: parts.append("ввод: соединение…")
        case .ready(guestDelivery: false): parts.append("ввод в гостевую систему пока не доставляется")
        case .ready(guestDelivery: true): parts.append("ввод доставляется в гостевую систему")
        case .closed: parts.append("ввод отключён")
        case .incompatible: parts.append("ввод отключён: несовместимая версия протокола")
        }
        if let profile { parts.append(profile) }
        return parts.joined(separator: " · ")
    }

    /// The menu item that toggles the capture.
    static func captureMenuTitle(captured: Bool) -> String {
        captured ? "Освободить ввод" : "Захватить ввод"
    }

    // MARK: closing

    enum ClosePolicy: Equatable {
        /// Nothing runs: the window closes without a question.
        case closeNow
        /// The machine runs or is paused: ask, and on a yes send `STOP` and close. The window cannot be opened again for this run
        /// (the frame socket is handed out once).
        case confirmStop
        /// The machine runs but `STOP` is not possible now (another request is in flight): do not close.
        case refuse
    }

    static func closePolicy(run: MachineRunState, hasSession: Bool, canStop: Bool) -> ClosePolicy {
        guard hasSession else { return .closeNow }
        switch run {
        case .running, .paused: return canStop ? .confirmStop : .refuse
        case .stopped, .starting, .shuttingDown, .failed: return .closeNow
        }
    }
}
