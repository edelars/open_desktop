import SwiftUI

/// Window of `OpenDesktop.app` (S11-05b). Intermediate fact, not the product: the window opens and says that the
/// machine library does not exist yet (S11-06). The application executes no guest code and does not link the engine.
struct LibraryPlaceholderView: View {
    let vmhost: VMHostLocation

    var body: some View {
        VStack(spacing: 12) {
            Text("Open Desktop").font(.largeTitle)
            Text("Библиотека машин — S11-06").font(.title3)
            Text(vmhostDescription)
                .font(.caption)
                .foregroundColor(.secondary)
                .textSelection(.enabled)
        }
        .padding(32)
        .frame(minWidth: 480, minHeight: 240)
    }

    private var vmhostDescription: String {
        let origin = vmhost.source == .argument ? "--vmhost" : "bundle"
        let state = vmhost.isExecutable ? "found" : "not found"
        return "VMHost (\(origin), \(state)): \(vmhost.url.path)"
    }
}

@main
struct OpenDesktopApp: App {
    private let vmhost: VMHostLocation

    init() {
        let options: AppOptions
        do {
            options = try AppOptions(arguments: Array(CommandLine.arguments.dropFirst()))
        } catch {
            FileHandle.standardError.write(Data("OpenDesktop: \(error)\n".utf8))
            exit(2)
        }
        vmhost = VMHostLocation.locate(options: options, bundleURL: Bundle.main.bundleURL)
    }

    var body: some Scene {
        WindowGroup("Open Desktop") {
            LibraryPlaceholderView(vmhost: vmhost)
        }
    }
}
