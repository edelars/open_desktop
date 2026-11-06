import Foundation

/// Command line of the application (S11-05b). Only `--vmhost PATH` (or `--vmhost=PATH`) is known: the override of the
/// VMHost helper for smoke runs. Other arguments (for example `-NSDocumentRevisionsDebugMode` from Xcode or
/// `-psn_...` from older launchers) are the system's and are ignored, not rejected.
struct AppOptions: Equatable {
    enum ParseError: Error, Equatable, CustomStringConvertible {
        case missingValue(String)
        case emptyValue(String)

        var description: String {
            switch self {
            case .missingValue(let option): return "\(option) needs a value"
            case .emptyValue(let option): return "\(option) value is empty"
            }
        }
    }

    /// Path given with `--vmhost`, or nil when the bundled helper is used.
    var vmhostOverride: String?

    /// `arguments` are the arguments after the program name.
    init(arguments: [String]) throws {
        var override: String?
        var index = 0
        while index < arguments.count {
            let argument = arguments[index]
            if argument == "--vmhost" {
                guard index + 1 < arguments.count else { throw ParseError.missingValue("--vmhost") }
                index += 1
                override = arguments[index]
            } else if argument.hasPrefix("--vmhost=") {
                override = String(argument.dropFirst("--vmhost=".count))
            }
            if let value = override, value.isEmpty { throw ParseError.emptyValue("--vmhost") }
            index += 1
        }
        vmhostOverride = override
    }
}

/// Where the application looks for VMHost. The helper lives inside the application bundle
/// (`tools/build/build_app_bundle.py` puts it at `Contents/Helpers/od-vmhost`); `--vmhost` overrides it for smoke.
/// The application never links the engine: it only finds and (from S11-06 on) spawns this executable.
struct VMHostLocation: Equatable {
    static let bundledRelativePath = "Contents/Helpers/od-vmhost"

    enum Source: Equatable {
        case argument
        case bundle
    }

    let url: URL
    let source: Source
    /// True when the file exists and is executable. The location is reported even when it is not.
    let isExecutable: Bool

    static func locate(
        options: AppOptions,
        bundleURL: URL,
        isExecutableFile: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> VMHostLocation {
        if let override = options.vmhostOverride {
            let url = URL(fileURLWithPath: override)
            return VMHostLocation(url: url, source: .argument, isExecutable: isExecutableFile(url.path))
        }
        let url = bundleURL.appendingPathComponent(bundledRelativePath)
        return VMHostLocation(url: url, source: .bundle, isExecutable: isExecutableFile(url.path))
    }
}
