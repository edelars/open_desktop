import Foundation

// S10-11: pure geometry of the display viewer. No AppKit, no Metal: the same integer functions are used by the
// window path, the offscreen dump and the unit test (apps/macos/Tests/ViewportTests.swift).
//
// Coordinates: pixels of the drawable, origin at the top-left corner, y down (Metal framebuffer convention).
// "Logical" size is the view bounds in points; "physical" size is the drawable in pixels:
// physical = round(logical * backingScaleFactor).

/// Rectangle of the guest frame inside the drawable. `x` and `y` may be negative in pixel-exact mode when the
/// drawable is smaller than the frame (the frame is then cropped, never scaled).
struct Viewport: Equatable, CustomStringConvertible {
    var x: Int
    var y: Int
    var width: Int
    var height: Int

    var isEmpty: Bool { width <= 0 || height <= 0 }

    /// Intersection with the drawable rectangle (0, 0, targetWidth, targetHeight); empty (all zero) when they do not overlap.
    func clipped(targetWidth: Int, targetHeight: Int) -> Viewport {
        let left = max(x, 0)
        let top = max(y, 0)
        let right = min(x + width, targetWidth)
        let bottom = min(y + height, targetHeight)
        guard right > left, bottom > top else { return Viewport(x: 0, y: 0, width: 0, height: 0) }
        return Viewport(x: left, y: top, width: right - left, height: bottom - top)
    }

    var description: String { "\(x),\(y),\(width),\(height)" }
}

enum PresentMode: Equatable {
    /// Largest integer-sized rectangle with the frame's aspect ratio that fits the drawable, centred; the rest is background.
    case fit
    /// One guest pixel is one drawable pixel, centred; the rest is background (or the frame is cropped).
    case pixelExact
}

enum ViewportLayout {
    /// Physical drawable size of a view with the given logical size (points) and backing scale factor.
    static func drawableSize(logicalWidth: Double, logicalHeight: Double, scale: Double) -> (width: Int, height: Int) {
        (max(1, Int((logicalWidth * scale).rounded())), max(1, Int((logicalHeight * scale).rounded())))
    }

    static func viewport(
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
    static func visibleUV(full: Viewport, visible: Viewport) -> (u0: Double, v0: Double, du: Double, dv: Double) {
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
