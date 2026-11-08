import AppKit
import Combine
import Foundation
import ODShared

// Conditions of the Mac that touch the machines (S11-12): sleep and wake, a change of the displays, free space of the volume of the bundle
// and memory pressure. The application executes no guest code and writes no disk of a running machine: it only tells the VMHost to pause
// or to resume (`PAUSE`/`RESUME` with a `reason`, CONTROL_PROTOCOL_V2.md 6.8) and shows what it knows.
//
// Sources (Context7, Apple documentation; checked 2026-11-06):
//  - `NSWorkspace.willSleepNotification` is posted before the device goes to sleep, `NSWorkspace.didWakeNotification` when it wakes. Both
//    are delivered by `NSWorkspace.shared.notificationCenter`, not by the default center (a different center receives nothing). The
//    documentation does not give a time limit for the handler and does not promise that an asynchronous request finishes before the
//    sleep: source for that not found, so nothing here claims it.
//  - `NSApplication.didChangeScreenParametersNotification`: the configuration of the displays changed; posted on the main actor.
//  - `URLResourceKey.volumeAvailableCapacityForImportantUsageKey` (`URLResourceValues.volumeAvailableCapacityForImportantUsage`): the
//    free bytes of the volume for important resources (used by `CreateMachineModel.volumeAvailableBytes(at:)`).
//  - `DispatchSource.makeMemoryPressureSource(eventMask:queue:)` with `DispatchSource.MemoryPressureEvent` `.normal`, `.warning`,
//    `.critical`; the source is inactive until `activate()`.
//
// The rules are pure values and functions (thresholds, window placement, the memory banner, the settings store) and the handlers take the
// events as plain calls, so the tests inject sleep, wake, screens, space and memory without any real event.

// MARK: events for `--simulate-host-event`

/// The value of the TEST-ONLY argument `--simulate-host-event` (S11-12). It makes the application run one handler as if the event happened.
/// It does not put the Mac to sleep and does not create memory pressure; it exists for the smoke S11-61.
enum HostEventKind: String, CaseIterable, Equatable {
    case sleep
    case wake
    case screens
    case lowSpace = "low-space"
    case memory
}

// MARK: free space

enum DiskSpaceLevel: Equatable {
    case normal
    case warning
    case critical
}

/// Thresholds for the free space of the volume of the bundle. HYPOTHESIS (docs/testing/DESKTOP_APP.md, section 6): there is no measurement
/// behind 5 GiB and 1 GiB; they are the first guess and have to be checked (S11-14, S11-61) before anyone relies on them.
struct DiskSpaceThresholds: Equatable {
    var warningBytes: UInt64
    var hardBytes: UInt64

    static let hypothesis = DiskSpaceThresholds(warningBytes: 5 << 30, hardBytes: 1 << 30)

    /// Strictly below a threshold: exactly the threshold is still the better level.
    func level(availableBytes: UInt64) -> DiskSpaceLevel {
        if availableBytes < hardBytes { return .critical }
        if availableBytes < warningBytes { return .warning }
        return .normal
    }
}

// MARK: memory

enum MemoryPressureLevel: Equatable {
    case normal
    case warning
    case critical

    /// The worst level in the event mask of the dispatch source; `nil` for an empty mask.
    init?(event: DispatchSource.MemoryPressureEvent) {
        if event.contains(.critical) {
            self = .critical
        } else if event.contains(.warning) {
            self = .warning
        } else if event.contains(.normal) {
            self = .normal
        } else {
            return nil
        }
    }
}

/// The banner of the library for the memory pressure: the explanation and the offer to stop machines. The application stops nothing itself.
/// Shown only when a machine process exists (without one the offer means nothing) and until the user hides it for this level.
enum MemoryBanner {
    static func presentation(level: MemoryPressureLevel, hasLiveMachines: Bool, dismissed: Bool) -> ErrorPresentation? {
        guard hasLiveMachines, !dismissed else { return nil }
        switch level {
        case .normal: return nil
        case .warning: return ErrorPresentation.host(.memoryPressureWarning)
        case .critical: return ErrorPresentation.host(.memoryPressureCritical)
        }
    }
}

// MARK: windows on displays

/// Where a machine window goes when the displays change. A pure function of the frames (global coordinates of AppKit).
enum WindowPlacement {
    /// A window counts as reachable when at least this much of it (or all of it, when it is smaller) is inside one visible area.
    static let minimumVisible = NSSize(width: 100, height: 60)

    static func isReachable(_ frame: NSRect, in area: NSRect) -> Bool {
        let overlap = frame.intersection(area)
        guard !overlap.isNull, !overlap.isEmpty else { return false }
        return overlap.width >= min(minimumVisible.width, frame.width) && overlap.height >= min(minimumVisible.height, frame.height)
    }

    /// The frame is kept when it is reachable on any display. Otherwise (its display is gone) the window moves to the primary display: the
    /// size is cut to the visible area of it and the window is centred there. `primary` is the visible frame of the primary display.
    static func adjusted(_ frame: NSRect, visibleFrames: [NSRect], primary: NSRect) -> NSRect {
        if visibleFrames.contains(where: { isReachable(frame, in: $0) }) { return frame }
        let size = NSSize(width: min(frame.width, primary.width), height: min(frame.height, primary.height))
        return NSRect(
            x: primary.midX - size.width / 2, y: primary.midY - size.height / 2, width: size.width, height: size.height)
    }
}

// MARK: settings

/// The place of the one setting. `UserDefaults` is the real one; the tests pass a dictionary, so no preferences file is written.
protocol SettingsStore: AnyObject {
    func bool(forKey key: String) -> Bool?
    func set(_ value: Bool, forKey key: String)
}

final class UserDefaultsStore: SettingsStore {
    private let defaults: UserDefaults
    init(_ defaults: UserDefaults = .standard) { self.defaults = defaults }
    func bool(forKey key: String) -> Bool? { defaults.object(forKey: key) as? Bool }
    func set(_ value: Bool, forKey key: String) { defaults.set(value, forKey: key) }
}

final class MemorySettingsStore: SettingsStore {
    private var values: [String: Bool] = [:]
    init() {}
    func bool(forKey key: String) -> Bool? { values[key] }
    func set(_ value: Bool, forKey key: String) { values[key] = value }
}

// MARK: the observer of the host

final class HostConditions: ObservableObject {
    static let pauseOnSleepKey = "pauseMachinesOnSleep"
    /// The check of free space of the running machines. The only timer of the application; VMHost is never polled (STAGE_11.md 4.1).
    static let diskCheckInterval: TimeInterval = 30

    /// "Приостанавливать машины при сне": on by default (an unset key is on).
    @Published var pauseOnSleep: Bool {
        didSet { if pauseOnSleep != oldValue { store.set(pauseOnSleep, forKey: Self.pauseOnSleepKey) } }
    }

    /// The displays changed: the machine windows rebuild their drawable and viewport and move off a display that is gone.
    var onScreenParametersChange: (() -> Void)?
    /// A level of the memory pressure arrived: the library shows or clears its banner.
    var onMemoryPressure: ((MemoryPressureLevel) -> Void)?

    private let machines: () -> [MachineController]
    private let store: SettingsStore
    private let workspaceCenter: NotificationCenter
    private let appCenter: NotificationCenter
    private var observers: [NSObjectProtocol] = []
    private var timer: DispatchSourceTimer?
    private var memorySource: DispatchSourceMemoryPressure?

    init(
        machines: @escaping () -> [MachineController], store: SettingsStore,
        workspaceCenter: NotificationCenter = NSWorkspace.shared.notificationCenter, appCenter: NotificationCenter = .default
    ) {
        self.machines = machines
        self.store = store
        self.workspaceCenter = workspaceCenter
        self.appCenter = appCenter
        pauseOnSleep = store.bool(forKey: Self.pauseOnSleepKey) ?? true
    }

    deinit { stop() }

    // MARK: real sources

    /// Subscribes to the system: the notifications, the timer of 30 s and the memory pressure source. Once; the tests call the handlers.
    func start() {
        guard observers.isEmpty, timer == nil else { return }
        subscribeToNotifications()
        startDiskTimer()
        startMemorySource()
    }

    func stop() {
        observers.forEach { observer in
            // The workspace center and the application center are different objects: the token is removed from the one that issued it.
            workspaceCenter.removeObserver(observer)
            appCenter.removeObserver(observer)
        }
        observers = []
        timer?.cancel()
        timer = nil
        memorySource?.cancel()
        memorySource = nil
    }

    /// The two centers of the notifications: the sleep and the wake come from `NSWorkspace.shared.notificationCenter`, the displays from
    /// the application's own center.
    func subscribeToNotifications() {
        // The application ends: the timer and the memory source are cancelled here (a delegate's `deinit` does not run at quit).
        observers.append(appCenter.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { [weak self] _ in
            self?.stop()
        })
        observers.append(workspaceCenter.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
            self?.handleWillSleep()
        })
        observers.append(workspaceCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            self?.handleDidWake()
        })
        observers.append(appCenter.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.handleScreenParametersChanged()
        })
    }

    private func startDiskTimer() {
        let source = DispatchSource.makeTimerSource(queue: .main)
        source.schedule(deadline: .now() + Self.diskCheckInterval, repeating: Self.diskCheckInterval)
        source.setEventHandler { [weak self] in self?.handleDiskCheck() }
        source.activate()
        timer = source
    }

    private func startMemorySource() {
        let source = DispatchSource.makeMemoryPressureSource(eventMask: [.normal, .warning, .critical], queue: .main)
        source.setEventHandler { [weak self, weak source] in
            guard let self, let source, let level = MemoryPressureLevel(event: source.data) else { return }
            self.handleMemoryPressure(level)
        }
        source.activate()
        memorySource = source
    }

    // MARK: handlers (the tests call these)

    /// Before the Mac sleeps: every running machine gets `PAUSE reason=system_sleep`, unless the user turned that off.
    func handleWillSleep() {
        guard pauseOnSleep else { return }
        machines().forEach { $0.prepareForSystemSleep() }
    }

    /// After the wake: only a machine that the application paused for the sleep is resumed. The wake does not look at the setting: a machine
    /// that was paused before the user turned the setting off still has to continue.
    func handleDidWake() {
        machines().forEach { $0.resumeAfterSystemWake() }
    }

    func handleScreenParametersChanged() {
        onScreenParametersChange?()
    }

    /// The tick of 30 s: every running machine looks at the free space of its volume.
    func handleDiskCheck() {
        machines().forEach { $0.checkDiskSpace() }
    }

    func handleMemoryPressure(_ level: MemoryPressureLevel) {
        onMemoryPressure?(level)
    }

    // MARK: test-only simulation

    /// TEST-ONLY (`--simulate-host-event`): runs the handler of the event as if it happened. The Mac does not sleep and the memory is not
    /// short. `low-space` makes every machine see 512 MiB free (below the hard threshold) from now on.
    func simulate(_ kind: HostEventKind) {
        switch kind {
        case .sleep: handleWillSleep()
        case .wake: handleDidWake()
        case .screens: handleScreenParametersChanged()
        case .lowSpace:
            machines().forEach { $0.availableSpace = { _ in 512 << 20 } }
            handleDiskCheck()
        case .memory: handleMemoryPressure(.critical)
        }
    }
}
