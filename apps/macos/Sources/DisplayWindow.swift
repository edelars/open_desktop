import AppKit
import Metal
import QuartzCore

// AppKit window of the S10-07 display viewer: an NSView backed by a CAMetalLayer. Every ready frame is
// shown through MetalPresenter; "no signal", "mode change", "disconnected" and "incompatible" are shown
// as a distinct colour and a text label, never as a frozen last frame (FRAME_TRANSPORT_V1.md 7.2).
// Scaling, aspect-ratio fitting and Retina handling are S10-11; here the texture covers the whole drawable,
// and the drawable has the frame size, so the compositor scales it with nearest-neighbour filtering.
// Call everything on the main thread.

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

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        metalLayer.contentsScale = window?.backingScaleFactor ?? 1
    }
}

final class DisplayWindow {
    let window: NSWindow
    private let view: DisplayView
    private let label: NSTextField
    private let presenter: MetalPresenter

    init(presenter: MetalPresenter, title: String, contentSize: NSSize) {
        self.presenter = presenter
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
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
    }

    /// Redraws for the reader state. `showing` draws the shown texture; every other state draws a colour and text.
    func refresh(state: FrameReaderState, detail: String?) {
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

        if state == .showing, let size = presenter.shownSize {
            layer.drawableSize = CGSize(width: size.width, height: size.height)
        } else {
            let scale = window.backingScaleFactor
            layer.drawableSize = CGSize(
                width: max(1, view.bounds.width * scale), height: max(1, view.bounds.height * scale))
        }
        guard let drawable = layer.nextDrawable() else { return }
        presenter.render(to: drawable, background: background)
    }
}
