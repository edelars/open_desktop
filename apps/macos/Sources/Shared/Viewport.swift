import Foundation

// S10-11: pure geometry of the display viewer. No AppKit, no Metal: the same integer functions are used by the
// window path, the offscreen dump and the unit test (apps/macos/Tests/ViewportTests.swift).
//
// Coordinates: pixels of the drawable, origin at the top-left corner, y down (Metal framebuffer convention).
// "Logical" size is the view bounds in points; "physical" size is the drawable in pixels:
// physical = round(logical * backingScaleFactor).

/// Rectangle of the guest frame inside the drawable. `x` and `y` may be negative in pixel-exact mode when the
/// drawable is smaller than the frame (the frame is then cropped, never scaled).
public struct Viewport: Equatable, CustomStringConvertible {
    public var x: Int
    public var y: Int
    public var width: Int
    public var height: Int

    public var isEmpty: Bool { width <= 0 || height <= 0 }

    /// Intersection with the drawable rectangle (0, 0, targetWidth, targetHeight); empty (all zero) when they do not overlap.
    public func clipped(targetWidth: Int, targetHeight: Int) -> Viewport {
        let left = max(x, 0)
        let top = max(y, 0)
        let right = min(x + width, targetWidth)
        let bottom = min(y + height, targetHeight)
        guard right > left, bottom > top else { return Viewport(x: 0, y: 0, width: 0, height: 0) }
        return Viewport(x: left, y: top, width: right - left, height: bottom - top)
    }

    public var description: String { "\(x),\(y),\(width),\(height)" }
}

public enum PresentMode: Equatable {
    /// Largest integer-sized rectangle with the frame's aspect ratio that fits the drawable, centred; the rest is background.
    case fit
    /// One guest pixel is one drawable pixel, centred; the rest is background (or the frame is cropped).
    case pixelExact
}

public enum ViewportLayout {
    /// Physical drawable size of a view with the given logical size (points) and backing scale factor.
    public static func drawableSize(logicalWidth: Double, logicalHeight: Double, scale: Double) -> (width: Int, height: Int) {
        (max(1, Int((logicalWidth * scale).rounded())), max(1, Int((logicalHeight * scale).rounded())))
    }

    public static func viewport(
        mode: PresentMode, frameWidth: Int, frameHeight: Int, targetWidth: Int, targetHeight: Int
    ) -> Viewport {
        guard frameWidth > 0, frameHeight > 0, targetWidth > 0, targetHeight > 0 else {
            return Viewport(x: 0, y: 0, width: 0, height: 0)
        }
        switch mode {
        case .pixelExact:
            // Floor division: with an odd positive difference the extra pixel goes to the right/bottom margin; with an odd negative one
            // (frame larger than the drawable) the extra pixel is cropped on the left/top.
            return Viewport(
                x: floorHalf(targetWidth - frameWidth), y: floorHalf(targetHeight - frameHeight),
                width: frameWidth, height: frameHeight)
        case .fit:
            var width = targetWidth
            var height = targetHeight
            // Compare aspect ratios without division: target is relatively narrower (or equal) -> fit the width.
            if targetWidth * frameHeight <= targetHeight * frameWidth {
                height = min(targetHeight, max(1, roundedDivide(frameHeight * targetWidth, frameWidth)))
            } else {
                width = min(targetWidth, max(1, roundedDivide(frameWidth * targetHeight, frameHeight)))
            }
            return Viewport(x: (targetWidth - width) / 2, y: (targetHeight - height) / 2, width: width, height: height)
        }
    }

    /// The part of the frame texture that `visible` (the viewport clipped to the drawable) shows, as texture coordinates:
    /// u = u0 + p.x * du, v = v0 + p.y * dv for p in 0...1 across `visible`. `full` is the unclipped viewport.
    /// For a viewport inside the drawable this is (0, 0, 1, 1); a pixel-exact frame cropped by a small drawable shows only
    /// the centre part, so the centre of the frame (not its corner) stays on screen.
    public static func visibleUV(full: Viewport, visible: Viewport) -> (u0: Double, v0: Double, du: Double, dv: Double) {
        (
            Double(visible.x - full.x) / Double(full.width), Double(visible.y - full.y) / Double(full.height),
            Double(visible.width) / Double(full.width), Double(visible.height) / Double(full.height)
        )
    }

    /// Nearest guest pixel under a drawable pixel (the transform S10-16 uses for the mouse), or nil outside the frame
    /// rectangle. `pixelX/pixelY` are drawable pixel coordinates (top-left origin); fractional values are allowed.
    /// The result is floor((p - origin) * frameSize / viewportSize), so the distance of the guest pixel centre from the
    /// real-valued ideal is at most half a guest pixel. Exactly on a texel boundary the GPU may pick the neighbouring
    /// texel because of float rounding (docs/testing/DISPLAY.md section 3); the result of this function is then the upper one.
    static func guestPixel(
        pixelX: Double, pixelY: Double, viewport: Viewport, frameWidth: Int, frameHeight: Int
    ) -> (x: Int, y: Int)? {
        guard !viewport.isEmpty, frameWidth > 0, frameHeight > 0 else { return nil }
        let u = (pixelX - Double(viewport.x)) / Double(viewport.width)
        let v = (pixelY - Double(viewport.y)) / Double(viewport.height)
        guard u >= 0, u < 1, v >= 0, v < 1 else { return nil }
        return (min(frameWidth - 1, Int(u * Double(frameWidth))), min(frameHeight - 1, Int(v * Double(frameHeight))))
    }

    private static func floorHalf(_ value: Int) -> Int {
        Int((Double(value) / 2).rounded(.down))
    }

    /// Round half up for positive operands.
    private static func roundedDivide(_ numerator: Int, _ denominator: Int) -> Int {
        (numerator * 2 + denominator) / (denominator * 2)
    }
}

// S10-14: placement of the cursor image over the frame (FRAME_TRANSPORT_V1.md 10.8). Pure geometry like ViewportLayout:
// the window, the offscreen dump and the unit test use the same function.

/// Where the cursor is drawn: `rect` in drawable pixels (real-valued edges, always inside the visible part of the drawable)
/// and the part of the cursor texture it shows, as texture coordinates: u = u0 + p.x * du, v = v0 + p.y * dv for p in 0...1
/// across `rect`.
public struct CursorPlacement: Equatable {
    public var x0: Double
    public var y0: Double
    public var x1: Double
    public var y1: Double
    public var u0: Double
    public var v0: Double
    public var du: Double
    public var dv: Double
}

public enum CursorOverlay {
    /// Placement of a `width` x `height` cursor image whose hotspot `(hotX, hotY)` is at frame pixel `(posX, posY)`, over a
    /// `frameWidth` x `frameHeight` frame drawn into `full` (the unclipped viewport, possibly with a negative origin) and
    /// `visible` (`full` clipped to the render target). Returns nil when nothing is drawn: the image lies outside the frame
    /// (step 2) or outside the visible part of the drawable (step 4).
    ///
    /// 1. image rectangle in frame pixels `[gx0, gx1) x [gy0, gy1)` (signed, may stick out of the frame);
    /// 2. clipped to the frame `[cx0, cx1) x [cy0, cy1)`;
    /// 3. drawable pixels with the affine map of the frame, `X(g) = full.x + g * full.width / frameWidth`, and the texture
    ///    coordinates of the clipped part;
    /// 4. intersected with `visible`; the texture coordinates are narrowed like `ViewportLayout.visibleUV` does for the frame.
    public static func placement(
        frameWidth: Int, frameHeight: Int, posX: Int, posY: Int, hotX: Int, hotY: Int, width: Int, height: Int,
        full: Viewport, visible: Viewport
    ) -> CursorPlacement? {
        guard frameWidth > 0, frameHeight > 0, width > 0, height > 0, !full.isEmpty, !visible.isEmpty else { return nil }
        let gx0 = posX - hotX
        let gy0 = posY - hotY
        let gx1 = gx0 + width
        let gy1 = gy0 + height
        let cx0 = max(gx0, 0)
        let cy0 = max(gy0, 0)
        let cx1 = min(gx1, frameWidth)
        let cy1 = min(gy1, frameHeight)
        guard cx1 > cx0, cy1 > cy0 else { return nil }

        func mapX(_ g: Int) -> Double { Double(full.x) + Double(g) * Double(full.width) / Double(frameWidth) }
        func mapY(_ g: Int) -> Double { Double(full.y) + Double(g) * Double(full.height) / Double(frameHeight) }
        let rx0 = mapX(cx0)
        let rx1 = mapX(cx1)
        let ry0 = mapY(cy0)
        let ry1 = mapY(cy1)
        let u0 = Double(cx0 - gx0) / Double(width)
        let du = Double(cx1 - cx0) / Double(width)
        let v0 = Double(cy0 - gy0) / Double(height)
        let dv = Double(cy1 - cy0) / Double(height)

        let x0 = max(rx0, Double(visible.x))
        let y0 = max(ry0, Double(visible.y))
        let x1 = min(rx1, Double(visible.x + visible.width))
        let y1 = min(ry1, Double(visible.y + visible.height))
        guard x1 > x0, y1 > y0 else { return nil }
        return CursorPlacement(
            x0: x0, y0: y0, x1: x1, y1: y1,
            u0: u0 + (x0 - rx0) / (rx1 - rx0) * du, v0: v0 + (y0 - ry0) / (ry1 - ry0) * dv,
            du: du * (x1 - x0) / (rx1 - rx0), dv: dv * (y1 - y0) / (ry1 - ry0))
    }
}
