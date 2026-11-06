import AppKit
import Darwin
import Dispatch
import Foundation

// S10-07 display viewer: entry point. Starts VMHost with the shared launch code (VMHostLaunch.swift) in the
// TEST-ONLY mode `--display-smoke pattern` (S10-04/S10-06 profile OD_DISPLAY_PROFILE_TEST_PATTERN) and shows
// the frames of FRAME_TRANSPORT_V1.md in a Metal window, or, with `--dump-frame PATH`, renders offscreen
// without any window and writes the raw frame (B, G, R, X rows, stride = width * 4).
//
// usage: display-viewer --vmhost PATH [--profile pattern] [--dump-frame PATH] [--timeout-ms N] [--close-after-ms N]
// The profile flag is for smoke use only; `pattern` is the only value until S10-09 adds `probe`.

private enum ViewerError: Error, CustomStringConvertible {
    case usage(String)
    case metal(String)
    case state(String)
    case vmhost(String)
    case io(String)

    var description: String {
        switch self {
        case let .usage(text):
            return "\(text)\nusage: display-viewer --vmhost PATH [--profile pattern] [--dump-frame PATH] [--timeout-ms N] [--close-after-ms N]"
        case let .metal(text): return "Metal: \(text)"
        case let .state(text): return "viewer state: \(text)"
        case let .vmhost(text): return "VMHost: \(text)"
        case let .io(text): return "I/O: \(text)"
        }
    }
}

private struct ViewerConfig {
    var vmhost: String
    var profile = "pattern"
    var dumpPath: String?
    var timeoutMs: UInt64 = 10_000
    var closeAfterMs: UInt64?

    static func parse(_ args: [String]) throws -> ViewerConfig {
        var values: [String: String] = [:]
        var index = 0
        while index < args.count {
            guard args[index].hasPrefix("--"), index + 1 < args.count else {
                throw ViewerError.usage("bad argument \(args[index])")
            }
            values[String(args[index].dropFirst(2))] = args[index + 1]
            index += 2
        }
        let known: Set<String> = ["vmhost", "profile", "dump-frame", "timeout-ms", "close-after-ms"]
        if let unknown = values.keys.first(where: { !known.contains($0) }) {
            throw ViewerError.usage("unknown option --\(unknown)")
        }
        guard let vmhost = values["vmhost"] else { throw ViewerError.usage("--vmhost is required") }
        var config = ViewerConfig(vmhost: vmhost)
        if let profile = values["profile"] {
            guard profile == "pattern" else {
                throw ViewerError.usage("unsupported --profile \(profile) (only `pattern`; `probe` arrives in S10-09)")
            }
            config.profile = profile
        }
        config.dumpPath = values["dump-frame"]
        if let text = values["timeout-ms"] {
            guard let value = UInt64(text), value > 0 else { throw ViewerError.usage("bad --timeout-ms") }
            config.timeoutMs = value
        }
        if let text = values["close-after-ms"] {
            guard let value = UInt64(text) else { throw ViewerError.usage("bad --close-after-ms") }
            config.closeAfterMs = value
        }
        return config
    }
}

/// A running VMHost with the UI end of its socketpair wrapped in a FrameReader.
private final class ViewerSession {
    let reader: FrameReader
    let presenter: MetalPresenter
    private let pid: pid_t

    init(config: ViewerConfig, presenter: MetalPresenter) throws {
        self.presenter = presenter
        let sockets = try VMHostLaunch.makeSocketPair()
        let child: pid_t
        do {
            child = try VMHostLaunch.spawnHelper(
                path: config.vmhost,
                extraArguments: ["--display-smoke", config.profile],
                parentFD: sockets.parent, childFD: sockets.child)
        } catch {
            Darwin.close(sockets.parent)
            Darwin.close(sockets.child)
            throw error
        }
        pid = child
        Darwin.close(sockets.child) // the UI closes its copy of the VMHost end after spawn
        reader = FrameReader(fd: sockets.parent)
        reader.start(announceTimeoutMs: config.timeoutMs)
    }

    /// One reader step: pump, then commit or discard the staged texture, and clear it when no frame is current.
    @discardableResult
    func step(timeoutMs: Int32) -> Bool {
        let acceptedBefore = reader.acceptedCount
        var uploaded = false
        let changed = reader.pump(timeoutMs: timeoutMs) { [presenter] pixels, frame in
            uploaded = true
            presenter.upload(pixels: pixels, frame: frame)
        }
        if reader.acceptedCount != acceptedBefore {
            presenter.commit()
        } else if uploaded {
            presenter.discard()
        }
        if reader.state != .showing { presenter.clear() }
        return changed
    }

    private let childLock = NSLock()
    private var childStatus: Int32?

    private static func decode(_ status: Int32) -> Int32 {
        if status & 0x7F != 0 { return 128 + (status & 0x7F) }
        return (status >> 8) & 0xFF
    }

    /// Waits up to `timeoutMs` for VMHost to end and caches its real status. Caller holds `childLock`.
    private func reapLocked(timeoutMs: Int) -> Int32? {
        if let childStatus { return childStatus }
        var waitedMs = 0
        while true {
            var status: Int32 = 0
            let waited = waitpid(pid, &status, WNOHANG)
            if waited == pid {
                childStatus = ViewerSession.decode(status)
                return childStatus
            }
            if waited < 0 && errno != EINTR {
                childStatus = 255 // the child is gone and its status is unavailable
                return childStatus
            }
            if waitedMs >= timeoutMs { return nil }
            usleep(10_000)
            waitedMs += 10
        }
    }

    /// Stops VMHost and reaps it; idempotent, returns its real outcome. The socket must already be closed:
    /// EOF is the normal stop signal (up to 2 s), then SIGTERM, up to 500 ms, then SIGKILL (section 2, item 4).
    @discardableResult
    func stopChild() -> Int32 {
        childLock.lock()
        defer { childLock.unlock() }
        if let status = reapLocked(timeoutMs: 2000) { return status }
        _ = kill(pid, SIGTERM)
        if let status = reapLocked(timeoutMs: 500) { return status }
        _ = kill(pid, SIGKILL)
        var status: Int32 = 0
        while true {
            let waited = waitpid(pid, &status, 0)
            if waited == pid { childStatus = ViewerSession.decode(status); break }
            if waited < 0 && errno == EINTR { continue }
            childStatus = 255
            break
        }
        return childStatus ?? 255
    }

    /// munmap + close the socket, then stop VMHost. Returns VMHost's exit status.
    func shutdown() -> Int32 {
        reader.teardown()
        return stopChild()
    }
}

private func describe(_ state: FrameReaderState) -> String {
    switch state {
    case let .disconnected(reason), let .incompatible(reason): return "\(state.name): \(reason)"
    default: return state.name
    }
}

// MARK: Offscreen

private func runOffscreen(config: ViewerConfig, dumpPath: String) throws {
    let presenter = try MetalPresenter()
    let session = try ViewerSession(config: config, presenter: presenter)
    let deadline = DispatchTime.now().uptimeNanoseconds + config.timeoutMs * 1_000_000
    while session.reader.state != .showing && !session.reader.state.isTerminal {
        if DispatchTime.now().uptimeNanoseconds >= deadline { break }
        session.step(timeoutMs: 50)
    }
    let finalState = session.reader.state
    var rendered: (width: Int, height: Int, pixels: [UInt8])?
    if finalState == .showing { rendered = presenter.renderOffscreen() }
    let generation = session.reader.lastFrame?.generation ?? 0
    let status = session.shutdown()
    guard finalState == .showing else {
        throw ViewerError.state("no frame before the end: \(describe(finalState)) (VMHost exit status \(status))")
    }
    guard let rendered else { throw ViewerError.metal("offscreen render failed") }
    guard FileManager.default.createFile(atPath: dumpPath, contents: Data(rendered.pixels)) else {
        throw ViewerError.io("cannot write \(dumpPath)")
    }
    guard status == 0 else { throw ViewerError.vmhost("exit status \(status) after EOF") }
    print("dump-frame ok: state=SHOWING width=\(rendered.width) height=\(rendered.height) "
        + "bytes=\(rendered.pixels.count) generation=\(generation) path=\(dumpPath)")
}

// MARK: Window

private final class ViewerDelegate: NSObject, NSApplicationDelegate {
    private let config: ViewerConfig
    private var session: ViewerSession?
    private var displayWindow: DisplayWindow?
    private let stopped = DispatchSemaphore(value: 0)
    private let stopLock = NSLock()
    private var stopFlag = false
    private var exitStatus: Int32 = 0
    private var readerStarted = false

    init(config: ViewerConfig) { self.config = config }

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            let presenter = try MetalPresenter()
            let session = try ViewerSession(config: config, presenter: presenter)
            self.session = session
            let window = DisplayWindow(
                presenter: presenter, title: "Open Desktop display viewer (test pattern)",
                contentSize: NSSize(width: 640, height: 480))
            displayWindow = window
            window.show()
            NSApp.activate(ignoringOtherApps: true)
            readerStarted = true
            Thread.detachNewThread { [weak self] in self?.readerLoop(session) }
            if let delay = config.closeAfterMs {
                DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Int(delay))) { NSApp.terminate(nil) }
            }
        } catch {
            fputs("display-viewer: \(error)\n", stderr)
            _ = session?.shutdown()
            exit(1)
        }
    }

    /// Reader thread: owns the FrameReader until it ends, then reports the state to the main thread.
    private func readerLoop(_ session: ViewerSession) {
        var published: FrameReaderState?
        var statusDetail: String?
        while !isStopRequested() {
            let changed = session.step(timeoutMs: 100)
            let state = session.reader.state
            if state.isTerminal, statusDetail == nil {
                // section 7.2: munmap and close are done by the reader; stop and reap VMHost, keep its real status.
                let status = session.stopChild()
                statusDetail = "\(describe(state)) (VMHost exit status \(status))"
                setFailed()
            }
            if changed || published == nil {
                published = state
                let detail = statusDetail
                DispatchQueue.main.async { [weak self] in
                    self?.displayWindow?.refresh(state: state, detail: detail)
                }
            }
            if state.isTerminal { break }
        }
        // The reader belongs to this thread; ownership returns to the main thread through the semaphore.
        stopped.signal()
    }

    private func setFailed() {
        stopLock.lock()
        exitStatus = 1
        stopLock.unlock()
    }

    private func isStopRequested() -> Bool {
        stopLock.lock()
        defer { stopLock.unlock() }
        return stopFlag
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        stopLock.lock()
        stopFlag = true
        stopLock.unlock()
        if readerStarted { _ = stopped.wait(timeout: .now() + 8) }
        if let status = session?.shutdown(), status != 0 {
            fputs("display-viewer: VMHost exit status \(status)\n", stderr)
            setFailed()
        }
        stopLock.lock()
        let code = exitStatus
        stopLock.unlock()
        exit(code)
    }
}

@main
struct DisplayViewerMain {
    static func main() {
        let config: ViewerConfig
        do {
            config = try ViewerConfig.parse(Array(CommandLine.arguments.dropFirst()))
        } catch {
            fputs("display-viewer: \(error)\n", stderr)
            exit(2)
        }
        if let dumpPath = config.dumpPath {
            do {
                try runOffscreen(config: config, dumpPath: dumpPath)
            } catch {
                fputs("display-viewer: \(error)\n", stderr)
                exit(1)
            }
            return
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = ViewerDelegate(config: config)
        app.delegate = delegate
        app.run()
    }
}
