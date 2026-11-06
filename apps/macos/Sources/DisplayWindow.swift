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

    init(presenter: MetalPresenter, title: String, contentSize: NSSize, mode: PresentMode) {
        self.presenter = presenter
        self.mode = mode
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
    }

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
        window.makeKeyAndOrderFront(nil)
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
