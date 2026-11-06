// swift-tools-version: 5.9
// Swift Package of the macOS products (S11-05a, docs/decisions/ADR-0006-desktop-app-shell.md). No external dependencies.
//
// Build from this directory: `swift build` / `swift build -c release` / `swift test`. `od-vmhost` links the Rust
// staticlib: build it first from the repository root (`cargo build -p od-host --offline` for debug, add `--release` for
// release). The path below is `target/<profile>/libod_host.a` of the repository root; nothing else links it.
import PackageDescription

// The directory of this manifest is `apps/`; the Cargo `target/` directory is in its parent.
let repositoryRoot = Context.packageDirectory + "/.."
let hostLibrary = { (profile: String) in repositoryRoot + "/target/" + profile + "/libod_host.a" }

let package = Package(
    name: "OpenDesktop",
    products: [
        .executable(name: "od-vmhost", targets: ["od-vmhost"]),
        .executable(name: "open-desktop-display-viewer", targets: ["open-desktop-display-viewer"]),
        .executable(name: "open-desktop-launcher-smoke", targets: ["open-desktop-launcher-smoke"]),
    ],
    targets: [
        // Atomic C shim of the frame transport (FRAME_TRANSPORT_V1.md 5.3). Only this header; no engine ABI.
        .target(name: "ODAtomics", path: "macos/Sources/ODAtomics", publicHeadersPath: "include"),
        // Only include/open_desktop/host_abi.h (ABI version and fragment length constants) for the launcher smoke.
        .target(name: "ODHostConstants", path: "macos/Sources/ODHostConstants", publicHeadersPath: "include"),
        // C ABI of the engine (include/open_desktop/*.h) and the VMHost helpers. Used by `od-vmhost` and its tests only;
        // it declares symbols, it does not link `libod_host.a`.
        .target(name: "ODHostABI", dependencies: ["ODAtomics"], path: "vmhost/Sources/ODHostABI", publicHeadersPath: "include"),
        // Code shared by the application, the CLI and VMHost. No FrameTransport, no libod_host.a.
        .target(name: "ODShared", dependencies: ["ODAtomics"], path: "macos/Sources/Shared"),
        // The only target that links the Rust engine.
        .executableTarget(
            name: "od-vmhost",
            dependencies: ["ODShared", "ODAtomics", "ODHostABI"],
            path: "vmhost/Sources",
            exclude: ["ODHostABI", ".gitkeep"],
            sources: [
                "InputChannel.swift", "FrameTransport.swift", "BundleLock.swift", "Lifecycle.swift", "ControlV2Host.swift", "VMHostMain.swift",
            ],
            linkerSettings: [
                .unsafeFlags([hostLibrary("debug")], .when(configuration: .debug)),
                .unsafeFlags([hostLibrary("release")], .when(configuration: .release)),
                .linkedLibrary("dl"), .linkedLibrary("pthread"), .linkedLibrary("m"),
                .linkedFramework("Security"), .linkedFramework("SystemConfiguration"),
                .linkedFramework("CoreFoundation"), .linkedFramework("CoreServices"), .linkedFramework("Foundation"),
            ]
        ),
        // Smoke programs of stage 10 (windows with Metal; spawn of VMHost). They link no Rust code.
        .executableTarget(
            name: "open-desktop-display-viewer",
            dependencies: ["ODShared"],
            path: "macos/Sources",
            exclude: ["Shared", "ODAtomics", "ODHostConstants", "launcher.swift", ".gitkeep"],
            sources: ["DisplayWindow.swift", "MetalPresenter.swift", "InputChannel.swift", "display_viewer_main.swift"],
            linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("Metal"), .linkedFramework("QuartzCore")]
        ),
        .executableTarget(
            name: "open-desktop-launcher-smoke",
            dependencies: ["ODShared", "ODHostConstants"],
            path: "macos/Sources",
            exclude: [
                "Shared", "ODAtomics", "ODHostConstants", "DisplayWindow.swift", "MetalPresenter.swift", "InputChannel.swift",
                "display_viewer_main.swift", ".gitkeep",
            ],
            sources: ["launcher.swift"]
        ),
        // Tests (XCTest). The former executables with `@main` are test cases now.
        .testTarget(
            name: "OpenDesktopMacTests",
            dependencies: ["ODShared", "open-desktop-display-viewer"],
            path: "macos/Tests"
        ),
        .testTarget(
            name: "OpenDesktopVMHostTests",
            dependencies: ["ODShared", "ODAtomics", "ODHostABI", "od-vmhost"],
            path: "vmhost/Tests"
        ),
    ]
)
