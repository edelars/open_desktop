import SwiftUI

/// `OpenDesktop.app` (S11-05b window, S11-06b library). The application executes no guest code and does not link the engine: it lists
/// machines, spawns VMHost per machine through `ControlClientV2` and shows the state that the events and the exit status say.
@main
struct OpenDesktopApp: App {
    @StateObject private var model: LibraryModel

    init() {
        let options: AppOptions
        do {
            options = try AppOptions(arguments: Array(CommandLine.arguments.dropFirst()))
        } catch {
            FileHandle.standardError.write(Data("OpenDesktop: \(error)\n".utf8))
            exit(2)
        }
        let vmhost = VMHostLocation.locate(options: options, bundleURL: Bundle.main.bundleURL)
        let library = LibraryModel(catalog: .standard(), vmhost: vmhost)
        library.load()
        if let path = options.openBundle {
            // `--open <bundle>` (smoke S11-14): list the bundle and select it.
            // A refusal is in the banner of the window and in stderr (a smoke run has no one to read the window).
            if !library.open(bundle: URL(fileURLWithPath: path)) {
                let reason = [library.banner, library.bannerDetails].compactMap { $0 }.joined(separator: " ")
                FileHandle.standardError.write(Data("OpenDesktop: --open: \(reason)\n".utf8))
            }
        }
        _model = StateObject(wrappedValue: library)
    }

    var body: some Scene {
        // One window: a second library window would show the same machines twice.
        Window("Open Desktop", id: "library") {
            LibraryView(model: model)
        }
        .commands { MachineCommands(model: model) }
    }
}
