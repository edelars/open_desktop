import AppKit
import Metal
import QuartzCore

// AppKit window of the S10-07 display viewer: an NSView backed by a CAMetalLayer. Every ready frame is
// shown through MetalPresenter; "no signal", "mode change", "disconnected" and "incompatible" are shown
// as a distinct colour and a text label, never as a frozen last frame (FRAME_TRANSPORT_V1.md 7.2).
// S10-11: the drawable is always the view's physical size (bounds x backingScaleFactor); the frame is drawn into a
// viewport inside it (aspect-preserving letterbox, or 1 guest pixel = 1 physical pixel with `pixelExact`), the margins
// are the plain background colour. A resize or a change of scale factor only redraws with a new drawable size and
// viewport: the shown texture is neither recreated nor dropped. Call everything on the main thread.
// S10-16: with an `InputChannelUI` the window also collects mouse, wheel and keyboard events (INPUT_TRANSPORT_V1.md sections 5 and 6),
// translates them (point -> physical pixel -> guest pixel through ViewportLayout, kVK_* -> HID usage) and hands them to the
// channel; focus changes become FOCUS GAINED/LOST. Without a channel nothing is collected and the window behaves as before.

final class DisplayView: NSView {
    private let metalLayer = CAMetalLayer()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func makeBackingLayer() -> CALayer { metalLayer }

    var layerForPresentation: CAMetalLayer { metalLayer }

    /// Called on every layout pass (window resize, live resize) and when the backing scale factor changes.
    var onGeometryChange: (() -> Void)?

    /// S10-16: every mouse, drag and wheel event of the view content (the title bar and the window frame are not forwarded).
    var onMouseEvent: ((NSEvent) -> Void)?

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(
            rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) { onMouseEvent?(event) }
    override func mouseDown(with event: NSEvent) { onMouseEvent?(event) }
    override func mouseUp(with event: NSEvent) { onMouseEvent?(event) }
    override func mouseDragged(with event: NSEvent) { onMouseEvent?(event) }
    override func rightMouseDown(with event: NSEvent) { onMouseEvent?(event) }
    override func rightMouseUp(with event: NSEvent) { onMouseEvent?(event) }
    override func rightMouseDragged(with event: NSEvent) { onMouseEvent?(event) }
    override func otherMouseDown(with event: NSEvent) { onMouseEvent?(event) }
    override func otherMouseUp(with event: NSEvent) { onMouseEvent?(event) }
    override func otherMouseDragged(with event: NSEvent) { onMouseEvent?(event) }
    override func scrollWheel(with event: NSEvent) { onMouseEvent?(event) }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        metalLayer.contentsScale = window?.backingScaleFactor ?? 1
        onGeometryChange?()
    }

    override func layout() {
        super.layout()
        onGeometryChange?()
    }
}

final class DisplayWindow {
    let window: NSWindow
    private let view: DisplayView
    private let label: NSTextField
    private let presenter: MetalPresenter
    private let mode: PresentMode
    private var lastState: FrameReaderState = .connecting
    private var lastDetail: String?
    private let input: InputChannelUI?
    /// The frame that is shown, as published with the last reader state (S10-16, section 5.3 step 6).
    private var shownInfo: ShownFrame?
    private var keyMonitor: Any?
    private var focusObservers: [NSObjectProtocol] = []

    init(
        presenter: MetalPresenter, title: String, contentSize: NSSize, mode: PresentMode, input: InputChannelUI? = nil
    ) {
        self.presenter = presenter
        self.mode = mode
        self.input = input
        view = DisplayView(frame: NSRect(origin: .zero, size: contentSize))
        let layer = view.layerForPresentation
        layer.device = presenter.device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.isOpaque = true
        layer.magnificationFilter = .nearest
        layer.minificationFilter = .nearest

        label = NSTextField(labelWithString: "No signal")
        label.textColor = .white
        label.font = .systemFont(ofSize: 22, weight: .medium)
        label.alignment = .center
        label.maximumNumberOfLines = 0
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            label.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 12),
            label.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -12),
        ])

        window = NSWindow(
            contentRect: NSRect(origin: .zero, size: contentSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = title
        window.contentView = view
        window.isReleasedWhenClosed = false
        window.center()
        refresh(state: .connecting, detail: nil)
        view.onGeometryChange = { [weak self] in self?.redraw() }
        setUpInput()
    }

    deinit { tearDownInput() }

    /// State of the input channel for reports (nil without `--input-channel`).
    var inputStatusText: String? { input?.statusText }

    /// Logical size (view bounds, points) and physical size (drawable, pixels) for reports.
    var geometryDescription: String {
        let scale = window.backingScaleFactor
        let drawable = view.layerForPresentation.drawableSize
        return "logical=\(Int(view.bounds.width))x\(Int(view.bounds.height)) "
            + "physical=\(Int(drawable.width))x\(Int(drawable.height)) scale=\(scale)"
    }

    /// Redraws the current state after a resize or scale change; the last frame stays in the presenter.
    private func redraw() {
        refresh(state: lastState, detail: lastDetail)
    }

    func show() {
        window.acceptsMouseMovedEvents = true
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
    }

    /// Reader state together with the frame that is shown (nil when no frame is current). `shown` feeds the absolute
    /// position of mouse events (INPUT_TRANSPORT_V1.md 5.3 step 6).
    func update(state: FrameReaderState, detail: String?, shown: ShownFrame?) {
        shownInfo = shown
        refresh(state: state, detail: detail)
    }

    /// Redraws for the reader state. `showing` draws the shown texture; every other state draws a colour and text.
    func refresh(state: FrameReaderState, detail: String?) {
        lastState = state
        lastDetail = detail
        let layer = view.layerForPresentation
        let background: MTLClearColor
        var text: String?
        switch state {
        case .showing:
            background = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
        case .connecting, .noSignal:
            background = MTLClearColor(red: 0.12, green: 0.12, blue: 0.14, alpha: 1)
            text = "No signal"
        case .modeChanging:
            background = MTLClearColor(red: 0.08, green: 0.16, blue: 0.40, alpha: 1)
            text = "Changing mode"
        case .disabled:
            // Screen 1 disabled by the guest (DISPLAY_DEVICE.md 16.6): no frame, no stale picture.
            background = MTLClearColor(red: 0.10, green: 0.10, blue: 0.10, alpha: 1)
            text = "Display disabled"
        case let .disconnected(reason):
            background = MTLClearColor(red: 0.40, green: 0.08, blue: 0.08, alpha: 1)
            text = "No signal: VMHost disconnected\n\(detail ?? reason)"
        case let .incompatible(reason):
            background = MTLClearColor(red: 0.45, green: 0.28, blue: 0.04, alpha: 1)
            text = "Incompatible version\n\(detail ?? reason)"
        }
        label.isHidden = text == nil
        if let text { label.stringValue = text }

        let scale = window.backingScaleFactor
        let size = ViewportLayout.drawableSize(
            logicalWidth: Double(view.bounds.width), logicalHeight: Double(view.bounds.height), scale: Double(scale))
        layer.drawableSize = CGSize(width: size.width, height: size.height)
        guard let drawable = layer.nextDrawable() else { return }
        presenter.render(to: drawable, background: background, mode: mode)
    }
}

// MARK: Input (S10-16)

extension DisplayWindow {
    fileprivate func setUpInput() {
        guard let input else { return }
        view.onMouseEvent = { [weak self] event in self?.handleMouse(event) }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
            self?.handleKey(event) ?? event
        }
        input.onChange = { [weak self] in self?.updateInputText() }
        // Section 6.4: GAINED on windowDidBecomeKey; LOST on resign key, resign active, minimise and close.
        let center = NotificationCenter.default
        let gained: [(Notification.Name, Bool)] = [
            (NSWindow.didBecomeKeyNotification, true), (NSWindow.didResignKeyNotification, false),
            (NSWindow.didMiniaturizeNotification, false), (NSWindow.willCloseNotification, false),
        ]
        for (name, value) in gained {
            focusObservers.append(center.addObserver(forName: name, object: window, queue: .main) { [weak input] _ in
                input?.windowFocus(gained: value)
            })
        }
        focusObservers.append(center.addObserver(
            forName: NSApplication.didResignActiveNotification, object: nil, queue: .main
        ) { [weak input] _ in input?.windowFocus(gained: false) })
        updateInputText()
    }

    /// Removes the monitor and the observers and closes the channel (VMHost sees EOF).
    func tearDownInput() {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        focusObservers.forEach { NotificationCenter.default.removeObserver($0) }
        focusObservers = []
        input?.shutdown()
    }

    fileprivate func updateInputText() {
        guard let input else { return }
        window.subtitle = input.statusText
    }

    /// Geometry of the moment (5.3): bounds, scale and shown frame are read again for every event.
    private func currentGeometry() -> InputGeometry {
        var shown: ShownFrame?
        if lastState == .showing, let info = shownInfo, let size = presenter.shownSize,
           size.width == info.width, size.height == info.height {
            shown = info
        }
        return InputGeometry(
            boundsWidth: Double(view.bounds.width), boundsHeight: Double(view.bounds.height),
            scale: Double(window.backingScaleFactor), mode: mode, shown: shown)
    }

    /// Keyboard (6.2, 6.3): the local monitor sees `keyDown`/`keyUp` before menu key equivalents. nil swallows the event.
    fileprivate func handleKey(_ event: NSEvent) -> NSEvent? {
        guard let input, event.window === window, input.isCollecting else { return event }
        let code = event.keyCode
        switch event.type {
        case .keyDown, .keyUp:
            let down = event.type == .keyDown
            if down && event.isARepeat {
                input.noteRepeat()
                return nil
            }
            guard let usage = HIDUsage.usage(forKeyCode: code) else {
                input.noteUnmapped(keyCode: code)
                return nil
            }
            input.usage(page: InputUsagePage.keyboard, id: usage, down: down)
        case .flagsChanged:
            if code == HIDUsage.capsLockKeyCode {
                input.capsLockToggle()
            } else if let mask = HIDUsage.deviceModifierMask(forKeyCode: code), let usage = HIDUsage.usage(forKeyCode: code) {
                input.usage(page: InputUsagePage.keyboard, id: usage, down: event.modifierFlags.rawValue & mask != 0)
            } else {
                input.noteUnmapped(keyCode: code)
            }
        default:
            return event
        }
        return nil
    }

    /// Mouse and wheel (5.2 to 5.5).
    fileprivate func handleMouse(_ event: NSEvent) {
        guard let input, input.isCollecting else { return }
        let geometry = currentGeometry()
        let point = view.convert(event.locationInWindow, from: nil)
        let absolute = geometry.absolute(pointX: Double(point.x), pointY: Double(point.y))
        switch event.type {
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            let delta = input.relativeDelta(dxPoints: Double(event.deltaX), dyPoints: Double(event.deltaY), geometry: geometry)
            input.motion(dx: delta.dx, dy: delta.dy, wheel: 0, pan: 0, absolute: absolute)
        case .leftMouseDown, .rightMouseDown, .otherMouseDown:
            input.button(number: event.buttonNumber, down: true, absolute: absolute)
        case .leftMouseUp, .rightMouseUp, .otherMouseUp:
            input.button(number: event.buttonNumber, down: false, absolute: absolute)
        case .scrollWheel:
            let units = input.scrollUnits(
                deltaX: Double(event.scrollingDeltaX), deltaY: Double(event.scrollingDeltaY),
                precise: event.hasPreciseScrollingDeltas, invertedFromDevice: event.isDirectionInvertedFromDevice)
            input.motion(dx: 0, dy: 0, wheel: units.wheel, pan: units.pan, absolute: absolute)
        default:
            break
        }
    }
}
