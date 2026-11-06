import Foundation

/// Command line of the application (S11-05b, S11-06b). Known: `--vmhost PATH` (or `--vmhost=PATH`), the override of the VMHost helper for
/// smoke runs, and `--open PATH` (or `--open=PATH`), a bundle that the library opens and selects (for the smoke S11-14). The value of the form `--option VALUE` cannot
/// start with `--` (it would be the next option: `--open --vmhost X` is an error, not a bundle named `--vmhost`). Other arguments
/// (for example `-NSDocumentRevisionsDebugMode` from Xcode or `-psn_...` from older launchers) are the system's and are ignored, not rejected.
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
    /// Path given with `--open` (the last one wins), or nil.
    var openBundle: String?

    private static let knownOptions = ["--vmhost", "--open"]

    /// `arguments` are the arguments after the program name.
    init(arguments: [String]) throws {
        var values: [String: String] = [:]
        var index = 0
        while index < arguments.count {
            let argument = arguments[index]
            for option in Self.knownOptions {
                if argument == option {
                    // The next argument is the value unless it is an option itself (`--open --vmhost X`): `--opt=--x` still passes `--x`.
                    guard index + 1 < arguments.count, !arguments[index + 1].hasPrefix("--") else { throw ParseError.missingValue(option) }
                    index += 1
                    values[option] = arguments[index]
                } else if argument.hasPrefix(option + "=") {
                    values[option] = String(argument.dropFirst(option.count + 1))
                } else {
                    continue
                }
                if values[option]?.isEmpty == true { throw ParseError.emptyValue(option) }
                break
            }
            index += 1
        }
        vmhostOverride = values["--vmhost"]
        openBundle = values["--open"]
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
