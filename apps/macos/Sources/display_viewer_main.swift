import AppKit
import Darwin
import Dispatch
import Foundation

// S10-07 display viewer: entry point. Starts VMHost with the shared launch code (VMHostLaunch.swift) in the
// TEST-ONLY mode `--display-smoke pattern` (S10-04/S10-06 profile OD_DISPLAY_PROFILE_TEST_PATTERN) and shows
// the frames of FRAME_TRANSPORT_V1.md in a Metal window, or, with `--dump-frame PATH`, renders offscreen
// without any window and writes the raw frame (B, G, R, X rows, stride = width * 4).
//
// usage: display-viewer --vmhost PATH [--profile pattern] [--dump-frame PATH [--dump-size WxH] [--backing-scale S] [--dump-cursor-test X,Y,HX,HY,E[,T]]] [--pixel-exact] [--input-channel] [--timeout-ms N] [--close-after-ms N]
// The profile flag is for smoke use only; `pattern` is the only value until S10-09 adds `probe`.
//
// S10-11 presentation: by default the frame is scaled with its aspect ratio kept (letterbox/pillarbox, plain background
// margins, nearest-neighbour sampling); `--pixel-exact` draws 1 guest pixel as 1 physical pixel, centred. For the offscreen
// dump, `--dump-size WxH` is the LOGICAL window size in points (default: the frame size) and `--backing-scale S` the
// backing scale factor (default 1), so the dumped drawable is round(W*S) x round(H*S) physical pixels, as for a window.
// Without both options the dump has the frame size and is the 1:1 reference dump.
//
// S10-14 cursor: the reader accepts CURSOR notifications and the presenter draws the cursor over the frame (FRAME_TRANSPORT_V1.md 10.8).
// The profile of this smoke never writes the cursor registers, so no cursor arrives from VMHost. `--dump-cursor-test X,Y,HX,HY,E`
// (test-only, offscreen only) therefore injects a synthetic accepted cursor state into the presenter after the frame is shown:
// a fixed 4x4 BGRA8888_PREMUL image (formula in tests/system/cursor_overlay_smoke.py), hotspot (HX, HY), position (X, Y), enabled E.
// It exercises the real upload and draw path; the transport and the device are not involved. It is not used in the normal path.
//
// S10-16 `--input-channel` (window mode only): a second socketpair (descriptor 4 in VMHost, `--input-fd 4`) carries the input
// channel of protocols/control/INPUT_TRANSPORT_V1.md. The window then collects mouse, wheel and keyboard events and
// sends them to VMHost's bounded queue. There is NO delivery to the guest (GUEST_DELIVERY = 0, shown in the window subtitle).
// Without the flag no second socketpair is made and VMHost gets no `--input-fd`: the launch is exactly the S10-07 one.

private enum ViewerError: Error, CustomStringConvertible {
    case usage(String)
    case metal(String)
    case state(String)
    case vmhost(String)
    case io(String)

    var description: String {
        switch self {
        case let .usage(text):
            return "\(text)\nusage: display-viewer --vmhost PATH [--profile pattern] [--dump-frame PATH [--dump-size WxH] [--backing-scale S] [--dump-cursor-test X,Y,HX,HY,E[,T]]] [--pixel-exact] [--input-channel] [--timeout-ms N] [--close-after-ms N]"
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
    var pixelExact = false
    var inputChannel = false
    var dumpLogicalSize: (width: Int, height: Int)?
    var backingScale = 1.0
    /// Test-only synthetic cursor (see the header comment): position, hotspot, enabled.
    var cursorTest: (x: Int, y: Int, hotX: Int, hotY: Int, enabled: Bool, translucent: Bool)?

    var mode: PresentMode { pixelExact ? .pixelExact : .fit }

    static func parse(_ args: [String]) throws -> ViewerConfig {
        var values: [String: String] = [:]
        var index = 0
        var pixelExact = false
        var inputChannel = false
        while index < args.count {
            if args[index] == "--pixel-exact" {
                pixelExact = true
                index += 1
                continue
            }
            if args[index] == "--input-channel" {
                inputChannel = true
                index += 1
                continue
            }
            guard args[index].hasPrefix("--"), index + 1 < args.count else {
                throw ViewerError.usage("bad argument \(args[index])")
            }
            values[String(args[index].dropFirst(2))] = args[index + 1]
            index += 2
        }
        let known: Set<String> = [
            "vmhost", "profile", "dump-frame", "timeout-ms", "close-after-ms", "dump-size", "backing-scale",
            "dump-cursor-test",
        ]
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
        config.pixelExact = pixelExact
        config.inputChannel = inputChannel
        if inputChannel, config.dumpPath != nil {
            throw ViewerError.usage("--input-channel needs the window mode (no --dump-frame)")
        }
        if values["dump-size"] != nil || values["backing-scale"] != nil {
            guard config.dumpPath != nil else {
                throw ViewerError.usage("--dump-size and --backing-scale need --dump-frame")
            }
        }
        if let text = values["dump-size"] {
            let parts = text.split(separator: "x", omittingEmptySubsequences: false)
            guard parts.count == 2, let width = Int(parts[0]), let height = Int(parts[1]),
                  width > 0, height > 0, width <= 16384, height <= 16384 else {
                throw ViewerError.usage("bad --dump-size \(text) (WxH, 1...16384)")
            }
            config.dumpLogicalSize = (width, height)
        }
        if let text = values["dump-cursor-test"] {
            guard config.dumpPath != nil else { throw ViewerError.usage("--dump-cursor-test needs --dump-frame") }
            let parts = text.split(separator: ",", omittingEmptySubsequences: false).map { Int($0) }
            guard parts.count == 5 || parts.count == 6, let x = parts[0], let y = parts[1], let hotX = parts[2],
                  let hotY = parts[3], let enabled = parts[4], (0...4095).contains(x), (0...2303).contains(y),
                  (0..<4).contains(hotX), (0..<4).contains(hotY), enabled == 0 || enabled == 1,
                  parts.count == 5 || parts[5] == 0 || parts[5] == 1 else {
                throw ViewerError.usage("bad --dump-cursor-test \(text) (X,Y,HX,HY,E[,T] with HX, HY in 0...3, E and T 0 or 1)")
            }
            config.cursorTest = (x, y, hotX, hotY, enabled == 1, parts.count == 6 && parts[5] == 1)
        }
        if let text = values["backing-scale"] {
            guard let value = Double(text), value >= 1, value <= 4 else {
                throw ViewerError.usage("bad --backing-scale \(text) (1...4)")
            }
            config.backingScale = value
        }
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
    /// S10-16: the UI end of the input socketpair; nil without `--input-channel`.
    let input: InputChannelUI?
    private let pid: pid_t

    init(config: ViewerConfig, presenter: MetalPresenter) throws {
        self.presenter = presenter
        let sockets = try VMHostLaunch.makeSocketPair()
        var inputSockets: (parent: Int32, child: Int32)?
        if config.inputChannel {
            do {
                inputSockets = try VMHostLaunch.makeSocketPair()
            } catch {
                Darwin.close(sockets.parent)
                Darwin.close(sockets.child)
                throw error
            }
        }
        let child: pid_t
        do {
            child = try VMHostLaunch.spawnHelper(
                path: config.vmhost,
                extraArguments: ["--display-smoke", config.profile],
                parentFD: sockets.parent, childFD: sockets.child, input: inputSockets)
        } catch {
            Darwin.close(sockets.parent)
            Darwin.close(sockets.child)
            if let inputSockets {
                Darwin.close(inputSockets.parent)
                Darwin.close(inputSockets.child)
            }
            throw error
        }
        pid = child
        Darwin.close(sockets.child) // the UI closes its copy of the VMHost end after spawn
        if let inputSockets {
            Darwin.close(inputSockets.child)
            input = try InputChannelUI(fd: inputSockets.parent)
        } else {
            input = nil
        }
        reader = FrameReader(fd: sockets.parent)
        reader.start(announceTimeoutMs: config.timeoutMs)
    }

    /// One reader step: pump, then commit or discard the staged texture, and clear it when no frame is current.
    @discardableResult
    func step(timeoutMs: Int32) -> Bool {
        let acceptedBefore = reader.acceptedCount
        let cursorAcceptedBefore = reader.cursorAcceptedCount
        var uploaded = false
        var cursorUploaded = false
        let changed = reader.pump(
            timeoutMs: timeoutMs,
            copy: { [presenter] pixels, frame in
                uploaded = true
                presenter.upload(pixels: pixels, frame: frame)
            },
            cursorCopy: { [presenter] pixels, cursor in
                cursorUploaded = true
                return presenter.uploadCursor(pixels: pixels, cursor: cursor)
            })
        if reader.acceptedCount != acceptedBefore {
            presenter.commit()
        } else if uploaded {
            presenter.discard()
        }
        // The cursor is independent of the frame: an accepted update commits the cursor state (and a new texture if the
        // shape changed); a discarded one drops only the staged texture. The reader state does not clear the cursor state
        // (10.6): the presenter draws it only over a shown frame.
        if reader.cursorAcceptedCount != cursorAcceptedBefore {
            presenter.commitCursor(reader.cursor)
        } else if cursorUploaded {
            presenter.discardCursor()
        }
        if reader.state != .showing { presenter.clear() }
        if reader.state.isTerminal { presenter.commitCursor(nil) }
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

/// Test-only image of `--dump-cursor-test`: 4x4 `BGRA8888_PREMUL`, pixel (i, j) = column i, row j. If (i + j) % 4 == 0 it is
/// transparent (0, 0, 0, 0); otherwise B = 40 + 50 i, G = 40 + 50 j, R = 255 - 40 (i + j), A = 255. Only A in {0, 255}.
/// With `translucent` (T = 1) the image has 0 < A < 255 everywhere: A = 64 * (1 + (i + j) % 3), B = A (i + 1) / 4,
/// G = A (j + 1) / 4, R = A ((i + j) % 4 + 1) / 4 (integer division; every colour channel <= A, premultiplied).
private func testCursorImage(translucent: Bool = false) -> [UInt8] {
    var bytes = [UInt8]()
    for j in 0..<4 {
        for i in 0..<4 {
            if translucent {
                let a = 64 * (1 + (i + j) % 3)
                bytes += [UInt8(a * (i + 1) / 4), UInt8(a * (j + 1) / 4), UInt8(a * ((i + j) % 4 + 1) / 4), UInt8(a)]
            } else if (i + j) % 4 == 0 {
                bytes += [0, 0, 0, 0]
            } else {
                bytes += [UInt8(40 + 50 * i), UInt8(40 + 50 * j), UInt8(255 - 40 * (i + j)), 255]
            }
        }
    }
    return bytes
}

/// Injects the synthetic accepted cursor through the presenter's real upload/commit path (no transport, no device).
private func injectTestCursor(
    _ test: (x: Int, y: Int, hotX: Int, hotY: Int, enabled: Bool, translucent: Bool), into presenter: MetalPresenter
) {
    let cursor = AcceptedCursor(
        cursorSeq: 1, enabled: test.enabled, shapeSeq: 1, width: 4, height: 4, hotX: test.hotX, hotY: test.hotY,
        posX: test.x, posY: test.y)
    _ = testCursorImage(translucent: test.translucent).withUnsafeBytes { presenter.uploadCursor(pixels: $0.baseAddress!, cursor: cursor) }
    presenter.commitCursor(cursor)
}

private func describe(_ placement: CursorPlacement?) -> String {
    guard let p = placement else { return "none" }
    return "rect=\(p.x0),\(p.y0),\(p.x1),\(p.y1) uv=\(p.u0),\(p.v0),\(p.du),\(p.dv)"
}

private func runOffscreen(config: ViewerConfig, dumpPath: String) throws {
    let presenter = try MetalPresenter()
    let session = try ViewerSession(config: config, presenter: presenter)
    let deadline = DispatchTime.now().uptimeNanoseconds + config.timeoutMs * 1_000_000
    while session.reader.state != .showing && !session.reader.state.isTerminal {
        if DispatchTime.now().uptimeNanoseconds >= deadline { break }
        session.step(timeoutMs: 50)
    }
    let finalState = session.reader.state
    var rendered: (width: Int, height: Int, pixels: [UInt8], viewport: Viewport, cursor: CursorPlacement?)?
    var logical = (width: 0, height: 0)
    if finalState == .showing, let frame = presenter.shownSize {
        logical = config.dumpLogicalSize ?? (frame.width, frame.height)
        let physical = ViewportLayout.drawableSize(
            logicalWidth: Double(logical.width), logicalHeight: Double(logical.height), scale: config.backingScale)
        // 16384 is the Metal 2D texture limit used by the option checks; a scaled size above it is a usage error.
        guard physical.width <= 16384, physical.height <= 16384 else {
            _ = session.shutdown()
            throw ViewerError.usage("drawable \(physical.width)x\(physical.height) exceeds 16384")
        }
        if let test = config.cursorTest { injectTestCursor(test, into: presenter) }
        rendered = presenter.renderOffscreen(targetWidth: physical.width, targetHeight: physical.height, mode: config.mode)
    }
    let frameSize = presenter.shownSize
    let generation = session.reader.lastFrame?.generation ?? 0
    let status = session.shutdown()
    guard finalState == .showing else {
        throw ViewerError.state("no frame before the end: \(describe(finalState)) (VMHost exit status \(status))")
    }
    guard let rendered, let frameSize else { throw ViewerError.metal("offscreen render failed") }
    guard FileManager.default.createFile(atPath: dumpPath, contents: Data(rendered.pixels)) else {
        throw ViewerError.io("cannot write \(dumpPath)")
    }
    guard status == 0 else { throw ViewerError.vmhost("exit status \(status) after EOF") }
    // width/height are the guest frame; logical (points) and physical (pixels) are the window and drawable sizes.
    print("dump-frame ok: state=SHOWING width=\(frameSize.width) height=\(frameSize.height) "
        + "bytes=\(rendered.pixels.count) generation=\(generation) mode=\(config.pixelExact ? "pixel-exact" : "fit") "
        + "logical=\(logical.width)x\(logical.height) physical=\(rendered.width)x\(rendered.height) "
        + "scale=\(config.backingScale) viewport=\(rendered.viewport) "
        + "visible=\(rendered.viewport.clipped(targetWidth: rendered.width, targetHeight: rendered.height)) "
        + "cursor=\(describe(rendered.cursor)) path=\(dumpPath)")
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
                contentSize: NSSize(width: 640, height: 480), mode: config.mode, input: session.input)
            displayWindow = window
            window.show()
            session.input?.start()
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
        var publishedShown: ShownFrame?
        while !isStopRequested() {
            let changed = session.step(timeoutMs: 100)
            let state = session.reader.state
            if state.isTerminal, statusDetail == nil {
                // section 7.2: munmap and close are done by the reader; stop and reap VMHost, keep its real status.
                let status = session.stopChild()
                statusDetail = "\(describe(state)) (VMHost exit status \(status))"
                setFailed()
            }
            // S10-16: the shown frame for absolute mouse positions: only while SHOWING, with the mode count of that frame.
            var shown: ShownFrame?
            if state == .showing, let frame = session.reader.lastFrame {
                shown = ShownFrame(width: frame.width, height: frame.height, modeChangeCount: frame.modeChangeCount)
            }
            if changed || published == nil || shown != publishedShown {
                published = state
                publishedShown = shown
                let detail = statusDetail
                DispatchQueue.main.async { [weak self] in
                    self?.displayWindow?.update(state: state, detail: detail, shown: shown)
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
        if let window = displayWindow {
            print("window geometry: \(window.geometryDescription)")
            if let text = window.inputStatusText { print("input channel: \(text)") }
            window.tearDownInput() // closes the input channel: VMHost sees EOF on it
        }
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
