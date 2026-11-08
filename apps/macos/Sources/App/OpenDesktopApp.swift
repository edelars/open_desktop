import AppKit
import SwiftUI

/// `OpenDesktop.app` (S11-05b window, S11-06b library). The application executes no guest code and does not link the engine: it lists
/// machines, spawns VMHost per machine through `ControlClientV2` and shows the state that the events and the exit status say.
@main
struct OpenDesktopApp: App {
    @StateObject private var model: LibraryModel
    /// The machine windows (S11-10): opens one on `running` and is the target of the menus "Вид" and "Машина".
    @StateObject private var windows: MachineWindowCenter
    /// Sleep and wake, displays, free space, memory pressure (S11-12).
    @StateObject private var conditions: HostConditions
    /// Quit with running machines (S11-11): the question, `STOP`, the wait.
    @NSApplicationDelegateAdaptor(AppLifecycle.self) private var lifecycle

    init() {
        let options: AppOptions
        do {
            options = try AppOptions(arguments: Array(CommandLine.arguments.dropFirst()))
        } catch {
            FileHandle.standardError.write(Data("OpenDesktop: \(error)\n".utf8))
            exit(2)
        }
        let vmhost = VMHostLocation.locate(options: options, bundleURL: Bundle.main.bundleURL)
        let center = MachineWindowCenter()
        let library = LibraryModel(
            catalog: .standard(), vmhost: vmhost, windowHost: center,
            logRevealer: { NSWorkspace.shared.activateFileViewerSelecting([$0]) })
        AppLifecycle.machines = { [weak library] in library?.machines ?? [] }
        library.load()
        if let path = options.openBundle {
            // `--open <bundle>` (smoke S11-14): list the bundle and select it.
            // A refusal is in the banner of the window and in stderr (a smoke run has no one to read the window).
            if !library.open(bundle: URL(fileURLWithPath: path)) {
                let reason = [library.banner, library.bannerDetails].compactMap { $0 }.joined(separator: " ")
                FileHandle.standardError.write(Data("OpenDesktop: --open: \(reason)\n".utf8))
            }
        }
        let host = HostConditions(machines: { [weak library] in library?.machines ?? [] }, store: UserDefaultsStore())
        host.onScreenParametersChange = { [weak center] in center?.screenParametersChanged() }
        host.onMemoryPressure = { [weak library] level in library?.setMemoryPressure(level) }
        host.start()
        if let kind = options.simulateHostEvent {
            // TEST-ONLY (`--simulate-host-event`): one handler, once, after the library is up. No real sleep and no real memory pressure.
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { host.simulate(kind) }
        }
        _model = StateObject(wrappedValue: library)
        _windows = StateObject(wrappedValue: center)
        _conditions = StateObject(wrappedValue: host)
    }

    var body: some Scene {
        // One window: a second library window would show the same machines twice.
        Window("Open Desktop", id: "library") {
            LibraryView(model: model)
        }
        .commands {
            MachineCommands(model: model, windows: windows, conditions: conditions)
            DisplayViewCommands(windows: windows)
        }
    }
}
